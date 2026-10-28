# Synchronous PCM core

`mixyard::audio::Processor` (Float32) and `Processor64` (Float64) process supplied
samples through the same `TypedProcessor<S>` implementation. The existing CLI,
runtime, web/desktop clients and native probes do not invoke it yet. This is an
offline core implementation, not native device delivery or audio qualification.

`Processor::compile(&Config, &Inventory, sample_rate_hz, Limits)` calls the
authoritative schema-2 `mixyard::plan` first. It consumes that plan's exact source
resolution, channel mappings, send effective gains (including mix master), and
output effective gains. It does not accept an independently editable routing
graph or plan. Exclusions, microphone-monitoring consent, recapture rejection,
conservative headroom, backend capacity and observation conflicts retain the
planner's validation. Missing/ambiguous sources and outputs remain disconnected;
there is no default-device or alternate-source fallback. Conservative headroom
still includes configured absent sources.

The caller explicitly selects interleaved IEEE754 Float32 (`Processor`, `Pcm`)
or Float64 (`Processor64`, `Pcm<f64>`) input, mix and output storage at compilation.
Both paths accumulate in Float64; the Float64 path has no Float32 intermediate.
`precision()` identifies the precision of all buffers, and `output_samples` returns
`&[f32]` or `&[f64]` accordingly. `output_format()` still describes rate/layout
only: schema 2 has no precision or general ordered-channel descriptor. Do not
infer endpoint encoding from that policy field. Samples must be finite and layouts
remain mono/stereo. This is the precision slice of [issue #29](https://github.com/MGRAFF2006/mixyard/issues/29),
not completion of its local/network format-preservation requirement; a versioned
format/channel policy and adapter negotiation are separate pending work.

All mixes and selected configured sources/outputs must have known formats
at the same explicit positive nominal rate; 48 kHz is not hard-coded. Compilation
rejects unknown selected formats, multichannel formats, invalid rates and any
required rate conversion, including on muted or unused selected ports. An
unsupported observed output format therefore rejects compilation explicitly,
rather than executing the planner's degraded unsupported output. Missing and
ambiguous ports can still compile into silence. This strict execution admission
does not change the planner's more permissive dry-run contract.

Same nominal rate is **not** proof of a common device clock. This synchronous API
requires callers to supply already aligned blocks in one timeline. It performs no
clock negotiation/adaptation, buffering between callbacks, resampling, drift
compensation, native capture, device publication, playback or recording.

## Adapter use

Compile off the audio callback. `Limits` has no implicit defaults: the host chooses
`max_frames`, source/mix/send/output ceilings, and `max_buffer_samples`. Counts
exceeding a ceiling fail without truncation. Buffer sizes use checked arithmetic
and addressable-slice checks. The scalar-storage budget includes one Float64
accumulator and one selected-precision buffer per mix channel, plus independent
selected-precision buffers for every output channel, all at `max_frames`. Each
counted slot costs at most eight bytes. Configured count ceilings also bound render loop work. Compiled
plan/edge metadata is additional; this budget does not cap planner input strings,
inventory observations or diagnostics. Compilation and planner validation allocate,
so callers must admit their actual resources before publishing a processor. Input
PCM is borrowed.

1. Inspect `processor.plan()` to bind source slots to the exact `observed_ids` and
   outputs to exact target/observed identities. Source, mix and output indices
   follow this immutable plan's deterministic order. Cache indices off callback;
   do not assume the input Config vector has the same order.
2. Prepare one `Option<Pcm>` (Float32) or `Option<Pcm<f64>>` (Float64) per
   planned source. Supply `None` for absent blocks and every unresolved source. Every supplied block declares its actual format
   and has exactly `frames * channels` samples. A missing block is zero this call;
   the preceding block is never reused. Caller-owned input descriptors can also
   be preallocated and reused.
3. Call `render(frames, &inputs)` with `1..=max_frames`. It clears all storage,
   including unused tails, validates **all** supplied data, and then processes
   sends into separate mastered mix buffers and output fanout buffers. Muted and
   unrouted supplied data must also be finite. Unknown extra source slots,
   incorrect lengths/formats, zero/oversized blocks and nonfinite values reject.
   Processing that overflows to nonfinite values also rejects the entire block
   with `RenderError::NonFiniteResult`, clearing every buffer again.
4. On success, read `mix_samples(index)` / `output_samples(index)`. They expose
   only the current `frames()` worth of PCM. A failed call leaves **all** mix and
   output buffers zero; `frames()` becomes `min(requested, max_frames)`. Handle the
   error and silence/degrade the native path rather than submitting a failed
   block as successful audio. No partially rendered result or prior PCM remains.
5. `output_format(index)` returns the selected output buffer's format, or `None`
   for missing/ambiguous ports. A disconnected port's inspection-only silence
   buffer uses its mix's nominal layout. Submission requires a selected planned
   output and independently qualified endpoint/clock binding. `plan().mode` remains
   `dry-run` because it describes the original policy computation, not native
   execution or delivery.
6. Recompile and rebind after config/inventory/format changes. The plan retains
   config/inventory revisions and evidence. Discard slot bindings from the old
   snapshot. The host owns freshness, callback lifetime, atomic graph replacement
   and reclamation off callback. Do not compile, drop or swap heap ownership in
   the realtime callback.

`render` performs bounded work proportional to compiled counts and the configured
maximum block size, with no I/O, locks, heap allocation/deallocation or external
callbacks. Rust's exclusive mutable borrow prevents concurrent rendering of the
same processor. Read-only output slices cannot outlive the borrow and then be
used across the next render. The API has no native per-output queues or clocks;
reading one output does not consume another's PCM, but downstream nonblocking
transport and slow-device isolation remain the adapter's responsibility.

A future network adapter can supply independent remote application streams or
received premixed buses as separately resolved source slots. It must bind exact
identities through the existing Config/Inventory planner contract, decode raw or
compressed payloads into the explicitly selected precision, and handle bounded
receive/jitter storage, common-timeline alignment and any required rate adaptation
before calling this API. Input slices stay valid for the synchronous call only; this core
retains no input references. Missing/stale stream data becomes `None`, with no
historical replay or cross-slot substitution. Sending mix/output buffers likewise
requires separately owned bounded transport and freshness/lifetime handling. This
slice adds no network protocol, compression choice or network I/O. A received
S32/F64 stream must use the Float64 path unless the caller explicitly authorized
narrowing or the particular final destination requires it. S16/S24/S32 can decode
exactly into Float64 using the helpers below. Destination conversion belongs as
late as practical in each destination's adapter; a limited sibling output must
not downgrade the source or other outputs. Choosing this processor's Float32 API
is not permission to narrow higher-precision source data silently.

Mono duplicates to stereo; stereo averages left/right into mono. Send gain/mute
is independent for each source-to-mix edge; mix master is applied once via the
planner's send coefficient. Each output applies its own gain/mute after the mix.
Summation and channel folding use Float64. Float32 processing rounds to Float32
at the mix and output boundaries; Float64 processing retains Float64 there. No
limiter, clipping, gain fitting or normalization is performed. With normalized input, planner headroom provides the conservative peak
bound; finite input above full scale is not silently altered. A single-source,
same-layout unity path preserves the selected floating-point bits, including
signed zero/subnormals. Averaging avoids intermediate overflow for maximum-finite
Float64 inputs. Other gains, folds and sums incur normal Float64 arithmetic
rounding (and Float32 boundary rounding when selected); arbitrary Float64 mixing
is not promised bit-identical to input. Requested DSP may lose low bits or
underflow. Nonfinite results reject rather than silently clip. No latency,
performance deadline or platform floating-point-mode qualification is claimed.

## Exact signed PCM conversion

`audio::pcm::{decode, encode_exact, SignedPcm, PcmError}` provides allocation-free
conversion with caller-owned slices. `SignedPcm` explicitly selects S16LE/BE,
packed S24LE/BE (three bytes, not 24-in-32), or S32LE/BE two's-complement samples.
No native byte order, padding, interleaving conversion or channel guessing is
implicit. Sample order is unchanged; callers retain the explicit frame/layout
contract. Source bytes and destination samples must match exactly in length.

`decode(format, bytes, &mut float64)` divides each signed integer by `2^(bits-1)`.
Every supported integer widens exactly: the normalized range is
`[-1, 1 - 2^-(bits-1)]`. In particular S32's positive maximum is
`1 - 2^-31`, not `1.0`. Decode directly into caller-preallocated Float64 input
storage, then supply `Pcm<f64>` to `Processor64`; no Float32 intermediate is used.

`encode_exact(format, float64, &mut bytes)` requires every scaled value to be an
integer within the destination's asymmetric signed range. It rejects NaN/infinity,
out-of-range values (including +1), and fractional destination LSBs. It never
clips, saturates or truncates; there is **no implicit quantization or dither**.
Signed floating zero encodes as integer zero. An unprocessed unity route can
round-trip every supported integer value exactly. DSP results may require explicit
caller-selected quantization before this exact encoder can accept them; that
policy is not implemented here. A half-gain fanout of an odd S32 integer, for
example, retains the fractional LSB in Float64 but rejects S32 encoding without an
explicit conversion decision. Its untouched sibling remains exact.

All conversion errors clear the entire destination before returning bounded
numeric error metadata; encoding validates all samples before writing any bytes.
Decode malformed lengths likewise leave all sample storage zero. Matching empty
slices are accepted. These helpers are PCM codecs only, not raw Float64/encoded/DSD
passthrough or native/universal format support.

## Executable evidence

From the repository root, with the existing Rust dependencies only:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --test audio_core --test audio_precision --test audio_allocations
CARGO_BUILD_JOBS=2 cargo test --locked --test audio_core offline_render_demo -- --nocapture
```

The demo renders three frames from the actual schema-2 example configuration and
prints every output. For example, the Call output is
`[0.5, 0.5, -0.25, -0.25, 0.125, 0.125]`; Record is
`[0.34375, 0.0625, -0.03125, -0.0625, 0.125, 0.25]`. Literal expected PCM in the
tests is independent of planner coefficients and implementation loops. Further
checks exercise independent gain/mute/master/fanout, stereo folding at both
boundaries, bit transparency, private-source omission, no fallback/replay, source
and output loss, malformed/nonfinite blocks, policy rejection, explicit budgets,
40 additional arbitrary mixes, and positive-rate admission (including 44.1, 96,
192, 384 and 768 kHz, low rates, and non-whitelisted rates). Precision tests cover
Float64 differences below Float32 resolution, S32 extrema and adjacent low bits,
packed PCM endian oracles, exact codec rejection/clearing, finite DSP overflow
rejection/recovery, independent gains/mutes/fanout and Float64 storage budgets.
Allocation instrumentation counts alloc/realloc/free operations on the actual render thread over successful,
silent and rejected calls, including Float64 and every signed codec; it is a
heap-behavior check, not a realtime benchmark.

These are actual **core sample-processing** results. The synthetic analyzer
self-check in [verification/README.md](../verification/README.md) separately tests
its oracle. Neither proves that an external consumer received virtual microphone
PCM, native producer-crash silence, independent device-clock stability, end-to-end
latency, dropout freedom or the quality/endurance release gates.

## Native integration next

No native adapter or held PR16 is modified. Linux's adapter still models a
separate PipeWire graph. The Windows ingress artifact has a fixed 48-kHz stereo
Float32 packet contract with bounded frames and generation/epoch/revision/
sequence checks; queue acceptance is not a sample-effective delivery ACK. The
macOS transport artifact likewise needs its own qualified endpoint binding and
producer lifetime. None is connected to this processor by this change.

The next integration must bind exact selected native capture identities, negotiate
formats, align capture timelines and preflight independent output queues/clocks
off callback. It can then supply these blocks and serialize each selected output
into its adapter's bounded transport with current generation/revision/freshness
metadata, zeroing on loss rather than replaying. Rate/layout/clock changes must
stop admission or use separately implemented and qualified adaptation; they must
never be relabeled as same-rate PCM. Real external-consumer recordings, failure
and reopen tests, and clock/latency/stress/endurance evidence remain required in
the separately authorized native environments. This slice enables none of those
host/driver mutations or private-proof retries.
