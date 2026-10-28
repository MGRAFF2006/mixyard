# Synchronous PCM core

`mixyard::audio::Processor` now processes supplied samples. The existing CLI,
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

The processor accepts interleaved IEEE754 Float32 mono/stereo PCM, with finite
samples. All mixes and selected configured sources/outputs must have known formats
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
accumulator and one Float32 buffer per mix channel, plus independent Float32
buffers for every output channel, all at `max_frames`. Each counted slot costs at
most eight bytes. Configured count ceilings also bound render loop work. Compiled
plan/edge metadata is additional; this budget does not cap planner input strings,
inventory observations or diagnostics. Compilation and planner validation allocate,
so callers must admit their actual resources before publishing a processor. Input
PCM is borrowed.

1. Inspect `processor.plan()` to bind source slots to the exact `observed_ids` and
   outputs to exact target/observed identities. Source, mix and output indices
   follow this immutable plan's deterministic order. Cache indices off callback;
   do not assume the input Config vector has the same order.
2. Prepare one `Option<Pcm>` per planned source. Supply `None` for absent blocks
   and every unresolved source. Every supplied block declares its actual format
   and has exactly `frames * channels` samples. A missing block is zero this call;
   the preceding block is never reused. Caller-owned input descriptors can also
   be preallocated and reused.
3. Call `render(frames, &inputs)` with `1..=max_frames`. It clears all storage,
   including unused tails, validates **all** supplied data, and then processes
   sends into separate mastered mix buffers and output fanout buffers. Muted and
   unrouted supplied data must also be finite. Unknown extra source slots,
   incorrect lengths/formats, zero/oversized blocks and nonfinite values reject.
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
compressed payloads to correctly declared Float32 PCM, and handle bounded receive/
jitter storage, common-timeline alignment and any required rate adaptation before
calling this API. Input slices stay valid for the synchronous call only; this core
retains no input references. Missing/stale stream data becomes `None`, with no
historical replay or cross-slot substitution. Sending mix/output buffers likewise
requires separately owned bounded transport and freshness/lifetime handling. This
slice adds no network protocol, codec, compression choice or network I/O.

Mono duplicates to stereo; stereo averages left/right into mono. Send gain/mute
is independent for each source-to-mix edge; mix master is applied once via the
planner's send coefficient. Each output applies its own gain/mute after the mix.
Summation and channel folding widen to Float64, then round to Float32 at the mix
and output boundaries. No limiter, clipping, gain fitting or normalization is
performed. With normalized input, planner headroom provides the conservative peak
bound; finite input above full scale is not silently altered. A single-source,
same-layout unity path preserves Float32 bits, including signed zero/subnormals.
Other gains/sums incur normal floating-point rounding. No latency, performance
deadline or platform floating-point-mode qualification is claimed.

## Executable evidence

From the repository root, with the existing Rust dependencies only:

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --test audio_core --test audio_allocations
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
40 additional arbitrary mixes, and non-48-kHz admission. Allocation instrumentation
counts alloc/realloc/free operations on the actual render thread over successful,
silent and rejected calls; it is a heap-behavior check, not a realtime benchmark.

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
