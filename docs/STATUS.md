# Integrated development status

Assembled 2026-10-14 from merged [#9](https://github.com/MGRAFF2006/mixyard/pull/9), exact `origin/main` base `26a096a2aa5045d6d7b0f9953e3d692f546978c1`. Four normal, non-rewriting merges add the reviewed runtime/diagnostics/profiles/native-desktop slices. The original seven components and their history remain present. This integration needs its own independent reviewer, different fixer/response checker, green CI, and an extra new final review for its exact HEAD. Only root may merge the consolidation into main, after no actionable findings and honest merge confidence strictly above 8/10.

## Exact reviewed imports

| Component / source PR | Origin branch (`feat/…`) | Approved imported head |
| --- | --- | --- |
| [Foundation #1](https://github.com/MGRAFF2006/mixyard/pull/1) | `reviewed-foundation` | `5aaf1d8d95c1be211f40f4d9789b04973da63e91` |
| [Qualification #2](https://github.com/MGRAFF2006/mixyard/pull/2) | `qualification-framework` | `fa99a9d8fbdb291ea72b7f594ec4e8c5e22c4e78` |
| [macOS #3](https://github.com/MGRAFF2006/mixyard/pull/3) | `macos-coreaudio-probe` | `301dcc6805286ca000ae35a065183f43783c20cd` |
| [Linux #4](https://github.com/MGRAFF2006/mixyard/pull/4) | `linux-pipewire-prototype` | `ff5f47929b798fe3275daaeec055ba2180ebff92` |
| [Windows #5](https://github.com/MGRAFF2006/mixyard/pull/5) | `windows-wasapi-probe` | `91d83499aa8553702c9c84a0b447e1bc70fd341f` |
| [Core policy #6](https://github.com/MGRAFF2006/mixyard/pull/6) | `dynamic-routing-policy` | `cf60301d6e55fb0aa384bb3c8b0c9387bf8a9c4d` |
| [Desktop/tray #7](https://github.com/MGRAFF2006/mixyard/pull/7) | `desktop-app-tray` | `089d271fc81cf1a7e5d61df83893a67cda0ab856` |
| [Runtime #8](https://github.com/MGRAFF2006/mixyard/pull/8) | `headless-runtime-ipc` | `0756cfb8cb69f24aa84565befdcb090a07300e84` |
| [Diagnostics #10](https://github.com/MGRAFF2006/mixyard/pull/10) | `routing-diagnostics` | `3c13da018780e3a3b7edefb09c00e34953efe953` |
| [Profiles #11](https://github.com/MGRAFF2006/mixyard/pull/11) | `profiles-preview-undo` | `2b61440ec3cf299359af8db8bef185289ac3a54c` |
| [Native desktop #12](https://github.com/MGRAFF2006/mixyard/pull/12) | `desktop-native-qualification` | `1207db55eb501967a776d473eb76e691fa0d60ef` |

All four remote heads were independently fetched and matched before merge. Their owner-managed independent reviews, different fix/response checks and applicable source CI were complete. Root's extra source-only finals reported no findings: runtime 8.7/10, diagnostics 9/10 (84 extra CLI invocations), profiles 9/10 (15 focused tests plus hosted storage). #9 had an extra final 9/10 and green CI before merging. Source scores do not approve this combined head.

After assembly, Git blobs and modes were checked against the pins for new modules/tests/docs, runtime manifest/lock/binary/workflow, and the complete desktop/native-evidence subtrees. Existing adapters, core policy/tests/fixtures and qualification data remain unchanged. Integration resolutions only combine library exports and CLI dispatch/help/error text. Root README/STATUS explain the assembled checkout; the inherited qualification workflow adds `tests/**` triggers and two Cargo jobs. The nested desktop Cargo workspace stays isolated with its own unchanged lockfile.

Native private-client #13 is **excluded**: no proof code, client/server evidence paths or audio enablement are imported.

## Compatibility and evidence

| Present component | Runnable evidence / compatibility boundary | First complete workflow gate |
| --- | --- | --- |
| [Core](../src/main.rs) | Schema-2 config validation and routing/media plans against simulated inventories; no GUI dependencies | No audio execution or native dispatch |
| [Qualification](../verification/README.md) | Independent CLI policy oracle and synthetic PCM analyzer; shared core fixtures now present | Synthetic PCM does not prove delivery to a consumer |
| [Linux](../adapters/linux/README.md) | Separate adapter config/schema; read-only discovery/health and offline PipeWire command plans | External mutation disabled; not connected to Rust policy; hostile relinking safety unproven |
| [Windows](../adapters/windows/README.md) | WASAPI discovery/explicit selected capture probe plus portable queue/WAV model | Virtual microphone injection unimplemented/unqualified; no mixer transport |
| [macOS](../adapters/macos/README.md) | Core Audio discovery/explicit selected capture probe plus portable buffer/writer checks | Virtual microphone injection unimplemented/unqualified; no qualified HAL transport |
| [Desktop](../apps/desktop/README.md) | Disconnected Vue UI, labelled in-memory demo, isolated Tauri shell; reviewed three-OS release compilation and scoped Linux webview checks | No runtime GUI bridge; physical tray/menu/Dock, Windows/macOS GUI, assistive technology and broader performance remain blocked |
| [Runtime](runtime.md) | Real foreground-process local IPC: status, events, shutdown; protected Unix sockets / Windows named pipes | `backendAvailable: false`; desired/audio/applied revisions null; audio mutations unavailable |
| [Diagnostics](diagnostics.md) | `inspect` / `explain` trace all configured branches through the authoritative planner | Projected intent/planning only; capture, permissions, consumers, applied/live levels and native sharing unknown |
| [Profiles](profiles.md) | Local named desired configs, validated drafts, preview and fresh-revision undo; revision/locking/atomic replacement checks | Never applies audio; `confirmedWorkingSetup: null`; stored consent is not fresh native consent; trusted local filesystem/cooperating writers only |

**First complete source → mix → consumer/physical-output workflow: BLOCKED.** Real IPC/storage/native compilation do not prove audio quality, privacy under hostile relinking, actual consent or consumer delivery. Multi-output isolation/recovery, gap-free PCM and latency remain gates; Windows/macOS virtual input injection remains absent. Device-scoped native media lifecycle and hardware origin/dispatch ownership are unproved. Reviewed native desktop evidence is scoped to its recorded runners and software-rendered Linux interaction samples; it does not establish the remaining physical/GUI/accessibility/performance gates.

## Combined smoke (2026-10-14)

Host: verified `homelab-dev`, headless Linux. Named tmux session: `mixyard-control-integration-check`. Reused the relinquished runtime target cache after verifying identical root manifest/lock and Rust 1.99.0; no concurrent cache writer. Cargo jobs=2; at least 10 GiB free (56 GiB before / 55 GiB after checks). Logs/caches/builds remain local and uncommitted. No host SDK installs, audio devices, services/defaults, real mic or operator session bus used.

| Check | Result / boundary |
| --- | --- |
| Locked offline `cargo test --lib --bins --test runtime_lifecycle --test diagnostics_cli --test profiles_cli` | PASS: 59 tests (37 library, 2 policy CLI, 12 diagnostics CLI, 3 profiles CLI, 5 Unix runtime lifecycle); real IPC and disposable storage, no audio |
| Locked offline build of both root binaries | PASS |
| Existing `policy_check.py` / `policy_selfcheck.py` | PASS: 10 / 22 independent checks; unchanged simulated fixtures/policy |
| Combined CLI smoke | PASS: all help/command families, original schema-2 plans, inspect/explain equivalence, no-write profile preview, IPC-v1 usage errors, unavailable audio mutations |
| Windows GNU / macOS ARM `cargo check --locked --offline --all-targets` | PASS: compilation only, not execution on those OSes |
| Frontend frozen offline install, `pnpm check`, `pnpm build` | PASS: nine tests, strict types, production build |
| Root/nested Rust formatting, shell/Python syntax, workflow YAML/trigger/jobs checks, whitespace | PASS |
| Git blob/mode audit | PASS: 95 protected files match the baseline/approved source overlays; all four pins are ancestors; complete expected file set |
| Local native desktop/adapter builds, physical GUI, capture/routing, services/defaults | NOT RUN: fresh native/IPC/storage execution belongs to hosted CI; audio/physical qualification remains blocked |

## Hosted integration gates

The [qualification workflow](../.github/workflows/qualification.yml) runs core/store/CLI tests on three OSes plus unchanged adapter builds/models and the independent policy oracle. [Runtime CI](../.github/workflows/runtime.yml) runs real three-OS lifecycle/endpoint checks, including Windows kernel DACL behavior. [Frontend CI](../.github/workflows/desktop.yml) runs the nine model tests, types and static build. [Native desktop CI](../.github/workflows/desktop-native.yml) compiles isolated release shells on three OSes and runs Linux webview checks in an owned hosted display.

These workflows must pass for the integration PR; historical source checks and local passes do not predeclare its CI. Compiler/SDK failures fail the build and never count as audio passes. CI owns fresh three-OS native/IPC/storage execution; local cross-target checks only establish compilation. No source PR or consolidation PR is merged by this owner.
