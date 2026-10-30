# Native desktop paired management

The desktop's existing shared Vue console now uses either the private numeric
loopback CSRF service or a separately paired native TLS transport. These remain
distinct management targets. Pairing grants neither audio transport nor native
capture/playback/arming/default-device changes. The independent runtime remains
outside the window lifecycle.

## Pair and use a device

1. On the owning device, provision the opt-in `mixyard-control` service and obtain
   `invite --permissions view,control,settings` locally while it is stopped. Choose
   only the independent roles this controller should receive. Start that device's
   control service explicitly on its exact private IP/port. See
   [remote control](https://github.com/MGRAFF2006/mixyard/blob/b3731ce51af02cfbcd0ad6fb7080694f08b6a0bd/docs/remote-control.md) in the service PR #76.
2. Transfer the invitation JSON through your existing trusted channel. In the
   desktop, expand **Pair and manage devices**, enter a display name and the exact
   `https://PRIVATE_NUMERIC_IP:PORT`, and paste the JSON. Verify its device ID and
   certificate SHA-256 against the owning device, then check the trust box and
   choose **Pair and save in secure vault**. No trust is downloaded from the network.
3. Select the saved device in **Manage device**. Use **Refresh** in its shared
   controls to establish a fresh session. The same routing, profiles, processing,
   channel, situation and settings controls apply where independently granted.
   Remote arm/review/live remain unsupported. All changes are stored intent;
   `backendAvailable` and `audioApplied` stay false.
4. **Disconnect** invalidates native and shared-console authority, including queued
   requests. Refresh explicitly to reconnect. An already admitted write may finish;
   reread and review it before any next write. Switching targets retains each
   host's unsaved work and never silently retargets a request.
5. **Revoke this controller on device** requires `settings`, including self-revoke.
   Other grants/controllers remain independent. A view-only controller must ask
   the owner to revoke its displayed controller ID locally. **Remove saved
   credential** deletes this app's native credential; it does not revoke the
   server-side record. Refreshing the saved list and adding a trusted device keep
   all existing API/workspace owners. Removing a target with unsaved work requires
   an explicit guard and retains a disconnected target in the selector for export.
   Re-pairing creates a credential-specific new target; old workspaces cannot
   acquire that new native credential, even if the device ID is unchanged.

Invitation contents live only in the onboarding form and are cleared on submission.
The bearer is returned directly to native Rust and never appears in JS, localStorage,
profile/draft exports, the tray or IPC results. JS uses a non-authorizing sentinel;
only narrow native commands can replace it with the native-owned bearer. The local
HTTP bridge rejects bearer headers and cannot use paired credentials.

## Native boundary

`apps/desktop/native-control` is the production library used by the Tauri shell
and the dedicated qualification executable. It opens only the configured numeric
loopback/private/Tailscale/IPv6 ULA endpoint. TLS 1.3 verifies the exact provisioned
leaf certificate bytes and normal IP SAN/name/time/signature checks against an
application-only trust store containing that single OOB certificate. No system
root import, global trust changes, TOFU, permissive verifier, DNS, proxy, redirect,
URL bridge or shell/filesystem bridge is introduced.

Each request has a 10-second total I/O deadline, a 2-second connect bound, a 1 MiB
request cap, 16 KiB headers and 2 MiB response cap (the shared web parser's bound).
HTTP/1.1 responses require one Content-Length and JSON content type; chunked and
ambiguous framing are rejected. Native operations admit four pending calls and
serialize within each host. Atomic per-host generations reject stale queued calls
and completions after disconnect/reconnect. Wrong device/authority/revision/status
fails closed. A 401/409 requires fresh authority without replay; ordinary 403
retains unrelated granted rights. Canonical expected settings/profile revisions
and exclusions/consent remain owned by the existing service handlers.

The original canonical JSON envelope string is sent without parsing/reserializing
its body. Responses remain raw strings through native IPC; separate parsing checks
identity/status only. This preserves `-0`, finite Float64 bit representations and
canonical duplicate-field rejection. PCM never crosses this JSON interface.

The native vault uses keyring 3 with explicit Secret Service on Linux, Windows
Credential Manager and macOS Keychain backends. No production memory or plaintext
fallback exists. A secure write/read/delete probe precedes consuming an invitation;
new credential writes are read back before registering the device. Vault entries
use the platform's binary secret API with UTF-8 bytes: Windows password helpers
would otherwise expand JSON to UTF-16 and exceed its credential blob limit. If storage fails
after network pairing, the invitation may be consumed: revoke the new controller
on the owner before retrying. Records are bounded for Windows credential storage;
the vault index contains at most 16 IDs. A locked/missing vault is an actionable
error; it is never reported as saved pairing.

## Qualification and remaining gates

[The dedicated workflow](../.github/workflows/desktop-paired.yml) builds the actual
production library on Linux/Windows/macOS and uses
[the qualification runner](../verification/desktop-paired/qualify.py) against the
actual frozen production control CLI (`b3731ce51af02cfbcd0ad6fb7080694f08b6a0bd`).
It provisions the CLI's real self-signed rcgen identity rather than a public CA.
Only the vault boundary is an API double. Tests cover pairing, session identity,
stored writes with A/B isolation, permission denial, native/controller and service
restart, stale authority, revocation persistence, explicit disconnect, certificate
mismatch before secret HTTP bytes, raw number preservation, redirects and storage
failure. Additional adversarial TLS responders supplement the actual service.

A successful harness proves production native TLS/service behavior with a vault
API double. A separate owned Windows/macOS platform-vault2303-byte smoke proves
binary secret write/read/reopen/delete on those hosted systems; it does not qualify
physical device lock/hardware security. The TLS harness does **not** prove successful
Linux Secret Service persistence, actual Tauri webview onboarding/OS prompts, physical
network reachability, accessibility, tray behavior, capture, playback, audio quality,
signing, installer execution, store publishing or deployment. Linux's deliberately
absent owned bus separately checks the actual platform-vault failure path.

Only one desktop process may own paired management for a per-user app data
location. An OS-released advisory lock protects multi-record vault index updates;
a second window/process receives an actionable error and can still use its local
console. The lock contains no credentials and is never a plaintext vault fallback.

Stale open/request completions can clean only their captured/returned native
generation. Their abort/rejection cannot invalidate a newer explicit reconnect.
The qualification includes actual shared Api/ConsoleSession late-open and
late-request success/rejection regressions with retained drafts, current validation
and an actual subsequent native-dispatch read.

Native generation and session now live in one short authority-state critical
section: admission/cancellation clears its own state atomically, and publication
or revision updates verify generation under that SAME lock. The separate operation
lock serializes TLS/vault work; authority state is never held across that I/O.
Forget also invalidates opens admitted while its credential removal was pending.

The hosted native suite runs deterministic worker barriers through production
Paired methods and actual pinned control TLS services, with only the vault API
doubled. It covers admission-before-operation replacement, delayed expected
disconnect, and stale TLS publication during another admission, then requires the
replacement's actual request to succeed. A historical exact-b2cb source gate adds
ONLY scheduling hooks and these same tests, requiring exactly three native RED
failures before current tests run. This is not the sequential stdin harness or a
double of native generation atomics. Current-source Rust/TLS execution is pending
until the hosted job actually runs; local formatting/source checks are not proof.

The first7df hosted runs passed the historical RED gate but then ran an executable
with the same three historical failure markers as current. Their shared Cargo
target made artifact reuse a concrete suspect; these runs do NOT qualify the
repair. Historical source now compiles into its own temporary target. Both gates
record source hashes and the exact Cargo-reported executable hash; current runs
that executable directly. Separately built current GREEN execution is still
required; no local native compilation was performed for this fixture correction.

Current pre-admission repair adds a nonauthorizing OpenIntent{generation,serial}.
Preparation is a fast native snapshot/counter increment: it performs no vault or
TLS work and never clears a session. The renderer checks its actual lifecycle
AFTER the snapshot reply and BEFORE dispatching a blocking open. Native ordering reservation
compares the captured generation and cancellation serial under the same Authority
lock before even the first vault read. It does not clear established authority;
credential validation and an atomic post-vault reservation/generation recheck
precede destructive activation. A worker delayed until after a successful
replacement cannot unconditionally advance that replacement. The synchronous
native convenience uses the same preparation/admission path; no JS clock, stored
counter, automatic replay or authorization token is introduced.

Cancellation of a never-admitted intent advances only a bounded serial fence,
not the generation/session. Existing established authority survives even if its
generation equals the cancelled snapshot's base. After admission, cancellation
can clear only the exact admitted serial AND generation. Other queued stale
intents fail and require explicit Refresh, with no automatic retry. Constant-size integer/
optional fields bound bookkeeping; no ticket map or waiter grows over time. All
session publication/revision writes still use Authority; no state lock crosses
vault/network I/O or waits for the separate operation mutex.

Management now emits authoritative disconnection on ALL settlement paths,
including vault failure, for the captured device AND credential identity. A
Refresh that completed while Forget waited cannot leave the shared API writable
or validated after native final invalidation. Retired/new-credential workspaces
are not silently rebound or cleared by a different target's management callback;
existing list reconciliation, cached drafts and codecHost ownership are retained.

Local evidence executes the actual compiled PairedDevices script setup with real
Vue reactivity, pairedApi/Api and ConsoleSession, doubling only invoke scheduling
and component event delivery. Exact-f6 management source is causally RED; current
is GREEN with pending Forget -> successful replacement Refresh/validation ->
vault error/final native invalidation -> shared authority clear/draft retention/
zero subsequent write dispatch. The four preceding late-open/request regressions
and new late-prepare success/rejection cases also pass locally.

Actual native current proof is still pending. The new same-source vault barrier
is compiled separately against exact-f6 production source (no production overlay)
and must fail ONLY after its replacement made a real TLS request and was then
erased by the obsolete open. Existing exact-b2 three-cause RED stays unchanged.
Current executable adds actual pre-vault, pre-worker, cancellation before/after
admission and pending-Forget vault-error TLS tests. Sources/executables are
independently attested in isolated targets. No local Rust/SDK build or native
race execution is claimed for these new tests. Fresh whole final reviews and
required current-head checks remain mandatory.

Actual e7 hosted jobs passed all13 native tests, including historical b2 three-RED
and f6 pre-vault RED, then FAILED the full qualifier at its existing wrong-credential
assertion: a rejected supplied credential had already cleared an established B.
Those failures are retained; the qualifier assertion is unchanged. Reservation
and activation are now separate. A pre-vault reservation fences obsolete intents
without clearing any session. Only after the supplied stored credential matches
can a SAME-lock freshness recheck atomically activate the replacement. Errors
cancel only that ticket, never restore a captured old session over concurrent B.
The current actual-TLS suite adds a14th regression: current B real request ->
wrong-credential open rejection -> same B real request must still work. Exact e7
production source must first be causal RED for this assertion in its own target.
Fresh current14-test execution AND the complete existing qualifier remain pending;
no local Rust compile/execution or successful full native qualification is claimed.

The exact-e0 native suite/full qualifier passed on all3OS with14/14 and all causal
historical gates, but final review found a renderer-only saved-list race: a
successful Pair/Forget notification was dropped while an older list reply was
pending. The current correction keeps one in-flight read and ONE pending boolean.
A newer notification makes that preceding response/error unpublishable and causes
a single coalesced followup. No timer, polling, growing queue, write replay or
manual-refresh workaround is added. Accepted snapshots preserve existing exact
credential API owners and codecHost; errors cannot publish a partial registry.
Only this operation's old error notice is cleared, preserving unrelated notices.
Actual compiled App.loadTargets plus actual PairedDevices Pair/Forget callers are
RED on e0 for both lost notifications and GREEN currently. Six controlled-invoke
consumer cases cover startup, removal, multiple changes/delayed latest response,
stale/current errors and notice ownership, with one native list maximum active.
These are Vue/script consumer checks, not newly executed native/physical UI proof.
Rust/TLS/control/Cargo inputs stay unchanged; fresh current-head hosted checks
and NEW whole independent final reviews remain mandatory.
