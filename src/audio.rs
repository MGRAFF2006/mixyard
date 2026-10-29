//! Bounded synchronous Float32/Float64 PCM processing, compiled from the policy planner.
//!
//! The processor performs no device transport, clock adaptation, resampling,
//! limiter or normalization. Explicit destination adaptation lives in `clock`.
//! See `docs/audio-core.md` for the adapter contract and qualification boundary.

use crate::{plan, AudioFormat, Config, Inventory, Plan, ResolutionStatus};

pub mod clock;
pub mod pcm;

mod sealed {
    pub trait Sealed {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

/// Explicit storage/processing precision. Native format negotiation is separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    Float32,
    Float64,
}

/// Sealed to the two supported IEEE754 storage types. Both mix in Float64.
pub trait Sample: sealed::Sealed + Copy + std::fmt::Debug {
    const PRECISION: Precision;
    fn widen(self) -> f64;
    fn from_f64(value: f64) -> Self;
}

impl Sample for f32 {
    const PRECISION: Precision = Precision::Float32;
    fn widen(self) -> f64 {
        self as f64
    }
    fn from_f64(value: f64) -> Self {
        value as f32
    }
}

impl Sample for f64 {
    const PRECISION: Precision = Precision::Float64;
    fn widen(self) -> f64 {
        self
    }
    fn from_f64(value: f64) -> Self {
        value
    }
}

/// Host-chosen ceilings, checked before allocating render storage. These are
/// resource budgets, not product mix-count limits. There are no implicit defaults.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_frames: usize,
    pub max_sources: usize,
    pub max_mixes: usize,
    pub max_sends: usize,
    pub max_outputs: usize,
    /// Total scalar storage slots: Float64 accumulators + selected-precision mix/output
    /// buffers. Each slot costs at most eight bytes. Input PCM remains borrowed.
    pub max_buffer_samples: usize,
}

/// Interleaved IEEE754 PCM in one compiled source slot: Float32 by default,
/// or explicitly `Pcm<f64>`. Supplying None means fresh silence for this block.
#[derive(Clone, Copy, Debug)]
pub struct Pcm<'a, S = f32> {
    pub format: AudioFormat,
    pub samples: &'a [S],
}

/// Render errors contain only bounded numeric metadata and never allocate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    FrameCount,
    SourceCount,
    DisconnectedSource {
        source: usize,
    },
    FormatMismatch {
        source: usize,
    },
    SampleCount {
        source: usize,
    },
    NonFinite {
        source: usize,
        sample: usize,
    },
    /// Finite input overflowed processing or the selected output precision.
    NonFiniteResult,
    MixStageClosed,
    MixCount,
    MixFormat {
        mix: usize,
    },
    MixSampleCount {
        mix: usize,
    },
    MixNonFinite {
        mix: usize,
        sample: usize,
    },
}

#[derive(Clone, Copy, Debug)]
enum Mapping {
    Mono,
    Stereo,
    DuplicateMono,
    AverageLeftRight,
}

impl Mapping {
    // Consume the authoritative planner's mapping; do not choose routing or
    // channel policy independently here.
    fn compile(mapping: &str) -> Result<Self, String> {
        match mapping {
            "mono" => Ok(Self::Mono),
            "stereo" => Ok(Self::Stereo),
            "duplicate-mono" => Ok(Self::DuplicateMono),
            "average-left-right" => Ok(Self::AverageLeftRight),
            _ => Err(format!("audio-core: unsupported channel mapping {mapping}")),
        }
    }

    fn sample<S: Sample>(self, data: &[S], frame: usize, channel: usize) -> f64 {
        match self {
            Self::Mono | Self::DuplicateMono => data[frame].widen(),
            Self::Stereo => data[frame * 2 + channel].widen(),
            Self::AverageLeftRight => {
                let left = data[frame * 2].widen();
                let right = data[frame * 2 + 1].widen();
                let sum = left + right;
                // Keep ordinary/subnormal rounding, but avoid overflowing a
                // representable average of two extreme finite Float64 values.
                if sum.is_finite() {
                    sum * 0.5
                } else {
                    left * 0.5 + right * 0.5
                }
            }
        }
    }
}

#[derive(Debug)]
struct Send {
    source: usize,
    mix: usize,
    gain: f64,
    mapping: Mapping,
}

#[derive(Debug)]
struct Output {
    mix: usize,
    gain: f64,
    mapping: Option<Mapping>,
}

#[derive(Debug)]
struct Buffer<S> {
    format: AudioFormat,
    samples: Vec<S>,
}

/// Caller-budgeted source storage for a native host. Slot identities follow the
/// processor's immutable plan; the host still owns endpoint/authority binding.
pub struct InputBank<S: Sample> {
    slots: Vec<Option<Buffer<S>>>,
    present: Vec<bool>,
    frames: Vec<usize>,
    max_frames: usize,
}
impl<S: Sample> InputBank<S> {
    pub fn clear(&mut self) {
        self.present.fill(false);
        self.frames.fill(0);
        for slot in self.slots.iter_mut().flatten() {
            slot.samples.fill(S::from_f64(0.0));
        }
    }
    pub fn set(
        &mut self,
        source: usize,
        frames: usize,
        samples: Option<&[S]>,
    ) -> Result<(), RenderError> {
        let result = self.set_inner(source, frames, samples);
        if result.is_err() {
            self.clear();
        }
        result
    }
    fn set_inner(
        &mut self,
        source: usize,
        frames: usize,
        samples: Option<&[S]>,
    ) -> Result<(), RenderError> {
        if source >= self.slots.len() {
            return Err(RenderError::SourceCount);
        }
        self.present[source] = false;
        self.frames[source] = 0;
        if frames == 0 || frames > self.max_frames {
            return Err(RenderError::FrameCount);
        }
        let Some(samples) = samples else {
            if let Some(slot) = &mut self.slots[source] {
                slot.samples.fill(S::from_f64(0.0));
            }
            return Ok(());
        };
        let slot = self.slots[source]
            .as_mut()
            .ok_or(RenderError::DisconnectedSource { source })?;
        let count = frames * slot.format.channels as usize;
        if samples.len() != count {
            return Err(RenderError::SampleCount { source });
        }
        if let Some(sample) = samples.iter().position(|v| !v.widen().is_finite()) {
            return Err(RenderError::NonFinite { source, sample });
        }
        slot.samples[..count].copy_from_slice(samples);
        slot.samples[count..].fill(S::from_f64(0.0));
        self.frames[source] = frames;
        self.present[source] = true;
        Ok(())
    }
}
enum Inputs<'a, 'samples, S: Sample> {
    Borrowed(&'a [Option<Pcm<'samples, S>>]),
    Bank(&'a InputBank<S>),
}
impl<S: Sample> Inputs<'_, '_, S> {
    fn len(&self) -> usize {
        match self {
            Self::Borrowed(v) => v.len(),
            Self::Bank(v) => v.slots.len(),
        }
    }
    fn get(&self, index: usize) -> Option<Pcm<'_, S>> {
        match self {
            Self::Borrowed(v) => v[index],
            Self::Bank(v) => {
                if v.present[index] {
                    let slot = v.slots[index].as_ref()?;
                    Some(Pcm {
                        format: slot.format,
                        samples: &slot.samples[..v.frames[index] * slot.format.channels as usize],
                    })
                } else {
                    None
                }
            }
        }
    }
}

/// An immutable policy snapshot plus exclusively owned, preallocated buffers.
/// Compile and replace away from the audio callback. Render uses no I/O, locks,
/// heap allocation or deallocation. All slices use the order of `plan()`.
#[derive(Debug)]
pub struct TypedProcessor<S: Sample> {
    plan: Plan,
    max_frames: usize,
    frames: usize,
    sources: Vec<Option<AudioFormat>>,
    sends: Vec<Send>,
    outputs: Vec<Output>,
    mixes: Vec<Buffer<S>>,
    output_buffers: Vec<Buffer<S>>,
    accumulators: Vec<Vec<f64>>,
    initialized: Vec<bool>,
    mix_stage_open: bool,
}

/// Backwards-compatible Float32 input, mix and output storage.
pub type Processor = TypedProcessor<f32>;
/// Explicit Float64 input, mix and output storage; no Float32 intermediate.
pub type Processor64 = TypedProcessor<f64>;

impl<S: Sample> TypedProcessor<S> {
    /// Input, mix and output sample precision, separate from policy rate/layout.
    pub fn precision(&self) -> Precision {
        S::PRECISION
    }

    /// Validate through `crate::plan`, then compile its resolved edges and gains.
    /// All configured mixes and selected ports must share this positive nominal
    /// sample rate. Unknown selected formats and required conversion reject the
    /// compilation, even on muted routes. Missing/ambiguous ports stay silent.
    pub fn compile(
        config: &Config,
        inventory: &Inventory,
        sample_rate_hz: u32,
        limits: Limits,
    ) -> Result<Self, String> {
        if limits.max_frames == 0 || sample_rate_hz == 0 {
            return Err("audio-core: positive max_frames and sample rate required".into());
        }
        if limits.max_frames > isize::MAX as usize / 8 {
            return Err("audio-core: block size exceeds addressable render slice".into());
        }
        for (name, count, limit) in [
            ("sources", config.sources.len(), limits.max_sources),
            ("mixes", config.mixes.len(), limits.max_mixes),
            ("sends", config.sends.len(), limits.max_sends),
            ("outputs", config.outputs.len(), limits.max_outputs),
        ] {
            if count > limit {
                return Err(format!(
                    "audio-core: {count} {name} exceed caller budget {limit}"
                ));
            }
        }
        let plan = plan(config, inventory)?;
        let check_format = |format: Option<AudioFormat>, subject: &str| {
            let format = format.ok_or_else(|| {
                format!("audio-core: {subject}: selected format unknown; discover before compile")
            })?;
            format.validate()?;
            if format.sample_rate_hz != sample_rate_hz {
                return Err(format!(
                    "audio-core: {subject}: rate conversion/clock adaptation unsupported ({} != {sample_rate_hz} Hz)",
                    format.sample_rate_hz
                ));
            }
            Ok(format)
        };
        let sources = plan
            .sources
            .iter()
            .map(|source| {
                if source.status == ResolutionStatus::Selected {
                    check_format(source.format, &source.source).map(Some)
                } else {
                    Ok(None)
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mix_formats = plan
            .mixes
            .iter()
            .map(|mix| {
                check_format(
                    Some(AudioFormat {
                        channels: mix.channels,
                        sample_rate_hz: mix.sample_rate_hz,
                    }),
                    &mix.id,
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut sends = Vec::new();
        for send in &plan.sends {
            if send.observed_id.is_none() {
                continue;
            }
            sends.push(Send {
                source: plan
                    .sources
                    .iter()
                    .position(|s| s.source == send.source)
                    .unwrap(),
                mix: plan.mixes.iter().position(|m| m.id == send.mix).unwrap(),
                gain: send.effective_gain, // Already includes mix master/mute.
                mapping: Mapping::compile(send.channel_mapping)?,
            });
        }
        let mut output_formats = Vec::new();
        let mut outputs = Vec::new();
        for output in &plan.outputs {
            let mix = plan.mixes.iter().position(|m| m.id == output.mix).unwrap();
            let (format, mapping) = match output.status {
                ResolutionStatus::Selected => {
                    // The planner validated/deduplicated observed identities.
                    let observed = inventory
                        .outputs
                        .iter()
                        .find(|o| o.observed_id == output.observed_ids[0])
                        .unwrap();
                    (
                        check_format(observed.format, &output.id)?,
                        Some(Mapping::compile(output.channel_mapping)?),
                    )
                }
                ResolutionStatus::UnsupportedFormat => {
                    return Err(format!(
                        "audio-core: {}: unsupported output format",
                        output.id
                    ));
                }
                // No endpoint format can be inferred. Keep a nominal mix-layout
                // silence buffer, never authorization to submit to an endpoint.
                _ => (mix_formats[mix], None),
            };
            output_formats.push(format);
            outputs.push(Output {
                mix,
                gain: output.effective_gain,
                mapping,
            });
        }

        let mut slots = 0usize;
        for (format, copies) in mix_formats
            .iter()
            .map(|f| (f, 2usize))
            .chain(output_formats.iter().map(|f| (f, 1usize)))
        {
            slots = limits
                .max_frames
                .checked_mul(format.channels as usize)
                .and_then(|n| n.checked_mul(copies))
                .and_then(|n| slots.checked_add(n))
                .ok_or("audio-core: buffer size overflow")?;
        }
        if slots > limits.max_buffer_samples
            || slots.checked_mul(8).is_none()
            || slots > isize::MAX as usize / 8
        {
            return Err(
                "audio-core: render storage exceeds caller budget/addressable allocation".into(),
            );
        }
        let buffer = |format: AudioFormat| Buffer {
            format,
            samples: vec![S::from_f64(0.0); limits.max_frames * format.channels as usize],
        };
        let accumulators = mix_formats
            .iter()
            .map(|f| vec![0.0; limits.max_frames * f.channels as usize])
            .collect();
        let initialized = vec![false; plan.mixes.len()];
        Ok(Self {
            plan,
            max_frames: limits.max_frames,
            frames: 0,
            sources,
            sends,
            outputs,
            mixes: mix_formats.into_iter().map(buffer).collect(),
            output_buffers: output_formats.into_iter().map(buffer).collect(),
            accumulators,
            initialized,
            mix_stage_open: false,
        })
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }
    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn input_bank(&self, max_samples: usize) -> Result<InputBank<S>, String> {
        self.make_bank(&self.sources, max_samples)
    }
    pub fn mix_bank(&self, max_samples: usize) -> Result<InputBank<S>, String> {
        let formats: Vec<_> = self.mixes.iter().map(|b| Some(b.format)).collect();
        self.make_bank(&formats, max_samples)
    }
    fn make_bank(
        &self,
        formats: &[Option<AudioFormat>],
        max_samples: usize,
    ) -> Result<InputBank<S>, String> {
        let count = formats
            .iter()
            .flatten()
            .try_fold(0usize, |sum, f| {
                self.max_frames
                    .checked_mul(f.channels as usize)
                    .and_then(|n| sum.checked_add(n))
            })
            .ok_or("audio-core: source bank size overflow")?;
        if count > max_samples || count > isize::MAX as usize / std::mem::size_of::<S>() {
            return Err("audio-core: source bank budget exceeded".into());
        }
        Ok(InputBank {
            slots: formats
                .iter()
                .map(|format| {
                    format.map(|format| Buffer {
                        format,
                        samples: vec![S::from_f64(0.0); self.max_frames * format.channels as usize],
                    })
                })
                .collect(),
            present: vec![false; formats.len()],
            frames: vec![0; formats.len()],
            max_frames: self.max_frames,
        })
    }
    pub fn render_bank(&mut self, frames: usize, inputs: &InputBank<S>) -> Result<(), RenderError> {
        self.render_inputs(frames, Inputs::Bank(inputs))
    }

    /// One optional post-mix processing stage per successful render. Rebuilds
    /// output mapping/gain using the same compiled policy, never a second graph.
    /// Validate all replacements before publishing any; failure clears all PCM.
    pub fn apply_mix_results(
        &mut self,
        replacements: &[Option<Pcm<'_, S>>],
    ) -> Result<(), RenderError> {
        self.apply_mix_inputs(Inputs::Borrowed(replacements))
    }
    pub fn apply_mix_bank(&mut self, replacements: &InputBank<S>) -> Result<(), RenderError> {
        self.apply_mix_inputs(Inputs::Bank(replacements))
    }
    fn apply_mix_inputs(&mut self, replacements: Inputs<'_, '_, S>) -> Result<(), RenderError> {
        let result = self.apply_mix_results_inner(&replacements);
        self.mix_stage_open = false;
        if result.is_err() {
            self.clear_storage();
        }
        result
    }
    fn apply_mix_results_inner(
        &mut self,
        replacements: &Inputs<'_, '_, S>,
    ) -> Result<(), RenderError> {
        if !self.mix_stage_open {
            return Err(RenderError::MixStageClosed);
        }
        if replacements.len() != self.mixes.len() {
            return Err(RenderError::MixCount);
        }
        for index in 0..replacements.len() {
            let replacement = replacements.get(index);
            if let Some(pcm) = replacement {
                let format = self.mixes[index].format;
                if pcm.format != format {
                    return Err(RenderError::MixFormat { mix: index });
                }
                if pcm.samples.len() != self.frames * format.channels as usize {
                    return Err(RenderError::MixSampleCount { mix: index });
                }
                if let Some(sample) = pcm.samples.iter().position(|v| !v.widen().is_finite()) {
                    return Err(RenderError::MixNonFinite { mix: index, sample });
                }
            }
        }
        for (index, mix) in self.mixes.iter_mut().enumerate() {
            let replacement = replacements.get(index);
            if let Some(pcm) = replacement {
                mix.samples[..pcm.samples.len()].copy_from_slice(pcm.samples);
            }
        }
        for buffer in &mut self.output_buffers {
            buffer.samples.fill(S::from_f64(0.0));
        }
        self.render_outputs()
    }

    /// Selected output buffer layout. Missing/ambiguous ports return None;
    /// their silence samples use the owning mix layout for inspection only.
    /// Sample encoding is not in schema 2; see `precision()` for buffer precision.
    pub fn output_format(&self, output: usize) -> Option<AudioFormat> {
        self.outputs
            .get(output)?
            .mapping
            .map(|_| self.output_buffers[output].format)
    }

    pub fn mix_samples(&self, mix: usize) -> Option<&[S]> {
        self.mixes
            .get(mix)
            .map(|b| &b.samples[..self.frames * b.format.channels as usize])
    }

    pub fn output_samples(&self, output: usize) -> Option<&[S]> {
        self.output_buffers
            .get(output)
            .map(|b| &b.samples[..self.frames * b.format.channels as usize])
    }

    /// Process one synchronous block. `inputs` has exactly one slot per planned
    /// source; unresolved sources require None. Validate *all* supplied samples,
    /// even muted/unrouted sources, before producing any audible result.
    ///
    /// Every call clears all storage, including unused tails. On Err, every mix
    /// and output is silence and frames() == min(requested, max_frames). Never
    /// submit a failed block as successful audio. No previous PCM is retained.
    pub fn render(
        &mut self,
        frames: usize,
        inputs: &[Option<Pcm<'_, S>>],
    ) -> Result<(), RenderError> {
        self.render_inputs(frames, Inputs::Borrowed(inputs))
    }
    fn render_inputs(
        &mut self,
        frames: usize,
        inputs: Inputs<'_, '_, S>,
    ) -> Result<(), RenderError> {
        self.mix_stage_open = false;
        self.frames = frames.min(self.max_frames);
        self.clear_storage();
        let result = self.render_block(frames, &inputs);
        if result.is_err() {
            self.clear_storage();
        }
        self.mix_stage_open = result.is_ok();
        result
    }

    fn clear_storage(&mut self) {
        for mix in &mut self.mixes {
            mix.samples.fill(S::from_f64(0.0));
        }
        for output in &mut self.output_buffers {
            output.samples.fill(S::from_f64(0.0));
        }
        for accumulator in &mut self.accumulators {
            accumulator.fill(0.0);
        }
        self.initialized.fill(false);
    }

    fn render_block(
        &mut self,
        frames: usize,
        inputs: &Inputs<'_, '_, S>,
    ) -> Result<(), RenderError> {
        if frames == 0 || frames > self.max_frames {
            return Err(RenderError::FrameCount);
        }
        if inputs.len() != self.sources.len() {
            return Err(RenderError::SourceCount);
        }
        for source in 0..inputs.len() {
            let input = inputs.get(source);
            let Some(input) = input else {
                continue;
            };
            let format = self.sources[source].ok_or(RenderError::DisconnectedSource { source })?;
            if input.format != format {
                return Err(RenderError::FormatMismatch { source });
            }
            if input.samples.len() != frames * format.channels as usize {
                return Err(RenderError::SampleCount { source });
            }
            if let Some(sample) = input.samples.iter().position(|s| !s.widen().is_finite()) {
                return Err(RenderError::NonFinite { source, sample });
            }
        }
        for send in &self.sends {
            if send.gain == 0.0 {
                continue;
            }
            let Some(input) = inputs.get(send.source) else {
                continue;
            };
            let channels = self.mixes[send.mix].format.channels as usize;
            let accumulator = &mut self.accumulators[send.mix];
            for frame in 0..frames {
                for channel in 0..channels {
                    let sample = send.mapping.sample(input.samples, frame, channel) * send.gain;
                    let dest = &mut accumulator[frame * channels + channel];
                    if self.initialized[send.mix] {
                        *dest += sample;
                    } else {
                        *dest = sample;
                    }
                }
            }
            // Assign the first contribution, preserving signed zero on unity.
            self.initialized[send.mix] = true;
        }
        for (mix, accumulator) in self.mixes.iter_mut().zip(&self.accumulators) {
            for (dest, sample) in mix.samples[..frames * mix.format.channels as usize]
                .iter_mut()
                .zip(accumulator)
            {
                *dest = S::from_f64(*sample);
                if !dest.widen().is_finite() {
                    return Err(RenderError::NonFiniteResult);
                }
            }
        }
        self.render_outputs()
    }
    fn render_outputs(&mut self) -> Result<(), RenderError> {
        let frames = self.frames;
        for (output, buffer) in self.outputs.iter().zip(&mut self.output_buffers) {
            if output.gain == 0.0 {
                continue;
            }
            let Some(mapping) = output.mapping else {
                continue;
            };
            let channels = buffer.format.channels as usize;
            let input = &self.mixes[output.mix].samples;
            for frame in 0..frames {
                for channel in 0..channels {
                    let sample = S::from_f64(mapping.sample(input, frame, channel) * output.gain);
                    if !sample.widen().is_finite() {
                        return Err(RenderError::NonFiniteResult);
                    }
                    buffer.samples[frame * channels + channel] = sample;
                }
            }
        }
        Ok(())
    }
}
