# Independent Float64 DSP chains

This is the first executable slice of [DSP #33](https://github.com/MGRAFF2006/mixyard/issues/33), preserving the [precision contract #29](https://github.com/MGRAFF2006/mixyard/issues/29). It depends on the Float64 core/codecs from PR #31. It adds no route permissions, native devices, web endpoints, callback installation, or changes to schema 2's existing routing policy. That policy's mono/stereo subset is **not** this DSP API's channel limit.

`mixyard::dsp::Chain` compiles an ordered, named effect list for interleaved `f64` blocks. Its format contains a positive negotiated integer sample rate and an explicit ordered list of unique opaque channel identities. Every positive `u32` rate is admissible, subject to parameters and caller resources, rather than a rate whitelist. There is no inferred layout, reordering, resampling, narrowing to `f32`, normalization, clipping or automatic limiter. Encoded/DSD passthrough is a separate capability, not a DSP input.

## Compile, process and attach

```rust
use mixyard::dsp::*;
let config = ChainConfig {
    name: "headphone correction".into(),
    format: Format {
        sample_rate_hz: 96_000,
        channels: vec!["front-right".into(), "front-left".into()],
    },
    effects: vec![EffectConfig {
        name: "polarity".into(), enabled: true,
        effect: Effect::Gain { gain: 1.0, invert_polarity: true },
    }],
};
let limits = Limits {
    max_block_frames: 256, max_channels: 8, max_effects: 32,
    max_memory_bytes: 16 * 1024 * 1024,
    max_work_units_per_block: 10_000_000,
};
let mut chain = Chain::compile(config, limits)?; // control thread
let report = chain.process(1, &[0.5, -0.25])?; // no allocation, locks or I/O
assert_eq!(chain.output(), &[-0.5, 0.25]);
assert_eq!(report.frames, 1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

A future caller owns attachment and authorization. A source chain placed before sends affects every send taking its processed output; a caller wanting both raw and processed sends must select those paths explicitly. A mix chain processes the mix after summing. An output chain gets its own instance and state, so headphone correction does not alter another output. Individual streams and already mixed network buses can use this same block API after separate admission. It has no route/source discovery or hidden access.

The caller must bind input length, rate and ordered identities to this compiled format. A block is only a slice, not self-describing metadata; the engine cannot detect a caller handing it a different layout/rate with the same sample count. Compile a replacement for format changes. The host owns source generations, authorization, exclusion provenance, and clearing copies/queues outside this instance. This API does not make those existing integration gates pass.

All coefficient work, IR ownership and state allocations occur in `compile`, off callback. `process`, `reset` and `set_bypassed` use preallocated storage and require exclusive `&mut Chain` access. The config is immutable after compilation; compile replacements off callback, swap at a safe host boundary, and destroy the old allocation off callback. No callback-side serde, file loading, compile, clone, drop, shared mutex or scheduler is supplied. Memory allocation errors during reserve reject clearly.

`process(frames, input)` accepts `1..=max_block_frames` and exactly `frames * channels` finite samples. Effects execute in list order, with independent per-channel state. The returned `BlockReport` describes that block; `output()` borrows exactly the last successful block and `meters()` follows channel order. Smaller later blocks do not expose the earlier suffix. There is no growing queue/backlog.

## Configuration and diagnostics contract

The serde contract uses camelCase fields and kebab-case effect/filter tags, denies unknown fields, and requires explicit `enabled` on each effect. Names are nonblank presentation labels; effect order, not name uniqueness, identifies the processing sequence. Channel identities are preserved exactly and must be nonblank and unique. This is an independent initial config, not a field added to frozen routing schema 2.

```json
{
  "name": "headphones",
  "format": {"sampleRateHz": 96000, "channels": ["R", "L", "aux"]},
  "effects": [
    {"name": "trim", "enabled": true,
     "effect": {"kind": "gain", "gain": 0.8, "invertPolarity": false}},
    {"name": "presence", "enabled": true,
     "effect": {"kind": "biquad", "filter": "peaking",
                "frequencyHz": 3000.0, "q": 0.7071067811865476, "gainDb": 3.0}},
    {"name": "echo", "enabled": false,
     "effect": {"kind": "delay", "frames": 960, "wet": 0.2, "dry": 0.8, "feedback": 0.4}},
    {"name": "dynamics", "enabled": true,
     "effect": {"kind": "compressor", "parameters": {
       "thresholdDb": -18.0, "ratio": 3.0, "kneeDb": 6.0,
       "attackMs": 5.0, "releaseMs": 100.0, "makeupDb": 0.0, "rangeDb": 80.0}}},
    {"name": "IR", "enabled": true,
     "effect": {"kind": "fir", "coefficients": [1.0, 0.0, -0.1]}}
  ]
}
```

`expander` has the same `parameters` structure as `compressor`. Filter tags are `low-pass`, `high-pass`, `band-pass`, `notch`, `peaking`, `low-shelf`, `high-shelf`, `all-pass`. There are no implicit defaults. Even disabled effects' parameters are validated at compile time.

`BlockReport` serializes as `{frames, overFullScaleSamples, bypassed}`. Each channel `Meter` is `{peak, rms, overFullScaleSamples}` for the current block, without smoothing or calibrated loudness. RMS uses scaled sums of squares to handle finite signals even near `f64::MAX` without squaring overflow. `abs(sample) > 1` is an over-full-scale diagnostic, not a mutation or automatic failure. The chain can intentionally produce >1.0; destination encoders must make their own explicit representability/conversion decision.

`Chain::admission()` serializes as `{memoryBytes, workUnitsPerBlock, algorithmicLatencyFrames, explicitDelayFrames, tail}`. Tail is `{kind:"finite", frames:N}` or `{kind:"infinite"}`. These are resource/structural metadata, **not measured speed**:

- `memoryBytes` bounds retained requested payload, including config vector/string capacities, output, state, meters and structs. It excludes caller input, transient compile work and allocator bookkeeping/fragmentation. Arithmetic is checked before state allocation. A caller needing a physical memory cap must account for those external costs separately.
- `workUnitsPerBlock` accounts abstract scalar work for the maximum admitted block, output checks/metering, and worst-case state reset. It is not a calibrated instruction count or CPU cost: transcendental operations and platform math differ. It allows explicit caller admission without declaring a slow algorithm unsupported.
- `algorithmicLatencyFrames` is **added implementation buffering/look-ahead latency**, currently 0. It does not mean an effect has no temporal/phase response. Filters have frequency-dependent group delay; a caller-provided IR may encode delay/phase of any kind. Neither filter/IR group delay nor total route/device/network latency is measured here.
- `explicitDelayFrames` sums configured delay tap lengths, distinct from throughput and added buffering. A dry path can arrive immediately, mixed with delayed sound. This field does not attempt to infer delay encoded in arbitrary FIR coefficients.
- Finite tail length is a conservative zero-input flushing bound after the final input frame, summing FIR and non-feedback delay support. Biquads and nonzero delay feedback conservatively report infinite tails; hosts must choose an explicit normal-render duration/tail tolerance. Dynamics can retain detector state but does not generate sound from zeros. No tail is automatically rendered, truncated, or allowed to survive a privacy reset.

Limits are supplied by a trusted caller, not guessed from source layout or silently increased. Channel count, effect count, requested memory and abstract work excess reject with the named resource, required amount and limit. Checked-size overflow rejects before allocation. There is no fixed product band/channel count. A host may explicitly admit a heavy chain for offline rendering even when its work estimate exceeds that host's chosen live allowance. Estimated or measured CPU cost never silently removes, bypasses, degrades or truncates an effect.

## Implemented effect semantics

- **Gain/polarity:** finite nonnegative linear gain, including boosts and zero, with explicit polarity inversion. Enabled unity gain with no inversion, disabled effects, an empty chain, and whole-chain bypass preserve finite input bits exactly, including signed zero and Float64 low bits. Polarity and non-unity processing are intentional arithmetic.
- **Parametric biquad family:** [W3C/RBJ cookbook](https://www.w3.org/TR/audio-eq-cookbook/) low/high-pass, unity-center-gain band-pass, notch, peaking EQ, low/high shelf and all-pass, in Float64 direct form I with per-channel history. Frequency must be strictly between zero and Nyquist; Q is finite and positive. Shelves use `alpha = sin(w)/(2Q)` rather than a shelf-slope S control. `gainDb` is required to be zero except for peaking/shelves. Gain must be finite within ±6000 dB; computed coefficients must be finite and pass strict second-order stability conditions. Extremely marginal/unrepresentable frequency/Q/gain combinations reject explicitly rather than returning unstable coefficients. All-pass preserves magnitude, not phase or samples. DC removal and graphic EQ can be explicitly composed from high-pass/parametric sections; there is no separate calibrated preset/UI yet.
- **Delay:** positive integer frame delay D. Per channel, `y[n] = dry*x[n] + wet*buffer[n-D]`, and the new retained sample is `x[n] + feedback*buffer[n-D]`. Wet and dry are independently in `[0,1]` and do not have to sum to one. Feedback is finite with `abs(feedback) < 1`, permitting alternating echoes. No fractional delay or hidden cross-channel feedback. Both retained and output results are checked for finiteness, including when wet is zero.
- **Compressor:** independent per-channel peak-amplitude detector; no stereo linking, sidechain, look-ahead or RMS detector. At every sample, the envelope is `a*old + (1-a)*abs(x)`; a rising magnitude selects attack, otherwise release. `a = exp(-1/(rate*time_seconds))`; zero milliseconds is instantaneous. Thus a unit step reaches `1-exp(-1)` in one attack time constant. These times describe the **detector**, not a gain-ramp time. In dB, a hard knee leaves input below threshold unchanged and maps above threshold at slope `1/ratio`. A positive knee width W uses the conventional quadratic transition over threshold ± W/2. Attenuation applies immediately from the current detector; makeup gain follows attenuation. It is not a limiter or a true-peak processor, and may overshoot.
- **Expander/gate:** same detector and time semantics. Above the knee there is no reduction; below a hard knee the output dB slope is `ratio`. Maximum attenuation is `rangeDb`; a positive knee uses a quadratic transition over the same interval. A large finite ratio with finite range gives gate-like attenuation, not an absolute silence latch. There is no hysteresis/hold. Attack opens through the amplitude detector; release closes as the detector falls. `rangeDb` is ignored for compressors, and makeup gain is applied to both kinds.
- **Dynamics domain:** finite ratio ≥1 and nonnegative finite attack/release times; threshold and makeup in ±6000 dB, knee and range in `[0,6000]` dB. These numerical domains cover the Float64 amplitude range without accepting undefined parameter math. Channels are deliberately unlinked. Replacement/control transitions are unsmoothed.

  Attenuation and makeup combine in dB before sample multiplication. If that
  linear gain would be subnormal or zero, processing uses the combined sample
  exponent so an intermediate underflow cannot erase a representable final
  amplitude. Exactly cancelling attenuation/makeup preserves the sample bits.
  True final Float64 underflow still rounds normally; nonfinite overflow rejects
  and resets the entire block.
- **Direct FIR convolution:** nonempty finite coefficients `h[k]`; each channel computes `y[n] = sum(h[k]*x[n-k])` with zero initial history. Coefficients and the ring are allocated at compile time. No normalization, IR truncation, resampling, partitioning or automatic CPU-based refusal. Work scales with frames × channels × taps, including leading/trailing zero taps. Very long impulses remain usable offline within explicit memory/work budgets, even when they cannot sustain live input.

Any effect can increase peaks: EQ boosts/resonance, summed echoes, FIR coefficient sums and dynamics makeup are examples. Existing routing planner peak bounds do not certify the post-DSP signal. Peak/RMS and over-full-scale counters observe actual output; they are neither a conservative post-effect bound nor intersample true-peak measurements. There is no silent headroom adjustment.

## Exact bypass, discontinuities and privacy reset

`set_bypassed(true)` skips effect arithmetic and copies finite input bits exactly while retaining block metering. A bypass change in either direction clears state, meters and the previous output. Repeating the same setting does not reset. Disabled effects are similarly skipped and have no allocated audio state. They can only be changed by compiling a replacement snapshot.

Transitions are immediate and can click. A constant 0.25 signal through gain 2 jumps from 0.5 to 0.25 when bypassed; an output test records this actual discontinuity. This is not a click-free automation claim. Hosts should disclose snapshot reset/discontinuity risk until explicit smoothing/crossfades exist. Do not crossfade revoked old audio into a new authorized generation.

For normal tails, keep processing zeros through the same instance. For source loss, revoke, restart, authorization/layout changes or generation replacement, call `reset()` and clear the caller's corresponding queues/copies. Reset discards every effect's retained audio/detector state and full internal output storage; it publishes an empty `output()` until another block succeeds. It is intentionally different from flushing normal effect tails.

Every invalid block (frame count, length, nonfinite input) and every nonfinite effect output/retained result returns a typed error, resets **all** effect state, clears meters and output, and publishes no partial block. An error after an earlier effect wrote state cannot leave that state live. No old output prefix/suffix or delay/FIR tail becomes available from that instance later. Callback error paths also perform no heap operations, locks or I/O. The caller must report the failure; no silent fallback route is provided.

## Reproducible offline render and timing

```sh
CARGO_BUILD_JOBS=2 cargo test --locked --test dsp_core --test dsp_allocations
CARGO_BUILD_JOBS=2 cargo run --locked --release --example dsp_render -- /tmp/mixyard-dsp-render
```

The example writes little-endian interleaved Float64 PCM files and `timing.json`; there is no audio playback or host-device access. It warms the code path, resets state, then measures each actual `process` call with `Instant`. Output collection, file writing and compilation are excluded. The report includes host/CPU where discoverable (otherwise explicitly unknown), OS/architecture/compiler/build, format, block size, total processing vs represented audio duration, maximum block time/budget ratio and observed deadline misses. Added buffering latency, explicit delay and tail metadata remain separate. It does not measure group delay or total route latency.

There are two deterministic scenarios: one second of a gain/EQ/compressor/echo/short-FIR chain at 48 kHz, and a complete stereo impulse plus **all** 32,767 tail frames for a 32,768-tap direct FIR at 192 kHz. The latter verifies every output sample against the explicit impulse coefficients, regardless of timing. The useful IIR/echo chain renders exactly the selected one-second interval, without claiming that its infinite theoretical tail has ended.

A report is `unmeasured` if no measurable blocks/timer durations exist; absent measurements are never passes. Otherwise it labels observed within-budget blocks or exceeded observed live budget, retaining the chain and complete offline output in both cases. A future UI should start unmeasured, show time **versus that block's frames/rate duration**, misses and context, and explain that sustained slower-than-input processing cannot promise uninterrupted live delivery. These synthetic measurements do not qualify hard realtime scheduling, native callbacks, devices, endurance, physical quality or another host/format/chain. A larger block does not make a sustained throughput deficit disappear.

The `dsp.yml` workflow runs output/allocation tests and the release render on Linux, Windows and macOS, retaining PCM/timing artifacts. A timing miss is evidence to report, not a failed rendering algorithm or permission to omit a feature. Required remote checks and independent exact-HEAD review remain separate merge gates.

## Verification and remaining implementation

Tests use actual output: known quarter-rate Butterworth impulse coefficients; DC/Nyquist shelf/pass gains; steady-tone cutoff, notch, band-pass, peaking and all-pass magnitudes; dynamics static laws, quadratic knee, attack/release time constants and bounded gate attenuation; exact delay/FIR impulse/feedback sequences and block partition continuity; explicit eight-channel isolation; ordered nonlinear effects; Float64 low bits/signed zero bypass; non-whitelisted high/low rates; typed format/parameter/resource rejection and overflow; tail flushing versus privacy/error reset; immediate bypass discontinuity; and heap allocation/reallocation/deallocation instrumentation around the real processing paths. These tests are offline algorithm/lifecycle evidence, not native audio qualification.

Remaining #33 work is explicit: web editor/preview and warnings; native source/mix/per-output attachment and provenance generations; click-free automation/control changes and A/B/presets; channel matrices, balance/width/crossfeed and crossovers; calibrated loudness; de-essing and sidechains; look-ahead/true-peak limiting; advanced/partitioned convolution and reverb tools; plugin discovery/isolation/hosting (CLAP, LV2, VST3, Audio Units) with actual precision/latency/crash compatibility probes. None is advertised as implemented by this slice. There is no simple limiter mislabeled true-peak.

[LV2 core](https://lv2plug.in/ns/lv2core) separates instantiation/audio execution and defines latency reporting; its [worker extension](https://lv2plug.in/ns/ext/worker) separates non-realtime tasks and offline execution. They inform these boundaries; this engine does not load LV2 code. Future hosts must probe native precision: LV2 core audio ports use float arrays, while [CLAP audio-port capabilities](https://github.com/free-audio/clap/blob/main/include/clap/ext/audio-ports.h) make 64-bit support explicit and optional. No plugin may silently narrow an admitted Float64 route.
