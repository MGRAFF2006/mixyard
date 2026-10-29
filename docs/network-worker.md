# Paced network workers and local arrival trends

The optional `mixyard_network::worker` module joins the production paired QUIC
`Session` and raw `Playout` APIs to monotonic send/receive loops. It opens no audio
device, changes no graph, and supplies no native compatibility badge.
`backendAvailable` and `audioApplied` remain false. The security, consent,
provenance and exclusion rules in [network audio](network-audio.md) still apply.

## Running the workers

Keep one `Session::run` task alive for each paired endpoint. Negotiate each exact
`Offer` reliably, then construct `SendWorker::new(session, offers, local_anchor)`.
Populate the existing bounded `enqueue`/`enqueue_pcm` queues with exact raw PCM.
Call `run(stop_receiver)` to send at each descriptor's own packet cadence.
`tokio::sync::watch::channel(false)` supplies a stop receiver; sending `true`,
closing/changing the watch, dropping the worker or aborting its future cancels
its owned generations. Construction exclusively claims each sender generation.
An already claimed/cancelled generation needs a higher reliable generation,
including after construction fails partway through claiming a list.

`ReceiveWorker::new(session, Vec<(Offer, JitterConfig)>, local_anchor)` claims one
exclusive Playout per generation and allocates its bounded raw blocks/window.
Call `run(bounded_mpsc_sender, stop_receiver)`. Each `RenderedBlock` reports the
exact offer, local scheduled block start, local output sequence, Playout counts,
arrival estimate, and any `(old_target, new_target)` local delay decision.
Use `copy_into` with one packet's caller-owned raw byte storage. It returns false
and zeros that storage if its generation was invalidated. This API is allocating
worker-side delivery; it is not a native callback API.

The output channel must be Tokio's bounded `mpsc::channel`, with an explicit
caller-chosen capacity. Full output drops the new block, increments
`ReceiveReport::output_drops`, and continues sibling timelines. The worker never
waits on a slow downstream consumer. Published data already copied successfully
cannot be recalled; downstream delivery must obey its own generation binding.
Queued old-generation blocks reject copying after receiver cancellation,
revocation or restart. Their private raw storage is scrubbed when rejected or
dropped; already published blocks are not physically recalled from a caller.
Inactive receive tracks retire; an empty worker exits. A new generation always
requires a new worker/handle and explicit new local anchor.

## Cadence, late producers and lifecycle

For packet frame count `F`, exact positive rate `R`, and local anchor `A`, source
slot `n` is due at `A + ceil(n * F * 1e9 / R)` nanoseconds. Deadlines derive from
that original rational cadence, never repeated rounded durations. At a late
wake, the controller directly selects the newest elapsed slot and counts missed
slots. It does not walk the missed duration or burst through timer backlog.
Rates faster than nanosecond timer resolution therefore skip/count slots rather
than claiming impossible scheduling precision. This is a network scheduling
policy, not proof that the OS meets an audio deadline.

`Session::flush_due(&[(Offer, absolute_sequence)])` validates the exact admitted
version, generation and descriptor under the sender lock, expires only that
stream's older queued packets, and submits at most its current slot. Duplicate,
backward, inactive or wrong-descriptor requests fail. Even an empty due slot
advances the producer assignment cursor; an expired partial packet is scrubbed.
Late producer data cannot later replay the expired slot. `paced_expired` counts
queued packets discarded by this boundary; `SendReport::skipped_slots` counts
missed cadence slots, including empty ones. The existing congestion/send-error
counters still apply. There is no quality fallback, resampling, channel reorder
or byte-level sample arithmetic.

Each stream has its own cadence, queue and generation. Simultaneously due streams
rotate first attempt priority; QUIC congestion remains shared. A restart retires
only that sender's old cadence. Its old lease cannot cancel the newly admitted
generation. Other streams continue at their own negotiated rates. The legacy
`flush_round` still services unpaced streams and never steals a paced stream's
packet. Mixing legacy and paced ownership for one generation is unsupported.

Sender cancellation synchronously clears only its matching local generation's
pending packets/partial data and blocks further enqueue for that generation.
It leaves a generation tombstone, so only a higher reliable generation can
restart. It does not recall datagrams already handed to QUIC or audio already
accepted by another endpoint. Stop remote consumption with that endpoint's
receive cancellation/revocation; no new remote-revocation control message is
claimed here. Receiver cancellation clears that matching generation's ingress,
arrival observation, Playout storage and controller. Permission remains available
for a higher reliable generation; explicit `revoke_receive` removes the grant.
Session close clears all local sender queues and invalidates receive handles.
Sender locks are short synchronous worker mutexes; no socket await is held under
them. Cancellation performs bounded worker work, never a callback operation.

## Adaptive jitter and observation meaning

Ingress retains only the latest validated, unique, strictly increasing source
packet position, its exact Offer, a local `Instant`, and an advancing-observation
count. It rejects inactive/incorrect generations. Reordered older packets can
still play but cannot refresh the latest observation. Duplicates, malformed data,
late/window-rejected packets and stale generations cannot refresh its timestamp.
Restart/revoke/cancel clear observations. The worker may sample fewer observations
than ingress accepts; `Estimate::sample_count` counts accepted controller samples,
whereas `ArrivalObservation::count` counts advancing ingress observations.

The std-only `worker::control` module estimates interarrival variation with a
constant-space EWMA of absolute local interarrival error relative to the exact
source sample distance. A target requests `min_frames + ceil(4*jitter*R/1e9)`,
clamped to the configured maximum; observed missing frames request an increase.
At most one packet of change occurs per configured adjustment interval (itself
at least a packet period). Initial/min/max targets must fit the admitted window.
Targets are local configuration, never measured path or physical audio latency.

`Playout::set_target_buffer_frames` applies an explicit bounded target without
moving its committed cursor backwards. Increasing delay produces counted
prebuffer/overlap silence; reducing it produces counted skipped/discarded frames
at the next render. Existing `prebuffer_frames`, `early_frames`, `missing_frames`,
`skipped_frames` and `discarded_buffered_frames` expose the actual local decisions.
`target_changes` counts decisions; the worker publishes each changed target.
There is no hidden resampling or promise of click-free delay changes.

After at least eight controller samples spanning `min_drift_span`, the estimate
exposes source sample slope relative to local arrival time as
`arrival_rate_error_ppm` and an empirical `uncertainty_ppm` envelope from jitter
and local one-sample quantization. They include changing network path delay and
worker scheduling; the envelope is not a guaranteed physical uncertainty bound.
The estimate also exposes its last local arrival, age and `Fresh`/`Stale`/
`Unavailable`/`Inactive` validity. Reads do not refresh it. A fresh observation
after a gap longer than `fresh_for` restarts the estimate window. Stale samples
never drive target changes. No remote timestamp or RTT enters this algorithm.

There is **no remote/native clock mapping**. An arrival slope does not identify
capture time, clock offset, hardware drift, one-way delay or end-to-end latency.
Neither RTT/2 nor queued sample duration supplies any of those values. Native
hardware observation/resampling owns actual clock correction. An eventual native
handoff must preserve the exclusive preallocated f32/f64 queue and exact
`Binding { generation, revision }`, `Format { sample_rate_hz, ordered_channels,
precision }`, `Stamp { binding, clock_id, sequence, first_frame, captured_ns,
deadline_ns }` contract. A native clock ID and valid, fresh measured mapping with
sample count/uncertainty must come from that clock's owner; local packet arrival
cannot fabricate `captured_ns` in another clock. Integer PCM and nonfinite raw
float bytes remain on this separate exact-byte path; finite native float queues
cannot qualify those raw formats. No native queue integration is claimed.

## Executable evidence

Run the isolated workspace's normal checks:

```sh
cargo fmt --manifest-path network/Cargo.toml -- --check
cargo test --locked --manifest-path network/Cargo.toml --test worker -j1
cargo clippy --locked --manifest-path network/Cargo.toml --all-targets -j1 -- -D warnings
```

`network/tests/worker.rs` exercises production asynchronous workers on mutually
authenticated loopback using actual monotonic timers, independent 25/50-frame
cadences, F64 nonfinite/low-bit words, ordered channels and exact S24 raw bytes.
It checks packets arrive across actual time instead of one initial burst,
restart/cancellation isolation, queued-output invalidation, a full bounded output
queue, a replacement generation and continued sibling progression. Deterministic
fixtures exercise rational non-whitelisted/high-rate cadence, an hour-long late
wake in constant work, empty slots/partial expiry, loss/reorder silence, bounded
jitter target changes, a 200 ppm arrival trend, stale samples and wrong epochs.

Local constrained verification recompiles production library/test sources with
Rust 1.99 and the exact locked cached dependencies into private outputs, rather
than running Cargo builds. This is executable source evidence; hosted Cargo and
Clippy remain separate checks. It is not two-host/LAN/Tailscale, physical/native
playback, sustained throughput/endurance or hardware synchronization qualification.
