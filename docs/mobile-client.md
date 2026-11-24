# Native mobile client contract

The Kotlin Android project and Swift iPhone/iPad project under
[`apps/mobile`](../apps/mobile/README.md) provide native controller shells and
executable worker contracts. Both embed the existing Vue console. No new mixer
model, desktop IPC import, root Rust change or personal audio graph is introduced.
This foundation does **not** implement mobile network audio or qualify mobile
hardware. The [roadmap](ROADMAP.md) and [mobile assessment](mobile-feasibility.md)
retain the full receiver/source/duplex/standalone outcome and platform limits.

## Shared controller and trust

The shell supplies `Api.forControl` with the narrowly scoped `MobileBridge.fetch`.
Only `/control/session` and `/control/request` reach this adapter. The shared API's
required credential slot contains the public sentinel `native-owned`; it is never
a remote credential. Native code retrieves the real bearer from secure storage
and permits only `/control/{pair,session,request,credentials,rotate,revoke}` at the
**one configured exact HTTPS origin**. This version uses pair/session/request;
credential-management UI is a later shared workflow. No HTTP 8384 fallback,
identity headers, local CSRF forwarding, cookie authentication, redirects,
automatic trust-on-first-use, public relay or wildcard endpoint exists.

The user enters the target identity, host-issued invitation and independently
verified leaf-certificate SHA-256 hex fingerprint into native controls. Pairing
requires both platform PKI/hostname validation and that exact leaf pin. A pin does
not turn an otherwise untrusted/self-signed certificate into a trusted one. Pin
or certificate rotation requires explicit new trust/pairing. Android keeps the
AES-GCM key in Keystore and only ciphertext in app-private preferences with backup
disabled. iOS uses `WhenUnlockedThisDeviceOnly` Keychain items. Neither sends
invitations or bearer credentials to the WebView, localStorage, asset bundle or
logs. Pairing replaces the currently selected native target; no discovery or audio
activation is implied. Multi-target native provisioning/rotation is not yet built.

The frozen v1 wire requests are JSON POSTs:

| Route | Body | Reply |
| --- | --- | --- |
| `/control/pair` | `{deviceId,invitation,name}` | `{deviceId,credentialId,credential,permissions}` |
| `/control/session` | `{}` plus native bearer | `{deviceId,authorityId,revision,permissions,backendAvailable:false,audioApplied:false}` |
| `/control/request` | `{deviceId,authorityId,revision,path,body}` plus native bearer | `{deviceId,authorityId,revision,status,body}` |

Permissions `view`, `control`, `settings` are independent grants; native validation
never invents implication. `authorityId` is an opaque string. Revision must be an
exact nonnegative JSON integer no larger than `9007199254740991`; unsafe numeric
responses cannot silently round into another authority. Canonical status equals
outer HTTP status. Protocol errors have `{error:{code,message}}`, never a fake
canonical envelope. Existing profile/revision/sensitive-route validation remains
on the owning service. Remote review/live/arm remain unsupported by shared `Api`;
native code also refuses review/arm. Management target selection is unrelated to
an audio destination.

Android uses bundled assets at `https://appassets.androidplatform.net`, disables
file/content/DOM-storage access and restricts the origin-aware WebMessage listener
to that origin and the main frame. Requests outside bundled assets are blocked.
iOS uses a nonpersistent WKWebView with read access restricted to its bundled
controller directory; the reply handler accepts only its exact main-frame index
file. Both deny foreign navigation. CSP blocks browser network connections;
only the native pinned client can contact the host. The platform bridges follow
[Android's origin-limited messaging](https://developer.android.com/develop/ui/views/layout/webapps/native-api-access-jsbridge)
and [Apple's request/reply handler](https://developer.apple.com/documentation/webkit/wkscriptmessagehandlerwithreply).

## Session and lifecycle safety

Every native bridge message captures the native epoch **before** it enters the
bounded worker/async task. A queued write cannot acquire another device's session
when it later runs. The caller's complete frozen identity/revision is checked
again against native authority before HTTP dispatch. Credentials/configuration
are captured for that exchange; responses must match device and authority and
cannot regress revision. The JavaScript adapter queues at most eight exchanges
and serializes the console's parallel refresh reads. A queued request whose
revision changed is rejected, never silently rebased onto the new revision.

Switch/pair, local forget, explicit disconnect, UI stop/background, interruption
or route change invalidates the relevant session and endpoint generation.
Android's Activity stop covers UI suspension; iOS observes scene phase, audio
interruption and route-change notifications. The old Vue tree is unmounted to
clear draft/arm authority from the active UI. Credentials already stored for an
unchanged pairing stay native until explicit forget; pending pairing results
cannot persist after epoch invalidation. Reopening is explicit and establishes
fresh authority. No automatic writes, replay, playback, microphone or arming.
A write already accepted by a server cannot be recalled by discarding its reply;
after uncertainty, refresh/review rather than retry blindly.

No audio hardware is opened in this foundation. Before enabling each native role,
its worker must acquire explicit consent and exact route/format admission. Audio
focus loss, call interruption, route unplug/profile change, permission/peer
revocation, network handover, restart and background restrictions must invalidate
workers and clear owned queues; resumption needs renewed admission. Android
foreground microphone/media-service permissions and iOS audio-session/background
modes are intentionally not declared as working: their workers are not linked.
Capture cannot start just to discover devices or display meters.

## Native PCM and shared core

PCM never crosses JSON or WebView messaging. Kotlin/Swift worker-only buffers
carry opaque packed interleaved little-endian S16/S24/S32/F32/F64 bytes with the
exact positive u32 sample rate and ordered unique nonempty channel IDs. No float
arithmetic, resampling, channel sorting/downmix, normalization or hidden fallback
occurs. Raw-file sources require a caller-selected exact descriptor and reject an
incomplete final frame without padding/truncation. Encoded/compressed file decoding
is unsupported. The finite generated source repeats an explicitly supplied raw
frame; it is a synthetic byte oracle, not a measured capture clock.

Each queue has immutable format/generation, explicit frame capacity and exclusive
worker ownership. Submissions include generation, sequence, first-frame offset,
local monotonic deadline and observation time. Replay, wrong generation, overflow,
partial frame or invalid clock terminates and clears that queue. Consumption
checks expiry too. Missing/inactive output is zero; a failed sibling cannot stop
an independent queue. Kotlin/Swift queues allocate worker storage and are **not**
qualified allocation-free callback or cross-thread SPSC implementations.

Native provenance admission rejects duplicate original sources, peer-path cycles
and any upstream premix carrying an excluded group. Mix-minus must be built
upstream: already summed call return cannot be removed by relabeling the receive
stream. These are admission checks, not a second standalone mixing/DSP graph.

The native lead owns mobile-safe root feature gating/extraction. The required
shared imports are canonical policy, audio, PCM, DSP, latency and
`runtime::worker`, compilable as `--no-default-features --lib` for
`aarch64-linux-android`, `aarch64-apple-ios` and `aarch64-apple-ios-sim` in hosted
qualification. Its `Queue` v1 `Format` is f32/f64 with exact rate/ordered channels;
`Binding {generation,revision}` is immutable and is not authorization. `Stamp`
uses the explicit engine monotonic clock domain. The mobile raw-byte descriptor
is separate and does not silently narrow integer/encoded sources to that engine.
The exclusive Rust handoff still needs a linked native ABI, consent/provenance
integration and separately qualified callback/clock/transport wiring. This PR
links neither root desktop IPC nor a pretend shared mixer. Every audio role
(receiver, microphone, external, file/generated sender, duplex, standalone DSP)
returns `shared-core-and-transport-not-linked`, with both availability flags false.

## Executable evidence and remaining gates

The Node suite runs the actual shared `Api` through `MobileBridge`: independent
permissions, unsafe revisions, canonical permission denial, remote arm refusal,
wrong target/authority, queued-write revision changes, concurrent refresh reads
and pending completion invalidation. It supplies synthetic native-boundary replies;
it does not prove native TLS or a phone-to-host service path.

Hosted qualification runs Kotlin JVM and Swift native raw-byte/session/provenance
contracts (all five precisions, unusual rates/order, low-bit Float64 generated/file
loopback, sibling revocation, terminal overflow/replay/expiry and frame tails),
Android SDK compile/lint and an unsigned iPhone/iPad simulator SDK build. These
checks must actually pass at the delivered revision before SDK compatibility is
claimed. No physical audio, signing, deployment or store publication is run.

Still required: both real WebViews and touch/accessibility flows; paired native
TLS against the production service (pin mismatch, invalid PKI/hostname, redirects,
protocol errors, remote revoke/restart and key storage failures); credential
rotation/multiple targets; Rust/native ABI and QUIC connection; real receiver,
consented mic/external/file/generated sender and simultaneous duplex mix-minus;
standalone DSP; route/focus/background/permission/device-lock recovery; hardware
clocks/pacing/drift, Bluetooth mode changes, native latency/dropout/endurance,
power/thermal behavior and per-platform signing/store privacy qualification.
Generic virtual microphone, arbitrary call capture and unrestricted per-app
capture remain unsupported. A hosted compile or fixture is never physical proof.

### Native control JSON boundary

Controller requests cross the native boundary as the original complete JSON
string. Kotlin/Swift parse a separate view to check the device, authority,
revision and route, then send the original UTF-8 bytes. Replies return
`{status, json}` with the original response string. WebMessage/WK messages carry
that string, so neither platform JSON serialization nor `Response.json` can
normalize canonical signed zero or other finite Float64 values. PCM still uses
native binary workers, never this control channel.

Unwrapped HTTP permission denials retain their status/code/message and preserve
unrelated grants; 401/409 invalidate native and shared authority. Transport
failures clear the shared API instead of leaving a connected indicator. Swift's
production default vault remains Keychain; `CredentialVault` injection is a
platform testing seam, with no plaintext or memory production fallback.
