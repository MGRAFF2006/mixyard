# Explicit loopback runtime handoff

`interoperability/mixyard_interop/runtime.py` supplies an off-callback lease
boundary for the HTTP and RTP workers. It does not capture devices, own a mixer
or queue, authorize OS access, or provide paired-device authentication.

An already authorized owner supplies an exact `LeaseBinding`: observed identity,
canonical Spec (including complete provenance), generation, revision, source
epoch and common-clock ID. `RuntimeLease` requires explicit ephemeral consent,
an observation callback returning that same current binding **including the full
ClockEvidence anchor and qualification**, a finite lifetime,
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
- `lease.source_lost()` is the actual owner's explicit source-loss signal. It
  terminally rejects pending/partial media; source restoration needs a fresh
  generation/epoch/host admission. `check_liveness()` permits a final pure
  revoke/expiry check after all external owner callbacks have completed.

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

## Optional actual C ABI worker

`native.NativeHost` consumes the optional ABI1 shared CPU engine, plus the
additive `mx_host_output_attestation_v1` and `mx_host_output_packet_v1` APIs.
An absent query/packet symbol makes processed egress unavailable; ordinary
`mx_host_output` is never used as a fallback. `HostLoan` requires an explicit
absolute library path, artifact SHA256, exact source revision, toolchain and
current owner loan callback. The borrower does not build, copy, modify or search
for a library. One serialized worker owns every context; Python is off callback.

The immutable query is complete **compiled-plan metadata**, not capture consent
or proof of a native identity/clock. The worker independently checks current
preauthorized source/output leases, exact observed format and channel mapping,
full common-clock anchors, generation/revision/epochs and whole-premix groups.
The output contributor set comes only from that output's actual compiled paths:
the fixture's Call contains shared; Record contains return and shared. Private
and canonically excluded paths are never reintroduced. Reachable muted/zero-gain
sources stay conservative for DSP tails. Missing/unknown/partial/oversize origins
reject the whole affected output; config labels alone are insufficient evidence.

Every processed packet must match the exact query-byte digest, context/output,
request-wire digest and full engine stamp. The graph owner copies bounded
Float64 output before the next process/revoke/destroy and rechecks all live
leases before returning it. External revoke signals the serialized owner;
it never calls C revoke concurrently. `host.source_lost(index)` revokes the
whole bridge generation, including Call when return is lost: this wrapper does
not claim per-output source-loss continuation. Destination-only loss disables
that destination while healthy siblings continue. No missing input is padded
and labelled captured PCM.

Copied processed blocks retain a local, revocation-only producer receipt.
`host.revoke()` terminally signals it and every output lease of that bridge
generation before waiting for native cleanup. `RuntimeLease.validate` checks
the receipt after external owner callbacks; HTTP rechecks pending records during
each write and RTP validates at send admission. Explicit destination conversions
preserve the same receipt, including copies held by a separately authorized
destination. Unrelated host generations/raw leases remain independent. The
receipt contains no native pointer/loan, is never sent as wire authority, and
does not replace owner, provenance, clock, consent or directional grant checks.
Native processing also retains inherited input stop signals for this generation
and its DSP history, with a 1024-signal limit that fails closed. Per-output
contributor metadata and canonical exclusions remain unchanged.
Previously admitted OS IO cannot be recalled; an incomplete HTTP record is not
released as a complete PCM block.

A loss notification is not stop completion. `host.quiesce()` waits for the
exclusive Python worker, invokes C revoke as that sole graph owner, and verifies
every packet is inactive and zeroed before returning a `StopWitness` tied to
the exact context/request/generation. `close()` requires that witness before
destruction. A busy/failed stop retains the context and loan ownership, exposes
no completion witness, and permits no release, automatic rebind or resume. This
witness covers the owned CPU context only; actual native endpoint workers need
their own independent stop/scrub completion evidence.

S16/S24/S32 inputs widen by exact normalized Float64 conversion; finite F32
widens, and finite F64 is processed. This is not an opaque raw/nonfinite bypass.
Processed output remains Float64. `convert_processed` requires separate explicit
destination consent, an exact unchanged rate/layout/provenance and the named
nearest-ties-even quantization to S16/S24. It rejects range/nonfinite values
instead of clipping; sibling Float64 output is unchanged. Its returned conversion
evidence must remain distinct from raw byte-preservation evidence.

Actual optional qualification uses an explicit pinned read-only loan:

```sh
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=interoperability \
  MIXYARD_NATIVE_HOST=/absolute/owner/library.so \
  MIXYARD_NATIVE_SHA256=<owner-artifact-sha256> \
  MIXYARD_NATIVE_REVISION=<full-owner-source-sha> \
  MIXYARD_NATIVE_TOOLCHAIN='<owner-toolchain-and-build-provenance>' \
  python3 -m unittest discover -s interoperability/tests -p test_native.py -v
```

Without those values, actual host tests explicitly skip. The fixture is read
from the exact tracked source revision with `git show`, not a moving checkout.
The local 2026-10-29 author proof used PR103 source
`1a8ff190474d92237369a81d5b08cda63b7b3b9b`, library SHA256
`32a2f1910a9fe03c98387f474e5306dc0bd81666a218bcc85e1de65efa6f323a`,
rustc 1.99.0 (`b940084d7eb6a299eb4bfeb8e34901bc051e7ac4`), LLVM 23.1.1,
x86_64 Linux. The owner directly compiled the changed cdylib against the frozen
canonical root library SHA256
`c53e28eebfb128785c1a5b200590b1174a0c428fd9b6276ec244a841b586cfe0`;
this is not a fresh full-Cargo build claim. Independent native-query and combined
integration review are separate gates.

Actual author tests prove both bounded cross-adapter flows: owned leased RTP
S24 source → actual compiled host/DSP → leased HTTP Float64 destination, and
leased HTTP S24 source → actual host → explicitly consented S24 quantization →
leased RTP destination. Additional tests cover five input encodings, Call/Record
contributor and PCM oracles, missing contributors, full clock-anchor mismatch,
revoke/reentry/source loss/restart, nonfinite rejection and destination isolation.
They use declared synthetic common clocks and owned in-memory/null consumers;
they do not prove physical capture/render, hardware clocks, native endpoint
transport, continuous gap-free audio, endurance, production pairing, SRTP,
Snapcast PCM/group sync, full DLNA or vendor receiver support.
