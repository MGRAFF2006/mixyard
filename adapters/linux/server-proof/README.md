# Private daemon ingress proof (not production enablement)

MIT, as in the repository LICENSE. This module intentionally uses PipeWire's
uninstalled `private.h`; it is tied to upstream 1.6.9 commit
`8fa27cabdc6c0c1350c69c026af5850ef0af1e26` and its matching server build.
Do not compile or load it with a host SDK/server. No host installation is needed.

Module: `libpipewire-module-mixyard-server-proof.so`, loaded by the **daemon**
through `context.modules` and `PIPEWIRE_MODULE_DIR`. Required args:
`consent.path=/private/consent control.path=/private/control.sock`.
The consent file must be an owned regular file with mode 0600 containing exactly
`MIXYARD_PRIVATE_SERVER_PROOF_V1\n`. Its parent directory and the socket directory
must be owned mode 0700. The daemon must run as that owner. This authorizes this
isolated proof only. Control uses AF_UNIX SOCK_SEQPACKET, mode 0600, same-UID
SO_PEERCRED, one ASCII request/reply per connection, maximum 512 bytes.

Implemented v1 commands (one trailing newline optional):

* `STATUS`: JSON state and exact hidden/public/fixture global IDs and serials.
* `SELECT <node-id> <node-serial> <port-global-id> <port-serial> <owner-id> <owner-serial>`:
  exact output endpoint, port direction and ownership, fixed raw F32P/48000 stereo
  FL/FR. Owner `4294967295 0` is permitted **only** for module-owned fixture nodes.
  A successful reply is `control_done`, never a PCM acknowledgement.
* `RETIRE`: discard the current generation and destroy its owned link.
* `DESTROY_LINK`: exercise destruction of the current owned link.
* `DESTROY_SOURCE <fixture-index>`: destroy fixture 0 or 1; no name/ID fallback.

Set `fixture.sources=true` to enable the two module-owned sources; default false.
Fixture 0 emits FL=0.125, FR=-0.125; fixture 1 emits FL=0.75, FR=-0.75.
`negative.control=true` requires a different explicit consent file containing
`MIXYARD_INSECURE_NEGATIVE_CONTROL_V1\n`. It deliberately exposes ingress and
marks every STATUS `insecure_negative_control=true`; it is only a leakage
negative control, never a protected-mode bypass or production mode.

There is one hidden input port carrying two planar channels, not two DSP mono
ports. Public output is a separate output-only Audio/Source, likewise one stereo
port. Fixtures 0 and 1 emit distinguishable actual frames (not control evidence).
STATUS exposes private identities to the consenting controller; hostile clients
must not obtain the ingress Node/Port/Link from the PipeWire registry.

The threat scope is graph substitution. A compromised selected producer/server
or arbitrary same-user memory/config access is excluded. Positive PCM evidence,
negative leakage, regrant, endpoint replacement, unload/restart and output fanout
must be established by the external harness. Until then the hypothesis is
UNVERIFIED; compile success is not proof.

Build in the coordinated container after building the exact server:
`bash adapters/linux/server-proof/compile.sh SOURCE OUTPUT_DIRECTORY`.
The script uses only the exact source/build include trees and creates a shared
module with unresolved daemon symbols. It never uses pkg-config or host SDK
headers. It requires the existing output directory to have at least 10 GiB free.
After compiling, Python ctypes loads that build's exact libpipewire with
RTLD_GLOBAL, then the module with RTLD_NOW, rejecting any unresolved symbols.
This load audit does not call the module entry point or start a daemon.

Transport limits: 4096 frames/block, eight queued blocks, sixteen SPA buffers,
one shared data loop (checked at initialization). No clock is rewritten.
FORMAT/IO/Start are synchronous; unsupported formats, quantum/rate changes,
source/port/owner/link destruction and format loss close the generation. Queue
and module PCM buffers are zeroed on retirement. Existing frames already read
by consumers cannot be recalled. A consumer reopen that clears output format
requires explicit re-selection. Output fanout is handled by PipeWire's normal
output tee, not by adding another module input or mixer.

SELECT requires a synchronously readable fixed EnumFormat; unresolved async
format enumeration fails closed. A temporary SPA result listener queries
`pw_impl_node_get_implementation(port->node)` at the exact port's direction and
local index; the control protocol still requires its global ID and serial.
No unexported enumeration helper or cached private implementation is used.
Client-owned sources must expose exact
`client.id` ownership. Raw formats must contain FL/FR positions; a port's optional
`audio.channel` property must agree (`FL,FR`). Names and local
port indexes are never accepted as control identities. No pointer is retained
without its destruction listener; listeners revoke before the referenced
object is freed. Failed SELECT retires the prior generation before resolving.

STATUS counters: per node `processed_frames` and `position_frames`, aggregate
`captured_frames`, `emitted_frames` (copied frames, not consumer PCM ACK),
`discarded_frames`, `format_errors`. Mainloop timer performs deferred cleanup
after an RT format/clock failure; the atomic generation gate closes immediately.
An output underflow emits zero; the bounded queue never sums sources.
