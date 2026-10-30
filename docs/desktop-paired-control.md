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
