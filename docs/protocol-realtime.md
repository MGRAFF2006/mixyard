# Realtime protocol adapters: RTP media and separate public control

Added 2026-10-29. These optional Python standard-library adapters execute real
IPv4 loopback socket IO. They do not connect the native runtime or physical audio:
`backendAvailable` and `audioApplied` remain false. No third-party player or
physical receiver has been qualified. Run from the repository root with
`PYTHONPATH=interoperability`; the package is `mixyard_interop.realtime`.

| Role | Implemented public profile | Evidence and outstanding gates |
| --- | --- | --- |
| RTP live send | RTP v2 L16/L24 with explicit dynamic payload type 96–127 | Golden wire bytes and real owned UDP sockets; no physical output, automatic pacing, RTCP or encryption |
| RTP live receive | One explicitly admitted source endpoint, SSRC and exact format | Actual sender → receiver PCM, rollover, loss/reorder/injection/revocation checks; no adaptive playout, concealment or clock sync |
| RTSP control client | Explicit RTSP/1.0 UDP-unicast playback session | Real TCP OPTIONS, DESCRIBE, SETUP, PLAY, PAUSE, TEARDOWN; no RTSP server, publication, RTSP/2.0, authentication or automatic media admission |
| Snapcast service control | Public newline-delimited TCP JSON-RPC 2.0 | Actual request/response socket tests for version/status, client status/volume and group mute; no Snapcast PCM transport, server/client audio role or group synchronization |
| SRTP | Unavailable | Requires an installed public cryptographic implementation, negotiated keys/identities and replay/security qualification; no homemade crypto or cleartext fallback |

## RTP exact media agreement

`RtpProfile(spec, payload_type=96)` accepts the canonical `Spec` codecs
`raw-s16-le` and `raw-s24-le` only. Each `RtpSender.send(pcm)` sends exactly
`frames_per_packet × frame_bytes` input bytes in one UDP datagram. Each
`RtpReceiver.receive()` returns a frozen `ReceivedPacket` containing the original
admitted `Spec`, canonical little-endian PCM, sequence, timestamp, SSRC and
`missing_before`. A gap is reported; samples are never silently fabricated.

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
bounded packet receiver, **not** a jitter buffer, RFC 3550 reception-report
algorithm, authenticated replay protection or synchronized playout engine.
Restarting with a new SSRC requires a newly configured receiver.

Limits: 1200 bytes per datagram including the RTP header; one datagram per call;
no application media queue; lifetime IO attempts default to 4096, configurable
1–65536; socket send/receive buffer requests of 16 KiB (actual kernel allocation
is OS-dependent); operation timeout greater than zero and at most one second.
Malformed/unadmitted receive attempts also consume the lifetime budget. Timed-out
receives consume an attempt. The caller must pace sends and implement bounded
scheduling; this Python adapter is not a native realtime audio callback.

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
operation, RTCP, measured latency, drift handling, adaptive jitter, pacing,
discovery/receiver advertisement, native delivery, Snapcast PCM/synchronization
and branded/platform/device compatibility remain unavailable or unqualified.
Linux synthetic loopback success does not qualify Windows/macOS/mobile devices.

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
response correlation and malformed/oversized inputs. They do not establish
interoperation with an upstream binary, playback timing or physical audio.
