//! Bounded synchronous Float32 PCM processing, compiled from the policy planner.
//!
//! No device transport, clock adaptation, resampling, limiter or normalization.
//! See `docs/audio-core.md` for the adapter contract and qualification boundary.

use crate::{plan, AudioFormat, Config, Inventory, Plan, ResolutionStatus};

/// Host-chosen ceilings, checked before allocating render storage. These are
/// resource budgets, not product mix-count limits. There are no implicit defaults.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_frames: usize,
    pub max_sources: usize,
    pub max_mixes: usize,
    pub max_sends: usize,
    pub max_outputs: usize,
    /// Total scalar storage slots: Float64 accumulators + Float32 mix/output
    /// buffers. Each slot costs at most eight bytes. Input PCM remains borrowed.
    pub max_buffer_samples: usize,
}

/// Interleaved IEEE754 Float32 PCM in one compiled source slot. Supplying None
/// instead means silence for this block, never replay of the preceding block.
#[derive(Clone, Copy, Debug)]
pub struct Pcm<'a> {
    pub format: AudioFormat,
    pub samples: &'a [f32],
}

/// Render errors contain only bounded numeric metadata and never allocate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    FrameCount,
    SourceCount,
    DisconnectedSource { source: usize },
    FormatMismatch { source: usize },
    SampleCount { source: usize },
    NonFinite { source: usize, sample: usize },
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

    fn sample(self, data: &[f32], frame: usize, channel: usize) -> f64 {
        match self {
            Self::Mono | Self::DuplicateMono => data[frame] as f64,
            Self::Stereo => data[frame * 2 + channel] as f64,
            // Widen before addition: finite full-scale Float32 must not overflow.
            Self::AverageLeftRight => (data[frame * 2] as f64 + data[frame * 2 + 1] as f64) * 0.5,
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
struct Buffer {
    format: AudioFormat,
    samples: Vec<f32>,
}

/// An immutable policy snapshot plus exclusively owned, preallocated buffers.
/// Compile and replace away from the audio callback. Render uses no I/O, locks,
/// heap allocation or deallocation. All slices use the order of `plan()`.
#[derive(Debug)]
pub struct Processor {
    plan: Plan,
    max_frames: usize,
    frames: usize,
    sources: Vec<Option<AudioFormat>>,
    sends: Vec<Send>,
    outputs: Vec<Output>,
    mixes: Vec<Buffer>,
    output_buffers: Vec<Buffer>,
    accumulators: Vec<Vec<f64>>,
    initialized: Vec<bool>,
}

impl Processor {
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
            return Err("audio-core: block size exceeds addressable stereo Float32 slice".into());
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
            samples: vec![0.0; limits.max_frames * format.channels as usize],
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

    /// Selected output buffer layout. Missing/ambiguous ports return None;
    /// their silence samples use the owning mix layout for inspection only.
    pub fn output_format(&self, output: usize) -> Option<AudioFormat> {
        self.outputs
            .get(output)?
            .mapping
            .map(|_| self.output_buffers[output].format)
    }

    pub fn mix_samples(&self, mix: usize) -> Option<&[f32]> {
        self.mixes
            .get(mix)
            .map(|b| &b.samples[..self.frames * b.format.channels as usize])
    }

    pub fn output_samples(&self, output: usize) -> Option<&[f32]> {
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
    pub fn render(&mut self, frames: usize, inputs: &[Option<Pcm<'_>>]) -> Result<(), RenderError> {
        self.frames = frames.min(self.max_frames);
        for mix in &mut self.mixes {
            mix.samples.fill(0.0);
        }
        for output in &mut self.output_buffers {
            output.samples.fill(0.0);
        }
        for accumulator in &mut self.accumulators {
            accumulator.fill(0.0);
        }
        self.initialized.fill(false);
        if frames == 0 || frames > self.max_frames {
            return Err(RenderError::FrameCount);
        }
        if inputs.len() != self.sources.len() {
            return Err(RenderError::SourceCount);
        }
        for (source, input) in inputs.iter().enumerate() {
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
            if let Some(sample) = input.samples.iter().position(|s| !s.is_finite()) {
                return Err(RenderError::NonFinite { source, sample });
            }
        }
        for send in &self.sends {
            if send.gain == 0.0 {
                continue;
            }
            let Some(input) = inputs[send.source] else {
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
                *dest = *sample as f32;
            }
        }
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
                    buffer.samples[frame * channels + channel] =
                        (mapping.sample(input, frame, channel) * output.gain) as f32;
                }
            }
        }
        Ok(())
    }
}
