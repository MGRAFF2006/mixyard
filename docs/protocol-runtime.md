# Explicit loopback runtime handoff

`interoperability/mixyard_interop/runtime.py` supplies an off-callback lease
boundary for the HTTP and RTP workers. It does not capture devices, own a mixer
or queue, authorize OS access, or provide paired-device authentication.

An already authorized owner supplies an exact `LeaseBinding`: observed identity,
canonical Spec (including complete provenance), generation, revision, source
epoch and common-clock ID. `RuntimeLease` requires explicit ephemeral consent,
an observation callback returning that same current binding, a finite lifetime,
and `ClockEvidence` with an explicit source frame/time origin. Checking local
monotonic time for expiry or pacing does not create a hardware timestamp or
establish clock equivalence. Synthetic evidence stays labelled synthetic.

`LeasedBlock(spec, pcm, generation, revision, source_epoch, base_frame, clock_id)`
contains immutable complete PCM, at most the admitted frames-per-packet and
1 MiB. Partial frames and padding are rejected. Opaque raw paths preserve bytes,
including nonfinite floating-point payloads; those bytes are not automatically
admissible to a finite processed Float64 engine path.

- `lease.check()` rechecks lifecycle and current owner binding during IO/pacing.
- `lease.validate(block)` is an idempotent exact binding/format check.
- `lease.read(source)` checks before and after the synchronous source callback.
- `lease.accept(block, consumer)` enforces contiguous frames, rejects a busy
  consumer without queuing, and advances only after a successful callback. A
  callback failure revokes the lease because a prefix may already be consumed.
- `lease.revoke()` prevents future admission. It cannot recall IO already handed
  to an OS. Workers retain explicit socket/cancellation/cleanup ownership.

An owner restart, source loss, changed format/revision or epoch requires fresh
admission. Missing/unknown samples are not reported as captured silent PCM.
Source exclusion rejects the entire premix; labels cannot prove that a premix
has complete provenance.

## Management integration

`control.py` is a local boundary for a management service that has already
verified `ManagementScope(device_id, authority_id, revision, permissions)`.
Its only management rights are `view`, `control`, and `settings`. This class is
not a credential verifier or remote service.

The registry snapshot uses `deviceId`, `authorityId`, and `revision`, with
per-role capabilities and explicitly untrusted inventory suggestions. Hostnames,
discovered URLs and vendor names never become grants. This loopback profile
accepts only the canonical literal-loopback Endpoint. A separate future inventory
profile may display broader addresses without making them eligible for IO.

`select` / `configure` store a complete revisioned selection without starting IO.
`configure` can select both media directions atomically; a replacement invalidates
old pending and active handoffs. `attach` is a separate runtime-owner action
requiring both a current RuntimeLease and independently granted Session media
role/endpoint/exact Spec. `activation.invoke(worker)` is the explicit worker
start. Neither settings permission, discovery, service-control nor advertisement
creates media rights. Unintegrated vendor roles remain unavailable. Callers must
wire activation checks into bounded worker cancellation and cleanup.

All native/backend/physical qualification flags remain false. This boundary
does not connect the browser or management server automatically. Socket proofs
qualify only the recorded owned synthetic loopback path, not production pairing,
hardware timestamps, physical audio, quality/endurance, or broad ecosystem support.
