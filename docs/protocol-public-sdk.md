# Public browser SDK interoperability profile

Updated 2026-10-29. Own implementation: MIT, as in the repository LICENSE.
Status: **implemented with mocked SDK contract evidence**, plus owned IPv4
loopback HTTP signaling ingress. No browser media, vendor network, account,
physical receiver, desktop receiver certification or audible output was executed.
`backendAvailable` and `audioApplied` remain false. This optional package is not
wired into the native runtime or web console.

The dependency-free ES modules live in
`interoperability/mixyard_interop/public_sdk/index.mjs`. They invoke production
browser/official SDK APIs through caller injection; no vendor code is bundled,
downloaded, installed or loaded by this package. Tests replace only these external
SDK/browser boundaries. They exercise the production adapters, not a second
fixture implementation. Parent integration remains responsible for the live
host policy and browser deployment.

## Implemented roles and limits

| Adapter | API calls and implemented role | Explicit unavailable gates |
| --- | --- | --- |
| `CastSender` | Adopt a caller-selected official CAF `CastSession`; construct `MediaInfo`/`LoadRequest`, call `loadMedia` with autoplay disabled; callback-based media play/pause/stop and session volume; end the owned session on revoke. | No SDK loader, autojoin, receiver discovery or session launcher; caller must present official Cast UI. No certified desktop receiver, arbitrary PCM sink, or media delivery proof. |
| `SafariAirPlay` | Observe `webkitplaybacktargetavailabilitychanged`, invoke the native route picker synchronously, play/pause and set volume on one already approved media element; remove observer and pause on revoke. | Safari feature detection and user activation required. No URL assignment, capture, RAOP implementation, receiver advertisement or forced route selection. A paused selected route may remain selected in Safari. |
| `SpotifyPlayer` | Instantiate injected `Spotify.Player`, register ready/error listeners, obtain ephemeral OAuth through an injected callback, connect/disconnect and expose the ready device ID. Invoke activate, resume, pause, next/previous, seek and volume methods. | Service-only: no arbitrary PCM input/export, content mixing/alteration, audio capture or certified hardware receiver. No Web API playback transfer/track selection implementation. Account, application access, browser DRM/autoplay and vendor policy remain gates. |
| `WebRTCAudio` | Native `RTCPeerConnection`, one sendonly or recvonly audio transceiver with explicit codec preferences, selected track clone, offer/answer, authenticated injected signaling, candidate admission, remote-track handoff and cleanup. | Cooperating application only. No capture permission request, playback element, Web Audio, microphone, arbitrary peer discovery, relay, LAN/WAN or production signaling service. No real browser/media execution on this host. |

`capabilities()` reports availability of injected API surfaces independently of
qualification. Every entry includes `evidence: 'mocked-sdk-contracts-only'`,
`vendorNetworkExecuted: false` and both runtime flags false. Missing APIs and
receiver/PCM-export roles return unavailable with an explicit reason. API presence
cannot demonstrate compatible content, account eligibility or receiver reachability.

Cast media URLs must use the exact configured literal loopback authority, with
an explicit host-owned immutable no-redirect media lease and explicit destination
conversion approval. **A remote Cast device cannot fetch this host's
`127.0.0.1` URL.** Remote media playback is therefore unavailable in this profile;
contract execution is not a remote delivery claim. A broader separately authorized
media exposure profile would need receiver-reachable URLs and its own access,
redirect, DNS, network, lifetime and device qualification. No such profile is
implemented here.

AirPlay acts on an existing element whose source is pinned to the same loopback
URL profile; actual receiver fetching/reachability remains unqualified. Neither
adapter fetches a URL itself. The media server owning the lease must prohibit
redirects and playlist indirection for that resource throughout its lifetime;
a boolean passed by an untrusted page is not a valid lease or security authority.
The host authorization callback receives the exact URL and must validate lease
ownership. SDK-internal fetch behavior cannot be sandboxed by this module.

## Injection and grant contract

All adapters require an owned endpoint and peer/service `identity`. Endpoint is a
copied frozen `{host: '127.0.0.1', port: 1..65535, scheme: 'http'}` (HTTPS also
accepted by the JS boundary, but not by the current shared Python endpoint).
Cast `identity` is the selected `chrome.cast.Receiver.label`, scoped to its
browser profile/origin; it is not paired-device authentication.
SDK objects, selected media elements and their lifecycle are exclusively owned
by this adapter until revoke; do not share a player/element with another route.

`authorize(role, context)` is synchronous and must explicitly return `true`.
It defaults to denial. False, exceptions and missing grants close the owned
session. Context contains the copied frozen endpoint, identity, unchanged source
`spec`, action and relevant session/track/device identity, media URL or destination
codec. The source spec and provenance are copied and deeply frozen before async
work, never rewritten to match a lossy destination.

| Adapter | Independent roles |
| --- | --- |
| Cast | `media-fetch` for load and play; `control` for media and receiver commands |
| Safari | `control` for availability observation; both `control` and `media-fetch` for picker/media actions |
| Spotify | `service-control` for SDK creation, listener registration, token delivery and commands; additionally `advertise` for connect/ready |
| WebRTC | Directional `live-send` or `live-receive` throughout; additionally `control` for peer construction and signaling |

The host callback is a policy boundary, **not implemented pairing or Python
session transport**. A host integration can map the exact endpoint/spec to its
live session grant check and separately validate peer/track identity, exclusions,
consent and media lease. Do not give a web page an always-true callback in the
real application. Management/control authority alone cannot start a Cast media
resource or authorize WebRTC tracks. Legacy discovery provides no trust.

The host must examine the full source provenance and deny the **whole premix**
when any source group is excluded for that destination, or when visited-peer
history creates a loop. A mixed source cannot be unmixed at this bridge. Tests
prove such a callback blocks native creation/media handoff. Browser code does
not replace `Spec.from_dict`, provenance path validation, pairing or revocation
transport. The focused fixtures use the canonical shared Spec shape.

The browser metadata profile has an 8 KiB encoded limit, 512 visited nodes,
12 nesting levels and 128 entries per collection. Only JSON metadata and exact
safe integer numbers are accepted. In particular, a canonical u64 stream ID
above JavaScript's safe integer range must be rejected by this bridge; never
round it. This profile may reject larger valid shared descriptors explicitly.
No raw sample conversion, reordering or rate change occurs in these adapters.

Each native SDK side effect rechecks its relevant live role. Authorization also
runs after asynchronous preparation before further effects. Native promises
cannot be cancelled or recall an operation already admitted; on timeout/revoke,
cleanup runs immediately and again after a late completion where necessary.
No subsequent setup/play/signal/track handoff may revive that session.

The **host grant-revocation/expiry notification must call `adapter.revoke()`**
synchronously to stop an active passive stream/player, even when no next API call
occurs. An authorize callback alone cannot observe revocation between operations
or check every frame inside a vendor/browser engine. Revoke is unconditional
safety cleanup and does not require a still-live grant. Call it on session loss,
page teardown, destination switch or sensitive-route consent withdrawal. Never
reuse a revoked object; obtain fresh consent and construct a new one.

## Cooperating WebRTC profile

This is a deliberately restricted one-direction, one-audio-track, one-codec session.
`secureContext` must reflect the real browser's secure-context state; caller
injection is not a browser security bypass. Require authenticated signaling
before feeding remote bytes to `accept()`.

- Supply `localIdentity`, remote `identity`, a fresh `sessionId` and the remote's
  exact uppercase SHA-256 DTLS fingerprint obtained from that authenticated peer.
  Never trust a fingerprint simply because it arrived in the SDP being checked.
- Supply 1..4 **owned** allowed loopback media ports. SDP and trickle candidates
  admit only literal `127.0.0.1`, UDP component 1, host type and a pinned port.
  Placeholders `0.0.0.0`/port 9 are admitted in SDP connection metadata only.
  DNS, mDNS, LAN/WAN, IPv6, TCP, STUN/TURN/relay and candidate extensions reject.
- Supply `{mimeType: 'audio/opus', clockRate: 48000, channels: 2}` and explicit
  `conversionAccepted: true`. Browser codec availability is checked and
  `setCodecPreferences` invoked. SDP must have exactly one encrypted
  `UDP/TLS/RTP/SAVPF` audio m-line, that one codec, RTCP mux, ICE credentials
  and the pinned remote DTLS fingerprint. Browser-native parsing is still required.
- `selectTrack(track)` accepts exactly one already consented live audio track;
  it clones it for exclusive ownership and returns the clone's wire track ID.
  The original track is never stopped. No `getUserMedia()` call is made.
  Callback `trackId` retains the original consented source ID and `wireTrackId`
  names the owned clone. The authenticated remote must select the returned ID with
  `selectRemoteTrack(id)` before receiving. Unselected/video/second tracks stop
  and close the session. `onTrack` is a synchronous injected consumer handoff,
  never implicit speaker playback. Its owner must bound further work/queues.
- `offer()` creates and emits an offer. `accept(raw)` validates an offer/answer
  or candidate, sets remote SDP and emits an answer when needed. No renegotiation,
  ICE restart, rollback, simultaneous-offer resolution or arbitrary data channel
  is provided; construct a fresh consented session for those changes.

`sendSignal(raw)` must synchronously return `true` for accepted bounded handoff;
false or a promise closes the session as backpressure. It must not perform an
unbounded retry/queue. Wire envelopes have exactly `v: 1`, `session`, `from`, `to`
and either `description: {type, sdp}` or `candidate` with candidate text,
`sdpMid: '0'`, `sdpMLineIndex: 0`. The transport must preserve description-before-
candidate order, pin its own literal-loopback destination, reject redirects and
bound its own bytes, requests, authentication and time. No transport credentials
are placed in this metadata.

Signaling is at most 32 KiB per message, SDP 24 KiB/256 lines/2048 characters per
line, candidate text 1024 characters, 64 candidates per direction and 68 total
inbound messages per session. Candidates before remote SDP reject; no candidate
queue exists. Async SDK operations admit one in-flight operation and no queued
operations; their default timeout is 5000 ms, configurable 1..10000 ms. WebRTC
session lifetime defaults to 30000 ms, configurable 1..60000 ms; expiry closes
and stops owned tracks. Malformed authenticated signaling closes the session.
Spotify permits one pending token request with the same bounded deadline and
at most eight requests per instance; credentials are callback-only and not saved.

Browser ICE APIs cannot bind a socket to a chosen local address/port. A normal
browser can therefore produce candidates this restricted profile rejects. Do not
claim ordinary browsers can complete loopback media under these restrictions;
that requires an installed isolated runtime with an independently verified
network/socket policy. No browser executable was installed here; only mocked
native peer operations and real loopback signaling transport were executed.

Opus is an explicit lossy destination constraint, not raw-quality preservation.
Original raw rate, precision and ordered channels remain in the frozen source
spec for the host's destination-local conversion decision. Do not change sibling
raw routes. Actual negotiated codec, media integrity, browser resampling/layout,
dropouts, clocks, revocation latency and end-to-end delay require real-media proof.

## Focused executable evidence

From repository root, using installed Node 24 (no packages or downloads):

```sh
node --test --test-timeout=10000 interoperability/tests/public_sdk/*.test.mjs
```

The tests invoke the production modules with mocked official/browser APIs. They
cover SDK calls, no-autoplay/muted player defaults, directional grants,
whole-premix exclusion, frozen pending metadata, exact source format/provenance,
source/session switch rejection, pinned signaling identity/fingerprint/codec,
selected-track lifecycle, candidate bounds, malformed endpoints, timeouts,
backpressure and revoke during pending SDK/SDP operations. One test starts an
owned ephemeral IPv4 loopback HTTP server, posts a bounded authenticated-envelope
fixture, executes production `accept()` and verifies the response/answer, then
closes all owned connections. Its native peer is still mocked.

Missing qualification: installed real browser, live official SDK, vendor account,
Cast/AirPlay device, Spotify service execution, real WebRTC peer/media, physical
capture/playback, platform UI, distribution eligibility and native graph delivery.
No throughput/latency/audio-quality claims derive from these tests.

## Public standards, vendor policies and licenses

Primary sources checked online 2026-10-29; no SDKs or standards were downloaded
and no accounts/purchases were made. These are source links and distribution
gates, not legal clearance.

- [Google CAF Web Sender integration](https://developers.google.com/cast/docs/web_sender/integrate)
  and [CastSession reference](https://developers.google.com/cast/docs/reference/web_sender/cast.framework.CastSession)
  describe the sender/session and media control APIs used here. A sender is not
  a desktop receiver. [Cast SDK terms](https://developers.google.com/cast/docs/terms)
  and [Google's policy summary](https://support.google.com/cast-developer/answer/4513288?hl=en)
  apply separately from this MIT adapter; application registration, Cast UX and
  authorized SDK use need review before distribution. Do not relicense the SDK
  under the repository's MIT license.
- [Apple's Safari AirPlay controls example](https://developer.apple.com/documentation/webkitjs/adding_an_airplay_button_to_your_safari_media_controls?changes=l_8_4)
  documents the availability event and user route picker.
  [Apple developer agreements](https://developer.apple.com/support/terms/) and
  [MFi scope](https://mfi.apple.com/en/faqs) require separate assessment for any
  later accessory/receiver work. This code uses the browser API, ships no Apple
  SDK implementation and asserts no AirPlay receiver certification or license.
- [Spotify SDK reference](https://developer.spotify.com/documentation/web-playback-sdk/reference)
  defines the injected player, token callback, device events and service methods;
  eligible Premium authorization and browser support remain prerequisites.
  [Developer terms](https://developer.spotify.com/terms) and
  [policy](https://developer.spotify.com/policy) govern usage and distribution,
  including content and commercial-streaming restrictions. A device ID proves
  neither arbitrary media transport rights nor DRM-free PCM access.
- [W3C WebRTC Recommendation](https://www.w3.org/TR/webrtc/) defines the native peer,
  transceiver, track, candidate and session-description APIs.
  [RFC 8827](https://www.rfc-editor.org/info/rfc8827/) specifies WebRTC security
  architecture: DTLS-SRTP media encryption does not authenticate an untrusted
  signaling channel by itself. No third-party WebRTC library is redistributed;
  standards publication licensing is separate from browser implementation rights.
