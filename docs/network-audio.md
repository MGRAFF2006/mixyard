# Secure network-audio foundation

Status: executable engineering foundation, 2026-10-28. This does **not** yet let
a laptop and desktop use one headphones pair. LAN/Tailscale is the first intended
deployment boundary. No public relay, cloud service, discovery, NAT traversal,
driver, native audio endpoint or pairing GUI is included.

## Product and core boundary

Network transport carries any number of independently named, explicitly permitted
streams up to configured resource budgets. Names are display metadata; stable
peer IDs and nonzero u64 stream IDs carry identity. Each direction has its own
send and receive allowlist. A stream can represent one selected application,
a selected source or an already safe premix; several buses remain independent.
There is no whole-system default and no fixed named-mix count.

The architecture remains `sources -> user-defined mixes -> outputs` from
[PLAN.md](PLAN.md). Network sources/outputs will be adapters to that graph,
not a second mixer. Desired Config and observed Inventory remain distinct;
this PR changes neither schema nor native adapters. Future Inventory must report
the paired remote source/stream identity, exact format, admission/degraded state
and counters. A connection is not evidence that PCM reached headphones.

Optional processing is independent of transport. Preserve source sample rate,
precision and channel layout through the local graph **and** raw network path.
Never truncate to float32, resample, normalize or downmix implicitly. A user-requested
conversion or a final destination's actual format limit belongs on that output's
conversion edge and must not reduce sibling streams. The existing core/native
implementations still need separate audit and qualification for this broader
contract; byte-preserving transport does not qualify their precision.

Every Spec pins the complete original source IDs/groups and ordered visited peer
path. The path ends in the sender, contains no repeated peer and cannot already
contain the receiver. Duplicate original source identities are rejected. Receive
permissions match the entire descriptor, and excluded groups reject a whole
premix: already summed private/call-return audio cannot be removed downstream.
Rebuild a safe upstream mix instead. Future integration must carry this provenance
through graph edges, enforce destination-specific mix-minus, mic monitoring
consent, and reject converging duplicate paths/cycles together with the core's
existing origin/recapture rules. It must never relabel network PCM as a new
unrelated local source and lose its origin.

These checks enforce declared provenance from a trusted paired peer. Encryption
cannot prove that its selected application actually supplied the declared PCM;
an authorized malicious peer can lie about its capture. Local source selection
and destination consent remain necessary.

## Transport and peer authorization

Decision: Quinn QUIC with TLS 1.3, reliable streams for admission and QUIC DATAGRAM
for PCM. [RFC 9221](https://www.rfc-editor.org/rfc/rfc9221.html) specifies datagrams
without retransmission or fragmentation, subject to path MTU and congestion;
applications supply their own flow IDs. Reliable control avoids losing stream
permissions while PCM can expire rather than building a retransmit backlog.
QUIC security is standardized in
[RFC 9001](https://www.rfc-editor.org/rfc/rfc9001.html); this implementation uses
[Quinn](https://docs.rs/quinn/0.11.12/quinn/) and
[rustls client-certificate verification](https://docs.rs/rustls/0.23.46/rustls/server/struct.WebPkiClientVerifier.html),
not custom crypto or unauthenticated UDP. Tailscale does not replace application
peer authentication.

Both endpoints require a paired certificate, verify TLS signatures/validity and
the server's expected name using WebPKI, then compare the authenticated leaf DER
against the exact expected certificate before creating a Session. Peer IDs bind
to those provisioned pins and directional permissions; remote control cannot
choose a different peer ID. There is no accept-any certificate verifier, anonymous
client, 0-RTT, automatic trust-on-first-use or silent key replacement. Certificate
expiry requires explicit repair. Tests create disposable identities in memory;
no persistent private keys are shipped. Production pairing, secure key storage,
revocation/key rotation and permission editing remain gates.

Explicit disconnect/close, receive-worker cancellation and connection failure
flush receive queues and gate sends; existing consumers get silence plus counters.
There is no device fallback, automatic reconnect or replay. A new connection
needs fresh stream admission; QUIC keys separate it from old connection traffic.
Within a connection, stream generations must increase on reliable restart;
datagrams never create/restart streams. A path failure is recognized by transport
closure/idle timeout (5 seconds); queued audio may still be consumed before that
failure is detected. Applications must continue ticking the PCM timeline during
loss, so old packets expire rather than accumulating.

## Version 1 framing and raw formats

ALPN: `mixyard-audio/1`. All header integers are unsigned, big endian. Raw PCM is
interleaved, packed little endian, in the descriptor's explicit channel order.

| Byte offset | Field |
| --- | --- |
| 0 | `MXAD` magic (4 bytes) |
| 4 | version u16 = 1 |
| 6 | payload length u16 |
| 8 | stable stream ID u64 |
| 16 | generation u64, nonzero after admission |
| 24 | packet sequence u64, starting at zero per generation |
| 32 | first sample-frame offset u64, `sequence * frames_per_packet` |
| 40 | exact PCM bytes |

The control Offer is length-prefixed JSON (u32 big endian), one Offer per reliable
bidirectional stream, followed by FIN; the response is a versioned accept/reject
Reply followed by FIN. Unknown fields, trailing data, invalid JSON, empty or
>8192-byte frames fail closed. A configured deadline bounds prefix/body/FIN/reply
waiting. One inbound control task runs concurrently with datagram reception;
extra simultaneous controls are reset. Outbound negotiation is serialized
separately from PCM queues. A failed/uncertain or cancelled negotiation closes the connection
instead of assuming either side applied it. All raw descriptors must exactly
match the preconfigured directional permission: selection never guesses a
common lower quality format. Renaming or changing format/provenance requires
explicit permission reconfiguration in a future control runtime.

Supported raw sample representations are S16LE, packed S24LE, S32LE, IEEE F32LE
and F64LE. Float payload bits, including NaN payloads, negative zero and subnormals,
remain opaque; the transport does no floating-point arithmetic. Sample rate is a
positive u32 without a rate whitelist. Layout is a nonempty ordered list of unique,
explicit channel IDs, never an implicit speaker count or guessed downmix.
Descriptors are bounded to 4096 serialized bytes, names/IDs to 128 UTF-8 bytes.
This limits metadata/resources, not source precision or a predetermined channel count.

Packets contain a negotiated fixed positive number of whole sample frames.
`sample_bytes * channel_count * frames_per_packet + 40` must fit the configured
application datagram budget (at most 1200 bytes) **and** Quinn's current path
limit. Reduce frames per packet explicitly if needed; if even one sample frame
cannot fit, admission fails with a limit error. Fragmenting one sample frame
across datagrams is unimplemented. The bounded `enqueue_pcm` packetizer accepts sample-aligned source blocks,
retains at most one incomplete packet per stream and never pads/truncates it.
Each worker submission is limited to that stream's configured queue payload budget;
split larger source blocks into smaller sample-aligned submissions. Disconnect or
generation restart discards the incomplete tail. Finite-file tail flushing is not
implemented.
High rate means more packets, not reduced precision. Path shrinkage causes an
explicit per-stream send error/counter while the round still attempts siblings.
A different packet descriptor requires explicit permission reconfiguration and a new
Session in this foundation; never codec/quality fallback.

Raw 48 kHz stereo S16 payload is 1.536 Mbit/s, F32 is 3.072 Mbit/s before any
framing/QUIC/IP overhead. Raw F64 768 kHz eight-channel is 393.216 Mbit/s before
overhead. Admission budgets must account for actual chosen packet sizes and
sample rate. `max_bytes_per_second` bounds aggregate PCM+40-byte application
headers in each direction; it excludes QUIC/IP overhead and is **not** measured
link-capacity qualification or a bandwidth pacer. Producers must pace on their
sample clock; the example deliberately sends short bursts to prove bytes.

[Opus](https://www.opus-codec.org/) is the planned **explicit lossy** compressed
choice. Its codec tag is recognized but unavailable and rejected. It is never
advertised as working, silently substituted for raw or enabled by congestion.
Real codec parameter negotiation, encode/decode, latency, loss behavior and
quality qualification are pending. Encoded/DSD passthrough is a distinct future
path; this PCM protocol neither mixes nor claims to carry those bitstreams.

## Bounded PCM timeline

Each admitted stream has its own bounded reorder window in packet/sample time.
Consumer-driven `pull` returns exactly the next negotiated packet. Out-of-order
packets inside the window can arrive before that pull. Missing data produces
zero PCM and `missing_silence`; no concealed or invented seamless quality.
Duplicates, late packets, stale/future unadmitted generations, invalid timestamps,
unadmitted streams and oversized/malformed frames are rejected and counted.
A future packet beyond the window is dropped rather than moving playout forward
or growing latency. Reliable generation restart clears the old stream queue and
resets its sample timeline. Counters reset per generation; an integrated runtime
must collect history separately if desired.

The caller chooses initial prebuffer and invokes pulls at each stream's packet
cadence. This foundation has no hardware clock, drift estimation, resampling or
adaptive jitter controller. Failure to tick a timeline leaves queued data until
an explicit restart/disconnect; no wall-clock freshness guarantee is claimed.

Sender queues are separately bounded per stream, drop their own oldest pending
packet on overflow and increment counters. A round submits at most one packet
per nonempty stream with rotating priority. Quinn send-buffer space is checked
before enqueue to avoid its documented eviction of an older sibling datagram;
insufficient space drops/counts the new packet. QUIC connection-wide congestion
and receive-buffer drops still affect all streams. This provides bounded queues
and sibling progress, not reserved network bandwidth or loss immunity.

`Limits` configures connection count, admitted stream count, packets per stream,
datagram size, aggregate rate and control deadline. Permission lists themselves
must fit those budgets. The same `max_streams * packets_per_stream * max_datagram`
budget bounds each QUIC send/receive datagram buffer and application payload queue
per direction per connection; one partial sender packet per stream, map/Vec/TLS
metadata and headers add overhead.
Pending handshake count and buffers are also bounded. The API caller must not
create unbounded endpoints. No OS resources are published by admission.

Socket/control tasks allocate and use short worker-side locks. Realtime callbacks
must use a separately qualified, preallocated bounded handoff of descriptor +
generation + sample-frame offset + exact PCM blocks. No socket, JSON, TLS,
codec work or these allocating `pull`/`enqueue` calls belong in an audio callback.
That future timeline adapter joins the core's source/output edges; it must not
become another mixer or silently change format.

## Executable evidence and remaining gates

Run commands are in [network/README.md](../network/README.md); CI is isolated in
[network-audio.yml](../.github/workflows/network-audio.yml). The loopback exercise
uses two independent forward streams (S24/96 kHz/6ch and F64/192 kHz/4ch) plus a
reverse S32/44.1 kHz/stereo stream, compares independent expected sample words
byte for byte, restarts a generation and verifies disconnect silence. Tests also
cover S16/S24/S32/F32/F64, eight-channel layouts, 16-channel F64 packetization,
rates through 768 kHz and a
non-whitelisted rate, queue saturation, omitted/reordered/duplicate packets,
malformed data/control, unpaired certificates and directional permissions.
Loss/reorder is injected before authenticated datagram submission, not a claim
of measured real-network impairment. A slow/incomplete control request runs
alongside audio until its deadline closes the unsafe session.

Unqualified: two actual computers/LAN/Tailscale, native app capture and virtual
endpoints, physical playback, source precision throughout existing adapters,
realtime callback handoff, packet pacing, drift adaptation, adaptive jitter,
end-to-end latency, clicks/transition behavior, long-running/high-rate throughput
and endurance, production pairing/storage/revocation, Opus and encoded/DSD.
The PLAN's audio quality and ease-of-use release gates still apply. Passing
loopback transport tests does not satisfy them.
