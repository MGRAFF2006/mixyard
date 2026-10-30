# Persistent private native companion

`companion.cpp` implements the offline `owned-private-generated-v1` companion.
It connects only to the parent's owned `mixyard-private` core, runs until an
explicit shutdown or authority EOF, and uses the actual `libmixyard_host.a` ABI.
There is no scenario timer, copied planner, duplicate Processor64, device
capture, default-server fallback, conversion, or resampling.

The wire authority is the parent's
`src/runtime/private_audio/protocol.rs`, including the receipt changes in
`d5b8cb0bcf7f0a88739ece8ec69c5ba4872dfc0f`. `protocol-v1.md` is parent-owned.
Requests and replies are newline JSON, at most 1 MiB each. Unknown keys,
duplicate decoded keys, wrong nonce/epoch, reused action/operation pairs and
unsupported actions fail closed. Stage and Install intentionally share one
operation ID. Fence and Shutdown share the current fence operation ID.

## Native behavior

Prepare binds **every** descriptor generator to a canonical Config source ID,
sorts sources and outputs into canonical planner order, allocates stable state,
and publishes a generator, an engine and one `Audio/Source` bus per output.
Bus IDs, mix IDs and display labels come from Config. Descriptor budgets govern
counts; the three historical fixtures are not a cap. `markerPeriod:0` disables
markers and `frameLimit:0` means infinite input. A finite source closes before
a quantum would exceed its limit. A marker writes signed stereo zero at that
source's generated-frame period. Source EOF or invalid admitted input closes
the generation and clears host histories and owned source copies; it never
automatically reopens that generation.

Link uses the observed private core's exact node/port IDs **and both serials**.
It waits for an actual ACTIVE native link and negotiated f64-le, 48000 Hz,
FL/FR stereo, 128-frame format. Labels and synthetic Config selectors are
policy/display metadata, never application/process authentication. Inventory
evidence remains `simulated`; native byte transport does not promote lifecycle
availability or physical/application capture claims.

Stage requires all links, exact formats, fresh source headers and one observed
SPA graph clock. A dedicated owned pthread constructs the actual host and checks
the compiled `source_info`/`output_info` against every binding. Control remains
available while the builder runs. Graph construction checks the priority socket
between native construction steps. Config SHA256 covers the **exact transmitted
compact serde Config bytes**, not a C++ reserialization. The request digest is
the exact bytes supplied to `mx_host_create`. The endpoint-set digest covers a
compact JSON array in canonical order: generated endpoint objects, followed by
`{outputId,published,consumer,format}` objects for each output.

Install only publishes a retained ticket. The checked exclusive DataLoop owner
switches on a fresh engine callback, stamps the actual admitted Config revision,
and rebases native frames to engine frame zero. `Installed` requires an actual
first process, each output bus's successful native commit and a subsequent owner
terminal barrier. Receipts are scalar/atomic publications; JSON is serialized
on control. All native input borrows are validated, copied and returned in the
same callback, including errors. Borrowed host output is copied immediately
and never retained. Callback code allocates/frees nothing and takes no locks,
waits, JSON, logging or I/O. Wrong owner is fatal. Destination failure is local
and cannot create a successful per-output install receipt.

Fence uses the separate priority socket even during Stage. It closes admission,
advances the epoch and queues a nonblocking exclusive-owner retirement barrier.
The exact retained pending control tuple remains stoppable after a physical
switch or queued Installed reply; the exact settled control tuple is also
accepted after that receipt was emitted. No other tuple is accepted. A pending
abort distinguishes canceled-before-switch from aborted-after-switch; the latter
has scalar `switchEvidence`, and a full `switched` receipt only when all commits
actually occurred. A settled stop is no-pending and can include its already
completed Installed receipt, with null switchEvidence.

Before Fenced, the builder must retire, the owner must revoke/scrub scratch and
host state, every live registered SDK buffer must be borrowed and zeroed, native
links/filters must be withdrawn, and a second owner barrier must acknowledge
withdrawal. Missing/failed queue returns, untracked registrations, lost SDK
ownership, or a buffer removed without its zero proof produce failed-held or
leave the proof pending. The companion does **not** turn a timeout, signal,
forced exit or reap into scrub proof. Shutdown requires the proven current
Fence operation and emits Stopped only after actual context destruction. The
parent still owns daemon shutdown/reaping and collective StopComplete. Copies
already held by an external recorder cannot be recalled.

This first slice requires successful PrivateStop and a **fresh private core and
companion** between Config R1 and R2, under the same product-runtime nonce and
current epoch. It rejects Prepare while the current child owns a generation;
there is no implicit hot replacement or restart. Startup accepts
`--control-epoch E` (default 1 only for an initial child):

```text
mixyard-private-companion --descriptor /absolute/descriptor.json \
  --instance-nonce <64-lowercase-hex> --cancel-fd <inherited-pipe-fd> \
  --control-epoch <current-positive-epoch>
```

## Explicit environment and offline build

The parent validates the descriptor, executable and complete prefix-manifest
file hashes before spawn. The companion additionally checks descriptor scope,
the pinned release commit `8b43ed5c910e892d53de5a96aa01e9280a5b6650`, exact
SDK version `1.4.8`, the manifest hash/shape, loaded PipeWire library path/version,
core identity/version, allowed private modules, and endpoint/format identities.
This is the known experimental prefix scope, **not** general SDK eligibility.
The #90/#115 historical holds, frozen references and retained WAVs are unchanged.

Runtime must provide:

- `PIPEWIRE_REMOTE=mixyard-private`.
- Owned 0700 `PIPEWIRE_RUNTIME_DIR` and `PIPEWIRE_CONFIG_DIR` (they may be the
  same owned private instance directory).
- `PIPEWIRE_CONFIG_NAME=client.conf` and an owned private regular client file.
- `PIPEWIRE_MODULE_DIR`, `SPA_PLUGIN_DIR` and `LD_LIBRARY_PATH`, each one
  canonical prefix-contained directory, without fallback search entries.
- A reviewed client config with protocol-native/client-node/metadata and the
  support SPA library. No adapter module is added by this companion.

Run `build.sh` **only in an independently authorized SDK build environment**
with existing reviewed artifacts. It never installs/downloads anything,
builds Rust, starts a daemon, runs the companion or accesses a shared target:

```sh
MIXYARD_PRIVATE_PREFIX=/absolute/reviewed-prefix \
MIXYARD_HOST_LIBRARY=/absolute/libmixyard_host.a \
MIXYARD_HOST_SHA256=<reviewed-archive-sha256> \
MIXYARD_JSON_C_PC_DIR=/absolute/reviewed-json-c-pkgconfig \
MIXYARD_PRIVATE_BUILD=/dev/shm/owned-fresh-build \
bash adapters/linux/private-audio/build.sh
```

The executable is restricted to little-endian hosts and uses public PipeWire
filters/registry/port/link/DataLoop APIs plus json-c. Pkg-config's native search
path is replaced with the explicitly supplied prefix. Parent owns the actual
Rust host build and its exclusive target. Source forecasts reserve root 2 GiB,
TMP 2 GiB, SHM 10 GiB + 16 MiB and cap the native area at 384 MiB. The helper guard counts only these source
subtrees; the area lead separately accounts for isolated worktrees, retained
evidence and the serialized portable Rust target.
Build/helper outputs use owned SHM only and must total at most 2 MiB; an oversized
build fails and preserves its output for inspection. The recipe's final linked
size is **unmeasured**, so the forecast needs checking in the authorized SDK
environment. Resident reservations cover bounded metadata, requested native
pools, scratch and explicit host allocation budgets; they are not an observed
RSS measurement of an unexecuted SDK process.

## Verification and remaining execution gaps

`bash adapters/linux/private-audio/test.sh --owner-only` compiles/runs the
production portable C helpers, without any PipeWire headers or SDK calls. It
covers actual revision R1/R2 stamps, native frame rebase, clock/gap rejection,
generation replay, both exact control views of a pending/settled stop, stale
ticket rejection, SHA256 vectors, and strict input validation. API-double
failure cases verify each successful/unavailable/malformed/canceled borrow is
returned exactly once in the call and failed queue return scrubs its owned copy.
The full `test.sh` also exercises the real json-c parser's duplicate/unknown
keys, bounded framing and exact nested Config bytes.

At source delivery, C helpers and shell syntax checks passed. The full JSON
helper was attempted and could not compile because this host lacks json-c
development files. It remains UNRUN. **The actual companion/PipeWire SDK
compile, native graph execution, standard-consumer recordings, allocation and
borrow instrumentation, owner/fence races against the real SDK, dynamic fourth
output, SDK scrub coverage and final linked footprint are UNRUN locally.** No
passing helper is presented as native qualification. Those checks require the
explicit reviewed prefix/static host and a separately authorized isolated run.

The separate product launcher/private CLI/Store ownership flow and still-unrun
R1/R2 ordinary-recorder acceptance are documented in
[product-flow.md](product-flow.md). Parent private status exposes only the
registered owned connection paths; this is not native admission/provenance.

The six findings in review5713498177 are addressed by exact UnixStream fd3
validation, native/engine coordinate separation, received priority reservations,
Ready-gated dispatch, non-retransmitting Busy on repeated Stop, and source-side
ongoing formats with destination-local format failure. These are source/portable
repairs, not executed SDK-handler or four-recorder qualification. The corrected
compile applicability/discovery proposal is [compile-plan.json](compile-plan.json);
the prior compile-only budget stopped before every compiler at its wrong
historical checkout path and is spent. No new cloud execution is implied.
