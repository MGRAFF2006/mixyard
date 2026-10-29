//! Explicit destination-local PCM adaptation. Construction/drop happen off callback.
//! Linear interpolation is not band-limited and is not a quality resampler.
//! No clock measurement, device negotiation, physical alignment or native I/O.

use std::mem::size_of;

/// Exact lane identities in interleaved sample order. No inferred speaker layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format {
    pub sample_rate_hz: u32,
    pub channels: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub generation: u64,
    pub revision: u64,
}

/// All times belong to the same host monotonic nanosecond domain as `now_ns`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub binding: Binding,
    pub clock_id: u64,
    pub sequence: u64,
    pub first_frame: u64,
    pub captured_ns: u64,
    pub deadline_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub stamp: Stamp,
    pub frames: usize,
}

/// Host resource admission, not format/rate/channel product ceilings.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_channels: usize,
    pub max_channel_label_bytes: usize,
    pub max_input_frames: usize,
    pub max_output_frames: usize,
    pub capacity_frames: usize,
    /// Ring samples, output samples and two u64 timestamps per ring frame.
    pub max_buffer_bytes: usize,
    /// Explicit correction range; must be less than one million ppm.
    pub max_correction_ppm: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Requires equal rates and the SAME clock ID. Copies the selected type's bits.
    Bypass,
    /// Explicit two-point interpolation. No anti-alias filter or dither.
    Linear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidFormat,
    ChannelOrder,
    ConversionRequired,
    Budget,
    Closed,
    Binding,
    Clock,
    Discontinuity,
    Expired,
    TimeReversed,
    Length,
    NonFinite,
    NotRepresentable,
    Overflow,
    Underflow,
    Correction,
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for f32 {}
    impl Sealed for f64 {}
}

/// Only IEEE754 Float32/Float64. The type is part of the exact PCM descriptor.
pub trait ClockSample: sealed::Sealed + Copy {
    fn widen(self) -> f64;
    fn from_f64(value: f64) -> Self;
}
impl ClockSample for f32 {
    fn widen(self) -> f64 {
        self as f64
    }
    fn from_f64(value: f64) -> Self {
        value as f32
    }
}
impl ClockSample for f64 {
    fn widen(self) -> f64 {
        self
    }
    fn from_f64(value: f64) -> Self {
        value
    }
}

fn validate_format(format: &Format, limits: Limits) -> Result<(), Error> {
    if format.sample_rate_hz == 0 || format.channels.is_empty() {
        return Err(Error::InvalidFormat);
    }
    if format.channels.len() > limits.max_channels {
        return Err(Error::Budget);
    }
    for (i, label) in format.channels.iter().enumerate() {
        if label.is_empty() {
            return Err(Error::InvalidFormat);
        }
        if label.len() > limits.max_channel_label_bytes {
            return Err(Error::Budget);
        }
        if format.channels[..i].contains(label) {
            return Err(Error::InvalidFormat);
        }
    }
    Ok(())
}

fn sample_count(frames: usize, channels: usize) -> Result<usize, Error> {
    frames.checked_mul(channels).ok_or(Error::Budget)
}

/// Does not infer clock equivalence or authorize a binding. The host owns those.
struct Gate {
    binding: Binding,
    clock_id: u64,
    active: bool,
    sequence: u64,
    first_frame: u64,
    last_capture: u64,
    last_now: u64,
}
impl Gate {
    fn new(binding: Binding, clock_id: u64) -> Result<Self, Error> {
        if binding.generation == 0 || binding.revision == 0 || clock_id == 0 {
            return Err(Error::Binding);
        }
        Ok(Self {
            binding,
            clock_id,
            active: true,
            sequence: 0,
            first_frame: 0,
            last_capture: 0,
            last_now: 0,
        })
    }
    fn check(&self, binding: Binding, now: u64) -> Result<(), Error> {
        if !self.active {
            return Err(Error::Closed);
        }
        if binding != self.binding {
            return Err(Error::Binding);
        }
        if now < self.last_now {
            return Err(Error::TimeReversed);
        }
        Ok(())
    }
    fn accept(&mut self, stamp: Stamp, now: u64, frames: usize) -> Result<(), Error> {
        self.check(stamp.binding, now)?;
        if stamp.clock_id != self.clock_id {
            return Err(Error::Clock);
        }
        if stamp.sequence != self.sequence || stamp.first_frame != self.first_frame {
            return Err(Error::Discontinuity);
        }
        if stamp.captured_ns < self.last_capture
            || now < stamp.captured_ns
            || now > stamp.deadline_ns
            || stamp.captured_ns > stamp.deadline_ns
        {
            return Err(Error::Expired);
        }
        let frame_count = u64::try_from(frames).map_err(|_| Error::Overflow)?;
        self.first_frame = self
            .first_frame
            .checked_add(frame_count)
            .ok_or(Error::Overflow)?;
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Overflow)?;
        self.last_capture = stamp.captured_ns;
        self.last_now = now;
        Ok(())
    }
}

/// One independent destination's ring and rational phase. No shared source mutation.
/// Allocate/validate once, then push/render/revoke without allocation, locks or I/O.
pub struct RateAdapter<S: ClockSample> {
    source: Format,
    destination: Format,
    method: Method,
    limits: Limits,
    gate: Gate,
    output_clock_id: u64,
    ring: Vec<S>,
    captured: Vec<u64>,
    deadlines: Vec<u64>,
    output: Vec<S>,
    output_frames: usize,
    output_sequence: u64,
    output_first_frame: u64,
    head: usize,
    len: usize,
    // Phase is relative to ring head in units of destination_rate * 1_000_000.
    // Keeping the fractional remainder avoids accumulated floating-point drift.
    phase: u128,
    denominator: u128,
    step: u128,
}

impl<S: ClockSample> RateAdapter<S> {
    pub fn new(
        source: Format,
        destination: Format,
        binding: Binding,
        input_clock_id: u64,
        output_clock_id: u64,
        method: Method,
        limits: Limits,
    ) -> Result<Self, Error> {
        validate_format(&source, limits)?;
        validate_format(&destination, limits)?;
        if source.channels != destination.channels {
            return Err(Error::ChannelOrder);
        }
        if output_clock_id == 0 {
            return Err(Error::Clock);
        }
        if method == Method::Bypass
            && (source.sample_rate_hz != destination.sample_rate_hz
                || input_clock_id != output_clock_id)
        {
            return Err(Error::ConversionRequired);
        }
        if limits.max_input_frames == 0
            || limits.max_output_frames == 0
            || limits.capacity_frames == 0
            || limits.max_correction_ppm >= 1_000_000
        {
            return Err(Error::Budget);
        }
        let ring_samples = sample_count(limits.capacity_frames, source.channels.len())?;
        let output_samples = sample_count(limits.max_output_frames, source.channels.len())?;
        let bytes = ring_samples
            .checked_add(output_samples)
            .and_then(|n| n.checked_mul(size_of::<S>()))
            .and_then(|n| {
                limits
                    .capacity_frames
                    .checked_mul(16)
                    .and_then(|t| n.checked_add(t))
            })
            .ok_or(Error::Budget)?;
        if bytes > limits.max_buffer_bytes || bytes > isize::MAX as usize {
            return Err(Error::Budget);
        }
        let gate = Gate::new(binding, input_clock_id)?;
        let denominator = destination.sample_rate_hz as u128 * 1_000_000;
        let step = source.sample_rate_hz as u128 * 1_000_000;
        Ok(Self {
            source,
            destination,
            method,
            limits,
            gate,
            output_clock_id,
            ring: vec![S::from_f64(0.0); ring_samples],
            captured: vec![0; limits.capacity_frames],
            deadlines: vec![0; limits.capacity_frames],
            output: vec![S::from_f64(0.0); output_samples],
            output_frames: 0,
            output_sequence: 0,
            output_first_frame: 0,
            head: 0,
            len: 0,
            phase: 0,
            denominator,
            step,
        })
    }

    pub fn source_format(&self) -> &Format {
        &self.source
    }
    pub fn destination_format(&self) -> &Format {
        &self.destination
    }
    pub fn method(&self) -> Method {
        self.method
    }
    pub fn buffered_frames(&self) -> usize {
        self.len
    }
    pub fn is_active(&self) -> bool {
        self.gate.active
    }
    pub fn output(&self) -> &[S] {
        &self.output[..self.output_frames * self.source.channels.len()]
    }

    /// Terminal flush. Recovery requires a new object and a fresh host binding.
    pub fn revoke(&mut self) {
        self.gate.active = false;
        self.ring.fill(S::from_f64(0.0));
        self.captured.fill(0);
        self.deadlines.fill(0);
        self.output.fill(S::from_f64(0.0));
        self.head = 0;
        self.len = 0;
        self.phase = 0;
    }
    fn fail<T>(&mut self, error: Error) -> Result<T, Error> {
        self.revoke();
        Err(error)
    }

    /// Positive ppm consumes source frames faster. Supply a measured correction;
    /// this method does NOT estimate drift or establish a timestamp correspondence.
    pub fn set_correction_ppm(&mut self, binding: Binding, ppm: i32) -> Result<(), Error> {
        self.output.fill(S::from_f64(0.0));
        self.output_frames = 0;
        if let Err(error) = self.gate.check(binding, self.gate.last_now) {
            return self.fail(error);
        }
        if self.method != Method::Linear {
            return self.fail(Error::ConversionRequired);
        }
        if ppm.unsigned_abs() > self.limits.max_correction_ppm {
            return self.fail(Error::Correction);
        }
        self.step = self.source.sample_rate_hz as u128 * (1_000_000i64 + ppm as i64) as u128;
        Ok(())
    }

    pub fn push(
        &mut self,
        now_ns: u64,
        stamp: Stamp,
        frames: usize,
        samples: &[S],
    ) -> Result<(), Error> {
        self.output.fill(S::from_f64(0.0));
        self.output_frames = 0;
        if frames == 0
            || frames > self.limits.max_input_frames
            || frames.checked_mul(self.source.channels.len()) != Some(samples.len())
        {
            return self.fail(Error::Length);
        }
        if frames > self.limits.capacity_frames - self.len {
            return self.fail(Error::Overflow);
        }
        if samples.iter().any(|s| !s.widen().is_finite()) {
            return self.fail(Error::NonFinite);
        }
        if let Err(error) = self.gate.accept(stamp, now_ns, frames) {
            return self.fail(error);
        }
        let channels = self.source.channels.len();
        for frame in 0..frames {
            let slot = (self.head + self.len + frame) % self.limits.capacity_frames;
            self.ring[slot * channels..(slot + 1) * channels]
                .copy_from_slice(&samples[frame * channels..(frame + 1) * channels]);
            self.captured[slot] = stamp.captured_ns;
            self.deadlines[slot] = stamp.deadline_ns;
        }
        self.len += frames;
        self.discard_passed_frames();
        Ok(())
    }

    fn discard_passed_frames(&mut self) {
        let discard = (self.phase / self.denominator).min(self.len as u128) as usize;
        self.head = (self.head + discard) % self.limits.capacity_frames;
        self.len -= discard;
        self.phase -= discard as u128 * self.denominator;
    }

    pub fn render(&mut self, now_ns: u64, binding: Binding, frames: usize) -> Result<Block, Error> {
        self.output.fill(S::from_f64(0.0));
        self.output_frames = frames.min(self.limits.max_output_frames);
        if frames == 0 || frames > self.limits.max_output_frames {
            return self.fail(Error::Length);
        }
        if let Err(error) = self.gate.check(binding, now_ns) {
            return self.fail(error);
        }
        let Some(next_frame) = self.output_first_frame.checked_add(frames as u64) else {
            return self.fail(Error::Overflow);
        };
        let Some(next_sequence) = self.output_sequence.checked_add(1) else {
            return self.fail(Error::Overflow);
        };
        let channels = self.source.channels.len();
        let mut captured_ns = u64::MAX;
        let mut deadline_ns = u64::MAX;
        for frame in 0..frames {
            let index = self.phase / self.denominator;
            let fraction = self.phase % self.denominator;
            let lookahead = usize::from(fraction != 0);
            if index + lookahead as u128 >= self.len as u128 {
                return self.fail(Error::Underflow);
            }
            let left = (self.head + index as usize) % self.limits.capacity_frames;
            let right = (left + lookahead) % self.limits.capacity_frames;
            if now_ns > self.deadlines[left] || now_ns > self.deadlines[right] {
                return self.fail(Error::Expired);
            }
            captured_ns = captured_ns
                .min(self.captured[left])
                .min(self.captured[right]);
            deadline_ns = deadline_ns
                .min(self.deadlines[left])
                .min(self.deadlines[right]);
            for channel in 0..channels {
                let a = self.ring[left * channels + channel];
                let value = if fraction == 0 {
                    a // Preserve signed zero/subnormal/low bits on integral positions.
                } else {
                    let t = fraction as f64 / self.denominator as f64;
                    let b = self.ring[right * channels + channel];
                    S::from_f64(a.widen() * (1.0 - t) + b.widen() * t)
                };
                if !value.widen().is_finite() {
                    return self.fail(Error::NonFinite);
                }
                self.output[frame * channels + channel] = value;
            }
            self.phase += self.step;
            self.discard_passed_frames();
        }
        self.gate.last_now = now_ns;
        let stamp = Stamp {
            binding,
            clock_id: self.output_clock_id,
            sequence: self.output_sequence,
            first_frame: self.output_first_frame,
            captured_ns,
            deadline_ns,
        };
        self.output_sequence = next_sequence;
        self.output_first_frame = next_frame;
        Ok(Block { stamp, frames })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Narrowing {
    Exact,
    RoundToNearest,
}

/// Destination-local precision conversion; no clipping, normalization or dither.
/// Whole destination is zero on error. Caller admits slice sizes off callback.
pub fn narrow_f64(input: &[f64], output: &mut [f32], policy: Narrowing) -> Result<(), Error> {
    output.fill(0.0);
    if input.len() != output.len() {
        return Err(Error::Length);
    }
    for value in input {
        if !value.is_finite() || !(*value as f32).is_finite() {
            return Err(Error::NonFinite);
        }
        if policy == Narrowing::Exact && (*value as f32) as f64 != *value {
            return Err(Error::NotRepresentable);
        }
    }
    for (source, destination) in input.iter().zip(output) {
        *destination = *source as f32;
    }
    Ok(())
}

pub fn widen_f32(input: &[f32], output: &mut [f64]) -> Result<(), Error> {
    output.fill(0.0);
    if input.len() != output.len() {
        return Err(Error::Length);
    }
    if input.iter().any(|value| !value.is_finite()) {
        return Err(Error::NonFinite);
    }
    for (source, destination) in input.iter().zip(output) {
        *destination = *source as f64;
    }
    Ok(())
}

/// Packed PCM encodings, with no native-endian or padding assumptions. Integer
/// precision wider than Float64 is retained here without a float conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawEncoding {
    S16Le,
    S16Be,
    S24Le,
    S24Be,
    S32Le,
    S32Be,
    S64Le,
    S64Be,
    F32Le,
    F32Be,
    F64Le,
    F64Be,
}
impl RawEncoding {
    fn width(self) -> usize {
        match self {
            Self::S16Le | Self::S16Be => 2,
            Self::S24Le | Self::S24Be => 3,
            Self::S32Le | Self::S32Be | Self::F32Le | Self::F32Be => 4,
            Self::S64Le | Self::S64Be | Self::F64Le | Self::F64Be => 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawFormat {
    pub format: Format,
    pub encoding: RawEncoding,
}

/// A bounded, synchronous raw PCM bypass. Each call replaces the whole output;
/// host binding/freshness checks precede copy. It never interprets sample bits.
pub struct RawBypass {
    format: RawFormat,
    gate: Gate,
    frame_bytes: usize,
    max_frames: usize,
    output: Vec<u8>,
    bytes: usize,
}
impl RawBypass {
    pub fn new(
        source: RawFormat,
        destination: RawFormat,
        binding: Binding,
        clock_id: u64,
        limits: Limits,
    ) -> Result<Self, Error> {
        validate_format(&source.format, limits)?;
        validate_format(&destination.format, limits)?;
        if source != destination {
            return Err(Error::ConversionRequired);
        }
        let frame_bytes = sample_count(source.format.channels.len(), source.encoding.width())?;
        let max_frames = limits.max_input_frames.min(limits.max_output_frames);
        let bytes = sample_count(max_frames, frame_bytes)?;
        if max_frames == 0 || bytes > limits.max_buffer_bytes || bytes > isize::MAX as usize {
            return Err(Error::Budget);
        }
        Ok(Self {
            format: source,
            gate: Gate::new(binding, clock_id)?,
            frame_bytes,
            max_frames,
            output: vec![0; bytes],
            bytes: 0,
        })
    }
    pub fn format(&self) -> &RawFormat {
        &self.format
    }
    pub fn output(&self) -> &[u8] {
        &self.output[..self.bytes]
    }
    pub fn revoke(&mut self) {
        self.gate.active = false;
        self.output.fill(0);
    }
    pub fn copy(
        &mut self,
        now_ns: u64,
        stamp: Stamp,
        frames: usize,
        input: &[u8],
    ) -> Result<Block, Error> {
        self.output.fill(0);
        self.bytes = frames.min(self.max_frames) * self.frame_bytes;
        let result = if frames == 0
            || frames > self.max_frames
            || frames.checked_mul(self.frame_bytes) != Some(input.len())
        {
            Err(Error::Length)
        } else {
            self.gate.accept(stamp, now_ns, frames)
        };
        if let Err(error) = result {
            self.revoke();
            return Err(error);
        }
        self.output[..input.len()].copy_from_slice(input);
        Ok(Block { stamp, frames })
    }
}
