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
All constructors/info JSON parsing allocate on the control owner. Native capture
and output callbacks allocate nothing, acquire no locks and perform no I/O. The
exclusive engine owner borrows source blocks through `mx_host_process`, copies
borrowed Float64 output to each consumer's bounded queue and retains no host PCM.

`step` currently requires source block lengths/frame positions to match the
engine period. A missing, expired or malformed source becomes permanent silence
for that generation. A source revoked during processing cancels the entire
in-flight generation before outputs publish. A stopped/overflowing/removed sink
revokes only its queue. `cancel` atomically revokes all transport queues; Rust host
revoke/destroy remain with the exclusive quiescent owner. Consent/authority
revocation must first cancel `HostLink` and revoke the associated output workers,
then stop capture; stopping a source alone is source loss and cannot revoke
already queued derived PCM. Close source/sink workers
and quiesce the engine before destroying queue/host storage. A failed native stop,
destroy or listener removal retains ownership and requires `close` retry; dropping
an object with live native ownership deliberately terminates rather than freeing
callback storage. Restart requires fresh queues/host admission and generation.

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
precision, explicit sink conversion, independent consumer close, malformed source
silence, cancel and source SDK failure cleanup. No AudioUnit, tap, device, TCC,
personal microphone/speaker, default endpoint or service is activated by tests.

The production portable `HostLink` and actual static Rust library pass 2048
SPSC/mixer/DSP/consumer blocks on Linux, including Float32/64 sources and sinks,
stalled-consumer isolation, source-loss silence, identity/order mismatch and
stale/cancel paths with zero malloc/calloc/realloc/free operations during processing.
`portable-check.sh READ_ONLY_HOST_STATIC_LIBRARY` reproduces this check without
building Rust or invoking Apple APIs. Measured generated artifacts peaked at
2324 KiB and were removed; the parent's library remained read-only. This is
transport/engine evidence only.

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
