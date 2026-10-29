# Foreground lifecycle runtime (IPC v1)

Build both binaries with `cargo build --locked --bins`. On Linux/macOS:

```sh
./target/debug/mixyard-runtime serve
# In another terminal:
./target/debug/mixyard status
./target/debug/mixyard events
# In a third terminal (events prints the stopping event and exits):
./target/debug/mixyard shutdown
```

Windows PowerShell uses `./target/debug/mixyard-runtime.exe serve` and
`./target/debug/mixyard.exe status`, `events`, or `shutdown`.
All four commands accept `--instance NAME` (1..32 ASCII letters, digits, hyphens)
for independent namespaces. The default name is `default`.

The runtime is an explicitly started, foreground OS process. Closing a client,
event subscription or UI does not stop it. Closing its owning terminal/session
can terminate a foreground process; use your existing terminal supervisor/tmux
when that lifetime is wanted. Shutdown is an explicit request from the same OS
user. There is no sidecar, UI dependency, client autospawn, service installation,
DBus connection, display requirement, audio capture or default-device change.
No signal handler is installed; forced termination can leave a Unix endpoint.

`status` reports `daemonReachable: true`, `backendAvailable: false`, backend
`unavailable`, and null desired/audio/applied revisions. Native audio proof is
`unverified`. There are no meters, fictional PCM events, accepted audio changes,
or applied audio revisions. `apply`, `mute` and native action dispatch remain
unavailable (IPC returns `backend-unavailable`; unsupported CLI commands exit 2).
Offline `validate`, `plan` and `plan-media` keep their policy schemaVersion 2,
existing semantics and exit codes; they need neither a daemon nor OS sessions.
Lifecycle CLI errors on stderr use ipcVersion 1, and transport failure exits 4.
A rejected IPC request is printed on stdout and also produces a CLI failure.

## Native endpoints and ownership

Linux uses `/tmp/mixyard-<effective-uid>-<instance>/runtime.sock`; macOS uses
`/private/tmp/mixyard-<effective-uid>-<instance>/runtime.sock`. The runtime creates
its directory with mode 0700, requires current-user directory ownership and exact
permissions, creates a mode 0600 pathname socket, and checks peer UID in both
directions (`SO_PEERCRED` on Linux, `getpeereid` on macOS). Symlink directories,
symlink endpoints, nonowned/wrong-permission endpoints and all preexisting
endpoints are refused. Startup never unlinks a stale socket. Shutdown removes
only the socket whose directory and socket device/inode identities match those
created by this listener, leaving the private namespace directory.
After a forced exit, inspect the owning process and manually remove its stale
socket only after confirming that no runtime owns it. No PID-based process kill
or automatic stale-path repair is performed. OS user/root is the trust boundary;
this is not isolation against hostile processes running as your own user.

Windows uses `\\.\pipe\mixyard-<current-user-SID>-<instance>`. It creates a
protected DACL granting only that SID access, rejects remote pipe clients, uses
`FILE_FLAG_FIRST_PIPE_INSTANCE` to refuse an existing server, and continuously
holds a pending pipe instance for the runtime lifetime. Both client and server
check the peer process token's user SID before exchanging requests. Native
handles use overlapped I/O and are not inherited. Cancellation completes before
its buffers/OVERLAPPED structures are released; there are no per-client threads.
Windows releases the endpoint through handle lifetime, with no stale files.
There is no TCP listener or secret/authentication-file scheme. Only Linux,
Windows and macOS are supported by this lifecycle transport.

## Protocol and operational bounds

One request per connection, newline-delimited UTF-8 JSON, for example:

```json
{"ipcVersion":1,"requestId":"terminal-1","command":"status"}
```

Replies contain `ipcVersion`, matching `requestId`, and either `result` or
`error: {code, message}`. The wire version is distinct from policy schemaVersion.
Requests require a nonempty ID of at most 64 UTF-8 bytes, reject unknown fields,
and check the version before command dispatch. Malformed, oversized, invalid-ID,
unsupported-version and timed-out requests have explicit JSON errors; an
unrecoverable/absent ID is null. `events` optionally accepts `afterSequence` (u64).
No shell commands, arbitrary file paths or audio mutation bridge are exposed
through IPC. Inventory is a read-only native observation.

These are operational limits, not mix-count or policy limits:

- 8192 request/event-response bytes before the newline (inventory has its own
  bound below). One bounded 1024-byte request read and
  at most 4096 bytes written per client per 10 ms tick; no blocking server I/O.
- At most 16 active connections and 8 event subscriptions, plus the Windows
  pending listener. Excess subscribers receive `busy`; saturated connections
  wait in OS backlog/pipe admission or fail. Clients never retry native actions.
- Complete a request within 2 seconds. Response delivery/client disconnect has a
  2-second deadline; client initial exchange/pipe-busy retry is bounded to 5
  seconds. Shutdown drains for at most 2 seconds, then closes all clients.
- Every event subscriber receives a current `snapshot` (sequence 1, running and
  unavailable), then a `lifecycle` stopping event (sequence 2) on shutdown.
  Sequence resets per runtime lifetime. Every reconnect resyncs, including when
  a cursor is supplied: `resync: true`, `historyAvailable: false`. No replay log.
  At most two frames are queued per subscriber; future availability updates
  must coalesce into current snapshots within that bound or disconnect slow
  consumers for resync. This slice produces only the two lifecycle frames.

## Evidence and remaining gates

`cargo test --locked --test runtime_lifecycle` exercises real daemon/CLI child
processes with DISPLAY, WAYLAND_DISPLAY and DBus environment variables removed:
concurrent clients, disconnect survival, malformed/oversized/stalled requests,
wrong versions, repeat connections, subscriber limits, explicit shutdown and
restart. Unix tests inspect actual directory/socket permissions, reject stale
and symlink paths, and verify replaced endpoints survive listener cleanup.
Windows's `actual_named_pipe_has_only_current_user_protected_dacl` test queries
the actual kernel object's owner/DACL and verifies a single current-user ACE
with inheritance disabled. It runs in hosted Windows builds; it is not a model.

`.github/workflows/runtime.yml` builds both binaries and runs core plus lifecycle
checks on hosted Linux, Windows and macOS. A compile-only cross-target check is
not proof of another OS's runtime. The desktop can explicitly attach a typed read-only subscription to an already
running owned instance. `mixyard::runtime::ReadOnlyClient` consumes a validated
initial snapshot, exposes bounded nonblocking `poll`, and disconnects on drop;
it cannot send mutations or shutdown. Unknown capabilities/fields, version or
request-ID mismatch, oversized/partial frames and event order mismatch fail
closed. No GUI dependency is added to the root library.
The isolated native harness in `verification/desktop-native` checks actual
WebKit Connect and runtime process/IPC survival across UI exit. Only a passing
current artifact establishes that scope. Native audio, device permission flows,
audio/UI independence under load, routing, recovery, latency and quality remain
unproved until a real native transport/engine is bound and qualified.

Implementation references: [Linux unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html),
[Apple getpeereid source](https://github.com/apple-oss-distributions/Libc/blob/main/gen/FreeBSD/getpeereid.c),
[Microsoft named-pipe security](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights),
[CreateNamedPipeW](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-createnamedpipew),
and [overlapped I/O](https://learn.microsoft.com/en-us/windows/win32/ipc/synchronous-and-overlapped-input-and-output).

## Read-only inventory (Linux PipeWire)

`mixyard-runtime inventory [--instance NAME]` sends the existing private IPC
`inventory` command and prints its envelope. On Linux the runtime explicitly
runs `/usr/bin/pw-dump` with no arguments, reads a single complete snapshot, and
translates the Node/Device/Port data into schemaVersion 2 `Inventory`. It does
not invoke the adapter's mutation commands, record microphone samples, create
streams, start playback, link ports, or change defaults. `backendAvailable`
remains false and native audio proof remains unverified; inventory does not
claim `audioApplied`, routing, media control or permission to capture a mic.
Existing policy consent, exclusions and producer recapture checks still apply.

The schema1 [backend contract](backend-contract.md) is a device discovery
envelope, not a schema2 inventory. Its device list drops application streams and
current formats, so this provider validates the same underlying Linux snapshot
shape directly. It does not import a fixture or accept helper paths/observations
through HTTP or IPC. `/usr/bin/pw-dump` is a trusted local installation path;
a missing executable, inaccessible server or diagnostic stderr returns scoped
`inventory-unavailable`, never successful empty discovery. Stderr contents are
not forwarded. A complete Core object is required even for a genuinely empty
graph. Known `support.node.driver` nodes are nonaudio; an audio node with missing
identity, missing device reference, error state or profile-switch placeholder
makes the observation unavailable. Unknown nonaudio interfaces are ignored.

Identity rules:

- Mic/output `deviceUid` is `pipewire:node.name:<exact node.name>`, matching the
  existing schema1 device catalog. Names are profile dependent; they are not
  permanent physical hardware IDs. There is no label, default-device, PID or
  numeric object-ID fallback. Distinct observations of the same selector remain
  distinct, so policy resolution reports ambiguity.
- Application streams require exact `application.id`. Communication/unknown/absent
  media roles fail closed because schema2 requires an explicit
  call-return group; no such group is invented. Unmapped or unclassified application streams need trusted local classification
  before they can be represented. No call is silently relabeled as ordinary media.
- Context discovery separately uses `context:pipewire:<digest>` over direction
  and the semantic identity tuple. A context catalog UID and an inventory
  selector UID are different namespaces. Nothing auto-merges them or treats
  matching display names as identity. Explicit user configuration must select
  each catalog's exact identity.
- Signed 64-bit numeric identities convert directly to decimal strings; floats
  and lossy encodings are rejected. `observedId` binds the runtime object ID,
  object serial if available, Core cookie, exact device serial, current native
  Format detail and channel-port identities. It is opaque and is never used as
  the semantic selector.
- `format` is present only for a single concrete current Node `Format` with
  positive exact integer rate and explicit `MONO` or ordered `FL,FR` positions.
  `EnumFormat`, configured `audio.rate`, default graph clocks, channel counts,
  AUX/reversed/surround positions and absent values do not establish a format.
  Unrepresentable or unknown format remains null, so the existing planner keeps
  that path unresolved. Schema2 does not expose PCM encoding or arbitrary
  layouts; their observed changes still invalidate the inventory revision.
- Mixyard producer sources retain `mixyard-producer` origin; they cannot be
  recaptured as external sources. An owned sink is unavailable because schema2
  has no output-origin field. Virtual sources receive mic consent protections;
  enumeration never grants consent. Media sessions/actions and backend capacity
  remain unproved, represented by an empty session list and unknown limits.

`inventory-revision-v1` in the existing private instance directory retains only
an 80-byte versioned, checksummed SHA256 digest and u64 high-water revision.
Normalized content is sorted, revision zeroed before hashing, and unchanged
refresh/restart preserves the revision. Changed content increments it, including
returning to older content. The state must be a real single-link current-user
file with mode 0600; namespace device/inode and ownership are rechecked before
completion. Updates use a random create-new 0600 temporary file, fsync, atomic
rename and directory fsync. Corruption, symlinks, insecure state, failed writes
and exhaustion produce unavailable without resetting or overwriting the bad
state. No raw inventory/application metadata is persisted. Keep this private
state when restarting an instance. Removing the namespace loses its high-water;
a connected owner then rejects regressed/reused revisions and must be explicitly
re-established. `/tmp` state does not promise persistence across machine reboot.

Additional operational bounds:

- One discovery at a time; concurrent inventory requests return `busy`. Status,
  event and shutdown clients continue while the helper runs. Client disconnect,
  cancellation and shutdown kill the owned helper process group and reap its
  leader. No unrelated process is targeted.
- Helper I/O/exit deadline 700 ms, at most 4 MiB raw stdout and 8192 objects.
  Validation/normalization/persistence/serialization have a 1200 ms total
  preparation cutoff from connection admission, leaving delivery time within
  the typed client's existing 2-second total deadline. This is a bounded
  synchronous validation of bounded data, not a real-time scheduling guarantee;
  OS scheduling/filesystem stalls can still make a client time out safely.
- Inventory alone permits a 1 MiB reply and up to 1024 entries per category,
  4096 UTF-8 bytes per identity. Requests/events retain their 8192-byte bounds;
  inventory server delivery writes at most 64 KiB in at most 32 calls per peer
  per loop, using a 1 ms idle interval while a reply remains pending. The typed
  client continues immediately after successful short reads (at most 64 KiB
  each), checks cancellation/deadline every iteration, yields every 32 successful
  operations and waits 1 ms on blocked reads after receiving partial data.
  Before receiving data it retains the ordinary 10 ms idle interval. Raw helper draining
  performs at most 32 bounded reads per tick. Oversize/count errors explicitly
  ask to reduce the visible graph; nothing is silently truncated. A large graph
  can hit the byte cap before its entry cap. The Windows overlapped transport
  has a matching 64 KiB read buffer; a completed read retains any bytes not yet
  consumed if the caller supplies a smaller slice on its next poll. Actual OS
  transport tests cover valid large inventories and this changing-slice case.
  Oversized-peer tests accept only expected disconnect errors from that hostile
  writer and require InvalidData at the client, so a timeout is not a passing
  admission result.

`src/runtime/inventory.rs` defines the platform-neutral `Capture::poll()`
boundary and trusted options. A provider returns `Result<Option<Inventory>>`
without blocking on subprocess I/O, owns cleanup on drop, translates exact
platform identities/formats, and starts at revision zero. The runtime completes
it with secure durable revision state before publishing. Windows/macOS use the
installed read-only providers and persistent secure stores described below.
Their adapter protocol is `snapshot`, one <=1 MiB schema2 discovery Inventory,
revision 0, actual platform, <=1024/category and <=4096 UTF-8 bytes per ID. The
trusted runtime wrapper assigns evidence and high-water; helper-supplied claims
do not authorize native evidence. Native OS qualification is a separate gate.

For reproducible process/IPC tests only, all three platforms accept a local absolute
`serve --inventory-fixture-helper /ABSOLUTE/HELPER` option. It invokes that helper
without arguments on Linux, expecting raw pw-dump-shaped stdout. Windows/macOS
fixtures use the schema-2 helper contract and fixed `snapshot` argument instead.
Every fixture inventory is **always** labelled `simulated`; none can establish
native evidence.
`cargo test --locked --test runtime_inventory` exercises actual helper/runtime
child processes and typed IPC with owned fixtures: precision, formats,
refresh/restart/content changes, malformed/partial/oversized output, timeouts,
concurrent busy, cancellation, shutdown and secure revision state. It also
starts the actual HTTP service and confirms `/api/automation/live` accepts the
actual runtime IPC inventory and its stable restart revision, with both
`audioApplied` and `backendAvailable` false. These are
process and IPC proofs, not native PipeWire discovery or audio qualification.

Linux native capture recipe (run in the laptop user's actual PipeWire session,
with no fixture option, no speaker playback and no graph mutations):

```sh
# Use a private external target; first confirm sufficient space.
df -h /tmp /dev/shm
export CARGO_TARGET_DIR=/tmp/mixyard-native-inventory-target
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2
cargo build --locked --bin mixyard-runtime --bin mixyard
"$CARGO_TARGET_DIR/debug/mixyard-runtime" serve --instance native-inventory
# In a second terminal, in the same checkout/user environment:
"$CARGO_TARGET_DIR/debug/mixyard-runtime" inventory --instance native-inventory
"$CARGO_TARGET_DIR/debug/mixyard-runtime" inventory --instance native-inventory
"$CARGO_TARGET_DIR/debug/mixyard" status --instance native-inventory
"$CARGO_TARGET_DIR/debug/mixyard" shutdown --instance native-inventory
# Restart the first command, repeat inventory to check stable content/revision,
# then explicitly shut down again. Keep the private revision state for this test.
```

Do not publish raw native output with private application/device IDs. Record
revision/SHA, exit codes, evidence/platform, endpoint counts, nullable-format
counts, revision equality/change and scoped failure reasons. Native tests on a
host without PipeWire can establish unavailable behavior only.

Discovery parsing references: [PipeWire pw-dump source](https://github.com/PipeWire/pipewire/blob/master/src/tools/pw-dump.c), [SPA audio format parameters](https://docs.pipewire.org/group__spa__param.html), and [standard PipeWire driver configuration](https://github.com/PipeWire/pipewire/blob/master/src/daemon/pipewire.conf.in).

## Installed Windows and macOS inventory providers

Production uses `current_exe()` to find exactly `mixyard-windows-inventory.exe`
or `mixyard-macos-inventory` beside the runtime. There is no native helper path
flag, environment override, PATH lookup, shell, HTTP path, or IPC observation
submission. Explicit `--inventory-fixture-helper` always assigns `simulated`,
regardless of the child's evidence claim. The sibling installation is the trust
boundary: the user explicitly selects reviewed local build artifacts to install.
Unsigned local development builds are supported; this is not code-signature or
publisher attestation. A different untrusted executable must never be copied to
the installed production helper name.

Windows requires a real single-link helper owned by the current SID, a protected
DACL with one current-SID full-access ACE, and the same private DACL on its parent
installation directory. Every ancestor must be owned by the current SID,
LocalSystem, Builtin Administrators or TrustedInstaller. Effective allow entries
granting deletion, child deletion, owner changes or DACL changes to any other
principal are rejected after mapping generic rights. Read/execute and unrelated
child creation are allowed; inherit-only entries are checked where they become
effective on the actual selected descendant. Unknown effective ACE types fail
closed, and deny entries do not excuse a dangerous allow entry. This conservative
policy can reject unusual managed ACLs even when a more complex access check
would deny the mutation. Ancestors are also checked for reparse points and pinned
without delete sharing. macOS requires a real single-link executable owned by
root/current UID, no set-ID bits, no group/other writes, and trusted real
ancestors owned by root/current UID with no group/other writes. Additional Darwin
ACL allow grants are rejected (deny entries are allowed). Keep installations in
a private local user directory; symlinked/reparse/redirected installations are
intentionally unavailable. Installers refuse to replace an existing installation.
They copy artifacts only; they do not launch a service, install drivers or SDKs,
or change login startup. The Windows installer also rejects unsafe Known Folder
ancestry before copying the selected runtime/helper executables.

Build using the existing SDK recipes in [native inventory](platform-native-inventory.md),
then install on Windows in PowerShell 7 with an already installed toolchain:

```powershell
cargo build --locked --bin mixyard-runtime
./scripts/install-inventory-runtime.ps1 -Runtime target/debug/mixyard-runtime.exe -Helper build/inventory-windows/Release/mixyard-windows-inventory.exe -Name dev-2026-10-29
# The script prints the installed runtime's absolute path under LocalAppData.
# Run that exact executable with: serve --instance native-check
# Query it with: inventory --instance native-check
# Stop it with the existing mixyard shutdown --instance native-check command.
```

On macOS, with developer tools and SDK 14.2+ already installed:

```sh
cargo build --locked --bin mixyard-runtime
bash adapters/macos/inventory/build.sh
bash scripts/install-inventory-runtime.sh "$PWD/target/debug/mixyard-runtime" "$PWD/adapters/macos/inventory/build/mixyard-macos-inventory" "$HOME/MixyardRuntime-2026-10-29"
# Use the printed runtime path with serve/inventory --instance native-check.
# Shut down explicitly with mixyard shutdown --instance native-check.
```

The provider spawns exactly `snapshot`, with null stdin and private pipes.
Windows creates the process suspended, assigns an owned job with kill-on-close
and no breakaway, and resumes only after assignment. Only the three standard
handles are inherited. Bounded `PeekNamedPipe`/`ReadFile` polling never waits for
unavailable bytes. macOS uses nonblocking pipes and an owned POSIX process group;
`waitid(WNOWAIT)` reserves its leader PID until cleanup. Cancellation, disconnect,
timeout, parse failure and shutdown terminate the owned group/job and reap the
leader. The runtime's 700 ms helper budget, 1200 ms preparation cutoff and typed
2-second client deadline still apply; requests/events retain their 8 KiB bounds.

Exactly one LF-terminated schema-2 snapshot, revision 0, matching the actual OS,
zero stderr bytes and successful exit is required. Native stdout is bounded to
1 MiB, each category to 1024 entries and each string to 4096 UTF-8 bytes (control
characters are rejected by shared admission). Wrong platform, duplicate JSON
keys/observed IDs, partial fields, malformed or invalid concrete formats,
nonzero exit, diagnostic stderr and oversized output return `inventory-unavailable`.
Private child errors are never forwarded. Successful empty enumeration is allowed;
an empty helper failure envelope is never treated as success.

The wrapper preserves byte-exact native selectors, nullable formats and origin
exclusions. It assigns evidence from invocation provenance, replaces ordinal
observed IDs with bounded selector digests plus occurrence IDs, and sorts all
represented properties before revision hashing. Duplicated semantic selectors
stay distinct and ambiguous. This prevents OS enumeration order alone from
advancing revisions. The helper's ordinal tokens and claimed revision/evidence
cannot establish freshness. Windows app-source identity remains unavailable;
no PID, endpoint, executable-label or media-session identity is substituted.
macOS bundle/process limitations and pre-14.2 app-discovery limits remain as
specified in the helper documentation. Native discovery does not grant consent.

Persistent state locations, per validated runtime instance:

| OS | Private state directory |
| --- | --- |
| Windows | `FOLDERID_LocalAppData/MixyardInventory/instance-<instance>` via `SHGetKnownFolderPath` |
| macOS | account-database home + `Library/Application Support/MixyardInventory/<instance>` via `getpwuid_r` |

`HOME`/`LOCALAPPDATA` overrides and request fields cannot choose these paths.
Only the 80-byte checksummed digest/counter is retained, using the same revision
codec as Linux. Unix state is 0700/0600, no symlinks, single-link regular files,
and pinned-directory `openat`/`renameat` plus file/directory fsync. Windows state
directories/files have protected current-SID-only ACLs, no reparse points or
hardlinks, and pinned ancestors under the same ownership/mutation policy used
for installations. Known Folder lookup establishes location, not ACL safety;
unsafe redirected ancestry fails before creating state. Updates use create-new private files,
`FlushFileBuffers` and same-volume atomic `MoveFileExW(REPLACE_EXISTING |
WRITE_THROUGH)`. Read, permission, corruption, persistence or counter exhaustion
failures do not reset the counter or replace bad state. Unchanged refresh/restart
retains its revision, changes and reversions increase it. Installation replacement
also retains state because it is outside the install directory. Deleting state
manually discards that guarantee and requires discarding outstanding reviews;
the runtime never automatically deletes or repairs revision state.

`runtime_native_inventory` runs actual runtime/fixture processes and typed OS IPC
on hosted Windows/macOS: provenance, exact IDs, null formats, revisions across
restart/reversion, private errors, malformed/partial output, cancellation,
descendant cleanup, shutdown, large replies, corruption, hardlinks and exhaustion.
Kernel-level state tests additionally reject insecure ACLs/modes, including a
private leaf under a shared parent before initialization and after prior pins
close. That regression retains an existing revision-2 record while rejecting
all delete/ACL/owner mutation grants and replacement-tree initialization. Runtime CI
builds the unchanged native SDK helpers, installs the actual sibling artifacts,
and exercises the production provider over real IPC. It accepts a scoped OS
helper failure on a headless runner, but fails for missing/untrusted installation,
unimplemented providers, malformed helper output or insecure state. It uploads
no raw native catalog. These are hosted OS/SDK and process/IPC checks, not physical
Windows/macOS audio, hardware, privacy-prompt, Bluetooth, latency or quality
qualification. `backendAvailable: false`, unavailable apply/mute/native dispatch
and `audioApplied: false` remain unchanged.

Implementation references: [Windows job ownership](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects),
[atomic Windows replacement](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw),
[parent deletion rights](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-deletefilew),
[generic rights mapping](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-mapgenericmask),
[Windows principals and TrustedInstaller SID](https://learn.microsoft.com/en-us/archive/msdn-magazine/2008/november/access-control-understanding-windows-file-and-registry-permissions),
and [Darwin descriptor ACL retrieval](https://github.com/apple-oss-distributions/Libc/blob/main/posix1e/acl_file.c).
