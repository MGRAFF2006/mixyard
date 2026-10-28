//! Offline signal-response metering. These are sample positions, not CPU time,
//! frequency-dependent group delay, device latency or unsynchronized network time.
use crate::dsp::{Chain, ProcessError, Tail};
use serde::{Deserialize, Serialize};
use std::mem::size_of;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Probe {
    pub block_frames: usize,
    pub observation_frames: usize,
    /// These lanes receive the same impulse at the same local sample position.
    pub reference_channels: Vec<String>,
    pub amplitude: f64,
    /// Absolute output magnitude required to count as a response; zero is exact.
    pub detection_threshold: f64,
    pub bypassed: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_observation_frames: usize,
    pub max_work_units: usize,
    pub max_buffer_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidProbe,
    Budget,
    Processing(ProcessError),
    InvalidSelection,
    NoResponse { channel: String },
    IncompletePeakWindow,
    AlignmentBudget,
    Allocation,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResponse {
    pub channel: String,
    pub first_response_frame: Option<usize>,
    /// Earliest occurrence when equal-magnitude peaks tie.
    pub strongest_peak_frame: Option<usize>,
    pub peak_magnitude: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub mode: &'static str,
    pub sample_rate_hz: u32,
    pub probe: Probe,
    /// A conservative finite support bound fits inside the observation window.
    /// False includes infinite/unknown support and windows ending before a tap.
    pub response_window_complete: bool,
    pub channels: Vec<ChannelResponse>,
}

/// Consumes an isolated prepared chain; never borrow a live route's state for a
/// probe. Only one block of samples is retained, regardless of observation time.
pub fn measure(mut chain: Chain, probe: Probe, limits: Limits) -> Result<Report, Error> {
    let config = chain.config();
    let channels = &config.format.channels;
    if probe.block_frames == 0
        || probe.observation_frames == 0
        || probe.reference_channels.is_empty()
        || !probe.amplitude.is_finite()
        || probe.amplitude == 0.0
        || !probe.detection_threshold.is_finite()
        || probe.detection_threshold < 0.0
        || probe
            .reference_channels
            .iter()
            .enumerate()
            .any(|(i, id)| !channels.contains(id) || probe.reference_channels[..i].contains(id))
    {
        return Err(Error::InvalidProbe);
    }
    let count = channels.len();
    let samples = probe.block_frames.checked_mul(count).ok_or(Error::Budget)?;
    let mut bytes = samples.checked_mul(size_of::<f64>()).ok_or(Error::Budget)?;
    bytes = count
        .checked_mul(size_of::<ChannelResponse>())
        .and_then(|n| n.checked_add(bytes))
        .ok_or(Error::Budget)?;
    for id in channels {
        bytes = bytes.checked_add(id.len()).ok_or(Error::Budget)?;
    }
    let blocks = probe.observation_frames.div_ceil(probe.block_frames);
    let work = chain
        .admission()
        .work_units_per_block
        .checked_mul(blocks)
        .and_then(|n| {
            probe
                .observation_frames
                .checked_mul(count)?
                .checked_mul(4)?
                .checked_add(n)
        })
        .ok_or(Error::Budget)?;
    if probe.observation_frames > limits.max_observation_frames
        || bytes > limits.max_buffer_bytes
        || work > limits.max_work_units
    {
        return Err(Error::Budget);
    }
    let sample_rate_hz = config.format.sample_rate_hz;
    let mut responses: Vec<_> = channels
        .iter()
        .map(|channel| ChannelResponse {
            channel: channel.clone(),
            first_response_frame: None,
            strongest_peak_frame: None,
            peak_magnitude: 0.0,
        })
        .collect();
    let mut input = Vec::new();
    input
        .try_reserve_exact(samples)
        .map_err(|_| Error::Allocation)?;
    input.resize(samples, 0.0);
    for (i, id) in channels.iter().enumerate() {
        if probe.reference_channels.contains(id) {
            input[i] = probe.amplitude;
        }
    }
    let complete = probe.bypassed
        || matches!(chain.admission().tail,
        Tail::Finite { frames } if frames < probe.observation_frames);
    chain.reset();
    chain.set_bypassed(probe.bypassed);
    let mut offset = 0;
    while offset < probe.observation_frames {
        let frames = probe.block_frames.min(probe.observation_frames - offset);
        chain
            .process(frames, &input[..frames * count])
            .map_err(Error::Processing)?;
        for (frame, row) in chain.output().chunks_exact(count).enumerate() {
            for (response, sample) in responses.iter_mut().zip(row) {
                let magnitude = sample.abs();
                if magnitude > response.peak_magnitude {
                    response.peak_magnitude = magnitude;
                    response.strongest_peak_frame =
                        (magnitude > probe.detection_threshold).then_some(offset + frame);
                }
                if magnitude > probe.detection_threshold {
                    response.first_response_frame.get_or_insert(offset + frame);
                }
            }
        }
        input.fill(0.0);
        offset += frames;
    }
    Ok(Report {
        mode: "offline-impulse-response",
        sample_rate_hz,
        probe,
        response_window_complete: complete,
        channels: responses,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Feature {
    FirstResponse,
    StrongestPeak,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentDelay {
    pub channel: String,
    pub frames: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentPlan {
    pub mode: &'static str,
    pub feature: Feature,
    pub sample_rate_hz: u32,
    pub target_frame: usize,
    pub delays: Vec<AlignmentDelay>,
}

/// Aligns a feature of this single common-reference offline probe. It does not
/// synchronize independent clocks, phase/frequency response or native outputs.
pub fn align(
    report: &Report,
    channels: &[String],
    feature: Feature,
    max_delay_frames: usize,
) -> Result<AlignmentPlan, Error> {
    if channels.len() < 2
        || channels
            .iter()
            .enumerate()
            .any(|(i, id)| channels[..i].contains(id))
    {
        return Err(Error::InvalidSelection);
    }
    if matches!(feature, Feature::StrongestPeak) && !report.response_window_complete {
        return Err(Error::IncompletePeakWindow);
    }
    let mut positions = Vec::with_capacity(channels.len());
    for id in channels {
        let response = report
            .channels
            .iter()
            .find(|c| &c.channel == id)
            .ok_or(Error::InvalidSelection)?;
        let frame = match feature {
            Feature::FirstResponse => response.first_response_frame,
            Feature::StrongestPeak => response.strongest_peak_frame,
        }
        .ok_or_else(|| Error::NoResponse {
            channel: id.clone(),
        })?;
        positions.push(frame);
    }
    let target = *positions.iter().max().ok_or(Error::InvalidSelection)?;
    let delays: Vec<_> = channels
        .iter()
        .zip(positions)
        .map(|(id, frame)| AlignmentDelay {
            channel: id.clone(),
            frames: target - frame,
        })
        .collect();
    if delays.iter().any(|d| d.frames > max_delay_frames) {
        return Err(Error::AlignmentBudget);
    }
    Ok(AlignmentPlan {
        mode: "offline-feature-alignment",
        feature,
        sample_rate_hz: report.sample_rate_hz,
        target_frame: target,
        delays,
    })
}
