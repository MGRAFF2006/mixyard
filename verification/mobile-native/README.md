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
`b3731ce51af02cfbcd0ad6fb7080694f08b6a0bd` (PR #76) in a separate hosted checkout.
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

## Readiness and failure evidence

The previous Swift run compiled but its first pairing RPC timed out after20s;
the subsequent cleanup remained blocked until job cancellation. That is a failed
suite, not evidence of a harmless preemption or successful TLS. Its precise cause
has not yet been established. The driver now announces readiness before stdin is
admitted. Trust preparation and the first RPC share the original20s total budget;
HTTP bounds, Security trust, hostname verification and leaf pinning are unchanged.
Only a small fixed phase vocabulary is exposed; arbitrary native stderr is drained
without logging paths, credentials, certificates or captured Security output.

The hosted Swift wrapper bounds each owned Security command and its total startup
and cleanup work. On failure it restores and verifies the original keychain search
list before removing only its owned fixture trust and keychain. Cleanup failures
are reported. Actor shutdown and independent service shutdown are both attempted,
so a trust-cleanup error cannot skip service reaping. Node actor lifecycle tests
use real owned child processes; Python trust-lifecycle tests replace Security and
never change local PKI. These tests do not qualify actual macOS cleanup or explain
the earlier failure; current-head hosted phase/readiness/TLS execution is required.

Actual94c738 [Swift92 failure](https://github.com/MGRAFF2006/mixyard/actions/runs/40985908895/job/123836760973)
and identical-fixture daf1dbc [Swift93 failure](https://github.com/MGRAFF2006/mixyard/actions/runs/40985960350/job/123836925627)
now isolate trust-a before child startup: setup/search completed, trust failed
around its4s limit, restore/readback completed, and owned trust removal failed.
Those observations did not distinguish a timeout from nonzero exit. New fixed
reason records distinguish timeout milliseconds, process exit/signal, errno and
exhausted budget; no raw captured output/exception arguments are exported. Typed
reason parsing rejects unknown phases/kinds/extra fields. No provisioning change
or deadline increase is made before actual typed evidence identifies the failure.

Apple's [tool implementation](https://github.com/apple-oss-distributions/Security/blob/main/SecurityTool/macOS/trusted_cert_add.c)
adds the certificate to the requested keychain and separately sets trust settings
in the user domain by default. The -k option does not scope trust settings to that
keychain. Apple's [trust implementation](https://github.com/apple-oss-distributions/Security/blob/main/OSX/libsecurity_keychain/lib/SecTrustSettings.cpp)
documents that setting trust may block for authorization. This establishes tooling
semantics, NOT the cause of the observed runner failure. User/admin trust policies,
normal PKI and exact production pinning remain unchanged pending the new evidence.
