# macOS native Rust-host attachment

This new library attaches the actual canonical `mixyard-host` ABI to the existing
`CaptureIngress` / SPSC `Queue` and output-only AUHAL `Worker`. It owns no second
mixer or routing model. The frozen worker/output-only files remain prerequisites.

The control owner opens `Capture` with an observed Core Audio process object/PID,
explicit consent to that process's stereo tap, and producer exclusions. Admission
rechecks the observed process list/PID, creates one private unmuted process tap and
private tap-only aggregate, then reads its actual UID and exact Float32/64 ASBD.
The source operation is explicitly a stereo process tap; it does not claim raw
multichannel application PCM. The callback changes storage layout only. Native
permissions remain macOS's responsibility; consent does not assert a TCC grant.

Construct the canonical host request **after** observing native source and sink
identities/formats. Source/output arrays follow compiled plan order. `HostLink`
compares ABI v1's fixed compact immutable port JSON with actual queue IDs, encoding, rate, ordered labels
and explicit canonical mapping. Processing supports canonical mono/stereo only;
the tap source currently admits stereo. It rejects wider processed paths and
different rates. There is no raw bypass route in this attachment.

Each output has its own `AdaptedEngine` SPSC and output-only `Worker::open(actualUid,
queue)`. Native ordered labels are `CA:1`, `CA:2`; the explicit engine mapping is
`FL`, `FR`. Float64 stays Float64 except at an explicitly Float32 destination.
Float32 overflow fails that sink; it does not clip or change sibling outputs.
All constructors and immutable JSON metadata comparisons allocate on the control owner. Native capture
and output callbacks allocate nothing, acquire no locks and perform no I/O. The
exclusive engine owner borrows source blocks through `mx_host_process`, copies
borrowed Float64 output to each consumer's bounded queue and retains no host PCM.

`step` requires source block lengths/frame positions to match the engine period.
Any selected native source that is missing, expired, malformed, revoked or on the
wrong timeline returns typed `StepResult::SourceLoss` (`source_loss`). The exclusive
engine owner immediately revokes **all** source/output queues and calls
`mx_host_revoke`. This conservatively stops the whole generation, including healthy
call-return and unrelated outputs. It rejects late source publication and future
native render reads of retained queued or partially consumed PCM. There is no
healthy-source continuation or implicit source resume. Fresh scheduled silence is
valid timestamped zero PCM; a missing native block is not ordinary scheduled
silence or network packet loss.

Destination-only DSP failure, Float32 conversion overflow, queue overflow, removal
or close still revokes only that destination. No source loss is inferred from an
inactive output. Contributor/epoch-aware per-output continuation is separately
tracked in [#104](https://github.com/MGRAFF2006/mixyard/issues/104), under #51 and
prerequisite #103; it is not implemented here. It must qualify canonical contributor
unions, native epochs and affected queue/cursor/DSP retirement while preserving
unaffected outputs, without another policy graph.

`source_loss` and atomic queue revocation are notifications, **not teardown
completion**. The control owner must stop its engine scheduler and finish the
exclusive graph owner before calling `finishGeneration`. The helper destroys the
revoked Rust host, closes the actual owned `Capture` and `NativeOutput` workers,
and returns `Pending` if any native stop/dispose/listener close failed. It retains
all transport storage for retry. `NativeOutput` wraps the existing output-only
worker and retires its private renderer context only after successful native close.
Once every consumer/capture has closed, the helper retires all contexts and scrubs
private inline SPSC storage before freeing it; only then does it return `Complete`.
Any external producers/readers of additional source queues must already be
quiescent. No live queue/cursor is cleared, no callback acquires an owner lock,
and no storage is freed while a native close remains pending.

Consent/authority revocation must first call `HostLink::cancel` and revoke/close
native workers; do not wait for the next source-loss observation to remove
permission. Already submitted hardware buffers have a separate commit boundary;
this API does not claim to retract them. Restart requires completed quiescent
teardown, fresh queues/host, a fresh generation and native admission. Dropping an
object with live native ownership deliberately terminates rather than freeing its
callback storage.
**Physical tap and AUHAL sample timelines remain unqualified.** `Timeline::physical`
always rejects before native acquisition. Equal nominal rates, mach host time,
and RTT do not prove a common sample clock. Production has no injected-clock
factory or mutable native API override. No runtime flags are promoted:
`backendAvailable=false`, `audioApplied=false`.

`check.sh SHARED_HOST_CHECKOUT SHARED_HOST_LIBRARY` runs only on isolated hosted
Apple SDK runners. It force-links production Rust/Objective-C++/SDK symbols without
executing that binary, then runs the same source/host/output code with strict SDK
doubles and one explicitly injected timestamp/sample timeline. The fixture remains
`evidence=simulated`; its source/sink IDs are injected observations and prove no
physical device or application identity. The PCM oracle checks canonical private
source exclusion, call-return routing, destination-only DSP, Float32/64 source
precision, explicit sink conversion, independent consumer close, terminal source
loss, queued/partial nonzero DSP PCM discard, pending native-stop retries, final
queue scrubbing and source SDK failure cleanup. Output callback timestamps use an
explicit future 100 ms mach-time schedule within the admitted PCM deadline. No AudioUnit, tap, device, TCC,
personal microphone/speaker, default endpoint or service is activated by tests.

The production portable `HostLink` and actual static Rust library pass 2048
steady-state SPSC/mixer/DSP/consumer blocks on Linux, plus empty/revoked-source
regressions with nonzero two-frame output delay PCM held wholly queued and half
consumed in the production native `Renderer`. Both cases reject retained PCM after
typed whole-generation loss, and instrumentation observes the graph owner's actual
`mx_host_revoke` call. Additional tests cover destination-only DSP overflow,
identity/order mismatch, stale/cancel, fresh valid zero PCM and fresh-generation
restart after teardown. All processing/loss/failure paths make zero
malloc/calloc/realloc/free calls. Quiescent queue destruction verifies the private
storage is zero before deallocation. `portable-check.sh READ_ONLY_HOST_STATIC_LIBRARY`
reproduces these checks without building Rust or invoking Apple APIs. The generated
artifact peak is 2420 KiB and artifacts are removed; the parent's pinned library
remains read-only. The queued-DSP source-loss regression was red before this repair
and passes after it. These are transport/engine results, not native SDK evidence.
Apple SDK compilation and execution remain pending the hosted check. Physical capture/output,
permissions, common-clock adaptation, fidelity/latency and native app identities
remain unqualified even after the injected test passes.

The separate `macos-native-host.yml` workflow pins shared-host dependency
`b71948fe0a5887ecd7226fe8b5d9beae8d24e1bf`; manual dispatch can select another exact
commit. Automatic PR runs use the exact attachment head. It checks the host out
separately and never overlays files on
the frozen base. Its run records both full revisions and retains the safe test
log. The host is built only on the disposable hosted runner, with one job, no
incremental compilation and no debug symbols; both host and adapter artifacts are
removed after the run. Adapter-generated SDK artifacts are limited to 16 MiB.
