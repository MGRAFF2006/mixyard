//! Local, caller-driven sample clock. No remote clock or audio device is inferred.
use crate::{Offer, Spec};
use serde::Serialize;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Instant,
};

const ACTIVE: u8 = 0;
const RENDERING: u8 = 1;
const INACTIVE: u8 = 2;
const REVOKING: u8 = 3;
const REVOKED_BIT: u8 = 2;

/// A nonblocking revocation receipt. Pending is not a safe-completion claim:
/// the consumer must run again to finish scrubbing an overlapping block.
#[derive(Clone)]
pub struct Revocation {
    lifecycle: Arc<Lifecycle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationStatus {
    Active,
    Pending,
    Complete,
}

impl Revocation {
    pub fn status(&self) -> RevocationStatus {
        self.lifecycle.status()
    }
}

/// Invalidation never waits for a descheduled renderer or holds a worker lock
/// while waiting. Its single fetch_or races the renderer's block-commit CAS.
pub(crate) struct Lifecycle(AtomicU8);
impl Lifecycle {
    pub(crate) fn new() -> Self {
        Self(AtomicU8::new(ACTIVE))
    }
    fn status(&self) -> RevocationStatus {
        match self.0.load(Ordering::Acquire) {
            ACTIVE | RENDERING => RevocationStatus::Active,
            REVOKING => RevocationStatus::Pending,
            INACTIVE => RevocationStatus::Complete,
            _ => unreachable!(),
        }
    }
    pub(crate) fn active(&self) -> bool {
        self.status() == RevocationStatus::Active
    }
    pub(crate) fn invalidate(&self) {
        self.0.fetch_or(REVOKED_BIT, Ordering::AcqRel);
    }
    pub(crate) fn receipt(self: &Arc<Self>) -> Revocation {
        Revocation {
            lifecycle: Arc::clone(self),
        }
    }
}

/// Sample zero is due `target_buffer_frames / sample_rate` after this local anchor.
/// Neither value is a remote timestamp or measured network/device latency.
#[derive(Clone, Copy, Debug)]
pub struct PlayoutConfig {
    pub anchor: Instant,
    pub target_buffer_frames: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayoutError {
    UnalignedBlock,
    BlockTooLarge,
    TimeWentBackwards,
    TimelineExhausted,
}
impl std::fmt::Display for PlayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PlayoutError {}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct PlayoutCounters {
    pub packets_buffered: u64,
    pub duplicate_packets: u64,
    pub late_packets: u64,
    /// Expired prefixes of packets arriving during a partially consumed slot.
    pub late_prefix_frames: u64,
    pub beyond_window_packets: u64,
    pub audio_frames: u64,
    pub missing_frames: u64,
    pub prebuffer_frames: u64,
    pub early_frames: u64,
    /// Timeline frames deliberately skipped after the local block schedule advances.
    pub skipped_frames: u64,
    pub inactive_frames: u64,
    pub discarded_buffered_frames: u64,
    /// Invalid blocks are entirely zeroed; bytes may not form whole sample frames.
    pub rejected_blocks: u64,
    pub rejected_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
pub struct PlayoutReport {
    pub first_sample_frame: Option<u64>,
    pub audio_frames: u64,
    pub missing_frames: u64,
    pub prebuffer_frames: u64,
    pub early_frames: u64,
    pub inactive_frames: u64,
    pub skipped_frames: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct PlayoutSnapshot {
    pub stream: u64,
    pub generation: u64,
    pub active: bool,
    pub revocation: RevocationStatus,
    pub sample_rate: u32,
    pub target_buffer_frames: u64,
    pub target_buffer_ms: f64,
    pub capacity_frames: u64,
    pub capacity_payload_bytes: usize,
    pub buffered_frames: u64,
    pub buffered_ms: f64,
    pub next_sample_frame: u64,
    pub counters: PlayoutCounters,
}

/// Single-owner, preallocated packet-to-block adapter. Create/service on a worker;
/// render into caller storage. This is not hardware-clock or realtime qualification.
/// A generation owns one handle; pull and additional handles are excluded.
pub struct Playout {
    offer: Offer,
    config: PlayoutConfig,
    pub(crate) lifecycle: Arc<Lifecycle>,
    tags: Vec<Option<u64>>,
    pcm: Vec<u8>,
    frame_bytes: usize,
    packet_bytes: usize,
    capacity_frames: u64,
    pub(crate) next: u64,
    last_now: Option<Instant>,
    counters: PlayoutCounters,
}

impl Playout {
    pub(crate) fn new(
        offer: Offer,
        config: PlayoutConfig,
        packets: usize,
        lifecycle: Arc<Lifecycle>,
    ) -> anyhow::Result<Self> {
        use anyhow::ensure;
        let packet_bytes = offer.spec.payload_bytes()?;
        let capacity_frames = (packets as u64)
            .checked_mul(u64::from(offer.spec.frames_per_packet))
            .ok_or_else(|| anyhow::anyhow!("playout frame capacity overflow"))?;
        ensure!(
            config.target_buffer_frames <= capacity_frames,
            "target buffer exceeds admitted window"
        );
        let capacity_bytes = packets
            .checked_mul(packet_bytes)
            .ok_or_else(|| anyhow::anyhow!("playout byte capacity overflow"))?;
        let frame_bytes = offer.spec.codec.sample_bytes()? * offer.spec.channels.len();
        let mut pcm = Vec::new();
        pcm.try_reserve_exact(capacity_bytes)?;
        pcm.resize(capacity_bytes, 0);
        let mut tags = Vec::new();
        tags.try_reserve_exact(packets)?;
        tags.resize(packets, None);
        Ok(Self {
            offer,
            config,
            lifecycle,
            tags,
            pcm,
            frame_bytes,
            packet_bytes,
            capacity_frames,
            next: 0,
            last_now: None,
            counters: PlayoutCounters::default(),
        })
    }

    pub fn spec(&self) -> &Spec {
        &self.offer.spec
    }
    /// Obtain before moving the single-owner handle to a consumer thread.
    pub fn revocation(&self) -> Revocation {
        self.lifecycle.receipt()
    }

    pub fn generation(&self) -> u64 {
        self.offer.generation
    }

    /// Explicit local delay adjustment. Increasing causes counted overlap/prebuffer
    /// silence; decreasing causes counted skips on the next render, never replay.
    pub fn set_target_buffer_frames(&mut self, frames: u64) -> anyhow::Result<()> {
        anyhow::ensure!(
            frames <= self.capacity_frames,
            "target buffer exceeds admitted window"
        );
        self.config.target_buffer_frames = frames;
        Ok(())
    }

    pub(crate) fn insert(&mut self, sequence: u64, payload: &[u8]) {
        let next_packet = self.next / u64::from(self.offer.spec.frames_per_packet);
        if sequence < next_packet {
            self.counters.late_packets = self.counters.late_packets.saturating_add(1);
        } else if sequence - next_packet >= self.tags.len() as u64 {
            self.counters.beyond_window_packets =
                self.counters.beyond_window_packets.saturating_add(1);
        } else {
            let slot = (sequence % self.tags.len() as u64) as usize;
            if self.tags[slot] == Some(sequence) {
                self.counters.duplicate_packets = self.counters.duplicate_packets.saturating_add(1);
            } else {
                let expired_prefix = self
                    .next
                    .saturating_sub(sequence * u64::from(self.offer.spec.frames_per_packet));
                self.counters.late_prefix_frames = self
                    .counters
                    .late_prefix_frames
                    .saturating_add(expired_prefix);
                let start = slot * self.packet_bytes;
                let expired_bytes = expired_prefix as usize * self.frame_bytes;
                self.pcm[start..start + expired_bytes].fill(0);
                self.pcm[start + expired_bytes..start + self.packet_bytes]
                    .copy_from_slice(&payload[expired_bytes..]);
                self.tags[slot] = Some(sequence);
                self.counters.packets_buffered = self.counters.packets_buffered.saturating_add(1);
            }
        }
    }

    fn buffered_frames(&self) -> u64 {
        let fpp = u64::from(self.offer.spec.frames_per_packet);
        self.tags
            .iter()
            .flatten()
            .map(|seq| {
                let start = seq * fpp;
                fpp.saturating_sub(self.next.saturating_sub(start))
            })
            .sum()
    }

    pub(crate) fn clear(&mut self) {
        self.counters.discarded_buffered_frames = self
            .counters
            .discarded_buffered_frames
            .saturating_add(self.buffered_frames());
        self.tags.fill(None);
        self.pcm.fill(0);
    }

    pub fn snapshot(&self) -> PlayoutSnapshot {
        let revocation = self.lifecycle.status();
        let active = revocation == RevocationStatus::Active;
        let buffered_frames = if active { self.buffered_frames() } else { 0 };
        PlayoutSnapshot {
            stream: self.offer.spec.stream,
            generation: self.offer.generation,
            active,
            revocation,
            sample_rate: self.offer.spec.sample_rate,
            target_buffer_frames: self.config.target_buffer_frames,
            target_buffer_ms: self.config.target_buffer_frames as f64 * 1000.0
                / f64::from(self.offer.spec.sample_rate),
            capacity_frames: self.capacity_frames,
            capacity_payload_bytes: self.pcm.len(),
            buffered_frames,
            buffered_ms: buffered_frames as f64 * 1000.0 / f64::from(self.offer.spec.sample_rate),
            next_sample_frame: self.next,
            counters: self.counters,
        }
    }

    /// `now` is the LOCAL start time represented by this output block, not a
    /// timestamp of remote capture or the moment a physical device plays it.
    /// Floor elapsed local nanoseconds * exact rate to choose the starting frame.
    /// Skip/count overdue frames; zero/count prebuffer and previously consumed
    /// overlap, then copy only available, whole, ordered frames. Missing frames
    /// irrevocably advance the timeline. Calls may render at most one window.
    /// Errors zero the entire block, count rejected bytes, and do not advance.
    pub fn render_into(
        &mut self,
        now: Instant,
        output: &mut [u8],
    ) -> Result<PlayoutReport, PlayoutError> {
        self.render_inner(now, output, || {})
    }

    // The private hook permits a deterministic race test at the real commit boundary.
    fn render_inner(
        &mut self,
        now: Instant,
        output: &mut [u8],
        before_commit: impl FnOnce(),
    ) -> Result<PlayoutReport, PlayoutError> {
        output.fill(0);
        let frames = (output.len() / self.frame_bytes) as u64;
        let elapsed = if now >= self.config.anchor {
            (now.duration_since(self.config.anchor).as_nanos()
                * u128::from(self.offer.spec.sample_rate)
                / 1_000_000_000) as i128
        } else {
            -((self.config.anchor.duration_since(now).as_nanos()
                * u128::from(self.offer.spec.sample_rate))
            .div_ceil(1_000_000_000) as i128)
        };
        let desired = elapsed - i128::from(self.config.target_buffer_frames);
        let end = desired + i128::from(frames);
        let error = if !output.len().is_multiple_of(self.frame_bytes) {
            Some(PlayoutError::UnalignedBlock)
        } else if frames > self.capacity_frames {
            Some(PlayoutError::BlockTooLarge)
        } else if self.last_now.is_some_and(|previous| now < previous) {
            Some(PlayoutError::TimeWentBackwards)
        } else if end > i128::from(u64::MAX) {
            Some(PlayoutError::TimelineExhausted)
        } else {
            None
        };
        if let Some(error) = error {
            self.counters.rejected_blocks = self.counters.rejected_blocks.saturating_add(1);
            self.counters.rejected_bytes = self
                .counters
                .rejected_bytes
                .saturating_add(output.len() as u64);
            return Err(error);
        }
        self.last_now = Some(now);
        let gate = Arc::clone(&self.lifecycle);
        if gate
            .0
            .compare_exchange(ACTIVE, RENDERING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.clear();
            self.counters.inactive_frames = self.counters.inactive_frames.saturating_add(frames);
            return Ok(PlayoutReport {
                inactive_frames: frames,
                ..PlayoutReport::default()
            });
        }
        let mut report = PlayoutReport::default();
        if frames > 0 && desired > i128::from(self.next) {
            report.skipped_frames = desired as u64 - self.next;
            let fpp = u64::from(self.offer.spec.frames_per_packet);
            // Retire bounded old slots before jumping; no iteration across a long pause.
            for slot in 0..self.tags.len() {
                if let Some(seq) = self.tags[slot] {
                    let start_frame = seq * fpp;
                    let before = fpp.saturating_sub(self.next.saturating_sub(start_frame));
                    let after = fpp.saturating_sub((desired as u64).saturating_sub(start_frame));
                    self.counters.discarded_buffered_frames = self
                        .counters
                        .discarded_buffered_frames
                        .saturating_add(before - after);
                    let expired_bytes = (fpp - after) as usize * self.frame_bytes;
                    let start = slot * self.packet_bytes;
                    self.pcm[start..start + expired_bytes].fill(0);
                    if after == 0 {
                        self.tags[slot] = None;
                    }
                }
            }
            self.next = desired as u64;
        }
        for frame in 0..frames {
            let position = desired + i128::from(frame);
            if position < 0 {
                report.prebuffer_frames += 1;
                continue;
            }
            if position < i128::from(self.next) {
                report.early_frames += 1;
                continue;
            }
            let position = position as u64;
            report.first_sample_frame.get_or_insert(position);
            let fpp = u64::from(self.offer.spec.frames_per_packet);
            let sequence = position / fpp;
            let slot = (sequence % self.tags.len() as u64) as usize;
            let offset = (position % fpp) as usize * self.frame_bytes;
            if self.tags[slot] == Some(sequence) {
                let start = slot * self.packet_bytes + offset;
                let destination = frame as usize * self.frame_bytes;
                output[destination..destination + self.frame_bytes]
                    .copy_from_slice(&self.pcm[start..start + self.frame_bytes]);
                report.audio_frames += 1;
                if position % fpp == fpp - 1 {
                    self.tags[slot] = None;
                    self.pcm[slot * self.packet_bytes..(slot + 1) * self.packet_bytes].fill(0);
                }
            } else {
                report.missing_frames += 1;
            }
            self.next = position + 1;
        }
        before_commit();
        if gate
            .0
            .compare_exchange(RENDERING, ACTIVE, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // Invalidation won the commit race. Clear before acknowledging it;
            // a receipt stays Pending until this scrub is complete.
            output.fill(0);
            self.clear();
            report = PlayoutReport {
                inactive_frames: frames,
                skipped_frames: report.skipped_frames,
                ..PlayoutReport::default()
            };
            gate.0.store(INACTIVE, Ordering::Release);
        }
        self.counters.audio_frames = self
            .counters
            .audio_frames
            .saturating_add(report.audio_frames);
        self.counters.missing_frames = self
            .counters
            .missing_frames
            .saturating_add(report.missing_frames);
        self.counters.prebuffer_frames = self
            .counters
            .prebuffer_frames
            .saturating_add(report.prebuffer_frames);
        self.counters.early_frames = self
            .counters
            .early_frames
            .saturating_add(report.early_frames);
        self.counters.skipped_frames = self
            .counters
            .skipped_frames
            .saturating_add(report.skipped_frames);
        self.counters.inactive_frames = self
            .counters
            .inactive_frames
            .saturating_add(report.inactive_frames);
        Ok(report)
    }
}

impl Drop for Playout {
    fn drop(&mut self) {
        self.lifecycle.invalidate();
        self.clear();
        // Exclusive ownership means no render can still be using this storage.
        self.lifecycle.0.store(INACTIVE, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;
    use std::sync::{Barrier, mpsc};

    #[test]
    fn invalidation_returns_pending_without_waiting_then_render_scrubs_before_completion() {
        let offer = Offer {
            version: VERSION,
            generation: 1,
            spec: Spec {
                stream: 1,
                name: "race".into(),
                codec: Codec::RawF64Le,
                sample_rate: 44_117,
                channels: vec!["FR".into(), "FL".into()],
                frames_per_packet: 5,
                provenance: Provenance {
                    sources: vec![],
                    visited_peers: vec![],
                },
            },
        };
        let anchor = Instant::now();
        let gate = Arc::new(Lifecycle::new());
        let mut p = Playout::new(
            offer,
            PlayoutConfig {
                anchor,
                target_buffer_frames: 0,
            },
            8,
            Arc::clone(&gate),
        )
        .unwrap();
        p.insert(0, &[0xa5; 80]);
        let barrier = Arc::new(Barrier::new(2));
        let (done_tx, done_rx) = mpsc::channel();
        let worker_gate = Arc::clone(&gate);
        let worker_barrier = Arc::clone(&barrier);
        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            worker_gate.invalidate();
            done_tx.send(worker_gate.receipt()).unwrap();
        });
        let mut out = [0xff; 48]; // Retains a partial packet before revocation.
        let report = p
            .render_inner(anchor, &mut out, || {
                barrier.wait();
                // The worker must finish immediately even while render is paused.
                let receipt = done_rx
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                assert_eq!(receipt.status(), RevocationStatus::Pending);
            })
            .unwrap();
        worker.join().unwrap();
        assert_eq!(gate.status(), RevocationStatus::Complete);
        assert_eq!(out, [0; 48]);
        assert_eq!(report.inactive_frames, 3);
        assert_eq!(report.audio_frames, 0);
        assert!(p.pcm.iter().all(|&b| b == 0));
        assert!(p.tags.iter().all(Option::is_none));
        assert!(!p.snapshot().active);
    }
}
