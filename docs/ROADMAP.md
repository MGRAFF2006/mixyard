# Mixyard current roadmap

Updated 2026-10-29. Baseline: main
`cd0fc6f515d4528d7becf98b1b0ef5059b2214e3`. This is the current priority and
acceptance index; [PLAN](PLAN.md) retains the detailed product/architecture
contract, [STATUS](STATUS.md) records evidence, and the linked workstreams retain
technical detail. Stages express dependencies, not dates or a fixed feature cap.
Device/native-client/mobile scope is tracked in
[#59](https://github.com/MGRAFF2006/mixyard/issues/59).

Native audio (1) and secure stored-intent control (2) can progress in parallel.
Desktop client work (3), protocol feasibility (7), and existing offline DSP work
can also proceed independently. Mobile control starts with (2); native mobile
audio depends on the endpoint/clock work in (1)/(4), not on finishing every
desktop feature first.

Build the maximum feasible feature set on Linux, Windows and macOS in parallel;
qualify each independently. Android/iOS/iPadOS join as controllers and native
audio participants, with actual API limits visible. Hands-on native testing
currently focuses on Linux. The laptop is offline: use homelab/hosted/fixture
work for now, no laptop access or SSH. Physical and mobile qualification stay open.

## Implemented foundations, not a finished audio router

| Area | Present foundation | Required next proof |
| --- | --- | --- |
| Web console | Local profiles/drafts, inspection, DSP/channel/delay workbench, situation builder/settings/reviewed stored-intent selection | Live audio control, remote device management and authenticated remote access |
| Desktop app/tray | Existing Tauri/Vue shell, local lifecycle/profile clients, three-OS builds and scoped Linux webview evidence | Shared current web workflows, native tray/desktop UX, live backend and remote target control |
| Policy, PCM and DSP | Independent sends/outputs, Float32/64 processing, PCM codecs, selected-channel matrices and offline effects | Wider native layouts, timeline/clock adaptation and safe live insertion |
| Runtime discovery | Bounded read-only Linux/Windows/macOS inventory, private IPC, durable revision ownership | Complete source identity/format coverage and actual native stream transport |
| Network audio | Authenticated raw QUIC, directional permissions, bounded packetization/playout/revocation and RTT/queue observations | Production pairing/discovery, native consumer delivery, pacing/drift/jitter and compression |
| Situations | Typed decisions, explanations, declarations/settings and guarded stored-profile ownership | Broader genuine platform observers and qualified live application |

`backendAvailable` and `audioApplied` remain false for native audio. Hosted SDK,
IPC and fixture success must not become a physical-audio compatibility badge.

## 1. Finish native audio and precision preservation

Priority: unblock a real source → independent mixes → real consumers on all three
desktop OSes. Own bounded capture/render workers, virtual inputs/outputs, exact
identity binding, independent output queues, lifecycle recovery and consent.
Prove multiple selectable consumer inputs and application capture separately;
retain destination-specific mix-minus and prevent duplicate original playback.

Preserve source precision, rate and ordered channels locally and over the network.
Convert only when explicitly requested, required by processing or required by
that particular destination; a limited sink cannot narrow sibling routes. Extend
native layouts/rates and encoded/DSD paths deliberately, without invented maxima.
Measure hardware clocks, resampling/drift, negotiated formats and actual consumer
output. Test null/file consumers in isolated Linux sessions before physical runs.

Tracks: [#51 native integration](https://github.com/MGRAFF2006/mixyard/issues/51),
[#29 format preservation](https://github.com/MGRAFF2006/mixyard/issues/29).

## 2. Manage any paired Mixyard device from every interface

This is a shared product feature, **not a mobile-only panel**. The browser,
Windows/macOS/Linux app, desktop quick controls and future mobile clients can
select a paired desktop, laptop, server, phone or tablet as the management target.
Managing another device does not require sending audio to it. Discovery/control,
audio receive and audio-source publication are separate granted roles.

- Add an explicit opt-in authenticated remote-control service and production
  pairing, encrypted sessions, credential rotation/revocation, device identity
  verification and scoped view/control/settings rights. LAN/Tailscale are initial
  connection options, not substitutes for application authorization. Keep local
  IPC private. The existing loopback session/CSRF token is not remote login;
  exposing its port or adding a reverse proxy alone is insufficient.
- Provide a persistent device selector and clear target name/identity, connection,
  capability and actual runtime state. Show which host owns profiles, automation,
  DSP and outputs; distinguish the controlled host from an audio destination.
  Local and remote editing use the same full controls where granted/supported.
- Include device status/meters, mixes/routes, profiles, DSP/channels, situations,
  source/output capabilities and diagnostics. Stage live controls behind actual
  backend support; until then explicitly label stored-intent edits. Permission
  and unsupported-capability explanations must be actionable.
- Scope drafts, revision tokens, reviews, arm authority and pending responses to
  the selected device/session. Switching/reconnecting cannot retarget a queued
  write or apply an old response. Handle stale revisions, offline recovery and
  competing controllers without silent overwrite; retain unsaved drafts per host.
- Pair/unpair, name devices and inspect/revoke sessions/permissions. Copy profiles
  only explicitly, with identity remapping and sensitive-route review; do not copy
  secrets or turn a source credential into administrative authority. A device
  switch, remote connection or restored session must not silently open a mic,
  enable speaker playback, arm automation or change OS defaults.
- Later add explicit device groups and coordinated profile/routing operations,
  with named targets, per-device outcomes, timeout/cancel/reconciliation and
  partial-failure handling. Do not promise atomic transactions across hosts.
  This manages Mixyard, not arbitrary OS shells, remote desktops or power state.

Acceptance: control server B from desktop A and a phone browser, switch A/B during
an in-flight request, revoke that controller, reconnect after restart, and prove
that B's edits never alter A. Test authorization and sensitive-route consent with
real service boundaries before advertising remote control.

## 3. Complete native desktop apps and quick controls

Keep the full native-app goal on **Windows, macOS and Linux**. Extend the existing
Tauri/Vue prototype; share the current console's components/contracts rather than
maintaining a second mixer implementation. The window and tray are clients of an
independent runtime. Closing the UI and stopping the runtime are separate actions.

| Platform | Planned quick interface | Required fallback/qualification |
| --- | --- | --- |
| Windows | Notification-area menu/panel: device target, profile, levels/mutes, status, open full app | Explorer/restart, DPI, keyboard, assistive technology and notification behavior |
| macOS | Menu-bar controls plus full native app/Dock behavior | Menu-bar/Dock/window lifecycle, permissions, accessibility, signing/notarization |
| Linux | StatusNotifier/AppIndicator integration on supported hosts, including compact controls where feasible | Qualify KDE/GNOME and relevant Wayland/X11 hosts separately; never assume an extension/tray host exists |

Linux stays on the roadmap despite shell differences. Baseline tray menus include
Open Mixer, target/status, profile and mute actions. Tauri does not emit Linux tray
mouse events, so rich anchored panels need a separately verified integration.
When a tray is unavailable, keep the full app, headless CLI and an optional compact
window usable without installing a desktop extension. A missing tray cannot block
mixing or remote device management. Verify native builds, installers, upgrades,
autostart opt-in and UI/runtime lifecycle; do not call a build finished UX.

## 4. Reliable network audio, compression and measured synchronization

Connect individual application/input streams and premixed buses in both directions
to the same graph. Add production pairing/discovery, explicit peer/provenance
identities, pacing, adaptive bounded jitter, clock-drift handling, native callback
handoff and independent stream/output recovery. Measure path delay and offer
explicit alignment across local/network routes with validity/uncertainty shown.
Application RTT, queue duration and clock offset are different measurements;
never manufacture one-way or physical latency from RTT/2.

Keep three explicit quality choices: raw; bit-preserving lossless (evaluate opaque
block compression such as Zstd and FLAC only for compatible integer PCM); and
optional lossy (Opus first candidate). No automatic lossy fallback or hidden rate,
precision/channel reduction. Negotiate actual capabilities, bound decoder/work
budgets, and show bitrate, CPU, latency and conversions. Broader codec choices
follow compatibility, licensing and exact-format tests. No compression is claimed
implemented by this roadmap.

Details: [network audio](network-audio.md), [playout](network-playout.md),
[timing/alignment](latency.md), [#29](https://github.com/MGRAFF2006/mixyard/issues/29).

## 5. Android, iOS and iPadOS as full participants

Reuse the Vue controller where practical; evaluate a native shell with shared Rust
policy/transport/DSP and Kotlin/Swift audio plugins. Extract/gate desktop-only IPC
before claiming a mobile build. Native workers own audio and background lifecycle;
sample buffers do not travel through JSON/webview IPC. No arbitrary mobile-only
cap on mixes, streams or controls: use honest resource/capability admission.

| Role | Planned delivery | Capability boundary |
| --- | --- | --- |
| Full controller, installable web/PWA option | First mobile milestone, sharing stage 2 with desktop/web clients | Authenticated remote service, touch/keyboard/accessibility and real browser tests |
| Audio receiver | Native playback to explicitly selected supported headphones, USB, Bluetooth or speakers | Actual route/format, focus, buffering, interruptions; playback is opt-in |
| Microphone/external-input sender | Native consented capture to a selected peer/mix | Permission, capture contention, foreground/background restrictions and revocation |
| Files/generated audio sender | Send supported local sources without cross-app capture | Explicit file access, decode formats and route permissions |
| Simultaneous send/receive | Desktop mix to phone headphones plus phone mic to desktop | Independent paths, provenance, mix-minus, feedback prevention and lifecycle recovery |
| Standalone mixer/DSP/network peer | Local mixes and effects without a desktop; multiple admitted streams | Measured callback/CPU/memory/thermal/battery budgets; slow effects remain available with warnings/offline paths |
| Other-app audio capture | Android eligible UID-filtered playback capture; Apple user-started broadcast/capture workflows | Source opt-outs, content/usage restrictions, consent and actual OS-version support |
| Specific Apple extensions | Investigate iOS 18.2+ supported-call audio addition, iOS/iPadOS 27+ ScreenCaptureKit and AVSystemRouting/MediaDevice, and supported dual-route configurations | Separately qualified optional workflows; beta/final SDK and app-role restrictions remain gates |
| Lock-screen/headset media control | Own playback session; optional authorized Android media-session access and explicit commands to a capable host | Preserve origin/dispatch ownership; app-wide player transport differs from muting one output |
| Browser audio edge | Optional consented mic/receive via authenticated WebRTC or a WebTransport gateway | Not a direct connection to custom QUIC/client-cert pairing; Web Audio Float32/rate conversion is an explicit destination constraint |

Investigate cooperative app-to-app/OS-specific audio injection where public APIs
permit it. A generic selectable virtual microphone for arbitrary mobile apps and
arbitrary call capture are **unsupported research questions**, not promised baseline
features; no root/jailbreak or permission bypass is assumed. Android opt-out is not
overridden by user projection consent. Apple's narrow call-audio addition is not
a generic virtual microphone or evidence of capture from the call return.

Plan mobile background playback/capture services, permission/focus/call recovery,
lock screen/UI suspension, route unplugging, network handover and peer revocation.
Qualify actual routes, raw-quality support, independent clocks, multi-device sync,
power/thermal behavior and Bluetooth mode changes. Do not auto-enable a mobile
mic for discovery/meters. UI simplicity follows beginner presets, complete everyday
controls and optional expert detail; advanced options preserve the full model.

The [supplied mobile feasibility assessment](mobile-feasibility.md) records source
links and version-specific limits. No physical mobile testing is complete.

## 6. Extend live DSP, channel tools and situation automation

Attach the existing DSP/channel/delay workbench to actual sources, buses and
outputs after callback/timeline qualification. Add smoothing/automation, advanced
dynamics/convolution, calibrated loudness/meters and plugin hosting where feasible;
retain complete parameters and visible processing-budget warnings. Slow processing
is not a reason to remove a feature or silently lower quality.

Broaden genuine device/application/power/network/time/manual observers across OSes,
keep unknown distinct from absent, and connect reviewed situation transitions to
qualified live routes. Retain manual override, settle/hold/cooldown, sensitive-path
review and revisioned ownership. Remote management uses the same local authority
checks; reconnect never revives stale arming.

Tracks: [#33 DSP](https://github.com/MGRAFF2006/mixyard/issues/33),
[#40 situations](https://github.com/MGRAFF2006/mixyard/issues/40).

## 7. Bidirectional casting, streaming and multiroom interoperability

Implement sending and receiving/advertising independently for each supported role:
UPnP AV/DLNA, Google Cast, AirPlay/RAOP, Spotify Connect, HTTP, WebRTC,
RTP/SRTP/RTSP, Snapcast, AES67/RAVENNA and contribution/professional candidates.
Browser Remote Playback is an API surface, not a universal protocol. Vendor,
account, licensing and certification gates stay visible. Live PCM forwarding,
URL-fetch playback and service control must have different UI labels. Extend
[the interoperability matrix](network-interoperability.md) to mobile capabilities
and Apple route-extension experiments without promising every role on every OS.

Track: [#56](https://github.com/MGRAFF2006/mixyard/issues/56).

## 8. Qualification, packaging and usable releases

Ship evidence-backed capabilities, not a blanket supported-platform checkbox.
Keep Linux physical-session tests independent of Windows/macOS hosted SDK/IPC
checks, and add physical Android/iOS tests before mobile claims. Exercise format
preservation, dropout/leakage/channel integrity, recovery, clocks, latency and
bounded memory under sustained load. The detailed plan's endurance/latency targets
are targets, not measurements. Test simultaneous GUI/tray/CLI/remote controllers,
small screens, offline/restart recovery and assistive technology.

Finish safe presets/onboarding, actionable diagnostics/export, supported recording
consumers and eventually a consented recoverable recorder; qualify Bluetooth
media/mic/call controls per route/app. Deliver per-OS packaging, signing,
notarization, updates and rollback after licensing/privacy checks. Test mobile
background modes, store entitlements and supported versions separately. Keep core
and headless CLI independent of desktop UI libraries. No date is promised before
these acceptance gates can be met.
