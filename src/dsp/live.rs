//! Bounded, revisioned hosting for the existing Float64 processors.
//!
//! Compile and reclaim on a control thread. See `docs/dsp-live.md` for the
//! attachment, smoothing, admission and host lifecycle contract.

use super::{
    add, budget, mul, storage, Admission, Chain, ChainConfig, CompileError, Format, Limits,
    ProcessError,
};
use std::mem::size_of;

/// Graph-resolved opaque handle, never an OS device ID, name or permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attachment {
    /// Before sends; every send explicitly choosing this result hears the DSP.
    Source(u64),
    /// After the host has summed the independently authorized sends.
    Mix(u64),
    /// After fan-out, with an instance exclusive to this destination.
    Output(u64),
}

/// The host must change generation after restart, loss or authorization changes.
/// Revisions are local to this instance; neither field is an authorization grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub attachment: Attachment,
    pub generation: u64,
    pub revision: u64,
}

/// Explicit post-chain controls. Wet zero selects dry input but keeps DSP warm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Controls {
    pub gain: f64,
    pub wet: f64,
}

impl Controls {
    pub const UNITY: Self = Self {
        gain: 1.0,
        wet: 1.0,
    };

    fn valid(self) -> bool {
        self.gain.is_finite()
            && self.gain >= 0.0
            && self.wet.is_finite()
            && (0.0..=1.0).contains(&self.wet)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidBinding,
    InvalidControls,
    Inactive,
    BindingMismatch,
    FormatMismatch,
    Revision,
    TransitionFrames,
    Compile(CompileError),
    Process(ProcessError),
    NonFiniteResult { sample: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
impl From<CompileError> for Error {
    fn from(error: CompileError) -> Self {
        Self::Compile(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Report {
    pub binding: Binding,
    pub frames: usize,
    /// Counts the final wet/dry/gain output, without clipping or normalizing it.
    pub over_full_scale_samples: usize,
    /// The controls reached on the last frame of this block.
    pub controls: Controls,
    pub transition_remaining_frames: usize,
}

struct Ramp {
    start: f64,
    current: f64,
    target: f64,
    total: usize,
    remaining: usize,
}

impl Ramp {
    fn constant(value: f64) -> Self {
        Self {
            start: value,
            current: value,
            target: value,
            total: 0,
            remaining: 0,
        }
    }

    fn set(&mut self, target: f64, frames: usize) {
        self.start = self.current;
        self.target = target;
        self.total = frames;
        self.remaining = frames;
        if frames == 0 {
            self.current = target;
        }
    }

    fn advance(&mut self) -> f64 {
        if self.remaining != 0 {
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            } else if self.start != self.target {
                let fraction = (self.total - self.remaining) as f64 / self.total as f64;
                self.current = self.start + (self.target - self.start) * fraction;
            }
        }
        self.current
    }
}

/// One independently owned attachment, not a registry, queue or permission owner.
pub struct LiveChain {
    chain: Chain,
    binding: Binding,
    active: bool,
    gain: Ramp,
    wet: Ramp,
    max_transition_frames: usize,
    output: Box<[f64]>,
    valid_samples: usize,
    admission: Admission,
}

impl LiveChain {
    /// Control-thread only. Limits cover this host and its Chain together.
    pub fn compile(
        config: ChainConfig,
        limits: Limits,
        max_transition_frames: usize,
        binding: Binding,
        controls: Controls,
    ) -> Result<Self, Error> {
        if binding.generation == 0 || binding.revision == 0 {
            return Err(Error::InvalidBinding);
        }
        if !controls.valid() {
            return Err(Error::InvalidControls);
        }
        let channels = config.format.channels.len();
        let samples = mul(limits.max_block_frames, channels)?;
        let host_memory = add(
            size_of::<Self>() - size_of::<Chain>(),
            mul(samples, size_of::<f64>())?,
        )?;
        // Include format comparison, full buffer clearing, both ramp advances,
        // crossfade/gain, final result checks and counts. Abstract units, not ns.
        let mut metadata_work = channels;
        for channel in &config.format.channels {
            metadata_work = add(metadata_work, channel.len())?;
        }
        let host_work = add(
            metadata_work,
            add(mul(samples, 16)?, mul(limits.max_block_frames, 24)?)?,
        )?;
        budget("memoryBytes", host_memory, limits.max_memory_bytes)?;
        budget(
            "workUnitsPerBlock",
            host_work,
            limits.max_work_units_per_block,
        )?;
        let chain = Chain::compile(
            config,
            Limits {
                max_memory_bytes: limits.max_memory_bytes - host_memory,
                max_work_units_per_block: limits.max_work_units_per_block - host_work,
                ..limits
            },
        )
        .map_err(|error| {
            // Report combined required payload/work against the original limit.
            Error::Compile(match error {
                CompileError::Budget {
                    resource: "memoryBytes",
                    required,
                    ..
                } => required
                    .checked_add(host_memory)
                    .map(|required| CompileError::Budget {
                        resource: "memoryBytes",
                        required,
                        limit: limits.max_memory_bytes,
                    })
                    .unwrap_or(CompileError::ResourceOverflow),
                CompileError::Budget {
                    resource: "workUnitsPerBlock",
                    required,
                    ..
                } => required
                    .checked_add(host_work)
                    .map(|required| CompileError::Budget {
                        resource: "workUnitsPerBlock",
                        required,
                        limit: limits.max_work_units_per_block,
                    })
                    .unwrap_or(CompileError::ResourceOverflow),
                other => other,
            })
        })?;
        let mut admission = chain.admission();
        admission.memory_bytes = add(admission.memory_bytes, host_memory)?;
        admission.work_units_per_block = add(admission.work_units_per_block, host_work)?;
        Ok(Self {
            chain,
            binding,
            active: true,
            gain: Ramp::constant(controls.gain),
            wet: Ramp::constant(controls.wet),
            max_transition_frames,
            output: storage(samples, 0.0)?,
            valid_samples: 0,
            admission,
        })
    }

    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn is_active(&self) -> bool {
        self.active
    }
    pub fn admission(&self) -> Admission {
        self.admission
    }
    pub fn format(&self) -> &Format {
        &self.chain.config().format
    }
    /// Last successful final output only; empty after every error or revoke.
    pub fn output(&self) -> &[f64] {
        &self.output[..self.valid_samples]
    }

    /// Apply at a block boundary after the host has validated authority/freshness.
    /// A transition of N frames first advances 1/N and reaches the exact target
    /// on frame N, independently of how those frames are partitioned into blocks.
    pub fn update(
        &mut self,
        expected: Binding,
        next_revision: u64,
        controls: Controls,
        transition_frames: usize,
    ) -> Result<Binding, Error> {
        let result = self.check_binding(expected).and_then(|()| {
            if self.binding.revision.checked_add(1) != Some(next_revision) {
                return Err(Error::Revision);
            }
            if !controls.valid() {
                return Err(Error::InvalidControls);
            }
            if transition_frames > self.max_transition_frames {
                return Err(Error::TransitionFrames);
            }
            self.gain.set(controls.gain, transition_frames);
            self.wet.set(controls.wet, transition_frames);
            self.binding.revision = next_revision;
            // Do not leave an earlier revision's block available after a change.
            self.output.fill(0.0);
            self.valid_samples = 0;
            Ok(self.binding)
        });
        if result.is_err() {
            self.revoke();
        }
        result
    }

    /// Privacy/loss/restart: immediately discard all retained audio, including
    /// tails. An inactive instance cannot resume; compile and bind a fresh one.
    pub fn revoke(&mut self) {
        self.chain.reset();
        self.invalidate();
    }

    // Chain::process already clears all history on its own error path. Keep
    // host invalidation separate so that path uses the one reset we admitted.
    fn invalidate(&mut self) {
        self.active = false;
        self.output.fill(0.0);
        self.valid_samples = 0;
        self.gain = Ramp::constant(0.0);
        self.wet = Ramp::constant(0.0);
    }

    fn check_binding(&self, binding: Binding) -> Result<(), Error> {
        if !self.active {
            Err(Error::Inactive)
        } else if self.binding != binding {
            Err(Error::BindingMismatch)
        } else {
            Ok(())
        }
    }

    /// Already admitted, aligned interleaved Float64 only. No resampling,
    /// precision conversion, implicit channel mapping, allocation, locks or I/O.
    pub fn process(
        &mut self,
        binding: Binding,
        format: &Format,
        frames: usize,
        input: &[f64],
    ) -> Result<Report, Error> {
        match self.process_inner(binding, format, frames, input) {
            Ok(report) => Ok(report),
            Err(error) => {
                if matches!(error, Error::Process(_)) {
                    self.invalidate();
                } else {
                    self.revoke();
                }
                Err(error)
            }
        }
    }

    fn process_inner(
        &mut self,
        binding: Binding,
        format: &Format,
        frames: usize,
        input: &[f64],
    ) -> Result<Report, Error> {
        self.check_binding(binding)?;
        if format != &self.chain.config().format {
            return Err(Error::FormatMismatch);
        }
        self.chain.process(frames, input).map_err(Error::Process)?;
        self.output.fill(0.0);
        let channels = format.channels.len();
        let processed = self.chain.output();
        let mut over_full_scale_samples = 0;
        for frame in 0..frames {
            let gain = self.gain.advance();
            let wet = self.wet.advance();
            for channel in 0..channels {
                let sample = frame * channels + channel;
                let dry = input[sample];
                let effect = processed[sample];
                // Endpoints and equal paths preserve signed zero, subnormals and
                // low bits. Convex mixing avoids overflow in effect - dry.
                let mixed = if wet == 0.0 || dry.to_bits() == effect.to_bits() {
                    dry
                } else if wet == 1.0 {
                    effect
                } else {
                    (1.0 - wet) * dry + wet * effect
                };
                let value = if gain == 1.0 { mixed } else { mixed * gain };
                if !value.is_finite() {
                    return Err(Error::NonFiniteResult { sample });
                }
                self.output[sample] = value;
                over_full_scale_samples += usize::from(value.abs() > 1.0);
            }
        }
        self.valid_samples = input.len();
        Ok(Report {
            binding: self.binding,
            frames,
            over_full_scale_samples,
            controls: Controls {
                gain: self.gain.current,
                wet: self.wet.current,
            },
            transition_remaining_frames: self.gain.remaining,
        })
    }
}
