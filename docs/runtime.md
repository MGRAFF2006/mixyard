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
  inventory delivery reads/writes at most 64 KiB per tick. Raw helper draining
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
it with secure durable revision state before publishing. Windows/macOS native
inventory providers and their secure revision stores are pending integration;
they currently return explicit provider-unavailable, not fabricated inventory.
Their adapter protocol is `snapshot`, one <=1 MiB schema2 discovery Inventory,
revision 0, actual platform, <=1024/category and <=4096 UTF-8 bytes per ID. The
trusted runtime wrapper assigns evidence and high-water; helper-supplied claims
do not authorize native evidence. Native OS qualification is a separate gate.

For reproducible process/IPC tests only, Linux accepts a local absolute
`serve --inventory-fixture-helper /ABSOLUTE/HELPER` option. It invokes that helper
without arguments, expecting raw pw-dump-shaped stdout, and **always** labels
its inventory `simulated`. This protocol is deliberately different from native
Windows/macOS normalized helper output. It cannot establish native evidence.
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
