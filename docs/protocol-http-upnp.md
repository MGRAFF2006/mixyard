# HTTP PCM and partial UPnP AVTransport

Implemented as optional Python standard-library APIs under
`interoperability/mixyard_interop/http_upnp`. Evidence: actual owned IPv4 loopback
sockets on Linux/homelab-dev, synthetic PCM only. No native capture, speaker output,
LAN, multicast, pairing, DLNA certification or physical-device compatibility is
qualified. `backendAvailable` and `audioApplied` remain false. The continuous
profile accepts explicit runtime lease callbacks. This adapter does not open a
native host, audio device, graph, existing network transport or web console.

## Executable profile and API

Use `PYTHONPATH=interoperability`. Every API receives a live injected
`Session.require(role, endpoint, spec=None)` boundary; the concrete shared
`Session` and `Grant` work directly. Explicit `endpoint=` values let a caller
preadmit a known owned port. MediaSource/Renderer may instead allocate an ephemeral
loopback port, which requires an injected boundary able to admit that exact port.
Nothing grants a wildcard permission automatically.

| API | IO / separately granted role |
| --- | --- |
| `MediaSource(session, spec, pcm, endpoint=..., wire='raw', mode='media')` | HTTP GET `/pcm`; `media-serve` with exact Spec at source endpoint |
| `MediaSource(..., mode='live')` | Same finite block transport, independently checks `live-send` |
| `HttpReceiver(session).fetch(resource, mode='media')` | HTTP GET to exactly `Resource(endpoint, path, spec, wire)`; `media-fetch` |
| `HttpReceiver(...).fetch(resource, mode='live')` | Independently checks `live-receive`; no implied media URL rights |
| `StreamSource(session, spec, read_block, check_lease=..., validate_block=..., endpoint=...)` | Paced continuous GET `/stream`; exact `live-send` and explicit source lease callbacks |
| `StreamReceiver(session).receive(resource, write_block, check_lease=..., block_factory=..., cancel=...)` | Continuous exact raw PCM handoff; `live-receive` plus destination lease checks |
| `Renderer(session, {'source-id': resource}, endpoint=..., identity=UUID)` | Description/SCPD and SOAP control at renderer endpoint; `control` |
| `Controller(session, renderer_endpoint)` | `set_uri(source_id, resource)`, `play()`, `stop()`, `info()`; `control` |
| `renderer.take_pcm()` | Drains exact `(Resource, bytes)` to an owned caller; fresh `control` and independent `media-fetch` checks |
| `Advertiser(session, renderer_endpoint, UUID, search_peer, endpoint=udp_endpoint)` | SSDP M-SEARCH responses and explicit `announce()` NOTIFY; `advertise` at advertiser, search peer and renderer endpoints |
| `discover(session, advertiser, local=udp_endpoint, expected_renderer=..., expected_identity=UUID)` | One connected unicast search/response; `advertise` at advertiser and local discovery endpoints |
| `NoticeReceiver(session, local, advertiser, renderer, UUID).receive()` | Connected unicast NOTIFY, validated against exact admitted identity/location; `advertise` at local and advertiser endpoints |

Sources, renderers, advertisers and notice receivers are context managers. Close
owns and joins only their own thread/socket. A renderer's default identity is a
fresh UUID; pass an explicit canonical UUID when continuity is needed. Discovery
returns `paired=False` and never creates grants. `service-control` has no role in
this adapter: controlling a media service is separate from AVTransport control.

The finite `MediaSource` accepts an immutable, nonempty, frame-aligned PCM
resource of at most 1 MiB. Its output is the caller's bytes, not a generated
fixture. The receiver
returns exact PCM bytes for use by a separately admitted consumer. `mode='live'`
is a bounded block transfer primitive with live role checks. It has no pacing or
continuous capture; use the separate leased stream API below for continuous
transport. Neither API implements clock adaptation, jitter buffering or physical
real-time playback.

The default wire profile is `application/vnd.mixyard.pcm` (a private bilateral
profile, not a registered standard MIME claim), preserving all canonical codecs:
`raw-s16-le`, `raw-s24-le`, `raw-s32-le`, `raw-f32-le`, `raw-f64-le`. Rate, precision,
frame bytes and ordered channel IDs remain exactly as the frozen shared Spec
states. There is no generic stereo/Float32/48 kHz cap; shared explicit resource
admission and the finite body/header budgets apply. NaN payload bits and integer
low bits are not decoded, normalized or rewritten.

Explicit `wire='l16'` is a destination-local signed-16 little-endian ↔ network-byte-
order conversion only. This implementation admits 44100 or 48000 Hz and channel
order `('FC',)` or `('FL','FR')` for this profile. It never changes rate, precision
or channel order, and rejects other specs instead of narrowing them. Sibling raw
routes keep their original bytes. These are this adapter's L16 subset limits,
not global PCM limits. [RFC 2586](https://www.rfc-editor.org/rfc/rfc2586.html)
defines L16 signed samples, network byte order, rate/channels parameters and
interleaving.

The finite source includes an ASCII Base64 canonical JSON `X-Mixyard-Spec` snapshot and
`X-Mixyard-Mode` header. A receiver validates these when present, and always checks
the expected MIME type. A preapproved generic L16 source may omit the private
headers: endpoint/path/Spec identity still comes from explicit local preadmission.
Wire headers never establish provenance or authorization. Frames-per-packet is
retained metadata, not HTTP chunking; body boundaries must be whole PCM frames.

## Continuous leased PCM profile

`StreamSource` starts a listener only when explicitly constructed with an exact
owned endpoint, live-send Session grant and preauthorized runtime callbacks.
`StreamReceiver.receive` explicitly connects to one preadmitted Resource; source
selection and discovery never start it. The receiver runs synchronously in the
calling worker and returns `StreamResult(blocks, frames, pcm_bytes)` on clean EOF.
It does not retain the stream or start another thread. A caller may own a receive
thread and pass a `threading.Event` as `cancel`; cancellation raises
`PermissionError` and closes its socket. No network queue or automatic reconnect,
rewind, listener, advertisement, native capture or management authority is added.

The parent runtime's `LeasedBlock` and `RuntimeLease` are the required integration
boundary. The transport uses callback duck typing so that this protocol concern
remains independently importable. Bind all callbacks on each side to its same
lease; a no-op callback is only a transport-test double, never runtime admission:

```python
# Both leases, sessions, owned endpoint and bounded callbacks are supplied by
# an already authorized owner. Construction here grants no source/device access.
with StreamSource(send_session, spec,
                  lambda: source_lease.read(read_owned_block),
                  check_lease=source_lease.check,
                  validate_block=source_lease.validate,
                  endpoint=owned_endpoint) as source:
    result = StreamReceiver(receive_session).receive(
        source.resource,
        lambda block: destination_lease.accept(block, consume_owned_block),
        check_lease=destination_lease.check,
        block_factory=LeasedBlock,
        cancel=cancel_event)
```

`read_owned_block()` returns a frozen `LeasedBlock` or `None` for clean EOF.
Its constructor is `(spec, pcm, generation, revision, source_epoch, base_frame,
clock_id)`. Every receive uses that constructor, then calls the destination lease
before consumer handoff. Runtime admission checks exact observed identity/Spec,
generation, revision, source epoch, common-clock ID and complete owner-attested
clock mapping, frame range, first-frame pin, gaps and repeats. Owner changes,
expiry or revoke fail closed; a connection or new
HTTP request never renews a lease. The Session independently checks directional
rights, canonical provenance and whole-premix exclusions. Neither wire metadata
nor a locally pinned logical peer establishes authenticated paired trust.

This private bilateral wire profile is `X-Mixyard-Stream: leased-pcm-v1`, exact
`application/vnd.mixyard.pcm`, mandatory canonical `X-Mixyard-Spec`, and HTTP/1.1
`Transfer-Encoding: chunked`. It is not generic HTTP audio, L16, UPnP URL playback
or a DLNA profile. Each HTTP chunk carries one record: four-byte big-endian JSON
length, that JSON, then nonempty exact raw PCM. JSON has exactly `generation`,
`revision`, `source_epoch`, `base_frame`, `clock_id`; duplicate/unknown keys,
bool/float integers and invalid ranges reject. JSON is at most 1024 bytes; PCM is
at most 65536 bytes, whole frames and at most Spec.frames_per_packet. The maximum
chunk is 66564 bytes; its hexadecimal size line is at most ten bytes including
CRLF. Extensions and trailers reject. A zero chunk followed by CRLF ends the
response. Content-Length, encodings, redirects, ambiguous framing, missing or
changed profile/Spec, truncation and oversized chunks reject before handoff.
There is no whole-stream body cap. The source retains at most bounded current
record/block data; the receiver hands off one complete bounded record at a time.

After sending a block, the source waits its frames/sample-rate period before
reading another. Producer stalls and backpressure extend the period; it never
bursts to catch up. This is local monotonic cadence, **not hardware time**,
measured clock synchronization, drift correction or latency. The common-clock
evidence must come from the runtime owner. Raw PCM remains opaque, preserving
integer low bits and floating-point NaN payloads/signed zero. This exact raw
result is separate from a native host's processed, finite Float64 output; no DSP
or native output precision claim follows from the raw transport test.

Header IO and each record have independent two-second absolute deadlines, renewed
only at record boundaries. Socket waits and cadence check cancellation and both
Session/lease lifecycle at least every 50 ms while callbacks return promptly.
Partial-record progress cannot extend a deadline. No whole-stream timeout is
imposed, but a peer gap over the record deadline fails. Disconnect during source
cadence stops further source reads. Source blocks are revalidated before each
bounded write, and complete receive records are checked before consumer release.
Already admitted OS IO and a callback already consuming data cannot be recalled.
Source failures close the active connection and expose only an error counter and
bounded error kind; receive failures propagate to the owner. A failed consumer
is not retried, and the runtime lease handles terminal partial-consumption safety.

Callbacks run off the native audio callback and must be bounded, synchronous and
cooperative: Python cannot preempt an arbitrary blocked function. Late source
results are rejected; late consumer return reports timeout with possible prior
delivery. `StreamSource.close()` cancels IO/pacing, shuts down its active socket
and joins only its owned worker; a callback that refuses to return causes an
explicit close failure. Callers must not introduce unbounded queues in callbacks.
The listener admits one active request and backlog two, so a healthy continuous
stream occupies that source instance until EOF, failure or explicit close.

## AVTransport and discovery subset

Description `/device.xml` identifies `MediaRenderer:1`, its runtime UUID and the
AVTransport:1 service at `/control`; `/avtransport.xml` lists only implemented
actions. SOAP 1.1 uses the AVTransport:1 namespace, exact quoted SOAPAction,
InstanceID `0`, and Play Speed `1`. Implemented actions: SetAVTransportURI, Play,
Stop, GetTransportInfo. Source identity is explicit DIDL-Lite `item.id`, title,
resource URI and protocolInfo, checked against the renderer's immutable local
source map. No source is learned from an unauthenticated URL or metadata.

SetAVTransportURI validates the whole approved resource and independently checks
media-fetch permission. Play fetches the selected finite resource into one bounded
PCM slot. Completion reports STOPPED; this silent byte receiver does not invent
PLAYING evidence. Stop discards the slot; selecting another resource clears it;
consumer handoff rechecks permission. Control permission alone cannot select or
fetch audio. There are no subscriptions/GENA, volume, seeking, pause, playlists,
ConnectionManager, RenderingControl, TLS, credentials or general media decoding.
Malformed/unsupported requests close the connection without reflecting input;
UPnP SOAP error-code compatibility is not implemented. This is a partial service,
not a conformant full MediaRenderer or AVTransport device/control point.
[AVTransport:1](https://upnp.org/specs/av/UPnP-av-AVTransport-v1-Service.pdf)
and [its control point requirements](https://openconnectivity.org/wp-content/uploads/2015/11/UPnP-av-AVTransport-v1-Service-AnnexA.pdf)
include actions beyond this subset.

SSDP uses only explicitly pinned unicast IPv4 loopback UDP ports. No multicast
membership, 239.255.255.250 traffic, LAN discovery, DNS, arbitrary interface or
broadcast is possible through these APIs. M-SEARCH accepts only MediaRenderer:1,
MX `1` and an exact pinned search peer. NOTIFY supports alive/byebye; closing an
announced advertiser sends byebye if permission remains live. Revocation forbids
further packets, including byebye; previously advertised max-age is one second.
Discovery/notice receivers pin the UDP source via connected sockets, require an
exact UUID and approved renderer description URL, and do not fetch that URL.
No discovery cache or automatic trust/pairing is maintained.

## Admission, privacy and bounds

All HTTP/UDP hosts must be literal `127.0.0.1`, ports 1–65535, with the exact
expected scheme. Paths are bounded ASCII absolute resource paths; complete URLs,
protocol-relative paths, escapes, queries, fragments and traversal reject. The
renderer compares CurrentURI with approved Resource URIs before any fetch. There
are no proxies, resolver calls, network callbacks, redirects, playlist recursion or URL
rewriting. Finite HTTP uses one exact HTTP/1.1 200 response, Content-Length and
connection close; transfer/content encodings, duplicate headers and other status
codes reject. The separate leased stream profile admits only the chunk framing
described above.
This strict bounded subset is based on [HTTP/1.1 framing](https://www.rfc-editor.org/rfc/rfc9112.html).

Each finite HTTP operation has a two-second absolute socket deadline, 8 KiB total
header budget and 4 KiB read/write chunks. A server handles one request at a time with a
listen backlog of two, at most 16 KiB SOAP input and 1 MiB PCM output/receive slot.
No unbounded application queue or thread per request exists. XML requires UTF-8,
rejects declarations beginning `<!` (including DTD/entity declarations), NUL bytes,
over 128 nodes, depth over eight and over eight attributes per node; no external
entity/network resolution is provided. SOAP arguments and DIDL fields have an
exact accepted structure. Renderer maps admit at most 32 selected sources. SSDP
has a 2048-byte datagram budget and two-second receive timeout. The responder
handles one datagram at a time; OS UDP queues may drop excess traffic. A slow
request can delay that listener for at most its deadline, without affecting
separate source/renderer objects.

Renderer Play passes the active request's remaining monotonic deadline through
the nested media fetch; body upload and media download share one request budget.
Expired fetches cannot populate the PCM slot. Internal Server dispatch callbacks
receive a keyword-only `deadline` and must keep their own work within it.
Worker creation/start failures close the already-bound HTTP/SSDP endpoint.
Advertiser withdrawal errors still join the owned worker and close its socket;
an unsuccessful byebye may propagate after cleanup, and does not imply delivery.

The session checks fresh consent, expiry/revocation, exact role/endpoint/Spec,
source provenance, visited-peer loops and excluded groups. A premix containing an
excluded source rejects as a whole before IO. Checks occur before connection or listener activation, before each bounded HTTP read/write and UDP
send/receive, after receive, and before media handoff. Binding a chosen HTTP port
precedes authorization only to learn the actual endpoint; listening starts after
authorization. Already admitted OS IO cannot be recalled; revocation prevents
subsequent admission and consumer release. A caller already holding drained bytes
owns their lifetime. Close clears the renderer slot.

These unauthenticated legacy loopback listeners are reachable by other local
processes while explicitly enabled. A Grant's logical peer is local policy, not
proof of the identity of a connecting process. Do not expose these sockets through
forwarding or mistake this constraint for remote authentication, encrypted
transport or paired Mixyard trust. Production remote exposure and trusted peer
establishment remain separate gates. Management/advertisement rights never imply
media rights, microphone capture, physical playback or default graph mutation.

## Verification and distribution

Run without installs, Cargo or caches:

```sh
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=interoperability \
  python3 -m unittest discover -s interoperability/tests/http_upnp -v
```

The focused suite uses production Session/Grant and real owned TCP/UDP sockets:
all five raw codecs at 96 kHz with nonstandard ordered channels, explicit L16 wire
bytes and round trip, source → controller/renderer → silent byte consumer,
M-SEARCH discovery and alive/byebye notifications, role isolation, no-consent,
revocation, excluded mixed provenance, relabelled specs, hostile URLs, malformed
SOAP/DTD/depth/instance/action, malformed datagrams/headers, response redirects,
oversized/truncated bodies, and recovery on subsequent requests. The standalone
regressions also cover equivalent L16 media types versus format mismatches,
failed worker startup, failed SSDP withdrawal and nested fetch deadline expiry.
The standalone
CI workflow runs the same Linux-only command. No Windows/macOS, physical device,
long-running clock/latency or DLNA compatibility claim follows.

Continuous worker tests exercise streams larger than the finite 1 MiB cap without
accumulating them, all five raw codecs with nonstandard ordered channels,
source cadence after a producer stall, total duration beyond one IO deadline,
explicit cancellation and revocation mid-record, source disconnect/revoke during
cadence, close/join, failed startup, late source results, changed block Spec,
packet/frame limits and hostile chunk/header/metadata input. The optional
`test_stream_runtime.py` tests use the actual parent `RuntimeLease` and
`LeasedBlock` when that prerequisite is importable; otherwise they explicitly
skip. Those integration tests cover exact raw send **and** receive through owned
leases (integer extrema/low bits, floating NaN payloads, signed zero and
subnormals), stale generation/revision/epoch/clock, frame gaps/repeats,
owner changes during pacing/partial receive, and terminal consumer failure.
These are short synthetic owned-loopback proofs, not endurance, native host
processing, hardware clock or audio-device qualification.

Original implementation is covered by the repository MIT license. No third-party
UPnP stack, SDK, vendor code or specification text is bundled. Python and its
standard-library distribution have their [own license notices](https://docs.python.org/3/license.html).
Public references were checked online on 2026-10-29 without downloads/purchases:
[OCF UPnP specifications/architecture and rights references](https://openconnectivity.org/developer/specifications/upnp-resources/upnp/)
and [OCF certification](https://openconnectivity.org/certification/upnp-certification/).
Public specification access does not grant a certification badge or settle every
IPR/distribution obligation; OCF describes separate certification agreements and
mark rights. This work makes no certification, trademark-license, DLNA-profile
or universal speaker compatibility claim. Broader interoperability, complete
service/state behavior and any shipping badge need separate qualification and
rights review for the exact role.
