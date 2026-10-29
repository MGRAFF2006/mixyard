# Destination-local PCM and clock-rate adaptation

`audio::clock` is a callable production module with synthetic execution evidence.
It does not connect a native device or change `backendAvailable`/`audioApplied`.
The existing policy processor remains mono/stereo; these general multichannel
adapters do not make that processor accept a wider layout.

`Format` contains a positive exact integer rate and unique, nonempty ordered lane
labels. `RateAdapter<f32>` and `RateAdapter<f64>` retain that selected storage
precision; there is no Float32 intermediate in the Float64 path. Source and
destination labels must match exactly in order. There is no implicit channel
selection, matrix, folding, normalization, clipping or universal rate ceiling.
The host chooses explicit resource limits. Unsupported orders and resources fail
without truncation.

## Exact bypass and explicit conversion

`Method::Bypass` requires equal rates and one common clock ID, then copies the
original floating values without arithmetic, including signed zero, subnormals
and Float64 bits below Float32 resolution. Equal nominal rates on independent
clocks do not satisfy this contract.

`RawBypass` requires equal `RawFormat` descriptors, including ordered lanes,
nominal rate, encoding, width and byte order. It supports packed signed
16/24/32/64-bit PCM and IEEE Float32/64 in explicit little/big endian order.
Samples are copied as bytes without interpretation, so even S64 low bits never
pass through Float64. Raw floating payloads are not checked for finiteness.
This is PCM only: encoded/DSD transport, padded native containers and arbitrary
planar buffers need their own exact descriptors and qualified bridges.

`narrow_f64` makes destination Float32 conversion explicit. `Narrowing::Exact`
rejects any finite value which would lose precision; `RoundToNearest` permits
the Rust floating conversion, including finite underflow to zero. Neither
permits overflow to infinity. `widen_f32` widens every finite Float32 exactly.
Errors clear the entire caller-owned destination. The caller admits those slice
sizes before callback use. These functions do not quantize to integer PCM or
provide dither. Apply narrowing to one destination's buffer, leaving the source
and its other destinations unchanged.

## Streaming rate conversion and correction

Construct a separate `RateAdapter<S>` for each destination, off callback. Its
ring retains source samples in `S`; output storage is independent. `push` copies
admitted interleaved blocks. `render` requests a bounded exact output frame
count and returns a `Block` plus `output()` samples. It never returns a partial
successful block. Underflow closes and clears this destination; the host must
admit input lookahead/prebuffer before rendering. There is no implicit EOF
padding or repeated final sample. Queue occupancy is not measured latency.

`Method::Linear` explicitly selects two-point linear interpolation. Phase uses
integer rational units (`destination_rate * 1_000_000`), retaining the remainder
across blocks and ring wrap. Integral positions copy samples unchanged;
fractional positions interpolate in Float64 and round to the selected `S`.
Interpolation has ordinary floating arithmetic error and may alter low bits.
This is **not band-limited**: downsampling can alias, and upsampling has passband
droop and imaging. It is a bounded reference method, not a high-fidelity quality
claim or an automatic choice for raw/high-quality routes. A qualified native or
band-limited converter remains necessary where those quality requirements apply.

`set_correction_ppm` explicitly applies a host-provided measured correction to
the source consumption rate; positive ppm consumes source frames faster. The
host sets a bounded permitted range below one million ppm. Changing correction
retains fractional phase. No correction is allowed in bypass mode. The module
does not estimate drift, synchronize clocks, infer hardware sample frequency,
translate clock offsets or treat RTT/2 as latency. A host must establish the
timeline relationship before choosing a different output clock ID. Abrupt
corrections are not smoothed; physical servo stability and audible continuity
remain unqualified.

## Lifecycle and host worker bridge

Local `Binding`/`Stamp` deliberately match the worker contract fields:
generation/revision, clock ID, sequence, first frame, host-monotonic capture time
and deadline. Map immutable descriptors/bindings once off callback; check that
worker `Precision` matches concrete `f32`/`f64` storage. `ClockSample` is sealed
to those two types and keeps this module independently compilable. No frozen
policy schema is replaced.

Bind input stamps to the actual source clock; output blocks carry the explicitly
selected engine/destination clock, their own contiguous sequence/frame count,
oldest contributing capture time and earliest contributing deadline. These
times describe source freshness, not an invented physical output timestamp.
`now_ns` and all stamp times must use the same host-monotonic domain. For each
new adapter, input sequence and first frame start at zero. A host accepting an
already-running stream must start a new explicit session numbering domain.

Push validates binding, clock, contiguous sequence/frame counts, lengths,
finiteness and freshness. Every ring frame retains capture/deadline metadata;
render checks each contributing frame's deadline again, including interpolation
lookahead. Reversed host time, stale/replayed/mismatched data, expired queued
audio, overflow, underflow and malformed requests terminally invalidate and
zero this adapter. Even a partially computed render leaves its entire visible
output silent. `revoke()` also clears ring/output state. Recovery requires a
new object with a fresh host binding; nothing rearms the old object or reuses
its history. Binding numbers themselves are not authorization credentials.

Successful push/correction clears the previous visible render output. A returned
sample borrow prevents mutating the adapter until it is released. Only the
current successful block may be sent to a consumer, promptly and subject to its
deadline; merely retaining a borrowed output is not a future freshness check.
Raw bypass performs the same stamp admission for each synchronous copied block.

This is exclusive `&mut` ownership, not an interthread SPSC transport. Native
bridges must independently qualify callback ownership, lock-free handoff,
clock measurement and bounded work deadlines. Creation, destruction and graph
replacement belong off callback. Push/render/correction/revoke perform bounded
work in the admitted ring/output sizes without heap operations, locks, I/O or
external calls. The byte budget includes ring/output samples and two timestamp
arrays; caller-owned descriptors/input buffers and small fixed metadata are
additional. Channel label count/length have separate admission limits.

## Verification

`tests/audio_clock.rs` imports the actual production module directly so it runs
without compiling unrelated dependencies. With a private target whose budget
and filesystem free-space floors have been checked:

```sh
TMPDIR="$clock_target" CARGO_INCREMENTAL=0 rustc --edition=2021 --test \
  -C debuginfo=0 tests/audio_clock.rs -o "$clock_target/audio_clock"
"$clock_target/audio_clock" --test-threads=1
```

The target is also discoverable by `cargo test --locked --test audio_clock`.
Local evidence is the portable rustc target, not a full Cargo/SDK integration
check. Tests cover exact ordered six-channel high-rate Float64 and raw PCM
bypass, source precision with independent destination narrowing, multichannel
fractional output, ring wrap, 44.1→48 kHz rational phase over 200 blocks,
downsampling across gaps between pushes, correction limits, timestamp
propagation, expiry, discontinuities, revocation/restart and sibling isolation.
Thread-local allocator instrumentation observes 512 iterations of successful
and terminal-error paths, counting allocation/reallocation/deallocation.

Physical device delivery, high-quality resampling, measured independent hardware
clocks, sustained timing/servo behavior, native callback integration, latency,
endurance and OS floating-point modes remain explicit qualification gaps.
