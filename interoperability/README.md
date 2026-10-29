# Optional protocol adapters

This standalone Python 3.12+ package has no third-party dependencies. It does not
load the native runtime, open a device or alter the paired QUIC transport. It is
an explicit production adapter boundary for owned loopback endpoints, not a LAN
compatibility claim. Browser modules are optional ES modules with injected SDKs.

Run the optional Python policy/socket suite and installed-Node SDK contracts
without installing packages or generating bytecode:

```sh
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=interoperability python3 -m unittest discover -s interoperability/tests -v
node --test --test-timeout=10000 interoperability/tests/public_sdk/*.test.mjs
```

The optional `test_descriptor_oracle.py` also compares a 50-case positive/negative
corpus with an independently built executable using production Rust
`Spec::validate_descriptor`. Set `MIXYARD_DESCRIPTOR_ORACLE` to that executable;
without it the conformance test explicitly skips. The executable consumes one
bounded JSON `{sender,receiver,spec}` per process and returns `{accepted:bool}`.
The corpus passed against the network lead's production oracle on 2026-10-29,
including 160 channels above the QUIC MTU, integer extrema, JSON bool/float type
rejection, unknown fields and provenance failures. It tests descriptor semantics,
not Python consent/resource policy or duplicate wire keys (a JSON Value oracle
has already normalized object keys). This package never builds Rust itself.

The exact raw descriptor is the existing `network/src/protocol.rs::Spec` JSON.
`Spec.from_json` accepts at most 4096 UTF-8 bytes, rejects duplicate/unknown keys,
and freezes the descriptor/provenance. `from_dict` is the local-object boundary.
It preserves sample rate, codec precision, ordered channels, stream identity and
all source groups. No global stereo/Float32/rate/QUIC-MTU limit is applied.
Adapter-specific format and packet budgets remain separate. Unsupported formats
reject; an actual conversion must happen explicitly for that destination.

```python
from mixyard_interop import Endpoint, Grant, Session, Spec

spec = Spec.from_dict({
    "stream": 1, "name": "file bus", "codec": "raw-f64-le",
    "sample_rate": 44117, "channels": ["FR", "FL", "AUX"],
    "frames_per_packet": 2,
    "provenance": {"sources": [{"peer": "source", "source": "file", "groups": []}],
                   "visited_peers": ["source"]},
})
destination = Endpoint("127.0.0.1", 9100, "http")
session = Session("source", [Grant("live-send", destination, "receiver", spec)],
                  consent=True, lifetime_seconds=60)
session.require("live-send", destination, spec)
session.revoke()
```

`Grant(role, endpoint, peer, spec=None)` is fresh **local** admission, not an
authentication credential. Caller-provided `peer` is a locally pinned logical
identity; unauthenticated protocol bytes cannot establish it. A session needs
explicit consent, a finite lifetime of at most one hour, bounded grant count and
per-stream frame/byte-rate budgets. No consent/grant persistence or restore API
exists. Caller must retain revocation handles and own worker/socket cleanup.

Roles are independently granted: `live-send`, `live-receive`, `media-serve`,
`media-fetch`, `control`, `service-control`, `advertise`. Media requires exact
Spec equality including provenance. Media-send paths end with the local identity;
receive paths end with the admitted sender; the receiving identity cannot occur
in the path. Excluded source groups reject an entire premix before IO. Control,
discovery and service accounts do not grant audio rights. All endpoints use exact
literal `127.0.0.1` and a nonzero port, with `http`, `tcp`, `udp` or `rtsp` scheme;
hostnames, redirects, other addresses and administration-token reuse are outside
this profile. Only endpoints whose processes the caller owns may be admitted.

Python adapters call `Session.require` before each IO operation and after a
receive before returning media. Browser adapters expose a synchronous
deny-by-default `authorize(role, context)` callback, and require the host's
canonical validation, live grants, exclusions and immediate revoke/expiry
notification. The callback is an explicit integration boundary; it is not a
Python Session implementation or pairing. See the public SDK profile for its
safe-integer budget, SDK/URL-lease assumptions and real browser media gates.
`revoke()` blocks later admissions. It cannot
recall an operation already admitted or bytes already handed to an OS or SDK;
stopping the owned worker and discarding queued media is the adapter/caller's
responsibility. `check()` is only a coarse lifecycle check, never an IO grant.
No physical-audio application or backend availability is claimed: both flags stay
false. Tests establish finite policy/loopback behavior, not hardware, pacing,
latency, clock synchronization, LAN security, throughput or endurance.

Implemented role profiles and executable examples are documented separately:

- [HTTP PCM, partial UPnP AV and unicast SSDP](../docs/protocol-http-upnp.md):
  finite media/live blocks, explicit generic L16 conversion, approved renderer
  source identity and separate control/media/advertise grants.
- [RTP, RTSP and Snapcast](../docs/protocol-realtime.md): actual L16/L24 UDP
  media, RTSP/1.0 control and Snapcast service control. SRTP, native clocks,
  Snapcast PCM and group synchronization remain explicit gaps.
- [Cast, Safari AirPlay, Spotify and WebRTC](../docs/protocol-public-sdk.md):
  injected public API methods with mocked SDK/native boundaries and real owned
  loopback signaling ingress. No vendor account/device/browser-media execution,
  arbitrary Spotify PCM or certified Cast/AirPlay receiver is claimed.

The adapters are optional importable production modules with focused socket or
SDK-contract proofs. No native-runtime/web-console route, persistent consent,
production signaling service or LAN deployment is connected by this package.

Our adapter code remains MIT. Python is an external runtime under its
[PSF license](https://docs.python.org/3/license.html), not vendored here. No SDK,
binary, vendor credential or third-party protocol implementation is bundled.
Protocol-specific primary standards and SDK gates belong in the linked protocol
documents; [HTTP semantics](https://www.rfc-editor.org/rfc/rfc9110.html) do not
provide paired trust, media permissions or a generic casting receiver.
