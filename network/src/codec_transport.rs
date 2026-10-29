//! Exact codec selection on the existing pinned-mTLS fragment session.
//! No device, management, mixer or additional media queue authority is created here.
use super::*;
use crate::{Codec, Playout, PlayoutConfig, playout::Lifecycle, worker::control::Cadence};
use mixyard_network_codecs as codec;
use serde::{Deserializer, Serializer, de};

/// Canonical decimal strings preserve all u64 bits in JavaScript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DecimalU64(u64);
impl DecimalU64 {
    pub fn new(value: u64) -> Result<Self> {
        ensure!(value != 0, "zero stream/generation");
        Ok(Self(value))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}
impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        if text.is_empty()
            || text.len() > 20
            || text.starts_with('0')
            || !text.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(de::Error::custom(
                "canonical nonzero decimal-u64 string required",
            ));
        }
        text.parse::<u64>().map(Self).map_err(de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Conversion {
    #[serde(rename = "normalized-f32-clamp-v1")]
    NormalizedF32ClampV1,
}
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "lowercase",
    deny_unknown_fields,
    from = "EncodingWire"
)]
pub enum Encoding {
    #[default]
    Raw,
    Zstd {
        level: u8,
    },
    Opus {
        bitrate: u32,
        complexity: u8,
        vbr: bool,
        conversion: Conversion,
    },
}
// Serde's internally tagged *unit* variant ignores extra content. An empty
// struct variant on the input side enforces the frozen raw object's exact shape.
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
enum EncodingWire {
    Raw {},
    Zstd {
        level: u8,
    },
    Opus {
        bitrate: u32,
        complexity: u8,
        vbr: bool,
        conversion: Conversion,
    },
}
impl From<EncodingWire> for Encoding {
    fn from(wire: EncodingWire) -> Self {
        match wire {
            EncodingWire::Raw {} => Self::Raw,
            EncodingWire::Zstd { level } => Self::Zstd { level },
            EncodingWire::Opus {
                bitrate,
                complexity,
                vbr,
                conversion,
            } => Self::Opus {
                bitrate,
                complexity,
                vbr,
                conversion,
            },
        }
    }
}
impl Encoding {
    fn library(self) -> codec::Encoding {
        match self {
            Self::Raw => codec::Encoding::Raw,
            Self::Zstd { level } => codec::Encoding::Zstd { level },
            Self::Opus {
                bitrate,
                complexity,
                vbr,
                conversion: Conversion::NormalizedF32ClampV1,
            } => codec::Encoding::Opus(codec::OpusParameters {
                bitrate,
                complexity,
                vbr,
                conversion: codec::Conversion::NormalizedF32Clamp,
            }),
        }
    }
}

/// Frozen UI request v1. References only the surrounding service's already
/// admitted canonical Spec; never accepts a replacement descriptor or authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RequestWire", into = "RequestWire")]
pub struct CodecRequest {
    stream_id: DecimalU64,
    generation: DecimalU64,
    encoding: Encoding,
    allow_lossy: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestWire {
    version: u8,
    stream_id: DecimalU64,
    generation: DecimalU64,
    encoding: Encoding,
    allow_lossy: bool,
}
impl From<CodecRequest> for RequestWire {
    fn from(v: CodecRequest) -> Self {
        Self {
            version: 1,
            stream_id: v.stream_id,
            generation: v.generation,
            encoding: v.encoding,
            allow_lossy: v.allow_lossy,
        }
    }
}
impl TryFrom<RequestWire> for CodecRequest {
    type Error = anyhow::Error;
    fn try_from(v: RequestWire) -> Result<Self> {
        ensure!(v.version == 1, "unsupported codec request version");
        Self::new(
            v.stream_id.get(),
            v.generation.get(),
            v.encoding,
            v.allow_lossy,
        )
    }
}
impl CodecRequest {
    pub fn stream_id(&self) -> u64 {
        self.stream_id.get()
    }
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }
    pub fn allow_lossy(&self) -> bool {
        self.allow_lossy
    }
    pub fn new(
        stream: u64,
        generation: u64,
        encoding: Encoding,
        allow_lossy: bool,
    ) -> Result<Self> {
        ensure!(
            allow_lossy == matches!(encoding, Encoding::Opus { .. }),
            "lossy consent must match explicit Opus selection"
        );
        match encoding {
            Encoding::Zstd { level } => ensure!((1..=3).contains(&level), "Zstd level budget"),
            Encoding::Opus {
                bitrate,
                complexity,
                ..
            } => ensure!(
                (6000..=512000).contains(&bitrate) && complexity <= 10,
                "Opus work parameters"
            ),
            Encoding::Raw => {}
        }
        Ok(Self {
            stream_id: DecimalU64::new(stream)?,
            generation: DecimalU64::new(generation)?,
            encoding,
            allow_lossy,
        })
    }
    fn validate(&self) -> Result<()> {
        Self::new(
            self.stream_id.get(),
            self.generation.get(),
            self.encoding,
            self.allow_lossy,
        )?;
        Ok(())
    }
}

/// Explicit directional exact-plan grants, provisioned by the service authority.
/// A canonical raw grant or management credential alone cannot select a codec.
#[derive(Clone, Debug, Default)]
pub struct CodecGrants {
    pub send: Vec<CodecRequest>,
    pub receive: Vec<CodecRequest>,
}
#[derive(Clone, Debug)]
pub struct CodecConfig {
    pub grants: CodecGrants,
    pub budget: codec::Budget,
    /// Per-call wall guard after bounded synchronous library work; not a preemptive deadline.
    pub max_work_wall: Duration,
}

fn format(spec: &Spec) -> Result<codec::Format> {
    let sample = match spec.codec {
        Codec::RawS16Le => codec::Sample::S16Le,
        Codec::RawS24Le => codec::Sample::S24Le,
        Codec::RawS32Le => codec::Sample::S32Le,
        Codec::RawF32Le => codec::Sample::F32Le,
        Codec::RawF64Le => codec::Sample::F64Le,
        Codec::Opus => anyhow::bail!("canonical source must be raw"),
    };
    Ok(codec::Format {
        rate: spec.sample_rate,
        sample,
        channels: spec.channels.clone(),
        frames: u32::from(spec.frames_per_packet),
    })
}
fn negotiated(
    spec: &Spec,
    request: &CodecRequest,
    config: &CodecConfig,
) -> Result<codec::Negotiated> {
    request.validate()?;
    ensure!(
        spec.stream == request.stream_id.get(),
        "request does not reference granted stream"
    );
    let plan = codec::Plan {
        format: format(spec)?,
        encoding: request.encoding.library(),
    };
    // The caller checks exact local authority; the reliable exchange verifies the
    // remote authority independently before either side commits its generation.
    Ok(codec::negotiate(
        &plan,
        std::slice::from_ref(&plan),
        std::slice::from_ref(&plan),
        request.allow_lossy,
        config.budget,
    )?)
}
pub(super) fn maximum(n: &codec::Negotiated) -> Result<usize> {
    let payload = match n.plan().encoding {
        codec::Encoding::Raw => n.block_bytes(),
        codec::Encoding::Opus(_) => codec::OPUS_MAX_PACKET_BYTES,
        codec::Encoding::Zstd { .. } => n.budget().max_encoded_bytes,
    };
    payload
        .checked_add(codec::HEADER_BYTES)
        .context("envelope maximum overflow")
}
fn geometry(offer: &Offer, n: &codec::Negotiated) -> Result<Descriptor> {
    Ok(Descriptor {
        block_bytes: maximum(n)?,
        ..descriptor(offer)?
    })
}
fn binding(session: &FragmentSession, request: &CodecRequest) -> Result<codec::Binding> {
    let mut connection = [0; 16];
    // This exporter is domain-separated from all other protocol uses. Stream and
    // generation are also explicit MXC1 fields; a reconnect derives a new nonce.
    session
        .base
        .connection
        .export_keying_material(
            &mut connection,
            b"EXPORTER-Mixyard-MXC1-v1",
            b"authenticated-codec-fragments-v3",
        )
        .map_err(|_| anyhow::anyhow!("TLS codec binding unavailable"))?;
    ensure!(connection != [0; 16], "zero codec binding");
    Ok(codec::Binding {
        connection,
        stream: request.stream_id.get(),
        generation: request.generation.get(),
    })
}

/// Same mTLS/pin factory and QUIC queues as raw framing, with separate codec grants.
pub fn codec_endpoint(
    bind: SocketAddr,
    identity: Identity,
    peer: PairedPeer,
    permissions: Permissions,
    limits: Limits,
    fragments: Config,
    codecs: CodecConfig,
) -> Result<FragmentEndpoint> {
    ensure!(
        !codecs.max_work_wall.is_zero() && codecs.max_work_wall <= limits.control_timeout,
        "codec work wall budget"
    );
    validate_grants(&permissions, &limits, &fragments)?;
    ensure!(
        (1..=codec::MAX_BLOCK_BYTES).contains(&codecs.budget.max_block_bytes)
            && (1..=codec::MAX_ENCODED_BYTES).contains(&codecs.budget.max_encoded_bytes),
        "codec budget outside supported profile"
    );
    for (requests, specs) in [
        (&codecs.grants.send, &permissions.send),
        (&codecs.grants.receive, &permissions.receive),
    ] {
        ensure!(
            requests.len() <= limits.max_streams.saturating_mul(codec::MAX_PLANS),
            "codec grant count budget"
        );
        let mut reservations = BTreeMap::<u64, Descriptor>::new();
        for (index, request) in requests.iter().enumerate() {
            ensure!(
                !requests[..index].contains(request),
                "duplicate exact codec grant"
            );
            let spec = specs
                .get(&request.stream_id.get())
                .context("codec without canonical audio grant")?;
            let n = negotiated(spec, request, &codecs)?;
            // Probe real FFI construction before reporting capability or reserving admission.
            let probe_binding = codec::Binding {
                connection: [1; 16],
                stream: spec.stream,
                generation: request.generation.get(),
            };
            let encoder = codec::Encoder::new(spec.clone(), n.clone(), probe_binding)?;
            let decoder = codec::Decoder::new(spec.clone(), n.clone(), probe_binding)?;
            ensure!(
                encoder
                    .context_bytes()
                    .checked_add(decoder.context_bytes())
                    .is_some(),
                "codec context budget overflow"
            );
            let offer = Offer {
                version: crate::VERSION,
                generation: request.generation.get(),
                spec: spec.clone(),
            };
            let d = geometry(&offer, &n)?;
            reservations
                .entry(spec.stream)
                .and_modify(|old| old.block_bytes = old.block_bytes.max(d.block_bytes))
                .or_insert(d);
        }
        let mut reassembly = Reassembler::new(fragments.clone())?;
        let mut rate = 0u128;
        for d in reservations.values() {
            reassembly.admit_variable(*d)?;
            let spec = &specs[&d.stream];
            let chunks = d
                .block_bytes
                .div_ceil(fragments.max_datagram - fragmentation::HEADER);
            rate = rate
                .checked_add(
                    ((d.block_bytes + chunks * fragmentation::HEADER) as u128
                        * u128::from(spec.sample_rate))
                    .div_ceil(u128::from(spec.frames_per_packet)),
                )
                .context("codec aggregate rate overflow")?;
        }
        ensure!(
            rate <= u128::from(limits.max_bytes_per_second),
            "codec maximum wire rate exceeds admission budget"
        );
    }
    let mut endpoint = fragment_endpoint(bind, identity, peer, permissions, limits, fragments)?;
    endpoint.codecs = Some(codecs);
    Ok(endpoint)
}

pub(super) struct SendingCodec {
    pub encoder: codec::Encoder<Spec>,
    pub cadence: Cadence,
    pub measurements: Measurements,
}
pub(super) struct ReceivingCodec {
    pub request: CodecRequest,
    pub decoder: codec::Decoder<Spec>,
    pub life: Arc<Lifecycle>,
    pub claimed: bool,
    // Stateful predictor progress is independent of how much buffered audio the
    // consumer has rendered. Raw/Zstd can fill any complete slot in the window.
    pub next_decode: u64,
    pub measurements: Measurements,
}

/// No CPU/one-way/hardware value is inferred from wall time or application RTT.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurements {
    pub blocks: u64,
    pub raw_bytes: u64,
    pub envelope_bytes: u64,
    pub fragment_bytes: u64,
    pub wall_nanos: u64,
    pub cpu_nanos: Option<u64>,
    pub cpu_unavailable_reason: Option<String>,
    pub lookahead_frames: u32,
    pub context_bytes: usize,
}
impl Measurements {
    fn record(
        &mut self,
        raw: usize,
        envelope: usize,
        datagram: usize,
        timing: (Duration, Option<Duration>),
        lookahead: u32,
        context: usize,
    ) {
        let (wall, cpu) = timing;
        self.blocks = self.blocks.saturating_add(1);
        self.raw_bytes = self.raw_bytes.saturating_add(raw as u64);
        self.envelope_bytes = self.envelope_bytes.saturating_add(envelope as u64);
        self.fragment_bytes = self.fragment_bytes.saturating_add(
            (envelope + envelope.div_ceil(datagram - fragmentation::HEADER) * fragmentation::HEADER)
                as u64,
        );
        self.wall_nanos = self
            .wall_nanos
            .saturating_add(u64::try_from(wall.as_nanos()).unwrap_or(u64::MAX));
        if self.cpu_unavailable_reason.is_none()
            && let Some(cpu) = cpu
        {
            self.cpu_nanos = Some(
                self.cpu_nanos
                    .unwrap_or(0)
                    .saturating_add(u64::try_from(cpu.as_nanos()).unwrap_or(u64::MAX)),
            );
        } else {
            self.cpu_nanos = None;
            self.cpu_unavailable_reason =
                Some("thread CPU clock unavailable on this target".into());
        }
        self.lookahead_frames = lookahead;
        self.context_bytes = context;
    }
}
// Linux thread CPU time measures only the synchronous library call. The portable
// wall clock remains separate. Other targets explicitly report unavailable CPU.
fn thread_cpu() -> Option<Duration> {
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    {
        use std::ffi::{c_int, c_long};
        #[repr(C)]
        struct Timespec {
            sec: c_long,
            nsec: c_long,
        }
        unsafe extern "C" {
            fn clock_gettime(clock: c_int, time: *mut Timespec) -> c_int;
        }
        let mut time = Timespec { sec: 0, nsec: 0 };
        // SAFETY: initialized, correctly laid-out output; Linux CLOCK_THREAD_CPUTIME_ID.
        if unsafe { clock_gettime(3, &mut time) } == 0
            && time.sec >= 0
            && (0..1_000_000_000).contains(&time.nsec)
        {
            return Some(Duration::new(time.sec as u64, time.nsec as u32));
        }
    }
    None
}

impl FragmentSession {
    fn codec_config(&self) -> Result<&CodecConfig> {
        self.codecs.as_ref().context("codec transport not enabled")
    }
    pub(super) fn incoming_codec(
        &self,
        offer: &Offer,
        request: &CodecRequest,
    ) -> Result<(ReceivingCodec, Descriptor)> {
        let config = self.codec_config()?;
        ensure!(
            config.grants.receive.contains(request),
            "exact receive codec/consent not granted"
        );
        ensure!(
            request.generation.get() == offer.generation,
            "codec generation mismatch"
        );
        let n = negotiated(&offer.spec, request, config)?;
        let d = geometry(offer, &n)?;
        let decoder = codec::Decoder::new(offer.spec.clone(), n, binding(self, request)?)?;
        Ok((
            ReceivingCodec {
                request: request.clone(),
                decoder,
                life: Arc::new(Lifecycle::new()),
                claimed: false,
                next_decode: 0,
                measurements: Measurements::default(),
            },
            d,
        ))
    }
    pub async fn negotiate_codec(
        self: &Arc<Self>,
        request: CodecRequest,
        anchor: Instant,
    ) -> Result<()> {
        self.connected()?;
        let config = self.codec_config()?;
        ensure!(
            config.grants.send.contains(&request),
            "exact send codec/consent not granted"
        );
        let spec = self
            .permissions
            .send
            .get(&request.stream_id.get())
            .context("no canonical send grant")?
            .clone();
        let n = negotiated(&spec, &request, config)?;
        let encoder = codec::Encoder::new(spec.clone(), n, binding(self, &request)?)?;
        let cadence = Cadence::new(anchor, spec.sample_rate, spec.frames_per_packet)?;
        let offer = Offer {
            version: crate::VERSION,
            generation: request.generation.get(),
            spec,
        };
        self.negotiate_inner(
            offer,
            Some((
                request,
                SendingCodec {
                    encoder,
                    cadence,
                    measurements: Measurements::default(),
                },
            )),
        )
        .await
    }

    /// A single borrowed block is paced on its original generation sample clock.
    /// A missed source slot retires the generation; it is never retimed or burst.
    /// Cancellation retires only this stream, even while blocked on QUIC capacity.
    pub async fn send_codec(
        self: &Arc<Self>,
        stream: u64,
        generation: u64,
        raw: &[u8],
    ) -> Result<()> {
        self.connected()?;
        let (mut codec, sequence) = {
            let mut tx = self.outgoing.lock().expect("send poisoned");
            ensure!(
                !tx.revoked.contains(&stream) && !tx.negotiating.contains(&stream),
                "send invalidated/negotiating"
            );
            let state = tx
                .streams
                .get_mut(&stream)
                .context("unadmitted codec sender")?;
            ensure!(
                state.active && !state.busy && state.offer.generation == generation,
                "inactive/busy/stale codec sender"
            );
            ensure!(
                raw.len() == state.offer.spec.payload_bytes()?,
                "exact canonical raw block length required"
            );
            let codec = state.codec.take().context("not a codec admission")?;
            state.busy = true;
            (codec, state.next)
        };
        let mut guard = SendingGuard {
            session: self.clone(),
            stream,
            generation,
            committed: false,
        };
        let deadline = codec.cadence.next_deadline()?;
        // Revoke/close wakes pacing as well as the actual enqueue future.
        let sleep = tokio::time::sleep_until(deadline.into());
        tokio::pin!(sleep);
        std::future::poll_fn(|cx| {
            let mut tx = self.outgoing.lock().expect("send poisoned");
            let Some(state) = tx.streams.get_mut(&stream) else {
                return Poll::Ready(Err(anyhow::anyhow!("codec sender removed")));
            };
            if self.closed.load(Ordering::Acquire)
                || !state.active
                || state.offer.generation != generation
            {
                return Poll::Ready(Err(anyhow::anyhow!("codec pacing invalidated")));
            }
            state.waiter = Some(cx.waker().clone());
            sleep.as_mut().poll(cx).map(Ok)
        })
        .await?;
        let tick = codec
            .cadence
            .due(Instant::now())?
            .context("codec slot not due")?;
        ensure!(
            tick.sequence == sequence && tick.skipped == 0,
            "missed source slot: explicit generation restart required"
        );
        let cpu = thread_cpu();
        let start = Instant::now();
        let packet = codec.encoder.encode(raw)?;
        let wall = start.elapsed();
        let cpu = cpu.and_then(|start| thread_cpu().and_then(|end| end.checked_sub(start)));
        ensure!(
            wall <= self.codec_config()?.max_work_wall,
            "codec work wall budget exceeded"
        );
        codec.measurements.record(
            raw.len(),
            packet.len(),
            self.config.max_datagram,
            (wall, cpu),
            codec.encoder.lookahead_frames(),
            codec.encoder.context_bytes(),
        );
        let offer = {
            let tx = self.outgoing.lock().expect("send poisoned");
            let state = tx.streams.get(&stream).context("codec sender removed")?;
            ensure!(
                state.active && state.offer.generation == generation,
                "codec encode invalidated"
            );
            state.offer.clone()
        };
        let fragmenter = Fragmenter::variable(
            self.config.clone(),
            geometry(&offer, codec.encoder.negotiated())?,
        )?;
        self.send_fragments(stream, generation, fragmenter.fragments(sequence, &packet)?)
            .await?;
        {
            let mut tx = self.outgoing.lock().expect("send poisoned");
            let state = tx
                .streams
                .get_mut(&stream)
                .context("codec sender removed")?;
            ensure!(
                state.active
                    && state.offer.generation == generation
                    && !self.closed.load(Ordering::Acquire),
                "codec send invalidated"
            );
            state.next = sequence
                .checked_add(1)
                .context("codec sequence exhausted")?;
            state.codec = Some(codec);
        }
        guard.committed = true;
        Ok(())
    }

    /// Reuse the existing raw Playout/lifecycle boundary, with no extra media FIFO.
    pub fn create_codec_playout(
        &self,
        stream: u64,
        generation: u64,
        config: PlayoutConfig,
    ) -> Result<Playout> {
        self.connected()?;
        let mut rx = self.receiver.lock().expect("receive poisoned");
        ensure!(!rx.revoked.contains(&stream), "receive revoked");
        let state = rx
            .codecs
            .get_mut(&stream)
            .context("unadmitted codec receiver")?;
        ensure!(
            state.decoder.binding().generation == generation
                && state.life.active()
                && !state.claimed,
            "inactive/stale/claimed codec receiver"
        );
        let offer = Offer {
            version: crate::VERSION,
            generation,
            spec: state.decoder.canonical().clone(),
        };
        let playout = Playout::new(offer, config, self.config.window, state.life.clone())?;
        state.claimed = true;
        Ok(playout)
    }
    /// Worker-side decode of complete ordered envelopes into the existing Playout.
    /// Opus loss retires this generation; raw/Zstd missing slots remain explicit silence.
    pub fn service_codec_playout(&self, playout: &mut Playout) -> Result<usize> {
        self.connected()?;
        let mut rx = self.receiver.lock().expect("receive poisoned");
        let id = playout.spec().stream;
        let generation = playout.generation();
        ensure!(!rx.revoked.contains(&id), "receive revoked");
        let ReceiveState {
            reassembler,
            codecs,
            ..
        } = &mut *rx;
        let state = codecs.get_mut(&id).context("unadmitted codec receiver")?;
        ensure!(
            state.claimed
                && Arc::ptr_eq(&state.life, &playout.lifecycle)
                && state.life.active()
                && state.decoder.binding().generation == generation,
            "foreign/stale codec playout"
        );
        let next = playout.next / u64::from(playout.spec().frames_per_packet);
        reassembler.advance(id, generation, next)?;
        let stateful = matches!(state.request.encoding, Encoding::Opus { .. });
        if stateful && state.next_decode < next {
            state.life.invalidate();
            state.decoder.revocation().revoke();
            anyhow::bail!("Opus source slot lost; fresh codec generation required");
        }
        let first = if stateful { state.next_decode } else { next };
        let mut inserted = 0;
        for sequence in first
            ..next
                .checked_add(self.config.window as u64)
                .context("codec window exhausted")?
        {
            let block = match reassembler.take(id, generation, sequence, Instant::now()) {
                Ok(Some(block)) => block,
                Ok(None) if stateful => break,
                // Missing, pending and already-transferred stateless slots do
                // not prevent later complete envelopes from prebuffering.
                Ok(None) => continue,
                // Expired raw/Zstd fragments are packet loss, not a failed
                // predictor. Keep the tombstone; render counts scheduled silence.
                Err(fragmentation::Error::Expired) if !stateful => continue,
                Err(e) => {
                    state.life.invalidate();
                    state.decoder.revocation().revoke();
                    return Err(e.into());
                }
            };
            let cpu = thread_cpu();
            let start = Instant::now();
            let result = state.decoder.decode(sequence, &block.payload);
            let wall = start.elapsed();
            let cpu = cpu.and_then(|start| thread_cpu().and_then(|end| end.checked_sub(start)));
            let mut raw = match result {
                Ok(raw) if wall <= self.codec_config()?.max_work_wall => raw,
                Ok(mut raw) => {
                    raw.fill(0);
                    state.life.invalidate();
                    state.decoder.revocation().revoke();
                    anyhow::bail!("codec decode work wall budget exceeded");
                }
                Err(error) => {
                    state.life.invalidate();
                    state.decoder.revocation().revoke();
                    return Err(error.into());
                }
            };
            state.measurements.record(
                raw.len(),
                block.payload.len(),
                self.config.max_datagram,
                (wall, cpu),
                state.decoder.lookahead_frames(),
                state.decoder.context_bytes(),
            );
            playout.insert(sequence, &raw);
            raw.fill(0);
            if stateful {
                state.next_decode = sequence
                    .checked_add(1)
                    .context("codec sequence exhausted")?;
            }
            inserted += 1;
        }
        Ok(inserted)
    }
    pub fn codec_measurements(
        &self,
        stream: u64,
        generation: u64,
        sending: bool,
    ) -> Result<Measurements> {
        if sending {
            let tx = self.outgoing.lock().expect("send poisoned");
            let state = tx.streams.get(&stream).context("unknown sender")?;
            ensure!(state.offer.generation == generation, "stale measurements");
            Ok(state
                .codec
                .as_ref()
                .context("codec worker busy/inactive")?
                .measurements
                .clone())
        } else {
            let rx = self.receiver.lock().expect("receive poisoned");
            let state = rx.codecs.get(&stream).context("unknown receiver")?;
            ensure!(
                state.decoder.binding().generation == generation,
                "stale measurements"
            );
            Ok(state.measurements.clone())
        }
    }
}

/// This codec-library work profile is independent of native endpoint capability.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecConstraints {
    pub max_source_channels: usize,
    pub zstd_levels: Vec<u8>,
    pub zstd_max_window_bytes: usize,
    pub zstd_dictionaries: bool,
    pub opus_rates: Vec<u32>,
    pub opus_mono_preserves_source_channel: bool,
    pub opus_stereo_ordered_channels: Vec<String>,
    pub opus_frame_micros: Vec<u32>,
    pub opus_min_bitrate: u32,
    pub opus_max_bitrate: u32,
    pub opus_max_complexity: u8,
    pub opus_float_input: String,
    pub automatic_resampling: bool,
    pub automatic_channel_mapping: bool,
}
impl Default for CodecConstraints {
    fn default() -> Self {
        Self {
            max_source_channels: 128,
            zstd_levels: vec![1, 2, 3],
            zstd_max_window_bytes: codec::MAX_BLOCK_BYTES,
            zstd_dictionaries: false,
            opus_rates: vec![8000, 12000, 16000, 24000, 48000],
            opus_mono_preserves_source_channel: true,
            opus_stereo_ordered_channels: vec!["FL".into(), "FR".into()],
            opus_frame_micros: vec![2500, 5000, 10000, 20000, 40000, 60000],
            opus_min_bitrate: 6000,
            opus_max_bitrate: 512000,
            opus_max_complexity: 10,
            opus_float_input:
                "finite normalized [-1,1]; S32/F64 conversion explicitly loses precision".into(),
            automatic_resampling: false,
            automatic_channel_mapping: false,
        }
    }
}

/// A per-direction/per-source/exact-request probe, scoped by the surrounding
/// device/authority session. This is codec availability, never native audio success.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecCapability {
    pub version: u8,
    pub request: CodecRequest,
    pub available: bool,
    pub agreement_state: String,
    pub availability_scope: String,
    pub unavailable_reason: Option<String>,
    pub constraints: CodecConstraints,
    pub zstd_version: Option<u32>,
    pub opus_version: Option<String>,
    pub max_raw_bytes: usize,
    pub max_encoded_bytes: usize,
    pub max_envelope_bytes: Option<usize>,
    pub max_fragments_per_block: Option<usize>,
    pub reserved_window_bytes: Option<usize>,
    pub max_aggregate_reassembly_bytes: usize,
    pub max_work_wall_nanos: u64,
    pub lookahead_frames: Option<u32>,
    pub context_bytes: Option<usize>,
    pub measurements: Option<Measurements>,
    pub measurement_unavailable_reason: Option<String>,
    pub network_delay_nanos: Option<u64>,
    pub physical_delay_nanos: Option<u64>,
    pub delay_unavailable_reason: String,
}
impl FragmentSession {
    pub fn codec_capability(&self, request: CodecRequest, sending: bool) -> CodecCapability {
        let config = self.codecs.as_ref();
        let mut report = CodecCapability {
            version: 1, request: request.clone(), available: false, unavailable_reason: None,
            agreement_state: "local-capability-only".into(),
            availability_scope: "local worker construction and exact grant on this authenticated session; no remote consumption or physical delivery proof".into(),
            constraints: CodecConstraints::default(),
            zstd_version: codec::zstd_version(), opus_version: codec::opus_version().map(str::to_owned),
            max_raw_bytes: config.map_or(0, |c| c.budget.max_block_bytes),
            max_encoded_bytes: config.map_or(0, |c| c.budget.max_encoded_bytes),
            max_envelope_bytes: None, max_fragments_per_block: None, reserved_window_bytes: None,
            max_aggregate_reassembly_bytes: self.config.max_buffered_bytes,
            max_work_wall_nanos: config.map_or(0, |c| u64::try_from(c.max_work_wall.as_nanos()).unwrap_or(u64::MAX)),
            lookahead_frames: None, context_bytes: None, measurements: None,
            measurement_unavailable_reason: Some("no completed codec operation for this exact generation".into()),
            network_delay_nanos: None, physical_delay_nanos: None,
            delay_unavailable_reason: "codec lookahead and local operation wall/CPU time do not measure network, device or physical latency".into(),
        };
        let probe = || -> Result<(codec::Negotiated, u32, usize)> {
            self.connected()?;
            let config = self.codec_config()?;
            let (grants, specs) = if sending {
                (&config.grants.send, &self.permissions.send)
            } else {
                (&config.grants.receive, &self.permissions.receive)
            };
            ensure!(grants.contains(&request), "exact codec/consent not granted");
            let id = request.stream_id.get();
            let revoked = if sending {
                self.outgoing
                    .lock()
                    .expect("send poisoned")
                    .revoked
                    .contains(&id)
            } else {
                self.receiver
                    .lock()
                    .expect("receive poisoned")
                    .revoked
                    .contains(&id)
            };
            ensure!(!revoked, "codec permission revoked");
            if sending {
                let tx = self.outgoing.lock().expect("send poisoned");
                if let Some(state) = tx.streams.get(&id) {
                    ensure!(
                        request.generation.get() >= state.offer.generation,
                        "stale codec generation"
                    );
                    if request.generation.get() == state.offer.generation {
                        ensure!(state.active, "codec generation retired");
                        if let Some(worker) = &state.codec {
                            ensure!(
                                worker.encoder.negotiated().plan().encoding
                                    == request.encoding.library(),
                                "different exact codec plan already admitted"
                            );
                        }
                    }
                }
            } else {
                let rx = self.receiver.lock().expect("receive poisoned");
                if let Some(state) = rx.codecs.get(&id) {
                    ensure!(
                        request.generation.get() >= state.request.generation.get(),
                        "stale codec generation"
                    );
                    if request.generation.get() == state.request.generation.get() {
                        ensure!(
                            state.life.active() && state.request == request,
                            "retired/different exact receive plan"
                        );
                    }
                }
            }
            let spec = specs.get(&id).context("canonical grant unavailable")?;
            let n = negotiated(spec, &request, config)?;
            if sending {
                let worker =
                    codec::Encoder::new(spec.clone(), n.clone(), binding(self, &request)?)?;
                Ok((n, worker.lookahead_frames(), worker.context_bytes()))
            } else {
                let worker =
                    codec::Decoder::new(spec.clone(), n.clone(), binding(self, &request)?)?;
                Ok((n, worker.lookahead_frames(), worker.context_bytes()))
            }
        };
        match probe() {
            Ok((n, lookahead, context)) => {
                let maximum = maximum(&n).expect("validated geometry");
                let chunks = maximum.div_ceil(self.config.max_datagram - fragmentation::HEADER);
                report.max_envelope_bytes = Some(maximum);
                report.max_fragments_per_block = Some(chunks);
                report.reserved_window_bytes = Some((maximum + chunks) * self.config.window);
                report.lookahead_frames = Some(lookahead);
                report.context_bytes = Some(context);
                report.available = true;
                if sending {
                    let tx = self.outgoing.lock().expect("send poisoned");
                    if let Some(state) = tx.streams.get(&request.stream_id.get())
                        && state.offer.generation == request.generation.get()
                        && state.active
                    {
                        report.agreement_state = if let Some(worker) = &state.codec {
                            if worker.encoder.negotiated().plan().encoding
                                == request.encoding.library()
                            {
                                "send-agreement-confirmed"
                            } else {
                                "different-plan-admitted"
                            }
                        } else if state.busy {
                            "send-operation-in-flight"
                        } else {
                            "local-capability-only"
                        }
                        .into();
                    }
                } else {
                    let rx = self.receiver.lock().expect("receive poisoned");
                    if let Some(state) = rx.codecs.get(&request.stream_id.get())
                        && state.request == request
                        && state.life.active()
                    {
                        report.agreement_state = "receive-admitted".into();
                    }
                }
                if ["send-agreement-confirmed", "receive-admitted"]
                    .contains(&report.agreement_state.as_str())
                    && let Ok(metrics) = self.codec_measurements(
                        request.stream_id.get(),
                        request.generation.get(),
                        sending,
                    )
                    && metrics.blocks > 0
                {
                    report.measurements = Some(metrics);
                    report.measurement_unavailable_reason = None;
                }
            }
            Err(error) => report.unavailable_reason = Some(error.to_string()),
        }
        report
    }
}

#[cfg(test)]
#[path = "../tests/private/codec_transport.rs"]
mod tests;

/// Annotation stays attached to the original sample-frame timeline. In particular,
/// a decoded F64/S32 container after Opus never claims source-bit preservation.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodecTimelineObservation {
    pub version: u8,
    pub request: CodecRequest,
    #[serde(serialize_with = "serialize_source_spec")]
    pub source_spec: Spec,
    pub next_source_sample_frame: String,
    pub bit_preserving: bool,
    pub destination_conversion: Option<Conversion>,
    pub codec_lookahead_frames: u32,
}
// Canonical wire Spec remains unchanged internally. This new UI observation also
// keeps its source ID exact in JavaScript, just like the frozen request IDs.
fn serialize_source_spec<S: Serializer>(
    spec: &Spec,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let mut value = serde_json::to_value(spec).map_err(serde::ser::Error::custom)?;
    value
        .as_object_mut()
        .expect("Spec is an object")
        .insert("stream".into(), spec.stream.to_string().into());
    value.serialize(serializer)
}
impl FragmentSession {
    pub fn codec_timeline_observation(
        &self,
        playout: &Playout,
    ) -> Result<CodecTimelineObservation> {
        self.connected()?;
        let rx = self.receiver.lock().expect("receive poisoned");
        let state = rx
            .codecs
            .get(&playout.spec().stream)
            .context("unknown codec receiver")?;
        ensure!(
            state.life.active()
                && Arc::ptr_eq(&state.life, &playout.lifecycle)
                && state.request.generation.get() == playout.generation(),
            "stale/foreign codec timeline"
        );
        let (bit_preserving, destination_conversion) = match state.request.encoding {
            Encoding::Opus { conversion, .. } => (false, Some(conversion)),
            _ => (true, None),
        };
        Ok(CodecTimelineObservation {
            version: 1,
            request: state.request.clone(),
            source_spec: state.decoder.canonical().clone(),
            next_source_sample_frame: playout.next.to_string(),
            bit_preserving,
            destination_conversion,
            codec_lookahead_frames: state.decoder.lookahead_frames(),
        })
    }
}
