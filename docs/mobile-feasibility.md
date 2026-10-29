# Mobile app feasibility

Evaluated 2026-10-29 against repository revision
`1ff3f290ff969c668ab8f7363ca1eeb46245256d` and current primary platform documentation.
This supplied assessment describes that historical baseline, not device qualification.
Accepted product work is now tracked in the [current roadmap](ROADMAP.md).
Refreshed 2026-10-29 against main `cd0fc6f515d4528d7becf98b1b0ef5059b2214e3`:
read-only native inventory is integrated on Linux/Windows/macOS and bounded raw
network playout is merged; neither establishes live capture/playback or a mobile
port. Shared desktop lifecycle IPC still needs separation/gating for mobile.
No physical mobile or revised laptop test was performed. The laptop is offline;
continue on the homelab without laptop access or SSH.

Android and iOS can become useful Mixyard controllers, playback endpoints and
microphone sources. The mixing model need not be reduced for mobile. Access to
other apps, physical routes and background execution has platform-specific limits;
a general replacement for the desktop audio subsystem is a different proposition.

## Current foundation

- [The web console](../apps/web/README.md) already provides shared Vue controls,
  stored routing intent, profiles, inspection and offline DSP. Narrow-layout tests
  exist, but do not qualify touch use or mobile browsers.
- [The service](web-service.md) binds only to `127.0.0.1`, checks the exact Host and
  Origin, and provides a process session/CSRF token. That token is available from
  `/api/session`; it is not remote-user authentication. Phones cannot presently
  connect directly. A reverse proxy alone does not establish device authorization.
- [The network crate](network-audio.md) has paired authenticated QUIC, directional
  stream permissions, provenance, raw PCM and bounded queues. It is independent
  of the root crate. Opus, production pairing/key management, discovery, native
  playback/capture, production pacing/drift adaptation and adaptive jitter remain pending.
  [Bounded playout](network-playout.md) now supplies explicit local sample scheduling
  and revocation, without qualifying a hardware callback or synchronized clocks.
- [The PCM engine](audio-core.md) supports Float32/Float64 processing, and the
  [DSP engine](dsp.md) supplies processing algorithms. Both need native hosting.
  The synchronous mixer currently requires aligned mono/stereo sources at one
  nominal rate; transport supports a wider set of raw formats/layouts.
- Directly importing the root crate into a mobile target currently fails:
  [runtime/mod.rs](../src/runtime/mod.rs) explicitly supports only Linux, macOS and
  Windows. Shared processing/policy must be separated or appropriately gated
  from desktop lifecycle IPC. The desktop shell also assumes local runtime IPC,
  tray and window behavior. It is not a mobile app waiting to be packaged.
- [Config selectors and targets](../src/lib.rs) have no explicit network-peer
  variant. Integration needs an identity/provenance contract rather than treating
  received audio as an unrelated local app or microphone.

Read-only [runtime inventory](runtime.md) is now available through the desktop
OS providers, subject to their documented coverage limits. It does not enable audio.

No current client applies live audio routes. A phone controller could initially edit
the same stored intent as the desktop browser; live controls depend on the
shared runtime work, independently of mobile support.

## Useful roles and limits

The table describes normal third-party apps using public APIs, without root,
jailbreak, privileged firmware or special external audio hardware.

| Role | Android | iOS and iPadOS |
| --- | --- | --- |
| Remote controls, profiles, status and diagnostics | Feasible; mostly shared UI and a secure remote control service | Feasible; same approach |
| Play a received mix through a phone, headset or USB interface | Feasible with native playback and actual route/format discovery | Feasible with an audio session and supported hardware/routes |
| Send microphone or supported external input | Feasible with capture consent and service lifecycle handling | Feasible with capture consent and audio-session lifecycle handling |
| Send files or audio generated/decoded inside Mixyard | Feasible; no need for cross-app capture | Feasible; no need for cross-app capture |
| Capture another app's playback | Conditional AudioPlaybackCapture, Android 10+ | User-started broadcast/capture paths; newer ScreenCaptureKit path requires iOS/iPadOS 27+ |
| Independent stems from every running app | No: source policy and eligible usages restrict capture | No demonstrated general desktop-style per-app enumeration and capture |
| Present a generic virtual microphone to arbitrary apps | No supported general solution identified for an ordinary app | No generic selectable endpoint identified; a narrow call-injection feature exists |
| Capture arbitrary telephone/VoIP call audio | Ordinary playback capture does not cover voice-communication usage; privileged cases are different | Not established by screen capture or call injection; qualify specific supported workflows |
| Mix and process locally without a desktop | Feasible architecture; callbacks, clocks and actual CPU/thermal budgets need qualification | Feasible architecture; same qualification requirements |

### Android app capture

[AudioPlaybackCapture](https://developer.android.com/media/platform/av-capture)
requires capture permission, user-approved MediaProjection and the same user
profile. It admits media/game/unknown usages only when the source's capture
policy allows it. UID filtering can select eligible app audio. This is meaningful
source support, but does not bypass opt-outs or provide arbitrary call capture.
Capturing copies playback; it does not automatically redirect or silence the
original output. Avoid promising compatibility with particular music/call apps
before testing their actual policies.

Microphone use can also conflict with other apps:
[Android input sharing](https://developer.android.com/media/platform/sharing-audio-input)
may leave one ordinary recorder receiving silence, and calls have priority.
Starting a mobile call while streaming the phone microphone is therefore a
specific interruption case to test, not guaranteed simultaneous capture.

### Apple capture and injection

[ReplayKit](https://developer.apple.com/documentation/replaykit) provides
user-started recording/broadcast workflows and app/microphone sample buffers,
including a broadcast extension path for system broadcasting. Apple explicitly
documents incompatibility with AVPlayer content. It is not a universal source of
independent application stems.

Current [ScreenCaptureKit documentation](https://developer.apple.com/documentation/screencapturekit)
adds iOS/iPadOS 27 availability and offers a different capture architecture
from ReplayKit's broadcast-extension model. The [iOS sample](https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-on-ios)
uses a system picker, full-display or current-app capture, and dedicated
background modes. That sample is explicitly marked **Beta Software**; verify the
final SDK/OS before selecting a deployment target. Do not infer that the Mac's
programmatic application/window capture surface exists on iOS.
The documented capture output supports
[8/16/24/48 kHz](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/samplerate)
and [mono/stereo](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/channelcount).
This capture path cannot promise arbitrary original-source format preservation.

iOS 18.2+ also supports
[adding app audio to calls](https://developer.apple.com/documentation/avfaudio/adding-synthesized-speech-to-calls).
It requires the global accessibility setting, an app permission and an available
voice-processing call. The documented spoken-audio mode adds app playback to
microphone input and plays it locally. This is a potential separately qualified
communication feature, not a generic virtual microphone or evidence of call
return capture. Fit for a general remote music/mix workflow remains unresolved.

Another future avenue is
[AVSystemRouting and MediaDevice](https://developer.apple.com/documentation/avsystemrouting/routing-and-streaming-media-to-remote-devices):
iOS 27+ can expose nearby playback devices through a dedicated media extension
and route-aware application integration. This merits a separate experiment for
phone-to-Mixyard playback. It does not establish passive capture of arbitrary
apps, and the documented deployment app has a restricted role.

## Browser client versus native endpoint

A browser or PWA is the smallest useful controller: reuse the web UI and keep
the authoritative state on the host. HTTPS and explicit device authorization
need to precede remote access. Scoped capabilities should distinguish control,
audio reception and microphone/source publication; permission for one should
not imply permission for the others. Keep revision/conflict handling and explicit
consent for sensitive routes.

Browser microphone capture is possible in a
[secure context with consent](https://www.w3.org/TR/mediacapture-streams/).
Browser audio reception is also possible, but requires a new compatible audio
edge. [WebTransport](https://www.w3.org/TR/webtransport/) uses web transport
protocols over HTTP rather than arbitrary custom QUIC ALPN; it cannot connect
directly to the existing `mixyard-audio/1` endpoint. It also does not expose the
foundation's client-certificate pairing mechanism. A WebTransport gateway or a
separate WebRTC edge would need explicit client authentication and permissions.

For a full endpoint, a native audio layer is the stronger default:

- [Web Audio exposes Float32 PCM](https://www.w3.org/TR/webaudio/#audio-sample-format)
  and has context-rate conversions. A Web Audio playback/capture edge therefore
  cannot preserve arbitrary S32/F64 detail. Transporting opaque bytes or doing
  Float64 arithmetic in WebAssembly is a separate capability; it does not change
  the playback API's format. Any narrowing belongs to the explicit browser edge.
- [Screen Capture](https://www.w3.org/TR/screen-capture/) makes audio optional,
  user-agent dependent and user-selected. It is no portable mobile guarantee of
  access to other apps' audio.
- A PWA install does not grant native background services. A
  [service worker can be terminated](https://www.w3.org/TR/service-workers/#service-worker-lifetime)
  and is not a substitute for an audio service. Browser background/lock-screen
  reception and capture need tests on each actual OS/browser/version; neither
  universal failure nor uninterrupted operation should be assumed.

[Tauri 2 mobile plugins](https://v2.tauri.app/develop/plugins/develop-mobile/)
can connect Vue/Rust to Kotlin/Swift and use JNI/FFI while the webview is suspended.
That makes a separate mobile shell with shared code a reasonable first candidate.
Native code must own audio sessions/services, callbacks, network workers and
interruption recovery; audio samples should not travel through JSON/webview IPC.
Mobile compilation and background behavior have not been demonstrated here.

## Quality, routes and lifecycle

Mobile is not intrinsically limited to low-quality compressed audio. The raw
transport can preserve admitted bytes; actual capture/playback depends on the
selected OS API and hardware. Apple's
[preferred sample rate](https://developer.apple.com/documentation/avfaudio/avaudiosession/preferredsamplerate)
is a request, so query the active rate. Android's
[Oboe guidance](https://github.com/google/oboe/blob/main/docs/GettingStarted.md#obtaining-optimal-latency)
likewise requires actual stream parameters. Declaring a high network rate does
not prove that the microphone, capture tap, mixer or DAC provides it.

There is no inherent one-stream or one-mix mobile limit. Multiple named network
streams and local processing are feasible within admitted memory, bandwidth and
CPU budgets. Arbitrary simultaneous physical outputs are different: OS routing
and hardware decide what can be opened. For example, iOS 26.2+
[dualRoute](https://developer.apple.com/documentation/avfaudio/avaudiosession/mode-swift.struct/dualroute)
supports built-in I/O plus a compatible second full-duplex route. It is a useful
specific capability, not an unlimited device graph. Bluetooth route/profile
changes, microphone use and local playback need separate qualification.
On Android, a
[preferred routing device](https://developer.android.com/reference/android/media/AudioRouting)
is a request; the observed route remains authoritative.

Lock-screen and headset control of Mixyard's own playback session is feasible.
[Android media-session access](https://developer.android.com/reference/android/media/session/MediaSessionManager)
can also expose other players with notification-listener authorization;
[Apple's remote command center](https://developer.apple.com/documentation/mediaplayer/mpremotecommandcenter)
handles commands for the app's own media. Neither establishes reliable
origin-device attribution and exclusive dispatch for Mixyard's device-specific
media-control policy. Explicit phone controls can instead send scoped commands
to a host that supports them. Muting the phone output and pausing a shared
upstream player remain distinct actions.

Native background audio is supported, subject to lifecycle rules. Android has
[playback, microphone and projection foreground-service types](https://developer.android.com/develop/background-work/services/fgs/service-types);
background microphone service startup is restricted even when ongoing capture
is allowed. [Audio focus](https://developer.android.com/media/optimize/audio-focus)
can interrupt or fade playback. iOS uses
[audio background modes and audio-session categories](https://developer.apple.com/documentation/avfaudio/configuring-your-app-for-media-playback).
Neither platform promises an unrestricted always-running desktop daemon.

Wi-Fi jitter, power management, network changes and independent hardware clocks
need explicit handling. A reliable receiver needs paced production, bounded
jitter storage, stale-data rejection and clock-drift adaptation. Two phones
playing the same mix are not automatically synchronized. Physical end-to-end
latency must be measured; application RTT and queue duration are not substitutes.
Wired/USB and Bluetooth routes must be measured separately.

## Recommended sequence

1. Add an opt-in authenticated remote control edge, initially LAN/Tailscale,
   using the existing web interface. Keep local IPC private; do not turn the
   existing session token into a remote login mechanism.
2. Integrate one real network source/output with the shared graph and qualify
   paced native delivery. This is common work for desktop and mobile endpoints.
3. Build a native mobile receiver and microphone sender, reusing transport and
   policy/DSP where practical. Receiving can start as a limited peer without
   requiring a complete local mixer. Keep original stream identity and consent.
4. Add eligible Android app capture. Investigate Apple broadcast/capture and
   route extensions as explicit version-specific features, separately from the
   cross-platform baseline.

The first useful acceptance scenarios are phone control of saved intent;
desktop mix to phone headphones; phone microphone to a desktop mix; and a
simultaneous send/receive path with mix-minus. Verify screen lock, UI suspension,
call interruption, permission revocation, route unplugging, network loss/change,
stale-stream silence and sibling-output independence on physical devices.
Measure actual latency, drift, dropouts and power/thermal behavior before claiming
production quality. These tests have not been run for this assessment.

The remaining product choices are supported OS versions, priority between
controller/receiver/source roles, particular third-party apps to qualify, and
whether standalone mixing or synchronized multi-device playback is required.
None blocks evaluating or beginning the common controller/endpoint foundation.
