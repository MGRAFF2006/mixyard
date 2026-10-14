# macOS Core Audio probe

This independent MIT probe implements device discovery, Core Audio process
inventory, and explicitly selected-process capture to stereo Float32 WAV. It does
not implement virtual microphone transport, independent bus sends, microphone
capture, routing recovery, or a product UI. No default device, physical-device
format, original playback mute, Bluetooth profile, media command registration,
or OS media-button ownership is changed. There is no dummy media-player session.

## Build and commands

Requires macOS, Apple developer tools, and a macOS SDK **14.2 or newer**. No third
party libraries. The SDK header macro is checked at compile time; the build script
checks the selected SDK version. Build independently from this directory:

```sh
bash adapters/macos/build.sh
adapters/macos/build/mixyard-coreaudio-probe snapshot
adapters/macos/build/mixyard-coreaudio-probe processes
adapters/macos/build/mixyard-coreaudio-probe capture \
  --bundle-id com.example.explicitly-selected-app --seconds 10 --wav selected.wav
# For helper processes, select each currently observed PID explicitly:
adapters/macos/build/mixyard-coreaudio-probe capture \
  --pid 123 --pid 456 --seconds 10 --wav selected-helpers.wav
```

The executable embeds `Info.plist`, including `NSAudioCaptureUsageDescription`,
for the system audio-recording permission request. CLI/terminal attribution and
signing-dependent privacy behavior still require native validation. Do not assume
an unsigned or ad-hoc-signed binary is permitted merely because creation succeeds.
The OS may prompt on explicit capture; discovery never requests recording access.
Permission errors preserve the JSON envelope, describe the permission setting,
and exit nonzero. Zero-only PCM is reported as silent-or-denied, **not** falsely
identified as permission denial or passed audio evidence.

`snapshot` emits the version-1 backend envelope, with stable opaque device UIDs,
channel counts, nominal rate, direction, and observed transport type. `processes`
adds bundle IDs, ephemeral PIDs and running-output state; it does not enumerate
unrelated command lines. A bundle selector must match exactly one process.
Missing, duplicate or ambiguous selections fail before creating capture objects.
PIDs are only explicit runtime selectors, not durable application identities;
browser/helper identities do not promise tab isolation. Mixyard and self capture
are rejected. A changed/lost process stops capture without choosing a replacement.

All commands emit exactly one JSON object to stdout; diagnostics go to stderr.
Errors exit nonzero. Successful capture adds format, frame count, selected sources,
nonzero-sample status, interruption and buffer/clock diagnostics. Existing files
are never overwritten. Signals stop and finalize a partial recording; the JSON
marks interruption. Capture is limited to 1–300 seconds and at most 32 selectors.
The source processes retain their normal audible playback.

`deviceDiscovery` becomes true after successful discovery. `processCapture` stays
false during snapshot/inventory because runtime permission has not been proved;
it becomes true only in a successful explicit capture result containing nonzero
PCM. That is functional evidence, not sound-quality qualification. `virtualInputs`,
`independentMixes` and `routeRecovery` stay false, including when a third-party
virtual device happens to be enumerated.

## Availability and audio boundaries

Inspected Apple SDK-derived API declarations on 2026-10-14:

- [AudioHardwareCreateProcessTap](https://developer.apple.com/documentation/coreaudio/audiohardwarecreateprocesstap(_:_:))
  and [AudioHardwareDestroyProcessTap](https://developer.apple.com/documentation/coreaudio/audiohardwaredestroyprocesstap(_:)):
  macOS **14.2**. Apple's documentation JSON `metadata.platforms.introducedAt`
  reports 14.2 for both functions.
- [Stereo process mixdown initializer](https://developer.apple.com/documentation/coreaudio/catapdescription/initstereomixdownofprocesses:):
  macOS 14.0. `CATapDescription` class availability alone (12.0) does not establish
  tap creation support.
- [Apple's capture sample](https://developer.apple.com/documentation/coreaudio/capturing-system-audio-with-core-audio-taps)
  explicitly requires 14.2+ in its setup text. Its current sample metadata lists
  newer SDK/Xcode versions; those do not replace individual API availability.

The binary deployment target is 13.0 so older Macs can discover devices and receive
an explicit `unsupported-os` error for process inventory/capture. Tap operations
are enclosed in `@available(macOS 14.2, *)` checks. SDK/API verification is source
evidence; no Apple SDK is installed here and no native compilation is claimed.

One private, uniquely named aggregate contains one private selected-process tap.
The negotiated tap format must be packed little-endian stereo Float32, planar or
interleaved, with an integer 8–192 kHz rate. WAV preserves that rate; there is no
resampling, silent multichannel guessing, volume normalization, Bluetooth quality
downgrade or physical-device format change. Core Audio performs the requested
process stereo mixdown; this is not Mixyard's independent bus mixer.

The HAL callback copies at most 4096 stereo frames into one of 32 preallocated
slots (~1 MiB); counters use lock-free 32-bit atomics. It performs no allocation,
logging, lock acquisition, disk I/O, Objective-C messaging or queue notification.
A separate writer thread drains the SPSC queue and validates/writes PCM. Queue
overflow, unexpected buffers, sample-timestamp discontinuity, format changes and
disk failures, stalled delivery and a short frame count against requested duration
produce nonzero failure and mark the recording unqualified. Drops
are not repaired or concealed. Permission failures and no-data capture cannot
pass. A failed disk write can leave an incomplete file; this probe is not a
recoverable production recorder.

After owned capture cleanup, draining and all WAV file operations (including
finalization and close after an error) must complete within two seconds. A blocked
writer reports `disk-write-timeout` and `capture-unqualified`, leaves process capture
unproved, and exits immediately without destroying storage still used by the worker.
There is no thread cancellation. A timeout can leave an incomplete WAV.

Only the tap clock is recorded. Drift compensation is disabled for this single
tap-only aggregate: there is no second physical clock to synchronize. This does
not prove clock stability or adaptation for future mic/monitor/virtual-device
mixing. Timestamp continuity is checked; sustained drift, latency, dropouts and
audio fidelity still need measured real-Mac evidence. The five-minute probe cap
does not constitute a sustained production-quality test.

Normal stop and error rollback stop/destroy only IDs returned by this invocation:
its IOProc, aggregate and tap. There is no device-list sweep, default routing edit,
audio-server restart or cleanup of someone else's aggregate. Failed teardown is
reported and the process exits immediately rather than freeing potentially live
callback storage. Forced termination, stale-object behavior and restart recovery
remain native gates; no recovery capability is advertised.

## Separate virtual-input milestone and license audit

**Tap capture does not create an app-selectable input endpoint.** The next milestone
is arbitrary externally supplied mixed PCM → HAL transport → **two independently
selectable user-named inputs**, with actual external-consumer recordings. Two is
the first proof, not a product limit. Call/Record/Monitor are starter presets, not
reserved names or a fixed three-mix schema. Product mixes have persistent IDs and
editable user names; any mix may feed multiple physical/virtual devices, with
separate output-edge gain/mute that leaves the mix and other listeners unchanged.
Native compilation, device enumeration, a tap WAV or a driver-generated sine tone
cannot pass that milestone.

Two foundations were inspected, without installing, bundling or copying source:

- [BlackHole LICENSE](https://github.com/ExistentialAudio/BlackHole/blob/561d934c77a1fa64987e01ccabe890fed30e181f/LICENSE)
  and [README](https://github.com/ExistentialAudio/BlackHole/blob/561d934c77a1fa64987e01ccabe890fed30e181f/README.md),
  inspected at commit `561d934c77a1fa64987e01ccabe890fed30e181f`: source is GPLv3;
  the notice separately reserves rights for official compiled binaries/installers
  and branding. Upstream requests a separate license for non-GPLv3 projects.
  Mixyard's MIT root does not authorize vendoring or relicensing this source, nor
  redistribution of upstream installers. An optional already-installed device
  can be evaluated via Core Audio, with packaging/legal decisions kept explicit.
  Do not assume separate installation resolves every integration/license question.
- [Apple HAL sample](https://developer.apple.com/documentation/coreaudio/creating-an-audio-server-driver-plug-in),
  original download inspected from
  [CreatingAnAudioServerDriverPlugIn.zip](https://docs-assets.developer.apple.com/published/430ad6501f6f/CreatingAnAudioServerDriverPlugIn.zip),
  SHA-256 `860334dfa9e83f2afb749a890ca5a79fc7d3d572d4e116ff9242bec458be4cb0`:
  `LICENSE.txt` is a 2024 Apple MIT-style permissive grant requiring retention of
  its copyright/permission notice. This is a viable permissive starting point;
  it is not automatically owned by Mixyard and is not imported here. The sample
  publishes one duplex Float32 device. Dynamic named devices and a robust arbitrary
  producer-to-consumer bridge still need implementation and proof.

For externally installed BlackHole, an explicit producer would open its **output**
by UID and supply arbitrary Float32 mixed PCM through a bounded output callback;
external consumers would open the corresponding input. BlackHole's channel count,
device names and optional mirror device are build-time configurations. A fixed
multichannel endpoint can carry several mixes in channel pairs, but ordinary app
microphone selectors do not thereby expose one named device per user mix. A
channel-aware consumer workaround is not arbitrary app-selectable mix devices;
adding more channels or renaming one installed device does not prove dynamic
publication. Two devices only establish the first proof. Inspection of
its `ReadInput`/`WriteMix` source shows a timestamped output ring and a zeroing path
when output stops advancing. This is source evidence, not a verified producer-crash
silence result. Existing producer collisions, reopen, clocks and no stale playback
must be tested; this probe does not feed BlackHole.

For an original HAL driver, derive stable endpoint UIDs from persistent mix IDs,
keep display names editable, and use a bounded producer bridge carrying mix PCM and clock
metadata. Use validity/generation and a bounded freshness deadline; input callbacks
must output zero for empty/stale producer data, including after producer death.
Do not loop stale PCM. Rate negotiation, clock adaptation and teardown must be
proved before adding independent-device mixing. No transport is implemented yet.

License decision for this batch: no BlackHole source adoption, linking or bundled
driver/installer. An externally installed BlackHole may be a fixed-transport
experiment. The audit recommends an original MIT-compatible HAL implementation
(optionally using Apple's sample with its notice retained) for dynamic named
endpoints; final driver selection remains an explicit root integration decision
after feasibility proof. Adopting BlackHole source would require a separate GPL
integration/distribution or upstream-license decision, not a silent MIT import.

### Dynamic publication and fan-out gate

HAL exposes the driver's device list through
[kAudioPlugInPropertyDeviceList](https://developer.apple.com/documentation/coreaudio/kaudiopluginpropertydevicelist)
and provides host
[PropertiesChanged](https://developer.apple.com/documentation/coreaudio/audioserverpluginhostinterface/propertieschanged)
notifications. These are building blocks, not a working Mixyard driver. The inspected
`NullAudio.c` sample returns `kAudioHardwareUnsupportedOperationError` from both
`CreateDevice` and `DestroyDevice`, commenting that these hooks serve Transport
Manager semantics. Do not treat
[CreateDevice](https://developer.apple.com/documentation/coreaudio/audioserverplugindriverinterface/createdevice)
as a ready arbitrary-virtual-microphone factory or confuse a client-owned capture
aggregate with a published transport driver. A public driver control path, dynamic
owned object graph/list, UID lookup and name/lifecycle notifications must be
implemented and qualified; no private API is assumed.

The native gate must add, rename and remove user-named devices without reinstallation
or audio-server restart while unrelated consumers keep recording. Exercise more
than three mixes, Unicode/duplicate display names distinguished by UIDs, reopening
after rename, deleting an in-use endpoint, and runtime/UI/driver restart. Publish
only fully prepared resources; allocate and admit graph/queue changes off the RT
path. Report actual HAL/device/queue capacity errors explicitly, with rollback of
owned objects only. Bounded resource admission is not a fixed product mix count.

Prove one mix concurrently feeding multiple physical/virtual devices. Each reader
needs independent bounded consumption and device-clock adaptation, so a stalled
or disconnected output cannot consume another listener's frames or stall the mix.
Changing one output gain/mute must leave other listeners unchanged. No needless
same-rate resampling, silent profile changes or default-device churn. Drift,
producer-failure silence and sustained fan-out recordings remain unproved.

The transport acceptance recording must have both inputs simultaneously selected
in independent external consumers, distinct known source sequences, independent
gain/mute and mix-minus checks, no cross-bus leakage, and producer-kill/reopen tests
proving silence rather than stale PCM or a substitute microphone. Retain consumer
recordings and format/clock evidence, with AGC/AEC disabled where supported; test
app fallback separately. Qualification and hosted workflows belong to their owner.

Distribution remains gated on [Developer ID signing and notarization](https://developer.apple.com/developer-id/),
HAL plug-in and installer packaging, architecture compatibility, privacy identity,
Gatekeeper behavior, upgrades/uninstall, explicit licensing and real-Mac permission
tests. Developer enrollment/certificates are not acquired. No driver, package,
certificate, system audio setting or audio service is installed/changed/restarted.

## Device-scoped Bluetooth control gate

The required policy resolves an attributable button's originating device against
a versioned snapshot of its current audible routes, then selects either its
explicit **primary player** or **all routed players**. Deduplicate by player/session,
even when a mix or source fans out. A missing primary stays unavailable; do not
pick another player. Unknown event origin causes no Mixyard dispatch, never a
broadcast, last-used-device guess or global-active-player substitution. Preserve
normal OS handling when device-scoped control is off, and qualify exclusive event
ownership to prevent both the OS and Mixyard dispatching the same event.

[MPRemoteCommandCenter](https://developer.apple.com/documentation/mediaplayer/mpremotecommandcenter)
registers handlers for the receiving app's player; Apple's
[event-handling guidance](https://developer.apple.com/documentation/mediaplayer/handling-external-player-events-notifications)
requires the app to be Now Playing. The documented
[MPRemoteCommandEvent](https://developer.apple.com/documentation/mediaplayer/mpremotecommandevent)
properties are command and timestamp, not originating hardware UID. Neither that
API nor a global media-key event proves headset attribution or arbitrary third-party
player control. Do not claim otherwise or create a fake Now Playing session to
intercept commands. No private MediaRemote API, synthetic global key dispatch or
undocumented Bluetooth hook is implemented.

Native support requires two independent proofs: public event attribution for the
specific headset/OS, and a documented cooperative or app-specific control interface
for each selected player, with any required permissions qualified. Neither is
implemented here; device-scoped control is unavailable. Test Spotify on device A
and VLC on B: A's pause targets only Spotify. With both routed to A, prove primary
vs all policy, one action per session, route-change races, missing primary and
unknown-origin no-dispatch. Pausing a shared player also pauses its playback on
other devices; app playback control is not per-output audio mute. Volume, hardware
mic mute and call answer/end retain separate scopes and device/app gates. Root owns
the current research/policy contract; this lane records the macOS native gap.

## Evidence from this batch

On the Linux host, 2026-10-14, `bash adapters/macos/check.sh` passed the focused
synthetic queue/WAV test under AddressSanitizer and UndefinedBehaviorSanitizer:
overflow/bounds, planar interleave, 10,000 threaded FIFO publications, exact WAV
header/data bytes and refusal to overwrite. Plist parsing and shell syntax passed.
Free disk remained 76 GiB. This is **portable synthetic evidence only**.

The writer shutdown regression also checks normal final WAV bytes and creation,
write, flush and close errors. On Linux, linker-injected blocking PCM writes,
flush, seek, final header write, close and destructor close run in isolated child
processes under ASan/UBSan, with a five-second external guard. Each blocked child
reports unqualified failure and exits at the two-second writer deadline. The
libc fault injection requires the Linux linker; it is not native Core Audio proof.

`bash adapters/macos/build.sh` exits 2 on Linux with an explicit unavailable error.
Native macOS compilation, actual audio, denied/revoked permission behavior,
tap-only aggregate support on 14.2, cleanup/crash behavior, sustained fidelity and
clock tests, Bluetooth controls/profile preservation and external app compatibility
are **pending**. No known Mac host was available. There is no production-quality
claim, virtual microphone qualification, or passing dependency/hardware gate.
