# Production native control qualification

`control.test.ts` runs the shipping Kotlin `PinnedControl` and Swift `ControlClient`
against isolated **actual mixyard-control** TLS services. The JVM project compiles
byte-for-byte copies of production client/core sources; Swift compiles the production
files directly. Neither driver implements TLS, dispatch, serialization, pinning or
authority logic. JSONL is test-driver control only; PCM never enters it.

The test vaults replace only the Android Keystore/iOS Keychain platform boundary.
They retain provisioned data across client reconstruction and can fail writes.
They do not prove encrypted persistence, device lock behavior or secure hardware.
Android and JVM use the identical shipping `android/http` Gradle module, pinned
OkHttp 4.12.0, TLS 1.3, real JSSE PKI and production pin/hostname checks. Calls have
one-shot bodies, disabled connectivity retry/redirect/authentication follow-up,
no proxy/cookie/cache, explicit total/connect/read/write bounds and bounded raw
UTF-8 request/reply bodies. The strict revoked-401 regression previously failed
because JDK HttpURLConnection discards its error stream in fixed-length streaming
mode. Buffered POST fallback would permit hidden retry; the shipping engine keeps
the real 401 body and rejects replay. Separate negative fixtures count uncertain
POSTs and 503 Retry-After:0 POSTs. Hosted Swift uses
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
The Settings regression commits a valid write, then submits its old settings
revision inside a fresh outer control envelope. The original scoped HTTP 409
`store-conflict` reason must survive unchanged and native authority must clear;
another read requires explicit session admission. A viewer's Settings write gets
403 while its authority and authorized Settings read remain intact. The hosted
Kotlin job first runs these strict assertions against the previous shipping
client `7da358ea416b4fae5ac011599014693e04648f36` and requires exactly this scoped
revocation failure, then rebuilds the current shipping source and runs the same
suite. No expected-authority assertion is relaxed for the old implementation.
The TLS redirect server is a negative test fixture only; successful scenarios use
the production service. All endpoints are owned loopback. No microphone, speaker,
human browser, network audio role, signing or deployment is enabled.

These are hosted native service tests, not physical Android/iPhone secure storage,
WebView, route/focus, background, capture, playback, battery or audio qualification.
Both availability flags remain false. The portable host ABI attachment is a
separate native outcome and cannot promote any role based on these tests.
