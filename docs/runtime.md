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
No shell commands, arbitrary file paths or device bridge are exposed.

These are operational limits, not mix-count or policy limits:

- 8192 request/response bytes before the newline. One bounded 1024-byte read and
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
