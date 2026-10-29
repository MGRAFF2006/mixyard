//! Exclusive, preallocated C ABI over the canonical policy/PCM/DSP implementation.
//! No native activation, endpoint authorization or clock equivalence is invented.

use mixyard::audio::{self, clock, pcm, InputBank, Processor64};
use mixyard::dsp::{self, live};
use mixyard::runtime::worker;
use mixyard::{AudioFormat, Config, Inventory, ResolutionStatus};
use serde::{Deserialize, Serialize};
use std::{ffi::c_char, ptr, slice};
mod attestation;
pub use attestation::MxOutputPacket;

pub const MX_OK: i32 = 0;
pub const MX_INVALID: i32 = 1;
pub const MX_POLICY: i32 = 2;
pub const MX_STALE: i32 = 3;
pub const MX_FORMAT: i32 = 4;
pub const MX_SAMPLES: i32 = 5;
pub const MX_EXPIRED: i32 = 6;
pub const MX_INACTIVE: i32 = 7;
pub const MX_BUFFER: i32 = 8;
pub const MX_PROCESS: i32 = 9;
pub const MX_UNAVAILABLE: i32 = 10;
const MAX_REQUEST: usize = 1024 * 1024;

#[no_mangle]
pub extern "C" fn mx_host_abi_version() -> u32 {
    1
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MxStamp {
    pub generation: u64,
    pub revision: u64,
    pub clock_id: u64,
    pub sequence: u64,
    pub first_frame: u64,
    pub captured_ns: u64,
    pub deadline_ns: u64,
}
impl MxStamp {
    fn clock(self) -> clock::Stamp {
        clock::Stamp {
            binding: clock::Binding {
                generation: self.generation,
                revision: self.revision,
            },
            clock_id: self.clock_id,
            sequence: self.sequence,
            first_frame: self.first_frame,
            captured_ns: self.captured_ns,
            deadline_ns: self.deadline_ns,
        }
    }
    fn worker(self) -> worker::Stamp {
        worker::Stamp {
            binding: worker::Binding {
                generation: self.generation,
                revision: self.revision,
            },
            clock_id: self.clock_id,
            sequence: self.sequence,
            first_frame: self.first_frame,
            captured_ns: self.captured_ns,
            deadline_ns: self.deadline_ns,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct MxInput {
    pub bytes: *const u8,
    pub byte_count: usize,
    pub encoding: u32,
    pub sample_rate_hz: u32,
    pub channels: u32,
    pub stamp: MxStamp,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum Encoding {
    #[serde(rename = "f32-le")]
    F32,
    #[serde(rename = "f64-le")]
    F64,
    #[serde(rename = "s16-le")]
    S16,
    #[serde(rename = "s24-le")]
    S24,
    #[serde(rename = "s32-le")]
    S32,
}
impl Encoding {
    fn code(self) -> u32 {
        match self {
            Self::F32 => 1,
            Self::F64 => 2,
            Self::S16 => 3,
            Self::S24 => 4,
            Self::S32 => 5,
        }
    }
    fn width(self) -> usize {
        match self {
            Self::F32 | Self::S32 => 4,
            Self::F64 => 8,
            Self::S16 => 2,
            Self::S24 => 3,
        }
    }
    fn decode(self, bytes: &[u8], samples: &mut [f64]) -> Result<(), i32> {
        if samples.len().checked_mul(self.width()) != Some(bytes.len()) {
            return Err(MX_SAMPLES);
        }
        match self {
            Self::F32 => {
                for (sample, packed) in samples.iter_mut().zip(bytes.as_chunks::<4>().0) {
                    *sample = f32::from_le_bytes(*packed) as f64;
                }
            }
            Self::F64 => {
                for (sample, packed) in samples.iter_mut().zip(bytes.as_chunks::<8>().0) {
                    *sample = f64::from_le_bytes(*packed);
                }
            }
            format => pcm::decode(
                match format {
                    Self::S16 => pcm::SignedPcm::S16Le,
                    Self::S24 => pcm::SignedPcm::S24Le,
                    Self::S32 => pcm::SignedPcm::S32Le,
                    _ => unreachable!(),
                },
                bytes,
                samples,
            )
            .map_err(|_| MX_SAMPLES)?,
        }
        if samples.iter().any(|v| !v.is_finite()) {
            samples.fill(0.0);
            return Err(MX_SAMPLES);
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceSpec {
    observed_id: String,
    encoding: Encoding,
    sample_rate_hz: u32,
    channels: Vec<String>,
    mapping: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutputSpec {
    observed_id: String,
    sample_rate_hz: u32,
    channels: Vec<String>,
    mapping: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Limits {
    max_sources: usize,
    max_mixes: usize,
    max_sends: usize,
    max_outputs: usize,
    max_buffer_samples: usize,
    max_total_bytes: usize,
    max_dsp_work_units_per_block: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    config: Config,
    inventory: Inventory,
    generation: u64,
    clock_id: u64,
    max_frames: usize,
    limits: Limits,
    sources: Vec<Option<SourceSpec>>,
    outputs: Vec<Option<OutputSpec>>,
    #[serde(default)]
    source_dsp: Vec<Option<dsp::ChainConfig>>,
    #[serde(default)]
    mix_dsp: Vec<Option<dsp::ChainConfig>>,
    #[serde(default)]
    output_dsp: Vec<Option<dsp::ChainConfig>>,
}
struct Effect {
    host: live::LiveChain,
    format: dsp::Format,
}
impl Effect {
    fn process(&mut self, frames: usize, samples: &[f64]) -> Result<&[f64], i32> {
        self.host
            .process(self.host.binding(), &self.format, frames, samples)
            .map_err(|_| MX_PROCESS)?;
        Ok(self.host.output())
    }
    fn revoke(&mut self) {
        if self.host.is_active() {
            self.host.revoke();
        }
    }
    fn discard_history(&mut self) -> Result<(), i32> {
        self.host
            .discard_history(self.host.binding())
            .map_err(|_| MX_PROCESS)
    }
}
struct Source {
    spec: SourceSpec,
    decoded: Vec<f64>,
    clock: clock::RateAdapter<f64>,
    effect: Option<Effect>,
    lost: bool,
}
struct Output {
    queue: worker::Queue<f64>,
    effect: Option<Effect>,
    samples: Vec<f64>,
    count: usize,
    active: bool,
}
impl Output {
    fn revoke(&mut self) {
        self.queue.revoke();
        if let Some(effect) = &mut self.effect {
            effect.revoke();
        }
        self.samples.fill(0.0);
        self.count = 0;
        self.active = false;
    }
}
#[repr(C)]
pub struct MxHost {
    processor: Processor64,
    bank: InputBank<f64>,
    mix_bank: InputBank<f64>,
    sources: Vec<Option<Source>>,
    mix_effects: Vec<Option<Effect>>,
    outputs: Vec<Option<Output>>,
    source_info: Vec<Vec<u8>>,
    output_info: Vec<Vec<u8>>,
    attestations: Vec<Option<attestation::Cached>>,
    output_stamp: Option<MxStamp>,
    binding: worker::Binding,
    clock_id: u64,
    next_sequence: u64,
    next_frame: u64,
    last_now: u64,
    last_capture: u64,
    active: bool,
}
fn canonical(channels: u16) -> Vec<String> {
    if channels == 1 {
        vec!["MONO".into()]
    } else {
        vec!["FL".into(), "FR".into()]
    }
}
fn labels(actual: &[String], mapping: &[String], format: AudioFormat) -> Result<(), String> {
    if actual.len() != format.channels as usize || mapping != canonical(format.channels) {
        return Err("host: explicit canonical MONO/FL,FR identity mapping required".into());
    }
    for (index, label) in actual.iter().enumerate() {
        if label.is_empty() || label.len() > 4096 || actual[..index].contains(label) {
            return Err("host: exact unique native channel IDs required".into());
        }
    }
    Ok(())
}
fn padded<T>(mut entries: Vec<Option<T>>, count: usize) -> Result<Vec<Option<T>>, String> {
    if entries.is_empty() {
        entries.resize_with(count, || None);
    }
    if entries.len() != count {
        return Err("host: DSP slot count differs from canonical plan".into());
    }
    Ok(entries)
}
struct Budget {
    memory: usize,
    work: usize,
}
fn effect(
    config: Option<dsp::ChainConfig>,
    format: AudioFormat,
    attachment: live::Attachment,
    binding: worker::Binding,
    frames: usize,
    budget: &mut Budget,
) -> Result<Option<Effect>, String> {
    let Some(config) = config else {
        return Ok(None);
    };
    let expected = dsp::Format {
        sample_rate_hz: format.sample_rate_hz,
        channels: canonical(format.channels),
    };
    if config.format != expected {
        return Err("host: DSP format must match its canonical graph attachment".into());
    }
    let host = live::LiveChain::compile(
        config,
        dsp::Limits {
            max_block_frames: frames,
            max_channels: format.channels as usize,
            max_effects: usize::MAX,
            max_memory_bytes: budget.memory,
            max_work_units_per_block: budget.work / 2,
        },
        0,
        live::Binding {
            attachment,
            generation: binding.generation,
            revision: binding.revision,
        },
        live::Controls::UNITY,
    )
    .map_err(|e| format!("host: DSP admission: {e}"))?;
    let admitted = host.admission();
    budget.memory = budget
        .memory
        .checked_sub(admitted.memory_bytes)
        .ok_or("host: DSP memory budget")?;
    budget.work = budget
        .work
        .checked_sub(
            admitted
                .work_units_per_block
                .checked_mul(2)
                .ok_or("host: DSP work overflow")?,
        )
        .ok_or("host: DSP work budget")?;
    Ok(Some(Effect {
        host,
        format: expected,
    }))
}
impl MxHost {
    fn new(request: Request, request_digest: String) -> Result<Self, String> {
        let Request {
            config,
            inventory,
            generation,
            clock_id,
            max_frames,
            limits,
            sources,
            outputs,
            source_dsp,
            mix_dsp,
            output_dsp,
        } = request;
        if generation == 0 || clock_id == 0 || config.revision == 0 || max_frames == 0 {
            return Err("host: positive generation/revision/clock/frame budget required".into());
        }
        let rate = config
            .mixes
            .first()
            .ok_or("host: at least one mix required")?
            .sample_rate_hz;
        let plan = mixyard::plan(&config, &inventory)?;
        if sources.len() != plan.sources.len() || outputs.len() != plan.outputs.len() {
            return Err("host: exact source/output plan slot count required".into());
        }
        // All sums are checked before any PCM/state allocation. Metadata/JSON
        // is separately bounded by the 1 MiB request; allocator overhead is extra.
        let source_channels = plan
            .sources
            .iter()
            .filter_map(|s| s.format)
            .try_fold(0usize, |n, f| n.checked_add(f.channels as usize))
            .ok_or("host: channel overflow")?;
        let mix_channels = plan
            .mixes
            .iter()
            .try_fold(0usize, |n, m| n.checked_add(m.channels as usize))
            .ok_or("host: channel overflow")?;
        let output_channels = outputs
            .iter()
            .flatten()
            .try_fold(0usize, |n, o| n.checked_add(o.channels.len()))
            .ok_or("host: channel overflow")?;
        let scalar = source_channels
            .checked_mul(4)
            .and_then(|n| mix_channels.checked_add(n))
            .and_then(|n| {
                output_channels
                    .checked_mul(2)
                    .and_then(|o| n.checked_add(o))
            })
            .ok_or("host: buffer overflow")?;
        let bytes = scalar
            .checked_mul(max_frames)
            .and_then(|n| n.checked_mul(8))
            .and_then(|n| {
                source_channels
                    .checked_mul(max_frames)
                    .and_then(|s| s.checked_mul(16))
                    .and_then(|s| n.checked_add(s))
            })
            .and_then(|n| {
                limits
                    .max_buffer_samples
                    .checked_mul(8)
                    .and_then(|p| n.checked_add(p))
            })
            .ok_or("host: buffer overflow")?;
        if bytes > limits.max_total_bytes {
            return Err("host: total PCM/state byte budget exceeded".into());
        }
        let mut budget = Budget {
            memory: limits.max_total_bytes - bytes,
            work: limits.max_dsp_work_units_per_block,
        };
        let processor = Processor64::compile(
            &config,
            &inventory,
            rate,
            audio::Limits {
                max_frames,
                max_sources: limits.max_sources,
                max_mixes: limits.max_mixes,
                max_sends: limits.max_sends,
                max_outputs: limits.max_outputs,
                max_buffer_samples: limits.max_buffer_samples,
            },
        )?;
        let attestations = attestation::compile(attestation::Input {
            plan: processor.plan(),
            inventory: &inventory,
            sources: &sources,
            outputs: &outputs,
            dsp: (&source_dsp, &mix_dsp, &output_dsp),
            generation,
            clock_id,
            request_digest,
        });
        let bank = processor.input_bank(
            source_channels
                .checked_mul(max_frames)
                .ok_or("host: bank overflow")?,
        )?;
        let mix_bank = processor.mix_bank(
            mix_channels
                .checked_mul(max_frames)
                .ok_or("host: bank overflow")?,
        )?;
        let binding = worker::Binding {
            generation,
            revision: config.revision,
        };
        let source_info = sources
            .iter()
            .map(serde_json::to_vec)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let output_info = outputs
            .iter()
            .map(serde_json::to_vec)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let source_dsp = padded(source_dsp, sources.len())?;
        let mut source_states = Vec::new();
        for (index, (spec, chain)) in sources.into_iter().zip(source_dsp).enumerate() {
            let selected = &processor.plan().sources[index];
            let Some(spec) = spec else {
                if selected.status == ResolutionStatus::Selected || chain.is_some() {
                    return Err("host: selected source needs actual binding".into());
                }
                source_states.push(None);
                continue;
            };
            let format = selected
                .format
                .ok_or("host: selected source format unknown")?;
            if selected.status != ResolutionStatus::Selected
                || selected.observed_ids.as_slice() != [spec.observed_id.as_str()]
                || spec.sample_rate_hz != format.sample_rate_hz
            {
                return Err(
                    "host: source endpoint/rate binding differs from authoritative plan".into(),
                );
            }
            labels(&spec.channels, &spec.mapping, format)?;
            let clock_format = clock::Format {
                sample_rate_hz: format.sample_rate_hz,
                channels: canonical(format.channels),
            };
            let samples = max_frames * format.channels as usize;
            let clock = clock::RateAdapter::new(
                clock_format.clone(),
                clock_format,
                clock::Binding {
                    generation,
                    revision: config.revision,
                },
                clock_id,
                clock_id,
                clock::Method::Bypass,
                clock::Limits {
                    max_channels: format.channels as usize,
                    max_channel_label_bytes: 16,
                    max_input_frames: max_frames,
                    max_output_frames: max_frames,
                    capacity_frames: max_frames,
                    max_buffer_bytes: samples * 16 + max_frames * 16,
                    max_correction_ppm: 0,
                },
            )
            .map_err(|e| format!("host: clock admission {e:?}"))?;
            let effect = effect(
                chain,
                format,
                live::Attachment::Source(index as u64 + 1),
                binding,
                max_frames,
                &mut budget,
            )?;
            source_states.push(Some(Source {
                spec,
                decoded: vec![0.0; samples],
                clock,
                effect,
                lost: false,
            }));
        }
        let mut mix_effects = Vec::new();
        for (index, chain) in padded(mix_dsp, processor.plan().mixes.len())?
            .into_iter()
            .enumerate()
        {
            let mix = &processor.plan().mixes[index];
            mix_effects.push(effect(
                chain,
                AudioFormat {
                    channels: mix.channels,
                    sample_rate_hz: mix.sample_rate_hz,
                },
                live::Attachment::Mix(index as u64 + 1),
                binding,
                max_frames,
                &mut budget,
            )?);
        }
        let mut output_states = Vec::new();
        for (index, (spec, chain)) in outputs
            .into_iter()
            .zip(padded(output_dsp, processor.plan().outputs.len())?)
            .enumerate()
        {
            let selected = &processor.plan().outputs[index];
            let Some(spec) = spec else {
                if processor.output_format(index).is_some() || chain.is_some() {
                    return Err("host: selected output needs actual binding".into());
                }
                output_states.push(None);
                continue;
            };
            let format = processor
                .output_format(index)
                .ok_or("host: output not selected")?;
            if selected.observed_ids.as_slice() != [spec.observed_id.as_str()]
                || spec.sample_rate_hz != format.sample_rate_hz
            {
                return Err(
                    "host: output endpoint/rate binding differs from authoritative plan".into(),
                );
            }
            labels(&spec.channels, &spec.mapping, format)?;
            let queue = worker::Queue::new(
                worker::Format {
                    sample_rate_hz: format.sample_rate_hz,
                    channels: spec.channels,
                    precision: audio::Precision::Float64,
                },
                binding,
                clock_id,
                1,
                max_frames,
                max_frames * format.channels as usize * 8,
            )?;
            let effect = effect(
                chain,
                format,
                live::Attachment::Output(index as u64 + 1),
                binding,
                max_frames,
                &mut budget,
            )?;
            output_states.push(Some(Output {
                queue,
                effect,
                samples: vec![0.0; max_frames * format.channels as usize],
                count: 0,
                active: true,
            }));
        }
        Ok(Self {
            processor,
            bank,
            mix_bank,
            sources: source_states,
            mix_effects,
            outputs: output_states,
            source_info,
            output_info,
            attestations,
            output_stamp: None,
            binding,
            clock_id,
            next_sequence: 0,
            next_frame: 0,
            last_now: 0,
            last_capture: 0,
            active: true,
        })
    }
    fn revoke(&mut self) {
        self.active = false;
        self.output_stamp = None;
        self.bank.clear();
        self.mix_bank.clear();
        for source in self.sources.iter_mut().flatten() {
            source.clock.revoke();
            source.decoded.fill(0.0);
            source.lost = true;
            if let Some(effect) = &mut source.effect {
                effect.revoke();
            }
        }
        for effect in self.mix_effects.iter_mut().flatten() {
            effect.revoke();
        }
        for output in self.outputs.iter_mut().flatten() {
            output.revoke();
        }
        let _ = self.processor.render(0, &[]);
    }
    fn current(&self, now: u64, stamp: MxStamp) -> Result<(), i32> {
        if stamp.generation != self.binding.generation
            || stamp.revision != self.binding.revision
            || stamp.clock_id != self.clock_id
        {
            return Err(MX_STALE);
        }
        if stamp.captured_ns > now
            || now > stamp.deadline_ns
            || stamp.deadline_ns < stamp.captured_ns
        {
            return Err(MX_EXPIRED);
        }
        Ok(())
    }
    unsafe fn process(
        &mut self,
        now: u64,
        stamp: MxStamp,
        frames: usize,
        inputs: &[MxInput],
    ) -> Result<(), i32> {
        if !self.active {
            return Err(MX_INACTIVE);
        }
        self.current(now, stamp)?;
        if frames == 0 || frames > self.processor.max_frames() || inputs.len() != self.sources.len()
        {
            return Err(MX_SAMPLES);
        }
        if now < self.last_now
            || stamp.captured_ns < self.last_capture
            || stamp.sequence != self.next_sequence
            || stamp.first_frame != self.next_frame
        {
            return Err(MX_STALE);
        }
        let next_sequence = stamp.sequence.checked_add(1).ok_or(MX_STALE)?;
        let next_frame = stamp
            .first_frame
            .checked_add(frames as u64)
            .ok_or(MX_STALE)?;
        self.output_stamp = None;
        let mut source_lost = false;
        self.bank.clear();
        self.mix_bank.clear();
        for output in self.outputs.iter_mut().flatten() {
            output.samples.fill(0.0);
            output.count = 0;
        }
        for (index, input) in inputs.iter().enumerate() {
            let Some(source) = &mut self.sources[index] else {
                if !input.bytes.is_null() || input.byte_count != 0 {
                    return Err(MX_STALE);
                }
                continue;
            };
            if input.bytes.is_null() && input.byte_count == 0 {
                if !source.lost {
                    source.clock.revoke();
                    source.decoded.fill(0.0);
                    source.lost = true;
                    source_lost = true;
                    if let Some(effect) = &mut source.effect {
                        effect.revoke();
                    }
                }
                self.bank.set(index, frames, None).map_err(|_| MX_PROCESS)?;
                continue;
            }
            if source.lost {
                return Err(MX_STALE);
            }
            let spec = &source.spec;
            if input.encoding != spec.encoding.code()
                || input.sample_rate_hz != spec.sample_rate_hz
                || input.channels as usize != spec.channels.len()
            {
                return Err(MX_FORMAT);
            }
            let count = frames * spec.channels.len();
            if input.bytes.is_null() || input.byte_count != count * spec.encoding.width() {
                return Err(MX_SAMPLES);
            }
            let source_stamp = input.stamp;
            if source_stamp.generation != stamp.generation
                || source_stamp.revision != stamp.revision
                || source_stamp.clock_id != stamp.clock_id
                || source_stamp.sequence != stamp.sequence
                || source_stamp.first_frame != stamp.first_frame
            {
                return Err(MX_STALE);
            }
            if source_stamp.captured_ns > stamp.captured_ns
                || source_stamp.deadline_ns < stamp.deadline_ns
            {
                return Err(MX_EXPIRED);
            }
            spec.encoding.decode(
                slice::from_raw_parts(input.bytes, input.byte_count),
                &mut source.decoded[..count],
            )?;
            source
                .clock
                .push(now, source_stamp.clock(), frames, &source.decoded[..count])
                .map_err(|_| MX_STALE)?;
            source
                .clock
                .render(
                    now,
                    clock::Binding {
                        generation: self.binding.generation,
                        revision: self.binding.revision,
                    },
                    frames,
                )
                .map_err(|_| MX_EXPIRED)?;
            let samples = if let Some(effect) = &mut source.effect {
                effect.process(frames, source.clock.output())?
            } else {
                source.clock.output()
            };
            self.bank
                .set(index, frames, Some(samples))
                .map_err(|_| MX_PROCESS)?;
        }
        if source_lost {
            // Derived histories can still contain a lost source. Clear them
            // before processing current authorized inputs; no source is rearmed.
            for effect in self.mix_effects.iter_mut().flatten() {
                effect.discard_history()?;
            }
            for output in self.outputs.iter_mut().flatten().filter(|o| o.active) {
                if let Some(effect) = &mut output.effect {
                    effect.discard_history()?;
                }
            }
        }
        self.processor
            .render_bank(frames, &self.bank)
            .map_err(|_| MX_PROCESS)?;
        for (index, effect) in self.mix_effects.iter_mut().enumerate() {
            if let Some(effect) = effect {
                let samples =
                    effect.process(frames, self.processor.mix_samples(index).ok_or(MX_PROCESS)?)?;
                self.mix_bank
                    .set(index, frames, Some(samples))
                    .map_err(|_| MX_PROCESS)?;
            }
        }
        self.processor
            .apply_mix_bank(&self.mix_bank)
            .map_err(|_| MX_PROCESS)?;
        for (index, output) in self.outputs.iter_mut().enumerate() {
            let Some(output) = output else {
                continue;
            };
            if !output.active {
                continue;
            }
            let raw = self.processor.output_samples(index).ok_or(MX_PROCESS)?;
            let samples = if let Some(effect) = &mut output.effect {
                match effect.process(frames, raw) {
                    Ok(samples) => samples,
                    Err(_) => {
                        output.revoke();
                        continue;
                    }
                }
            } else {
                raw
            };
            if output
                .queue
                .push(now, stamp.worker(), frames, samples)
                .is_err()
                || output
                    .queue
                    .pop(now, self.binding, &mut output.samples)
                    .is_err()
            {
                output.revoke();
                continue;
            }
            output.count = frames * output.queue.format().channels.len();
        }
        self.output_stamp = Some(stamp);
        self.next_sequence = next_sequence;
        self.next_frame = next_frame;
        self.last_now = now;
        self.last_capture = stamp.captured_ns;
        Ok(())
    }
}

unsafe fn error_text(pointer: *mut c_char, capacity: usize, text: &str) {
    if pointer.is_null() || capacity == 0 {
        return;
    }
    let count = text.len().min(capacity - 1);
    ptr::copy_nonoverlapping(text.as_ptr(), pointer.cast(), count);
    *pointer.add(count) = 0;
}
/// # Safety
/// All pointers must be valid for their stated lengths; the returned context has
/// one exclusive owner and must not be destroyed until every user is quiescent.
#[no_mangle]
pub unsafe extern "C" fn mx_host_create(
    bytes: *const u8,
    length: usize,
    out: *mut *mut MxHost,
    error: *mut c_char,
    capacity: usize,
) -> i32 {
    if out.is_null() {
        return MX_INVALID;
    }
    *out = ptr::null_mut();
    error_text(error, capacity, "");
    if bytes.is_null() || length == 0 || length > MAX_REQUEST {
        error_text(error, capacity, "host: invalid request length");
        return MX_INVALID;
    }
    let request = match serde_json::from_slice::<Request>(slice::from_raw_parts(bytes, length)) {
        Ok(request) => request,
        Err(message) => {
            error_text(
                error,
                capacity,
                &format!("host: invalid request: {message}"),
            );
            return MX_INVALID;
        }
    };
    match MxHost::new(
        request,
        attestation::digest(slice::from_raw_parts(bytes, length)),
    ) {
        Ok(host) => {
            *out = Box::into_raw(Box::new(host));
            MX_OK
        }
        Err(message) => {
            error_text(error, capacity, &message);
            MX_POLICY
        }
    }
}
/// # Safety
/// A live context from create, exclusively owned and fully quiescent, or null.
#[no_mangle]
pub unsafe extern "C" fn mx_host_destroy(host: *mut MxHost) {
    if !host.is_null() {
        drop(Box::from_raw(host));
    }
}
/// # Safety
/// Context is live and no concurrent writer exists.
#[no_mangle]
pub unsafe extern "C" fn mx_host_source_count(host: *const MxHost) -> usize {
    host.as_ref().map_or(0, |h| h.sources.len())
}
/// # Safety
/// Context is live and no concurrent writer exists.
#[no_mangle]
pub unsafe extern "C" fn mx_host_output_count(host: *const MxHost) -> usize {
    host.as_ref().map_or(0, |h| h.outputs.len())
}
unsafe fn info(
    entries: &[Vec<u8>],
    index: usize,
    bytes: *mut u8,
    capacity: usize,
    needed: *mut usize,
) -> i32 {
    if needed.is_null() {
        return MX_INVALID;
    }
    *needed = 0;
    let Some(value) = entries.get(index) else {
        return MX_INVALID;
    };
    *needed = value.len();
    if bytes.is_null() || capacity < value.len() {
        return MX_BUFFER;
    }
    ptr::copy_nonoverlapping(value.as_ptr(), bytes, value.len());
    MX_OK
}
/// # Safety
/// Context/needed are valid; byte buffer is valid for capacity and cannot alias context.
#[no_mangle]
pub unsafe extern "C" fn mx_host_source_info(
    host: *const MxHost,
    index: usize,
    bytes: *mut u8,
    capacity: usize,
    needed: *mut usize,
) -> i32 {
    let Some(host) = host.as_ref() else {
        if !needed.is_null() {
            *needed = 0;
        }
        return MX_INVALID;
    };
    info(&host.source_info, index, bytes, capacity, needed)
}
/// # Safety
/// Same ownership/buffer requirements as source_info.
#[no_mangle]
pub unsafe extern "C" fn mx_host_output_info(
    host: *const MxHost,
    index: usize,
    bytes: *mut u8,
    capacity: usize,
    needed: *mut usize,
) -> i32 {
    let Some(host) = host.as_ref() else {
        if !needed.is_null() {
            *needed = 0;
        }
        return MX_INVALID;
    };
    info(&host.output_info, index, bytes, capacity, needed)
}
/// # Safety
/// Exclusive live context. Inputs and non-null PCM pointers address complete
/// caller-owned buffers and cannot alias any context/output storage.
#[no_mangle]
pub unsafe extern "C" fn mx_host_process(
    host: *mut MxHost,
    now: u64,
    stamp: MxStamp,
    frames: usize,
    inputs: *const MxInput,
    count: usize,
) -> i32 {
    let Some(host) = host.as_mut() else {
        return MX_INVALID;
    };
    if count != host.sources.len()
        || (count != 0
            && (inputs.is_null()
                || !(inputs as usize).is_multiple_of(std::mem::align_of::<MxInput>())))
    {
        host.revoke();
        return MX_INVALID;
    }
    let inputs = if count == 0 {
        &[]
    } else {
        slice::from_raw_parts(inputs, count)
    };
    match host.process(now, stamp, frames, inputs) {
        Ok(()) => MX_OK,
        Err(error) => {
            host.revoke();
            error
        }
    }
}
/// # Safety
/// Live context, valid out-pointers and no concurrent process/revoke/destroy.
#[no_mangle]
pub unsafe extern "C" fn mx_host_output(
    host: *const MxHost,
    index: usize,
    samples: *mut *const f64,
    count: *mut usize,
) -> i32 {
    if samples.is_null() || count.is_null() {
        return MX_INVALID;
    }
    *samples = ptr::null();
    *count = 0;
    let Some(host) = host.as_ref() else {
        return MX_INVALID;
    };
    if !host.active {
        return MX_INACTIVE;
    }
    let Some(Some(output)) = host.outputs.get(index) else {
        return MX_INVALID;
    };
    if !output.active || output.count == 0 {
        return MX_INACTIVE;
    }
    *samples = output.samples.as_ptr();
    *count = output.count;
    MX_OK
}
/// # Safety
/// Exclusive live graph owner; native users must quiesce separately before destroy.
#[no_mangle]
pub unsafe extern "C" fn mx_host_revoke(host: *mut MxHost) {
    if let Some(host) = host.as_mut() {
        host.revoke();
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
