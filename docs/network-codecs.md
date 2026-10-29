# Optional bounded network codec workers

Implemented with synthetic loopback evidence, 2026-10-29. The independent
[`network/codecs`](../network/codecs/README.md) workspace provides callable raw,
opaque lossless Zstd and explicitly lossy Opus envelope workers. It has no Rust
dependencies and links installed libraries only when selected by build features.
The existing raw QUIC protocol, Codec enum, Session, root workspace and native
runtime are unchanged. Authenticated Session integration is a separate gate;
this envelope is not automatically accepted by the existing raw transport.
`backendAvailable` and `audioApplied` remain false for native audio.

## Exact admission and canonical identity

The public contract is:

```rust,ignore
Format { rate: u32, sample: Sample, channels: Vec<String>, frames: u32 }
Plan { format: Format, encoding: Encoding }
Encoding::Raw
Encoding::Zstd { level: u8 }
Encoding::Opus(OpusParameters { bitrate: u32, complexity: u8, vbr: bool,
                              conversion: Conversion::NormalizedF32Clamp })
Budget { max_block_bytes: usize, max_encoded_bytes: usize }
Binding { connection: [u8; 16], stream: u64, generation: u64 }

let negotiated = negotiate(&requested, &sender_plans, &receiver_plans,
                           explicit_lossy_consent, budget)?;
let mut encoder = Encoder::new(canonical_spec.clone(), negotiated.clone(), binding)?;
let mut decoder = Decoder::new(canonical_spec, negotiated, binding)?;
let envelope = encoder.encode(&whole_interleaved_raw_block)?;
let raw = decoder.decode(admitted_sequence, &envelope)?;
```

Both endpoints validate their local budget and require the **same complete Plan**
in both capability lists: rate, sample type, channel IDs in their original order,
frames, encoding and every parameter. There is no preference search, quality
fallback, automatic lossy choice, implicit resampling or inferred channel map.
Lists contain at most 16 entries; the enclosing reliable-control parser must bound
message bytes, nested metadata and parse work before constructing them. A missing
feature returns Unavailable. Worker creation must succeed before the parent
commits admission; FFI initialization can reject a library/parameter mismatch.

`Encoder<T>`/`Decoder<T>` own the complete caller-supplied canonical Spec as opaque
`T`, expose it by `canonical() -> &T`, and cannot rewrite its provenance. That does
**not** validate it or prove its association with a separately supplied Format.
The Session adapter must derive Format from the admitted raw Spec: map its five
raw codec variants directly to the matching Sample, copy exact `sample_rate`,
ordered `channels`, and `frames_per_packet` (widening u16 to u32). Reject encoded
or unknown source types. Check stream ID against Binding. Retain the entire
original Spec, including name, sources/groups and visited peers, in T; carry the
explicit conversion Plan separately. F64 containers after Opus do not regain
source precision. Existing raw sibling destinations keep their original bytes.

The parent owns certificate/peer authentication, directional and exact-plan
permissions, capture/receive/destination consent, exclusions, mix-minus and loop
prevention from [network-audio](network-audio.md). Codec selection grants none of
these. Legacy discovery never establishes paired trust. T must obey the parent's
existing metadata budget; this crate cannot bound arbitrary generic caller data.

## Versioned envelope

All integer fields are unsigned big endian. Payload raw samples are interleaved
little endian, in exactly the admitted order. The 56-byte header is:

| Offset | Length | Field |
| --- | --- | --- |
| 0 | 4 | `MXC1` magic/version |
| 4 | 1 | Encoding: raw 0, Zstd 1, Opus 2 |
| 5 | 3 | Reserved; must be zero |
| 8 | 16 | Connection nonce |
| 24 | 8 | Stream ID |
| 32 | 8 | Generation |
| 40 | 8 | Sequence, starting at zero per encoder |
| 48 | 4 | Exact decoded block bytes |
| 52 | 4 | Exact encoded payload bytes |
| 56 | variable | Exactly one codec payload |

Use a unique nonzero connection nonce from the authenticated connection context,
nonzero stream ID and nonzero monotonically increasing generation. Parameters
belong to reliable admission and cannot change within that tuple. The nonce is
a binding identifier, **not** a MAC, credential or encryption mechanism. The
enclosing authenticated transport must bind the entire plan to that tuple, reject
unadmitted data, and provide bounded packet age/replay/reorder/rate/queue handling.
This crate owns no media queue, socket, timers, fragment reassembly or trust store.
Do not expose its synthetic TCP fixture as an unauthenticated media service.

`decode(expected_sequence, packet)` checks exact tuple, tag, sequence, decoded
size, encoded size, reserved bytes and total packet length before decoding.
The transport supplies the already admitted sequence; raw/Zstd can decode bounded
reordered blocks and do not themselves track replay. Opus requires sequence 0,
1, 2… without gaps or duplicates because its predictors carry state. Recover loss
through explicit reliable new-generation admission with fresh workers. PLC, FEC,
automatic replay, and cross-generation predictor reuse are absent. FFI decode
failure invalidates an Opus worker so a mutated predictor cannot continue.

## Resource and lossless frame bounds

Local budgets may narrow the implementation ceilings: 262144 decoded bytes,
270336 encoded bytes, and 56 header bytes. These are per-call work/resource
limits, not global audio-quality caps. Split larger source blocks explicitly at
whole sample frames. Raw/Zstd preserve any positive u32 rate, all five sample
types, and up to 128 uniquely named ordered channels (IDs at most 128 bytes).
The raw path has no 48 kHz/stereo/float32 limit.

Zstd compression levels 1–3 use a maximum 256 KiB window, known input length,
content-size field, checksum, no dictionary and zero internal worker threads.
Each block is an independent standard frame. Compression can expand entropy-rich
data; it remains Zstd within the explicitly accepted encoded budget or fails.
There is no disguised raw fallback or lossy fallback.

Before allocating decoded storage, the decoder checks normal frame magic,
reserved bits, required checksum, absence of dictionary IDs, **known exact frame
content size**, and window at most 256 KiB. Unknown-size frames, claimed expansion
beyond the admitted block, large windows, dictionary-bearing frames, skippable
frames, truncated input, trailing bytes and concatenated frames reject. The
bounded frame-size scan must consume exactly the input. Single-pass
`ZSTD_decompressDCtx` uses output storage allocated from the already admitted
block size; it does not allocate an input-advertised streaming window. The
[upstream API](https://github.com/meta/zstd/blob/v1.5.7/lib/zstd.h) explicitly
distinguishes single-pass decode from streaming window allocation. Header parsing
follows the [pinned frame format](https://github.com/meta/zstd/blob/v1.5.7/doc/zstd_compression_format.md).

Encoding owns a bounded payload buffer plus the final envelope; decoding owns
one exact output block. Opus additionally uses a bounded f32 conversion buffer
and a 4000-byte packet buffer. Each worker owns a pair of encoder/decoder library
contexts, even though it uses one direction; `context_bytes()` includes both.
Zstd reports current library context size; Opus states use sized, aligned Rust
storage with a 1 MiB ceiling for each state. Zstd context memory depends on the
installed library, bounded input, window and fixed compression levels; no numeric
portable allocator or RSS guarantee is claimed. Context observations exclude
canonical T, caller buffers, envelope/conversion buffers, transport, allocator
overhead and OS memory. The parent must budget their aggregate stream count.

Calls run on an exclusively owned worker, never in a realtime callback. Input,
output, compression level, complexity and frame duration bound per-call work;
they do not establish a scheduling deadline or hard CPU-time limit. Parent
workers need admission rate limits and cancellation/timeouts around their own
queues/control IO. An already running synchronous FFI call cannot be interrupted.

## Explicit Opus domain

The implemented single-stream Opus profile accepts exactly 8000, 12000, 16000,
24000 or 48000 Hz; a single explicitly named mono lane or ordered `[FL, FR]`
stereo; and 2.5, 5, 10, 20, 40 or 60 ms frames. Other rates/layouts reject with an
explicit external-conversion requirement; the crate supplies no resampler or
channel mapper. These limits apply to the Opus destination only.

Parameters are target bitrate 6000–512000 bit/s, complexity 0–10 and explicit
VBR/CBR. The worker checks the library's reported target bitrate after setting
it and rejects a changed value. Actual achieved bitrate is measured from bytes,
not promised by a target, especially for VBR and short fixtures. Application is
fixed AUDIO, forced negotiated mono/stereo coding, FEC off, DTX off. The profile
requires at least 4000 encoded bytes of local capacity, including for long high
bitrate frames; it never silently imposes the smaller 1275-byte packet cap.
The [official encoder API](https://www.opus-codec.org/docs/opus_api-1.5/group__opus__encoder.html)
documents supported frame durations and packet-capacity versus bitrate control.

`NormalizedF32Clamp` explicitly converts source samples into the Opus float API:
S16/S24/S32 divide signed integers by 2^15/2^23/2^31. F32/F64 must be finite and in
[-1,1]. F64/S32 lose precision in f32 conversion before coding. Nonfinite or
out-of-range source samples reject; they are never silently normalized. On
decode, finite float samples clamp to [-1,1], integers round to nearest with ties
away from zero and saturate to their signed range, and floats store the clamped
value in their admitted container. These conversions and Opus coding are lossy.
NaN payloads, signed zero and low bits have **only raw/Zstd exactness claims**.

`lookahead_frames()` reports the library's encoder algorithmic lookahead. The
fixture observes 104 frames at 16 kHz and 312 at 48 kHz (both 6.5 ms). It does not
remove leading delay, flush trailing codec history, or measure physical latency.

## Revocation, restart and reconnect

Keep each worker's `revocation()` handle in the parent Session before moving the
worker. Revoke it on permission withdrawal, reliable restart, disconnect or
receive-worker cancellation, and drop old workers. Encoder and decoder have
separate handles; invalidate all relevant workers. A fresh worker does not
invalidate an old one automatically. Parent admission must enforce generation
monotonicity and nonce uniqueness; local constructors alone cannot prove them.

`revoke()` is a nonblocking atomic invalidation. Encode/decode check before work
and at the output commit check. If revocation precedes that final check, output
is zeroed and Inactive is returned. An operation whose final check wins may still
hand its committed bytes to its caller during a concurrent revocation; invalidate
downstream delivery too. Later calls fail Inactive. There is no completion receipt
or promise to recall already committed blocks or caller/transport copies. Codec
history is inaccessible after invalidation and released on worker drop; Opus
state is cleared on drop. This is not secure erasure of library/allocator memory.
Dropping a worker removes its state; no media can instantiate a new worker.

## Executable evidence

On homelab-dev Linux x86_64, kernel 6.12.111+deb13-cloud-amd64, Rust 1.99.0,
system libzstd **1.5.7** and libopus **1.5.2**, the dependency-free
[`verify.sh`](../network/codecs/verify.sh) passed nine full-feature tests, two
default-feature tests and actual TCP loopback with 64 blocks per mode. It compiles
with warnings denied, no debug information or incremental output, and opt-level 2
for the example. Final private output occupied about 12 MiB. No Cargo build,
downloads, root installs, real devices or audio server were used.

Coverage includes S16/S24/S32/F32/F64 integer extremes/low bits and IEEE quiet and
signaling NaN payloads, infinity, subnormals and signed zero as opaque bytes;
irregular/extreme positive raw rates; ordered multichannel layouts; 256 KiB,
128-channel F64; entropy-rich Zstd expansion; three independent stream lifecycles;
malformed headers, expansion/unknown size, window/dictionary/checksum/trailing
frame rejection; exact parameter mismatch; revoke, restart and foreign reconnect
envelopes. Opus runs all 300 combinations of five rates, six durations, two layouts
and five containers, plus conversion oracles, nonfinite rejection, wrong packet
duration/layout and predictor invalidation. The 512 kbit/s CBR 60 ms fixture checks
the actual 4000-byte capacity argument forwarded to libopus, then uses its public
packet-padding API to create a valid 4000-byte packet and proves byte-identical
decode through the public envelope worker. Native output size is not assumed:
installed libopus versions may emit at most 1275 bytes for the same input while
reporting the requested target bitrate. The target readback and measured encoded
bytes remain distinct. Physical native precision remains unproved.

One example run on 2026-10-29 produced these **fixture observations**:

| Mode / source | Raw bytes | Envelope bytes | Encode / decode wall ms | Whole loop wall / process CPU ms | Largest loop call ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Raw, F64/44117 Hz/6ch | 393216 | 396800 | 0.029 / 0.016 | 2.185 / 2.188 | 0.046 |
| Zstd level 3, same source | 393216 | 6848 | 1.012 / 0.138 | 3.400 / 3.411 | 0.205 |
| Opus 64 kbit/s VBR complexity 5, F32/16 kHz/mono | 40960 | 8768 | 2.170 / 0.968 | 5.269 / 4.977 | 0.181 |
| Opus same parameters, F32/48 kHz/stereo | 245760 | 8768 | 4.332 / 2.298 | 12.325 / 11.121 | 0.521 |

Wire counts include the 56-byte envelope, not TCP/IP overhead. Zstd input is a
highly repetitive special-word oracle; its 0.0174 ratio is not representative
music compression. Separate entropy-rich fixtures expand and still decode
exactly. Opus waveform SNR after aligning reported lookahead was 36.44 dB mono
and 28.00 dB stereo, a deterministic tone-fixture result rather than a listening
quality qualification. The full admitted raw path stays byte-exact in parallel.

Worker elapsed/max_elapsed use `Instant` wall time and count successful operations;
they include allocations/conversion and scheduling. Linux process CPU is measured
separately with CLOCK_PROCESS_CPUTIME_ID and includes signal generation, copying,
encode/decode and local socket work in the entire loop, not isolated codec CPU.
The largest local encode→TCP→decode duration includes both endpoints in one
process; it is neither one-way network latency nor capture-to-speaker latency.
No real-time pacing or callback deadline is tested.

Observed per-worker library context bytes were Zstd 242968 at the encoder / 101280
at the decoder, Opus mono 184696 / 184696 and stereo 198376 / 198376. Linux process
VmHWM ended at 5316 kB across all four sequential modes, including fixtures,
buffers, library overhead and prior allocations; it is not per-codec peak memory.
Throughput, CPU, latency and memory will change with inputs, libraries and host load.

Authentication, production reliable-control plan serialization, exact Spec adapter
validation, Session integration, datagram fragmentation/MTU, packet loss recovery,
pacing, native endpoints, two-host/LAN tests, physical latency, sustained heavy
load remain separate qualification gates. The dedicated
[platform workflow](network-codec-platforms.md) now requests actual system-library
worker tests and synthetic TCP loopback on Linux, Windows and macOS, separately
from raw-only checks. A requested job is not passing platform evidence: inspect
the exact revision's run and uploaded logs before making a qualification claim.
Authenticated integration remains independently owned by the parent.

## Dependency sources and distribution

The actual linked libraries are the installed shared `libzstd.so.1` (1.5.7) and
`libopus.so.0` (1.5.2), with versions checked using pkg-config before proof runs
and confirmed from linked runtime calls: `zstd_version() == Some(10507)` and
`opus_version() == Some("libopus 1.5.2")`. The example prints both at startup.
No third-party source is copied or vendored here. Zstd 1.5.7's
[BSD license](https://github.com/meta/zstd/blob/v1.5.7/LICENSE) is a permissive
distribution option; upstream also provides
[GPL-2.0 as an alternative](https://github.com/meta/zstd/blob/v1.5.7/COPYING).
Opus 1.5.2's [COPYING](https://github.com/xiph/opus/blob/v1.5.2/COPYING) supplies
its BSD-style terms and additional patent-license references;
[upstream README](https://github.com/xiph/opus/blob/v1.5.2/README) describes the
associated royalty-free patent grants. Retain applicable notices and review
those terms for each distribution. This implementation makes no certification,
trademark, third-party patent clearance or shipped-binary licensing claim.
