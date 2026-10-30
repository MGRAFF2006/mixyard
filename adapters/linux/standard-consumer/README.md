# Separate standard recording consumer experiment

This is a runnable **test host**, not the Mixyard product's audio backend. The
product CLI/runtime still has no native activation/publication path. The tested
path is generated synthetic program/return/private sources → the actual shared
Rust planner/Processor64/policy/DSP → actual private PipeWire F64 ports → three
separate standard `pw-record` processes → retained IEEE Float32 WAVs.

No microphone, speaker, physical endpoint, default graph, system service or
public port is used. These DC/marker recordings are machine-oracle data, not a
listening demo. Never autoplay them. Policy application labels are synthetic;
neither those labels nor cloud clock observations establish native provenance,
consent or physical clock/latency qualification. All product availability flags
remain false. Both historical ordinary graph cause gaps from #90 stay open.

## Actual 2026-10-30 result

One `small` Boat sandbox, `bx_t32duk6y`, started with `--no-env`, TTL 1800s.
It was explicitly stopped in the host copyout `finally`, with terminal
`state=stopped`, `stop.status=completed`, `forced=false` at
`2026-10-30T11:52:35.174Z`, before automatic stop. No other sandbox was touched,
resumed or created; no TTL extension or paid upgrade occurred.

The SDK is exactly PipeWire 1.4.8 commit
`8b43ed5c910e892d53de5a96aa01e9280a5b6650`, original archive SHA256
`6abd8fe4aaf2f435ee1770fc668acebc2e6b69504600861c794d19b2559ba5db`.
Only Boat installed/build tools and SDK dependencies. Native source/host inputs
were the frozen #90 commit `5b3399a63468bdcd1cb0dbc4b40d0ecc5f14fc24`.
The experiment is an explicit source overlay, not a GitHub test merge. Its
descriptor records Boat/provider/source/toolchain/prefix identities and runtime
binding observations; it never sets `GITHUB_ACTIONS` or claims GitHub attestation.

The build adds `-Dsndfile=enabled -Dpw-cat=enabled` and `libsndfile1-dev` to the
earlier pinned comparison recipe because upstream requires these for standard
`pw-record`. This is a declared experimental SDK build delta, not product SDK
eligibility or a minimum version. The old 1.0.5 failed readbacks remain failed.

Actual simultaneous consumer processes in the final capture were PIDs 34475,
34476 and 34477. Their **native input ports** negotiated F64LE/48000 Hz/two
channels/FL,FR. Each recorder's own client-local adapter explicitly converted to
Float32; actual WAV `fmt` headers establish IEEE Float32, stereo and 48000 Hz.
The file oracle accepts only the actual captured 16-byte format-tag-3 header;
extensible or additional format metadata is unsupported and fails closed. This
does not constrain product audio formats or claim general WAV decoder support.
No F64 low-bit preservation through the recorder is claimed.

| Bus | Recorded steady frame (L,R) | Final capture frames | Steady frames |
| --- | --- | ---: | ---: |
| Call | `(0.25, -0.125)` | 194560 | 55800 |
| Record | `(0.125, 0.015625)` | 194560 | 55800 |
| Monitor | `(0.125, -0.0625)` | 194560 | 55800 |

This final capture spans about 4.05 seconds per recorder, not an endurance test.

Record additionally contains 1800 distinctive return-only frames
`(0.0625,0.046875)` when program is zero. All recorded frames match the declared
Float32 destination oracle: no private `0.875` leakage, and return is absent
from Call under this configured plan. An attempted excluded return send was
not separately tested here. All three files end with at least 1024 actual zero
frames following the graph's acknowledged whole-producer stop/removal. WAV file
positions do not identify native sequence/generation or hardware time. The
oracle's `tailZeroFrames=1024` field is a verified lower bound, not an exact
trailing-zero count; zero-valued marker counts also include startup/tail zeros.

The actual native graph returned stage10/error0/mismatches0, 450 rendered blocks,
delivery counts `[450,450,262]`, zero tracked callback heap operations, and
source-stop-before-destroy/source-removal/owner-retirement/scrub witnesses true.
Five declared-point loaded SDK observations matched the exact prefix for the
daemon, graph and each recorder. They do not prove full-lifetime closure.

**PCM_ORACLE_PASS; aggregate experiment remains FAIL.** All three standard
recorders were alive before our SIGINT request, subsequently returned **1**, and
were reaped. Every daemon/graph/recorder/linker stderr was empty. The precise
upstream 1.4.8 recording implementation returns success only if it reaches its
drained state; its signal handler only quits the main loop. Source:
[pw-cat.c](https://github.com/PipeWire/pipewire/blob/8b43ed5c910e892d53de5a96aa01e9280a5b6650/src/tools/pw-cat.c#L2085).
An explicit owned-interruption classification could require exact version and
source, a live owned PID before SIGINT, signal/reap brackets, exit1, empty
diagnostics, complete final WAV headers and all sample checks. That is only a
reviewable proposal: this code retains failure and does not accept nonzero as
a drained or clean-exit pass. No #90 classifier or qualification gate changed.

Earlier attempt4 also produced three complete files (192000 frames each) and
passed independent sample analysis; its original strict exit1 failure is
preserved. Attempt1 failed before PCM because full `pw-dump` queried unsupported
`ProcessLatency` on the new standard adapter nodes (ENOENT, 576 stderr bytes).
Attempt2 failed observer strict compilation on signedness; attempt3 rejected
incomplete link identity serialization. All are retained as failures.

The dedicated test observer queries only the public registry, exact identities
and readable **Format** parameter. It never queries ProcessLatency or accepts
its stderr. The new experiment requires exactly seven owned nodes/nine links,
the same six server modules and zero device/recapture/unowned nodes; this does
not relax the old four-node qualifier. The standard clients' adapter module is
local to those clients. Actual negotiated formats are checked before admission.

The actual product `mixyard-runtime serve`, `mixyard status` and owned shutdown
were also built/run from 5b in Boat. Status reports reachable daemon but
`backendAvailable=false`, `apply=false`, `mute=false`,
`nativeActionDispatch=false`. Runtime and shutdown exited0, stderr empty, all
owned processes reaped. The next product gap is a canonical control-owned
native host lifecycle plus discoverable supported outputs/ordinary client
attachment; this test controller does not implement that product service.

## Reproduction boundary

An authorized owner first checks Boat budget/status/limits and creates one
small, no-env, automatically stopped sandbox. This script does not create or
extend sandboxes. The tested source layout is an exact clean 5b checkout plus
only this reviewed `standard-consumer` directory as an overlay. The script
intentionally refuses other native source HEADs, so do not pretend a later
whole checkout was executed.

Inside that owned sandbox (never the implementation host): install the scoped
Meson/Ninja/gettext/json-c/pkg-config/libsndfile development tools, then:

```sh
python3 -B adapters/linux/standard-consumer/boat_recording.py prepare --sandbox-id EXPLICIT_ID
python3 -B adapters/linux/standard-consumer/boat_recording.py run --sandbox-id EXPLICIT_ID --attempt 1
```

The first command builds the pinned SDK and canonical host in
`/home/user/mixyard-run`, with j2 SDK/j1 Rust and a forecasted cloud disk reserve.
The observed SDK build was bootstrapped with those operations; the standalone
`prepare` subcommand was not independently rerun. Existing actual `run` attempts
used the retained controller source SHA; publication additionally fixed only a
pure WAV parser whole-file alignment assumption and added bounded reads, tested
against all six retained recordings and generated malformed files. The subsequent
review repair also validates the complete RIFF extent and chunk padding, and
reuses `native-host/process_capture.py` for live bounded runtime diagnostics
(64 KiB graph stdout, 16 KiB other stdout/stderr, plus one overflow witness byte).
An overflowing owned child is stopped; only bounded buffers are persisted after
the readers finish. This replaces the former ordinary-file logging and unbounded
post-run reads. Generated malformed-container and owned Python flood checks cover
these repairs; neither the repaired controller nor its pipe wiring has been run
with the SDK. Historical executed hashes and results remain unchanged. No further
cloud recording was run just to turn a failure green.

Always copy bounded selected recordings/evidence out and stop the explicit
sandbox in an outer `finally`. The run command stops/joins only its own native
processes; it does not manage cloud lifetime. It deliberately exits nonzero for
the recorded undrained recorder outcomes.

Raw recordings and full receipts were copied to the owner's private
`/dev/shm/mixyard-standard-consumer-bx_t32duk6y` and all 85 manifest-listed files hash-verified (86 regular archive members
including the manifest itself) before cloud stop. Six WAVs plus manifests/logs/receipts occupy roughly
10.14 MB, under the 16 MiB evidence cap. `evidence-2026-10-30.json` retains exact
hashes and results. SHM is volatile and Boat stopped snapshots have finite
retention; neither is an indefinite artifact guarantee. No SDK binaries, cloud
trees, credentials or large PCM blobs are committed.
