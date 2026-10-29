//! In-process Linux PCM execution with owned null/file consumers.
//! No PipeWire connection, device publication, hardware clock adaptation or IPC
//! capability promotion. Compile/bind/create/drop and file drains run off callback.

use super::worker::{Binding, Block, Error as QueueError, Format, Queue, Stamp};
use crate::audio::{pcm, Pcm, Precision, Processor64, RenderError};
use crate::{AudioFormat, Platform, ResolutionStatus};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Exact identity and actual negotiated format supplied by the source/consumer
/// owner, in immutable Processor64 plan order. This is not native discovery.
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub observed_id: String,
    pub format: Format,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Inactive,
    Stamp,
    Source(usize),
    Render(RenderError),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub queued: usize,
    pub failed: usize,
}

/// Metadata accompanies each supplied Pcm; absent input must have absent metadata.
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    pub observed_id: &'a str,
    pub format: &'a Format,
    pub stamp: Stamp,
}

pub struct LinuxAudio {
    processor: Processor64,
    binding: Binding,
    clock_id: u64,
    sources: Vec<Option<Endpoint>>,
    source_capture_ns: Vec<Option<u64>>,
    outputs: Vec<Option<Queue<f64>>>,
    consumer_claimed: Vec<bool>,
    active: bool,
}

fn canonical(format: AudioFormat) -> Format {
    Format {
        sample_rate_hz: format.sample_rate_hz,
        channels: if format.channels == 1 {
            vec!["MONO".into()]
        } else {
            vec!["FL".into(), "FR".into()]
        },
        precision: Precision::Float64,
    }
}

impl LinuxAudio {
    /// Processor64 has already compiled authoritative routing/policy. Unknown or
    /// noncanonical source order/precision/rate cannot be relabeled for execution.
    /// max_queue_bytes bounds total output PCM storage (queue metadata is extra).
    pub fn new(
        processor: Processor64,
        binding: Binding,
        clock_id: u64,
        sources: Vec<Option<Endpoint>>,
        destinations: Vec<Option<Endpoint>>,
        queue_slots: usize,
        max_queue_bytes: usize,
    ) -> Result<Self, String> {
        let plan = processor.plan();
        if plan.platform != Platform::Linux
            || binding.revision != plan.config_revision
            || binding.generation == 0
            || binding.revision == 0
            || clock_id == 0
            || queue_slots == 0
            || sources.len() != plan.sources.len()
            || destinations.len() != plan.outputs.len()
        {
            return Err("linux-audio: invalid session, revision or port count".into());
        }
        for (selected, bound) in plan.sources.iter().zip(&sources) {
            match (selected.status, bound) {
                (ResolutionStatus::Selected, Some(bound))
                    if selected.observed_ids.len() == 1
                        && selected.observed_ids[0] == bound.observed_id
                        && selected.format.map(canonical).as_ref() == Some(&bound.format) => {}
                (ResolutionStatus::Missing | ResolutionStatus::Ambiguous, None) => {}
                _ => return Err("linux-audio: source identity/format binding mismatch".into()),
            }
        }
        let mut required = 0usize;
        for (index, bound) in destinations.iter().enumerate() {
            let selected = &plan.outputs[index];
            match (processor.output_format(index), bound) {
                (Some(format), Some(bound))
                    if selected.observed_ids.len() == 1
                        && selected.observed_ids[0] == bound.observed_id
                        && canonical(format) == bound.format =>
                {
                    required = processor
                        .max_frames()
                        .checked_mul(format.channels as usize)
                        .and_then(|n| n.checked_mul(8))
                        .and_then(|n| n.checked_mul(queue_slots))
                        .and_then(|n| required.checked_add(n))
                        .ok_or("linux-audio: queue storage overflow")?;
                }
                (None, None) => {}
                _ => return Err("linux-audio: output identity/format binding mismatch".into()),
            }
        }
        if required > max_queue_bytes {
            return Err("linux-audio: total queue storage exceeds budget".into());
        }
        let outputs = destinations
            .into_iter()
            .map(|bound| {
                bound
                    .map(|bound| {
                        Queue::new(
                            bound.format,
                            binding,
                            clock_id,
                            queue_slots,
                            processor.max_frames(),
                            max_queue_bytes,
                        )
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let consumer_claimed = vec![false; plan.outputs.len()];
        let source_capture_ns = vec![None; sources.len()];
        Ok(Self {
            processor,
            binding,
            clock_id,
            sources,
            source_capture_ns,
            outputs,
            consumer_claimed,
            active: true,
        })
    }

    pub fn binding(&self) -> Binding {
        self.binding
    }

    pub fn output_format(&self, output: usize) -> Option<&Format> {
        self.outputs.get(output)?.as_ref().map(Queue::format)
    }

    pub fn revoke(&mut self) {
        self.active = false;
        for queue in self.outputs.iter_mut().flatten() {
            queue.revoke();
        }
        // The core clears its storage on every failed render, without allocation.
        let _ = self.processor.render(0, &[]);
    }

    fn current(&self, now_ns: u64, stamp: Stamp) -> bool {
        stamp.binding == self.binding
            && stamp.clock_id == self.clock_id
            && stamp.captured_ns <= now_ns
            && now_ns <= stamp.deadline_ns
    }

    /// One aligned engine-timeline block. Input descriptors are caller-owned,
    /// so this operation does no allocation, locking, I/O or heap reclamation.
    /// Sources must share the explicitly supplied timeline, not just nominal rate.
    /// Invalid source metadata terminally revokes this instance and all queued PCM.
    /// A full/expired/revoked output queue fails only that destination.
    pub fn render(
        &mut self,
        now_ns: u64,
        stamp: Stamp,
        frames: usize,
        pcm: &[Option<Pcm<'_, f64>>],
        metadata: &[Option<Input<'_>>],
    ) -> Result<Rendered, Error> {
        if !self.active {
            return Err(Error::Inactive);
        }
        if !self.current(now_ns, stamp) {
            self.revoke();
            return Err(Error::Stamp);
        }
        if pcm.len() != self.sources.len() || metadata.len() != self.sources.len() {
            self.revoke();
            return Err(Error::Source(self.sources.len()));
        }
        for (index, ((pcm, input), bound)) in
            pcm.iter().zip(metadata).zip(&self.sources).enumerate()
        {
            let valid = match (pcm, input, bound) {
                (None, None, _) => true,
                (Some(_), Some(input), Some(bound)) => {
                    input.observed_id == bound.observed_id
                        && input.format == &bound.format
                        && self.current(now_ns, input.stamp)
                        && self.source_capture_ns[index]
                            .is_none_or(|previous| input.stamp.captured_ns >= previous)
                        && input.stamp.sequence == stamp.sequence
                        && input.stamp.first_frame == stamp.first_frame
                        // Output handoff must not extend a contributing source's
                        // lifetime or claim to predate its capture.
                        && stamp.deadline_ns <= input.stamp.deadline_ns
                        && stamp.captured_ns >= input.stamp.captured_ns
                }
                _ => false,
            };
            if !valid {
                self.revoke();
                return Err(Error::Source(index));
            }
        }
        if let Err(error) = self.processor.render(frames, pcm) {
            self.revoke();
            return Err(Error::Render(error));
        }
        for (previous, input) in self.source_capture_ns.iter_mut().zip(metadata) {
            if let Some(input) = input {
                *previous = Some(input.stamp.captured_ns);
            }
        }
        let mut result = Rendered::default();
        for (index, queue) in self.outputs.iter_mut().enumerate() {
            if let Some(queue) = queue {
                match queue.push(
                    now_ns,
                    stamp,
                    frames,
                    self.processor.output_samples(index).unwrap(),
                ) {
                    Ok(()) => result.queued += 1,
                    Err(_) => result.failed += 1,
                }
            }
        }
        Ok(result)
    }
}

/// Each destination selects its own encoding after Float64 mixing/queueing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Float64Le,
    /// Explicitly requested rounding for this destination only; overflow rejects.
    Float32Le,
    SignedExact(pcm::SignedPcm),
}
impl Encoding {
    fn width(self) -> usize {
        match self {
            Self::Float64Le => 8,
            Self::Float32Le => 4,
            Self::SignedExact(format) => format.bytes_per_sample(),
        }
    }
}

#[derive(Debug)]
pub enum ConsumerError {
    Queue(QueueError),
    Binding,
    Budget,
    Conversion,
    Exact(pcm::PcmError),
    Io(io::Error),
    Stopped,
}

/// A consumer owns one output binding, preallocated scratch and an optional
/// exclusively created file. Drain performs file I/O and is NEVER a callback API.
/// Drain each output independently off render; this is not an interthread SPSC.
pub struct Consumer {
    output: usize,
    binding: Binding,
    format: Format,
    encoding: Encoding,
    samples: Vec<f64>,
    bytes: Vec<u8>,
    file: Option<File>,
    max_total_frames: u64,
    frames: u64,
    active: bool,
}

impl Consumer {
    pub fn null(
        engine: &mut LinuxAudio,
        output: usize,
        max_total_frames: u64,
        max_scratch_bytes: usize,
    ) -> Result<Self, ConsumerError> {
        let consumer = Self::prepare(
            engine,
            output,
            Encoding::Float64Le,
            max_total_frames,
            max_scratch_bytes,
            false,
        )?;
        engine.consumer_claimed[output] = true;
        Ok(consumer)
    }

    pub fn create_file(
        engine: &mut LinuxAudio,
        output: usize,
        path: &Path,
        encoding: Encoding,
        max_total_frames: u64,
        max_scratch_bytes: usize,
    ) -> Result<Self, ConsumerError> {
        let mut consumer = Self::prepare(
            engine,
            output,
            encoding,
            max_total_frames,
            max_scratch_bytes,
            true,
        )?;
        // create_new rejects existing files and final symlinks atomically. Keep
        // this acquired descriptor; never reopen by name or truncate old output.
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(ConsumerError::Io)?;
        consumer.file = Some(file);
        engine.consumer_claimed[output] = true;
        Ok(consumer)
    }

    fn prepare(
        engine: &LinuxAudio,
        output: usize,
        encoding: Encoding,
        max_total_frames: u64,
        max_scratch_bytes: usize,
        file: bool,
    ) -> Result<Self, ConsumerError> {
        let queue = engine
            .outputs
            .get(output)
            .and_then(Option::as_ref)
            .ok_or(ConsumerError::Binding)?;
        if engine.consumer_claimed[output] {
            return Err(ConsumerError::Binding);
        }
        if !engine.active || !queue.active() {
            return Err(ConsumerError::Stopped);
        }
        let count = queue
            .max_frames()
            .checked_mul(queue.format().channels.len())
            .ok_or(ConsumerError::Budget)?;
        let bytes = if file {
            count
                .checked_mul(encoding.width())
                .ok_or(ConsumerError::Budget)?
        } else {
            0
        };
        let required = count
            .checked_mul(8)
            .and_then(|n| n.checked_add(bytes))
            .ok_or(ConsumerError::Budget)?;
        if max_total_frames == 0 || required > max_scratch_bytes {
            return Err(ConsumerError::Budget);
        }
        Ok(Self {
            output,
            binding: engine.binding,
            format: queue.format().clone(),
            encoding,
            samples: vec![0.0; count],
            bytes: vec![0; bytes],
            file: None,
            max_total_frames,
            frames: 0,
            active: true,
        })
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn format(&self) -> &Format {
        &self.format
    }
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// Empty is a normal silence/no-data result. Every other error terminally
    /// stops this consumer and revokes only its output queue, preventing retry
    /// after a partial file write or conversion failure. No automatic reopen.
    pub fn drain(
        &mut self,
        engine: &mut LinuxAudio,
        now_ns: u64,
    ) -> Result<Option<Block>, ConsumerError> {
        if !self.active {
            return Err(ConsumerError::Stopped);
        }
        let result = self.drain_one(engine, now_ns);
        if result.is_err() {
            self.active = false;
            self.samples.fill(0.0);
            self.bytes.fill(0);
            // A stale consumer must not revoke a new session's queue.
            if engine.binding == self.binding {
                if let Some(Some(queue)) = engine.outputs.get_mut(self.output) {
                    queue.revoke();
                }
            }
        }
        result
    }

    fn drain_one(
        &mut self,
        engine: &mut LinuxAudio,
        now_ns: u64,
    ) -> Result<Option<Block>, ConsumerError> {
        if !engine.active || engine.binding != self.binding {
            return Err(ConsumerError::Binding);
        }
        let queue = engine
            .outputs
            .get_mut(self.output)
            .and_then(Option::as_mut)
            .ok_or(ConsumerError::Binding)?;
        if queue.format() != &self.format {
            return Err(ConsumerError::Binding);
        }
        let block = match queue.pop(now_ns, self.binding, &mut self.samples) {
            Ok(block) => block,
            Err(QueueError::Empty) => return Ok(None),
            Err(error) => return Err(ConsumerError::Queue(error)),
        };
        let frames = self
            .frames
            .checked_add(block.frames as u64)
            .filter(|n| *n <= self.max_total_frames)
            .ok_or(ConsumerError::Budget)?;
        let count = block.frames * self.format.channels.len();
        if let Some(file) = &mut self.file {
            let samples = &self.samples[..count];
            let bytes = &mut self.bytes[..count * self.encoding.width()];
            match self.encoding {
                Encoding::Float64Le => {
                    for (value, out) in samples.iter().zip(bytes.as_chunks_mut::<8>().0.iter_mut())
                    {
                        out.copy_from_slice(&value.to_le_bytes());
                    }
                }
                Encoding::Float32Le => {
                    if samples.iter().any(|v| !(*v as f32).is_finite()) {
                        return Err(ConsumerError::Conversion);
                    }
                    for (value, out) in samples.iter().zip(bytes.as_chunks_mut::<4>().0.iter_mut())
                    {
                        out.copy_from_slice(&(*value as f32).to_le_bytes());
                    }
                }
                Encoding::SignedExact(format) => {
                    pcm::encode_exact(format, samples, bytes).map_err(ConsumerError::Exact)?
                }
            }
            file.write_all(bytes).map_err(ConsumerError::Io)?;
        }
        self.frames = frames;
        Ok(Some(block))
    }
}
