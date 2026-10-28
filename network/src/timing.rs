//! Worker-side observations. Sample duration is not wall-clock residence or device latency.
use crate::{Counters, Offer};
use serde::Serialize;
use std::time::{Duration, Instant};

pub const RTT_FRESH_FOR: Duration = Duration::from_secs(3);
pub const PROBE_INTERVAL: Duration = Duration::from_secs(1);
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RttValidity {
    Unavailable,
    Fresh,
    Stale,
    Disconnected,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Never,
    InFlight,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Serialize)]
pub struct RttTiming {
    pub kind: RttKind,
    pub validity: RttValidity,
    pub last_attempt: ProbeStatus,
    pub last_attempt_at_session_ms: Option<f64>,
    pub value_ms: Option<f64>,
    pub measured_at_session_ms: Option<f64>,
    pub age_ms: Option<f64>,
    pub fresh_for_ms: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RttKind {
    /// Includes reliable stream scheduling, retransmission, and peer worker turnaround.
    ApplicationControlRoundTrip,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSyncState {
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct ClockTiming {
    pub state: ClockSyncState,
    pub offset_ms: Option<f64>,
    pub uncertainty_ms: Option<f64>,
}

/// Each queue field is sample-domain occupancy, not measured residence time.
#[derive(Debug, Serialize)]
pub struct StreamTiming {
    pub stream: u64,
    pub generation: u64,
    pub sample_rate: u32,
    pub frames_per_packet: u16,
    pub queued_packets: usize,
    pub queued_sample_frames: u64,
    pub queued_sample_ms: f64,
    pub partial_sample_frames: u64,
    pub partial_sample_ms: f64,
    pub capacity_packets: usize,
    pub capacity_sample_frames: u64,
    pub capacity_sample_ms: f64,
    /// Receiver: next consumer position; sender: next packet to be assigned.
    /// None means the u64 sample timeline has exhausted its representable range.
    pub next_sample_frame: Option<u64>,
    /// Receiver only: next pull through furthest buffered packet, including holes.
    pub buffered_timeline_sample_frames: Option<u64>,
    pub buffered_timeline_ms: Option<f64>,
    /// Caller-owned and not reported to this worker. Never infer it from capacity.
    pub consumer_prebuffer_ms: Option<f64>,
    pub counters: Counters,
}

impl StreamTiming {
    pub(crate) fn new(offer: &Offer, packets: usize, capacity: usize, counters: Counters) -> Self {
        let frames = |n: usize| n as u64 * u64::from(offer.spec.frames_per_packet);
        let queued = frames(packets);
        let capacity_frames = frames(capacity);
        Self {
            stream: offer.spec.stream,
            generation: offer.generation,
            sample_rate: offer.spec.sample_rate,
            frames_per_packet: offer.spec.frames_per_packet,
            queued_packets: packets,
            queued_sample_frames: queued,
            queued_sample_ms: queued as f64 * 1000.0 / f64::from(offer.spec.sample_rate),
            partial_sample_frames: 0,
            partial_sample_ms: 0.0,
            capacity_packets: capacity,
            capacity_sample_frames: capacity_frames,
            capacity_sample_ms: capacity_frames as f64 * 1000.0 / f64::from(offer.spec.sample_rate),
            next_sample_frame: None,
            buffered_timeline_sample_frames: None,
            buffered_timeline_ms: None,
            consumer_prebuffer_ms: None,
            counters,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SessionTiming {
    pub connected: bool,
    /// Local monotonic milliseconds since this Session was created, not a UTC timestamp.
    pub observed_at_session_ms: f64,
    pub application_rtt: RttTiming,
    pub clock_sync: ClockTiming,
    pub one_way_network_ms: Option<f64>,
    pub end_to_end_audio_ms: Option<f64>,
    pub send_streams: Vec<StreamTiming>,
    pub receive_streams: Vec<StreamTiming>,
    pub rejected: Counters,
}

pub(crate) struct ProbeState {
    pub origin: Instant,
    pub last_attempt: Option<Instant>,
    pub status: ProbeStatus,
    pub sample: Option<(Instant, Duration)>,
    pub nonce: u64,
}

impl ProbeState {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            last_attempt: None,
            status: ProbeStatus::Never,
            sample: None,
            nonce: 0,
        }
    }

    pub fn snapshot(&self, now: Instant, connected: bool) -> RttTiming {
        let age = self.sample.map(|(at, _)| now.duration_since(at));
        RttTiming {
            kind: RttKind::ApplicationControlRoundTrip,
            validity: if !connected {
                RttValidity::Disconnected
            } else {
                match age {
                    None => RttValidity::Unavailable,
                    Some(age) if age <= RTT_FRESH_FOR => RttValidity::Fresh,
                    Some(_) => RttValidity::Stale,
                }
            },
            last_attempt: self.status,
            last_attempt_at_session_ms: self
                .last_attempt
                .map(|at| ms(at.duration_since(self.origin))),
            value_ms: self.sample.map(|(_, rtt)| ms(rtt)),
            measured_at_session_ms: self
                .sample
                .map(|(at, _)| ms(at.duration_since(self.origin))),
            age_ms: age.map(ms),
            fresh_for_ms: ms(RTT_FRESH_FOR),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_sample_expires_without_refreshing_when_observed() {
        let mut state = ProbeState::new();
        let at = state.origin + Duration::from_secs(1);
        assert_eq!(state.snapshot(at, true).validity, RttValidity::Unavailable);
        state.sample = Some((at, Duration::from_millis(42)));
        state.status = ProbeStatus::Succeeded;
        let fresh = state.snapshot(at + Duration::from_secs(2), true);
        assert_eq!(fresh.validity, RttValidity::Fresh);
        assert_eq!(fresh.value_ms, Some(42.0));
        assert_eq!(fresh.measured_at_session_ms, Some(1000.0));
        assert_eq!(fresh.age_ms, Some(2000.0));
        assert_eq!(
            state
                .snapshot(at + RTT_FRESH_FOR + Duration::from_nanos(1), true)
                .validity,
            RttValidity::Stale
        );
        assert_eq!(
            state.snapshot(at, false).validity,
            RttValidity::Disconnected
        );
        assert_eq!(
            state
                .snapshot(at + Duration::from_secs(20), true)
                .measured_at_session_ms,
            Some(1000.0)
        );
    }
}
