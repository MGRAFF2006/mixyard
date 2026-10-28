use crate::protocol::{Frame, Limits, Offer, Permissions, VERSION};
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub accepted: u64,
    pub missing_silence: u64,
    pub disconnected_silence: u64,
    pub duplicate: u64,
    pub late: u64,
    pub stale_generation: u64,
    pub beyond_window: u64,
    pub malformed: u64,
    pub unadmitted: u64,
    pub send_overflow: u64,
    pub congestion_drop: u64,
    pub send_error: u64,
}

struct Timeline {
    offer: Offer,
    next: u64,
    packets: BTreeMap<u64, Vec<u8>>,
    counters: Counters,
}

/// Bounded reorder/jitter window driven by the consumer's sample timeline.
/// Call pull at the negotiated packet cadence, after a caller-chosen initial prebuffer.
/// This allocating worker API must never be called from a realtime callback.
pub struct Receiver {
    permissions: Permissions,
    limits: Limits,
    connected: bool,
    streams: BTreeMap<u64, Timeline>,
    pub rejected: Counters,
}

impl Receiver {
    pub fn new(permissions: Permissions, limits: Limits) -> Result<Self> {
        permissions.validate(&limits)?;
        Ok(Self {
            permissions,
            limits,
            connected: true,
            streams: BTreeMap::new(),
            rejected: Counters::default(),
        })
    }

    pub fn admit(&mut self, offer: Offer) -> Result<()> {
        ensure!(
            self.connected,
            "disconnected: explicit new session required"
        );
        ensure!(
            offer.version == VERSION && offer.generation > 0,
            "unsupported version/zero generation"
        );
        ensure!(
            self.permissions.receive.get(&offer.spec.stream) == Some(&offer.spec),
            "stream/format/provenance is not explicitly permitted"
        );
        if let Some(previous) = self.streams.get(&offer.spec.stream) {
            ensure!(
                offer.generation > previous.offer.generation,
                "generation must increase on restart"
            );
        }
        // Permission validation bounds all unique slots and aggregate admitted rate.
        self.streams.insert(
            offer.spec.stream,
            Timeline {
                offer,
                next: 0,
                packets: BTreeMap::new(),
                counters: Counters::default(),
            },
        );
        Ok(())
    }

    pub fn ingest(&mut self, data: &[u8]) {
        if !self.connected {
            self.rejected.unadmitted += 1;
            return;
        }
        let frame = match Frame::decode(data, self.limits.max_datagram) {
            Ok(frame) => frame,
            Err(_) => {
                self.rejected.malformed += 1;
                return;
            }
        };
        let Some(stream) = self.streams.get_mut(&frame.stream) else {
            self.rejected.unadmitted += 1;
            return;
        };
        if frame.generation != stream.offer.generation {
            stream.counters.stale_generation += 1;
        } else if frame.validate(&stream.offer).is_err() {
            stream.counters.malformed += 1;
        } else if frame.sequence < stream.next {
            stream.counters.late += 1;
        } else if frame.sequence - stream.next >= self.limits.packets_per_stream as u64 {
            stream.counters.beyond_window += 1;
        } else if let std::collections::btree_map::Entry::Vacant(entry) =
            stream.packets.entry(frame.sequence)
        {
            entry.insert(frame.payload);
            stream.counters.accepted += 1;
        } else {
            stream.counters.duplicate += 1;
        }
    }

    /// Exactly one negotiated packet's PCM bytes, preserving raw payload bits.
    /// Missing/expired packets contribute all-zero PCM and observable counters.
    pub fn pull(&mut self, id: u64) -> Result<Vec<u8>> {
        let stream = self
            .streams
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted stream"))?;
        let silence = || -> Result<Vec<u8>> { Ok(vec![0; stream.offer.spec.payload_bytes()?]) };
        if !self.connected {
            stream.counters.disconnected_silence += 1;
            return silence();
        }
        let next = stream
            .next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("sequence exhausted: restart with a new generation"))?;
        let payload = match stream.packets.remove(&stream.next) {
            Some(payload) => payload,
            None => {
                stream.counters.missing_silence += 1;
                silence()?
            }
        };
        stream.next = next;
        Ok(payload)
    }

    pub fn counters(&self, id: u64) -> Result<Counters> {
        Ok(self
            .streams
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted stream"))?
            .counters
            .clone())
    }

    pub fn queued(&self, id: u64) -> usize {
        self.streams.get(&id).map_or(0, |s| s.packets.len())
    }

    pub fn disconnect(&mut self) {
        self.connected = false;
        for stream in self.streams.values_mut() {
            stream.packets.clear();
        }
    }
}

struct Pending {
    offer: Offer,
    next: u64,
    packets: VecDeque<Frame>,
    overflows: u64,
    congestion_drops: u64,
    send_errors: u64,
    partial: Vec<u8>,
}

impl Pending {
    fn enqueue(&mut self, payload: Vec<u8>, limit: usize) -> Result<()> {
        ensure!(limit > 0, "empty sender queue budget");
        let next = self
            .next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("sequence exhausted"))?;
        let frame = Frame {
            stream: self.offer.spec.stream,
            generation: self.offer.generation,
            sequence: self.next,
            sample_timestamp: self
                .next
                .checked_mul(u64::from(self.offer.spec.frames_per_packet))
                .ok_or_else(|| anyhow::anyhow!("sample timestamp exhausted"))?,
            payload,
        };
        frame.validate(&self.offer)?;
        if self.packets.len() == limit {
            self.packets.pop_front();
            self.overflows += 1;
        }
        self.packets.push_back(frame);
        self.next = next;
        Ok(())
    }
}

/// Each round takes at most one packet per admitted stream. Producers cannot grow
/// a shared FIFO or wait on a sibling. Congestion remains shared at the QUIC layer.
pub struct Outbox {
    limit: usize,
    streams: BTreeMap<u64, Pending>,
    round_start: usize,
}

impl Outbox {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            streams: BTreeMap::new(),
            round_start: 0,
        }
    }

    pub fn admit(&mut self, offer: Offer) {
        self.streams.insert(
            offer.spec.stream,
            Pending {
                offer,
                next: 0,
                packets: VecDeque::new(),
                overflows: 0,
                congestion_drops: 0,
                send_errors: 0,
                partial: Vec::new(),
            },
        );
    }

    pub fn generation(&self, id: u64) -> Option<u64> {
        self.streams.get(&id).map(|s| s.offer.generation)
    }

    pub fn enqueue(&mut self, id: u64, payload: Vec<u8>) -> Result<()> {
        let stream = self
            .streams
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted sender"))?;
        ensure!(
            stream.partial.is_empty(),
            "pending partial PCM: continue through enqueue_pcm"
        );
        stream.enqueue(payload, self.limit)
    }

    /// Packetize whole interleaved sample frames; at most one partial packet is retained.
    /// No padding, truncation, conversion or cross-datagram sample-frame fragmentation.
    pub fn enqueue_pcm(&mut self, id: u64, pcm: &[u8]) -> Result<usize> {
        let stream = self
            .streams
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted sender"))?;
        let packet_bytes = stream.offer.spec.payload_bytes()?;
        ensure!(
            pcm.len()
                <= packet_bytes
                    .checked_mul(self.limit)
                    .ok_or_else(|| anyhow::anyhow!("PCM block budget overflow"))?,
            "PCM block exceeds per-stream worker budget: submit smaller sample-aligned chunks"
        );
        let frame_bytes =
            stream.offer.spec.codec.sample_bytes()? * stream.offer.spec.channels.len();
        ensure!(
            frame_bytes > 0 && packet_bytes > 0 && pcm.len().is_multiple_of(frame_bytes),
            "PCM block must contain whole ordered sample frames"
        );
        let mut bytes = pcm;
        let mut packets = 0;
        while !bytes.is_empty() {
            let take = (packet_bytes - stream.partial.len()).min(bytes.len());
            stream.partial.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if stream.partial.len() == packet_bytes {
                let payload = std::mem::take(&mut stream.partial);
                stream.enqueue(payload, self.limit)?;
                packets += 1;
            }
        }
        Ok(packets)
    }

    pub fn round(&mut self) -> Vec<Frame> {
        let mut frames: Vec<_> = self
            .streams
            .values_mut()
            .filter_map(|s| s.packets.pop_front())
            .collect();
        if !frames.is_empty() {
            let start = self.round_start % frames.len();
            frames.rotate_left(start);
            self.round_start = start + 1;
        }
        frames
    }

    pub fn overflows(&self, id: u64) -> u64 {
        self.streams.get(&id).map_or(0, |s| s.overflows)
    }
    pub fn congestion_drop(&mut self, id: u64) {
        if let Some(stream) = self.streams.get_mut(&id) {
            stream.congestion_drops += 1;
        }
    }
    pub fn send_error(&mut self, id: u64) {
        if let Some(stream) = self.streams.get_mut(&id) {
            stream.send_errors += 1;
        }
    }
    pub fn counters(&self, id: u64) -> Result<Counters> {
        let stream = self
            .streams
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("unadmitted sender"))?;
        Ok(Counters {
            send_overflow: stream.overflows,
            congestion_drop: stream.congestion_drops,
            send_error: stream.send_errors,
            ..Counters::default()
        })
    }
    pub fn clear(&mut self) {
        self.streams.clear();
    }
}
