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

## Disposable admin fixture lane

The actual2b/a306 Swift runs classified the first user-domain add as timeout4000;
they did not establish an OS/UI cause and never started the Swift client. Root
authorized a separate explicit disposable-hosted admin fixture mechanism. Fresh
owned certificate DER snapshots are SHA256-bound. A read-only Security helper
checks their ADMIN trust records are absent BEFORE any add; preexisting or unknown
state fails without modifying/removing it. Certificates are stored by the owner
in its private keychain; only exact new ADMIN SSL trust records use
sudo -n /usr/bin/security. There is no user-domain fallback, interactive sudo,
SIP/authdb/policy change, ignored validation error or production verifier change.

Search-list restoration and verification occur FIRST during cleanup. Partial adds
are tracked before dispatch; matching ADMIN records are removed only for attempted
owned fingerprints, followed by actual Security absence readback. Only fixed
phase/kind, numeric statuses and boolean absence leave memory. Captured trust state,
certificates, argv, passwords and private paths are never diagnostic output.
Cleanup failure fails qualification and retains fixture ownership.

Privileged commands keep an owned supervisor handle: timeout sends SIGTERM and
reserves part of the existing bound for drain. SIGKILL of sudo would not relay to
its command ([upstream monitor](https://github.com/sudo-project/sudo/blob/main/src/exec_monitor.c)).
An unproved stop retains ownership, blocks cleanup mutations and fails explicitly;
no admin-removal success is claimed while an add may still run. Overall18s startup,
20s shared first-call budget and14s cleanup remain unchanged. Local mocked OS tests
prove command-domain/ownership/failure behavior only. Actual hosted readback,
normal URLSession PKI/name/pin/TLS exercise and successful cleanup are still required.

Actual admin-fixture runs now establish a narrower result: [92@d06](https://github.com/MGRAFF2006/mixyard/actions/runs/40993167486/job/123860108783)
and [93@eade](https://github.com/MGRAFF2006/mixyard/actions/runs/40993195716/job/123860202857)
started the shipping Swift client and passed all nine functional TLS/lifecycle
cases, including raw precision and scoped403/409. Both qualifications nevertheless
FAILED cleanup: the first two owned removals returned, the last viewer removal
exceeded its bound with an unproved sudo/security stop; readback still found that
owned record. Restoration/search verification succeeded, but keychain deletion
was correctly withheld. This is not successful trust cleanup or full qualification.

A controlled next diagnostic reports ADMIN-domain record COUNT and OSStatus only,
alongside the three owned-fingerprint statuses, before/ready/precleanup/after.
The resolved certificate array stays in memory; unrelated identities/trust settings are
never output and a count grants no removal ownership. ONLY the same three owned
removals are reversed (viewer,b,a), distinguishing a last-entry condition from a
certificate-specific one without extra certificates or trust-policy fallback.
Apple's [flush implementation](https://github.com/apple-oss-distributions/Security/blob/main/OSX/libsecurity_keychain/lib/TrustSettings.cpp)
treats an empty settings domain differently; that is a hypothesis for these
failures until actual count/order evidence exists, not an observed OS/UI cause.

Before a timed-out privileged command is terminated, bounded read-only metadata
can include only its owned sudo/security process ancestry and numeric PID/PPID/UID,
fixed process name and state. No argv, environment, private path or unrelated
process row is printed. An exited supervisor with unproved inherited pipes is
explicitly distinguished; ownership is retained until actual drain. Sampling uses
part of the EXISTING command/14s cleanup budget. No SIGKILL of a sudo monitor,
deadline increase, success-on-timeout or absent-readback waiver was added. New
count/order/process evidence and actual successful cleanup still require the
hosted runner; local mocked tests cannot qualify them.

Actual [92@f3c](https://github.com/MGRAFF2006/mixyard/actions/runs/40997623898/job/123874357515)
and [93@6c07](https://github.com/MGRAFF2006/mixyard/actions/runs/40997677265/job/123874529769)
reverse-order execution passed9 Swift functional cases but again FAILED cleanup:
viewer and b removed quickly, then LAST a remained after stop-incomplete/drain
failure. The last-removal pattern follows order, not viewer identity; this does
NOT prove empty-domain causation. The reported count0/status-25263 throughout,
even while owned per-certificate statuses were0, was invalid as record-count proof.

Apple's [enumeration implementation](https://github.com/apple-oss-distributions/Security/blob/main/OSX/libsecurity_keychain/lib/SecTrustSettings.cpp)
resolves ADMIN certificates from System.keychain plus system roots, excluding our
private fixture keychain. In contrast, per-certificate CopyTrustSettings reads
fresh domain settings for the supplied certificate. The diagnostic is therefore
now named resolvedAdminCertificateCount; non-success returns null/UNKNOWN, never
zero-as-empty-domain. It is not a total record count even on success. Owned direct
fingerprint absence and actual privileged process/pipes completion remain the
safety gates; no export/import/trust dictionary/policy reset or extra anchor was
introduced to obtain a count. Unexpected OSStatus still fails closed.

Before TERM/drain, a fixed syscall report now distinguishes accepted from numeric
OSError/errno. Accepted means only that terminate() returned, NOT that sudo,
security or inherited pipes stopped; the existing communicate()/readback proof
and ownership retention remain mandatory. No exception text/argv is output and
no privileged signal or kill policy changed. The actual stop/authorization cause
remains unproved until fresh runner evidence; this correction is diagnostic-only.

The next c9/dbd/a24 actual runs show terminate() accepted to the sudo supervisor,
not EPERM, while supervisor/security stayed alive and owned trust remained. A
separate concrete source defect was then found: cleanup sets parent SIGTERM to
SIG_IGN before Popen. Python's [restore_signals](https://docs.python.org/3/library/subprocess.html#subprocess.Popen)
resets its startup-ignored signals, not this application-set SIGTERM. Sudo's
[initialization/restoration](https://github.com/sudo-project/sudo/blob/main/src/signal.c)
preserves inherited ignored dispositions. The new exec-only child launcher resets
ONLY its own SIGTERM to default, then execs the SAME sudo argv/PID; the protected
parent remains unchanged. No shell/preexec_fn/privileged kill, changed timeout or
policy fallback. Actual owned unprivileged POSIX children demonstrate ignored TERM
before and default termination after. That proves the launcher mechanism locally,
not actual macOS privileged cleanup or the last trust-removal cause. Stop/drain
AND owned trust absence still must pass; stopping an authorization wait is not
removal. Actual runner qualification is still held pending that evidence.
