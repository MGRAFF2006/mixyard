# Revisioned DSP hosting and sample-domain smoothing

`mixyard::dsp::live::LiveChain` runs the existing Float64 `Chain` processors at
an explicit source, mix or output attachment. It adds bounded block hosting,
generation/revision guards, post-chain gain ramps and wet/dry ramps. It does
not discover routes, grant permissions, start a worker, install callbacks or
change `backendAvailable`/`audioApplied`. Native integrated delivery and physical
qualification remain separate gates. See [the processor contract](dsp.md).

## Attachment and worker integration

The host resolves each `Attachment::{Source,Mix,Output}(u64)` handle against its
immutable graph. These opaque handles are not OS IDs, names or authorization
credentials. A `Source` instance runs before the sends that explicitly choose
its processed result; a `Mix` instance runs after authorized sends are summed;
each `Output` instance runs after fan-out and has separate audio state. Keeping
raw and processed source sends requires explicit host paths. Processing one
destination must never alter the upstream mix or a sibling destination.

Compile one instance per attachment off the callback, with nonzero generation
and revision. Every block declares the exact binding and immutable `dsp::Format`
(positive sample rate and ordered unique channel identities). The API accepts
interleaved `f64` only. There is no hidden resampling, Float32 intermediate,
channel reordering, limiter or normalization. A format match does not prove a
common clock; the host must already align the timeline.

For the runtime worker contract, map `worker::Binding` generation/revision plus
the graph attachment into `live::Binding` at a host boundary. Cache a matching
`dsp::Format` from the exact worker rate and ordered channels on the control
thread. Only a worker descriptor explicitly admitting Float64 maps directly to
this API. A Float32 or integer source needs an explicitly selected DSP conversion
path; this module never infers or performs it. Convert for a limited final sink
only on that destination's path, retaining sibling precision and channels.

The worker owns stamp sequence, first-frame continuity, expiry and explicit
engine-clock validation. Validate these before DSP and publish only successful
DSP output with the same admitted timeline. Control revisions must be reconciled
at a quiescent boundary with downstream workers: an old immutable queue binding
cannot be relabeled as the new DSP revision. Recreate/rebind queues off callback
when required. Neither the host handle nor this module verifies permissions or
provenance on its own.

```rust
use mixyard::dsp::{ChainConfig, Format, Limits};
use mixyard::dsp::live::{Attachment, Binding, Controls, LiveChain};

let format = Format { sample_rate_hz: 96_000, channels: vec!["R".into(), "L".into()] };
let binding = Binding { attachment: Attachment::Output(42), generation: 7, revision: 1 };
let limits = Limits {
    max_block_frames: 128, max_channels: 8, max_effects: 32,
    max_memory_bytes: 1024 * 1024, max_work_units_per_block: 1_000_000,
};
let config = ChainConfig { name: "output trim".into(), format: format.clone(), effects: vec![] };
let mut live = LiveChain::compile(config, limits, 9_600, binding, Controls::UNITY)?;
let next = live.update(binding, 2, Controls { gain: 0.0, wet: 1.0 }, 4)?;
let report = live.process(next, &format, 4, &[0.5; 8])?;
assert_eq!(live.output(), &[0.375, 0.375, 0.25, 0.25, 0.125, 0.125, 0.0, 0.0]);
assert_eq!(report.transition_remaining_frames, 0);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Control and lifecycle behavior

`update(expected_binding, next_revision, controls, transition_frames)` validates
the exact current binding and requires the next consecutive revision, without
wrapping. Apply it at a block boundary after checking host authority/freshness.
It clears the previously published block. A transition of N frames advances
1/N on its first frame and reaches the exact target on frame N, regardless of
block partition. A new update starts at the current value actually reached,
including an interrupted ramp. All channels advance together once per frame.
Zero frames explicitly selects an immediate change, which can click.

Gain must be finite and nonnegative; wet must be finite in `[0,1]`. The final
sample is the wet/dry mixture followed by the post-chain gain. Wet zero is dry
monitoring while the effects continue running, including tails and resource
cost. It does not disable the chain. Warm processing avoids an old frozen delay
tail reappearing when wet rises; it also means an effect error fails the whole
block even when dry is selected. Whole-chain wet/dry mixing does not compensate
filter phase or delay; a ramp is not a guarantee of inaudibility for every
signal. This is linear amplitude interpolation, not equal-power mixing.

Unity and endpoint assignments preserve Float64 low bits, signed zero and
subnormals. Values over full scale remain intact and are counted from the
**final** output. Finite-result failure rejects the whole block without exposing
a prefix. There is no automatic quality reduction or fallback chain.

Any invalid block, stale binding, invalid control update or explicit `revoke()`
clears all effect histories, output buffers and ramp state, then permanently
latches this instance inactive. Retrying with an earlier valid token cannot
resume it. On source loss, revoke, restart, authorization/format changes or
generation replacement, the host must invoke this privacy path and clear its
own queues/copies. A process that never receives notice cannot detect revocation.
Compile and explicitly authorize a fresh generation off callback; never crossfade
the revoked old instance into the new one.

Effect parameters/topology remain immutable. Compile replacements off callback,
quiesce processing, swap ownership and reclaim old allocations off callback.
This module smooths post-chain trim and wet/dry controls; it does not claim
arbitrary coefficient automation, plugin hosting, graph scheduling, a concurrent
SPSC transport or click-free replacement of a chain snapshot.

## Bounds and verification

The supplied `Limits` cover the host and existing chain together, including the
extra output storage and host struct, format comparison and abstract smoothing
work. `max_transition_frames` explicitly bounds transition duration. Admission
retains the underlying effect latency/tail metadata; smoothing adds no buffered
frames. Memory excludes allocator bookkeeping, caller buffers and transient
compile allocations. Work units are conservative abstract counts, not CPU time.
No effect is dropped to meet a budget: compilation rejects excess.

`process`, `update`, `revoke` and their error paths use exclusive mutable access
and preallocated storage, with no heap allocation, deallocation, locks, I/O,
timers or external callbacks. Reset work is bounded by admitted state. These
methods do not make an unqualified platform scheduler hard realtime.

Run `cargo test --locked --test dsp_live -- --nocapture` for attachment isolation,
sample-time and interrupted ramps, warm wet/dry behavior, precision and exact
format checks, generation/revision/restart/revoke safety, invalid blocks and
overflow, combined resource rejection and allocator instrumentation. The timing
test warms then measures 512 actual `process` calls externally at Float64,
96 kHz, two ordered channels and 128 frames with gain/EQ/delay/FIR/compressor.
It prints build context, total/max processing time, the actual frames/rate block
duration and observed misses. Timing misses are observations, not permission to
degrade effects, and no timing value is asserted as a portable performance gate.
This synthetic run does not qualify native callbacks, physical devices, sustained
endurance, other hosts or other admitted chains. Cargo/integrated and physical
results must be reported separately from a portable serde-stripped rustc fixture.
