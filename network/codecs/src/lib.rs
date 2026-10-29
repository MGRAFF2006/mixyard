//! Optional worker-side codecs. This crate grants no capture, receive or playback permission.
//! The caller retains and validates its canonical Spec/provenance in `T` before admission.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[cfg(feature = "system-opus")]
mod opus;
#[cfg(feature = "system-zstd")]
mod zstd;

pub const HEADER_BYTES: usize = 56;
pub const MAX_BLOCK_BYTES: usize = 262_144;
pub const MAX_ENCODED_BYTES: usize = 270_336;
pub const MAX_PLANS: usize = 16;
pub const OPUS_MAX_PACKET_BYTES: usize = 4000;
pub type Result<T> = std::result::Result<T, Error>;

/// Actual linked Zstd runtime version in upstream numeric form, e.g. 10507.
pub fn zstd_version() -> Option<u32> {
    #[cfg(feature = "system-zstd")]
    {
        Some(zstd::version())
    }
    #[cfg(not(feature = "system-zstd"))]
    {
        None
    }
}
/// Actual linked Opus runtime version string, e.g. "libopus 1.5.2".
pub fn opus_version() -> Option<&'static str> {
    #[cfg(feature = "system-opus")]
    {
        opus::version()
    }
    #[cfg(not(feature = "system-opus"))]
    {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Invalid(&'static str),
    Unavailable(&'static str),
    Library(&'static str),
    Inactive,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
fn require(ok: bool, reason: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(reason))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    S16Le,
    S24Le,
    S32Le,
    F32Le,
    F64Le,
}
impl Sample {
    pub fn bytes(self) -> usize {
        match self {
            Self::S16Le => 2,
            Self::S24Le => 3,
            Self::S32Le | Self::F32Le => 4,
            Self::F64Le => 8,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub sample: Sample,
    pub channels: Vec<String>,
    pub frames: u32,
}
impl Format {
    pub fn block_bytes(&self) -> Result<usize> {
        require(self.rate > 0 && self.frames > 0, "empty rate/frames")?;
        require(
            !self.channels.is_empty() && self.channels.len() <= 128,
            "channel metadata budget",
        )?;
        for (i, channel) in self.channels.iter().enumerate() {
            require(
                !channel.is_empty()
                    && channel.len() <= 128
                    && !channel.chars().any(char::is_control),
                "channel identity",
            )?;
            require(!self.channels[..i].contains(channel), "duplicate channel")?;
        }
        let bytes = (self.frames as usize)
            .checked_mul(self.channels.len())
            .and_then(|n| n.checked_mul(self.sample.bytes()))
            .ok_or(Error::Invalid("block overflow"))?;
        require(bytes <= MAX_BLOCK_BYTES, "block work budget")?;
        Ok(bytes)
    }
}

/// Destination-local conversion: integers / 2^(bits-1), floats require finite [-1,1].
/// Decode clamps [-1,1]; integers round to nearest, ties away from zero, saturating.
/// F64/S32 conversion to the Opus f32 API explicitly loses precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conversion {
    NormalizedF32Clamp,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpusParameters {
    pub bitrate: u32,
    pub complexity: u8,
    pub vbr: bool,
    pub conversion: Conversion,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Raw,
    Zstd { level: u8 },
    Opus(OpusParameters),
}
impl Encoding {
    fn tag(self) -> u8 {
        match self {
            Self::Raw => 0,
            Self::Zstd { .. } => 1,
            Self::Opus(_) => 2,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub format: Format,
    pub encoding: Encoding,
}

/// Local admission budget. Both endpoints independently validate the exact plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub max_block_bytes: usize,
    pub max_encoded_bytes: usize,
}
impl Budget {
    fn validate(self) -> Result<()> {
        require(
            (1..=MAX_BLOCK_BYTES).contains(&self.max_block_bytes)
                && (1..=MAX_ENCODED_BYTES).contains(&self.max_encoded_bytes),
            "invalid budget",
        )
    }
}

#[derive(Debug, Clone)]
pub struct Negotiated {
    plan: Plan,
    budget: Budget,
    block_bytes: usize,
}
impl Negotiated {
    pub fn plan(&self) -> &Plan {
        &self.plan
    }
    pub fn budget(&self) -> Budget {
        self.budget
    }
    pub fn block_bytes(&self) -> usize {
        self.block_bytes
    }
}

/// Select only the requested exact plan present at both ends, never a fallback.
/// Lists must already have been parsed with bounded control-message parsers.
pub fn negotiate(
    request: &Plan,
    sender: &[Plan],
    receiver: &[Plan],
    allow_lossy: bool,
    budget: Budget,
) -> Result<Negotiated> {
    budget.validate()?;
    require(
        sender.len() <= MAX_PLANS && receiver.len() <= MAX_PLANS,
        "capability list budget",
    )?;
    let bytes = request.format.block_bytes()?;
    require(bytes <= budget.max_block_bytes, "decoded budget")?;
    require(
        sender.contains(request) && receiver.contains(request),
        "no exact parameter agreement",
    )?;
    match request.encoding {
        Encoding::Raw => require(bytes <= budget.max_encoded_bytes, "raw encoded budget")?,
        Encoding::Zstd { level } => {
            require((1..=3).contains(&level), "zstd compression work level")?;
            if !cfg!(feature = "system-zstd") {
                return Err(Error::Unavailable("system-zstd"));
            }
        }
        Encoding::Opus(p) => {
            require(allow_lossy, "explicit lossy consent required")?;
            if !cfg!(feature = "system-opus") {
                return Err(Error::Unavailable("system-opus"));
            }
            let f = &request.format;
            require(
                [8000, 12000, 16000, 24000, 48000].contains(&f.rate),
                "Opus rate requires explicit external resampling",
            )?;
            require(
                f.channels.len() == 1 || f.channels == ["FL", "FR"],
                "Opus layout requires explicit external channel mapping",
            )?;
            // Supported standard durations: 2.5, 5, 10, 20, 40, 60 ms.
            require(
                [1u64, 2, 4, 8, 16, 24]
                    .iter()
                    .any(|n| u64::from(f.frames) * 400 == u64::from(f.rate) * n),
                "Opus frame duration",
            )?;
            require(
                (6000..=512000).contains(&p.bitrate) && p.complexity <= 10,
                "Opus bitrate/complexity budget",
            )?;
            require(
                budget.max_encoded_bytes >= OPUS_MAX_PACKET_BYTES,
                "Opus profile requires 4000-byte packet capacity",
            )?;
        }
    }
    Ok(Negotiated {
        plan: request.clone(),
        budget,
        block_bytes: bytes,
    })
}

/// Unique authenticated connection nonce, nonzero stream and increasing generation.
/// The parent session owns uniqueness, admission permissions and generation monotonicity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub connection: [u8; 16],
    pub stream: u64,
    pub generation: u64,
}
impl Binding {
    fn validate(self) -> Result<()> {
        require(
            self.connection != [0; 16] && self.stream != 0 && self.generation != 0,
            "invalid admission binding",
        )
    }
}

#[derive(Clone)]
pub struct Revocation(Arc<AtomicBool>);
impl Revocation {
    pub fn revoke(&self) {
        self.0.store(false, Ordering::Release);
    }
    pub fn active(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<()> {
        if self.active() {
            Ok(())
        } else {
            Err(Error::Inactive)
        }
    }
}

/// Worker telemetry: local wall duration includes scheduling, not thread CPU time.
#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub blocks: u64,
    pub raw_bytes: u64,
    pub envelope_bytes: u64,
    pub elapsed: Duration,
    pub max_elapsed: Duration,
}
impl Metrics {
    fn record(&mut self, raw: usize, wire: usize, elapsed: Duration) {
        self.blocks = self.blocks.saturating_add(1);
        self.raw_bytes = self.raw_bytes.saturating_add(raw as u64);
        self.envelope_bytes = self.envelope_bytes.saturating_add(wire as u64);
        self.elapsed = self.elapsed.saturating_add(elapsed);
        self.max_elapsed = self.max_elapsed.max(elapsed);
    }
}

enum Engine {
    Raw,
    #[cfg(feature = "system-zstd")]
    Zstd(zstd::Engine),
    #[cfg(feature = "system-opus")]
    Opus(opus::Engine),
}
impl Engine {
    fn new(n: &Negotiated) -> Result<Self> {
        match n.plan.encoding {
            Encoding::Raw => Ok(Self::Raw),
            #[cfg(feature = "system-zstd")]
            Encoding::Zstd { level } => Ok(Self::Zstd(zstd::Engine::new(level)?)),
            #[cfg(feature = "system-opus")]
            Encoding::Opus(p) => Ok(Self::Opus(opus::Engine::new(&n.plan.format, p)?)),
            #[allow(unreachable_patterns)]
            _ => Err(Error::Unavailable("compiled codec feature")),
        }
    }
    fn encode(&mut self, n: &Negotiated, input: &[u8]) -> Result<Vec<u8>> {
        let _ = n;
        match self {
            Self::Raw => Ok(input.to_vec()),
            #[cfg(feature = "system-zstd")]
            Self::Zstd(e) => e.encode(input, n.budget.max_encoded_bytes),
            #[cfg(feature = "system-opus")]
            Self::Opus(e) => e.encode(&n.plan.format, input),
        }
    }
    fn decode(&mut self, n: &Negotiated, input: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::Raw => {
                require(input.len() == n.block_bytes, "raw block length")?;
                Ok(input.to_vec())
            }
            #[cfg(feature = "system-zstd")]
            Self::Zstd(e) => e.decode(input, n.block_bytes),
            #[cfg(feature = "system-opus")]
            Self::Opus(e) => e.decode(&n.plan.format, input),
        }
    }
    fn context_bytes(&self) -> usize {
        match self {
            Self::Raw => 0,
            #[cfg(feature = "system-zstd")]
            Self::Zstd(e) => e.context_bytes(),
            #[cfg(feature = "system-opus")]
            Self::Opus(e) => e.context_bytes(),
        }
    }
    fn lookahead_frames(&self) -> u32 {
        match self {
            #[cfg(feature = "system-opus")]
            Self::Opus(e) => e.lookahead_frames(),
            _ => 0,
        }
    }
}

pub struct Encoder<T> {
    worker: Worker<T>,
    next: u64,
}
pub struct Decoder<T> {
    worker: Worker<T>,
    next_opus: u64,
}
struct Worker<T> {
    canonical: T,
    negotiated: Negotiated,
    binding: Binding,
    life: Revocation,
    engine: Engine,
    metrics: Metrics,
}
impl<T> Worker<T> {
    fn new(canonical: T, negotiated: Negotiated, binding: Binding) -> Result<Self> {
        binding.validate()?;
        let engine = Engine::new(&negotiated)?;
        Ok(Self {
            canonical,
            negotiated,
            binding,
            life: Revocation(Arc::new(AtomicBool::new(true))),
            engine,
            metrics: Metrics::default(),
        })
    }
    fn finish(&mut self, result: Result<Vec<u8>>, start: Instant, wire: usize) -> Result<Vec<u8>> {
        let mut output = result?;
        if !self.life.active() {
            output.fill(0);
            return Err(Error::Inactive);
        }
        self.metrics
            .record(self.negotiated.block_bytes, wire, start.elapsed());
        Ok(output)
    }
}
macro_rules! observations {
    () => {
        pub fn canonical(&self) -> &T {
            &self.worker.canonical
        }
        pub fn negotiated(&self) -> &Negotiated {
            &self.worker.negotiated
        }
        pub fn binding(&self) -> Binding {
            self.worker.binding
        }
        pub fn revocation(&self) -> Revocation {
            self.worker.life.clone()
        }
        pub fn metrics(&self) -> &Metrics {
            &self.worker.metrics
        }
        /// Library context storage at observation time, excluding caller/envelope/conversion buffers.
        pub fn context_bytes(&self) -> usize {
            self.worker.engine.context_bytes()
        }
        /// Codec-reported algorithmic lookahead; not measured network or physical latency.
        pub fn lookahead_frames(&self) -> u32 {
            self.worker.engine.lookahead_frames()
        }
    };
}
impl<T> Encoder<T> {
    pub fn new(canonical: T, negotiated: Negotiated, binding: Binding) -> Result<Self> {
        Ok(Self {
            worker: Worker::new(canonical, negotiated, binding)?,
            next: 0,
        })
    }
    observations!();
    pub fn encode(&mut self, raw: &[u8]) -> Result<Vec<u8>> {
        let start = Instant::now();
        let w = &mut self.worker;
        w.life.check()?;
        require(
            raw.len() == w.negotiated.block_bytes,
            "exact admitted raw block required",
        )?;
        let next = self
            .next
            .checked_add(1)
            .ok_or(Error::Invalid("sequence exhausted"))?;
        let payload = match w.engine.encode(&w.negotiated, raw) {
            Ok(payload) => payload,
            Err(error) => {
                if matches!(w.negotiated.plan.encoding, Encoding::Opus(_)) {
                    w.life.revoke();
                }
                return Err(error);
            }
        };
        require(
            !payload.is_empty() && payload.len() <= w.negotiated.budget.max_encoded_bytes,
            "encoded budget",
        )?;
        let mut packet = Vec::with_capacity(HEADER_BYTES + payload.len());
        packet.extend_from_slice(b"MXC1");
        packet.extend_from_slice(&[w.negotiated.plan.encoding.tag(), 0, 0, 0]);
        packet.extend_from_slice(&w.binding.connection);
        packet.extend_from_slice(&w.binding.stream.to_be_bytes());
        packet.extend_from_slice(&w.binding.generation.to_be_bytes());
        packet.extend_from_slice(&self.next.to_be_bytes());
        packet.extend_from_slice(&(raw.len() as u32).to_be_bytes());
        packet.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        packet.extend_from_slice(&payload);
        self.next = next;
        let wire = packet.len();
        w.finish(Ok(packet), start, wire)
    }
}
impl<T> Decoder<T> {
    pub fn new(canonical: T, negotiated: Negotiated, binding: Binding) -> Result<Self> {
        Ok(Self {
            worker: Worker::new(canonical, negotiated, binding)?,
            next_opus: 0,
        })
    }
    observations!();
    /// The transport supplies its admitted sequence after bounded replay/reorder admission.
    /// Opus additionally requires contiguous order; loss recovery needs explicit new generation.
    pub fn decode(&mut self, expected_sequence: u64, packet: &[u8]) -> Result<Vec<u8>> {
        let start = Instant::now();
        let w = &mut self.worker;
        w.life.check()?;
        require(
            packet.len() >= HEADER_BYTES
                && packet.len() <= HEADER_BYTES + w.negotiated.budget.max_encoded_bytes,
            "envelope input budget",
        )?;
        require(
            &packet[..4] == b"MXC1"
                && packet[4] == w.negotiated.plan.encoding.tag()
                && packet[5..8] == [0; 3],
            "envelope version/encoding/reserved",
        )?;
        require(
            packet[8..24] == w.binding.connection
                && packet[24..32] == w.binding.stream.to_be_bytes()
                && packet[32..40] == w.binding.generation.to_be_bytes(),
            "stale or foreign admission",
        )?;
        require(
            packet[40..48] == expected_sequence.to_be_bytes(),
            "sequence mismatch",
        )?;
        require(
            packet[48..52] == (w.negotiated.block_bytes as u32).to_be_bytes(),
            "decoded size mismatch",
        )?;
        let encoded = u32::from_be_bytes(packet[52..56].try_into().unwrap()) as usize;
        require(
            encoded != 0 && encoded == packet.len() - HEADER_BYTES,
            "encoded size mismatch/trailing bytes",
        )?;
        let is_opus = matches!(w.negotiated.plan.encoding, Encoding::Opus(_));
        if is_opus {
            require(
                expected_sequence == self.next_opus,
                "Opus requires contiguous decode or explicit restart",
            )?;
        }
        let result = w.engine.decode(&w.negotiated, &packet[HEADER_BYTES..]);
        if is_opus {
            // A decoder error may mutate predictor state. Never continue that generation.
            if result.is_err() {
                w.life.revoke();
            }
            self.next_opus = expected_sequence
                .checked_add(1)
                .ok_or(Error::Invalid("sequence exhausted"))?;
        }
        w.finish(result, start, packet.len())
    }
}

#[cfg(test)]
mod tests;
