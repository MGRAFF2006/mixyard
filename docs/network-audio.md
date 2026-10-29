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
waiting. One inbound control exchange runs concurrently with datagram reception;
Quinn limits inbound bidirectional streams to two, so one successor can wait in
bounded transport storage until the active exchange finishes. Outbound negotiation is serialized
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

`Spec::validate_descriptor(sender, receiver)` checks the canonical raw format,
identity and complete provenance independently of packet framing. Block and
protocol adapters can reuse it without inheriting the original datagram MTU.
It grants no send/receive permission or resource admission: each adapter must
also check its exact payload, aggregate rate, queues and framing budgets.
The existing `Spec::validate` and directional permissions retain the v1 datagram
limit below; a large valid descriptor does not enable that transport implicitly.

Packets contain a negotiated fixed positive number of whole sample frames.
`sample_bytes * channel_count * frames_per_packet + 40` must fit the configured
application datagram budget (at most 1200 bytes) **and** Quinn's current path
limit. Reduce frames per packet explicitly if needed; if even one sample frame
cannot fit, v1 admission fails with a limit error. The separate opt-in
[authenticated fragmented-block endpoint](network-fragment-transport.md) lifts
that framing limit with exact reliable version-2 agreement and its own block,
fragment-header rate and reassembly admission. It does not narrow this v1
descriptor or silently switch modes. The bounded `enqueue_pcm` packetizer accepts sample-aligned source blocks,
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
must fit those budgets. Application payload queues use
`max_streams * packets_per_stream * max_datagram` per direction per connection.
Each QUIC send/receive buffer additionally reserves 128 bytes per packet for
transport metadata, using checked arithmetic. Session creation checks Quinn's
reported empty send capacity against the full configured datagram size; an
impossible configuration rejects explicitly instead of permanently dropping
admitted packets. One partial sender packet per stream and map/Vec/TLS metadata
add further overhead.
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

## Executable timing and health observations

`Session::probe_rtt().await` performs a caller-triggered, authenticated application
control round trip on the existing paired QUIC connection. `timing_snapshot().await`
returns a serializable `timing::SessionTiming` without sending traffic. These are
allocating/locking network-worker APIs, never realtime callback APIs. The
standalone `telemetry` example emits JSON observations for initial, queued sender,
fresh RTT, queued receiver, stale and disconnected states using ephemeral
certificates and encrypted loopback transport. It also compares the delivered
raw sample bytes; no devices are opened.

Quinn 0.11.12 `Connection::rtt()` and quinn-proto 0.11.19 `PathStats::rtt` expose
an estimator without the latest measurement timestamp. The estimator is seeded
from the transport's initial RTT (default 333 ms). Reading these stats, observing
new packets, or completing a TLS handshake cannot establish a fresh application
probe. We therefore do not publish Quinn's estimator as a measured RTT. The
application measurement includes stream opening, local scheduling, reliable
transport/retransmission and peer worker turnaround, measured entirely with
local `std::time::Instant`. It is not a pure wire RTT or an audio delivery time.

The public snapshot contract is:

- `connected`: no local QUIC close reason at observation. This is not proof that
  the remote process is still alive; undetected failures wait for QUIC timeout.
- `observed_at_session_ms`: local monotonic milliseconds since Session creation,
  meaningful only within this Session. No UTC or remote timestamps are used.
- `application_rtt.kind = "application_control_round_trip"`; `value_ms`,
  `measured_at_session_ms`, and `age_ms` are null until a successful matching echo.
  `validity` is `unavailable`, `fresh` (age at most `fresh_for_ms = 3000`), `stale`,
  or `disconnected`. Repeated reads never refresh a sample. Disconnection takes
  precedence; historical values retain their original timestamps and ages.
  `last_attempt` is `never`, `in_flight`, `succeeded`, `failed`, `timed_out`, or
  `cancelled`, with `last_attempt_at_session_ms`. Failed/cancelled attempts do not
  replace the last successful sample or its age. A recent successful sample can
  coexist with a failed latest attempt; callers must inspect both fields.
- `send_streams` and `receive_streams`: bounded by the directional admitted
  stream budget. Each contains `stream`, `generation`, `sample_rate`,
  `frames_per_packet`, `queued_packets`, `queued_sample_frames`,
  `queued_sample_ms`, `partial_sample_frames`, `partial_sample_ms`,
  `capacity_packets`, `capacity_sample_frames`, `capacity_sample_ms`,
  `next_sample_frame`, `buffered_timeline_sample_frames`, `buffered_timeline_ms`,
  `consumer_prebuffer_ms`, and per-stream loss/error `counters`. Session-level
  `rejected` counters describe malformed/unadmitted datagrams.
- Occupancy milliseconds are `sample_frames * 1000 / negotiated_sample_rate`,
  per stream, independent of channel count and codec width. Sender partial
  packets are reported separately and are not ready to transmit. Receive timeline
  span runs from the next pull to the end of the furthest queued packet and
  includes missing slots; occupied frames exclude those holes. Sender timeline
  span is null. `next_sample_frame` is the next consumer sample position on
  receive or next full packet's assigned sample position on send (null on u64
  exhaustion). Capacity describes the configured bound and is never actual
  playout residence. Neither occupancy nor span measures elapsed residence.
- `consumer_prebuffer_ms` is always null: the consumer owns initial prebuffer
  and pull cadence, and has not reported that configuration to this worker.
  A generation restart resets that stream's queue, timeline and counters;
  sibling generations remain independent. A disconnected snapshot flushes
  receive packets and sender state; admitted receiver metadata/counters remain,
  while sender entries are removed. QUEUED bytes inside QUIC, downstream devices,
  or the caller's own buffers are outside the per-stream occupancy observation.
- `clock_sync.state = "unavailable"`, `offset_ms = null`, and
  `uncertainty_ms = null`. There is no cross-host clock mapping or drift estimate.
  `one_way_network_ms` and `end_to_end_audio_ms` are always null. RTT/2 is never
  labeled measured one-way delay, and RTT is never added into total audio latency.
  Route alignment requires separately qualified clock and endpoint observations.

Each probe is exactly 16 reliable bytes: `MYTP`, big-endian u16 version `1`,
u16 kind (`1` ping, `2` echo), and u64 correlation nonce, followed by FIN. Echoes
are the same size as requests and never recursive. The magic lies outside the
accepted JSON length range, so a probe cannot deserialize as an `Offer`. No peer
clock value is trusted. Exact mTLS leaf pins, stream permissions, protocol version,
40-byte audio header, PCM bytes and datagram budgets are unchanged; probing an older
peer without this optional envelope can close that connection, without yielding
a timing value. Use probes only when the paired peer supports this extension. Probe
requests never grant PCM admission. The application nonce is connection-local
correlation; QUIC/TLS supplies authentication and replay protection.

Only one local probe may be in flight, and attempts start no faster than once per
second. Concurrent/rate-exceeded probe calls fail immediately without spawning
tasks. Probes and outbound PCM negotiation share the reliable-control gate, so
they cannot reset one another as simultaneous inbound requests. The probe's
deadline is the lesser of one second and `control_timeout`, including the bounded
wait for that gate and stream opening. RTT starts after obtaining the gate;
local admission waiting is not counted as network round-trip time. Cancellation
releases the probe slot and its QUIC streams without invalidating negotiated
audio permissions. Failure to deliver an echo for a fully validated probe
(including a caller that stopped receiving after cancellation/timeout) is local
to that optional stream; malformed requests still fail closed. Probes
share the single inbound reliable control worker, polled directly in the receive
loop so reply FIN and worker availability complete together (avoiding a spawned
task-completion race on successive requests). The worker does not accept the next
control until the active exchange completes; otherwise an abandoned probe could
reset a subsequent valid negotiation after the local caller released its gate.
At most one successor waits under Quinn's two-stream receive limit, rather than
an unbounded application queue. Datagrams remain independently serviced. Each
accepted bidirectional control counts toward a
`max_streams + 16` per-one-second-window limit (a full configured admission
burst plus telemetry headroom); exceeding it closes the paired session. A malformed
or incomplete inbound frame fails closed at the existing control deadline. This
bounds work per connection and response size; it cannot remove QUIC congestion
shared with a peer or protect application polling by an unbounded local caller.

Tests cover actual encrypted matching echoes, a delayed peer, no initial sample,
stale/disconnected states, an injected local monotonic observation time, invalid
nonce/version/kind/length/trailing data, slow peers, cancellation and flooding.
Production-worker regressions pause peer turnaround until after probe cancellation
or timeout and then verify exact audio delivery; another overlaps probing and
negotiation. All three reproduced connection teardown before the repair. Two
further regressions cancel or time out a probe and immediately negotiate a new
generation before peer turnaround (20 attempts per variant); both reproduced
busy resets before serializing inbound acceptance as well as outbound calls.
The mixed-rate queue test distinguishes holes and partial packets from occupancy,
checks restart/sibling isolation, and preserves the existing raw precision oracles.
These results establish local application RTT and worker queue semantics only.
Two-machine LAN/Tailscale behavior, native capture/playback, hardware clocks,
physical one-way/end-to-end latency, drift and synchronized route playout remain
unqualified.
