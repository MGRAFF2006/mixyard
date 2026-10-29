# Authenticated stored-intent device control

`control/` is an independent workspace, not an extension of the unauthenticated
loopback CSRF token. It uses TLS 1.3, a persisted device certificate/key and
256-bit random pairing invitations and bearer credentials. It calls the existing
typed `src/web/api.rs` handlers through `src/web/remote.rs`; profile validation,
atomic expected-revision writes, automation ownership and sensitive-route policy
remain canonical. `backendAvailable` and `audioApplied` stay false. Managing a
device does not make it an audio destination.

## Provisioning and lifecycle

All commands require `--state DIRECTORY`, a dedicated private local directory.
Initialize once with `init --ip IP`. IP must be exact loopback, private IPv4,
IPv6 unique-local or a Tailscale `100.64.0.0/10` address. The generated certificate
contains SANs for that IP, `127.0.0.1`, `localhost` and `mixyard-control`. The
stable random device ID, DER certificate, PEM certificate and PKCS#8 key persist
in `state.json`. Corruption is an error, never identity regeneration.

`init` and `trust` print one public JSON object:

```json
{"deviceId":"…","serverName":"mixyard-control","certificatePem":"…","certificateSha256":"…"}
```

Provision this certificate/device ID out of band. Native clients use this exact
certificate as their provisioned trust anchor, normal TLS name/time/signature
validation, and `serverName: mixyard-control`; address resolution is separate.
The service supplies no trust-on-first-network-contact mechanism, trust bypass,
public certificate download or unauthenticated administrative endpoint.

`invite --permissions view,control,settings [--ttl-seconds 300]` is a **local**
operation while the server is stopped. Choose any nonempty subset of the three
independent roles. Lifetime is 1–300 seconds. Output adds `invitation`,
`permissions` and `expiresAtMs` to the public trust object. Only its hash persists;
pairing consumes it atomically. HTTP cannot choose or upgrade its roles.

Start explicitly:

```sh
mixyard-control --state STATE serve --enable-control --bind 127.0.0.1:0 \
  --store PROFILES --assets ASSETS --instance isolated-instance
```

`--bind` accepts the exact provisioned private IP, or loopback for isolated
testing; no wildcard/public listener or reverse proxy is provided. A private
network is transport reachability, never authorization. Every API operation still
requires pairing or authentication. The first stdout line is:

```json
{"event":"listening","address":"127.0.0.1:PORT","deviceId":"…","authorityId":"…","tls":true}
```

The service owns one canonical automation worker and starts disarmed. Do not run
another web/automation owner against the same profile store. Remote arm, review
and live-reevaluation routes are unsupported: remote credential revocation cannot
yet invalidate a canonical arming review. Stored settings cannot grant native
consent. No source format, rate or channel adaptation occurs in this service.

SIGTERM on Unix and Ctrl-C drain owned connections and release the identity lock.
Connections have a 15-second total deadline. Blocking commits retain their owner
until finished, even if the socket times out. Wait for process exit before
restarting. Credentials, device identity, certificate and stored intent persist;
the new authority ID invalidates old scoped requests.

## HTTP contract

All routes require POST, `Content-Type: application/json`, the exact configured
TLS Host and an optional Origin matching `https://HOST:PORT`. There is no CORS
grant or preflight support. A direct browser needs an already trusted certificate
and the same origin. Cross-origin browser deployment and native certificate
provisioning remain client integration gates; an HTTP-only local desktop bridge
must explicitly report remote TLS as unsupported. Never send bearer credentials
to the existing loopback HTTP CSRF API.

`POST /control/pair` accepts `{deviceId,invitation,name}` and returns
`{deviceId,credentialId,credential,permissions}`. The credential is returned once;
the server persists only its SHA-256 hash. Names are presentation, at most 128
UTF-8 bytes, not paths or identity. There are at most 32 outstanding invitations
and 128 active credentials; admission fails explicitly rather than pruning.

Every other operation requires exactly one header:
`Authorization: Bearer CREDENTIAL`. Credentials and invitations contain 256 bits
of OS entropy. Bearers are management authentication, never audio grants or QUIC
client certificates. Do not put them in URLs, draft exports, logs or browser
localStorage. A client may hold an ephemeral credential or use its native secure
credential store; this crate does not implement a client keychain.

`POST /control/session` accepts `{}` and returns:

```ts
type Permission = 'view' | 'control' | 'settings'
type Session = {
  deviceId: string
  authorityId: string
  revision: number
  permissions: Permission[]
  backendAvailable: false
  audioApplied: false
}
```

`POST /control/request` requires this frozen envelope:

```ts
{ deviceId: string, authorityId: string, revision: number,
  path: string, body: unknown }
```

`revision` is the current **profile store revision**, including for reads. Identity,
authority and revision are checked before dispatch. Canonical bodies retain their
own `expectedRevision`, `expectedSettingsRevision` and typed policy fields. The
response is `{deviceId,authorityId,revision,status,body}`, with HTTP status equal to
`status`, canonical JSON in `body`, and the freshly read profile revision. Error
responses from canonical handlers retain this envelope and exact status. Protocol
admission errors return `{error:{code,message}}` without an identity envelope.

| HTTP status | Admission error |
| --- | --- |
| 400 | `invalid-json`, invalid provisioning fields |
| 401 | `unauthorized`: missing, rotated/revoked credential or expired/consumed invitation |
| 403 | `permission-denied`, `invalid-host`, `invalid-origin` |
| 409 | `target-mismatch`, `session-changed`, `store-conflict` |
| 413 / 415 | bounded/invalid body or JSON content type required |
| 422 | `unsupported-route`, including remote arming and native audio |
| 503 / 504 | unavailable/busy store or operation failure/timeout |

Clients must freeze scope before I/O and discard completions after switching,
revocation, reconnect or restart. Reread `/control/session` after conflict; never
automatically replay a write. A timeout/disconnect does not prove a write failed.
Preserve unsaved drafts per device and review against the new session. The API
never retargets an old request, copies credentials between devices or retries.

## Least privilege and credential administration

Roles are independent, with no implicit administrator or wildcard-route grant:

| Permission | Exact canonical routes |
| --- | --- |
| `view` | `/api/status`, `/api/validate`, `/api/inspect`, `/api/profiles/read`, `/api/dsp/{preview,validate,delay}`, `/api/automation/{status,validate,evaluate,context}`, `/api/automation/settings/read` |
| `control` | `/api/profiles/edit`, `/api/automation/{disarm,manual}` |
| `settings` | `/api/automation/settings/write` and credential administration below |

All other canonical paths fail closed. Profile edits remain stored intent with
the existing sensitive-route reporting, not fresh microphone or playback consent.

`/control/credentials`, `/control/revoke` and `/control/rotate` require `settings`
and `{deviceId,authorityId,revision,body}` with no `path` field. Bodies respectively
are `{}`, `{credentialId}`, and `{}`. Listing returns
`body:{credentials:[{credentialId,name,permissions}]}`. Revocation returns
`body:{revoked:true}`. Rotation replaces **only the caller's own** bearer and
returns `{deviceId,credentialId,credential,permissions}` in `body`; it cannot mint
another controller's privileges. All responses use the scoped reply envelope.
Old bearers immediately fail. Revoking a different credential does not change
other controllers' roles. Local `revoke --credential-id ID` works while stopped.

Authenticated operations and credential changes serialize through one bounded
operation slot. Revocation cannot acknowledge success while an earlier admitted
write is still executing; later writes authenticate against the updated store.
Concurrent requests can get `503 service-busy`; callers must refresh and review.
Every HTTP body is at most 1 MiB, responses at most 9 MiB including the envelope,
and at most 32 TLS/HTTP connections are admitted. HTTP/1 framing is Hyper-owned,
TLS is Rustls/Tokio-Rustls, 0-RTT is disabled, and no wire parser is handwritten.

## Persistence, recovery and local audio provisioning

The identity directory permits only `state.json`, `owner.lock` and `pending.json`.
On Unix, the existing core filesystem checks reject symlinks, foreign files,
group/world access and hardlinks; files are 0600 and directories 0700. The Unix
trusted-local-filesystem/cooperating-writer boundary is unchanged.

Windows uses a control-specific secure store, separate from the nonsecret profile
store. The directory must have an existing, trusted parent on a local fixed NTFS
volume. It creates the directory, state, pending file and owner lock with an
explicit current-user owner and a protected DACL granting only that SID full
access, before writing any key bytes. Existing objects must have that same private
owner/DACL, the expected type, no reparse point and (for files) exactly one link.
Every ancestor's owner and replacement/ACL-changing permissions must belong to
the current SID, LocalSystem, Administrators or TrustedInstaller; unsafe or
unsupported ACLs fail closed. A parent may permit reading or unrelated child-directory
creation, but cannot grant an untrusted principal deletion, child deletion,
owner/DACL changes or rights that could install a reparse point (data, attribute
or extended-attribute writes). The volume root permits unrelated child creation
because it cannot become a reparse directory. No existing ACL is repaired or broadened automatically.

The Windows store resolves the drive to its volume GUID and holds directory
handles without delete sharing for every ancestor throughout ownership. It
rechecks their ACLs before writes, retains an exclusive owner-lock handle, reads
state through its validated handle, and renames the still-open private pending
file by handle. Unsupported paths, missing parents, network filesystems and
unprovable protection fail closed. This protects against other local accounts;
it does not isolate secrets from the same account or Windows administrators.

The process exclusively creates `owner.lock` and holds it for its lifetime.
Updates write a bounded pending file, sync it, atomically rename it and sync the
Unix directory before returning success. A write error poisons the in-process
owner and retains the recovery lock; old credentials are not served from a stale
in-memory copy. An unclean crash can leave lock/pending files. There is **no**
automatic stale-PID, age-based or process-name reclamation. Stop and verify no
owner is running; back up the dedicated state, inspect complete/pending state
and only then remove the abandoned sentinel/pending file. Do not replace state
with pending data blindly. Restart to reread the committed state. Power-loss
durability and manual crash-recovery operations are not qualified here.

The local Rust API is
`mixyard_control::IdentityStore::audio_identity_material(expected_device_id)`.
It requires ownership of the private state and an exact stable ID, returning
`AudioIdentityMaterial { device_id, certificate: CertificateDer<'static>,
private_key: PrivateKeyDer<'static>, server_name: String }`. This structure is not
serialized or exposed through HTTP. Network callers must provide independently
consented exact directional audio grants and own audio-session revocation. Merely
having a management bearer grants no access to this API or permission to audio.
Credential rotation leaves the persisted TLS identity/pin unchanged; certificate
rotation/reprovisioning and its active-audio invalidation are not implemented.

## Evidence and limits

Two real loopback TLS Rust integration tests passed: authentication, one-use
non-escalating pairing, view/control/settings denial, target mismatch, Origin
denial, canonical profile persistence, competing revision rejection, disabled
remote arm, credential rotation/revocation across restart, stable identity/pin,
fresh authority, exact-ID local audio export, competing local owner denial,
different trusted-certificate failure, and explicit opt-in/wildcard/public-bind
denial. Tests use private disposable state and the actual canonical handlers.

Parent cross-client acceptance separately exercised the compiled service with
the actual Vue control API against two isolated TLS stores. Its durable harness
and revision evidence belong to the parent integration PR. This crate's tests do
not claim that frontend integration, physical audio, real LAN connectivity,
browser certificate installation, native/mobile keychains, certificate rotation,
crash recovery or platform GUI workflows have been qualified.
