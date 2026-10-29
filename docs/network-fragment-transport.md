# Authenticated fragmented raw blocks

`network::fragment_transport::fragment_endpoint` is an explicit worker-side
transport for canonical raw blocks larger than a QUIC datagram. It reuses the
existing mutual TLS, normal WebPKI verification and exact certificate pins. Its
private base Session has **empty audio grants**; the wrapper validates separate,
exact directional `Permissions`, full source provenance and receive exclusions.
Management permissions do not grant audio publication or reception.

The default v1 `endpoint` and its framing remain unchanged. Both block endpoints
must explicitly use this wrapper. Reliable version-2 offers carry the entire
canonical Spec, nonzero increasing generation and exact fragment datagram size.
An existing v1 worker rejects that offer; there is no downgrade or retry with a
narrower format. The wrapper currently reuses the base `mixyard-audio/1` TLS ALPN
and distinguishes this opt-in mode through strict reliable control framing. This
is a cooperative Mixyard extension, not another device's interoperability profile.

The configured fragment size must fit both local limits and the current Quinn
path limit. Admission rejects a larger requested size; it does not silently
narrow chunk geometry. Path shrinkage later produces an explicit send error.
The loopback fixture requests 1000-byte fragments: a 1200-byte application
fragment is not assumed to fit every initial QUIC path.

## Caller contract

After starting one `run` task on each authenticated session, the sending host
calls `negotiate(Offer)` before `send(stream, generation, raw_bytes)`. The original
S16/S24/S32/F32/F64 little-endian representation, positive source rate, ordered
channel IDs, frame count and provenance remain unchanged. IEEE nonfinite bits,
signed zero and integer low bits remain opaque. There is no codec, resampling,
channel mapping, native callback or mixer in this path.

`send` borrows exactly one admitted block, accepts at most one in-flight block
per stream and yields between fragments. It waits for QUIC capacity rather than
evicting an already queued sibling datagram; one bounded deadline covers the
whole block. Completion means local enqueue, not remote consumption or physical
delivery. Producers own source-clock pacing. QUIC congestion is connection-wide;
this API does not promise reserved bandwidth, independently paced fragments or
loss immunity. No unbounded submission queue is supplied.

`receive(stream, generation, sequence)` waits for one complete, unexpired block,
returning `(Spec, Block)` with the original descriptor and exact sample-frame
offset. `take` is its synchronous nonwaiting equivalent. Neither returns partial
PCM. The host chooses and counts silence for a missing block, then explicitly
calls `advance` to publish its monotonically increasing consumer cursor. The
bounded reassembly window cannot move itself to make a future packet fit. A
consumed sequence cannot replay. These are allocating worker APIs, not realtime
callback operations; the host owns an explicitly mapped consumer clock.

## Resource and lifecycle limits

All granted streams are checked before endpoint creation, including full
per-stream-window reservation and aggregate application wire rate. Rate admission
counts **every** 48-byte fragment header at the actual negotiated chunk geometry,
including tiny blocks. This excludes QUIC/IP overhead and is not a measured link
capacity. The existing bounded QUIC buffers are independent of block reservations.

Reassembly reserves payload plus fragment-presence bytes within its configured
aggregate budget, with bounded map metadata in addition. Rust map allocation can
abort on process OOM; the payload bound is not a total-process memory guarantee.
At most one borrowed outbound block per stream is active. Returned owned blocks,
caller-retained buffers and application-created pending futures are outside this
transport's retained-memory budget and must be bounded by their owner.

A worker timer expires complete and incomplete retained blocks even without new
traffic. Missing, malformed, conflicting, late and unadmitted fragments cannot
create a grant or release partial data; the underlying reassembler preserves
bounded tombstones until consumer advance. Control frames, concurrent control
work and control rate are bounded. Malformed or uncertain reliable negotiations
close the session instead of assuming one peer applied an offer.

A canceled/failed send invalidates that stream generation locally; an explicit
higher reliable generation is needed to restart. Reliable admission cannot
replace a stream while its send is busy. A pending replacement reserves the
stream against newly starting old-generation sends. Each fragment's authority
check and actual QUIC enqueue poll share the revocation lock; a pending poll
releases that lock, and revocation wakes that stream's waiter before it can
enqueue later. Local send or receive revocation removes
that direction until a new session, including revocation before admission.
Sibling grants remain independent. Already queued datagrams and returned caller
blocks cannot be recalled; remote consumption needs its own receive revocation.
Closing or canceling the sole receive worker closes the session and discards
retained reassembly state. No automatic reconnect or replay occurs.

## Executable evidence

`network/tests/fragment_quic.rs` uses actual mutually authenticated loopback QUIC,
three independent directions and a file destination. It compares original Spec
and byte oracles for 160-channel F64 at 44117 Hz (a single 1280-byte sample frame
exceeds the datagram), S24 and S32 blocks. It exercises generation restart,
consumed-block replay rejection, independent sender revocation, receiver
revocation, disconnect, exact fragment-size negotiation, aggregate header-rate
admission and premix exclusions. The isolated file is removed after comparison.
Current-thread manual-poll regressions exercise revocation with a final fragment
blocked on capacity, replacement admission racing a newly starting old send,
and cancellation followed by explicit generation recovery with sibling progress.

The underlying fragmentation tests additionally exercise reordered/duplicate/lost
chunks, complete unconsumed expiry, canonical malicious input, failed replacement
reservations and time rollback. Existing v1 QUIC and descriptor tests remain in
the standalone network suite. Run the ordinary required checks with an authorized
private build allocation:

```sh
cargo fmt --manifest-path network/Cargo.toml -- --check
cargo clippy --locked --manifest-path network/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path network/Cargo.toml
```

Local proof recompiles actual production sources against exact-lock read-only
cached dependencies; hosted Cargo and three-OS results are separate gates.
Unqualified: native consumer delivery, hardware clocks, cross-host transport,
physical latency, endurance/high-rate capacity, codec integration and production
pairing storage. This work never sets `backendAvailable` or `audioApplied`.

Optional [authenticated codec delivery](network-codec-transport.md) uses a
separately admitted variable-envelope MXAFv3 mode with reserved maximum windows.
The v2 raw block contract remains exact; encoded bytes never become a raw Spec.
