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

The console edits routing intent only: it cannot apply audio, start/stop the
runtime, discover hardware, capture sound or show live levels. Runtime status
is the last read-only snapshot; Refresh updates it. Imported inventory claims,
including native-discovery, remain unverified. Example identities and simulated
inventory stay labelled simulated.

Inventory and unsaved edits are held only in browser memory; export before
closing to keep them. Config/profile persistence goes through the service, not
localStorage. The browser warns on navigation with unsaved config edits, but it
cannot guarantee recovery after a crash. Inventory is not part of profiles.

Imports are bounded to 128 KiB; API responses are bounded to 2 MiB and requests
time out after 12 seconds. Revisions must fit JavaScript's exact safe-integer
range; unsupported schemas and malformed responses are rejected. Core policy
validation decides route correctness, headroom and monitoring constraints.
The UI can add sends and mixes; source/output identity creation, policy editing
and removal use imported config. Arbitrary imported mixes render horizontally;
on narrow screens the matrix scrolls in its own region with sticky source labels.
