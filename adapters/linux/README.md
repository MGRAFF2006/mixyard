# Linux PipeWire prototype

## Owned in-process Rust PCM path

`src/runtime/linux_audio.rs` provides a callable Linux host bridge: an already
compiled authoritative `Processor64`, exact source/destination bindings, aligned
in-process PCM, independent bounded output queues, and owned null/raw-file
consumers. This path executes shared policy and actual PCM; it does not connect
the Python graph planner to an audio server. The lifecycle IPC and inventory
`backendAvailable`/`audioApplied` flags remain false.

Construct `LinuxAudio::new` off callback with one optional `Endpoint` per immutable
processor source/output slot. Each selected endpoint must match the exact planned
observed ID, nominal rate, Float64 precision and canonical ordered `MONO` or
`FL,FR` channels. Missing/ambiguous ports require `None`; formats/layouts outside
the current core are rejected. The constructor admits the **total** output PCM
queue budget; processor buffers, endpoint metadata and consumer scratch are
separate explicit budgets.

Call `render` with caller-owned PCM and matching `Input` metadata. All supplied
sources must share the explicit engine clock ID, generation/revision, sequence
and first frame, with current monotonic capture/deadline timestamps. Output
deadlines cannot extend a contributing source's lifetime. Identity, format,
stamp or sample errors revoke the instance and erase queued PCM. Missing input
is fresh silence. A full or expired output queue fails only that destination.
Revocation is terminal; restart needs a new host generation, fresh binding and
new consumers. Numeric stamps are lifetime checks, not capture authorization.

Create one `Consumer::null` or `Consumer::create_file` per selected output off
callback, then drain each independently outside render. Exclusive consumer claims
reject duplicate attachment instead of splitting a stream.
Additional consumers need separate explicitly planned output routes/queues.
The queues use exclusive ownership and are **not** cross-thread SPSC queues; native callback scheduling
and lock-free interthread ownership transport remain unqualified. A file drain
can block its calling thread, so it must never run on a realtime callback.
Files are exclusively created with mode 0600 and never reopened/truncated;
callers choose a trusted owned parent directory. They contain headerless
interleaved PCM, with the admitted format retained on `Consumer::format`.
Each destination explicitly selects Float64LE, rounded Float32LE, or an exact
signed codec. No rounding/clipping/dither is implicit for signed PCM. A conversion,
frame-budget or I/O failure stops that consumer and clears its queue; sibling
output precision/queues are unchanged. Partial I/O failure may leave an incomplete
file and is never retried automatically. Total recorded frames and scratch bytes
are admitted before file creation.

Run the focused production-path tests after integrating the worker/module exports:

```sh
CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 cargo test --locked --test native_linux_pcm -- --nocapture
```

For constrained hosts, `python3 -B adapters/linux/check-owned-pcm.py --help`
describes a `rustc` harness using existing compatible dependency artifacts read
only. It compiles the unchanged full root library and these tests, checks
resource floors and deletes its private `/dev/shm` artifacts. This verifies the
actual library module graph; Cargo manifests, bins and native SDKs are not checked.

The executable checks compare generated-source consumer file bytes (including
Float64 low bits, signed zero, subnormals, independent mixes and an explicitly
narrowed sibling), queue saturation/null progress, stale/revoked/restarted
bindings, no-overwrite and exact-codec rejection. Allocation instrumentation
covers render, queue handoff and null drain; file I/O is outside that scope.

On 2026-10-29 this implementation host had no `libpipewire-0.3` pkg-config SDK,
PipeWire runtime library in `ldconfig`, `pipewire` or `pw-cat`. No installation,
server, native graph, personal source, physical output or desktop session was
used. Native PipeWire transport/private-server isolation, selectable virtual
inputs, hardware clocks, drift, latency and physical/endurance qualification
remain open. Generated in-process delivery does not establish those gates.

## Existing Python graph prototype

The Python PipeWire prototype is independent of the Rust policy CLI. `example.json` is an adapter
sample, not the shared core config. There is no integration yet. Python 3.10+
stdlib plans native PipeWire filter-chain streams; Python never sees PCM.
External graph mutation is disabled. Discovery, health observation and offline
command/graph modeling remain available. There is no custom
realtime engine, sample FIFO, media player, recorder or Bluetooth controller.
The product contract is **user-defined mixes and device fanout**, without a
fixed mix-count limit. Call/Record/Monitor are sample templates only. The current
three-template proof is offline; the number of native audio-qualified mixes is
**zero**. Custom mix counts and fanout are model/command-plan evidence only.

## Commands

From the repository root:

```sh
python3 -B adapters/linux/mixyard_linux.py snapshot
python3 -B -m unittest discover -s adapters/linux -p test_linux.py -v
```

`snapshot` writes exactly the backend-contract v1 envelope to stdout, exits
nonzero on unavailable/denied discovery, and omits process command lines. It
enumerates native Audio/Source and Audio/Sink nodes using semantic `node.name`
identities, never transient IDs. Duplicate names require a more specific selector.
Only discovery is advertised; transport and recovery capabilities remain false
pending native qualification. A third-party source does not establish transport.

Before discovery or offline planning consumes a dump, it validates the entire
observation: object/type structure, unique unsigned 32-bit native IDs, Node/Device/
Port info and property objects, and the identity, state, media class, ID reference,
channel/direction and boolean fields those commands use. Missing optional properties
stay absent; unknown interfaces and unconsumed forward fields are ignored safely.
Malformed consumed fields or invalid dump JSON return `malformed-observation` in
versioned JSON with exit 1. No partial device inventory or route/producer plan is
retained, and discovery capabilities remain unavailable. Unsupported semantic
identity encodings retain their separate diagnostic below.

`health --remote NAME --iterations 3` optionally observes native `pw-top` ERR
counters for Mixyard nodes, omitting unrelated application rows. ERR combines
xruns and errors within that profiler's observation window. Missing counters
exit nonzero; even an observed zero never proves zero PCM gaps or past dropouts.
This command is read-only, and needs `pw-top` and the server profiler facility.
Malformed owned rows or ERR counters reject the entire health sample with a
versioned diagnostic and no retained owned rows/counters; `-` remains unknown.
Other applications' rows are ignored rather than exposed.

Save `pw-dump` JSON on a test server, then inspect an offline plan:

```sh
python3 -B adapters/linux/mixyard_linux.py plan adapters/linux/example.json --dump /path/to/dump.json
```

`plan` emits native client configuration and semantic routes. It is command-plan
evidence, not measured sound. Missing/ambiguous sources and mismatched port maps
produce disconnected-route diagnostics without selecting any default.

`apply` is unavailable and exits nonzero before reading config, discovering a
server, acquiring locks or starting clients. This includes disposable servers,
armed outputs and repeated invocations; there is no unsafe override.

The removed CLI controller could connect replacement audio after numeric
node/port ID reuse between observation and link creation. Post-link readback
cannot retract leaked audio. It also treated command exit zero as native
control acknowledgment, which `pw-cli` does not guarantee. Consent does not
solve either problem. A future backend must gate audio until identities are
verified and native controls acknowledged, before mutation can return.

To inspect a physical-output path offline, enable its explicit output in the
config and run `plan --arm-outputs`. Microphone paths stay disconnected and
`requiredMicConsents` lists each source/mix/output path, its selectors and token.
After reviewing those paths and the config, repeat the plan with one
`--consent-mic-path TOKEN` per required path. Every microphone feeding an output
must have consent before that output appears in the route plan. These tokens
acknowledge only the exact offline path; they never enable native mutation.

## Routing and limits

These describe the proposed offline graph; no endpoints or routes are published.

- `mixes[]` declares arbitrary unique `id`, display `name`, `channels`, `master`,
  `virtualInput`, explicit `excludeRoles`, and optional `excludeSources`.
  IDs determine stable names `mixyard.<instance>.mix.<id>.out`; display names do
  not determine policy. `virtualInput=true` plans a selectable Audio/Source;
  false plans an internal output stream. Multiple consumers are intended to use
  a published mix. The sample plans mono Call and stereo Record, with internal Monitor.
- `sources[].gains` maps mix IDs to independent sends; omitted sends mean zero.
  Each mix has separate native capture ports and per-source copy/mixer filter
  nodes. Gains live in that mix's builtin mixer controls, never source volume.
  The sample represents simultaneous mic gains 1.0 / 0.5 / 0.0.
- `outputs[]` is separate from mixes: unique route `id`, `mix` reference, exact
  sink `selector`, `channels`, independent `gain` in [0,1], and `enabled`.
  A mix can feed several outputs/devices. Every route has a separate native
  output filter/stream named `mixyard.<instance>.output.<id>.out`; changing one
  output gain does not change the mix or other outputs. Hardware routes require
  `enabled=true`, `--arm-outputs`, and explicit consent for every microphone path
  feeding that output. Tokens bind the instance, source, mix and output config,
  including selectors, gains, master, mute and channels; edits require renewed
  consent. Muted/zero-master paths with configured positive sends still require
  consent. This deliberately also invalidates consent on unrelated fields within
  those objects. The sample's headphone route is off.
  No output is inferred from system defaults. Separate device clocks and rate
  adaptation are delegated to PipeWire, with native qualification still pending.
  Sample alignment between devices, especially Bluetooth, is not promised.
- MONO duplicates to stereo; stereo averages L/R into any mono mix/output. Only
  MONO and FL/FR are accepted; observed port maps must match. Native eight-input
  builtin mixers are cascaded, rather than imposing an eight-source product
  limit. This prototype accepts 64 configured source channels per native capture
  stream, with no arbitrary mix/output count cap. Native resources still limit
  practical counts. Sample rate is left to native negotiation: no forced 48 kHz,
  quantum or latency setting. Native float processing and necessary native rate
  conversion remain.
- Sends must be finite/nonnegative; masters are in [0,1]. Every mix must satisfy
  `master * sum(unmuted gains) <= 1`, including absent sources. Lower the master
  explicitly to reserve headroom. Multiple enabled outputs into the same
  configured or resolved sink are rejected regardless of headroom, mute, gain,
  arming or consent. Nothing silently normalizes other sends.
- Offline gain/mute edits change only affected mix/output coefficient plans and
  retain the planned topology. There is no native gain/mute application, live
  config reload, endpoint publication or reconciliation in this version.
- Exact `node.name` plus optional `device.serial` / `application.id` selectors
  are supported. Serial is resolved through the parent Device when needed.
  `pw-dump` string properties remain exact; signed 64-bit JSON integers use their
  canonical decimal string for discovery and matching. Configured selectors stay
  strings: `"00123"` never matches observed integer `123`. Original numeric spelling
  is unavailable after `pw-dump` conversion; duplicate canonical identities remain
  ambiguous. Boolean, null, empty, container, out-of-range integer and floating-point
  identities are rejected explicitly; `pw-dump` float conversion can lose precision.
  `snapshot` and `plan` report unsupported encodings in versioned JSON with exit 1,
  without publishing partial discovery or a route plan.
  Names may change with device profiles/drivers or app relaunch. IDs/PIDs are
  never persisted. Ambiguity is disconnected. No app grouping/tab isolation.
- Mixyard nodes of any instance are forbidden as external sources. Excluded
  roles/source IDs must have zero destination send even while muted. The Call
  template excludes `call-return`; custom communication mixes declare their own
  exclusions. Renaming does not weaken exclusions. Routing is external source
  -> mix -> output device, without mix-to-mix edges. `role` is trusted config:
  this cannot infer hidden in-app paths or acoustic feedback and is not general
  multi-call mix-minus. Listening routes require deliberate devices.
- `bluez5.loopback=true` placeholders stay disconnected: linking them can initiate
  headset-profile switching. Select an already active supported source instead.
  No profile/codec, BlueZ, MPRIS, OS media/session or device-button policy changes.
  Media event attribution is a later gate: device origin must map to that
  device's selected primary application session or explicitly selected all-routed
  sessions. Unknown origin must never fall back to global/all players. No
  generic Bluetooth interception or dummy player is implemented.

## Ownership and failure behavior

Planned producer nodes carry `mixyard.owner=<instance>` and disable autoconnect,
fallback and movement. These are proposed native properties, not evidence of
native isolation. No graph mutation or reconciliation code remains: `apply`
always fails, so malformed edits, rejected topology, server errors, ID reuse,
control rejection or repeated disappearance cannot leave this adapter's routes
active. There are no active native controls to acknowledge or link repair
counters to reset. External applications and defaults are never changed.

Before restoring a controller, require fresh observations and last-accepted-config
safety reconciliation even when edited config is rejected. Require a lifetime
per-identity creation/repair budget across temporary disappearance, identity-safe
gated link creation, acknowledged controls with fail-closed handling of partial
updates, and deterministic cleanup during mutation. Do not restore the retired
CLI implementation behind an opt-in flag. Downstream application fallback remains
a separate native qualification gate.

A retained Linux process-lifetime helper is tested using an ordinary sleeping
child. Atomic PID publication and a bounded handshake expose no partial PID;
parent SIGKILL terminates the child. This utility test does not start PipeWire,
prove native graph cleanup or provide an alternate application path.

## Evidence and native prerequisites

On **2026-10-14, homelab-dev**, 76 GiB was free. `python3` and `tmux` were present.
`pipewire`, `pw-cli`, `pw-dump`, `pw-cat`, `pw-loopback`, `pw-link`, `pw-top`, `pw-record`, and
`wireplumber` were absent; no PipeWire installation was found under `/usr`.
Nothing was installed and no live graph was changed.

Offline tests interpret emitted native mixer coefficients: scalar 0.2 produces
Call 0.2, Record [0.1,0.1], Monitor [0,0]. They check channel rules, headroom,
feedback rejection, selectors, path-specific microphone consent and invalidation,
configured/resolved sink convergence rejection, custom IDs/names, twelve configured
mixes, sixteen-source mixer cascades, independent two-device output gain planning,
stable control planning, unconditional mutation rejection before external access,
and actual child termination after parent SIGKILL. **None of this proves native PCM, dropout-free audio or sound quality.**
Actual CLI regressions use isolated fake `pw-dump`/`pw-top` executables to check
malformed mixed observations, failure envelopes, exact identities and unknown
forward fields. These fixtures do not establish that ordinary PipeWire emits
the tested malformed structures and never connect to an audio server.
`snapshot` on this host correctly returns `dependency-unavailable` with exit 1.

Native prerequisite set, to be provided separately rather than installed here:

1. PipeWire daemon, `pw-dump`, `pw-cli`, native `libpipewire-module-protocol-native`,
   `client-node`, `adapter`, `filter-chain`, builtin filter SPA plugin,
   audioconvert SPA library, and the test server's `link-factory`, `spa-node-factory`,
   `metadata` and native client access modules. No PulseAudio bridge is required.
2. WirePlumber 0.5+ and a private session D-Bus (`dbus-run-session`) for policy
   recovery. Start a separate PipeWire server/socket in a private runtime with a
   dummy driver, no ALSA/BlueZ/V4L2/libcamera hardware monitors, and no default
   live config fragments. WirePlumber must use isolated config/state directories
   and have those monitor features disabled **before starting**. Never launch a
   second unrestricted WirePlumber against live hardware for these tests.
3. Native deterministic test source and uncompressed file consumer(s), e.g.
   PipeWire builtin sine source and native `pw-record` directly to files, plus
   `pw-top` or a profiler exposing xrun/error counters. Test consumer PCM must not
   pass through the controller or shell sample pipes. There is no native build.

Still-blocked native acceptance (requires a replacement backend before these
checks can qualify this adapter):

- Measure simultaneous source-to-Call unity, Record half amplitude, Monitor
  silence; then change only Record. Capture with two simultaneous consumers of
  each source, reopen them, and verify unchanged endpoint identity and no leakage.
- Verify stereo L/R averaging, mono duplication, missing/ambiguous mic silence,
  unrelated-route preservation, source replacement, default changes, explicit
  Monitor disconnect, controller SIGKILL **during** link creation, producer kill,
  server restart, and WirePlumber removal/restart. Test downstream fallback as
  a separate application result. Validate native filter controls/readback.
- Sustain simultaneous custom mixes and multi-device fanout under realistic
  CPU/I/O load and independent device clock drift,
  rate/quantum changes and suspend/reconnect. Analyze captured PCM for gaps,
  discontinuities, clipping, channel errors and long-run drift; observe native
  error/xrun deltas and latency. `pw-dump` supplies no reliable dropout count:
  unavailable counters are unknown, never zero. No current quality/dropout claim.
- With real Bluetooth hardware, record active profile/codec/rate before/after,
  test headset recording/profile loss/reconnect, hardware volume and play/pause/
  next/call buttons with existing apps, device-origin attribution, selected primary/
  all-routed session targets, and unknown-origin rejection. A2DP and HFP/HSP have
  different quality
  and duplex constraints; codec availability is device/build dependent. Existing
  OS auto-switch policy can degrade playback when recording begins. Do not
  override it or add a dummy player to hide incompatibility. Wired-headphone
  latency and long-running independent clocks remain separate native gates.

## Sources studied

The implementation is original; no Sonusmix MPL code was copied. Sonusmix already
offers group nodes, app endpoints, configuration locking and best-effort identity
restoration. This prototype limits itself to explicit named routes rather than
recreating its grouping and policy system.

- [Sonusmix README](https://github.com/sonusmix/sonusmix) and
  [native link/group implementation](https://github.com/sonusmix/sonusmix/blob/main/src/pipewire_api/mainloop.rs).
- [PipeWire loopback](https://docs.pipewire.org/page_module_loopback.html),
  [filter-chain and builtin mixer](https://docs.pipewire.org/page_module_filter_chain.html),
  [base client filter configuration](https://github.com/PipeWire/pipewire/blob/master/src/daemon/filter-chain.conf.in),
  [pw-cli local module/link lifecycle](https://docs.pipewire.org/page_man_pw-cli_1.html),
  [pw-cli asynchronous error handling](https://github.com/PipeWire/pipewire/blob/master/src/tools/pw-cli.c),
  [link factory](https://github.com/PipeWire/pipewire/blob/master/src/modules/module-link-factory.c).
- [WirePlumber linking/fallback properties](https://pipewire.pages.freedesktop.org/wireplumber/policies/linking.html),
  [Bluetooth profiles/codecs/hardware volume](https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/bluetooth.html),
  [profile auto-switch settings](https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/settings.html),
  [auto-switch graph traversal](https://github.com/PipeWire/wireplumber/blob/master/src/scripts/device/autoswitch-bluetooth-profile.lua).
- [PipeWire native profiler counters](https://docs.pipewire.org/page_man_pw-top_1.html).
- [PipeWire 1.4.9 `pw-dump` property encoding (`put_value` / `put_dict`)](https://github.com/PipeWire/pipewire/blob/1.4.9/src/tools/pw-dump.c#L269-L292).
