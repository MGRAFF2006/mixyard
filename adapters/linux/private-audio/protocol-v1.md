# Shared prototype wire baseline

The parent-owned Rust definition is `src/runtime/private_audio/protocol.rs`.
All field names are camelCase, action/event/operation kind tags kebab-case.
Unknown keys/versions fail closed. This is owned-private-generated-v1 only;
legacy lifecycle status stays unavailable/false/null.

Public private endpoint: same-user0600 `private-audio.sock` in the owned0700
runtime instance directory. Requests8KiB/results64KiB, newline-delimited JSON.
Companion control: stdin/stdout newline JSON <=1MiB, with a separate priority
cancel socket passed by `--cancel-fd`. No PCM on either channel. Native callbacks
publish only preallocated scalar receipt state; control serializes off callback.

Companion command:
`{companionVersion:1,instanceNonce,controlEpoch,operationId,action:{kind,...}}`.
Reply echoes those four keys and has `event:{kind,...}`. Exact schemas are in
the Rust enums; do not introduce independently named alias fields.

- `prepare {generation,snapshot:{storeRevision,configRevision,configSha256,config}}`
  creates owned generated sources and dynamic canonical bus publication; no
  host installed. Returns `prepared {preparedToken,inventoryRevision,outputs}`.
- `link {preparedToken,outputId,consumer:{nodeId,nodeSerial,portId,portSerial}}`
  binds only actual private-core endpoint/serial/format; returns `linked`.
- `stage {preparedToken,expectedInventoryRevision}` compiles the canonical host
  outside the Store guard after native formats/headers/clock are ready. Returns
  `staged {stagedToken,generation,configRevision,inventoryRevision,requestDigest,
  endpointSetDigest}`. This is NOT Installed.
- `install {stagedToken}` queues a retained native-owner ticket. `installed`
  carries actual generation/revision/clock/switch-frame/first-process and
  per-output first commit receipts. The parent now holds the canonical Store
  current-snapshot guard, not before Prepare/Stage.
- `fence {expected:StopTarget,newControlEpoch}` is accepted on the priority
  cancel socket even when the mutator is occupied. Advance epoch, reject old
  tickets permanently, revoke/clear owned state on the actual DataLoop owner.
  Reply `fenced {receipt}` distinguishes canceled-before-switch and actual
  aborted-after-switch. No late queued install is allowed after fence proof.
- `snapshot` returns the cached scoped state; no Store writer guard.
- `shutdown` terminates only after actual ownership retirement; it must not
  fabricate a successful fence/scrub acknowledgment.

`StopTarget` is the exact tuple `{instanceNonce,controlEpoch,activeGeneration,
pendingInstall:null|{operationId,reservedGeneration}}`, including null keys.
A new runtime uses a fresh256-bit nonce; counters never wrap. Status exposes
reserved generation/pending op. Stale Stop rejects a replacement runtime.

Runtime owns two children: private pinned-prefix daemon, and this companion.
Runtime creates private config/namespace before starting either. Companion argv:
`--descriptor ABS --instance-nonce HEX64 --control-epoch E --cancel-fd FD`.
The runtime supplies sanitized PIPEWIRE_RUNTIME_DIR/CONFIG_DIR and explicit
prefix library/module/plugin paths; remote is `mixyard-private`, never personal
or system. The descriptor and all qualified-file hashes are validated by the
runtime before spawn and observed SDK bindings remain point-scoped evidence.

Descriptor schemaVersion1/scope owned-private-generated-v1 has `sdkPrefix`,
`prefixManifest`, `prefixManifestSha256`, `companionExecutable`,
`companionSha256`, `sdkVersion` (literal `1.4.8`, experimental scope), `sdkCommit`,
`generators:[{sourceId,left,right,markerPeriod,frameLimit}]`,
`limits:{maxSources,maxOutputs,maxResidentBytes,maxFrames}`. All paths absolute;
critical files regular and prefix-contained, generators finite, frameLimit0
means unbounded input (not a finite backend timer). Format is explicit
f64-le/48000/stereo FL,FR/128. Dynamic outputs derive from Config outputs/mixes;
three starter buses are not a cap. No arbitrary executable/server request.

Normal final install deadline2s; timeout is Pending and retains operation/guard.
Fence2s, then collective owned TERM1s/KILL1s. Forced/unreaped/unknown ownership
never implies scrub/StopComplete. Store lock remains on uncertainty; existing
Store never reclaims it automatically. No new cloud execution or system SDK
eligibility is granted by this source prototype.

The exact Config wire digest is SHA256 over the compact serde Config bytes
transmitted as `snapshot.config`. It is not a second JSON canonicalization.
Stage and Install share the same operation ID; replay defense distinguishes
the action while binding Install to its exact completed Stage. Initial Ready
uses operation `startup` and the current epoch. Ready may arrive during Prepare
and cannot discard its reserved generation or pending operation.

Installed requires `ownerInstallTerminal:true` and `noLateInstall:true`, plus
all configured output commits. A physical switch alone is not Installed. Fence
uses the exact coordinator control target, including a pending Stage ticket
even if a switch or complete Installed reply has raced ahead. A partial switch
is represented by mandatory-nullable `switchEvidence`; complete `switched` is
separate. Both nullable keys must be present. A known late complete Installed
cannot be omitted from the later fence receipt.

The fence reply uses the NEW epoch and the Fence operation ID. Shutdown/Stopped
use that same operation/current epoch. A valid Fenced receipt permits releasing
the canonical writer guard but is not StopComplete. StopComplete requires both
actual registered children normally reaped after the fence; forced termination
or unknown teardown remains failed and does not claim scrub. Explicit Prepare
then starts fresh private children and namespace under the same runtime nonce
and current epoch. This prototype requires Stop between R1 and R2; it does not
claim in-place endpoint handoff. Link identities must be reacquired.

Default lifecycle status and its typed clients remain unavailable/false/null,
even when the separate private protocol is enabled. The pinned SDK descriptor
is an explicit experimental test prerequisite, not a general minimum-version
or backend-availability decision. PR90/115 and historical native cause holds
remain separate; portable owner doubles do not qualify native processing.

Private status has optional closed `privateCore` connection metadata:
`{runtimeDirectory,configDirectory,remote:"mixyard-private"}`. Both directories
are the actual owned spawn namespace (they may be equal), never a guessed
default/personal server. Metadata is absent before child registration and
cleared on retirement; explicit restart returns a new namespace. This grants
no identity, format or admission proof. Ordinary recorders use the separately
owned `recorder.conf` client config; the companion uses client.conf, and the
daemon uses private.conf. Only the recorder client config adds its public
adapter factory, not the six-module daemon or native mixer graph.

The concrete unrun product acceptance and command sequence is in
[product-flow.md](product-flow.md).

Review repair: the inherited cancellation transport is the anonymous connected
AF_UNIX SOCK_STREAM pair created by the real Rust launcher, not a FIFO. Native
startup validates that exact socket kind, anonymous addresses and same-owner
parent peer credentials before reading the closed nonce/epoch wire.

Before Prepare, Link or Stage, the supervisor sends a bounded priority
`reserve {actionKind,reservedGeneration,requestDigest}` using the SAME operation
ID. requestDigest is SHA256 of the exact normal CompanionRequest bytes without
its newline. Native returns matching `reserved {reservedGeneration,requestDigest}`
and retains the exact control tuple/digest before normal bytes can execute.
Normal mutation dispatch requires BOTH this acknowledgement and the current
registered child's Ready. Reserve is inert control metadata and may precede
Ready; it creates no native graph/host. One bounded reservation plus one Fence
may share the priority writer, preserving their socket order under backpressure.

Stop can therefore fence a pending Prepare/Link whose normal bytes were never
consumed. It never manufactures a reservation from the expected Stop tuple.
After the epoch/owner barrier, only the exact canceled normal digest (or exact
known staged Install token) may drain inertly; all other stale requests reject.
A repeated Stop using the now-current tuple returns Busy without retransmission
or resetting the first operation's deadline. Startup remains bounded by the
existing 2s budget and cannot dispatch a deferred mutation after Stop.

firstProcessedFrame and nativeSwitchFrame are NATIVE positions; engineFirstFrame
is rebased zero. The native production helper checks their mapping. Full graph
formats remain mandatory at Stage/Install; ongoing engine/source readiness
checks source-side formats only. A bus or engine-output Format loss closes that
destination, while source input corruption/format/clock faults remain global.

The startup budget starts before owned child spawning, including daemon socket
publication; Ready and Reserved waits do not reset any peer/operation deadline.
An expired startup queues the same priority fence before a late Ready can be
consumed. These sequencing checks are portable owner-double evidence only.
