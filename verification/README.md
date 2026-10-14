# Qualification (offline tools, native evidence kept separate)

Python 3.10+ standard library only. These tools neither open devices nor change
routes, defaults, permissions, drivers or services. Run from the repository root:

```sh
python3 -B verification/selfcheck.py
python3 -B verification/policy_check.py target/debug/mixyard
python3 -B verification/qualify.py tones /tmp/mixyard-tones --seconds 3
python3 -B verification/qualify.py analyze /path/to/app-call.wav --scenario baseline --bus call --offset 1 --duration 2
python3 -B verification/qualify.py analyze /path/to/app-record.wav --scenario baseline --bus record --offset 1 --duration 2
python3 -B verification/qualify.py analyze /path/to/wired-monitor.wav --scenario baseline --bus monitor --offset 1 --duration 2
```

Mixes are arbitrary user-named stable-ID objects, with **no fixed count or name
enum**. Call/Record/Monitor commands below are retained starter audio fixtures,
not a v1 core-config contract. Dynamic capture expectations deliberately have
their own small format, independent of the core's schema 2:

```sh
python3 -B verification/qualify.py analyze /path/to/headphones-a.wav --expectations verification/dynamic-audio.json --output headphones-a --duration 2
python3 -B verification/qualify.py analyze /path/to/speakers-b.wav --expectations verification/dynamic-audio.json --output speakers-b --duration 2
python3 -B verification/qualify.py analyze-set verification/dynamic-audio.json /path/to/captures.json
```

`dynamic-audio.json` has four arbitrary names (including Unicode), six physical/
virtual output edges, and one mix fanned out three ways: unity headphones, .5-gain
speakers, and a muted virtual copy. `tone_peaks` is an independent expected linear
amplitude map **per output channel**, not inferred from captured data or imported
from implementation code. Add arbitrary IDs/mixes/outputs without editing the
analyzer. Omitted known tones mean zero; unknown/duplicate output IDs, unsupported
layouts, invalid/nonfinite peaks and missing outputs block explicitly. Physical
mic routing still requires separate native consent regardless of the mix label;
this synthetic expectation file grants no consent or device access.

For `analyze-set`, `captures.json` maps every output ID to its externally recorded
file and optional steady interval, for example
`{"headphones-a":{"path":"a.wav","offset":1,"duration":2}, ...}` (replace
the ellipsis with all six records). Relative paths resolve beside the capture map.
The tool reports every requested output, marks a missing capture blocked, fails
wrong gain/mute/leakage per device, and continues checking siblings. An unknown
capture ID blocks instead of choosing a replacement. No silent truncation or
aggregate fanout pass can hide an untested device. Clocks/gaps/xruns/latency and
failure isolation require separate native evidence **for each** output.

`media-control-scenarios.json` specifies device-scoped resolver/control cases:
Spotify on A versus VLC on B; explicit primary versus all-routed groups; configured
sessions after Pause removes their stream; fanout session dedup; revision-bound
plans; explicit group Pause/Play with a remembered, revalidated resume subset;
partial outcomes; no fallback on absent primary/unknown origin; OS duplicate
events versus distinct intentional presses; no blind Next/Previous retry. Player
Pause affects that global player on every routed device; device-only silence is
output mute and must leave siblings untouched. OS-default mode preserves the
OS-selected player; scoped mode uses explicit configured targets. Native scoped
controls require **both** origin-device identity proof and original OS dispatch
suppression/ownership proof. A synthetic resolver result proves neither.

The first command is **synthetic analyzer self-check evidence only**. It generates
expected PCM in a temporary directory, checks the oracle and deliberately damaged
files, then removes them. It does not test Mixyard, native routing, policy CLI,
microphone capture or virtual-input transport. It needs no Rust/backend package.
`policy-scenarios.json` lists independent semantic cases for the policy owner;
these scenarios are not audio evidence. `policy_check.py` runs a focused schema-2
black-box subset against an **already built** CLI using public config/inventory
fixtures and temporary mutations, with independent expected invariants. It checks
the public `plan-media CONFIG INVENTORY REQUEST` invocation without dispatch,
all requested mix/output IDs, four-plus arbitrary mixes, rename/duplicate labels,
create/delete, duplicate-ID rejection and superseded-version rejection. It never
builds/imports implementation code and never falls back to the old fixed-three
schema. Exit 0 means those model checks pass, 1 means contract failures, 2 means
blocked executable/environment/absent schema-2 fixtures. Media resolver and native
lifecycle cases beyond this command smoke check remain separately unexecuted;
native prerequisites are required. A schema-2 plan pass never proves native controls.

`tones` writes only the four **source** fixtures. For a routing test, send these
through separately identified source producers in a disposable test graph, select
the actual virtual input in each consumer app, and export each app's received
uncompressed PCM. A backend render buffer/file does not prove that a consumer
received a virtual microphone. Never analyze the stimulus or a modeled mix and
call that a routing pass. Playback capture and virtual-input injection are separate
native gates on Windows/macOS.

Sources, peak amplitude 0.08 per channel at 48 kHz PCM16:

| Source | Channels / tones |
| --- | --- |
| mic | mono 600 Hz |
| media | stereo: L 1100 Hz, R 1700 Hz |
| private | mono 2300 Hz, unselected in these fixtures |
| callreturn | mono 2900 Hz, excluded from the starter Call mix |

Keep all four sources active to expose exclusions. Calibrate the reference source
level, record the reference capture and fix playback/app gain; do not assume an
acoustic physical mic reproduces peak 0.08. Disable app processing where possible,
and record the actual AGC/AEC/noise settings. If exact level or raw PCM is
unavailable, the gain gate is blocked, not relaxed into a pass.

Every starter bus master is 1. The following scenarios are independent expected contracts,
not generated policy configs. Any omitted send is absent/zero. Muting a send keeps
its configured gain for unmute.

| `--scenario` | Call mono | Record stereo | Monitor stereo |
| --- | --- | --- | --- |
| `baseline` | mic 1 | mic .5 + media .25 + callreturn .25 | media 1 |
| `call-gain` | mic .25 | baseline | baseline |
| `record-mic-muted` | baseline | media .25 + callreturn .25; mic muted | baseline |
| `mic-missing` | silence | media .25 + callreturn .25 | baseline |
| `shared-media` | mic .5 + media .5 | baseline | baseline |
| `silence` | silence | silence | silence |

Record all buses simultaneously for each stage, then analyze every bus for that
scenario. Changing Call alone must not affect Record or Monitor; changing Record
mute must not affect the other sends. Unmute restores baseline. In `mic-missing`,
keep another mic/private source active to expose forbidden replacement. Analyze
before, during and after loss. A single bus file cannot establish independence.

Mono duplicates into L/R; stereo averages `(L+R)/2` into Call. Thus baseline Record
mic peaks at .04 on both channels, with media .02 on its respective channel and
callreturn .02 on both. Shared-media Call has .04 mic plus .02 for **each** media
tone. Media channels are distinct so channel swaps and incorrect folding fail.
All modeled sends satisfy the conservative peak bound in `docs/PLAN.md`.

The analyzer accepts mono/stereo custom outputs, or mono Call and stereo
Record/Monitor starter fixtures, in signed integer PCM WAV
(16/24/32 bit; no float, compressed or multichannel input), 8–192 kHz with rate
divisible by 10. It never silently downmixes. Select at least 0.5 seconds in whole
100 ms windows. An arbitrary-length recording requires an explicit duration;
unqualified short tails cause a blocked result. Offset and duration intentionally
select steady intervals: record excluded transitions separately and assess them
for transient leaks. No automatic delay alignment or gain normalization occurs.

Each 100 ms window/channel measures phase-independent peaks at **all five** known
frequencies, residual RMS for unknown content, DC, absolute peak and silence RMS.
Wrong gain, missing tones, forbidden private/callreturn leakage, channel swaps,
dropout, impulses and unknown tones have negative regression checks. Defaults are
linear full scale, not dB:

| Check | Default |
| --- | --- |
| expected tone amplitude | absolute error .002 + 3% of expected peak |
| forbidden tone peak | <= .001 (about -60 dBFS) |
| unknown-content residual RMS | <= .002 |
| silent-channel total RMS | <= .0005 |
| absolute DC | <= .0005 |
| any sample absolute peak | < .98 |

CLI flags `--amplitude-absolute`, `--amplitude-relative`, `--leak-peak`,
`--residual-rms`, `--silence-rms`, `--dc`, `--peak` override defaults for a
**predeclared** hardware calibration budget. Results include every effective
tolerance. Do not loosen them after observing a failure. Membership is bounded by
these floors and 100 ms resolution; a pass does not promise zero leakage or prove
timing/phase, subwindow transitions, physical clock adaptation or latency.
Clock error/resampling may itself fail the coherent-tone oracle; investigate and
record that failure rather than calling drift qualification complete. Streaming
analysis uses bounded memory; long captures may take time, so run them in tmux.

The analyzer prints JSON and exits 0 for PCM checks passed, 1 for measured failure,
2 for missing/unsupported prerequisites or invalid input. A missing file, corrupt
header, truncated data, unsupported sample format or short interval is **blocked**.
`native_routing_verified` remains false: the tool cannot know how a file was made.
Only the native scenario record plus externally captured artifacts can establish
a routing result. No native evidence is generated by the self-check or hosted CI.

For the stricter **transparent wired unity/bypass** gate, use an independent
reference and externally recorded consumer WAV with the same negotiated rate and
channel layout. Declare alignment as integer frames; this check never fits gain,
searches delay, resamples or drops data silently:

```sh
python3 -B verification/qualify.py reference /tmp/mixyard-reference.wav --seconds 3 --channels 2 --seed 42
python3 -B verification/qualify.py compare /path/to/reference.wav /path/to/consumer.wav --reference-offset 0 --capture-offset 480 --frames 96000
```

`reference` generates a seeded, channel-distinct, nonrepeating PCM source bounded
to peak .08. Send it through the actual route and compare consumer PCM against the
original WAV. Use different seeds for mic/media/private/callreturn when testing
crossmix exclusion. For non-unity mixes, provide an independently computed golden
reference with the declared sends/channel mapping; never fit it to captured data.
Freeze conversion/quantization tolerances up front. Sample-by-sample comparison
checks brief corruption, dropped/repeated samples and brief crossmix leakage that
the 100 ms tone oracle could miss; self-checks deliberately inject each defect.
Same-format comparison allows no gain/rate fitting. Analog paths, asynchronous
clocks and negotiated conversion need a separately declared reference/error model
and native evidence; they cannot inherit a bit-transparent pass.

Every compared sample must be within one normalized LSB of the coarser PCM16/24/32
format; any sample error beyond that or peak >= .98 fails. It also reports maximum
and RMS errors. Missing/unequal frames or mismatched rates/layouts block unless an
explicit matching interval is selected. Known conversions require their own
quality evidence. A comparison pass establishes neither native provenance nor
dropout/xrun absence outside the selected interval.

`quality-release-matrix.json` defines proposed per-OS/version/device/driver/app/
format/USB/Bluetooth-profile gates, all **blocked and not executed**. Its proposed
endurance target is **eight hours** with independent clocks and simultaneous
call+record+monitor, continuous capture, USB/profile/suspend transitions, measured
drift/queue trends and end-to-end latency. Budgets must be declared before a run.
No clipping/unexpected processing/unexplained gaps are allowed in the declared
workload. Missing xrun, dropped-frame or queue-overflow telemetry is **unknown,
never zero**. A probe overflow/write failure must fail or explicitly degrade the
run even if WAV tone checks pass. This tool cannot infer capture metadata from PCM.
The matrix separately tests BT play/pause/volume and mute/call controls where
supported, preserving OS player/session ownership; it also requires clear presets/
selectors and actionable degraded reasons without a mandatory graph editor.
Control interception, UI and eight-hour native runs are outside this batch.
The current proposed sequence is two-hour stress before eight-hour endurance;
proposed wired Monitor targets are p95 <=20 ms and p99 <=30 ms, **not measured**.
`interface-scenarios.json` records later per-platform app/tray/menu-bar and headless
CLI gates, including GUI close/reopen without sample interruption, consistent
concurrent tray/CLI state, separate configured/connected/audio-verified status,
and no global microphone-muted indicator while Record remains unmuted.

Copy `native-evidence.template.json` to an ignored `verification/local-evidence/`
directory per OS/device/app run. Fill exact OS build, hardware, stable device UIDs,
actual format/clock domains, consumer version/selected endpoint, commands,
timestamps, file hashes, graph snapshots and results. Each scenario starts
`blocked`. Use `skipped` with the missing prerequisite when intentionally not run;
only mark a scenario `pass` after its stated observations and linked artifacts
exist. A PCM pass cannot automatically turn a scenario into pass. Do not commit
microphone recordings, unrelated app data or machine credentials.

The template covers two endpoints/multiple consumers/reopen; simultaneous 1/.5/0
mic sends; mix-minus/private exclusion; mono/stereo and unsupported multichannel;
producer and controller crash **during** mutation; default changes; exact-source
loss to silence without replacement; unrelated links/owned rollback/release;
isolated WirePlumber recovery; denied/revoked permission; endpoint-loss app
fallback; independent-clock endurance with telemetry and declared budgets.
Default changes, service/driver interruption and device permissions require a
dedicated authorized test environment; this batch runs none of them.

Hosted workflows run offline checks and compile/test known build targets only.
Absent build definitions/dependencies and absent native hardware remain explicit
blocked/skipped gates. Hosted runners never certify native audio or signing,
Secure Boot, virtual-driver injection, app compatibility, or latency. No
self-hosted/homelab runner is used. The workflow runs only after a future push/PR
or manual dispatch; this batch does not trigger it.
