# Bounded block fragmentation

`network/src/fragmentation.rs` implements optional MXAF version 2 block framing,
independent of the current MXAD version 1 packet format. It splits opaque blocks
inside sample frames when necessary. A single 160-channel F64 frame occupies
1,280 bytes and roundtrips across two datagrams with a 1,200-byte limit, including
NaN payloads, signed zero and low precision bits. No sample interpretation,
resampling, channel reordering or precision conversion occurs here. Positive
arbitrary rates and ordered channels remain the exact external descriptor's
responsibility.

## Integration boundary

This PR provides the production module and executable UDP loopback proof. The
parent network integration owns its library export and authenticated QUIC Session
hooks. Version 1 remains the default. The existing version 1 `Spec::validate`
and whole-frame MTU gate remain unchanged in this branch. Version 2 requires an
explicit reliable agreement on the full exact `Spec`, framing version, generation
and canonical datagram size before either direction uses this module. It must
not be selected from magic bytes or silently used when version 1 admission fails.

The caller must first validate the descriptor's format and provenance independently
of packet size, enforce exact directional grants, exclusions and consent, then
derive `Descriptor { stream, generation, block_bytes, frames_per_block }` from that
trusted agreement. A `Descriptor` alone does not authorize a stream or describe
its rate/precision/channel identities. Never feed datagram fields to `admit`.
Use one `Reassembler` per authenticated connection so one peer cannot impersonate
another peer's stream. UDP tests qualify framing and local datagram IO only;
they do not establish authentication, discovery, trust or physical audio delivery.
`backendAvailable` and `audioApplied` remain false.

Production worker sequence:

1. Configure `Config` and create `Fragmenter`/`Reassembler`. Configure the same
   canonical datagram limit on both sides, at most 1,200 bytes and within the
   actual transport path limit. A path limit change needs an explicit new framing
   agreement; changing canonical chunk size midway through a block is invalid.
2. After reliable exact consented admission, call `Reassembler::admit`. Reserve
   the sender's queues and actual fragmented wire rate independently. Application
   bytes per block are `block_bytes + 48 * ceil(block_bytes / (max_datagram - 48))`;
   QUIC/IP overhead is additional.
3. Call `Fragmenter::fragments(sequence, &block)` and send each yielded datagram
   from a paced, bounded worker queue. The iterator retains only borrowed source
   bytes and one bounded datagram at a time. It does not enqueue, schedule,
   retransmit or grant authority. Caller-owned sequence progression must not wrap
   or reuse a sequence in the same generation. Restart before exhaustion.
4. Feed received datagrams to `ingest(bytes, now)` using a monotonic `Instant`.
   `Pending` is incomplete; `Ready { stream, generation, sequence }` identifies
   a complete stored block using validated IDs; `Duplicate` emits nothing.
   Call `take(stream, generation, sequence, now)` to move a complete,
   unexpired `Block` out exactly once. The consumer must separately bound its own
   queues and recheck lifecycle authority before handing data to a sink.
5. At the consumer's negotiated sample cadence, call `advance` with the first
   still-acceptable sequence. Missing data is the consumer's explicit concealment
   policy; this module never labels a partial block successful or invents PCM.
   `advance` is trusted local input, cannot decrease and cannot change generation.
6. Call `expire(now)` from a bounded worker timer even without traffic. `ingest`
   and `take` also expire the targeted stream. Deadlines start at the first valid
   fragment, apply to incomplete **and complete unconsumed** blocks and are never
   extended by a duplicate or later fragment. These are reassembly deadlines,
   not source-clock/physical freshness measurements. Advancing the media cursor
   remains essential to rejecting an entirely late first fragment.
7. Revocation calls `revoke(stream)` immediately and invalidates the corresponding
   downstream lifecycle. It drops that stream's data and prevents readmission in
   the session. Restart without revocation uses a strictly newer generation and
   clears the previous window. `disconnect` drops all buffers and permanently
   closes the instance; create a new instance for an explicitly authorized session.

## Canonical wire envelope

All integers are unsigned big endian. PCM payload stays packed in its original
byte representation and exact negotiated order.

| Offset | Field |
| --- | --- |
| 0 | `MXAF` magic, 4 bytes |
| 4 | version u16 = 2 |
| 6 | fragment payload length u16 |
| 8 | stream u64 |
| 16 | generation u64 |
| 24 | block sequence u64 |
| 32 | sample-frame timestamp u64 = sequence × admitted frames per block |
| 40 | full admitted block length u32 |
| 44 | byte offset within block u32 |
| 48 | fragment payload |

Payload capacity is `max_datagram - 48`. Offsets must be exact multiples of this
capacity, and every fragment has exactly `min(capacity, block_bytes - offset)`
bytes. This permits splitting a sample or frame at any byte while ruling out
arbitrary overlaps, zero-length fragments and ambiguous fragment counts. The
receiver derives fragment count from its **admitted** block length; no count field
from the network controls allocation. Extra/truncated payloads, bad timestamps,
wrong total sizes, noncanonical offsets and oversized datagrams fail closed.

## Resource and lifecycle bounds

Configuration explicitly bounds block bytes (at most u32), datagram bytes,
stream identities, window slots per stream, aggregate payload/presence bytes and
timeout. Before admission, the receiver reserves
`window * (block_bytes + ceil(block_bytes / payload_capacity))` bytes for that
stream. Aggregate reservations cannot exceed `max_buffered_bytes`; failure leaves
the previous admission and sibling streams unchanged. Payloads and one-byte
presence entries allocate lazily only after the entire header matches admission.
Actual payload/presence usage never exceeds the reservation. Map bookkeeping is
separately bounded by `max_streams` and `max_streams * window` entries; the byte
budget does not pretend to measure allocator/map overhead or downstream queues.
Allocation failure returns an error; no partial slot is installed.

One stream cannot consume another's reservation. Window position changes only
through the local consumer cursor; a future datagram cannot advance it. Identical
duplicates are idempotent. Conflicting bytes in the same canonical slot poison
only that block and free its buffers; malformed noncanonical chunks do not damage
an existing valid block. Poisoned, expired and consumed blocks retain bounded
tombstones until the consumer advances past them, preventing resurrection/replay.
Revoked stream identities also retain bounded tombstones until session teardown;
they count against `max_streams`, so repeatedly changing stream IDs cannot grow
an unbounded revocation history. New generations cannot revive a revoked grant.

The caller supplies monotonic time. Any rollback across `ingest`, `take` or
`expire` returns `TimeRollback` before modifying buffers or releasing output.
`expire` visits at most the configured total window entries. An ingest/take scans
at most its admitted stream's window, and copies/compares at most one fragment.
Memory/time budgets are explicit admission settings, not product format limits.

## Executable proof

This dependency-free module can be checked without compiling QUIC dependencies:

```sh
proof_dir=$(mktemp -d /tmp/mixyard-fragmentation-proof.XXXXXX)
rustc --edition=2024 --test -C debuginfo=0 -C codegen-units=1 \
  network/tests/fragmentation.rs -o "$proof_dir/tests"
"$proof_dir/tests" --test-threads=1
```

The tests import the exact production source. They exercise full-frame-over-MTU
F64 preservation, arbitrary opaque sample bytes/timestamps, out-of-order receipt,
loss without partial output, duplicate/conflict/truncation/overlap/huge-size
rejection, complete-block expiry, rollback, bounded reservations and windows,
generation/revoke/disconnect isolation, canonical tails and real connected UDP
loopback. Parent integration must additionally prove authenticated reliable v2
negotiation, exact permission/provenance/exclusion handling, wire-rate admission,
bounded pacing/queues, and lifecycle propagation into the real PCM consumer.
