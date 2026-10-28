use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const VERSION: u16 = 1;
pub const HEADER: usize = 40;
pub const CONTROL_LIMIT: usize = 8192;
pub const ALPN: &[u8] = b"mixyard-audio/1";

/// Resource budgets, not product mix-count limits. Both endpoints configure their own.
#[derive(Clone, Debug)]
pub struct Limits {
    pub max_streams: usize,
    pub max_connections: usize,
    pub packets_per_stream: usize,
    pub max_datagram: usize,
    pub max_bytes_per_second: u64,
    pub control_timeout: std::time::Duration,
}

impl Limits {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.max_streams > 0 && self.max_connections > 0 && self.packets_per_stream > 0,
            "empty resource budget"
        );
        ensure!(
            (HEADER + 2..=1200).contains(&self.max_datagram),
            "datagram budget outside protocol limit"
        );
        ensure!(
            self.max_bytes_per_second > 0 && !self.control_timeout.is_zero(),
            "empty rate/timeout budget"
        );
        self.queue_bytes()?;
        Ok(())
    }

    pub fn queue_bytes(&self) -> Result<usize> {
        self.max_streams
            .checked_mul(self.packets_per_stream)
            .and_then(|n| n.checked_mul(self.max_datagram))
            .ok_or_else(|| anyhow::anyhow!("queue budget overflow"))
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    RawS16Le,
    RawS24Le,
    RawS32Le,
    RawF32Le,
    RawF64Le,
    /// Recognized for an explicit unavailable-codec error, never advertised or selected.
    Opus,
}

impl Codec {
    pub fn sample_bytes(self) -> Result<usize> {
        match self {
            Self::RawS16Le => Ok(2),
            Self::RawS24Le => Ok(3),
            Self::RawS32Le | Self::RawF32Le => Ok(4),
            Self::RawF64Le => Ok(8),
            Self::Opus => bail!("Opus unavailable: lossy codec support is pending"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub peer: String,
    pub source: String,
    pub groups: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Original, unsummed logical source identities, including privacy/mix-minus groups.
    pub sources: Vec<Source>,
    /// In traversal order, ending with the sending peer. Receiver must not occur here.
    pub visited_peers: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub stream: u64,
    pub name: String,
    pub codec: Codec,
    pub sample_rate: u32,
    /// Ordered, explicit channel identifiers (e.g. FL, FR, FC); no implicit downmix.
    pub channels: Vec<String>,
    pub frames_per_packet: u16,
    pub provenance: Provenance,
}

fn identifier(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control),
        "invalid identity/name"
    );
    Ok(())
}

impl Spec {
    pub fn payload_bytes(&self) -> Result<usize> {
        self.codec
            .sample_bytes()?
            .checked_mul(self.channels.len())
            .and_then(|n| n.checked_mul(usize::from(self.frames_per_packet)))
            .ok_or_else(|| anyhow::anyhow!("PCM frame size overflow"))
    }

    pub fn wire_bytes_per_second(&self) -> Result<u64> {
        ensure!(self.frames_per_packet > 0, "empty packet");
        let bytes = self.payload_bytes()? + HEADER;
        Ok(
            (bytes as u64 * u64::from(self.sample_rate))
                .div_ceil(u64::from(self.frames_per_packet)),
        )
    }

    pub fn validate(&self, sender: &str, receiver: &str, limits: &Limits) -> Result<()> {
        identifier(sender)?;
        identifier(receiver)?;
        ensure!(
            sender != receiver && self.stream != 0,
            "invalid peer/stream identity"
        );
        identifier(&self.name)?;
        ensure!(
            self.sample_rate > 0 && !self.channels.is_empty(),
            "sample rate and channel layout must be explicit"
        );
        let mut channels = BTreeSet::new();
        for channel in &self.channels {
            identifier(channel)?;
            ensure!(channels.insert(channel), "duplicate channel identifier");
        }
        ensure!(self.frames_per_packet > 0, "empty packet");
        ensure!(
            self.payload_bytes()? + HEADER <= limits.max_datagram,
            "packet exceeds datagram budget: reduce frames_per_packet; a single sample frame must fit"
        );
        let p = &self.provenance;
        ensure!(
            !p.sources.is_empty() && p.visited_peers.last().map(String::as_str) == Some(sender),
            "missing source provenance/sender"
        );
        let mut peers = BTreeSet::new();
        for peer in &p.visited_peers {
            identifier(peer)?;
            ensure!(
                peer != receiver && peers.insert(peer),
                "peer loop in provenance"
            );
        }
        let mut sources = BTreeSet::new();
        for source in &p.sources {
            identifier(&source.peer)?;
            identifier(&source.source)?;
            ensure!(
                peers.contains(&source.peer),
                "source origin missing from path"
            );
            ensure!(
                sources.insert((&source.peer, &source.source)),
                "duplicate source path"
            );
            let mut groups = BTreeSet::new();
            for group in &source.groups {
                identifier(group)?;
                ensure!(groups.insert(group), "duplicate source group");
            }
        }
        // Bound metadata even when constructed locally, before encoding/allocating on control paths.
        ensure!(
            serde_json::to_vec(self)?.len() <= CONTROL_LIMIT / 2,
            "provenance metadata too large"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub version: u16,
    pub generation: u64,
    pub spec: Spec,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub version: u16,
    pub accepted: bool,
    pub reason: String,
}

/// Explicit directional allowlists, bound by Session to the authenticated peer.
/// Exact specs include provenance: a pre-mix containing excluded sources must be rejected upstream.
#[derive(Clone, Debug)]
pub struct Permissions {
    pub local_peer: String,
    pub remote_peer: String,
    pub send: BTreeMap<u64, Spec>,
    pub receive: BTreeMap<u64, Spec>,
    pub excluded_receive_groups: BTreeSet<String>,
}

impl Permissions {
    pub fn validate(&self, limits: &Limits) -> Result<()> {
        limits.validate()?;
        identifier(&self.local_peer)?;
        identifier(&self.remote_peer)?;
        ensure!(self.local_peer != self.remote_peer, "peer loop");
        for (specs, sender, receiver) in [
            (&self.send, &self.local_peer, &self.remote_peer),
            (&self.receive, &self.remote_peer, &self.local_peer),
        ] {
            ensure!(
                specs.len() <= limits.max_streams,
                "stream admission limit exceeded"
            );
            let mut rate = 0u64;
            for (id, spec) in specs {
                ensure!(*id == spec.stream, "permission key mismatch");
                spec.validate(sender, receiver, limits)?;
                rate = rate
                    .checked_add(spec.wire_bytes_per_second()?)
                    .ok_or_else(|| anyhow::anyhow!("rate overflow"))?;
            }
            ensure!(
                rate <= limits.max_bytes_per_second,
                "configured stream rate exceeds admission budget"
            );
        }
        for spec in self.receive.values() {
            ensure!(
                !spec
                    .provenance
                    .sources
                    .iter()
                    .flat_map(|s| &s.groups)
                    .any(|g| self.excluded_receive_groups.contains(g)),
                "excluded source in premix: rebuild a safe upstream mix"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub stream: u64,
    pub generation: u64,
    pub sequence: u64,
    /// First sample-frame offset in this generation; not wall-clock time.
    pub sample_timestamp: u64,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn encode(&self, limit: usize) -> Result<Vec<u8>> {
        ensure!(
            self.payload.len() + HEADER <= limit && limit <= 1200,
            "oversize datagram"
        );
        let mut bytes = Vec::with_capacity(HEADER + self.payload.len());
        bytes.extend_from_slice(b"MXAD");
        bytes.extend_from_slice(&VERSION.to_be_bytes());
        bytes.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        for n in [
            self.stream,
            self.generation,
            self.sequence,
            self.sample_timestamp,
        ] {
            bytes.extend_from_slice(&n.to_be_bytes());
        }
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8], limit: usize) -> Result<Self> {
        ensure!(
            bytes.len() >= HEADER && bytes.len() <= limit && limit <= 1200,
            "invalid datagram length"
        );
        ensure!(
            &bytes[..4] == b"MXAD" && u16::from_be_bytes(bytes[4..6].try_into()?) == VERSION,
            "unknown audio protocol"
        );
        ensure!(
            usize::from(u16::from_be_bytes(bytes[6..8].try_into()?)) == bytes.len() - HEADER,
            "payload length mismatch"
        );
        let number = |start: usize| -> Result<u64> {
            Ok(u64::from_be_bytes(bytes[start..start + 8].try_into()?))
        };
        Ok(Self {
            stream: number(8)?,
            generation: number(16)?,
            sequence: number(24)?,
            sample_timestamp: number(32)?,
            payload: bytes[HEADER..].to_vec(),
        })
    }

    pub fn validate(&self, offer: &Offer) -> Result<()> {
        ensure!(
            self.stream == offer.spec.stream && self.generation == offer.generation,
            "unadmitted stream/generation"
        );
        ensure!(
            self.payload.len() == offer.spec.payload_bytes()?,
            "wrong PCM payload length"
        );
        ensure!(
            self.sequence
                .checked_mul(u64::from(offer.spec.frames_per_packet))
                == Some(self.sample_timestamp),
            "invalid sample timeline"
        );
        Ok(())
    }
}
