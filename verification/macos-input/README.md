# macOS input publication artifact

This original MIT artifact adds an input-only `MixyardInput.driver`, a real Apple
ABI in-process harness, and a separate explicit-UID HAL consumer source. Production
apply remains disabled. The release bundle publishes one fixed 48 kHz stereo
interleaved Float32 input and returns initialized zeros. It has no injection IPC
or fixture exports. A successful build, ad-hoc signature, or in-process harness
does **not** prove HAL loading, producer authentication, external PCM, microphone
access, privacy, latency, or the product workflow.

On macOS with Apple developer tools, from the repository root:

```sh
bash verification/macos-input/build-check.sh
```

The recipe uses the selected `xcrun --sdk macosx clang++`, records OS/architecture,
SDK/compiler and exact git HEAD, checks the exact 10,737,418,240-byte free reserve
before/during/after work, and compiles serially (within jobs=2). It compiles the
release bundle, an ASan/UBSan harness using the actual SDK ABI, and the consumer;
ad-hoc signs/verifies the bundle and consumer, checks the single release export,
and runs only the harness. Nothing is installed into HAL; the consumer is never
run by CI. Native compilation is explicitly unavailable on Linux (exit 2), with
no replacement HAL stub. The workflow uploads artifacts, toolchain, checks and
SHA-256 manifest under `verification/macos-input/build/` (git-ignored).

The harness loads the exact signed bundle with `dlopen` into its ordinary process
and tests release ABI/silence. A separately compiled `MIXYARD_INPUT_TESTING` source
instance then exercises owned registry/UID translation/notifications, stereo
format and property errors, clients, first/tail samples, exact half gain/fanout,
independent read positions, packet generation/clock/revision/length/nonfinite
rejection, delivery-path revision evidence, rename, retirement approval/abort,
late callbacks and reopen with a new epoch. Fixture data is **in-process**, not
authenticated producer or HAL consumer evidence. Evidence reports the most
recent read, not a producer admission acknowledgment. Errors/zero-fill do not
establish privacy against an actual unrelated producer.

This bounded artifact has two lifetime endpoint slots, 16 lifetime client slots
per endpoint, 4,096 frames per packet/callback and 16,384 retained frames. IDs are
never recycled; exceeding a bound fails explicitly. A stopped client consumes a
lifetime slot, so reopen eventually fails instead of unsafe reuse. One endpoint
A is initialized; B is dynamically published only through fixture controls.
Public `CreateDevice`/`DestroyDevice` remain unsupported. Rename keeps the UUID;
retirement revokes admission immediately, asks the host for a configuration
change, and leaves a tombstone for late stop/remove/read callbacks. Abort keeps
the published device silent until fresh admission. No persisted production
registry or unlimited endpoint capability is claimed. A stalled reader cannot
drain another; overwritten/missing positions return zeros. The fixture buffer
does not implement the future credited IPC broker.

Post-change host notification errors do not roll back committed state.
`StartIO`, `StopIO`, `RemoveDeviceClient` and performed retirement return success
once their state transition commits; repeating the operation is not a recovery
method. Failed notices and their most recent `OSStatus` are retained outside RT.
Later lifecycle transitions retry a single bounded snapshot of pending notices.
Fixture publication/rename return `MY_INPUT_COMMITTED_NOTIFY_PENDING` when they
commit with notifications outstanding: the returned device ID, UID, names and
consumed lifetime slot remain valid. Do not republish to recover. The fixture-only
`MixyardInputTest_ReconcileNotifications` retries only outstanding host calls,
reports their remaining count and retains the most recent error after recovery.
One pass makes at most nine calls; the plugin device/owned-object lists share one
call. Successful halves of device/stream rename notifications are not retried.
Reentrant changes remain queued for a later pass, with no lock held across host
callbacks. There is no background retry loop, production reconciliation control
or restart persistence. The native harness includes failed start/stop/remove,
fresh epochs, failed publication, each/both rename notices and failed retirement
notices; local Linux source checks cannot establish that these assertions pass.

All control, allocation, property access and notifications happen outside
`DoIOOperation` and `GetZeroTimeStamp`; those callbacks only use bounded scans,
lock-free atomics, host time and copies. No hardware-clock drift/adaptation proof
is provided. In-process data tests use one synthetic clock domain.

## Blocked native recipe (requires separate authorization)

`BLOCKED.json` records concrete unmet gates. The present overnight authorization
allows artifact compile/sign/in-process tests only, including on hosted CI.

1. Obtain an owned disposable macOS guest and explicit authorization for HAL
   installation, helper installation, permission interaction and audio-service
   restart/reboot. Capture exact source/artifact hashes, OS, SDK and signatures.
2. Implement and review a private broker in the **system bootstrap namespace**,
   binding approved producer authority to an executed per-build code requirement
   and fresh connection/generation. UID/PID/bundle name alone cannot qualify.
   Declare only that broker in `AudioServerPlugIn_MachServices`; validate its
   sandbox contract and credit/packet bounds outside callbacks. The current
   release plist declares no Mach service because none is implemented.
3. After the new source passes its own independent reviews, install only its
   exact signed bundle into the disposable guest's `/Library/Audio/Plug-Ins/HAL`.
   Apple's sample specifies reboot. Ad-hoc HAL acceptance and reload equivalence
   are unresolved: record failure as BLOCKED, never infer acceptance from
   `codesign`/`dlopen`. Reboot/reload is not authorized by this recipe.
4. Enumerate exact UIDs and stream negotiation. Launch the separate consumer
   before admitting fixture PCM, with supported microphone authorization in an
   interactive disposable guest if needed. The embedded usage description is
   a prerequisite, not evidence of TCC permission or a substitute for it.
   No raw/default microphone fallback or TCC bypass is permitted.

   ```sh
   verification/macos-input/build/mixyard-input-consumer \
     org.mixyard.input.c963dc90-9a0d-4c6c-8934-d8d5af579260 \
     480000 A.wav A.json
   ```

   It exclusively creates WAV/metadata files and keeps whole first/final
   callbacks (received frames can exceed the requested target). Timeout, bad
   timestamps, buffer shape or nonfinite PCM fails and retains received prefix.
   `qualified:false` always remains until an independent acceptance analysis.
5. Inject distinguishable complete L/R sequences with exact unity/half gain,
   first/final markers, startup silence and a silence tail beyond retained
   capacity. Repeat A/B independence, compute-once fanout, slow reader, rename,
   reopen/retire, producer crash/restart, malformed/stale packets and unrelated
   producer/sentinel attacks. Compare **every** consumer sample and actual frame
   timestamps with delivery-path revision/frame evidence. Never trim failures,
   fit gain/alignment, reconstruct gaps, or count absent sentinel reproduction
   as privacy PASS. Repeat after authorized server restart/reconstruction.
6. Real microphone + selected app -> separate call/record/headphone consumers,
   sustained quality and physical independent clocks remain additional gates.

## Primary contracts checked

- [Apple Audio Server plug-in sample](https://developer.apple.com/documentation/coreaudio/creating-an-audio-server-driver-plug-in):
  input format/driver bundle publication and documented install/reboot boundary.
- [Apple configuration-change contract](https://developer.apple.com/documentation/coreaudio/audioserverpluginhostinterface/requestdeviceconfigurationchange):
  host approval may be deferred or aborted; I/O/structure changes require approval.
- [Apple QA1811](https://developer.apple.com/library/archive/qa/qa1811/_index.html):
  plug-in must not call client HAL APIs; sandbox Mach service declarations and
  supported host storage. The consumer is a separate executable.
- [Apple code requirements](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements):
  future exact code identity is an unresolved native execution requirement.
- [Apple command-line tools](https://developer.apple.com/library/archive/technotes/tn2339/_index.html):
  `xcrun` selects SDK developer tools; the build records the actual selected version.

The source was independently written against Apple's interface contract. No
Apple sample implementation or third-party driver code is vendored or relicensed.
