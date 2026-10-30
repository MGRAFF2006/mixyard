# Shared prototype wire baseline

The parent-owned Rust definition is `src/runtime/private_audio/protocol.rs`.
All field names are camelCase, action/event/operation kind tags kebab-case.
Unknown keys/versions fail closed. This is owned-private-generated-v1 only;
legacy lifecycle status stays unavailable/false/null.

Public private endpoint: same-user0600 `private-audio.sock` in the owned0700
runtime instance directory. Requests8KiB/results64KiB, newline-delimited JSON.
Companion control: stdin/stdout newline JSON <=1MiB, with a separate priority
cancel pipe passed by `--cancel-fd`. No PCM on either channel. Native callbacks
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
  cancel pipe even when the mutator is occupied. Advance epoch, reject old
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
`--descriptor ABS --instance-nonce HEX64 --cancel-fd FD`.
The runtime supplies sanitized PIPEWIRE_RUNTIME_DIR/CONFIG_DIR and explicit
prefix library/module/plugin paths; remote is `mixyard-private`, never personal
or system. The descriptor and all qualified-file hashes are validated by the
runtime before spawn and observed SDK bindings remain point-scoped evidence.

Descriptor schemaVersion1/scope owned-private-generated-v1 has `sdkPrefix`,
`prefixManifest`, `prefixManifestSha256`, `companionExecutable`,
`companionSha256`, `sdkVersion` (experimental1.4.8), `sdkCommit`,
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
