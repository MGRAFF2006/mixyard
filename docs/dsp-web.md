# Offline DSP workbench contract

The Processing workspace edits and previews actual Float64 DSP chains, separately
from the frozen schema-2 routing/profile format. It does not attach effects to
native audio or claim live delivery. Chain JSON can be imported/exported; presets
and edits remain browser state until exported. No audio plays automatically.

The existing protected, same-origin API adds `POST /api/dsp/preview` with the
normal session token. Request:

```json
{
  "config": {"name":"My chain","format":{"sampleRateHz":48000,"channels":["FL","FR"]},"effects":[]},
  "frames":128,
  "iterations":16,
  "bypassed":false,
  "signal":{"kind":"impulse","amplitude":0.25}
}
```

`config` is exactly the serde `dsp::ChainConfig` in [dsp.md](dsp.md), including
all implemented effect/filter parameters. Signals are `impulse` or `constant`
with `amplitude`, `sine` with `amplitude` and `frequencyHz`, or `samples` with
explicit interleaved `samples: number[]`. All scalar samples/parameters must be
finite. Preview is explicit; a normal editor action never triggers rendering.

Import validation uses `POST /api/dsp/validate` with `{config,frames:1}`. It
compiles without processing samples and returns `{valid:true,precision:"float64",
admission:Admission}`. Import validation creates no timing report.

Response:

```json
{
  "mode":"offline-dsp-preview","precision":"float64",
  "frames":128,"sampleRateHz":48000,"channels":["FL","FR"],
  "output":[],
  "report":{"frames":128,"overFullScaleSamples":0,"bypassed":false},
  "meters":[{"peak":0.25,"rms":0.022,"overFullScaleSamples":0}],
  "admission":{"memoryBytes":1234,"workUnitsPerBlock":1000,"algorithmicLatencyFrames":0,"explicitDelayFrames":0,"tail":{"kind":"finite","frames":0}},
  "timing":{"measured":true,"iterations":16,"meanMs":0.01,"maxMs":0.02,"blockBudgetMs":2.6667,"deadlineMisses":0,"overBudget":false,"build":"debug","os":"linux","arch":"x86_64"}
}
```

The example numbers above describe response shape only, never a measurement.
The actual endpoint returns complete output for the requested block, one meter
per channel, and measured processing time. Timing excludes compilation, input
generation, state reset, output copying and JSON/network overhead; each repetition
starts reset. Output/meters/report are from the first isolated render, so repeated
benchmarking cannot silently advance the displayed signal through a delay tail.
`measured:false` uses null durations and no within-budget claim when elapsed time
cannot be measured. Memory/work estimates are not speed measurements.

A chain that exceeds the measured block budget stays editable and renderable.
Show a clear warning about unreliable live speed and the availability of offline
rendering, alongside format, block size, build/OS/architecture and actual misses.
Passing this synthetic benchmark does not qualify native realtime operation.
Added buffering/look-ahead latency, intentional delay/tails and filter/IR group
delay are distinct. No silent bypass, downmix, narrowing, gain fitting or limiting.

The web preview has bounded frame/channel/effect/IR/work/memory/repetition budgets
and the service's existing request/deadline/concurrency limits. Rejections return
the existing structured error shape with an explanation; larger full offline
rendering remains available through the DSP engine/example. These are endpoint
resource limits, not product limits or grounds to omit slow effects.

The interface follows the existing console style: ordered effect list with
add/remove/reorder/enable controls, type-specific parameters and named presets;
explicit sample rate and ordered channel identities; separate block/signal and
benchmark controls; output/meters with over-full-scale warnings; unknown timing
until a fresh preview. Any edit invalidates the old preview. Exact whole-chain
bypass is an explicit switch. Snapshot changes can click; no smoothing is claimed.
