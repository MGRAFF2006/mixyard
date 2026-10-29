# Mobile binary-buffer attachment to the canonical Rust host

Kotlin/JNI and Swift call `adapters/engine/include/mixyard_host.h` directly. The
host owns the only canonical policy/Processor64/DSP/worker graph. These attachment
files do not implement a second mixer, queue or network protocol. Neither mobile
app currently enables an audio role through them. Availability/applied flags stay
false until an actual admitted native route is qualified.

Construction runs on one exclusive worker and compares every observed source and
output ID, rate, encoding, ordered channel ID and mapping against immutable host
port information. Kotlin retains direct binary input/output/stamp buffers; Swift
retains native raw buffers. Each block carries the explicit engine binding and
common clock stamp. Kotlin supports the positive signed-Long subset of u64 stamps;
sample rate retains the full positive u32 domain. Native host failures revoke the
graph and clear copied output. Buffers are worker-owned, never WebView/JSON PCM.

The host's processed mono/stereo and finite Float64 limits remain authoritative.
An integer or F32 ingress requires the host's explicit conversion; arbitrary raw
IEEE payloads, NaNs, wider layouts and encoded streams are **not** preserved by
this processed path. Existing `MobileCore` raw queues remain separate exact-byte
tools; this adapter does not silently funnel them through a narrower format.
Outputs here are explicitly F64LE; a later destination must admit and implement
its own exact conversion. No output role or hardware format fallback is implied.

These are **offline worker** APIs. They allocate at construction and expose no
capture, playback, network, lifecycle, permission or background service. They are
not qualified realtime callbacks or cross-thread SPSC ports. Use explicit close
after all users quiesce; do not retain borrowed Swift buffers past close. Kotlin
checks the constructing thread and keeps direct buffers alive through native use.
The native host processes preallocated buffers without JNI calls on that path.

Before a production audio attachment, its platform owner must bind actual device
observation/generation, independent audio grants, explicit mic consent and full
source provenance/visited peers/exclusions to canonical admission. Management
roles never grant audio authority. The host must map network generation and
`playout.first_sample_frame` into the engine binding and qualified `clock_id/ns`;
network tick sequence/`Instant` is not a source offset or physical capture clock.
No mapping, clock qualification or consent may be inferred from caller numbers.
Background/focus/interruption/route/permission/peer/network changes must revoke
the graph and clear owned outputs before readmission. Those OS paths remain
unimplemented here and cannot be proved by the offline linked test.

Build JNI with the existing JDK/CMake and reviewed host artifacts:

```sh
cmake -S apps/mobile/native -B /owned/build/mobile-jni \
  -DMIXYARD_ENGINE_ROOT=/reviewed/host-checkout \
  -DMIXYARD_ENGINE_LIBRARY=/reviewed/host-checkout/path/to/actual/library
cmake --build /owned/build/mobile-jni
```

The Swift module map imports the same actual host header supplied with `-Xcc -I`.
No copied header, placeholder host implementation or downloaded local SDK belongs
to this subtree. Hosted linkage tests must record both the actual engine revision
and this attachment revision; a source/syntax check alone is not linkage proof.

The actual executable host prerequisite is #96 at
`b71948fe0a5887ecd7226fe8b5d9beae8d24e1bf`, merged without rewriting history.
The dedicated hosted workflow builds that merged engine source and executes both
shipping Kotlin/JNI and Swift workers against it. Its tests use explicit synthetic
common-clock stamps, known binary source levels, canonical mix-minus/independent
DSP expectations, stale bindings and retained-PCM scrubbing. They open no audio
device and create no network/native capture authority.

The small Java source-file probe declares only actual JNI entrypoints; it is not
a second Kotlin worker or policy implementation. A local execution linked this
PR's actual JNI C++ code to the native lead's read-only production library and
passed policy/DSP/output and staged-input revocation checks. This qualifies the
JNI/Rust boundary; the full Kotlin and Swift worker executions belong to hosted
qualification. The local private target peaked at 95,830 bytes and was removed;
the parent library/cache was neither copied nor mutated.
