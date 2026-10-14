# Mixyard implementation plan

Research date: 2026-10-14. Working name: Mixyard. Own code: MIT.
Decision: develop Linux, Windows and macOS concurrently; qualify each independently.
Hard requirement: excellent audio quality and ease of use. Previous routing setups
had dropouts/bad sound. Reliability is a release gate, not a post-release polish task.

## Product contract

Create arbitrary user-defined mixes that applications can select as audio inputs
or physical devices can play. Mixes have stable IDs and editable display names;
there is no fixed product mix count or requirement to use special names.
Call/Record/Monitor below are safe starter presets, not the only supported buses.

One mix may feed several physical devices and/or virtual input endpoints. A device
can receive a chosen mix; separate output edges own device gain/mute without
changing the mix for other listeners. Native endpoint/device resource limits must
be reported explicitly, never silent truncation. Dynamic endpoint publication and
renaming are Windows/macOS driver feasibility gates, not assumed from a fixed
two-channel virtual cable. A bounded realtime implementation limits actual resource
use deliberately; this does not imply an arbitrary preset/count cap.

| Bus | Initial sources | Destination |
| --- | --- | --- |
| Call | Mic and explicitly shared audio, never its own call return | Call virtual microphone |
| Record | Explicitly selected sources; call return only when selected | Record virtual input / supported recording consumer |
| Monitor | Selected sources; mic sidetone disabled initially | Explicit listening output, preferably wired headphones |

Every source has a separate gain and mute for each destination. Do not expose all
system playback to calls by default. Concrete gate: the same mic simultaneously
at gain 1.0 in Call, 0.5 in Record and 0 in Monitor; changing one send must leave
the others unchanged. Per-source global volume cannot implement this requirement.
Call-return exclusion is a per-mix rule using logical source groups. Multiple
calls need separate destination-specific mix-minus; no global one-call assumption
is built into the custom configuration. Starter examples can remain one call.
Software monitoring requires no external audio interface, but is subject to device
latency, clock drift and feedback. No zero-latency promise.

Protected routing means enforcing Mixyard-owned routes and reporting deviations.
It cannot force arbitrary applications to choose this microphone, prevent fallback
outside our graph, or disable their AGC, AEC, noise suppression or permissions.
Browser process identities do not guarantee tab-level isolation.
Automatic recording means a stable Record input and explicit supported consumer
rules, not silently changing every recorder or the system default microphone.

## Architecture

Initial dynamic model: sources -> user-defined mixes -> physical/virtual outputs.
Per-edge sends and output gain/mute; per-mix master, channel format and exclusions.
Keep this simple layered shape initially; nested mixes are a later explicit feature,
not required for custom labels, counts, sends or multi-device fan-out.
Optional acyclic mix-to-mix routing follows the initial native transport proof.
Before enabling it, propagate source provenance, private-source exclusions,
destination-specific mix-minus and mic-to-physical-output consent through every
reachable mix/output. Reject a whole path carrying an excluded source: once PCM
is summed, downstream routing cannot unmix it to remove private audio or call
return. Rebuild an upstream safe mix from its original sources instead. Reject
cycles and duplicate source paths converging at a destination initially; any later
intentional duplication needs an explicit policy and qualification. Compute
transitive gain/peak bounds across every path, mix master and output gain; require
explicit headroom and leakage/consent tests before enabling nested routes.
The policy prototype branch plans a fixed-bus v1 to dynamic v2 schema upgrade.
Safety follows source/output IDs and exclusions, never mix labels. Explicit mic
monitoring consent is required when a mic path reaches any physical output, even
if a custom mix is not named Monitor. Renaming preserves all safety rules and IDs.
Initially reject multiple Mixyard output routes converging on the same device /
channel set rather than silently summing; combine selected sources into one mix.
Include output gain in peak bounds. One failing/slow output cannot stall siblings.

Device-scoped playback control: map an attributable button event's originating
device to that device's current audible routing and media sessions. Configure
`primary player` (explicit app/session) or `all routed players`; also permit normal
OS behavior as an explicit default mode. Spotify on device A and VLC on device B:
pause from A affects Spotify only. If a device carries both, its chosen policy
decides which player(s). Deduplicate by media session; one app routed twice still
receives one action. Missing primary stays unsupported/waiting rather than choosing
another player. Resolve from a versioned routing snapshot to handle route changes.
Retain configured player/session associations after pause removes its audio stream;
Play must still find the intended session. Group PlayPause is resolved once into
explicit operations: pause eligible playing sessions, then resume only the
remembered previously-playing group. Do not individually toggle mixed playback
states. Unknown state/capability and partial failures are explicit.
Revalidate remembered sessions after routing/session changes; do not restart an
unrelated intentionally paused player. Report per-session partial/unknown results;
non-idempotent next/previous commands are not blindly retried after uncertain
delivery. Deduplicate duplicate events without suppressing distinct intentional presses.

Player transport is global to that media session. Spotify feeding devices A and B
pauses on both if Spotify is paused from A. Device-only silence is a separate
output mute/gate; it does not pause or seek the shared player. Device-panel control
can supply an explicit origin even where hardware events lack identity, subject
to actual application-control capabilities. Label these actions accurately.

Native event attribution and player-control capabilities differ per OS/device/app.
Global media events that lack source-device identity cannot implement this policy
reliably. Native device-scoped buttons require both reliable origin identity AND
ownership/suppression of the original OS dispatch; observing HID alone is not
enough to prevent another player receiving the default event. Report unavailable
device-specific control; never guess by timing/last active device or broadcast. Avoid
double dispatch through both our bridge and the desktop. Media controls preserve
normal OS ownership when the explicit device-scoped mode is off. Volume controls
target native device/output gain by documented policy, not source sends; mute/call
commands keep their separate qualification and scopes.

Interface requirement: polished desktop apps on Linux/Windows/macOS, a genuinely
headless CLI for people and agents, and tray/menu-bar panels for supported desktops.
The audio runtime owns routing and survives UI closure. GUI, tray and CLI are
clients of the same state/control contract; no second mixer implementation.

UI decision: Tauri 2 + Vue 3/Vite/TypeScript with platform tray integration.
Mathis delegates the choice conditional on performance and UX; acceptance is
measured, not assumed from toolkit choice. Native audio is a separately managed
runtime, not a Tauri-owned sidecar terminated when the app exits. Keep the core
and CLI build graph free of Tauri/WebKit/GUI dependencies.

Qualify UI response and frame timing in each actual OS webview and tray host under
normal/contended load. Proposed target: local interaction p95 <=100 ms, prompt
visible state updates, no sustained scrolling/dragging stalls. Meter events are
bounded/coalesced, not sample buffers; UI activity must not introduce audio gaps.
Freeze exact reference thresholds before qualification. Fix measured client issues;
if webview/platform integration cannot meet the gates, replace that shell/client
without changing the audio runtime or CLI. Do not accept a laggy interface as the
price of a shared codebase.

The CLI must not require the GUI to be installed/running or a webview/display session.
Plan validation, status, devices, route operations, gain/mute and diagnostics with
stable --json outputs, meaningful nonzero failures and JSONL events where useful.
Commands act on the same desired/observed state, not a competing audio graph.
Mutating runtime commands follow the control runtime milestone, not the first
dry-run policy prototype. Per-user local IPC uses OS permissions (Unix sockets /
Windows named pipes); no public control listener or app OAuth scheme is needed.
Use private per-user sockets with peer checks / current-user named-pipe ACLs;
reject remote pipe clients. The OS user account is the authorization boundary,
not isolation from other programs of the same user. Webview bridge capabilities
allow narrow commands only, no generic shell/remote-page control interface.
Version requests and state, acknowledge accepted vs applied changes separately,
and use revision checks so stale GUI/CLI state cannot overwrite newer choices.
Slow/disconnected clients cannot block audio: bounded event queues with fresh
snapshot on reconnect, discard old-runtime events. Query state after uncertain
accepted mutation rather than blindly retrying; explicit set-mute is safer than
toggle, recording operations need request IDs. Measure command-to-effective-audio
latency separately from visual response. Test CLI/tray/GUI contention and runtime
startup/packaging without any GUI library or tray host.

Tray panels: quick Call/Record/Monitor levels and mute, selected-device status,
recording indicator and open-app action. Start/stop runtime and quit-UI are distinct.
On macOS use a menu-bar panel, Windows notification area, and Linux supported
StatusNotifier/AppIndicator hosts; desktops without tray support retain app + CLI.
Host integration is qualified per desktop, not assumed from an OS name alone.
Tauri Linux tray mouse events are unsupported; context-menu Open Mixer is the
baseline. Rich anchored panels need verified host/native integration rather than
assuming identical icon-click behavior. App + CLI remain usable without a tray.
Hardware mic mute, Call-send mute and recording mute remain separate; never show
global microphone-muted while an unmuted recording route is still active.

Everyday UX: choose mic/headphones, use safe presets, select named Mixyard inputs
in call/recording apps once; persisted choices and explicit degraded status on
reconnect. Graph editing is advanced. Keyboard/accessibility and native window/tray
behavior are acceptance requirements, not just appearance. CLI-only operation,
GUI close/reopen, runtime restart and simultaneous CLI/tray changes are tested.

Use a small Rust configuration/policy CLI, with JSON serialization and standard
library process control. It models desired routing and diagnoses observed state;
it does not run realtime audio callbacks. No generic backend framework or GUI dependencies in this crate.
Native probes exchange the small versioned envelope in backend-contract.md.

Linux delegates graph mixing, fan-out, formats and virtual endpoints to PipeWire.
Windows uses native C++ WASAPI capture and a separately proved virtual-driver
transport. macOS uses native Objective-C++ Core Audio capture and a separately
proved HAL virtual-device transport. Windows/macOS independent sends will need
actual mixing, bounded realtime buffering, rate conversion and clock adaptation;
choose the shared sample engine only after native transport experiments.

Persist semantic device UIDs/serials and application identities, never transient
node IDs/PIDs. An ambiguous selector stays disconnected. Resolve exact configured
sources at runtime. A lost source contributes silence; never substitute another
microphone. Distinguish desired, observed and verified-audio states. Reconciliation
must be idempotent and bounded, own only our objects, and avoid policy oscillation.
Preflight rejects cycles, invalid gains and call-return paths into Call.
Reject Mixyard output/producer recapture as an application source. Route repair
is opt-in per managed application; provide an explicit release-management command.
Applying/reapplying configurations and a failure halfway through a change must
preserve unrelated system routes and roll back only owned objects. Test controller
crash during mutation for stale links and leakage, not only after a clean shutdown.

Virtual device identities should survive UI/controller restart where the platform
permits. Treat controller failure, audio-server failure and driver failure as
different cases. No claim that a downstream app's fallback is prevented.

## First parallel implementation batch

The original prototype batch used one canonical checkout with disjoint ownership.
Publication now uses separate branches/worktrees and focused PRs with independent
review and a different fixer. This foundation branch contains docs/README only;
the implementation paths below describe the next branches. Report cross-owner
requests instead of editing their files.

| Thread / ownership | Deliverable | First acceptance gate |
| --- | --- | --- |
| Policy: Cargo.toml, Cargo.lock, src/, examples/ | Dynamic mix/output config, deterministic dry-run plan and media target resolver | Arbitrary names/counts; fan-out; independent gains; mix-minus; no fallback; device-scoped player selection |
| Linux: adapters/linux/ | Native discovery, Call/Record virtual sources and Monitor route prototype | Disposable PipeWire graph; independent sends; multiple consumers; owned teardown only |
| Windows: adapters/windows/ | WASAPI discovery and explicit selected-process or mic capture; virtual-transport audit | Native build and useful errors; actual capture and endpoint injection separately tracked |
| macOS: adapters/macos/ | Core Audio discovery and selected-process tap capture; HAL transport audit | Native build and denied-permission errors; actual capture and endpoint injection separately tracked |
| Qualification: verification/, .github/workflows/ | Synthetic PCM analyzer/scenarios and targeted platform build workflow | Model tests separated from real capture tests; absent prerequisites cannot pass |
| Desktop: apps/desktop/ | Tauri/Vue/Vite UI and platform tray prototype | Visible Demo/Not connected; no fake applied routes; keyboard/layout checks; real shell checks separately tracked |

Root owns docs/, README.md and repo integration. Backend tools can build
independently; no platform owner modifies the root Cargo package. Configuration
compatibility is a contract, not a claim that the first native probes implement
the full policy model. Build workflows use hosted runners, not a new homelab CI
runner. Do not buy signing certificates, install drivers, change global audio
defaults or restart a live audio service during this batch.

## Qualification and release sequence

Quality gate proposals (targets, not results):

- Transparent wired PCM path: include a delay-aligned null comparison on a supported
  single-source unity-gain same-format route; no unintended processing, clipping or channel/rate
  changes; sample comparisons account for explicitly negotiated conversions and
  quantization. Playback and recording must not apply hidden normalization.
- First run a two-hour simultaneous-bus stress test, then zero unexplained gaps/dropouts
  in a proposed overnight (at least eight-hour) endurance run on each
  supported reference setup under simultaneous Call/Record/Monitor and defined
  normal desktop load. Instrument xrun/gap/queue overflows and drift; absent
  instrumentation means unknown, never zero. Publish OS/device/profile/load evidence.
- Measure end-to-end monitoring latency on actual wired reference devices, including
  median, p95 and p99 behavior under load. Proposed qualified wired target: p95
  no more than 20 ms and p99 no more than 30 ms; confirm on actual reference setups.
  Bluetooth has separate measured targets; do not claim hardware-direct latency.
- No sample-data subprocess pipelines: native graphs/callbacks carry PCM. Subprocess
  commands are for control/discovery only. Avoid repeated route rebuilds and default
  device churn; stable routes stay untouched during reconciliation.
- Native tests cover clocks, negotiated rates/layouts, replug, rate/profile changes,
  consumer restarts, suspend, CPU contention and producer failure. RT queue overflow
  causes visible degraded/failure status, not silent successful capture with holes.
- Use a known nonrepeating waveform in time-resolved checks for dropped/repeated
  samples, brief corruption/leakage and channel swaps; tones/average levels alone
  miss short faults. Freeze negotiated-conversion tolerances before qualification.
  Endurance also requires bounded memory/no growing latency; intentional transport
  interruptions are separately recorded with recovery times, not steady-state passes.
- Test gain/mute/apply/startup/shutdown/recovery transitions for clicks, clipping
  and leakage. Intentional gain ramps/fades must be specified and measured.
- Ease gate: safe Call/Record/Monitor presets, clear device/application selectors,
  saved setup, visible active/degraded status and a concrete fix. Ordinary operation
  must not require a graph editor, terminal commands or global device defaults.
  Initial probes are engineering tools, not the finished user experience.

Bluetooth support includes preserving supported headset play/pause, track and
volume controls through the OS's existing media/session stack. Do not hijack the
active player or rewrite Bluetooth. Mic mute and call answer/end are separate,
device/OS/application-dependent integration gates. Test selected supported devices.
Mathis requests all of these controls. Record support and synchronization per
control, including hardware mute state vs router/application mute, not one blanket
"Bluetooth supported" checkbox. Answer/end targets a chosen supported call app;
media controls retain the active media owner. Unsupported controls stay explicit.
Test two media players plus a call: normal mode preserves the OS-selected session;
scoped mode targets the configured primary/group once with no parallel OS action.
Volume retains OS device-volume semantics instead of
silently changing mix sends. Hardware mute and Call/Record mute states are distinct.
Expose active profile/codec and mic-activation quality changes where observable.
Some headsets cannot combine high-quality stereo playback with their mic on the
selected platform; never silently promise that software can remove that limit.
Wireless quality/latency is certified separately from transparent wired routing.
Never open a BT microphone for discovery/meters alone: use it lazily only when an
active selected route needs samples. Windows Classic also switches to HFP for
Communications-category playback; headphone Monitor defaults to ordinary media
category to retain stereo with a separate mic. PipeWire/WirePlumber profile policy
must be inspected and tested; don't globally change auto-switch policy silently.

1. Freeze semantics and execute offline policy tests alongside all native spikes.
2. Prove real independent buses using distinct deterministic source tones; observe
   each output in uncompressed PCM. Verify gain, mute, mix-minus, routing leakage,
   consumer reopen and cleanup. Linux uses an isolated audio server first.
   Run WirePlumber in recovery tests: bare PipeWire cannot prove session-manager
   fallback/reconnect policy. Test no fallback, silence and app input changes as
   separate outcomes. Endpoint-loss consumer behavior is an app compatibility gate.
   For captured app playback, qualify original rendering separately from capture
   and injection: select a supported non-audible render endpoint in the app, or use
   documented redirection/suppression that preserves captured PCM, before replaying
   through Monitor. Do not assume session mute preserves loopback capture. Native
   tests must start with the app playing directly to the intended Monitor device,
   establish exactly one audible path after routing, and prove Monitor send and
   output gain/mute affect that audio while capture for other mixes continues.
   Repeat on app relaunch, default-device change and recovery; report an original
   path that bypasses control as unsupported for managed monitoring, not working.
   Capture-only support may be reported separately, without Monitor gain/mute claims.
3. Prove Windows/macOS real-data injection into two separately selectable input
   endpoints; discovery or a sample tone alone does not prove transport.
4. Add lifecycle ownership, reconnection and source identity recovery. Measure
   unplug/replug, app relaunch, duplicate events, suspend, changed default device,
   rate changes, router/server/driver crashes, permission denial and exclusive use.
5. Add recording integration and connect the parallel UI prototype to live controls
   after transport proof. A future owned recorder
   needs recoverable segments, bounded queues, disk-full/gap diagnostics, explicit
   arming and no silent automatic recording after recovery.
6. Ship per-platform installers only after native tests, signing/license audit,
   crash/privacy qualification and measured wired-headphone latency. Publish a
   capability matrix rather than a universal reliability claim.

Real-time paths cannot allocate, log, take blocking locks, touch disk or call UI.
Capture-to-file probes use bounded preallocated queues and a separate disk writer,
and diagnose queue overflow. Windows initially uses shared mode, never exclusive.
Test independent device clocks over long recordings; buffer tuning alone cannot
prove latency. Bluetooth profiles, exclusive applications, protected playback,
stream changes, OS updates and permission revocation remain test cases.

Initial format policy: each mix explicitly selects mono/stereo. Call preset is
mono; Record/Monitor presets stereo. Mono to stereo duplicates
the sample and stereo to mono averages L/R. Reject unhandled multichannel layouts
instead of guessing. Prefer 48 kHz but negotiate supported physical formats and
delegate conversion to native facilities where possible. Independent clocks still
require measured adaptation; one fixed FIFO cannot solve drift.

Initial gain policy: every send, mix master and output gain must be finite and
in [0,1], including muted routes. For unit-full-scale source peaks, define effective
level as 0 when muted, otherwise the configured gain. Compute each conservative
mix peak bound = effective master × sum(effective send levels), then each output
peak bound = its mix peak bound × effective output level. Reserve headroom for all
configured sources, including temporarily absent ones. Reject any mix or output
bound above 1; choose explicit send/master attenuation rather than using output
attenuation or mute to hide an unsafe upstream mix. Never silently normalize other
sends when a source appears/disappears. This conservative prototype avoids adding
an untested limiter; a later sample mixer can introduce explicit limiting. Tone
tests use non-clipping levels and test stereo/mono mapping separately.
This is a prototype restriction, not the finished UX. Finished mixes need visible
headroom and peak/clipping indication, adjustable explicit bus attenuation and any
limiter clearly labelled; worst-case estimates alone must not make normal mixing
unusable. Capture diagnostics expose negotiated rate/layout/buffer and available
gap/overflow counters; unavailable counters must not be presented as zero.

## Platform and licensing constraints

PipeWire already supplies virtual sinks/sources. Sonusmix overlaps substantially
with Linux app grouping and configuration locking; study it before duplicating
policy mechanisms. Any copied MPL files retain their license obligations.

WASAPI process loopback captures process trees; it does not create a microphone
or redirect original playback. Capturing and replaying an app already rendering
to the Monitor device can create duplicate delayed audio; muting a Mixyard route
cannot silence that original path. Windows managed monitoring must pass the
original-render gate above using supported app endpoint selection or documented
routing facilities; capture alone is insufficient. SysVAD is a driver starting
point, not a ready sample-carrying audio cable. Evaluate permissively licensed
virtual transport foundations and prove capture/render bridge, multi-endpoint
support, maintenance, and current Secure Boot/HVCI/WHCP compatibility before
selecting one. Certification
and developer enrollment can cost money even for a free open-source product.

Core Audio taps capture process audio; virtual microphone publication requires a
HAL plug-in. BlackHole is an established GPLv3 candidate. Do not vendor/relicense
it into the MIT project. An optional externally installed driver can be evaluated
without bundling; adopting its source requires an explicit integration/license
decision. Apple sample code also has its own license; inspect before copying.
macOS packaging/signing/notarization and SDK availability are release gates.

Desktop scope initially targets current Linux/PipeWire, Windows 11 and a macOS
version established by tap API availability and native tests. Linux without
PipeWire and older desktops are separate potential adapters. Android/iOS public
capture APIs do not establish universal virtual-mic injection; mobile starts as
possible controller or cooperating-app integrations, not promised desktop parity.

## Verified setup and open gates

GitHub: https://github.com/MGRAFF2006/mixyard . T3 project registered. Canonical
checkout is on homelab-dev; independent Git checkouts are the intended laptop
sharing model. Bootstrap README/LICENSE/.gitignore pushed as 1c7aedc.
At the research snapshot, laptop Tailscale ping succeeded but SSH timed out;
laptop checkout/sync remained pending. The separate setup prompt is in
[LAPTOP-SSH-PROMPT.md](LAPTOP-SSH-PROMPT.md); publication is not proof of SSH setup.
Syncthing currently shares config only. No live Git metadata/worktree sync added.

No verified Windows/macOS test hosts. Hosted CI compilation does not prove audio
devices, microphone permissions, Secure Boot, latency or call-app compatibility.
Review: an independent subagent approved proceeding after corrections for edge
gains, ownership-aware rollback, WirePlumber policy tests, consumer fallback,
channel/headroom policy and clock drift. Its in-memory synthetic model passed
independent gains, mix-minus, private-source exclusion, disabled mic monitor and
absent-mic silence. Counterexamples: three aligned 0.5-amplitude sources peak at
1.5; hypothetical 100 ppm drift produces 288 frames / 6 ms per minute at 48 kHz.
These are model evidence, not native audio certification. Real platform tests
remain pending. Capture capability does not imply endpoint injection capability.
Follow-up independent reviews approved the quality gates, UI/runtime separation
and dynamic mix/media-control model after correcting mic consent by output semantics,
fan-out failure isolation, event origin AND dispatch ownership, remembered playback
sessions, partial command outcomes and dynamic endpoint capacity/lifecycle tests.

## Primary sources

- [PipeWire loopback / virtual sources](https://docs.pipewire.org/page_module_loopback.html)
- [PipeWire filter-chain](https://docs.pipewire.org/page_module_filter_chain.html)
- [WirePlumber linking and missing targets](https://pipewire.pages.freedesktop.org/wireplumber/policies/linking.html)
- [Sonusmix, MPL-2.0](https://github.com/sonusmix/sonusmix)
- [Windows process-loopback sample](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/)
- [Windows SysVAD sample](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/sample-audio-drivers)
- [Windows driver policy, 2026 changes](https://support.microsoft.com/en-us/windows/hardware/drivers/the-windows-driver-policy)
- [Windows EV enrollment](https://learn.microsoft.com/en-gb/windows-hardware/drivers/dashboard/code-signing-reqs)
- [Windows virtual driver candidate](https://github.com/VirtualDrivers/Virtual-Audio-Driver)
- [Core Audio taps](https://developer.apple.com/documentation/CoreAudio/capturing-system-audio-with-core-audio-taps)
- [Core Audio HAL plug-in](https://developer.apple.com/documentation/coreaudio/creating-an-audio-server-driver-plug-in)
- [BlackHole GPLv3 guidance](https://github.com/ExistentialAudio/BlackHole)
- [Apple distribution signing](https://developer.apple.com/developer-id/)
- [Browser media constraints](https://w3c.github.io/mediacapture-main/)
- [Android playback capture](https://developer.android.com/media/platform/av-capture)
- [Apple ReplayKit](https://developer.apple.com/documentation/replaykit)
- [Windows Classic Bluetooth audio profiles](https://learn.microsoft.com/en-us/windows-hardware/drivers/bluetooth/bluetooth-classic-audio)
- [WirePlumber Bluetooth configuration](https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/bluetooth.html)
- [Apple Bluetooth audio quality with microphone](https://support.apple.com/en-us/102217)
- [Tauri process/webview model](https://v2.tauri.app/concept/process-model/)
- [Tauri tray integration and platform limits](https://v2.tauri.app/learn/system-tray/)
- [Windows SMTC button event fields](https://learn.microsoft.com/en-us/uwp/api/windows.media.systemmediatransportcontrolsbuttonpressedeventargs)
- [Windows device-specific Raw Input](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-raw-input)
- [Apple remote-command event fields](https://developer.apple.com/documentation/mediaplayer/mpremotecommandevent)
- [BlueZ registered-player forwarding source](https://github.com/bluez/bluez/blob/master/profiles/audio/media.c)
- [BlackHole static device implementation](https://github.com/ExistentialAudio/BlackHole/blob/master/BlackHole/BlackHole.c)
- [PipeWire multi-output combine stream](https://docs.pipewire.org/page_module_combine_stream.html)
