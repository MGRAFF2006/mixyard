# Integrated development status

Assembled 2026-10-14 against bootstrap `main` (`1c7aedc672103d80f6f2f37391fbe98aff47f05e`). Seven normal merge commits preserve the approved source history. All initial scopes and five qualification-base integrations were approved; this consolidation still needs its own review. No main merge or GitHub default-branch change is authorized.

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

Every origin head was verified against its pin before any merge. Component Git blob IDs and file modes were compared against those heads after assembly: foundation documents, qualification/workflow, each adapter subtree, core manifest/lock/source/examples, desktop subtree/workflow. All match. The only integration-owned changes are root README and this new file; no component source or existing workflow edits.

## Compatibility and evidence

| Present component | Runnable evidence / compatibility boundary | First complete workflow gate |
| --- | --- | --- |
| [Core](../src/main.rs) | Schema-2 config validation and routing/media plans against simulated inventories; no GUI dependencies | No execution, native dispatch or persistent runtime |
| [Qualification](../verification/README.md) | Independent CLI policy oracle and synthetic PCM analyzer; shared core fixtures now present | Synthetic PCM does not prove delivery to a consumer |
| [Linux](../adapters/linux/README.md) | Separate adapter config/schema; read-only discovery/health and offline PipeWire command plans | External mutation disabled; not connected to Rust policy; hostile relinking safety unproven |
| [Windows](../adapters/windows/README.md) | WASAPI discovery/explicit selected capture probe plus portable queue/WAV model | Virtual microphone injection unimplemented/unqualified; no mixer transport |
| [macOS](../adapters/macos/README.md) | Core Audio discovery/explicit selected capture probe plus portable buffer/writer checks | Virtual microphone injection unimplemented/unqualified; no qualified HAL transport |
| [Desktop](../apps/desktop/README.md) | Disconnected Vue UI, labelled in-memory demo, independent Tauri tray shell/workspace | No runtime IPC integration or live mixer controls; native interaction/lifecycle not qualified |
| [Pending runtime #8](https://github.com/MGRAFF2006/mixyard/pull/8) | Excluded; independently being repaired/re-reviewed at `997bb3668e19ef25665675eb09bbbabcea980c00` when assembled | Import requires owner/root confirmation of exact reviewed head |

**First complete source → mix → consumer/physical-output workflow: BLOCKED.** Current private native proof reports PCM gaps and unproven hostile relinking. Those observations are supplied owner evidence, not a native experiment performed by this integration. Zero profiler errors or a build pass cannot certify gap-free PCM. Native consent, privacy under relinking, consumer delivery, multi-output isolation/recovery, and latency remain gates; Windows/macOS virtual input injection remains absent. Full device-scoped media lifecycle and hardware origin/dispatch ownership proof remain unexecuted.

## Combined smoke (2026-10-14)

Host: `homelab-dev` Linux, no audio/display used. Named tmux session: `mixyard-integrated-smoke`. Root Cargo cache: `target/integration`; local check/install artifacts: `build/integration`; reused by review/fix checks. Rust builds use jobs=2. Disk retained at least 10 GiB free (67 GiB observed before and after checks). Logs/artifacts are ignored local evidence, not recordings or release qualification.

| Check | Result | Scope |
| --- | --- | --- |
| `cargo test --locked --offline --package mixyard -j 2` | PASS: 27 tests (25 library + 2 CLI) | Headless policy model |
| Locked offline CLI build + `validate`, `plan`, `plan-media` on `examples/` | PASS | JSON fixtures; no audio or dispatch |
| `verification/policy_check.py` / `policy_selfcheck.py` against built CLI | PASS: 10 / 22 checks | Independent black-box policy and corruption oracle |
| `verification/selfcheck.py` | PASS: 100 checks | Synthetic analyzer only |
| Linux `--help` / `test_linux.py` | PASS: CLI help / 19 tests | Offline model/subprocess boundary; no PipeWire graph |
| Windows portable `model_check.cpp` (C++17, warnings as errors, pthread) | PASS | Buffer/packet/WAV model; CMake unavailable locally, compiled directly with installed C++ |
| `bash adapters/macos/check.sh` | PASS | Portable sanitizer buffer/writer and injected I/O faults; no Core Audio |
| Desktop locked install + `pnpm check` + `pnpm build` | PASS: typecheck, 9 tests, build | Typecheck, 9 fixture model tests, static frontend |
| Native macOS probe / Windows probe builds | UNAVAILABLE locally | Linux host lacks those SDKs; existing hosted CI builds now see all imported inputs |
| Native Tauri build / webview / tray run | SKIPPED | No host SDK installs or desktop session |
| Live capture, routing, services/default changes, native qualification | SKIPPED / BLOCKED | Explicitly outside this smoke; no fake native evidence |

Existing [qualification CI](../.github/workflows/qualification.yml) now sees all core/adapter files together and can run its conditional jobs; [desktop CI](../.github/workflows/desktop.yml) sees the standalone frontend. Hosted build outcomes must be checked on the integration PR; local passes do not predeclare CI passes. Native compiler/SDK failures must be investigated and never recorded as audio passes.
