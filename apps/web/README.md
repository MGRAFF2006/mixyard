# Mixyard local web console

Standalone Vue 3 / TypeScript / Vite frontend. No Tauri or native SDK is needed.
The Rust `mixyard-web` server serves the production files and same-origin API.

From the repository root (Node 24.13.1 and pnpm 11.10.0, matching desktop tooling):

```sh
pnpm --dir apps/web install --frozen-lockfile
pnpm --dir apps/web build
cargo run --bin mixyard-web -- --assets apps/web/dist
```

Open `http://127.0.0.1:8384`. The service chooses the profile directory and runtime
instance; use its `--store`, `--instance` and `--port` flags for isolated setups.
The root `scripts/run-web.sh` launcher performs the install/build/run sequence.
Vite development (`pnpm --dir apps/web dev`) is frontend-only on 127.0.0.1:5174;
it intentionally has no cross-origin API proxy. Use the Rust server for actual
persistence, validation and inspection.

## Workflow

- A fresh store opens empty. Import schema-2 config, choose a saved profile, or
  explicitly **Load simulated example**. Nothing is seeded or saved on startup.
- Sources are rows, named mixes are columns. Each configured send owns its gain
  and mute; mix masters and output gains/mutes are separate. Missing sends may
  be added at zero gain. Add/rename mixes without changing their stable IDs.
- **Save draft** persists through the API and adopts the returned config revision.
  **Refresh** reads fresh status/store revisions and preserves unsaved local edits.
  **Reload stored draft** explicitly discards local changes after confirmation.
- In **Profiles**, save a new named copy, replace a selected profile, rename it,
  or load it into the stored draft. Saving a profile alone leaves the stored
  draft unchanged. **Undo stored draft** uses the store's previous draft.
- A conflict or uncertain write pauses further writes. Refresh, review the
  stored draft, and explicitly choose save or reload. Writes never auto-retry.
  Replacements and page navigation warn about unsaved local draft changes.
- Config imports require shape checks and authoritative `/api/validate` success
  before replacement. Export keeps all raw fields, including media policies,
  selectors, sample rates, groups and configured monitoring consent. This UI
  never rebuilds config from the visible controls alone.
- Import/export inventory separately. **Inspect** sends the local config and
  imported inventory to `/api/inspect`; it shows planner failures, reason codes
  and the full authoritative report. Edits invalidate prior results.

## Offline processing workbench

**Processing** is a separate in-browser Float64 chain draft, never part of routing
profiles. Export chain to keep it; reload discards it. Set any positive u32 sample
rate and explicit ordered unique channel IDs, then add, enable, reorder or remove
gain/polarity, eight biquad variants, delay, compressor, expander, direct FIR and
explicit channel matrices. Every non-matrix effect can target selected channel IDs.
FIR coefficients are kept in full, without normalization or quality reduction.
Starter presets explicitly replace the chain. Snapshot/reset/bypass transitions
can click; no automation or smoothing is provided.

Chain file/paste imports are shape-checked and compiled by `/api/dsp/validate`
before replacement. Errors leave the edited chain intact. Raw fields and channel
identities survive edits/export; Rust decides which fields and parameter domains
are supported. DSP JSON uses finite Float64 semantics, including signed zero and
large finite integers, independently of the safe-integer routing revisions.

Only **Render and measure** calls `/api/dsp/preview`, using the shared same-origin
session. Choose impulse, constant, sine or exact custom interleaved samples, block
frames and timing repetitions. Every repetition starts reset; output is the first
isolated block, not a complete tail render. All edits invalidate prior output and
timing. No preview plays audio or attaches effects to a native route.

The report separates observed peak/RMS/sample headroom, added buffering latency,
explicit delay, tail bounds and measured processing time. Filter/IR group delay
and route/device/network latency remain unmeasured. Abstract work/memory estimates
are not speed. Slower-than-budget measurements warn about live delivery and keep
all editing/render controls usable; a synthetic within-budget observation does
not certify native realtime operation. Unmeasured timings stay unknown. Endpoint
resource limits may reject large previews; full offline rendering remains
available through the DSP engine/example without removing slow effects.

Beginner shortcuts append level/polarity, explicit-pair balance, swap/copy and
independent delay stages. Everyday controls expose per-effect channel selection
and all existing parameters; expert detail exposes matrix contributions and raw
JSON. Switching views preserves the complete chain, including custom matrices.
Renames update same-identity references; referenced removal and incomplete matrix
format edits are blocked. Millisecond delays require exact whole samples or an
explicit quantization choice; fractional delay is not implemented.

**Measure signal delay** runs an actual isolated impulse-response probe through
`/api/dsp/delay`. It shows per-output onset and peak positions, threshold/amplitude
and window context. No response is unknown, not zero delay. Explicitly append a
common-reference alignment plan, then measure again; no native output changes.
Incomplete windows cannot authorize strongest-peak alignment. Existing settings
are kept, stale work is discarded, and errors/offline states keep the chain.
Export the measurement with its exact request for reproducibility. See
[the full delay contract](../../docs/latency.md). Network/device timing and live
clock synchronization remain separate, unqualified boundaries.

## Verification

```sh
pnpm --dir apps/web check
pnpm --dir apps/web build
pnpm --dir apps/web exec playwright install chromium
pnpm --dir apps/web test:e2e
```

The focused Node tests cover independent levels, complete JSON preservation,
API failures, draft reopening, unsaved edits and revision conflicts. Isolated
Playwright tests exercise the actual Vue interface against an intercepted
contract API: example opt-in, send/output independence, profile save/rename/load/
undo, refresh/conflict recovery, import errors and narrow-screen keyboard use.
The DSP Node tests cover channel/effect order, complete raw-field and Float64
preservation, authoritative import failures, response validation and stale result
suppression. Mocked browser flows exercise all editor controls, exact request
payloads, compile-only imports, slow/unknown timing, invalid input, offline recovery
and narrow keyboard operation. Their fixture output/timing is not DSP evidence.
These tests do not prove Rust integration or disk persistence; the parent web
milestone verifies that combined path with the actual server.

The integrated checks run the built assets against actual Rust binaries,
disposable on-disk stores and an explicitly owned lifecycle runtime:

```sh
cargo build --locked --bins --jobs 2
pnpm --dir apps/web test:integration
```

They cover service/browser restart persistence, independent send/output edits,
profile operations, competing windows, real inspection, invalid imports, offline
recovery, 390 px keyboard operation and read-only runtime survival after UI exit.
Set `MIXYARD_WEB_BINARY` to use a binary outside the root `target/debug`; the CLI
and runtime binaries must be beside it. The harness stops only its own children
and removes its temporary store. CI retains desktop/mobile browser screenshots.

For an already-cached compatible Chromium, avoid another browser download:

```sh
PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH=/absolute/path/to/chrome-headless-shell \
  pnpm --dir apps/web test:e2e
```

## Actual limits

The routing workspace edits routing intent only: it cannot apply audio, start/stop the
runtime, discover hardware, capture sound or show live levels. Runtime status
is the last read-only snapshot; Refresh updates it. Imported inventory claims,
including native-discovery, remain unverified. Example identities and simulated
inventory stay labelled simulated.

Inventory and unsaved edits are held only in browser memory; export before
closing to keep them. Config/profile persistence goes through the service, not
localStorage. The browser warns on navigation with unsaved config edits, but it
cannot guarantee recovery after a crash. Inventory is not part of profiles.

Routing config/inventory imports are bounded to 128 KiB; API responses are bounded to 2 MiB and requests
time out after 12 seconds. Revisions must fit JavaScript's exact safe-integer
range; unsupported schemas and malformed responses are rejected. Core policy
validation decides route correctness, headroom and monitoring constraints.
The UI can add sends and mixes; source/output identity creation, policy editing
and removal use imported config. Arbitrary imported mixes render horizontally;
on narrow screens the matrix scrolls in its own region with sticky source labels.

## Situations workspace (format 1)

Situations edits one complete decision configuration in browser memory. Export
**full configuration** before closing: it includes graph, catalog, simulation
snapshot, monotonic clock, revision context and manual-override input. Switching
workspaces keeps the draft. It does not save graphs or change profiles/audio.
Activation and controller ownership are separate integration work.

Start with configurable headphones, explicit interface endpoint-group hardware monitoring, or
application-combination presets. Supply exact device/profile/source/mix/output
IDs; saved profile IDs and current routing identities are offered as suggestions.
No display-name-to-ID inference occurs. The monitor preset requires a complete
connected endpoint set equal to an explicitly listed group (contains-all plus
exact count), with a hardware-monitor declaration on a chosen listed member. A
single UID is a group of one. It suppresses only the chosen source-to-monitor
send; multiple endpoints are never inferred to be one physical box. Unknown gets its own configurable profile or
no-change terminal. Time windows compose provider-supplied timezone and civil
minutes; the browser does no fixed-offset conversion.

The same graph has recursive all/any/not conditions, every typed predicate,
three explicit branch links, profile/scoped-reduction actions and expert Request
JSON. Node renaming relinks incoming branches/root atomically; referenced nodes
cannot be deleted. Cycles, missing targets, invalid bounds/types and excessive
input are rejected. Lists and identities are never trimmed or truncated.
Changing a condition/operator/action type intentionally replaces its operands;
unrecognized raw fields remain attached for authoritative rejection. Ordinary
edits retain all other parameters and raw fields. Automation requests/exports
use finite Float64 JSON, preserving signed zero and low bits, with separate
safe-integer revision/time checks; legacy routing JSON is unchanged.

The directed diagram shows all three labeled branches, selected nodes and the
evaluated path. Its scroll region is keyboard-focusable; an equivalent node and
branch button list provides full keyboard navigation without dragging or color.
Simulation reasons link to deduplicated evidence by observationIndex. Core
traceTruncated is explicitly shown as an incomplete prefix; facts remain supplied
claims even when their evidence label says platform-observed. Every edit,
including invalid text, invalidates prior trace/validation; older asynchronous
responses cannot replace later edits.

The fixed integration boundary uses the existing same-origin Api/CSRF session:

- `POST /api/automation/validate {graph,catalog}` -> `{valid:true}`
- `POST /api/automation/evaluate <complete Request>` ->
  `{mode:"simulation",evaluation:<core Evaluation>}`
- `POST /api/automation/context {}` ->
  `{mode:"read-only-live-context",snapshot,nowMs,catalog,providers}`

Provider reports contain `{provider,status,reason}` with complete/partial/
unsupported/error status. Application mapping-domain IDs are displayed when reported. Unmapped
applications stay unknown; their absence never proves they are stopped.
Collection never changes simulation inputs until the
explicit copy action. Collection failure keeps local work and allows retry.
Imports run graph validation **and full-request simulation** before replacing
any active data. Services lacking these endpoints return a visible error;
editing/export remain available. No fabricated local evaluator substitutes for
the authoritative service.

Format-1 admission: 256 nodes, graph/condition depth 32, 512 catalog/fact entries,
256 items per set/list/override collection, 1024-byte text, 4096 admitted work
items, 1 MiB request. Rust enforces the complete work budget and reserved catalog
policy; frontend shape checks do not replace it. Trace has at most 256 entries
and a shared 256 KiB compact evidence budget.

`src/situations.test.ts` covers exact edits/presets, full-parameter preservation,
safe metadata, unknown-path expectations, import failure atomicity, stale-response
suppression and offline recovery. `tests/situations.spec.ts` exercises the real
Vue forms and accessible diagram against mocked API responses, including 390 px
keyboard use, shared evidence and bounded trace. These are frontend contract
checks, not actual Rust evaluator/provider or automatic activation evidence.
The core is PR #41; the service adapter and live-context collector are separate
dependencies under issue #40.


Copied mapped `application.running` sets remain explicitly partial in simulation
and exports: format 1 has no field carrying a provider's restricted mapping domain.
The original read-only provider report retains `applicationScope`. This prevents
an unmapped application's absence becoming false after copying or changing rules;
scoped live comparisons require the separate service adapter's domain check.
Integer metadata uses exact nonnegative decimal integer literals (no fractional or
exponent forms); predicate and fact numeric values retain finite Float64 precision.
Successful imports/context replacements rebuild form controls to discard obsolete
DOM validation errors. Simulation responses must agree with graph terminals,
actions, branches, manual override and shared observation identities; truncated
trace prefixes remain valid.
