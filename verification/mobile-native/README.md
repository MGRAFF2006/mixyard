# Production native control qualification

`control.test.ts` runs the shipping Kotlin `PinnedControl` and Swift `ControlClient`
against isolated **actual mixyard-control** TLS services. The JVM project compiles
byte-for-byte copies of production client/core sources; Swift compiles the production
files directly. Neither driver implements TLS, dispatch, serialization, pinning or
authority logic. JSONL is test-driver control only; PCM never enters it.

The test vaults replace only the Android Keystore/iOS Keychain platform boundary.
They retain provisioned data across client reconstruction and can fail writes.
They do not prove encrypted persistence, device lock behavior or secure hardware.
JVM uses real JSSE PKI plus the production pin/hostname checks; hosted Swift uses
real URLSession/Security with temporary fixture trust in a disposable CI keychain.
`org.json` on JVM is the upstream JVM implementation, not Android's platform JSON
runtime. The native raw-string canonical payload path must not depend on either
implementation's floating-point reserialization.

The dedicated workflow builds the control service from exact production source
`812eee1479702635dc51994dc795ca9f8a655351` (PR #76) in a separate hosted checkout.
This is an explicit stack dependency, not an unversioned download. The client
revision is the PR checkout under test. An SDK build or queued job is not a pass.
Do not upload fixture directories: they contain private disposable TLS identities
and one-use invitations. Only test reports are suitable evidence artifacts.

Scenarios cover OOB pairing and invitation replay, frozen queued targets, canonical
stored writes, nested signed-zero/low-bit/subnormal Float64 offline DSP, client
reconstruction from the boundary vault and real server restart, role 403
preserving authority, revoked 401 clearing authority,
certificate/hostname mismatch, rejected redirect and failed vault writes/forget.
The TLS redirect server is a negative test fixture only; successful scenarios use
the production service. All endpoints are owned loopback. No microphone, speaker,
human browser, network audio role, signing or deployment is enabled.

These are hosted native service tests, not physical Android/iPhone secure storage,
WebView, route/focus, background, capture, playback, battery or audio qualification.
Both availability flags remain false. The portable host ABI attachment is a
separate native outcome and cannot promote any role based on these tests.
