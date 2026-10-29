# Explicit-UID output-only AUHAL worker

This separate C++ library replaces neither the gated legacy `OutputWorker` nor
its probe. It implements a real AUHAL output path, but is not linked to Rust or
exposed by the product. `backendAvailable` and `audioApplied` remain false.

`Worker::open(uid, adaptedQueue)` binds one explicitly chosen device UID and
one output stream. The queue must already carry destination-clock PCM with
exact rate, Float32/Float64 precision and ordered positional labels `CA:1..N`.
These positions do not claim a speaker layout. No default fallback, device rate
or format mutation, semantic channel remapping, numeric conversion or resampling
is performed. Physical stream, virtual stream, AUHAL device and client formats
must all match; storage interleaving may differ. Integer physical sinks, multiple
streams, mismatched formats and nonidentity mappings reject, with explicit
conversion left to a future host adapter.

The private Apple HAL output unit is created without starting it. Input I/O is
explicitly disabled using `EnableIO`, input scope, bus 1, and read back immediately.
Output bus 0 is enabled, the selected device bound, client format and an explicit
identity map set. Input-disable, output-enable, device, both AU formats, map and
callback bound are read back **before initialize and again before start**. The
unit is private; this library has no path that enables input, registers an input
callback or calls `AudioUnitRender` to obtain input. Duplex devices are therefore
not excluded solely because they expose input streams.

`start()`, `revoke()` and `close()` synchronously execute on one private serial
libdispatch control queue. Hardware property-listener blocks execute on that
same queue. A revoke ordered before start prevents native start; a revoke ordered
during an already entered start stops it afterward. Native start/stop never run
concurrently. Device inventory, UID, alive, nominal rate, output/input topology,
physical/virtual format and starting-channel notifications terminally revoke the
queue and stop the owned unit. They never reconfigure or re-enable it. Admission
and start revalidate after configuration/start as well. A notification can arrive
after a hardware change; this design does not promise zero notification latency.
The disabled input bus does not depend on timely topology notification.

Lifecycle revocation must use `Worker::revoke()`: calling `Queue::revoke()` directly
only invalidates PCM and does not serialize native lifetime. Queue producer faults
also silence subsequent callbacks; the host must revoke/close the worker to stop
native output. Stop failure leaves stop ownership intact. Failed stop, uninitialize,
dispose or listener removal returns false from close, retaining storage for retry.
Destruction terminates on failed teardown rather than freeing potentially live
callback storage. An already queued listener retains an independent state object
and becomes harmless after successful removal/close.

Keep the queue and worker alive until close succeeds. Concurrent control calls
are serialized, but destruction and queue lifetime remain the caller's exclusive
ownership responsibility. Do not call control operations from callbacks. One
producer/one render consumer owns each queue; use a fresh queue and worker for
any new binding, revision, generation or restart.

The actual render pull callback has no input-data parameter. It uses the public
`Renderer`, `Queue`, `exactFloat`, `samplePosition` and `HostClock` helpers. There
are no callback allocations, locks, logging, property queries, dispatch calls or
file/device operations. At most 4096 frames, 32 channels and 65536 PCM bytes are
admitted per slice, further reduced by frame width. Valid outputs are cleared
before validation; queue faults, underflow, expired PCM, invalid timestamps or
wrong positions produce silence for the entire slice, including a copied prefix.
Malformed/unbounded native buffers return an AudioUnit error plus the silence flag;
this does not claim safely zeroing arbitrarily large or invalid native memory.

The host must schedule/prebuffer using the selected destination timeline. Starting
AUHAL does not adapt clocks or manufacture a first device frame. The callback
requires a valid exact sample position and host timestamp, a future/current host
presentation time and unexpired queued blocks. Missing that contract fails silent;
there is no retry, delayed replay, guessed timestamp, start alignment or implicit
clock conversion. Native scheduling, physical latency and stable clocks remain
unqualified.

## Public Apple API basis

Investigated 2026-10-29 against primary Apple documentation:

- [TN2091: AUHAL](https://developer.apple.com/library/archive/technotes/tn2091/_index.html)
  documents the separate input-enable control on bus 1, device selection,
  flattened AU formats and 1:1 channel maps. This implementation explicitly
  sets input enable to zero and rejects mismatched formats.
- [EnableIO](https://developer.apple.com/documentation/audiotoolbox/kaudiooutputunitproperty_enableio)
  is the native I/O control; output callbacks alone do not prove disabled input.
- [Core Audio common tasks](https://developer.apple.com/library/archive/documentation/MusicAudio/Conceptual/CoreAudioOverview/ARoadmaptoCommonTasks/ARoadmaptoCommonTasks.html)
  distinguishes AUHAL's selected-device binding from default output behavior.
- [AudioObjectAddPropertyListenerBlock](https://developer.apple.com/documentation/coreaudio/audioobjectaddpropertylistenerblock(_:_:_:_:))
  supplies the dispatch-queue listener boundary. The implementation never uses
  an arbitrary HAL listener thread to race native start/stop.

## Verification and remaining proof

Run `bash adapters/macos/output-only/check.sh` only on an isolated hosted Apple
runner. It builds a static production library and links a caller against the
actual Apple SDK/frameworks; that production caller is **never executed**. It
then recompiles the same source with a fixed API table substituting every native
AudioUnit/HAL operation. The production build has no override mechanism.

Strict doubles check property scopes/buses/values, absence of capture/default
mutation, input proof before initialize/start, exact formats/map, ownership and
teardown. They inject failure at every API operation in a successful lifecycle,
exercise rejected admission/readback contracts, topology/start ordering,
concurrent start/revoke, delayed listener lifetime and the actual render callback.
Callback tests use an allocation guard, bit-exact Float32/Float64 signed-zero,
subnormal and Float64 low-bit data, interleaved/planar storage, partial blocks,
underflow, prefix clearing, expiry, timeline mismatch and revoked queues.
ASan/UBSan are enabled. Apple leak detection is disabled under the existing
hosted sanitizer constraint. Private temporary artifacts are capped at 20 MiB
and removed; no build cache or SDK is installed locally.

SDK compilation/linking proves ABI and symbol compatibility. API doubles prove
application behavior under modeled native outcomes. Neither proves actual AUHAL
input exclusion, callback scheduling/teardown, device topology/format behavior,
physical PCM/channel integrity, output audibility, TCC behavior, fidelity or latency.
No native device/AudioUnit is activated during tests; no physical input/output,
permission requests, GUI, default/service/OS mutations or Rust integration occur.
These physical and integration gaps must be qualified separately before enabling
product audio availability.
