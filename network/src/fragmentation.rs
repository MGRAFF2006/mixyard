//! Optional MXAFv2 framing for opaque, exactly admitted blocks. Worker-only API.
//!
//! This module supplies no authentication or permission policy: a caller must
//! bind one instance to one authenticated session and admit exact granted specs.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub const HEADER: usize = 48;
pub const VERSION: u16 = 2;

#[derive(Clone, Debug)]
pub struct Config {
    pub max_block_bytes: usize,
    pub max_datagram: usize,
    /// Includes revoked identities retained as generation/permission tombstones.
    pub max_streams: usize,
    pub window: usize,
    /// Sum of reserved payload and fragment-presence bytes, across all streams.
    pub max_buffered_bytes: usize,
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfig,
    InvalidDescriptor,
    Budget,
    Unadmitted,
    Revoked,
    Generation,
    Window,
    Malformed,
    Conflict,
    Expired,
    TimeRollback,
    SequenceExhausted,
    Allocation,
    Disconnected,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "fragmentation: {self:?}")
    }
}
impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// Derived locally from the exact negotiated format. No rate/channel conversion.
/// Variable-size encoded blocks require a separately negotiated framing contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub stream: u64,
    pub generation: u64,
    pub block_bytes: usize,
    pub frames_per_block: u64,
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.max_block_bytes == 0
            || self.max_block_bytes > u32::MAX as usize
            || !(HEADER + 1..=1200).contains(&self.max_datagram)
            || self.max_streams == 0
            || self.window == 0
            || self.max_buffered_bytes == 0
            || self.timeout.is_zero()
            || Instant::now().checked_add(self.timeout).is_none()
            || self.max_streams.checked_mul(self.window).is_none()
        {
            return Err(Error::InvalidConfig);
        }
        Ok(())
    }

    fn payload_capacity(&self) -> usize {
        self.max_datagram - HEADER
    }

    fn reservation(&self, descriptor: Descriptor) -> Result<usize> {
        if descriptor.stream == 0
            || descriptor.generation == 0
            || descriptor.frames_per_block == 0
            || descriptor.block_bytes == 0
            || descriptor.block_bytes > self.max_block_bytes
        {
            return Err(Error::InvalidDescriptor);
        }
        descriptor
            .block_bytes
            .checked_add(descriptor.block_bytes.div_ceil(self.payload_capacity()))
            .and_then(|n| n.checked_mul(self.window))
            .ok_or(Error::Budget)
    }
}

/// Constant-space sender: holds configuration only; one <=1200-byte allocation
/// per iterator item. The caller owns pacing, sequence progression and send queues.
pub struct Fragmenter {
    config: Config,
    descriptor: Descriptor,
}

impl Fragmenter {
    pub fn new(config: Config, descriptor: Descriptor) -> Result<Self> {
        config.validate()?;
        if config.reservation(descriptor)? > config.max_buffered_bytes {
            return Err(Error::Budget);
        }
        Ok(Self { config, descriptor })
    }

    pub fn fragments<'a>(&'a self, sequence: u64, block: &'a [u8]) -> Result<Fragments<'a>> {
        if block.len() != self.descriptor.block_bytes {
            return Err(Error::InvalidDescriptor);
        }
        let timestamp = sequence
            .checked_mul(self.descriptor.frames_per_block)
            .ok_or(Error::SequenceExhausted)?;
        Ok(Fragments {
            sender: self,
            sequence,
            timestamp,
            block,
            offset: 0,
        })
    }
}

pub struct Fragments<'a> {
    sender: &'a Fragmenter,
    sequence: u64,
    timestamp: u64,
    block: &'a [u8],
    offset: usize,
}

impl Iterator for Fragments<'_> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset == self.block.len() {
            return None;
        }
        let end = self
            .offset
            .saturating_add(self.sender.config.payload_capacity())
            .min(self.block.len());
        let payload = &self.block[self.offset..end];
        let mut bytes = Vec::with_capacity(HEADER + payload.len());
        bytes.extend_from_slice(b"MXAF");
        bytes.extend_from_slice(&VERSION.to_be_bytes());
        bytes.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        for value in [
            self.sender.descriptor.stream,
            self.sender.descriptor.generation,
            self.sequence,
            self.timestamp,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&(self.block.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&(self.offset as u32).to_be_bytes());
        bytes.extend_from_slice(payload);
        self.offset = end;
        Some(bytes)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    pub stream: u64,
    pub generation: u64,
    pub sequence: u64,
    pub sample_timestamp: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Ready {
        stream: u64,
        generation: u64,
        sequence: u64,
    },
    Duplicate,
}

struct Pending {
    payload: Vec<u8>,
    present: Vec<u8>,
    remaining: usize,
    deadline: Instant,
}

enum Slot {
    Data(Pending),
    Failed(Error),
    Consumed,
}

struct Stream {
    descriptor: Descriptor,
    next: u64,
    reservation: usize,
    revoked: bool,
    slots: BTreeMap<u64, Slot>,
}

/// One authenticated session, independently reserved stream windows. No datagram
/// creates a stream or changes an admission. Completed blocks still expire.
pub struct Reassembler {
    config: Config,
    streams: BTreeMap<u64, Stream>,
    reserved: usize,
    last_now: Option<Instant>,
    connected: bool,
}

impl Reassembler {
    pub fn new(config: Config) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            streams: BTreeMap::new(),
            reserved: 0,
            last_now: None,
            connected: true,
        })
    }

    /// Trusted control path only, after exact format/provenance/consent checks.
    /// Restart needs a strictly newer generation; revocation needs a new session.
    pub fn admit(&mut self, descriptor: Descriptor) -> Result<()> {
        self.ensure_connected()?;
        let reservation = self.config.reservation(descriptor)?;
        let previous = self.streams.get(&descriptor.stream);
        if let Some(previous) = previous {
            if previous.revoked {
                return Err(Error::Revoked);
            }
            if descriptor.generation <= previous.descriptor.generation {
                return Err(Error::Generation);
            }
        } else if self.streams.len() >= self.config.max_streams {
            return Err(Error::Budget);
        }
        let reserved = self.reserved - previous.map_or(0, |s| s.reservation);
        let reserved = reserved.checked_add(reservation).ok_or(Error::Budget)?;
        if reserved > self.config.max_buffered_bytes {
            return Err(Error::Budget);
        }
        self.streams.insert(
            descriptor.stream,
            Stream {
                descriptor,
                next: 0,
                reservation,
                revoked: false,
                slots: BTreeMap::new(),
            },
        );
        self.reserved = reserved;
        Ok(())
    }

    /// Immediate local revocation, retaining identity so old traffic cannot re-admit.
    pub fn revoke(&mut self, stream: u64) -> Result<()> {
        self.ensure_connected()?;
        let stream = self.streams.get_mut(&stream).ok_or(Error::Unadmitted)?;
        stream.slots.clear();
        stream.revoked = true;
        self.reserved -= stream.reservation;
        stream.reservation = 0;
        Ok(())
    }

    /// Consumer cursor only. Missing/failed blocks are concealed by the consumer,
    /// never emitted as partial PCM here. Cannot move the cursor backwards.
    pub fn advance(&mut self, id: u64, generation: u64, next: u64) -> Result<()> {
        self.ensure_connected()?;
        let stream = self.streams.get_mut(&id).ok_or(Error::Unadmitted)?;
        check_stream(stream, generation)?;
        if next < stream.next {
            return Err(Error::Window);
        }
        stream.slots.retain(|sequence, _| *sequence >= next);
        stream.next = next;
        Ok(())
    }

    /// Parse/validate borrowed header before allocating from the admitted size.
    /// Canonical chunk offsets rule out overlaps and fragmentation-count attacks.
    pub fn ingest(&mut self, bytes: &[u8], now: Instant) -> Result<Outcome> {
        self.check_time(now)?;
        if bytes.len() <= HEADER
            || bytes.len() > self.config.max_datagram
            || &bytes[..4] != b"MXAF"
            || u16::from_be_bytes(bytes[4..6].try_into().unwrap()) != VERSION
            || usize::from(u16::from_be_bytes(bytes[6..8].try_into().unwrap()))
                != bytes.len() - HEADER
        {
            return Err(Error::Malformed);
        }
        let number = |offset| u64::from_be_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let (id, generation, sequence, timestamp) = (number(8), number(16), number(24), number(32));
        let block_bytes = u32::from_be_bytes(bytes[40..44].try_into().unwrap()) as usize;
        let offset = u32::from_be_bytes(bytes[44..48].try_into().unwrap()) as usize;
        let capacity = self.config.payload_capacity();
        let stream = self.streams.get_mut(&id).ok_or(Error::Unadmitted)?;
        check_stream(stream, generation)?;
        expire_stream(stream, now);
        if sequence < stream.next || sequence - stream.next >= self.config.window as u64 {
            return Err(Error::Window);
        }
        if block_bytes != stream.descriptor.block_bytes
            || sequence.checked_mul(stream.descriptor.frames_per_block) != Some(timestamp)
            || offset >= block_bytes
            || !offset.is_multiple_of(capacity)
            || bytes.len() - HEADER != capacity.min(block_bytes - offset)
        {
            return Err(Error::Malformed);
        }
        let index = offset / capacity;
        if let std::collections::btree_map::Entry::Vacant(entry) = stream.slots.entry(sequence) {
            let count = block_bytes.div_ceil(capacity);
            let deadline = now
                .checked_add(self.config.timeout)
                .ok_or(Error::InvalidConfig)?;
            entry.insert(Slot::Data(Pending {
                payload: zeros(block_bytes)?,
                present: zeros(count)?,
                remaining: count,
                deadline,
            }));
        }
        let slot = stream.slots.get_mut(&sequence).unwrap();
        let pending = match slot {
            Slot::Failed(error) => return Err(*error),
            Slot::Consumed => return Ok(Outcome::Duplicate),
            Slot::Data(pending) => pending,
        };
        let end = offset + bytes.len() - HEADER;
        if pending.present[index] != 0 {
            if pending.payload[offset..end] == bytes[HEADER..] {
                return Ok(Outcome::Duplicate);
            }
            *slot = Slot::Failed(Error::Conflict);
            return Err(Error::Conflict);
        }
        pending.payload[offset..end].copy_from_slice(&bytes[HEADER..]);
        pending.present[index] = 1;
        pending.remaining -= 1;
        Ok(if pending.remaining == 0 {
            Outcome::Ready {
                stream: id,
                generation,
                sequence,
            }
        } else {
            Outcome::Pending
        })
    }

    /// Moves a complete, unexpired block out once; its tombstone prevents replay.
    pub fn take(
        &mut self,
        id: u64,
        generation: u64,
        sequence: u64,
        now: Instant,
    ) -> Result<Option<Block>> {
        self.check_time(now)?;
        let stream = self.streams.get_mut(&id).ok_or(Error::Unadmitted)?;
        check_stream(stream, generation)?;
        expire_stream(stream, now);
        let Some(slot) = stream.slots.get_mut(&sequence) else {
            return Ok(None);
        };
        match slot {
            Slot::Failed(error) => return Err(*error),
            Slot::Consumed => return Ok(None),
            Slot::Data(pending) if pending.remaining != 0 => return Ok(None),
            Slot::Data(_) => (),
        }
        let Slot::Data(pending) = std::mem::replace(slot, Slot::Consumed) else {
            unreachable!()
        };
        Ok(Some(Block {
            stream: id,
            generation,
            sequence,
            sample_timestamp: sequence * stream.descriptor.frames_per_block,
            payload: pending.payload,
        }))
    }

    /// Schedule on a worker timer even if traffic stops. O(max_streams * window).
    pub fn expire(&mut self, now: Instant) -> Result<()> {
        self.check_time(now)?;
        for stream in self.streams.values_mut() {
            expire_stream(stream, now);
        }
        Ok(())
    }

    pub fn disconnect(&mut self) {
        self.streams.clear();
        self.reserved = 0;
        self.connected = false;
    }

    pub fn reserved_bytes(&self) -> usize {
        self.reserved
    }

    /// Payload plus one byte per canonical chunk, excluding bounded map metadata.
    pub fn buffered_bytes(&self) -> usize {
        self.streams
            .values()
            .flat_map(|s| s.slots.values())
            .map(|s| match s {
                Slot::Data(p) => p.payload.len() + p.present.len(),
                _ => 0,
            })
            .sum()
    }

    fn ensure_connected(&self) -> Result<()> {
        if self.connected {
            Ok(())
        } else {
            Err(Error::Disconnected)
        }
    }

    fn check_time(&mut self, now: Instant) -> Result<()> {
        self.ensure_connected()?;
        if self.last_now.is_some_and(|previous| now < previous) {
            return Err(Error::TimeRollback);
        }
        self.last_now = Some(now);
        Ok(())
    }
}

fn check_stream(stream: &Stream, generation: u64) -> Result<()> {
    if stream.revoked {
        Err(Error::Revoked)
    } else if generation != stream.descriptor.generation {
        Err(Error::Generation)
    } else {
        Ok(())
    }
}

fn expire_stream(stream: &mut Stream, now: Instant) {
    for slot in stream.slots.values_mut() {
        if matches!(slot, Slot::Data(pending) if now >= pending.deadline) {
            *slot = Slot::Failed(Error::Expired);
        }
    }
}

fn zeros(len: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_| Error::Allocation)?;
    bytes.resize(len, 0);
    Ok(bytes)
}
