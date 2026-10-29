//! Allocating network workers. Local arrival trends are not hardware clock synchronization.
use crate::{Offer, Playout, PlayoutConfig, PlayoutReport, Session};
use anyhow::{Context, Result, ensure};
use std::{sync::Arc, time::Instant};
use tokio::sync::{mpsc, watch};

/// Only produced for admitted, unique, strictly increasing ingress positions.
#[derive(Clone, Debug)]
pub struct ArrivalObservation {
    pub offer: Offer,
    pub sequence: u64,
    pub first_frame: u64,
    pub arrived_at: Instant,
    /// Advancing observations in this generation; not inferred missing packets.
    pub count: u64,
}

// BEGIN STD CONTROLLER: independently compilable without transport dependencies.
pub mod control {
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Error {
        InvalidConfig,
        TimeWentBackwards,
        TimelineExhausted,
    }
    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }
    impl std::error::Error for Error {}

    #[derive(Clone, Copy, Debug)]
    pub struct Tick {
        pub sequence: u64,
        pub skipped: u64,
        pub scheduled_at: Instant,
    }

    /// Rational sample cadence: deadlines always derive from the original anchor.
    /// Late wakeups jump directly to the current slot, never a burst of missed work.
    pub struct Cadence {
        anchor: Instant,
        rate: u32,
        frames: u16,
        next: u64,
        last: Option<Instant>,
    }
    impl Cadence {
        pub fn new(anchor: Instant, rate: u32, frames: u16) -> Result<Self, Error> {
            if rate == 0 || frames == 0 {
                return Err(Error::InvalidConfig);
            }
            Ok(Self {
                anchor,
                rate,
                frames,
                next: 0,
                last: None,
            })
        }
        fn deadline(&self, sequence: u64) -> Result<Instant, Error> {
            let ns = (u128::from(sequence) * u128::from(self.frames) * 1_000_000_000)
                .div_ceil(u128::from(self.rate));
            let duration = Duration::new(
                u64::try_from(ns / 1_000_000_000).map_err(|_| Error::TimelineExhausted)?,
                (ns % 1_000_000_000) as u32,
            );
            self.anchor
                .checked_add(duration)
                .ok_or(Error::TimelineExhausted)
        }
        pub fn next_deadline(&self) -> Result<Instant, Error> {
            self.deadline(self.next)
        }
        pub fn due(&mut self, now: Instant) -> Result<Option<Tick>, Error> {
            if self.last.is_some_and(|last| now < last) {
                return Err(Error::TimeWentBackwards);
            }
            self.last = Some(now);
            if now < self.anchor {
                return Ok(None);
            }
            let sequence = now
                .duration_since(self.anchor)
                .as_nanos()
                .checked_mul(u128::from(self.rate))
                .ok_or(Error::TimelineExhausted)?
                / (u128::from(self.frames) * 1_000_000_000);
            let sequence = u64::try_from(sequence).map_err(|_| Error::TimelineExhausted)?;
            if sequence < self.next {
                return Ok(None);
            }
            let tick = Tick {
                sequence,
                skipped: sequence - self.next,
                scheduled_at: self.deadline(sequence)?,
            };
            self.next = sequence.checked_add(1).ok_or(Error::TimelineExhausted)?;
            Ok(Some(tick))
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct JitterConfig {
        pub min_frames: u64,
        pub max_frames: u64,
        pub initial_frames: u64,
        /// At least one packet duration; at most one packet of target change per interval.
        pub adjust_interval: Duration,
        pub fresh_for: Duration,
        pub min_drift_span: Duration,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Validity {
        Unavailable,
        Fresh,
        Stale,
        Inactive,
    }
    #[derive(Clone, Copy, Debug)]
    pub struct Estimate {
        pub validity: Validity,
        pub sample_count: u64,
        pub last_arrival: Option<Instant>,
        pub age: Option<Duration>,
        /// Source sample slope relative to local packet arrival time; includes path/scheduling variation.
        pub arrival_rate_error_ppm: Option<f64>,
        /// Empirical envelope, not a guaranteed bound or a remote-to-local timestamp map.
        pub uncertainty_ppm: Option<f64>,
        pub jitter_ns: f64,
        pub target_frames: u64,
        pub target_changes: u64,
    }
    #[derive(Clone, Copy)]
    struct Sample {
        frame: u64,
        at: Instant,
    }
    pub struct Controller {
        rate: u32,
        packet: u16,
        config: JitterConfig,
        generation: u64,
        first: Option<Sample>,
        last: Option<Sample>,
        count: u64,
        jitter: f64,
        target: u64,
        changes: u64,
        last_adjust: Option<Instant>,
        loss: bool,
        active: bool,
    }
    impl Controller {
        pub fn new(
            rate: u32,
            packet: u16,
            generation: u64,
            config: JitterConfig,
            capacity: u64,
        ) -> Result<Self, Error> {
            let period_ns = (u128::from(packet) * 1_000_000_000).div_ceil(u128::from(rate.max(1)));
            if rate == 0
                || packet == 0
                || generation == 0
                || config.min_frames > config.initial_frames
                || config.initial_frames > config.max_frames
                || config.max_frames > capacity
                || config.adjust_interval.as_nanos() < period_ns
                || config.fresh_for.is_zero()
                || config.min_drift_span.is_zero()
            {
                return Err(Error::InvalidConfig);
            }
            Ok(Self {
                rate,
                packet,
                config,
                generation,
                first: None,
                last: None,
                count: 0,
                jitter: 0.0,
                target: config.initial_frames,
                changes: 0,
                last_adjust: None,
                loss: false,
                active: true,
            })
        }
        /// Reject stale epochs and reordered/repeated source positions without refreshing age.
        pub fn observe(&mut self, generation: u64, frame: u64, at: Instant) -> bool {
            if !self.active || generation != self.generation {
                return false;
            }
            if let Some(last) = self.last {
                if frame <= last.frame || at <= last.at {
                    return false;
                }
                let elapsed = at.duration_since(last.at);
                if elapsed > self.config.fresh_for {
                    self.first = None;
                    self.count = 0;
                    self.jitter = 0.0;
                } else {
                    let expected = (frame - last.frame) as f64 * 1e9 / f64::from(self.rate);
                    let deviation = (elapsed.as_nanos() as f64 - expected).abs();
                    self.jitter += (deviation - self.jitter) / 16.0;
                }
            }
            let sample = Sample { frame, at };
            self.first.get_or_insert(sample);
            self.last = Some(sample);
            self.count = self.count.saturating_add(1);
            true
        }
        pub fn note_loss(&mut self, missing_frames: u64) {
            self.loss |= missing_frames > 0;
        }
        /// Returns an explicit local target decision. Playout accounts its silence/skips.
        pub fn adjust(&mut self, now: Instant) -> Option<(u64, u64)> {
            if !self.active
                || self.last.is_none_or(|last| {
                    now < last.at || now.duration_since(last.at) > self.config.fresh_for
                })
                || self.last_adjust.is_some_and(|last| {
                    now < last || now.duration_since(last) < self.config.adjust_interval
                })
            {
                return None;
            }
            self.last_adjust = Some(now);
            let margin = (4.0 * self.jitter * f64::from(self.rate) / 1e9).ceil() as u64;
            let desired = if self.loss {
                self.target.saturating_add(u64::from(self.packet))
            } else {
                self.config.min_frames.saturating_add(margin)
            }
            .clamp(self.config.min_frames, self.config.max_frames);
            self.loss = false;
            let old = self.target;
            let step = u64::from(self.packet);
            self.target = desired.clamp(old.saturating_sub(step), old.saturating_add(step));
            if old == self.target {
                return None;
            }
            self.changes = self.changes.saturating_add(1);
            Some((old, self.target))
        }
        pub fn invalidate(&mut self) {
            self.active = false;
            self.first = None;
            self.last = None;
            self.count = 0;
            self.jitter = 0.0;
            self.loss = false;
        }
        pub fn estimate(&self, now: Instant) -> Estimate {
            let age = self
                .last
                .and_then(|last| now.checked_duration_since(last.at));
            let validity = if !self.active {
                Validity::Inactive
            } else if self.last.is_none() {
                Validity::Unavailable
            } else if age.is_some_and(|age| age <= self.config.fresh_for) {
                Validity::Fresh
            } else {
                Validity::Stale
            };
            let mut ppm = None;
            let mut uncertainty = None;
            if let (Some(first), Some(last)) = (self.first, self.last) {
                let elapsed = last.at.duration_since(first.at);
                if self.count >= 8 && elapsed >= self.config.min_drift_span {
                    let ns = elapsed.as_nanos() as f64;
                    ppm = Some(
                        ((last.frame - first.frame) as f64 * 1e9 / f64::from(self.rate) / ns - 1.0)
                            * 1e6,
                    );
                    uncertainty = Some((2.0 * self.jitter + 1e9 / f64::from(self.rate)) / ns * 1e6);
                }
            }
            Estimate {
                validity,
                sample_count: self.count,
                last_arrival: self.last.map(|s| s.at),
                age,
                arrival_rate_error_ppm: ppm,
                uncertainty_ppm: uncertainty,
                jitter_ns: self.jitter,
                target_frames: self.target,
                target_changes: self.changes,
            }
        }
    }
}
// END STD CONTROLLER

use control::{Cadence, Controller, Estimate, JitterConfig};

#[derive(Clone, Copy, Debug, Default)]
pub struct SendReport {
    pub sent_packets: u64,
    pub skipped_slots: u64,
    pub rounds: u64,
}

/// Paced sender with exclusive, generation-scoped ownership. Keep Session::run alive
/// separately for QUIC ingress. Cancellation scrubs only this worker's generations.
pub struct SendWorker {
    session: Arc<Session>,
    tracks: Vec<(SendLease, Cadence)>,
}
struct SendLease {
    session: Arc<Session>,
    offer: Offer,
}
impl Drop for SendLease {
    fn drop(&mut self) {
        self.session.cancel_sender(&self.offer);
    }
}
impl SendWorker {
    pub async fn new(session: Arc<Session>, offers: Vec<Offer>, anchor: Instant) -> Result<Self> {
        ensure!(
            !offers.is_empty() && offers.len() <= session.worker_stream_limit(),
            "worker stream budget"
        );
        let mut tracks = Vec::with_capacity(offers.len());
        for offer in offers {
            ensure!(
                !tracks
                    .iter()
                    .any(|(other, _): &(SendLease, Cadence)| other.offer.spec.stream
                        == offer.spec.stream),
                "duplicate worker stream"
            );
            let admitted = session
                .sender_offer(offer.spec.stream)
                .await
                .ok_or_else(|| anyhow::anyhow!("unadmitted sender"))?;
            ensure!(
                same_offer(&admitted, &offer),
                "worker descriptor/generation mismatch"
            );
            let cadence =
                Cadence::new(anchor, offer.spec.sample_rate, offer.spec.frames_per_packet)?;
            session.claim_pacer(&offer)?;
            tracks.push((
                SendLease {
                    session: session.clone(),
                    offer,
                },
                cadence,
            ));
        }
        Ok(Self { session, tracks })
    }
    pub async fn run(mut self, mut stop: watch::Receiver<bool>) -> Result<SendReport> {
        let mut report = SendReport::default();
        let mut due = Vec::with_capacity(self.tracks.len());
        loop {
            if *stop.borrow() {
                return Ok(report);
            }
            let deadline = next_deadline(self.tracks.iter().map(|(_, c)| c))?;
            tokio::select! {
                biased;
                _ = stop.changed() => { return Ok(report); }
                _ = tokio::time::sleep_until(deadline.into()) => {}
            }
            let now = Instant::now();
            due.clear();
            for (lease, cadence) in &mut self.tracks {
                if let Some(tick) = cadence.due(now)? {
                    report.skipped_slots = report.skipped_slots.saturating_add(tick.skipped);
                    due.push((lease.offer.clone(), tick.sequence));
                }
            }
            if !due.is_empty() {
                let start = (report.rounds % due.len() as u64) as usize;
                due.rotate_left(start);
            }
            for request in &due {
                match self.session.flush_due(std::slice::from_ref(request)).await {
                    Ok(sent) => {
                        report.sent_packets = report.sent_packets.saturating_add(sent as u64);
                    }
                    Err(error) => {
                        let current = self.session.sender_offer(request.0.spec.stream).await;
                        if current
                            .as_ref()
                            .is_some_and(|offer| same_offer(offer, &request.0))
                        {
                            return Err(error);
                        }
                        // A restarted/revoked stream retires only its old cadence/lease.
                        self.tracks
                            .retain(|(lease, _)| lease.offer.spec.stream != request.0.spec.stream);
                    }
                }
            }
            if self.tracks.is_empty() {
                return Ok(report);
            }
            report.rounds = report.rounds.saturating_add(1);
        }
    }
}

fn next_deadline<'a>(cadences: impl Iterator<Item = &'a Cadence>) -> Result<Instant> {
    let mut earliest = None;
    for cadence in cadences {
        let next = cadence.next_deadline()?;
        earliest = Some(earliest.map_or(next, |old: Instant| old.min(next)));
    }
    earliest.ok_or_else(|| anyhow::anyhow!("empty worker"))
}

pub(crate) fn same_offer(a: &Offer, b: &Offer) -> bool {
    a.version == b.version && a.generation == b.generation && a.spec == b.spec
}

pub struct RenderedBlock {
    pub offer: Offer,
    pub scheduled_at: Instant,
    pub sequence: u64,
    raw: Vec<u8>,
    revocation: crate::Revocation,
    pub playout: PlayoutReport,
    pub estimate: Estimate,
    pub target_change: Option<(u64, u64)>,
}
impl RenderedBlock {
    /// A queued old-generation block cannot be consumed after invalidation.
    /// As with Playout, a copy that committed before revocation cannot be recalled.
    pub fn copy_into(&mut self, output: &mut [u8]) -> bool {
        output.fill(0);
        if output.len() != self.raw.len()
            || self.revocation.status() != crate::RevocationStatus::Active
        {
            self.raw.fill(0);
            return false;
        }
        output.copy_from_slice(&self.raw);
        if self.revocation.status() != crate::RevocationStatus::Active {
            output.fill(0);
            self.raw.fill(0);
            return false;
        }
        true
    }
}
impl Drop for RenderedBlock {
    fn drop(&mut self) {
        self.raw.fill(0);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReceiveReport {
    pub blocks: u64,
    pub output_drops: u64,
    pub skipped_slots: u64,
}
struct ReceiveTrack {
    session: Arc<Session>,
    offer: Offer,
    playout: Playout,
    cadence: Cadence,
    controller: Controller,
    block: Vec<u8>,
}
impl Drop for ReceiveTrack {
    fn drop(&mut self) {
        self.controller.invalidate();
        self.session.cancel_receive_generation(&self.offer);
        self.block.fill(0);
    }
}
/// Exclusive worker-side playout. Output is a bounded nonblocking raw-byte queue;
/// this is deliberately separate from the native finite-float callback boundary.
pub struct ReceiveWorker {
    session: Arc<Session>,
    tracks: Vec<ReceiveTrack>,
}
impl ReceiveWorker {
    pub fn new(
        session: Arc<Session>,
        streams: Vec<(Offer, JitterConfig)>,
        anchor: Instant,
    ) -> Result<Self> {
        ensure!(
            !streams.is_empty() && streams.len() <= session.worker_stream_limit(),
            "worker stream budget"
        );
        let mut tracks = Vec::with_capacity(streams.len());
        for (offer, config) in streams {
            ensure!(
                offer.version == crate::VERSION,
                "unsupported worker version"
            );
            // Reject invalid controller settings before acquiring a lifecycle.
            let controller = Controller::new(
                offer.spec.sample_rate,
                offer.spec.frames_per_packet,
                offer.generation,
                config,
                (session.worker_packet_limit() as u64)
                    .checked_mul(u64::from(offer.spec.frames_per_packet))
                    .context("worker capacity overflow")?,
            )?;
            let cadence =
                Cadence::new(anchor, offer.spec.sample_rate, offer.spec.frames_per_packet)?;
            let playout = session.create_worker_playout(
                &offer,
                PlayoutConfig {
                    anchor,
                    target_buffer_frames: config.initial_frames,
                },
            )?;
            let block = vec![0; offer.spec.payload_bytes()?];
            tracks.push(ReceiveTrack {
                session: session.clone(),
                offer,
                playout,
                cadence,
                controller,
                block,
            });
        }
        Ok(Self { session, tracks })
    }
    pub async fn run(
        mut self,
        output: mpsc::Sender<RenderedBlock>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<ReceiveReport> {
        let mut report = ReceiveReport::default();
        loop {
            if *stop.borrow() {
                return Ok(report);
            }
            let deadline = next_deadline(self.tracks.iter().map(|t| &t.cadence))?;
            tokio::select! {
                biased;
                _ = stop.changed() => { return Ok(report); }
                _ = output.closed() => { return Ok(report); }
                _ = tokio::time::sleep_until(deadline.into()) => {}
            }
            let now = Instant::now();
            for track in &mut self.tracks {
                let Some(tick) = track.cadence.due(now)? else {
                    continue;
                };
                report.skipped_slots = report.skipped_slots.saturating_add(tick.skipped);
                self.session.service_playout(&mut track.playout)?;
                if !track.playout.snapshot().active {
                    track.controller.invalidate();
                    continue;
                }
                if let Some(arrival) = self
                    .session
                    .arrival_observation(track.offer.spec.stream, track.offer.generation)
                {
                    ensure!(
                        same_offer(&arrival.offer, &track.offer),
                        "arrival descriptor mismatch"
                    );
                    track.controller.observe(
                        arrival.offer.generation,
                        arrival.first_frame,
                        arrival.arrived_at,
                    );
                }
                let change = track.controller.adjust(now);
                if let Some((_, frames)) = change {
                    track.playout.set_target_buffer_frames(frames)?;
                }
                let rendered = track
                    .playout
                    .render_into(tick.scheduled_at, &mut track.block)?;
                track.controller.note_loss(rendered.missing_frames);
                self.session.service_playout(&mut track.playout)?;
                let block = RenderedBlock {
                    offer: track.offer.clone(),
                    scheduled_at: tick.scheduled_at,
                    sequence: tick.sequence,
                    raw: track.block.clone(),
                    revocation: track.playout.revocation(),
                    playout: rendered,
                    estimate: track.controller.estimate(now),
                    target_change: change,
                };
                match output.try_send(block) {
                    Ok(()) => {
                        report.blocks = report.blocks.saturating_add(1);
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        report.output_drops = report.output_drops.saturating_add(1);
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        return Ok(report);
                    }
                }
            }
            self.tracks.retain(|track| track.playout.snapshot().active);
            if self.tracks.is_empty() {
                return Ok(report);
            }
        }
    }
}
