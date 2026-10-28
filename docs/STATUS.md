# Integrated development status

## Channel controls and measured signal delay (2026-10-28)

The merged channel DSP core (#37) supports explicit per-effect channel selection
and signed matrices. The browser adds beginner channel operations, everyday
selection/parameter controls and expert coefficients around one complete chain.
View changes keep custom settings; renames update the same lane's references.
Removing referenced lanes or implicitly changing matrix format is blocked.

An isolated real-engine impulse probe measures per-output first-response and
strongest-peak positions in an explicit window. It distinguishes no response,
threshold, dry/wet branches and incomplete/infinite tails. Explicit compensation
appends pure per-channel sample delays, then a second probe verifies alignment of
that observed feature. CPU throughput, filter group delay, native output latency
and clock synchronization are separate. See [delay semantics](latency.md).

Local checks passed: 98 focused Rust tests, 23 Node tests, 17 contract-browser
flows and nine actual browser/service/store flows, including operation output,
rename/reference preservation, measured unequal paths, compensation/re-measure,
measurement export and a 390 px layout. Screenshots were inspected. Independent
reviews and hosted checks remain merge gates. This is offline chain alignment;
live source/mix/output attachment and physical delivery remain unqualified.

Separate network timing work measures authenticated application RTT and queue
sample occupancy. Its review/CI and service/dashboard integration remain separate;
one-way delay, consumer prebuffer, device clocks and physical end-to-end latency
must not be inferred from RTT or queue capacity.

## DSP processing workbench (2026-10-28)

The browser Processing workspace edits ordered Float64 chains with gain/polarity,
eight biquad variants, delay, compressor, expander and full direct-FIR coefficients.
Import validation compiles without rendering. Explicit previews execute the actual
DSP engine with independent channel state and return full requested block output,
peak/RMS/over-full-scale counts and measured processing time versus block duration.
Buffering latency, intentional delay/tails and unmeasured group/route latency are
separate. Measured overruns warn and leave editing/rendering enabled; no effect is
silently removed or quality reduced.

Two additional real-service browser flows passed: signed-zero/low-bit Float64
bypass plus repaired extreme dynamics, and compile-only imports/full FIR export,
actual over-budget warning, 390 px layout and offline recovery. They run alongside
the four existing routing/store/runtime flows. Three added HTTP checks cover real
DSP output, reset/measurement independence, invalid/resource/overflow rejection
and service recovery. Fifteen Node and twelve contract-browser checks pass. These
are offline preview results; DSP does not attach to source/mix/output callbacks,
play audio or qualify native scheduling. Independent reviews and hosted checks
still govern merge readiness.

The original PCM foundation #28, Float64/codecs #31, browser console #32 and secure
raw network foundation #30 are merged. Wider native formats/layouts, actual
consumer delivery, clocks, recovery/quality/endurance and compression remain
required work. DSP #33 additionally tracks native insertion, automation, channel
tools, calibrated loudness, advanced dynamics/convolution and plugin hosting.

## Web-first milestone (2026-10-28)

The local Rust `mixyard-web` service and standalone browser console now work
together without a native desktop SDK. `scripts/run-web.sh` builds and serves the
console on a protected loopback URL. A fresh store starts empty. Explicit imports
or the labelled example feed a source/mix matrix, independent output levels,
persisted drafts, named save/rename/load profiles, revision conflicts and undo.
Inspection delegates to the core; no route is applied to audio.

Actual production assets with the Rust service passed four isolated browser
flows: independent gain/mute edits surviving browser/service restart, real
profile operations and competing windows, diagnostics/import errors/offline
recovery with 390 px keyboard operation, and the same explicitly started runtime
surviving UI/service closure. The web service has ten real HTTP tests, including
admission of inspection expansion before allocation. Both reviewer-reproduced
memory-exhaustion inputs now reject early with subsequent reads succeeding;
the selected-observation fanout case was exercised with four simultaneous
requests under a 512 MiB process address-space limit. Fresh independent final
review and hosted checks still govern merge readiness.

These results establish local configuration/service behavior. Native source
capture, application/physical consumer delivery, quality, drift, recovery,
endurance and complete source-format preservation remain required and unproved.
Network and wider-precision core foundations proceed in separate PRs; see
[#29](https://github.com/MGRAFF2006/mixyard/issues/29). Historical evidence and
remaining platform limitations follow below.

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

## Desktop lifecycle connection slice (2026-10-14)

Based on merged main `8fbd97c0f64e442643476f3d7841353f8b7d56cc`, the desktop now explicitly connects read-only to an already-running owned runtime. A typed core client reuses the existing transport; the separate desktop workspace depends on the GUI-free root library. One worker owns one subscription, with bounded startup/partial-frame handling and generation checks after Disconnect/reconnect. No server, CLI, profile, diagnostics or audio semantics change.

The [scoped native manifest](../verification/desktop-native/linux-container-evidence.json) records actual Linux WebKit Connect and the same runtime PID/status surviving three UI exit paths, followed by explicit CLI shutdown/reaping. Late old-generation responses cannot restore a cancelled connection. Eight new client tests, five existing lifecycle tests, ten frontend model checks and a locked Linux native build passed locally. The hosted three-OS gates still qualify this slice's exact HEAD. These checks establish lifecycle IPC survival with `backendAvailable: false` and null audio/applied/desired revisions; audio and the physical/platform GUI gates remain blocked. The integration smoke below is historical evidence for the merged baseline.

## Compatibility and evidence

The desktop additionally imports bounded schema-2 config/inventory content for
read-only **Why silent / What is sharing** inspection through the unchanged core
`diagnostics::inspect`. Revisions/platform/declared evidence and stale/imported
status are explicit; every intended path is retained, while accepted dry-run
coefficients remain separate from unknown applied/permission/consumer/live state.
The lifecycle connection supplies no routing snapshots. No protocol, runtime,
profile, core policy, discovery adapter or production-apply behavior changes.
See the focused [inspection checks and evidence limits](../verification/desktop-inspection/README.md).

The separate **Named local profiles** panel explicitly refreshes the current-user
default local store, shows stable-ID saved configs and calls `profiles::preview`
against a fresh current draft plus imported schema-2 inventory. Stale store
revisions fail closed; no GUI writes, repair, consent or activation. The core
adds only opt-in bounded reading through its existing path/parser/validation;
CLI `Store::read` behavior is unchanged. See [profile checks and evidence](../verification/desktop-profiles/README.md).

| Present component | Runnable evidence / compatibility boundary | First complete workflow gate |
| --- | --- | --- |
| [Core](../src/main.rs) | Schema-2 config validation and routing/media plans against simulated inventories; no GUI dependencies | No audio execution or native dispatch |
| [Qualification](../verification/README.md) | Independent CLI policy oracle and synthetic PCM analyzer; shared core fixtures now present | Synthetic PCM does not prove delivery to a consumer |
| [Linux](../adapters/linux/README.md) | Separate adapter config/schema; read-only discovery/health and offline PipeWire command plans | External mutation disabled; not connected to Rust policy; hostile relinking safety unproven |
| [Windows](../adapters/windows/README.md) | WASAPI discovery/explicit selected capture probe plus portable queue/WAV model | Virtual microphone injection unimplemented/unqualified; no mixer transport |
| [macOS](../adapters/macos/README.md) | Core Audio discovery/explicit selected capture probe plus portable buffer/writer checks | Virtual microphone injection unimplemented/unqualified; no qualified HAL transport |
| [Desktop](../apps/desktop/README.md) | Explicit read-only local lifecycle connection, labelled in-memory demo, separate Tauri shell; scoped Linux webview/runtime survival proof and three-OS compile gates | Audio unavailable; physical tray/menu/Dock, Windows/macOS GUI, assistive technology and broader performance remain blocked |
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
