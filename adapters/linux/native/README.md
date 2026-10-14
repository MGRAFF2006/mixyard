# Native PRIVATE graph proof — host enablement blocked

A standalone C/libpipewire fixture publishes selectable `Audio/Source` endpoints
and sends real graph PCM to external consuming applications. This is **not a
production adapter**: no `apply`, host integration, production configuration,
Rust planner integration, hardware capture, GUI, default-device fallback, or
WirePlumber library dependency. Runtime source/mix names and counts are arguments;
the harness's two sources and three starter branches are fixtures, not product caps.

The identity qualification deliberately remains non-green. With **injected
registry observation delay**, an unowned replacement link delivers the private
sentinel before the cached DSP gate closes. That safety case **fails**, never an
expected-pass or xfail. It is a simulated notification lag, not a claimed natural
host exploit. Holding the entire control loop can also prevent negotiation; a
silent result in that separate case remains **blocked**, never a safety pass.
Production enablement and native hardware/quality certification remain blocked.

## Reproduce without host audio

From the repository root on a Docker development host with at least 10 GiB free:

```sh
bash verification/linux-private/build.sh
# Detach downloads/builds as mixyard-native-proof-sdk when useful.
tmux new-session -d -s mixyard-native-proof-run \
  'bash verification/linux-private/run.sh verification/linux-private/build/run-2026-10-14 > /tmp/mixyard-native-proof-run.log 2>&1'
tail -f /tmp/mixyard-native-proof-run.log
```

Each run requires a **new** evidence directory. The current qualification exits
**1** when unverified PCM is measured; any measured PCM/clock/probe failure also
fails. Blocked prerequisites and unproved identity cases cannot produce exit 0.
No threshold is relaxed after measurement. Custom names/additional mixes:

```sh
bash verification/linux-private/run.sh verification/linux-private/build/custom \
  --mixes 'Conference mix' 'Session archive α' 'Headphones private' 'Another send'
```

The image pins Debian's base digest, PipeWire/libpipewire **1.6.9-2~bpo13+1** and
WirePlumber **0.5.12-1~bpo13+1**. Package versions and image ID are recorded.
Downloads occur only during image construction. Runtime uses an unprivileged
container, network disabled, two CPUs, 1 GiB RAM, 128 PIDs, read-only root/source,
private tmpfs and one new writable evidence mount. No Docker socket, host runtime,
audio socket, D-Bus, host network or `/dev/snd` is mounted. Private PipeWire,
policy-only WirePlumber, isolated socket/config/state, dummy driver, producers and
consumers run inside it. The harness rejects root, `/dev/snd` and unexpected graph
hardware devices. Do not invoke it against a host graph. Artifacts are ignored
under `build/`; generated sound is synthetic, not microphone recordings.

SDK-only build: `make -j2` in this directory checks libpipewire >=1.6 through
pkg-config. The harness compiles with `-O2 -Wall -Wextra -Werror`. Host package
installation is unnecessary. The inherited analyzers remain unchanged.

## Runtime and evidence

A source starts with an inactive, closed `pw_filter` ingress and two exclusive DSP
input ports. Control resolves a unique fixture name, records observed serials,
binds Node/Port proxies, merges partial info updates, validates serials, node/port
ownership and FL/FR mapping, and creates owned non-lingering links. Link info
readback checks actual endpoints; core barriers precede activation and opening.
Activation processes closed zero blocks before opening; a generation change
invalidates staged data. Removal/error/ambiguity retires the ingress. Explicit
recovery creates a fresh process, connection and closed ingresses; no persisted
raw ID or queued memory is reused across connections.

Only ingress-gated samples reach summing. Each published `pw_stream` has separate
preallocated eight-slot SPSC queues. Stalling one logical reader drops only its
new data and reports loss; siblings remain independently bounded. A complete
coefficient matrix is published through two slots; another revision is refused
until every output callback acknowledges the previous one. Gain is neither
normalized nor clamped by DSP. `set MIX SOURCE GAIN MUTE` supplies configured gain
and effective mute. All callback work uses startup-sized storage, lock-free atomics
and no allocation, locks, logging, file I/O or synchronous control calls.

The scheduling fix uses `PW_STREAM_FLAG_TRIGGER`: ingress callbacks publish a
common ready clock position before triggering each output. Timestamp freshness
also rejects/discards older queue blocks **with explicit loss counters** instead
of accumulating FIFO latency. There is no extra buffer priming, interpolation,
missing-sample replacement, delay search or fitted gain. A late discard or missing
clock quantum during a compared interval fails qualification. The original
independent scheduling mode can be reproduced with `--automatic`; its failures
remain failures. This is shared-dummy-clock scheduling, not clock adaptation or
proof that a stalled real output cannot block PipeWire scheduling.

The first workflow is a simulated mic (600 Hz) plus selected application
(1100/1700 Hz L/R) into independent call, record and headphone branches. Additional
branches use the headphone fixture sends. Private (2300 Hz) and call-return
(2900 Hz) producers stay active but unselected. Two private null sinks fan out the
headphone branch; their monitor consumers and selected exported sources are
externally recorded by `pw-record`. Two-mix runs instead fan out the record branch.
Consumer target moving, default following, reconnection and fallback are disabled.
Mic-path consent is explicitly declared even on the zero-gain headphone path;
a narrow fixture check rejects absent consent at zero and mute. This grants no
real-device access and does not replace production policy validation. Original
playback bypass links are absent by construction; graph snapshots audit the
fixture and owned shutdown cleanup.

An independent native **consuming app** also records every received float sample,
absolute clock frame, monotonic cycle timestamp, clock ID/rate and discontinuity
flags into a bounded capture queue. Only its main loop writes the external file.
The Python oracle computes expected PCM from fixture frequencies, known requested
coefficients and recorded clock frames; it never imports DSP code or fits the
capture. It checks **all samples and tails** through closed→open startup, live
independent gain/mute/unmute edits, and disposable recovery. The declared alignment
is zero frames. Received PCM is compared through the inherited `qualify.compare`
at its one-PCM16-LSB limit, plus float error and clock continuity checks. Direct
producer consumers distinguish source/capture scheduling faults from mixer faults.
Output queue frame/timestamp traces correlate consumer frames to revision
acknowledgments; missing/mismatched frames, clock jumps, trace/capture overflow and
write failure all fail. No backend render is injection evidence. Separate tone
checks validate selected-endpoint transport, stereo mapping, consumer fanout/reopen,
logical stall siblings, exclusive second-link rejection and missing-source silence
with an active replacement sentinel.

`timing-diagnosis.json`, per-packet errors, producer callback deadlines, queue
underrun/discard/stall counters, host load throughout capture, versions, graph
snapshots, SHA256 hashes and full failures are retained. Actual fixture rate is
48 kHz, float stereo DSP / PCM16 WAV, 256-frame quantum. Oversized/mismatched blocks
are gated/dropped. Earlier automatic/trigger-only runs exposed callback-order FIFO
lag and skipped graph quanta, especially under high host load. Those observations
motivated explicit readiness and timestamp freshness; they are not reclassified
as passes. Consult the [observed record](../../../verification/linux-private/observed-2026-10-14.json)
for the exact executed revision and remaining failed/blocked gates.

The latest low-load run still **fails quality**: the direct mic consumer and
headphone consumer each skipped one 256-frame quantum. Both producer callbacks
and all native queues contain those exact positions with valid timestamps. The
loss is downstream of native queue consumption; its precise cause remains
unresolved. Correct received PCM does not replace the missing frames. The
container had no cgroup CPU throttling, 10 ms guest CPU steal across 46.37 s,
`RLIMIT_RTPRIO=0`, and only `SCHED_OTHER` threads (some with RESET_ON_FORK).

The native consuming app copies PCM into a preallocated queue in its RT callback;
only a main-loop timer drains that queue to a file. Its inactive ports are linked
before `start` requests activation. There is no sample-effective start readiness
acknowledgment or always-process property. This consumer boundary remains part of
the failing continuous-capture gate, not an environment or audio certification.
The separate `pw-record` recovery WAV also fails its first analyzed tone window:
mic amplitude falls to 0.06114 versus 0.08 expected, with residual RMS 0.03388.
That WAV has no absolute-frame metadata, so its exact corruption cause remains
unresolved. Passing frame-metadata recovery captures do not override this failure.

## Remaining gates

- **Host identity safety:** public client binding/link creation does not atomically
  compare expected endpoint serials at each admitted sample. Exclusive ports
  prevent simultaneous fan-in, not replacement before observation. The injected
  delay test demonstrates a failure. A stronger server-side identity/link-ownership
  boundary or independently proven pre-sum safeguard is required.
- **Quality scope:** short continuous startup/edit/recovery comparisons cover the
  declared private fixture captures, not every lifecycle transition. Source kill,
  endpoint disappearance, relinking and logical stall have additional graph/tone
  evidence but no complete independent golden capture for every transition sample.
  A rerun with gaps remains failed. No physical latency, independent clocks,
  Bluetooth, suspend, hardware adaptation, endurance or native product-quality
  certification follows. RTKit/realtime privileges are unavailable in this container.
- **Production policy/lifecycle:** Rust v2 execution, general layouts/device
  selectors, server-parameter control/readback, production consent and call-group
  semantics, convergence rejection, dynamic config create/delete/rename, duplicate
  selectors/ID reuse at every bind/link/gate stage, denied/stale controls, abrupt
  crashes, default changes, and PipeWire/WirePlumber restart remain unexecuted or
  unsupported. Fixtures never substitute a broad policy implementation. Real
  mic/application/headphone testing is a later **explicitly operator-armed** gate.
- **Application redirection:** these synthetic producers have no original bypass
  and the harness owns their routes. Arbitrary host app redirection remains unsupported.

The base is independently approved qualification commit
`fa99a9d8fbdb291ea72b7f594ec4e8c5e22c4e78`. No other lane or inherited analyzer/workflow
is edited. API basis: [Filter](https://docs.pipewire.org/group__pw__filter.html),
[Stream / RT-safe triggering](https://docs.pipewire.org/group__pw__stream.html),
[Registry](https://docs.pipewire.org/group__pw__registry.html),
[link factory](https://github.com/PipeWire/pipewire/blob/1.6.9/src/modules/module-link-factory.c),
[exclusive ports](https://github.com/PipeWire/pipewire/blob/1.6.9/src/pipewire/impl-port.c),
[explicit consumer targets](https://github.com/PipeWire/wireplumber/blob/0.5.12/src/scripts/linking/find-defined-target.lua).
