# Realtime protocol adapters: RTP media and separate public control

Added 2026-10-29. These optional Python standard-library adapters execute real
IPv4 loopback socket IO. The leased wrappers accept explicit callbacks from an
already authorized runtime owner; they do not open a native capture/output or
physical audio endpoint:
`backendAvailable` and `audioApplied` remain false. No third-party player or
physical receiver has been qualified. Run from the repository root with
`PYTHONPATH=interoperability`; the package is `mixyard_interop.realtime`.

| Role | Implemented public profile | Evidence and outstanding gates |
| --- | --- | --- |
| RTP live send | RTP v2 L16/L24 with explicit dynamic payload type 96–127 | Golden wire bytes, owned UDP and leased runtime callbacks; no physical output, automatic pacing or encryption |
| RTP live receive | One explicitly admitted source endpoint, SSRC and exact format | Actual sender → receiver PCM, rollover, loss/reorder/injection/revocation checks; no adaptive playout, concealment or clock sync |
| RTCP diagnostics | Caller-driven compound SR/RR plus single-CNAME SDES | Real owned UDP, explicit NTP/RTP observations, loss/jitter counters and locally correlated RTT; no scheduled multiparty RTCP engine, hardware timestamps or one-way/sync claim |
| RTSP control client | Explicit RTSP/1.0 UDP-unicast playback session | Real TCP OPTIONS, DESCRIBE, SETUP, PLAY, PAUSE, TEARDOWN; no RTSP server, publication, RTSP/2.0, authentication or automatic media admission |
| Snapcast service control | Public newline-delimited TCP JSON-RPC 2.0 | Actual request/response socket tests for version/status, client status/volume and group mute; no Snapcast PCM transport, server/client audio role or group synchronization |
| SRTP | Unavailable | Requires an installed public cryptographic implementation, negotiated keys/identities and replay/security qualification; no homemade crypto or cleartext fallback |

## RTP exact media agreement

`RtpProfile(spec, payload_type=96)` accepts the canonical `Spec` codecs
`raw-s16-le` and `raw-s24-le` only. Each `RtpSender.send(pcm)` sends exactly
`frames_per_packet × frame_bytes` input bytes in one UDP datagram. Each
`RtpReceiver.receive()` returns a frozen `ReceivedPacket` containing the original
admitted `Spec`, canonical little-endian PCM, sequence, timestamp, SSRC and
`missing_before`, extended sequence/timestamp counters and local arrival time in
monotonic nanoseconds. A gap is reported; samples are never silently fabricated.

The L16 representation and sample interleaving follow
[RFC 3551 §4](https://www.rfc-editor.org/rfc/rfc3551.html#section-4): signed integer
samples in network byte order, consecutive channels for each sampling instant,
with timestamps advancing by frames independently of channel count. L24 uses the
three-octet signed representation in
[RFC 3190 §4](https://www.rfc-editor.org/rfc/rfc3190.html#section-4). The explicit
per-destination conversion reverses bytes within each sample only; it preserves
every sample bit, rate, precision and channel position. There is no resampling,
quantization, normalization, downmixing or lossy fallback. A Float32 or oversized
profile rejects instead of narrowing sibling routes.

The wire header uses [RTP v2 fields](https://www.rfc-editor.org/rfc/rfc3550.html#section-5.1):
12 bytes, no CSRC/extension/padding, zero marker for continuous PCM, chosen dynamic
payload type, unsigned wrapping 16-bit sequence and 32-bit sample timestamp, and
one configured SSRC. Initial sender sequence/timestamp/SSRC default to random
values; explicit values support an external session agreement and reproducible
tests. The receiver pins SSRC at construction and accepts only this header subset.

Rate, ordered channel identities, precision, fixed frame count and provenance are
agreed out of band through the exact local `Spec`. RTP bytes do **not** establish
those identities or paired trust. Arbitrary ordered layouts remain unchanged
when explicitly agreed by both endpoints. `RtpProfile.sdp(destination)` only
emits a basic [SDP](https://www.rfc-editor.org/rfc/rfc8866.html) description for
`("mono",)` or ordered `("FL", "FR")`; other layouts reject SDP publication
rather than mislabeling their channel convention. SDP generation is a pure
function, not discovery or listener advertisement, and does not grant permission.

`RtpSender(session, destination, profile, *, ssrc=None, sequence=None,
timestamp=None, local=None, timeout=0.25, max_packets=4096)` owns its UDP socket.
By default it binds an ephemeral port on literal `127.0.0.1`; `.local` exposes
that endpoint for separate receive preadmission. An optional `local` fixes the
source endpoint. `RtpReceiver(session, local, peer, profile, *, ssrc,
timeout=0.25, max_packets=4096)` binds the explicit local port. Both are context
managers with `close()`; only their sockets are closed.
Adapter instances have one synchronous owner; callers must not invoke methods
concurrently on the same socket/session-control instance.

The sender checks `Session.require("live-send", destination, spec)` before bind
and each send. The receiver checks `Session.require("live-receive", peer, spec)`
before bind, every receive, after blocked IO and before returning PCM. The local
listener is an explicit caller-owned endpoint; the receive grant pins the remote
source endpoint. A packet from any other source is rejected before sequence state
or media delivery. Revocation cannot recall an OS send already admitted; it
blocks subsequent operations and release of a received packet after a blocked IO.

The receiver accepts a forward modular sequence distance of 1–128. The timestamp
must equal the prior timestamp plus that distance × the exact frame count,
modulo 2³². Duplicates, older/reordered packets, larger jumps and inconsistent
timestamps reject without advancing state. Forward gaps increment `missing` and
appear in `missing_before`; malformed packets increment `rejected`. This is a
bounded packet receiver, **not** a jitter buffer, general RFC 3550 sequence
validation/reordering algorithm, authenticated replay protection or synchronized
playout engine.
Restarting with a new SSRC requires a newly configured receiver.

Sender `sent_packets` and `sent_octets` count successful media sends, excluding
RTP headers from octet counts. Receiver `extended_sequence` and
`extended_timestamp` unwrap accepted forward packets across rollovers.
`jitter_frames` applies the 1/16 interarrival variation filter from
[RFC 3550 §6.4.1](https://www.rfc-editor.org/rfc/rfc3550.html#section-6.4.1)
to **profile-admitted packets only**, using local arrival differences in sample
frame units. Rejected late, duplicate or malformed packets do not enter these
statistics. Loss/fraction/jitter reports therefore describe this strict delivery
profile, not a full RFC receiver that counts late/duplicate arrivals. Jitter is
not device clock drift, buffered duration, network-only delay or output latency.

Limits: 1200 bytes per datagram including the RTP header; one datagram per call;
no application media queue; lifetime IO attempts default to 4096, configurable
1–65536; socket send/receive buffer requests of 16 KiB (actual kernel allocation
is OS-dependent); operation timeout greater than zero and at most one second.
Malformed/unadmitted receive attempts also consume the lifetime budget. Timed-out
receives consume an attempt. The caller must pace sends and implement bounded
scheduling; this Python adapter is not a native realtime audio callback.

## Owned runtime lease callbacks

`mixyard_interop.realtime.leased` depends on the separately integrated
`mixyard_interop.runtime` concern. It intentionally does not provide a fallback
lease, copy of runtime authority or fake native source. The RTP/RTCP primitives
remain importable without that module; runtime wrapper imports require it.

```python
from mixyard_interop.realtime.leased import LeasedRtpSender, LeasedRtpReceiver

# Supply two separately authorized owner leases and separately granted RTP sockets.
# The callback returns a frozen runtime.LeasedBlock with immutable exact PCM.
sender = LeasedRtpSender(rtp_sender, source_lease, source_callback,
    generation=7, revision=3, source_epoch="epoch-1", base_frame=8, clock_id=11)
receiver = LeasedRtpReceiver(rtp_receiver, destination_lease, sink_callback,
    generation=7, revision=3, source_epoch="epoch-1", base_frame=8, clock_id=11,
    timestamp=initial_rtp_timestamp)
sender.send()
received_block = receiver.receive()
```

Construction requires fresh RTP instances and exact agreement with the owner's
lease binding and clock origin. Each send calls the source, validates its exact
Spec, generation, revision, source epoch, frame and clock ID, requires a full RTP
packet, then uses `lease.accept(block, send)` for sequential admission. Each
receive checks the lease before and after UDP IO, requires the agreed initial
timestamp, maps extended RTP frame offsets to the admitted base frame and calls
`lease.accept(block, sink)`. Runtime validation is nonadvancing and repeatable;
sequential admission advances only after callback success. Callback failure may
have consumed a prefix and terminally revokes the runtime lease.

Missing/repeated frames are denied; the wrapper supplies no concealment, queue,
resampling or implicit timeline reset. Recovery requires explicit fresh leases,
RTP instances and a newly allocated SSRC. The caller must allocate a different
SSRC on source restart and separately admit its out-of-band epoch/timestamp;
these wrappers do not maintain a global history of SSRC use. Old-SSRC packets
cannot establish the new epoch. Reusing an SSRC/timestamp would lose this wire
distinction and is outside the contract. Ordinary cleartext RTP cannot prove
generation, provenance, source epoch or clock identity from network bytes.

The source/sink callbacks must be bounded and run off any native audio callback.
Callers own pacing and call `lease.check()` during their own waits; no wait or
scheduler is implemented here. Revocation blocks subsequent admission but cannot
recall an admitted OS send or an already executed consumer callback. Raw byte
identity is a separate assertion from destination-local conversion or processing:
the tests retain exact received integer PCM and separately evaluate a synthetic
host sink's explicit half-gain Float64 conversion. That host callback is not a
native engine or hardware playback qualification.

## RTCP reports and honest clock evidence

`RtcpPeer(session, local, peer, *, local_ssrc, peer_ssrc, cname,
timeout=0.25, max_packets=256)` owns a separately configured literal loopback UDP
socket. It needs an exact `control` grant on every IO and after receive; a media
grant does not authorize RTCP and RTCP never admits media. SSRCs are distinct
and locally pinned. CNAME is explicit visible ASCII, 1–128 characters, never an
automatically exposed hostname, account or authentication credential.

The implemented [RFC 3550 §6 compound profile](https://www.rfc-editor.org/rfc/rfc3550.html#section-6)
is precisely SR with zero report blocks or RR with one block, followed by one
single-CNAME SDES chunk. Other counts, padding bits, extensions, extra packets,
missing SDES, foreign endpoints/SSRCs and oversized/malformed packets reject
before report state changes. Outgoing RR requires admitted RTP from the pinned
source; outgoing SR requires the sender's live-send grant. Both reports use
actual profile counters. Report reception returns `ReceivedReport(report, cname,
round_trip_seconds)`; remote fields and CNAME remain unauthenticated diagnostics.

`send_sender_report(rtp_sender, ntp_timestamp=..., rtp_timestamp=...)` requires
an explicit contemporaneous NTP 32.32/RTP observation. A leased sender's
`send_report(rtcp, RtpClockSample(source_epoch, clock_id, base_frame,
ntp_timestamp))` additionally checks the runtime epoch and clock ID and maps the
observed frame to the RTP timestamp origin. **The caller must actually observe
the current common-clock frame at that NTP instant.** Matching integer labels
alone prove no clock accuracy or synchronization. Report send time is not the
last packet's sample time. Local monotonic timestamps are never promoted to NTP,
hardware time or externally qualified common-clock evidence.

`send_receiver_report(rtp_receiver)` echoes LSR and calculates DLSR in 1/65536 s
from local elapsed time since the accepted SR. For a matching outstanding SR,
receipt of RR computes local elapsed time since send minus the peer's reported
DLSR. At most 32 SR correlations are retained; an outstanding compact timestamp
cannot be reused. Unknown, already consumed or impossible correlations yield
`None`, not zero RTT. DLSR is quantized and untrusted; this is observed application
round-trip timing including host scheduling, not authenticated network latency,
one-way delay, physical output latency or synchronization. A malicious local
peer can forge plausible reports; no cryptographic replay/security claim exists.

RTCP uses the same 1200-byte datagram, 16 KiB requested socket buffers, at-most-one
second timeout and 1–65536 lifetime IO-attempt bounds as RTP. It starts no timer,
thread or listener beyond the explicit socket; report scheduling, RTCP bandwidth
allocation and multiparty membership/collision handling remain unavailable.
Use a fresh RTCP peer per receive epoch; report counters are not reset in place.

## RTSP control profile

`RtspClient(session, Endpoint("127.0.0.1", port, "rtsp"), "/resource")` implements
the public [RTSP/1.0 control syntax](https://www.rfc-editor.org/rfc/rfc2326.html).
This is an explicit legacy profile; RTSP/2.0 is separately specified by
[RFC 7826](https://www.rfc-editor.org/rfc/rfc7826.html) and is unsupported here.
Each connection and socket read/write requires the exact `control` grant.

Call `options()` and `describe()` to inspect the service. `describe()` requires
`application/sdp` and returns bounded untrusted bytes: it does not parse/follow
embedded control URLs or admit media formats. `setup(rtp, rtcp, track_path=None)`
requires explicit consecutive loopback UDP client ports. It accepts only matching
UDP-unicast transport with explicit consecutive server ports; `.server_ports`
exposes that untrusted proposal for separate admission. No media socket is opened.
The ready → `play()` → playing → `pause()` → ready lifecycle and `teardown()`
validate local state and session identity. A server-supplied session ID is bounded
and cannot inject headers. The client does not renew keepalive automatically;
callers own the server's stated timeout and explicit session renewal.

Only literal configured host/port and bounded ASCII paths are used. Redirects,
non-success responses, authentication challenges, interleaved TCP media,
multicast, changed source/client endpoints, duplicate headers, changed session
IDs and transfer encoding fail closed. Closing the socket resets local session
state; a failed side-effecting request has unknown remote outcome and is not
retried. RTSP control permission grants no live-send/live-receive authority.
There is no RTSP-controlled receiver/server or discovery claim.

## Snapcast service control

`SnapcastControl(session, Endpoint("127.0.0.1", port, "tcp"))` uses the upstream
[public TCP JSON-RPC API](https://github.com/snapcast/snapcast/blob/develop/doc/json_rpc_api/control.md),
typically port 1705. Available methods are `get_rpc_version()`, `get_status()`,
`get_client_status(client_id)`, `set_client_volume(client_id, percent=0..100,
muted=bool)` and `set_group_mute(group_id, muted=bool)`. Mutations are explicit
service commands, never evidence of audio being transported through Mixyard.
Each socket operation requires `service-control`; a generic `control` grant
does not authorize these methods. Upstream result objects are returned as
untrusted data and never cause follow-up network access.

Requests use increasing integer IDs and responses must match exactly. Up to eight
notifications are discarded per call, with no notification backlog. Errors are
reported without retry. JSON rejects duplicate members, nesting deeper than 16,
oversized numbers, non-finite numbers, batches and mismatched response IDs. Client
and group IDs are bounded ASCII strings. No arbitrary stream URL, filesystem
path, executable, plugin or server-configuration methods are exposed. No upstream
Snapcast process was installed, started or contacted for qualification.

**Snapcast synchronization is unimplemented and unqualified.** JSON-RPC controls
neither implement its PCM wire protocol nor prove synchronized group playback.
Future audio-client/server roles need upstream-version-specific framing, codec
and timing negotiation, actual server/client proof and separate consented routes.

Both TCP clients own at most one connection and allow 64 lifetime requests.
Each complete operation has a deadline (default 0.5 s, maximum 1 s), 128 IO-call
budget and 32768 received-byte budget. RTSP headers are at most 8192 bytes/64
fields and bodies at most 16384 bytes. Snapcast lines are at most 16384 bytes.
All limits count framing bytes. There are no retries, DNS lookups, redirects,
background threads, endpoint discovery, subprocesses or automatic playback.

## Safety, licensing and qualification

Core `Session`/`Grant` admission preserves consent, whole-premix group exclusions
and provenance/loop checks. The complete received `Spec` is retained locally;
bridging onward requires a separately validated path and destination grant.
Legacy RTP/RTSP/Snapcast is unauthenticated here. Loopback endpoint pinning is a
local resource boundary, not cryptographic identity or protection from malicious
same-user programs. Discovery never creates paired-device trust. Management
rights never imply capture, publication, live media or speaker rights.

No encryption is claimed. [SRTP](https://www.rfc-editor.org/rfc/rfc3711.html) has
separate cryptographic/key-management requirements; this implementation never
approximates it with custom encryption. SRTP transport, key exchange, remote/LAN
operation, physical/one-way latency, drift handling, adaptive jitter, pacing,
discovery/receiver advertisement, native delivery, Snapcast PCM/synchronization
and branded/platform/device compatibility remain unavailable or unqualified.
Linux synthetic loopback success does not qualify Windows/macOS/mobile devices.
No libsrtp2 installation was available in the lead's host snapshot; no library
was installed, downloaded or approximated here. SRTP remains unavailable rather
than silently falling back to cleartext.

The adapter code is original project MIT code using Python's standard library;
no RFC code components or upstream Snapcast source are copied or bundled.
The [Snapcast upstream license](https://github.com/snapcast/snapcast/blob/develop/LICENSE)
is GPLv3; any future source reuse/bundling needs a separate license decision.
RFC text retains its copyright notices; the
[IETF Trust provisions](https://trustee.ietf.org/documents/trust-legal-provisions/tlp-5/)
describe document/code-component terms. Protocol implementation does not grant a
certification, trademark or physical-device compatibility claim. Public sources
above were checked online on 2026-10-29 without SDK installs or downloads.

Focused executable proof, from the repository root:

```sh
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=interoperability \
  python3 -m unittest discover -s interoperability/tests/realtime -v
```

The tests use concrete canonical `Spec`, `Grant`, and `Session`, real owned IPv4
loopback UDP/TCP sockets, golden RTP bytes and bounded synthetic public-wire
peers. They cover exact PCM/order/provenance retention, both rollover counters,
loss/reorder/duplicate/SSRC/timestamp validation, endpoint injection, no-grant and
wrong-role/format denial, revocation during blocked receive, RTSP lifecycle and
redirect/transport rejection, Snapcast status/volume/mute, notification limits,
response correlation and malformed/oversized inputs. Timing tests add golden
compound reports, controlled elapsed-time unit/subtraction checks, real UDP
SR/RR correlation, loss/jitter counters and malformed compound rejection.
Runtime integration tests use the actual parent `RuntimeLease` implementation:
leased send and receive, raw L16/L24 identity, distinct processed Float64 output,
stale generation/revision/epoch/clock/frame, source and receive revocation,
first-timestamp mismatch, loss, failed sink and old-SSRC/new-epoch isolation.
Standalone leaf checkouts explicitly skip those integration tests until the
runtime concern is present; no mock lease is counted as runtime evidence.
They do not establish
interoperation with an upstream binary, playback timing or physical audio.
