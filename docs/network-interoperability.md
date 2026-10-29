# Network audio interoperability roadmap

Added: 2026-10-29. Status: planned; this document does not advertise implemented
compatibility. Track interoperability in [#56](https://github.com/MGRAFF2006/mixyard/issues/56),
native integration in [#51](https://github.com/MGRAFF2006/mixyard/issues/51),
format preservation in [#29](https://github.com/MGRAFF2006/mixyard/issues/29),
and processing/alignment in [#33](https://github.com/MGRAFF2006/mixyard/issues/33).

## Product goal and direction

Implement as many useful casting and streaming integrations as feasible on Linux,
Windows and macOS. For each ecosystem, independently implement sending from
Mixyard and receiving into Mixyard, including advertising Mixyard as a selectable
destination where that ecosystem permits it. An integration is not complete just
because its sender works, nor because one OS works. Genuine platform, hardware,
vendor and account constraints must be recorded rather than invented as excuses
for missing implementations. Hands-on native testing currently focuses on Linux;
Windows/macOS hardware qualification remains a separate gap.

Keep three behaviors distinct in the interface and capability model:

- **Live audio:** transport selected application/input/mix samples to a receiver.
- **Media playback:** tell a receiver to fetch a URL and control its player.
- **Service playback/control:** operate a particular service and its authorized
  player. This does not establish an arbitrary PCM input or output.

A browser API is a fourth integration surface, not a replacement for the actual
receiver protocol. Device discovery, media delivery, transport controls, volume,
group synchronization and format support are separate capabilities.

## Integration targets

All rows below are unimplemented roadmap targets. Ordering is an implementation
proposal, not a reduction of the accepted breadth. Every receiver/advertisement
role needs its own executable implementation and compatibility evidence.

| Family | Send/control from Mixyard | Receive into / advertise Mixyard | Boundary and initial priority |
| --- | --- | --- | --- |
| UPnP AV / DLNA | Discover renderers; expose a selected bus as a compatible media URL; negotiate media and control transport/volume. | Implement a MediaRenderer destination accepting supported media URLs and controls, ingest decoded media as an explicit source; evaluate a MediaServer role for exported content. | First open-device integration. Separate Control Point, Renderer and Server capabilities. Codec/profile negotiation and live-URL latency need device tests. [OCF device/service specifications](https://openconnectivity.org/developer/specifications/upnp-resources/upnp/mediaserver4-and-mediarenderer3/) |
| Google Cast | Sender/controller for compatible Cast receivers; export selected audio through a supported media delivery path. | Investigate supported receiver/player integrations and the conditions under which a Mixyard endpoint can appear to Cast senders. | Vendor feasibility lane. A Web Receiver application runs on a Cast-capable receiver; the SDK does not itself turn a desktop process into a certified Chromecast. Qualify sender, receiver and device discovery separately. [Google architecture](https://developers.google.com/cast/docs/overview) |
| AirPlay / RAOP family | Investigate compatible audio sending and remote volume/control. | Offer a discoverable audio receiver where permitted and supported, feeding an explicit Mixyard input. | Vendor/version-specific lane. Track AirPlay versions, pairing/authentication, media formats, synchronization, implementation licenses and distribution eligibility separately. Safari media controls are not a general receiver implementation. [Apple web integration](https://developer.apple.com/documentation/webkitjs/adding_an_airplay_button_to_your_safari_media_controls), [MFi technology scope](https://mfi.apple.com/faqs) |
| Spotify Connect | Authorized service playback/control and transfer to eligible Spotify players. | Evaluate an authorized Spotify player identity selectable by Connect, including the Web Playback SDK's supported browser path. | Service-specific lane. This is not an arbitrary stream sink/source. Account, SDK, policy and distribution requirements are gates; no DRM bypass or generic PCM-export claim. [Spotify Connect](https://developer.spotify.com/documentation/web-playback-sdk/concepts/spotify-connect) |
| W3C Remote Playback API | Optional browser controls for eligible HTML media elements, with runtime feature detection and an explicit fallback. | Actual destination advertising belongs to the underlying receiver integration, not this API. | Frontend enhancement, not a wire protocol. Do not claim universal browser/device support or arbitrary mixed-system-audio casting. [W3C specification](https://www.w3.org/TR/remote-playback/) |
| HTTP(S) audio / internet-radio-style streams | Serve explicitly selected live buses or supported media resources with negotiated encoding and scoped access. | Open an approved stream URL and decode it to an input; evaluate appropriate playlist/radio metadata support. | Reusable delivery foundation for URL-fetching renderers, not universal discovery. Container/codec, authentication, buffering and seeking capabilities are explicit. |
| WebRTC | Send selected tracks to compatible peers/browser clients. | Receive peer tracks as routed inputs; expose a cooperating browser receiver. | Open live-media lane. Signaling, identity, secure media, NAT/relay choices, codec negotiation and permissions require implementation. Do not promise original sample precision through a lossy media codec. [W3C WebRTC](https://www.w3.org/TR/webrtc/) |
| RTP / SRTP with RTSP where appropriate | Negotiate and send supported RTP audio payloads, optionally controlled through a supported RTSP profile. | Receive matching RTP/SRTP sessions; implement and advertise an RTSP-controlled receiver where a tested profile defines it. | Open live-media lane. RTSP session control and RTP media are distinct; payload, security/keying, clock and discovery profiles must be explicit. [RTP](https://www.rfc-editor.org/rfc/rfc3550.html), [SRTP](https://www.rfc-editor.org/rfc/rfc3711.html), [RTSP](https://www.rfc-editor.org/rfc/rfc7826.html) |
| Snapcast | Feed selected streams to compatible Snapcast deployments; evaluate a compatible server role. | Evaluate a compatible client role and mapping received streams into Mixyard. | Open ecosystem, not a universal casting standard. Prove grouped synchronization and negotiated codec behavior independently of Mixyard's own QUIC transport. [Upstream project](https://github.com/snapcast/snapcast) |
| AES67 / RAVENNA interoperability | Evaluate standards-compatible professional PCM transmission. | Evaluate matching receive/clock/discovery interoperability and selectable endpoint publication. | Professional lane after stable native clocks. Precise synchronization, network configuration, packet timing and interoperability tests are prerequisites; nominal sample rate alone proves none of these. [AES67 scope](https://aes.org/publications/standards-store/?id=140) |
| SRT / RIST | Evaluate audio contribution feeds to compatible production receivers. | Evaluate contribution receivers feeding Mixyard inputs. | Professional contribution candidates, not consumer speaker discovery. Select container, audio codec, security and recovery profiles; compare added delay with the use case. [SRT upstream](https://github.com/Haivision/srt), [RIST Forum](https://www.rist.tv/) |

Additional candidates for scoped feasibility work include Squeezebox/Lyrion
compatibility, SMPTE ST 2110 audio profiles, AVB/Milan and proprietary ecosystems
such as Dante. Determine usable public/authorized interfaces, hardware needs,
supported directions and distribution terms before implementation commitments.
Do not market AES67 interoperability as complete compatibility with every Dante
or RAVENNA feature, or a brand's UPnP subset as support for all its devices.

## Shared implementation and UI

Use protocol adapters around the existing source/mix/output model. Received audio
gets a stable source identity and provenance; a chosen local or network output
owns its own capabilities and conversion. A limited speaker never lowers the
quality of sibling raw/high-quality routes. Preserve original rate, precision and
ordered channels until an explicitly chosen conversion or the final destination
requires otherwise. Raw/lossless/lossy and passthrough/DSP choices remain visible;
never silently fall back to lossy compression or discard channels.

Provide an ordinary **Send to device** flow and an opt-in **Allow receiving**
flow. Show the actual protocol and whether the action streams audio, hands over a
URL or controls a service. Expose supported formats, measured delay where known,
volume scope, group membership and disconnect state. Expert settings retain the
complete protocol/encoding/buffering parameters. A controller's UI cannot claim
capture or playback succeeded before the backend reports actual evidence.

Advertising is opt-in per interface and receiver role. Use an explicit endpoint
identity/name and withdraw it when disabled. Receiving must not grant microphone
capture consent, change OS defaults, enable speakers or bypass existing route
exclusions. Apply source-origin/loop prevention across protocol bridges; a mix
cannot be unmixed later to remove private audio.

The loopback administrative web interface remains private. Media listeners,
discovery and subscriptions use separately admitted service endpoints; do not
expose the administration token or port to make casting work. Legacy unauthenticated
LAN discovery is not equivalent to paired Mixyard trust. Define explicit access
and exposure policy per adapter, with bounded parsers, sessions, media queues and
resource budgets. URL receivers need approved destinations, redirect/DNS checks,
private-address policy and bounded fetch/decode behavior; never turn a renderer
into an unrestricted fetch proxy. Treat XML, metadata, playlists and remote
control callbacks as untrusted inputs.

Keep service credentials machine-local. Prefer supported SDK/protocol integrations;
record dependency licenses, redistribution/trademark/certification constraints and
any account/hardware requirements. Optional external bridges can be evaluated
without silently bundling incompatible code into the MIT project.

## Delivery stages and acceptance

1. Build a shared protocol-adapter capability model and real format/transport
   negotiation. Record discovery, sending, receiving, advertising, control and
   synchronization independently for each OS/version. Preserve existing paired
   raw transport while integrating bounded playout and native endpoints.
2. Implement open-device/live-media foundations: HTTP delivery plus UPnP/DLNA,
   WebRTC and defined RTP/SRTP/RTSP profiles. Add receiver advertisement alongside
   sender work, rather than leaving it permanently deferred.
3. Run vendor feasibility in parallel: Cast, AirPlay and Spotify. Deliver every
   supported role that can actually be implemented; unresolved vendor gates remain
   explicit work items. Add Snapcast where it reuses the multiroom foundation.
4. Add professional/contribution profiles after clock, recovery and native
   delivery gates. Expand the candidate list through separately reviewed scopes.

For every implemented role, require an exact protocol/version/OS/device matrix,
real discovery/advertisement and connection tests, actual media delivery/control,
format negotiation and visible conversions. Exercise loss, slow/absent receivers,
clock drift, bounded backlog, revoke/disconnect/restart, malformed discovery/media
and privacy/loop exclusion. Measure throughput and latency; do not infer physical
latency from RTT/2, buffer capacity or a successful API call.

Current hands-on testing is Linux in isolated laptop-owned sessions/private audio
servers, without speaker playback, personal audio capture or personal-session
mutation. Silent null/file-consumer evidence is useful but does not qualify
physical speakers or every branded receiver. Windows/macOS build/fixture checks
and Linux success do not complete Windows/macOS native/device qualification.

Public compatibility badges and destination advertising claims require evidence
for that exact role. Use **planned**, **implemented with fixture evidence** and
**device-qualified** distinctly; never collapse them into a universal supported
checkbox. The roadmap aims for broad bidirectional coverage, not a claim that one
standard or browser API can reach every speaker.
