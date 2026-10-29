# Bounded PCM worker handoff v1

`runtime::worker::Queue<S>` is the shared in-process exact-format handoff for
Float32 or Float64 PCM. Each queue has one exclusive owner (`&mut`); it is **not
a concurrent SPSC**, native callback transport, or proof of realtime scheduling.
Native adapters separately qualify their interthread queues and ownership.

Construction admits a positive exact rate, unique ordered opaque channel IDs,
the sample type's precision, immutable generation/revision binding and clock ID.
Slots, maximum frames and PCM byte budget are caller selected. No stereo, 48 kHz
or Float32 ceiling is imposed here. The existing policy processor still admits
only its documented mono/stereo layouts: a general queue does not extend that
planner or grant native format support. Integer/encoded raw bytes stay in their
separate exact-format transport; passing them through float is an explicit
conversion, never an implied fallback.

Each source/output has a separate queue, bound by the host to one exact endpoint
identity and provenance. That binding is created only after policy, privacy,
consent, exclusions and destination-specific mix-minus admission. Numeric
generation/revision values are freshness metadata, not credentials or proof of
authorization. Reconfiguration/restart needs fresh host authority and a new
object. No method silently changes source, destination, clock or format.

`Stamp` carries the exact binding, `clock_id`, sequence, absolute first sample
frame, capture time and deadline. Times use one host monotonic nanosecond domain
shared with `now_ns`. A native source clock remains a source clock; only an
explicit, qualified host mapping/adaptation may assign engine frame positions
and an engine clock ID. Network arrival time, RTT and nominal sample rate do not
establish that mapping. Network jitter and hardware clock adaptation are separate.

`push` copies a complete finite block without conversion; it requires exact
frame/channel count, contiguous frames and sequence, nondecreasing capture times,
valid deadlines and nondecreasing host time. A full queue, malformed block,
stale binding, clock mismatch, expired block or timeline fault permanently
revokes that queue and clears all retained PCM. Failure is visible and local to
that endpoint; the host continues siblings. Host-selected storage bounds also
bound the clearing work; they are not measured callback deadline guarantees.

`pop` first silences the entire caller-owned destination, which must hold at
least `max_frames * channels` scalar samples. It copies each block once, leaves
unused tails zero, and clears consumed storage. Empty means fresh silence and
`Error::Empty`; it does not revoke an otherwise active queue. Expired queued PCM
cannot replay. Invalid authority or insufficient destination storage revokes.
All handoff, empty, failure and revoke paths allocate/free no heap memory, take
no locks and perform no I/O. Construction, destruction and any host replacement
must stay off the callback. The host must serialize revoke with native callback
ownership; this exclusive API does not solve concurrent revocation by itself.

The six `runtime_worker` tests exercise actual production source: ordered
multichannel Float64 low bits/subnormals/signed zero, single consumption and zero
tails, full/expired output isolation, generation/clock/replay/gap rejection,
revoke and invalid PCM, resource admission and callback heap operations. They
do not prove native device delivery, hardware clocks, physical quality or backend
availability. This module does not change `backendAvailable` or `audioApplied`.
