//! Exact-format, single-owner bounded PCM handoff.
//! Allocation is confined to construction/drop. This is not a cross-thread SPSC.
use crate::audio::{Precision, Sample};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub generation: u64,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Format {
    pub sample_rate_hz: u32,
    pub channels: Vec<String>,
    pub precision: Precision,
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Inactive,
    Binding,
    Clock,
    Timeline,
    Expired,
    Frames,
    Samples,
    NonFinite,
    Full,
    Empty,
    Destination,
}

struct Slot<S> {
    samples: Vec<S>,
    block: Option<Block>,
}

/// One queue belongs to one immutable endpoint binding and clock domain.
/// Endpoint identity/provenance/consent is the host's off-callback authorization
/// boundary; possession of numeric revisions alone grants no capture authority.
pub struct Queue<S: Sample> {
    format: Format,
    binding: Binding,
    clock_id: u64,
    max_frames: usize,
    slots: Vec<Slot<S>>,
    read: usize,
    write: usize,
    count: usize,
    active: bool,
    next_sequence: Option<u64>,
    next_frame: Option<u64>,
    last_capture_ns: Option<u64>,
    last_now_ns: Option<u64>,
}

impl<S: Sample> Queue<S> {
    /// Host-selected budgets, no product channel/rate/slot ceilings.
    /// max_bytes covers PCM slots; metadata/labels are separately host admitted.
    pub fn new(
        format: Format,
        binding: Binding,
        clock_id: u64,
        slots: usize,
        max_frames: usize,
        max_bytes: usize,
    ) -> Result<Self, String> {
        if binding.generation == 0
            || binding.revision == 0
            || clock_id == 0
            || slots == 0
            || max_frames == 0
            || format.sample_rate_hz == 0
            || format.channels.is_empty()
            || format.precision != S::PRECISION
        {
            return Err("worker: invalid binding, format or capacity".into());
        }
        for (index, channel) in format.channels.iter().enumerate() {
            if channel.is_empty() || format.channels[..index].contains(channel) {
                return Err("worker: ordered channels must be nonempty and unique".into());
            }
        }
        let samples = max_frames
            .checked_mul(format.channels.len())
            .ok_or("worker: frame capacity overflow")?;
        let bytes = samples
            .checked_mul(slots)
            .and_then(|n| n.checked_mul(std::mem::size_of::<S>()))
            .ok_or("worker: storage capacity overflow")?;
        if bytes > max_bytes
            || samples > isize::MAX as usize / std::mem::size_of::<S>()
            || slots > isize::MAX as usize / std::mem::size_of::<Slot<S>>()
        {
            return Err("worker: storage budget exceeded".into());
        }
        let zero = S::from_f64(0.0);
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(slots)
            .map_err(|_| "worker: metadata allocation failed")?;
        for _ in 0..slots {
            let mut pcm = Vec::new();
            pcm.try_reserve_exact(samples)
                .map_err(|_| "worker: PCM allocation failed")?;
            pcm.resize(samples, zero);
            storage.push(Slot {
                samples: pcm,
                block: None,
            });
        }
        Ok(Self {
            format,
            binding,
            clock_id,
            max_frames,
            slots: storage,
            read: 0,
            write: 0,
            count: 0,
            active: true,
            next_sequence: None,
            next_frame: None,
            last_capture_ns: None,
            last_now_ns: None,
        })
    }

    pub fn format(&self) -> &Format {
        &self.format
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn max_frames(&self) -> usize {
        self.max_frames
    }
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn queued_blocks(&self) -> usize {
        self.count
    }

    /// Irreversible for this object. Restart requires fresh host authorization
    /// and a newly constructed queue; never reset old authority into a new epoch.
    pub fn revoke(&mut self) {
        self.active = false;
        self.count = 0;
        for slot in &mut self.slots {
            slot.samples.fill(S::from_f64(0.0));
            slot.block = None;
        }
    }

    fn fail<T>(&mut self, error: Error) -> Result<T, Error> {
        self.revoke();
        Err(error)
    }

    fn check_now(&mut self, now_ns: u64) -> Result<(), Error> {
        if self.last_now_ns.is_some_and(|previous| now_ns < previous) {
            return self.fail(Error::Timeline);
        }
        self.last_now_ns = Some(now_ns);
        Ok(())
    }

    /// Exact selected-precision copy. Full/invalid data fails this endpoint
    /// closed; callers continue independent sibling queues.
    pub fn push(
        &mut self,
        now_ns: u64,
        stamp: Stamp,
        frames: usize,
        samples: &[S],
    ) -> Result<(), Error> {
        if !self.active {
            return Err(Error::Inactive);
        }
        self.check_now(now_ns)?;
        if stamp.binding != self.binding {
            return self.fail(Error::Binding);
        }
        if stamp.clock_id != self.clock_id {
            return self.fail(Error::Clock);
        }
        if frames == 0 || frames > self.max_frames {
            return self.fail(Error::Frames);
        }
        if frames.checked_mul(self.format.channels.len()) != Some(samples.len()) {
            return self.fail(Error::Samples);
        }
        if stamp.captured_ns > now_ns
            || stamp.deadline_ns < stamp.captured_ns
            || now_ns > stamp.deadline_ns
        {
            return self.fail(Error::Expired);
        }
        if self.next_sequence.is_some_and(|n| n != stamp.sequence)
            || self.next_frame.is_some_and(|n| n != stamp.first_frame)
            || self.last_capture_ns.is_some_and(|n| stamp.captured_ns < n)
        {
            return self.fail(Error::Timeline);
        }
        let Some(sequence) = stamp.sequence.checked_add(1) else {
            return self.fail(Error::Timeline);
        };
        let Ok(frames_u64) = u64::try_from(frames) else {
            return self.fail(Error::Frames);
        };
        let Some(frame) = stamp.first_frame.checked_add(frames_u64) else {
            return self.fail(Error::Timeline);
        };
        if samples.iter().any(|sample| !sample.widen().is_finite()) {
            return self.fail(Error::NonFinite);
        }
        if self.count == self.slots.len() {
            return self.fail(Error::Full);
        }
        let slot = &mut self.slots[self.write];
        slot.samples[..samples.len()].copy_from_slice(samples);
        slot.samples[samples.len()..].fill(S::from_f64(0.0));
        slot.block = Some(Block { stamp, frames });
        self.write = (self.write + 1) % self.slots.len();
        self.count += 1;
        self.next_sequence = Some(sequence);
        self.next_frame = Some(frame);
        self.last_capture_ns = Some(stamp.captured_ns);
        Ok(())
    }

    /// Destination is always silenced first, including inactive/empty/errors.
    /// It must hold at least max_frames * ordered channels scalar samples.
    /// All unused tail samples remain zero; a block is consumed exactly once.
    pub fn pop(
        &mut self,
        now_ns: u64,
        binding: Binding,
        destination: &mut [S],
    ) -> Result<Block, Error> {
        destination.fill(S::from_f64(0.0));
        if !self.active {
            return Err(Error::Inactive);
        }
        self.check_now(now_ns)?;
        if binding != self.binding {
            return self.fail(Error::Binding);
        }
        if destination.len() < self.max_frames * self.format.channels.len() {
            return self.fail(Error::Destination);
        }
        if self.count == 0 {
            return Err(Error::Empty);
        }
        let block = self.slots[self.read].block.expect("occupied ring slot");
        if now_ns > block.stamp.deadline_ns {
            return self.fail(Error::Expired);
        }
        let samples = block.frames * self.format.channels.len();
        let slot = &mut self.slots[self.read];
        destination[..samples].copy_from_slice(&slot.samples[..samples]);
        slot.samples.fill(S::from_f64(0.0));
        slot.block = None;
        self.read = (self.read + 1) % self.slots.len();
        self.count -= 1;
        Ok(block)
    }
}
