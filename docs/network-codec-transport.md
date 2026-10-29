# Authenticated exact codec delivery

The optional `network` feature `codecs` connects the existing callable codec
workers to the pinned mTLS QUIC `FragmentSession`, its directional canonical
raw grants, bounded reassembly and revocation locks. It is an executable adapter
for caller-owned PCM and the existing `Playout`, without another trust store,
mixer, device worker or media FIFO. Default builds and the original raw v1/v2
paths keep their existing behavior. `Encoding::default()` is raw; no negotiation,
congestion, decode failure or missing library silently selects another mode.

This does not wire the adapter into a native runtime or controller service.
`audioApplied` and native `backendAvailable` remain false. Production pairing,
remote service ownership and actual device qualification remain separate work.
The [codec library](network-codecs.md) owns the compression/conversion profile;
[raw fragmentation](network-fragment-transport.md) owns the shared transport.

## Frozen configuration request version 1

```json
{"version":1,"streamId":"18446744073709551615","generation":"9007199254740993","encoding":{"mode":"raw"},"allowLossy":false}
```

The three exact `encoding` shapes are:

```json
{"mode":"raw"}
{"mode":"zstd","level":2}
{"mode":"opus","bitrate":128000,"complexity":5,"vbr":false,"conversion":"normalized-f32-clamp-v1"}
```

`CodecRequest` serializes that exact camelCase contract. IDs are nonzero canonical
ASCII decimal strings within u64, without whitespace, signs, leading zeroes or
numeric JSON alternatives. Every field is required. Unknown/duplicate fields,
unsupported versions/conversions, invalid level/bitrate/complexity and ambiguous
consent fail parsing. `allowLossy` must be true for Opus and false for raw/Zstd.
An empty-struct input variant enforces the raw object's exact shape: Serde's
internally tagged unit variant alone otherwise accepts ignored extra content.

The request references the server's **already admitted exact raw Spec**. It has
no descriptor, peer identity, device selector, authority token or new grant.
The surrounding service must bind it to its authenticated device, controller
rights, session, canonical grant and stream generation; switching devices or
reconnecting must not retarget a pending request. A management credential does
not become an audio or codec grant. Keep request IDs as strings in every client.

`CodecGrants` separately provisions exact send and receive requests, including
every encoding parameter, generation and lossy consent. The `Permissions` raw
Spec grant is additionally mandatory. Both endpoints independently require their
local exact request and original canonical Spec/provenance before admission.
The adapter derives `Format` directly from that Spec, including precision, exact
rate, frame geometry and ordered channel IDs. It never accepts an independently
supplied Format or relabels encoded bytes as raw PCM. An excluded premix remains
rejected by the existing source-group/mix-minus/provenance policy.

## Admission, binding and variable framing

`codec_endpoint(..., permissions, limits, fragmentation, codec_config)` reuses
`fragment_endpoint` and the same authenticated `Session`/QUIC queues. Feature
forwarding is `codecs` (raw envelopes, no FFI), `system-zstd` and `system-opus`.
The latter two enable only the corresponding existing codec-library feature.
Neither raw grants nor an ordinary raw offer can select compressed framing.
Each explicit configured request is checked with real encoder and decoder
construction before endpoint creation; admission constructs the actual worker
again before committing the generation. Missing libraries/features or unsupported
source/plan parameters fail explicitly, with no raw or lossy fallback.

Reliable offers contain the original Spec, generation, fragment size, exact
maximum envelope size and version-1 codec request. Both endpoints must agree
the maximum geometry, including their Zstd encoded capacity; unequal local
maxima fail explicitly before admission. Existing bounded control parsing limits the whole frame
to 8192 bytes and rejects trailing data. Successful agreement owns the new
generation; replacement reserves the sender before its reliable exchange so an
old send cannot enter during replacement. An explicit negative reliable reply
made no admission mutation and preserves the old generation and siblings. An
uncertain canceled/failed negotiation still closes the session, preserving the
inherited v2 safety rule rather than guessing which peer committed.

The MXC1 binding uses the actual QUIC TLS exporter, label
`EXPORTER-Mixyard-MXC1-v1`, context `authenticated-codec-fragments-v3`, and a
16-byte nonzero connection nonce. MXC1 separately checks stream and generation.
New TLS connections derive different nonces, so an envelope replayed inside an
authenticated new connection still fails. QUIC supplies authentication; the
exported identifier is not a replacement MAC or another credential.

Codec envelopes use **MXAF version 3** with the same canonical 48-byte fragment
header layout, source sequence and `sequence * original frames_per_block`
timestamp. Version 2 still requires an exact raw length. Version 3 separately
admits a **maximum envelope size**: exact raw bytes + 56 for raw; 4000 + 56 for
Opus; configured encoded capacity + 56 for Zstd. Each block declares its actual
length within that maximum. All fragments for a slot must agree on that length,
use canonical aligned offsets/full non-tail chunks, and match the original
source-frame timestamp. A changed length poisons only that slot. Unsupported
versions, sizes, offsets and unadmitted stream/generation fail before allocation.
Variable encoded data cannot pass an exact-raw v2 admission.

The reassembler reserves `(maximum envelope bytes + maximum fragment count) *
window` per stream **before the first fragment**, then enforces the aggregate
reservation across streams. Fragment presence, consumed/error tombstones,
reorder window, replay, deadline and maximum stream count reuse the existing
bounded authority. Smaller first blocks cannot under-reserve later blocks.
Both directions also admit worst-case envelope bytes **plus actual fragment
headers** per source sample cadence against `Limits.max_bytes_per_second`.
Queues never expand according to an untrusted encoded/decompressed size.

## Pacing, decode and actual delivery commits

```rust,ignore
use mixyard_network::codec_transport::{CodecRequest, Encoding};

let request = CodecRequest::new(stream, generation, Encoding::Raw, false)?;
session.negotiate_codec(request, source_anchor).await?;
let mut output = peer.create_codec_playout(stream, generation, local_playout_config)?;
session.send_codec(stream, generation, &exact_raw_source_block).await?;
peer.service_codec_playout(&mut output)?;
output.render_into(local_scheduled_block_start, &mut destination_storage)?;
```

Keep each `FragmentSession::run` alive separately. Send futures pace independently
with the existing rational `Cadence`, anchored to the source sample clock;
no accumulated relative sleeps, timestamp reset, catch-up burst or hardware clock
inference is used. One borrowed raw block and one bounded envelope can be in
flight per stream. Caller storage is outside the transport's queue budget.
A missed source slot retires that generation and requires an explicit restart;
the adapter does not pretend that a later block belongs to an earlier sample time.
Use a source anchor in the future to allow reliable negotiation to finish.

Encoding yields no transport authority. Before each actual QUIC enqueue poll,
the shared sender checks activity/revocation/generation **under the same lock as
the enqueue**. The lock is released on Pending. Revocation wakes a pacing or
capacity-blocked future; canceled/failed sends retire only their generation.
There is no enqueue after a completed local revoke, even if the driver later
frees capacity. Already committed transport data cannot be recalled. Explicit
restart needs a strictly greater granted generation. Independent send/decode
failures and explicit admission rejection preserve siblings; session close or
an uncertain session-wide negotiation invalidates all its streams.

Receive workers decode only complete bounded envelopes into the **existing**
single-owner raw `Playout`. The adapter permits one handle per admitted generation
and checks its exact lifecycle identity; raw `take` cannot expose encoded or
unfenced decoded buffers. Restart, revoke, close and worker exit invalidate the
same lifecycle that fences `render_into`. Codec errors invalidate only the
affected generation; its old buffered samples then render silence. Reordered
fragments/blocks wait for source-order consumption. Missing raw/Zstd blocks are
explicit local-schedule silence; Opus requires contiguous predictor order, so a
consumer skip/loss requires an explicit new generation. There is no hidden PLC.
The receive worker scans the bounded raw/Zstd window past missing, consumed or
expired fragment slots so later complete blocks can prebuffer on their original
sample positions. Lossless expiry retains replay tombstones and does not revoke
the generation. Playout counts missing silence and schedule skips. Opus has a
separate decode cursor: already-buffered blocks do not stall later arrivals, but
an undecoded source slot passed by the renderer still requires a new generation.

`codec_timeline_observation(&playout)` carries the original Spec/provenance,
exact request, original sample-frame cursor as a decimal string, explicit
conversion, bit-preserving flag and codec lookahead. It never shifts timestamps
to hide lookahead. Decoded S32/F64 containers after Opus are explicitly lossy;
raw/Zstd sibling destinations retain original bits and formats. Its serialized
`sourceSpec.stream` is also a decimal string; the internal transport Spec is
unchanged.

## Work profile and honest capabilities

The existing codec profile bounds raw blocks at 262144 bytes, encoded payloads at
270336 bytes and source metadata at 128 ordered channels. These are this optional
worker profile's resource limits, not global Mixyard or raw-v2 limits. Local
`CodecConfig.budget` can admit smaller sizes. Fragmentation's configured maximum
must also accommodate the complete envelope and its aggregate reservation.

Zstd levels are 1–3, window at most 262144 bytes, single synchronous worker,
known exact output size and checksum required, no dictionaries, concatenated,
unknown-size or skippable frames. Decoding allocates only the already admitted
raw size. This opaque path preserves S16/S24/S32/F32/F64 bits including IEEE
specials. Input, window and level bound library work/context growth; observed
library context bytes exclude caller/conversion/envelope storage. Configured
reassembly and raw Playout each reserve their own bounded windows; they are
separate from bounded library contexts and the existing QUIC queue budget.

Opus admits only 8/12/16/24/48 kHz, one original channel or ordered FL/FR stereo,
2.5/5/10/20/40/60 ms exact blocks, bitrate 6000–512000 and complexity 0–10, with
a 4000-byte encoded capacity. `normalized-f32-clamp-v1` explicitly converts
integers by 2^(bits-1), requires finite float input in [-1,1], and uses the Opus
f32 API; decoding clamps to [-1,1] and integer conversion rounds/saturates.
S32/F64 precision is explicitly reduced on this destination only. Unsupported
rates/layouts require a separately requested upstream conversion; no resampler,
channel map, global 48 kHz or stereo cap is introduced.

`max_work_wall` rejects a completed encode/decode that exceeded its local wall
budget and retires that generation before delivery. It is a post-call guard,
not preemption of a native library. Finite block/codec/window/parameter bounds
limit work inside the call. Worker telemetry measures local operation wall time,
actual raw/envelope/fragment bytes, reported lookahead and context storage.
On 64-bit Linux a separate `CLOCK_THREAD_CPUTIME_ID` bracket measures actual thread CPU;
other targets return null with a reason. Instrumentation has its own overhead
and clock resolution; wall values are never called CPU. A failed CPU observation
keeps cumulative CPU null rather than presenting a partial total as complete.

`codec_capability(request, sending)` serializes `CodecCapability` version 1:
exact request, actual local worker construction availability/reason, runtime
library versions, source/parameter constraints, configured budgets/maximum
fragments/reservation, lookahead, context bytes and optional measured counters.
`availabilityScope` explicitly limits availability to local construction and
exact grant on this authenticated session. `agreementState` separately reports
`local-capability-only`, `send-agreement-confirmed`, `receive-admitted` or an
in-flight operation. It is not remote consumption or physical delivery proof.
Measurements are null with a reason until a completed operation for the exact
admitted plan/generation exists. Network and physical delay remain null with
an explanation: codec lookahead, operation wall/CPU, queue duration and RTT are
different observations. No RTT/2, hardware resampler, native availability or
`audioApplied` success is claimed.

## Verification and measured scope

The production tests use paired pinned-mTLS sockets, the real compression
libraries and the existing Playout; test-only private access injects malformed
traffic through actual authenticated QUIC rather than replacing the transport.
Coverage includes exact mixed-direction raw/Zstd/explicit Opus; independent
integer/IEEE byte oracles; lossy sine/conversion quality; strict DTO parsing;
foreign Spec/provenance/plan/generation; dictionary/content-size bombs; variable
length switches; reorder/loss; new-TLS replay; real capacity-blocked revocation;
pacing/partial-send cancellation; replacement reservation; failed generations,
missed source slots and post-call work limits, with siblings preserved.

Local sample evidence (Linux, linked Zstd 1.5.7 / libopus 1.5.2, one loopback
fixture, unoptimized production sources; values are not capacity guarantees):

| Stream | Raw bytes | MXC1 envelope bytes | Fragment bytes including MXAF headers |
| --- | ---: | ---: | ---: |
| S32 96 kHz, 3 ordered channels, raw | 23040 | 23096 | 24296 |
| S32 96 kHz, 3 ordered channels, Zstd level 3, independent destination | 23040 | 90 | 138 |
| F64 44117 Hz, 2 ordered channels, Zstd level 2, repeated IEEE patterns | 16000 | 104 | 152 |
| F32 48 kHz FL/FR, Opus 128000 CBR complexity 5 | 7680 | 376 | 424 |

One observed Zstd encode was 220051 ns wall / 225012 ns thread CPU; Opus encode
487188 ns wall / 490569 ns thread CPU. CPU and wall brackets include different
clock-read overhead, so sub-millisecond values must not be subtracted or treated
as precise throughput benchmarks. Numbers exclude QUIC/TLS/IP overhead. The S32
mono 440 Hz sine fixture measured 40.25 dB SNR after accounting for the library's
312-frame lookahead at 48 kHz. That is algorithmic delay/fixture quality evidence,
not physical, one-way network, hardware-clock or general listening quality proof.

On a machine with build/storage authorization, use the normal locked checks:

```sh
cargo fmt --manifest-path network/Cargo.toml -- --check
cargo clippy --locked --manifest-path network/Cargo.toml --features codecs --all-targets -- -D warnings
cargo test --locked --manifest-path network/Cargo.toml --features codecs
cargo clippy --locked --manifest-path network/Cargo.toml --features system-zstd,system-opus --all-targets -- -D warnings
cargo test --locked --manifest-path network/Cargo.toml --features system-zstd,system-opus
```

For a source-only host with a preexisting exact-lock compatible dependency cache,
`network/verify-codec-source.py --cache CACHE_ROOT --target PRIVATE_TMP_DIR`
compiles these actual sources with rustc against read-only cached rlibs, reserves
2 GiB root / 10 GiB tmp+shm, and caps retained private output at 50 MiB. Add
`--features system-zstd,system-opus`, `--lib-test-filter fragment_transport::codecs`,
`--test codec_quic` and/or `--clippy` as needed. It performs no Cargo build or
dependency download. This supplements hosted locked Cargo qualification. Linked
platform/package parity is an independent prerequisite; local Linux success does
not establish Windows/macOS/mobile native availability or physical audio.
