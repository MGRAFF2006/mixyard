//! Independent, explicitly attached Float64 DSP. See `docs/dsp.md` for semantics.
//!
//! Compile on a control thread. `process`, `reset` and bypass transitions use
//! preallocated storage only. This module neither authorizes nor discovers routes.

mod filters;

use serde::{Deserialize, Serialize};
use std::mem::size_of;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Format {
    pub sample_rate_hz: u32,
    /// Unique opaque identities, in interleaved sample order. No layout inference.
    pub channels: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChainConfig {
    pub name: String,
    pub format: Format,
    pub effects: Vec<EffectConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectConfig {
    pub name: String,
    pub enabled: bool,
    /// None selects every format lane. Explicit IDs are nonempty and unique.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<String>>,
    pub effect: Effect,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Effect {
    Gain {
        gain: f64,
        invert_polarity: bool,
    },
    Biquad {
        filter: Filter,
        frequency_hz: f64,
        q: f64,
        gain_db: f64,
    },
    Delay {
        frames: usize,
        wet: f64,
        dry: f64,
        feedback: f64,
    },
    Compressor {
        parameters: Dynamics,
    },
    /// Downward expander; large ratios and a finite range provide gate behavior.
    Expander {
        parameters: Dynamics,
    },
    Fir {
        coefficients: Vec<f64>,
    },
    /// Explicit mapping within the existing format; never changes its identities.
    ChannelMatrix {
        routes: Vec<MatrixRoute>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatrixRoute {
    pub output: String,
    pub inputs: Vec<MatrixInput>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatrixInput {
    pub channel: String,
    pub gain: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Filter {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Peaking,
    LowShelf,
    HighShelf,
    AllPass,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dynamics {
    pub threshold_db: f64,
    pub ratio: f64,
    pub knee_db: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub makeup_db: f64,
    /// Maximum attenuation for the expander; ignored by the compressor.
    pub range_db: f64,
}

/// Caller admission, never inferred from a CPU estimate or a product channel cap.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_block_frames: usize,
    pub max_channels: usize,
    pub max_effects: usize,
    /// Retained requested payload, including config capacities and struct storage;
    /// excludes allocator bookkeeping and caller-owned input buffers.
    pub max_memory_bytes: usize,
    /// Conservative abstract work accounting, NOT nanoseconds or realtime proof.
    pub max_work_units_per_block: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileError {
    InvalidFormat,
    InvalidName,
    InvalidParameter {
        effect: usize,
        parameter: &'static str,
    },
    ResourceOverflow,
    Budget {
        resource: &'static str,
        required: usize,
        limit: usize,
    },
    Allocation,
}
impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CompileError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessError {
    FrameCount,
    SampleCount,
    NonFiniteInput { sample: usize },
    NonFiniteResult { effect: usize },
}
impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProcessError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Tail {
    Finite { frames: usize },
    Infinite,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Admission {
    pub memory_bytes: usize,
    pub work_units_per_block: usize,
    /// No built-in buffers a block or uses look-ahead. Filter group delay is not 0.
    pub algorithmic_latency_frames: usize,
    /// Conservative sum of explicit taps, including delays on independent lanes.
    /// Dry paths may also pass immediately; this is not per-channel group delay.
    pub explicit_delay_frames: usize,
    pub tail: Tail,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meter {
    pub peak: f64,
    pub rms: f64,
    pub over_full_scale_samples: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockReport {
    pub frames: usize,
    pub over_full_scale_samples: usize,
    pub bypassed: bool,
}

struct Prepared {
    lanes: Box<[usize]>,
    state: State,
}
struct CompiledRoute {
    output: usize,
    inputs: Box<[(usize, f64)]>,
}

enum State {
    Stateless,
    Matrix {
        routes: Box<[CompiledRoute]>,
        scratch: Box<[f64]>,
    },
    Biquad {
        coefficients: [f64; 5],
        history: Box<[f64]>,
    },
    Delay {
        history: Box<[f64]>,
        position: usize,
    },
    Dynamics {
        envelope: Box<[f64]>,
        attack: f64,
        release: f64,
    },
    Fir {
        history: Box<[f64]>,
        position: usize,
    },
}
impl State {
    fn reset(&mut self) {
        match self {
            Self::Stateless => {}
            Self::Matrix { scratch, .. } => scratch.fill(0.0),
            Self::Biquad { history, .. } => history.fill(0.0),
            Self::Delay { history, position } | Self::Fir { history, position } => {
                history.fill(0.0);
                *position = 0;
            }
            Self::Dynamics { envelope, .. } => envelope.fill(0.0),
        }
    }
}

pub struct Chain {
    config: ChainConfig,
    states: Box<[Prepared]>,
    output: Box<[f64]>,
    meters: Box<[Meter]>,
    max_block_frames: usize,
    valid_samples: usize,
    bypassed: bool,
    admission: Admission,
}

fn add(a: usize, b: usize) -> Result<usize, CompileError> {
    a.checked_add(b).ok_or(CompileError::ResourceOverflow)
}
fn mul(a: usize, b: usize) -> Result<usize, CompileError> {
    a.checked_mul(b).ok_or(CompileError::ResourceOverflow)
}
fn budget(resource: &'static str, required: usize, limit: usize) -> Result<(), CompileError> {
    if required > limit {
        Err(CompileError::Budget {
            resource,
            required,
            limit,
        })
    } else {
        Ok(())
    }
}
fn storage<T: Clone>(len: usize, value: T) -> Result<Box<[T]>, CompileError> {
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| CompileError::Allocation)?;
    v.resize(len, value);
    Ok(v.into_boxed_slice())
}
fn parameter(index: usize, parameter: &'static str) -> CompileError {
    CompileError::InvalidParameter {
        effect: index,
        parameter,
    }
}

impl Chain {
    pub fn compile(config: ChainConfig, limits: Limits) -> Result<Self, CompileError> {
        let channels = config.format.channels.len();
        if channels == 0 || config.format.sample_rate_hz == 0 || limits.max_block_frames == 0 {
            return Err(CompileError::InvalidFormat);
        }
        budget("channels", channels, limits.max_channels)?;
        budget("effects", config.effects.len(), limits.max_effects)?;
        for (i, channel) in config.format.channels.iter().enumerate() {
            if channel.trim().is_empty() || config.format.channels[..i].contains(channel) {
                return Err(CompileError::InvalidFormat);
            }
        }
        if config.name.trim().is_empty() || config.effects.iter().any(|e| e.name.trim().is_empty())
        {
            return Err(CompileError::InvalidName);
        }
        let samples = mul(channels, limits.max_block_frames)?;
        let mut memory = add(size_of::<Self>(), mul(samples, size_of::<f64>())?)?;
        memory = add(memory, mul(channels, size_of::<Meter>())?)?;
        memory = add(memory, config.name.capacity())?;
        memory = add(
            memory,
            mul(config.format.channels.capacity(), size_of::<String>())?,
        )?;
        for id in &config.format.channels {
            memory = add(memory, id.capacity())?;
        }
        memory = add(
            memory,
            mul(config.effects.capacity(), size_of::<EffectConfig>())?,
        )?;
        memory = add(memory, mul(config.effects.len(), size_of::<Prepared>())?)?;
        // Baseline accounts for buffer clearing, input checks, copy and metering.
        let mut per_frame_work = mul(channels, 16)?;
        let mut reset_samples = 0;
        let mut explicit_delay_frames = 0;
        let mut tail = Tail::Finite { frames: 0 };
        for (index, effect) in config.effects.iter().enumerate() {
            memory = add(memory, effect.name.capacity())?;
            validate_channels(effect, &config.format.channels, index)?;
            let selected = effect.channels.as_ref().map_or(channels, Vec::len);
            if let Some(ids) = &effect.channels {
                memory = add(memory, mul(ids.capacity(), size_of::<String>())?)?;
                for id in ids {
                    memory = add(memory, id.capacity())?;
                }
            }
            if let Effect::ChannelMatrix { routes } = &effect.effect {
                memory = add(memory, mul(routes.capacity(), size_of::<MatrixRoute>())?)?;
                for route in routes {
                    memory = add(memory, route.output.capacity())?;
                    memory = add(
                        memory,
                        mul(route.inputs.capacity(), size_of::<MatrixInput>())?,
                    )?;
                    for input in &route.inputs {
                        memory = add(memory, input.channel.capacity())?;
                    }
                }
                if effect.enabled {
                    memory = add(memory, mul(routes.len(), size_of::<CompiledRoute>())?)?;
                    memory = add(memory, mul(channels, size_of::<f64>())?)?;
                    reset_samples = add(reset_samples, channels)?;
                    for route in routes {
                        memory = add(memory, mul(route.inputs.len(), size_of::<(usize, f64)>())?)?;
                    }
                }
            } else if effect.enabled {
                memory = add(memory, mul(selected, size_of::<usize>())?)?;
            }
            let (state_samples, work, effect_tail) =
                validate(&effect.effect, index, config.format.sample_rate_hz)?;
            if let Effect::Fir { coefficients } = &effect.effect {
                memory = add(memory, mul(coefficients.capacity(), size_of::<f64>())?)?;
            }
            if effect.enabled {
                memory = add(
                    memory,
                    mul(mul(state_samples, selected)?, size_of::<f64>())?,
                )?;
                let effect_work = if matches!(effect.effect, Effect::ChannelMatrix { .. }) {
                    add(channels, work)?
                } else {
                    mul(selected, work)?
                };
                per_frame_work = add(per_frame_work, effect_work)?;
                reset_samples = add(reset_samples, mul(state_samples, selected)?)?;
                if let Effect::Delay { frames, .. } = effect.effect {
                    explicit_delay_frames = add(explicit_delay_frames, frames)?;
                }
                tail = match (tail, effect_tail) {
                    (Tail::Finite { frames: a }, Tail::Finite { frames: b }) => {
                        Tail::Finite { frames: add(a, b)? }
                    }
                    _ => Tail::Infinite,
                };
            }
        }
        let work = add(
            add(mul(limits.max_block_frames, per_frame_work)?, reset_samples)?,
            config.effects.len(),
        )?;
        budget("memoryBytes", memory, limits.max_memory_bytes)?;
        budget("workUnitsPerBlock", work, limits.max_work_units_per_block)?;
        let mut states = Vec::new();
        states
            .try_reserve_exact(config.effects.len())
            .map_err(|_| CompileError::Allocation)?;
        for e in &config.effects {
            let mut lanes = Vec::new();
            if e.enabled && !matches!(e.effect, Effect::ChannelMatrix { .. }) {
                let count = e.channels.as_ref().map_or(channels, Vec::len);
                lanes
                    .try_reserve_exact(count)
                    .map_err(|_| CompileError::Allocation)?;
                if let Some(ids) = &e.channels {
                    for id in ids {
                        lanes.push(
                            config
                                .format
                                .channels
                                .iter()
                                .position(|c| c == id)
                                .expect("validated channel"),
                        );
                    }
                } else {
                    lanes.extend(0..channels);
                }
            }
            let selected = lanes.len();
            let state = if !e.enabled {
                State::Stateless
            } else {
                match &e.effect {
                    Effect::Gain { .. } => State::Stateless,
                    Effect::ChannelMatrix { routes } => {
                        let mut compiled = Vec::new();
                        compiled
                            .try_reserve_exact(routes.len())
                            .map_err(|_| CompileError::Allocation)?;
                        for route in routes {
                            let mut inputs = Vec::new();
                            inputs
                                .try_reserve_exact(route.inputs.len())
                                .map_err(|_| CompileError::Allocation)?;
                            for input in &route.inputs {
                                inputs.push((
                                    config
                                        .format
                                        .channels
                                        .iter()
                                        .position(|c| c == &input.channel)
                                        .expect("validated input"),
                                    input.gain,
                                ));
                            }
                            compiled.push(CompiledRoute {
                                output: config
                                    .format
                                    .channels
                                    .iter()
                                    .position(|c| c == &route.output)
                                    .expect("validated output"),
                                inputs: inputs.into_boxed_slice(),
                            });
                        }
                        State::Matrix {
                            routes: compiled.into_boxed_slice(),
                            scratch: storage(channels, 0.0)?,
                        }
                    }
                    Effect::Biquad {
                        filter,
                        frequency_hz,
                        q,
                        gain_db,
                    } => State::Biquad {
                        coefficients: filters::coefficients(
                            *filter,
                            *frequency_hz,
                            *q,
                            *gain_db,
                            config.format.sample_rate_hz,
                        )
                        .expect("validated coefficients"),
                        history: storage(mul(selected, 4)?, 0.0)?,
                    },
                    Effect::Delay { frames, .. } => State::Delay {
                        history: storage(mul(selected, *frames)?, 0.0)?,
                        position: 0,
                    },
                    Effect::Fir { coefficients } => State::Fir {
                        history: storage(mul(selected, coefficients.len())?, 0.0)?,
                        position: 0,
                    },
                    Effect::Compressor { parameters } | Effect::Expander { parameters } => {
                        State::Dynamics {
                            envelope: storage(selected, 0.0)?,
                            attack: time_coefficient(
                                parameters.attack_ms,
                                config.format.sample_rate_hz,
                            ),
                            release: time_coefficient(
                                parameters.release_ms,
                                config.format.sample_rate_hz,
                            ),
                        }
                    }
                }
            };
            states.push(Prepared {
                lanes: lanes.into_boxed_slice(),
                state,
            });
        }
        Ok(Self {
            config,
            states: states.into_boxed_slice(),
            output: storage(samples, 0.0)?,
            meters: storage(channels, Meter::default())?,
            max_block_frames: limits.max_block_frames,
            valid_samples: 0,
            bypassed: false,
            admission: Admission {
                memory_bytes: memory,
                work_units_per_block: work,
                algorithmic_latency_frames: 0,
                explicit_delay_frames,
                tail,
            },
        })
    }

    pub fn config(&self) -> &ChainConfig {
        &self.config
    }
    pub fn admission(&self) -> Admission {
        self.admission
    }
    /// Last successful block only; empty following reset or any processing error.
    pub fn output(&self) -> &[f64] {
        &self.output[..self.valid_samples]
    }
    /// One meter per explicitly ordered channel, for the last successful block.
    pub fn meters(&self) -> &[Meter] {
        &self.meters
    }
    pub fn is_bypassed(&self) -> bool {
        self.bypassed
    }

    /// Privacy/loss reset: discard all tails and previously published samples.
    pub fn reset(&mut self) {
        for s in &mut self.states {
            s.state.reset();
        }
        self.output.fill(0.0);
        self.meters.fill(Meter::default());
        self.valid_samples = 0;
    }

    /// Immediate, unsmoothed transition. Changing bypass resets tails in both
    /// directions; old PCM cannot reappear on re-enable. Can audibly click.
    pub fn set_bypassed(&mut self, bypassed: bool) {
        if self.bypassed != bypassed {
            self.reset();
            self.bypassed = bypassed;
        }
    }

    pub fn process(&mut self, frames: usize, input: &[f64]) -> Result<BlockReport, ProcessError> {
        match self.process_inner(frames, input) {
            Ok(report) => Ok(report),
            Err(error) => {
                self.reset();
                Err(error)
            }
        }
    }

    fn process_inner(&mut self, frames: usize, input: &[f64]) -> Result<BlockReport, ProcessError> {
        if frames == 0 || frames > self.max_block_frames {
            return Err(ProcessError::FrameCount);
        }
        let channels = self.config.format.channels.len();
        // This product was checked during compilation at max_block_frames.
        let samples = frames * channels;
        if input.len() != samples {
            return Err(ProcessError::SampleCount);
        }
        for (sample, x) in input.iter().enumerate() {
            if !x.is_finite() {
                return Err(ProcessError::NonFiniteInput { sample });
            }
        }
        self.output.fill(0.0);
        self.output[..samples].copy_from_slice(input);
        if !self.bypassed {
            for (index, (effect, state)) in self
                .config
                .effects
                .iter()
                .zip(self.states.iter_mut())
                .enumerate()
            {
                if effect.enabled
                    && !run(
                        &effect.effect,
                        &mut state.state,
                        &state.lanes,
                        channels,
                        &mut self.output[..samples],
                    )
                {
                    return Err(ProcessError::NonFiniteResult { effect: index });
                }
            }
        }
        let mut over = 0;
        for (ch, meter) in self.meters.iter_mut().enumerate() {
            *meter = Meter::default();
            // Scaled sum of squares cannot overflow on finite over-full-scale PCM.
            let mut sum = 0.0;
            for frame in 0..frames {
                let x = self.output[frame * channels + ch].abs();
                if x > 1.0 {
                    meter.over_full_scale_samples += 1;
                }
                if x > meter.peak {
                    let ratio = meter.peak / x;
                    sum = 1.0 + sum * ratio * ratio;
                    meter.peak = x;
                } else if meter.peak > 0.0 {
                    let ratio = x / meter.peak;
                    sum += ratio * ratio;
                }
            }
            meter.rms = meter.peak * (sum / frames as f64).min(1.0).sqrt();
            over += meter.over_full_scale_samples;
        }
        self.valid_samples = samples;
        Ok(BlockReport {
            frames,
            over_full_scale_samples: over,
            bypassed: self.bypassed,
        })
    }
}

fn validate_channels(
    effect: &EffectConfig,
    format: &[String],
    index: usize,
) -> Result<(), CompileError> {
    if let Effect::ChannelMatrix { routes } = &effect.effect {
        if effect.channels.is_some() {
            return Err(parameter(index, "channels"));
        }
        if routes.len() != format.len() {
            return Err(parameter(index, "routes"));
        }
        for (i, route) in routes.iter().enumerate() {
            if !format.contains(&route.output)
                || routes[..i].iter().any(|r| r.output == route.output)
            {
                return Err(parameter(index, "routes.output"));
            }
            for (j, input) in route.inputs.iter().enumerate() {
                if !format.contains(&input.channel)
                    || route.inputs[..j].iter().any(|v| v.channel == input.channel)
                {
                    return Err(parameter(index, "routes.inputs.channel"));
                }
                if !input.gain.is_finite() {
                    return Err(parameter(index, "routes.inputs.gain"));
                }
            }
        }
    } else if let Some(ids) = &effect.channels {
        if ids.is_empty()
            || ids
                .iter()
                .enumerate()
                .any(|(i, id)| !format.contains(id) || ids[..i].contains(id))
        {
            return Err(parameter(index, "channels"));
        }
    }
    Ok(())
}

fn validate(
    effect: &Effect,
    index: usize,
    rate: u32,
) -> Result<(usize, usize, Tail), CompileError> {
    let finite_tail = |frames| Tail::Finite { frames };
    match effect {
        Effect::ChannelMatrix { routes } => {
            let mut work = routes.len();
            for route in routes {
                work = add(work, mul(route.inputs.len(), 3)?)?;
            }
            Ok((0, add(work, routes.len())?, finite_tail(0)))
        }
        Effect::Gain { gain, .. } => {
            if !gain.is_finite() || *gain < 0.0 {
                return Err(parameter(index, "gain"));
            }
            Ok((0, 1, finite_tail(0)))
        }
        Effect::Biquad {
            filter,
            frequency_hz,
            q,
            gain_db,
        } => {
            filters::coefficients(*filter, *frequency_hz, *q, *gain_db, rate)
                .map_err(|p| parameter(index, p))?;
            Ok((4, 9, Tail::Infinite))
        }
        Effect::Delay {
            frames,
            wet,
            dry,
            feedback,
        } => {
            if *frames == 0 {
                return Err(parameter(index, "frames"));
            }
            for (name, value) in [("wet", wet), ("dry", dry)] {
                if !value.is_finite() || !(0.0..=1.0).contains(value) {
                    return Err(parameter(index, name));
                }
            }
            if !feedback.is_finite() || feedback.abs() >= 1.0 {
                return Err(parameter(index, "feedback"));
            }
            Ok((
                *frames,
                4,
                if *feedback == 0.0 {
                    finite_tail(*frames)
                } else {
                    Tail::Infinite
                },
            ))
        }
        Effect::Fir { coefficients } => {
            if coefficients.is_empty() || coefficients.iter().any(|c| !c.is_finite()) {
                return Err(parameter(index, "coefficients"));
            }
            Ok((
                coefficients.len(),
                mul(coefficients.len(), 2)?,
                finite_tail(coefficients.len() - 1),
            ))
        }
        Effect::Compressor { parameters: p } | Effect::Expander { parameters: p } => {
            for (name, v) in [("thresholdDb", p.threshold_db), ("makeupDb", p.makeup_db)] {
                if !v.is_finite() || v.abs() > 6000.0 {
                    return Err(parameter(index, name));
                }
            }
            for (name, v) in [("kneeDb", p.knee_db), ("rangeDb", p.range_db)] {
                if !v.is_finite() || !(0.0..=6000.0).contains(&v) {
                    return Err(parameter(index, name));
                }
            }
            if !p.ratio.is_finite() || p.ratio < 1.0 {
                return Err(parameter(index, "ratio"));
            }
            for (name, v) in [("attackMs", p.attack_ms), ("releaseMs", p.release_ms)] {
                if !v.is_finite() || v < 0.0 {
                    return Err(parameter(index, name));
                }
            }
            Ok((1, 32, finite_tail(0)))
        }
    }
}

fn time_coefficient(ms: f64, rate: u32) -> f64 {
    if ms == 0.0 {
        0.0
    } else {
        (-1000.0 / rate as f64 / ms).exp()
    }
}

fn dynamics_reduction_db(level: f64, p: &Dynamics, compressor: bool) -> f64 {
    let delta = level - p.threshold_db;
    let half = p.knee_db / 2.0;
    if compressor {
        let above = if delta <= -half {
            0.0
        } else if delta >= half {
            delta
        } else {
            let x = delta + half;
            x * (x / (2.0 * p.knee_db))
        };
        above * (1.0 - 1.0 / p.ratio)
    } else {
        let below = if delta >= half {
            0.0
        } else if delta <= -half {
            -delta
        } else {
            let x = delta - half;
            x * (x / (2.0 * p.knee_db))
        };
        let slope = p.ratio - 1.0;
        // Clamp attenuation before multiplying a potentially very large ratio.
        if slope == 0.0 {
            0.0
        } else if below >= p.range_db / slope {
            p.range_db
        } else {
            below * slope
        }
    }
}

fn apply_db_gain(sample: f64, gain_db: f64) -> f64 {
    if sample == 0.0 || gain_db == 0.0 {
        return sample;
    }
    let gain = 10f64.powf(gain_db / 20.0);
    if gain.is_normal() {
        return sample * gain;
    }
    // A gain can underflow even when the final amplitude is normal. Compute the
    // combined exponent only on that path; ordinary levels keep normal multiply
    // rounding, and cancelling attenuation/makeup preserves input bits exactly.
    10f64
        .powf(sample.abs().log10() + gain_db / 20.0)
        .copysign(sample)
}

fn run(
    effect: &Effect,
    state: &mut State,
    lanes: &[usize],
    channels: usize,
    block: &mut [f64],
) -> bool {
    match (effect, state) {
        (Effect::ChannelMatrix { .. }, State::Matrix { routes, scratch }) => {
            for frame in block.chunks_exact_mut(channels) {
                scratch.copy_from_slice(frame);
                for route in routes.iter() {
                    let y = if let [(input, gain)] = &*route.inputs {
                        if *gain == 1.0 {
                            scratch[*input]
                        } else {
                            scratch[*input] * *gain
                        }
                    } else {
                        let mut sum = 0.0;
                        for (input, gain) in &route.inputs {
                            let term = scratch[*input] * *gain;
                            sum += term;
                            if !term.is_finite() || !sum.is_finite() {
                                return false;
                            }
                        }
                        sum
                    };
                    if !y.is_finite() {
                        return false;
                    }
                    frame[route.output] = y;
                }
            }
        }

        (
            Effect::Gain {
                gain,
                invert_polarity,
            },
            _,
        ) => {
            // An explicit unity gain also preserves signed zero and f64 low bits.
            if *gain == 1.0 && !invert_polarity {
                return true;
            }
            let gain = if *invert_polarity { -*gain } else { *gain };
            for frame in block.chunks_exact_mut(channels) {
                for &lane in lanes {
                    frame[lane] *= gain;
                    if !frame[lane].is_finite() {
                        return false;
                    }
                }
            }
        }
        (
            Effect::Biquad { .. },
            State::Biquad {
                coefficients: c,
                history,
            },
        ) => {
            for frame in block.chunks_exact_mut(channels) {
                for (ch, &lane) in lanes.iter().enumerate() {
                    let x = &mut frame[lane];
                    let h = &mut history[ch * 4..ch * 4 + 4];
                    let y = c[0] * *x + c[1] * h[0] + c[2] * h[1] - c[3] * h[2] - c[4] * h[3];
                    if !y.is_finite() {
                        return false;
                    }
                    h[1] = h[0];
                    h[0] = *x;
                    h[3] = h[2];
                    h[2] = y;
                    *x = y;
                }
            }
        }
        (
            Effect::Delay {
                wet, dry, feedback, ..
            },
            State::Delay { history, position },
        ) => {
            for frame in block.chunks_exact_mut(channels) {
                for &lane in lanes {
                    let x = &mut frame[lane];
                    let delayed = history[*position];
                    let retained = *x + *feedback * delayed;
                    let y = *dry * *x + *wet * delayed;
                    if !retained.is_finite() || !y.is_finite() {
                        return false;
                    }
                    history[*position] = retained;
                    *x = y;
                    *position += 1;
                    if *position == history.len() {
                        *position = 0;
                    }
                }
            }
        }

        (Effect::Fir { coefficients }, State::Fir { history, position }) => {
            let taps = coefficients.len();
            for frame in block.chunks_exact_mut(channels) {
                for (ch, &lane) in lanes.iter().enumerate() {
                    let x = &mut frame[lane];
                    history[*position * lanes.len() + ch] = *x;
                    let mut y = 0.0;
                    let mut slot = *position;
                    for coefficient in coefficients {
                        y += *coefficient * history[slot * lanes.len() + ch];
                        slot = if slot == 0 { taps - 1 } else { slot - 1 };
                    }
                    if !y.is_finite() {
                        return false;
                    }
                    *x = y;
                }
                *position += 1;
                if *position == taps {
                    *position = 0;
                }
            }
        }
        (
            Effect::Compressor { parameters: p } | Effect::Expander { parameters: p },
            State::Dynamics {
                envelope,
                attack,
                release,
            },
        ) => {
            let compressor = matches!(effect, Effect::Compressor { .. });
            for frame in block.chunks_exact_mut(channels) {
                for (ch, &lane) in lanes.iter().enumerate() {
                    let x = &mut frame[lane];
                    let magnitude = x.abs();
                    let coefficient = if magnitude > envelope[ch] {
                        *attack
                    } else {
                        *release
                    };
                    envelope[ch] = coefficient * envelope[ch] + (1.0 - coefficient) * magnitude;
                    if !envelope[ch].is_finite() {
                        return false;
                    }
                    let level = if envelope[ch] == 0.0 {
                        -6500.0
                    } else {
                        20.0 * envelope[ch].log10()
                    };
                    let reduction = dynamics_reduction_db(level, p, compressor);
                    *x = apply_db_gain(*x, p.makeup_db - reduction);
                    if !x.is_finite() {
                        return false;
                    }
                }
            }
        }
        _ => unreachable!("compiled state matches enabled effect"),
    }
    true
}
