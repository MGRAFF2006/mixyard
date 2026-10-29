use crate::playout::{Lifecycle, Playout, PlayoutConfig, Revocation};
use crate::protocol::{Frame, Limits, Offer, Permissions, VERSION};
use crate::timing::StreamTiming;
use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counters {
    pub accepted: u64,
    pub missing_silence: u64,
    pub disconnected_silence: u64,
    pub duplicate: u64,
    pub late: u64,
    pub stale_generation: u64,
    pub beyond_window: u64,
    pub malformed: u64,
    pub unadmitted: u64,
    pub send_overflow: u64,
    pub congestion_drop: u64,
    pub send_error: u64,
}

struct Timeline {
    offer: Offer,
    next: u64,
    packets: BTreeMap<u64, Vec<u8>>,
    counters: Counters,
    lifecycle: Arc<Lifecycle>,
    playout_claimed: bool,
    arrival: Option<crate::worker::ArrivalObservation>,
}

impl Drop for Timeline {
    fn drop(&mut self) {
        self.lifecycle.invalidate();
    }
}

/// Bounded reorder/jitter window driven by the consumer's sample timeline.
/// Call pull at the negotiated packet cadence, after a caller-chosen initial prebuffer.
/// This allocating worker API must never be called from a realtime callback.
pub struct Receiver {
    permissions: Permissions,
    limits: Limits,
    connected: bool,
    streams: BTreeMap<u64, Timeline>,
    pub rejected: Counters,
}

impl Receiver {
    pub fn new(permissions: Permissions, limits: Limits) -> Result<Self> {
        permissions.validate(&limits)?;
        Ok(Self {
            permissions,
            limits,
            connected: true,
            streams: BTreeMap::new(),
            rejected: Counters::default(),
        })
    }

    pub fn admit(&mut self, offer: Offer) -> Result<()> {
        ensure!(
            self.connected,
            "disconnected: explicit new session required"
        );
        ensure!(
            offer.version == VERSION && offer.generation > 0,
            "unsupported version/zero generation"
        );
        ensure!(
            self.permissions.receive.get(&offer.spec.stream) == Some(&offer.spec),
            "stream/format/provenance is not explicitly permitted"
        );
        if let Some(previous) = self.streams.get(&offer.spec.stream) {
            ensure!(
                offer.generation > previous.offer.generation,
                "generation must increase on restart"
            );
        }
        // Permission validation bounds all unique slots and aggregate admitted rate.
        self.streams.insert(
            offer.spec.stream,
            Timeline {
                offer,
                next: 0,
                packets: BTreeMap::new(),
                counters: Counters::default(),
                lifecycle: Arc::new(Lifecycle::new()),
                playout_claimed: false,
                arrival: None,
            },
        );
        Ok(())
    }

    pub fn ingest(&mut self, data: &[u8]) {
        if !self.connected {
            self.rejected.unadmitted += 1;
            return;
        }
        let frame = match Frame::decode(data, self.limits.max_datagram) {
            Ok(frame) => frame,
            Err(_) => {
                self.rejected.malformed += 1;
                return;
            }
        };
        let Some(stream) = self.streams.get_mut(&frame.stream) else {
            self.rejected.unadmitted += 1;
            return;
        };
        if !stream.lifecycle.active() {
            stream.counters.unadmitted += 1;
        } else if frame.generation != stream.offer.generation {
            stream.counters.stale_generation += 1;
        } else if frame.validate(&stream.offer).is_err() {
            stream.counters.malformed += 1;
        } else if frame.sequence < stream.next {
            stream.counters.late += 1;
        } else if frame.sequence - stream.next >= self.limits.packets_per_stream as u64 {
            stream.counters.beyond_window += 1;
        } else if let std::collections::btree_map::Entry::Vacant(entry) =
            stream.packets.entry(frame.sequence)
        {
            entry.insert(frame.payload);
            // Only unique, admitted, strictly advancing source positions are observations.
            if stream
                .arrival
                .as_ref()
                .is_none_or(|last| frame.sequence > last.sequence)
            {
                let count = stream
                    .arrival
                    .as_ref()
                    .map_or(1, |last| last.count.saturating_add(1));
                stream.arrival = Some(crate::worker::ArrivalObservation {
                    offer: stream.offer.clone(),
                    sequence: frame.sequence,
                    first_frame: frame.sample_timestamp,
                    arrived_at: std::time::Instant::now(),
                    count,
                });
            }
            stream.counters.accepted += 1;
        } else {
            stream.counters.duplicate += 1;
        }
    }

    pub fn arrival_observation(
        &self,
        id: u64,
        generation: u64,
    ) -> Option<crate::worker::ArrivalObservation> {
        let stream = self.streams.get(&id)?;
        if !self.connected || !stream.lifecycle.active() || stream.offer.generation != generation {
            return None;
        }
        stream.arrival.clone()
    }

    pub(crate) fn cancel_generation(&mut self, offer: &Offer) {
        if let Some(stream) = self.streams.get_mut(&offer.spec.stream)
            && crate::worker::same_offer(&stream.offer, offer)
        {
            stream.lifecycle.invalidate();
            for payload in stream.packets.values_mut() {
                payload.fill(0);
            }
            stream.packets.clear();
            stream.arrival = None;
        }
    }

    /// Exactly one negotiated packet's PCM bytes, preserving raw payload bits.
    /// Missing/expired packets contribute all-zero PCM and observable counters.
    pub fn pull(&mut self, id: u64) -> Result<Vec<u8>> {
        let stream = self
            .streams
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted stream"))?;
        ensure!(
            !stream.playout_claimed,
            "generation is owned by a playout handle"
        );
        let silence = || -> Result<Vec<u8>> { Ok(vec![0; stream.offer.spec.payload_bytes()?]) };
        if !self.connected || !stream.lifecycle.active() {
            stream.counters.disconnected_silence += 1;
            return silence();
        }
        let next = stream
            .next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("sequence exhausted: restart with a new generation"))?;
        let payload = match stream.packets.remove(&stream.next) {
            Some(payload) => payload,
            None => {
                stream.counters.missing_silence += 1;
                silence()?
            }
        };
        stream.next = next;
        Ok(payload)
    }

    /// Worker-only allocation; one exclusive consumer per stream generation.
    pub(crate) fn create_playout_for(
        &mut self,
        offer: &Offer,
        config: PlayoutConfig,
    ) -> Result<Playout> {
        let stream = self
            .streams
            .get(&offer.spec.stream)
            .ok_or_else(|| anyhow::anyhow!("unadmitted stream"))?;
        ensure!(
            crate::worker::same_offer(&stream.offer, offer),
            "worker descriptor/generation mismatch"
        );
        // Validation and claim share this exclusive Receiver borrow. A stale
        // constructor must never acquire/drop the newer generation's lifecycle.
        self.create_playout(offer.spec.stream, config)
    }

    pub fn create_playout(&mut self, id: u64, config: PlayoutConfig) -> Result<Playout> {
        let stream = self
            .streams
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted stream"))?;
        ensure!(
            self.connected && stream.lifecycle.active(),
            "inactive stream"
        );
        ensure!(
            !stream.playout_claimed && stream.next == 0,
            "generation already consumed/claimed"
        );
        let playout = Playout::new(
            stream.offer.clone(),
            config,
            self.limits.packets_per_stream,
            Arc::clone(&stream.lifecycle),
        )?;
        stream.playout_claimed = true;
        Ok(playout)
    }

    /// Bounded worker handoff, not a realtime callback API. Transfers exact PCM
    /// and publishes the consumer cursor back to ingress's reorder window.
    pub fn service_playout(&mut self, playout: &mut Playout) -> Result<()> {
        if !playout.lifecycle.active() {
            playout.clear();
            return Ok(());
        }
        let stream = self
            .streams
            .get_mut(&playout.spec().stream)
            .ok_or_else(|| anyhow::anyhow!("unadmitted stream"))?;
        ensure!(
            Arc::ptr_eq(&stream.lifecycle, &playout.lifecycle),
            "playout belongs to another receiver/generation"
        );
        stream.next = playout.next / u64::from(stream.offer.spec.frames_per_packet);
        while let Some((sequence, payload)) = stream.packets.pop_first() {
            playout.insert(sequence, &payload);
        }
        Ok(())
    }

    /// Local receive permission removal. A new Session is needed to permit again.
    pub fn revoke(&mut self, id: u64) -> Result<Revocation> {
        // Remove even a not-yet-admitted grant so a racing Offer cannot restore it.
        let permitted = self.permissions.receive.remove(&id).is_some();
        if let Some(stream) = self.streams.get_mut(&id) {
            stream.lifecycle.invalidate();
            stream.packets.clear();
            stream.arrival = None;
            return Ok(stream.lifecycle.receipt());
        }
        ensure!(permitted, "unpermitted stream");
        let lifecycle = Arc::new(Lifecycle::new());
        lifecycle.invalidate();
        Ok(lifecycle.receipt())
    }

    pub fn counters(&self, id: u64) -> Result<Counters> {
        Ok(self
            .streams
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted stream"))?
            .counters
            .clone())
    }

    pub fn queued(&self, id: u64) -> usize {
        self.streams.get(&id).map_or(0, |s| s.packets.len())
    }

    pub fn timing_snapshot(&self) -> Vec<StreamTiming> {
        self.streams
            .values()
            .map(|stream| {
                let mut timing = StreamTiming::new(
                    &stream.offer,
                    stream.packets.len(),
                    self.limits.packets_per_stream,
                    stream.counters.clone(),
                );
                timing.next_sample_frame = stream
                    .next
                    .checked_mul(u64::from(stream.offer.spec.frames_per_packet));
                let span = stream
                    .packets
                    .last_key_value()
                    .map_or(0, |(&last, _)| last - stream.next + 1)
                    * u64::from(stream.offer.spec.frames_per_packet);
                timing.buffered_timeline_sample_frames = Some(span);
                timing.buffered_timeline_ms =
                    Some(span as f64 * 1000.0 / f64::from(stream.offer.spec.sample_rate));
                timing
            })
            .collect()
    }

    pub fn disconnect(&mut self) {
        self.connected = false;
        for stream in self.streams.values_mut() {
            stream.lifecycle.invalidate();
            stream.packets.clear();
            stream.arrival = None;
        }
    }
}

struct Pending {
    offer: Offer,
    next: u64,
    packets: VecDeque<Frame>,
    overflows: u64,
    congestion_drops: u64,
    send_errors: u64,
    partial: Vec<u8>,
    last_due: Option<u64>,
    paced_expired: u64,
    paced_claimed: bool,
    cancelled: bool,
}

impl Pending {
    fn enqueue(&mut self, payload: Vec<u8>, limit: usize) -> Result<()> {
        ensure!(limit > 0, "empty sender queue budget");
        let next = self
            .next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("sequence exhausted"))?;
        let frame = Frame {
            stream: self.offer.spec.stream,
            generation: self.offer.generation,
            sequence: self.next,
            sample_timestamp: self
                .next
                .checked_mul(u64::from(self.offer.spec.frames_per_packet))
                .ok_or_else(|| anyhow::anyhow!("sample timestamp exhausted"))?,
            payload,
        };
        frame.validate(&self.offer)?;
        if self.packets.len() == limit {
            self.packets.pop_front();
            self.overflows += 1;
        }
        self.packets.push_back(frame);
        self.next = next;
        Ok(())
    }
}

/// Each round takes at most one packet per admitted stream. Producers cannot grow
/// a shared FIFO or wait on a sibling. Congestion remains shared at the QUIC layer.
pub struct Outbox {
    limit: usize,
    streams: BTreeMap<u64, Pending>,
    round_start: usize,
}

impl Outbox {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            streams: BTreeMap::new(),
            round_start: 0,
        }
    }

    pub fn admit(&mut self, offer: Offer) {
        self.streams.insert(
            offer.spec.stream,
            Pending {
                offer,
                next: 0,
                packets: VecDeque::new(),
                overflows: 0,
                congestion_drops: 0,
                send_errors: 0,
                partial: Vec::new(),
                last_due: None,
                paced_expired: 0,
                paced_claimed: false,
                cancelled: false,
            },
        );
    }

    pub fn generation(&self, id: u64) -> Option<u64> {
        self.streams.get(&id).map(|s| s.offer.generation)
    }

    pub fn enqueue(&mut self, id: u64, payload: Vec<u8>) -> Result<()> {
        let stream = self
            .streams
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted sender"))?;
        ensure!(!stream.cancelled, "sender generation cancelled");
        ensure!(
            stream.partial.is_empty(),
            "pending partial PCM: continue through enqueue_pcm"
        );
        stream.enqueue(payload, self.limit)
    }

    /// Packetize whole interleaved sample frames; at most one partial packet is retained.
    /// No padding, truncation, conversion or cross-datagram sample-frame fragmentation.
    pub fn enqueue_pcm(&mut self, id: u64, pcm: &[u8]) -> Result<usize> {
        let stream = self
            .streams
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted sender"))?;
        ensure!(!stream.cancelled, "sender generation cancelled");
        let packet_bytes = stream.offer.spec.payload_bytes()?;
        ensure!(
            pcm.len()
                <= packet_bytes
                    .checked_mul(self.limit)
                    .ok_or_else(|| anyhow::anyhow!("PCM block budget overflow"))?,
            "PCM block exceeds per-stream worker budget: submit smaller sample-aligned chunks"
        );
        let frame_bytes =
            stream.offer.spec.codec.sample_bytes()? * stream.offer.spec.channels.len();
        ensure!(
            frame_bytes > 0 && packet_bytes > 0 && pcm.len().is_multiple_of(frame_bytes),
            "PCM block must contain whole ordered sample frames"
        );
        let mut bytes = pcm;
        let mut packets = 0;
        while !bytes.is_empty() {
            let take = (packet_bytes - stream.partial.len()).min(bytes.len());
            stream.partial.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if stream.partial.len() == packet_bytes {
                let payload = std::mem::take(&mut stream.partial);
                stream.enqueue(payload, self.limit)?;
                packets += 1;
            }
        }
        Ok(packets)
    }

    pub fn sender_offer(&self, id: u64) -> Option<Offer> {
        self.streams
            .get(&id)
            .filter(|s| !s.cancelled)
            .map(|s| s.offer.clone())
    }

    pub(crate) fn claim_pacer(&mut self, offer: &Offer) -> Result<()> {
        let stream = self
            .streams
            .get_mut(&offer.spec.stream)
            .ok_or_else(|| anyhow::anyhow!("unadmitted sender"))?;
        ensure!(
            crate::worker::same_offer(&stream.offer, offer) && !stream.cancelled,
            "inactive sender descriptor"
        );
        ensure!(!stream.paced_claimed, "sender generation already paced");
        stream.paced_claimed = true;
        Ok(())
    }

    pub(crate) fn cancel_sender(&mut self, offer: &Offer) {
        if let Some(stream) = self.streams.get_mut(&offer.spec.stream)
            && crate::worker::same_offer(&stream.offer, offer)
        {
            stream.cancelled = true;
            for frame in &mut stream.packets {
                frame.payload.fill(0);
            }
            stream.packets.clear();
            stream.partial.fill(0);
            stream.partial.clear();
        }
    }

    /// Validate before mutating: one absolute slot per descriptor/generation.
    pub fn due(&mut self, offer: &Offer, sequence: u64) -> Result<Option<Frame>> {
        let stream = self
            .streams
            .get_mut(&offer.spec.stream)
            .ok_or_else(|| anyhow::anyhow!("unadmitted paced sender"))?;
        ensure!(!stream.cancelled, "sender generation cancelled");
        ensure!(
            crate::worker::same_offer(&stream.offer, offer),
            "paced descriptor/generation mismatch"
        );
        ensure!(
            stream.last_due.is_none_or(|last| sequence > last),
            "duplicate/backward due slot"
        );
        let next = sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("paced sequence exhausted"))?;
        next.checked_mul(u64::from(offer.spec.frames_per_packet))
            .ok_or_else(|| anyhow::anyhow!("paced sample timeline exhausted"))?;
        stream.last_due = Some(sequence);
        stream.paced_claimed = true;
        while stream
            .packets
            .front()
            .is_some_and(|f| f.sequence < sequence)
        {
            stream.packets.pop_front();
            stream.paced_expired = stream.paced_expired.saturating_add(1);
        }
        // A partial packet belongs to the old assignment cursor and cannot survive expiration.
        if stream.next < next {
            stream.partial.fill(0);
            stream.partial.clear();
            stream.next = next;
        }
        if stream
            .packets
            .front()
            .is_some_and(|f| f.sequence == sequence)
        {
            Ok(stream.packets.pop_front())
        } else {
            Ok(None)
        }
    }

    pub fn paced_expired(&self, id: u64) -> u64 {
        self.streams.get(&id).map_or(0, |s| s.paced_expired)
    }

    pub fn round(&mut self) -> Vec<Frame> {
        let mut frames: Vec<_> = self
            .streams
            .values_mut()
            .filter(|s| !s.cancelled && !s.paced_claimed)
            .filter_map(|s| s.packets.pop_front())
            .collect();
        if !frames.is_empty() {
            let start = self.round_start % frames.len();
            frames.rotate_left(start);
            self.round_start = start + 1;
        }
        frames
    }

    pub fn overflows(&self, id: u64) -> u64 {
        self.streams.get(&id).map_or(0, |s| s.overflows)
    }
    pub fn congestion_drop(&mut self, id: u64) {
        if let Some(stream) = self.streams.get_mut(&id) {
            stream.congestion_drops += 1;
        }
    }
    pub fn send_error(&mut self, id: u64) {
        if let Some(stream) = self.streams.get_mut(&id) {
            stream.send_errors += 1;
        }
    }
    pub fn counters(&self, id: u64) -> Result<Counters> {
        let stream = self
            .streams
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted sender"))?;
        Ok(Counters {
            send_overflow: stream.overflows,
            congestion_drop: stream.congestion_drops,
            send_error: stream.send_errors,
            ..Counters::default()
        })
    }
    pub fn timing_snapshot(&self) -> Vec<StreamTiming> {
        self.streams
            .values()
            .map(|stream| {
                let mut timing = StreamTiming::new(
                    &stream.offer,
                    stream.packets.len(),
                    self.limit,
                    self.counters(stream.offer.spec.stream)
                        .expect("admitted stream"),
                );
                timing.next_sample_frame = stream
                    .next
                    .checked_mul(u64::from(stream.offer.spec.frames_per_packet));
                let frame_bytes = stream
                    .offer
                    .spec
                    .codec
                    .sample_bytes()
                    .expect("admitted codec")
                    * stream.offer.spec.channels.len();
                timing.partial_sample_frames = (stream.partial.len() / frame_bytes) as u64;
                timing.partial_sample_ms = timing.partial_sample_frames as f64 * 1000.0
                    / f64::from(stream.offer.spec.sample_rate);
                timing
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.streams.clear();
    }
}
