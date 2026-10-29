# Local sample-aligned network playout

Status: executable foundation, 2026-10-29. This joins admitted encrypted raw PCM
packets to caller-owned sample blocks. It opens no audio device and changes no
source selection, microphone consent, exclusion, native endpoint or routing graph.
The [network protocol/security contract](network-audio.md) remains authoritative.

## Worker and consumer contract

After reliable stream admission, create one handle per stream generation:

```rust,ignore
let mut playout = session.create_playout(stream_id, PlayoutConfig {
    anchor: std::time::Instant::now(),
    target_buffer_frames: 240,
})?;
let receipt = playout.revocation();
// On the owning worker, periodically, before/after consumption:
session.service_playout(&mut playout)?;
// On the same exclusive owner, with preallocated caller storage:
let report = playout.render_into(local_block_start, &mut raw_interleaved_bytes)?;
```

`Receiver` provides the same `create_playout` / `service_playout` boundary for
non-socket callers. Creation allocates bounded storage and preserves the admitted
`Spec` (including original provenance, ordered channel IDs, exact positive u32
sample rate and raw S16LE/S24LE/S32LE/F32LE/F64LE). `spec()` exposes that identity;
`generation()` exposes its epoch. No float arithmetic touches PCM: NaN payloads,
negative zero, subnormals and low integer bits remain byte-exact. No codec,
resampling, channel mapping, downmix, precision reduction or hidden quality fallback
is introduced.

The handle exclusively consumes this stream generation; legacy allocating `pull`
and second handles fail. Creating after legacy pulls also fails. Reliable restart
requires a new handle and an explicit new anchor; it never infers a clock mapping
from the prior generation. Other streams keep their own independent clocks,
permissions, formats, windows and counters. A dropped handle cannot be replaced
within its generation; deliberately restart to establish a new timeline.

`service_playout` transfers the receiver's bounded packet window, without decoding
samples, into preallocated slots. It also publishes the consumer's current packet
cursor to ingress. Service regularly, including during loss; otherwise ingress
can retain an old window and reject newer packets beyond it. This is a counted
loss boundary, not an automatic growth of latency or memory. A handle from another
active receiver is rejected. Inactive old handles only clear their own storage.
Service, construction, sockets/TLS/JSON and the Session mutex are worker operations.

`render_into` takes exclusive `&mut` ownership and does not access Session,
network tasks or their locks. Construction preallocates all packet slots and PCM
storage. The scoped allocator test observes allocation, zeroed allocation,
reallocation **and deallocation** on the actual rendering thread, across audio,
prebuffer, overlap, holes, skips, errors and inactive clearing. It reports zero
operations for these exercised paths. This is not proof of a realtime callback,
a concurrent worker-to-callback handoff, scheduling deadlines, hardware clock
behavior or all future integrations. The current API requires the caller to
arrange exclusive ownership; no new cross-thread audio queue is supplied.

## Exact local timing and silence rules

`local_block_start` is the local monotonic time represented by the **start** of
this output block. It can be a scheduled block time; passing observation/call time
instead exposes caller scheduling jitter as skips/overlap. It is never a remote
capture timestamp or proof of physical playback. For exact negotiated rate `R`,
target buffer `B` sample frames, and anchor `A`, the desired first frame is:

```text
floor((local_block_start - A) in nanoseconds * R / 1_000_000_000) - B
```

Integer arithmetic defines this mapping, including negative times before the
anchor. The target is an explicit nonnegative number of sample frames, bounded
by the admitted window. It is configuration, not measured elapsed residence.
All-zero little-endian samples are silence for every supported raw format.

- Positions before frame zero produce counted `prebuffer_frames` silence. A
  block may cross the prebuffer boundary and then deliver available PCM.
- Positions before the next unconsumed frame produce counted `early_frames`
  silence. Overlapping/early calls never replay committed audio. The remaining
  part of a block may still consume new frames.
- If a nonempty block begins after the cursor, skip directly to its desired
  first frame, counting `skipped_frames`. Retire expired slots with work bounded
  by the window, even after a long pause; never walk the entire missed duration.
- An available packet can be split across arbitrarily sized whole-frame blocks.
  A block can span packets. Channel order and raw bytes remain unchanged.
- Missing frames produce counted `missing_frames` silence and irrevocably advance
  the cursor. A packet arriving partway through its slot may supply its unexpired
  suffix; its expired prefix is counted in `late_prefix_frames`. Wholly late
  packets are dropped/count as `late_packets`. No old PCM fills a gap.
- Reordering is accepted within the fixed window. Duplicates and packets beyond
  it are dropped/counted separately, both at ingress and after handoff. A far
  future packet never moves the clock. Invalid/stale generations never admit
  data or restart a clock.
- An output length not divisible by exact bytes per interleaved frame, a block
  larger than the configured window, a backwards observation, or exhausted u64
  sample position fails closed: zero the entire block, count rejected block and
  bytes, return a typed error, leave the cursor unchanged. Empty blocks advance
  no samples. A forward `now` is not a promise that the caller met a deadline.

At extreme legal rates, nanosecond timestamp resolution cannot name every single
sample boundary (at u32::MAX, one nanosecond spans more than four frames). This
mapping remains explicit and counted; it does not lower the negotiated rate or
invent fractional clock evidence. Callers needing another clock mapping must
supply and qualify it separately. Local `Instant` arithmetic is portable; the
fixtures use checked additions for their known bounded schedules.

## Nonblocking lifecycle and ownership boundary

Local receive revocation, reliable generation restart, disconnect, cancellation
of the receive worker, or receiver destruction atomically invalidates the old
handle. No handoff tick is needed to prevent old-generation output. Revocation
removes that receive permission until a new Session, including before initial
admission; sibling streams continue.

Invalidation uses one atomic operation and **never waits** under a Session lock,
spins on a renderer, or blocks network/control progress. A render enters a gate
before touching PCM and commits the block with an atomic compare-and-exchange:

- If invalidation wins while rendering, it marks `Pending` and immediately lets
  the worker continue. The renderer zeros the whole output and retained slots,
  then acknowledges `Complete`. All block frames count as `inactive_frames`.
- If render commits first, the caller owns that block. Later invalidation cannot
  recall bytes already handed to the caller, a downstream queue or a device.
- If no render is in flight, invalidation immediately becomes `Complete`: future
  renders produce counted inactive silence. Disabled private packet bytes are
  inaccessible to rendering and are zeroed at the next render/service/drop.
  This is logical PCM clearing, not a secure-erasure guarantee for allocator/OS
  memory or copies already owned by the caller.

`Playout::revocation()` supplies a clonable receipt before moving the handle;
`Receiver::revoke` / `Session::revoke_receive` return the same receipt. Its
`status()` is `Active`, `Pending`, or `Complete`; snapshots expose the state too.
Pending is **not** safe revocation completion. A consumer descheduled indefinitely
can leave a receipt Pending indefinitely. There is no invented wall-clock timeout
or realtime guarantee. The application may stop downstream consumption while
pending and must observe completion before claiming an overlapping output was
scrubbed. A generation restart can admit a new independent handle while the old
one is pending, but cannot restore the old gate. Dropping the exclusively owned
handle clears its storage and completes its receipt.

The deterministic race test pauses the real renderer after copying PCM into the
actual caller block. The invalidating worker must return a Pending receipt while
that renderer remains paused; only resuming and scrubbing can make it Complete.
It checks the final output, retained packet bytes and slot tags, not a simulated
counter alone.

## Memory and telemetry

For packet count `P`, exact frame width `W` and frames per packet `F`, a handle
owns `P * W * F` preallocated payload bytes plus `P` slot tags and bounded admitted
metadata. Target buffer and maximum render size are at most `P * F` frames. The
receiver retains its existing independent bound, so worker + handle payload
capacity is at most twice the admitted per-stream payload window. It does not
include QUIC/TLS, caller output blocks or downstream queues. Generations cannot
create handles from datagrams. Callers own old handles/receipts and must drop
unneeded ones rather than accumulating old generations themselves.

`snapshot()` reports stream/generation, lifecycle, rate, target/capacity/buffered
frames and their sample-domain milliseconds, exact payload capacity, cursor and
saturating counters. Buffered frames exclude consumed prefixes and missing slots.
Counters distinguish audio, missing/prebuffer/early/inactive silence, local skips,
late prefixes, whole late/duplicate/window drops, retired buffered frames and
rejected blocks/bytes. Receive ingress counters remain separately available via
`Session::counters`; add both stages when reporting transport drops. Snapshots
are current local observations, not a cross-thread atomic history or lifetime
counter ledger. Collect per-generation history separately. Existing
`SessionTiming.consumer_prebuffer_ms` remains null because its legacy worker
snapshot does not incorporate a consumer handle's configuration.

No queue milliseconds, configured target buffer or RTT is labeled measured
one-way network or physical audio latency. No RTT/2, peer clock offset,
synchronization, drift estimate, adaptive jitter control or end-to-end latency is
supplied. Anchoring a sample clock locally does not synchronize two computers.

## Run and qualification limits

```sh
cargo test --locked --manifest-path network/Cargo.toml
cargo run --locked --manifest-path network/Cargo.toml --example playout
cargo clippy --locked --manifest-path network/Cargo.toml --all-targets -- -D warnings
```

The runnable example and `playout_quic` test use actual mutually authenticated
QUIC loopback, three independent streams (S24/96 kHz/6ch, F64/44117 Hz/4ch,
F32/192 kHz/mono), source block packetization, consumer packet/block splits,
explicit prebuffer, restart, revoke and disconnect. Byte oracles are independent
signed/IEEE sample words. Block times are **injected local monotonic schedule
fixtures**, not measured native callback times. The example prints per-stream
playout observations and verifies the output bytes; no device or speaker opens.

Deterministic tests additionally cover all five raw types, irregular and extreme
positive rates, ordered 16-channel high precision, partial late arrivals, holes,
reorder, duplicate and window drops, early/overdue blocks, invalid blocks, an
aggregate three-stream lifecycle, and 64 bounded windows with sustained injected
loss/reorder. They are fixtures, not native capture/playback evidence. Focused CI
runs this standalone network workspace; native tests remain Linux-only work owned
separately. Windows/macOS network checks need actual CI results before claiming
verified execution there.

Still unqualified: physical devices, native capture precision and consent flows,
hardware sample clocks, source/destination drift, cross-thread callback handoff,
resampling, adaptive jitter, pacing, packet-age/physical latency calibration,
two-host LAN/Tailscale behavior, heavy/high-rate throughput and endurance. A
successful finite fixture or zero observed heap operations cannot establish
these gates. No held experimental native module is used.
