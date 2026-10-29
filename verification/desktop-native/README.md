# Current shared-console qualification

The desktop now consumes `apps/web/src/App.vue`, replacing the former prototype
fixture mixer. The current hosted harness starts both an independent real runtime
and an independent `mixyard-web` process, using disposable storage and a numeric
loopback port. It exercises the actual native `console_request` transport, explicit
simulated example edits (stored intent only), native keyboard rendering, compact
resize/profile navigation, real service-backed draft/profile writes, cancelled
unsaved-draft close, and confirmed close.
The same-runtime kernel peer PID/CLI survival, late-generation subscription checks
and explicit CLI shutdown remain required. `MIXYARD_DESKTOP_NO_TRAY=1` exercises the
full-window fallback without a tray host. No extension is installed or assumed.

The wrapper now takes **five arguments**:

```sh
bash verification/desktop-native/run-linux.sh DESKTOP_BINARY RUNTIME_BINARY CLI_BINARY WEB_BINARY EVIDENCE_DIRECTORY
```

`WEB_BINARY` must be the separately built production `mixyard-web`, with the
current checkout's built `apps/desktop/dist` assets. The runtime connection form
is explicitly separate local lifecycle diagnostics and never changes the console's
management target. Editable console ranges are stored intent; the live audio mute
remains disabled even while lifecycle IPC is connected.

The workflow also runs std-only native HTTP boundary tests on all three OSes and
generates unsigned deb/AppImage, MSI/NSIS and app/DMG bundles with hosted SDKs.
`packages.py` fails if any declared format is absent, and fingerprints actual
artifacts. Installation, upgrades, rollback, autostart, signing/notarization,
physical desktop hosts, Windows/macOS GUI behavior and audio are unqualified.
The [desktop matrix](../../apps/desktop/README.md) records these boundaries.

**The 2026-10-14 results below are historical prototype evidence. They do not
qualify the shared console or the changed harness. Only a passing current-HEAD
`linux-webview.json` and current per-runner artifacts establish current checks.**

---

# Desktop native qualification

This component builds and executes the Tauri shell with an explicitly
harness-started root `mixyard-runtime serve` child and the real root CLI.
The app never starts or shuts down that child; there is no sidecar or audio backend.
`.github/workflows/desktop-native.yml` builds locked release binaries on Ubuntu 24.04, Windows 2022 and macOS 14 with
Rust 1.99.0, Node 24.13.1 and pnpm 11.10.0. Hosted SDK packages are ephemeral.
Actual image/OS, architecture, SDK/package versions, command logs, source hashes,
binary SHA-256 and failures are uploaded in per-runner/HEAD evidence artifacts.
Binaries are separate, explicitly labelled artifacts; no build-cache upload.
Runner labels are not exact OS/SDK versions: use the recorded inventory.

The second isolated Linux container run on 2026-10-14 passed. In that container's
PID namespace, runtime PID 57 survived Alt+F4 (UI PID 67), Close window (338)
and Quit UI (406), with matching kernel peer PID and real CLI status after each
exit. A fourth UI (474) observed stopping; explicit CLI shutdown reaped runtime
57 with exit 0. The backend stayed unavailable and revisions stayed null.

Late snapshot testing observed no stale publication; the old stopping socket was
already closed, so no delivered-late-stopping-frame claim is made. Exactly one
worker was observed; the two measured Disconnect calls took 66.4 and 69.1 ms.
Demo keydown-to-second-rAF p95 was 16 and 12 ms (60 events per recorded load).
The manifest preserves the first failed focus/input attempt separately. These
results clear only the scoped container checks below, not the physical/audio gates.

## Isolated Linux release checks

Prerequisites: built release shell, separately built root runtime and CLI,
Python 3.11+, `tauri-driver`, matching
`WebKitWebDriver`, Xvfb, xauth, Openbox, xdotool, dbus-x11. Do not install these
on the operator host for this task; use hosted runners or an owned container.
The included Dockerfile supplies an ephemeral Ubuntu 24.04 SDK/display image.
Mount only this checkout, a component-owned Cargo cache/target and build tools;
never mount host display, session bus, audio sockets or `/dev/snd`.
Use two Cargo jobs and check at least 10 GiB free before/after installs/builds.

```sh
# Inside a disposable SDK container or ephemeral hosted runner, from repo root:
python3 verification/desktop-native/evidence.py --guard
# Root graph builds independently; no desktop SDK dependency in the root crate.
env -u DISPLAY -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS -u DBUS_SYSTEM_BUS_ADDRESS \
  CARGO_TARGET_DIR=target/headless-runtime cargo build --release --locked --bins --jobs 2
cd apps/desktop
pnpm install --frozen-lockfile
pnpm tauri build --no-bundle -- --locked --jobs 2
cd ../..
cargo install tauri-driver --locked --jobs 2
bash verification/desktop-native/run-linux.sh \
  apps/desktop/src-tauri/target/release/mixyard-desktop \
  target/headless-runtime/release/mixyard-runtime target/headless-runtime/release/mixyard target/headless-runtime/release/mixyard-web \
  /tmp/desktop-native-evidence
```

If `CARGO_TARGET_DIR` is shared, pass that directory's actual release binaries.
The native workflow separately builds the root release graph on Linux for this
harness. `.github/workflows/runtime.yml` owns the focused `runtime_lifecycle`
and `runtime_readonly` tests on all three OSes, plus existing core/binary checks;
there is no duplicate runtime test matrix in the native workflow.
The wrapper always starts its own Xvfb, Openbox, session bus and temporary home.
It does not connect to the caller's display. Only its own PIDs/process groups
are cleaned up. Direct `webdriver.py` execution is guarded against accidental
use on an existing display. No tray/panel host is started.

The stdlib Python W3C client drives **the actual release Tauri/WebKit webview**.
The locked Tauri runtime enables automation at runtime through the external
driver's `TAURI_WEBVIEW_AUTOMATION` environment; no debug build, app plugin,
test IPC command, ACL or CSP expansion is needed. See the official
[WebDriver CI](https://v2.tauri.app/develop/tests/webdriver/ci/) and
[manual external-driver setup](https://v2.tauri.app/develop/tests/webdriver/manual-setup/).

The harness verifies initial disconnected state without automatic connection,
locked audio controls, real shell IPC, Tab/Enter demo opening,
trusted native arrow-key range updates and >=2 px focus outlines, a 640 px
compact native window, native Alt+F4, the UI Close window button and Quit UI.
Each close must end that UI PID; a fresh real native window is then launched.
Each launch uses native typing into `#runtime-instance` (initially `default`)
and trusted Tab/Enter on Connect to subscribe to the existing private runtime.
It asserts connected phase, a running/resync snapshot, visible IPC v1, unavailable
backend and null desired/audio/applied revisions. IPC connectivity never unlocks
audio controls. Disconnect/reconnect must resync without stopping the child.

The real endpoint ignores XDG paths: Linux uses
`/tmp/mixyard-UID-native-HARNESS_PID/runtime.sock`. The harness refuses an
existing namespace, verifies 0700/0600 ownership and uses kernel `SO_PEERCRED`
to tie successful CLI status to the exact original child PID after Alt+F4,
UI Close window and Quit UI. A fourth attached UI observes explicit CLI
shutdown, the CLI event snapshot/stopping sequence, socket removal and child
reaping, then a refused reconnect without autospawn. `linux-webview.json` has
`runtime-survival.status: passed` only after those three survival cases and
explicit successful shutdown/reap complete. This proves headless lifecycle
independence only; native audio remains unavailable.

One additional protected same-UID Unix test endpoint delays its generated
snapshot until after Disconnect/reconnect. Only the harness-owned real child
is briefly SIGSTOP/SIGCONT gated so the new real snapshot cannot mask stale
publication. Both a late old snapshot and a late old stopping frame must leave
the new instance connecting with no event until the real child resumes.
An already-closed old subscription is recorded without claiming frame delivery.
Disconnect must return within one second and `/proc/UI_PID/task/*/comm` must
show exactly one `runtime-readon*` worker throughout the repeated connections.
Fixture frames are labelled test data and never substitute for real-runtime
survival evidence.

Transport errors during exit are retained and cannot pass without PID exit.
`trayCreated` is recorded as object construction, never proof of visibility.
Physical menu actions/visible-tray recovery remain blocked in this no-tray host.

For each normal-no-added-load and one-owned-busy-CPU-worker load, 60 trusted OS key events are
measured using the actual webview's `performance.now()` from keydown dispatch
to the second animation frame. The rendered Vue output must match the changed
range value. Raw samples, exact host/load and nearest-rank p95 are retained;
the job fails when either p95 exceeds 100 ms. This excludes time queued in the
OS before keydown dispatch and does not measure physical scanout, audio timing
or another desktop host. The shared host's load averages are recorded; the
normal case does not guarantee an idle machine. Xvfb/software rendering is explicitly labelled.
No browser or synthetic DOM event replaces a native sample.

## Blocked physical/platform gates and repeatable protocol

Until a corresponding result is recorded, **BLOCKED**: Windows/macOS UI
lifecycle and p95, actual visible tray/menu on every host, macOS Dock reopen,
screen-reader accessibility on every platform, physical presentation/performance,
and audio-engine survival. The software-rendered demo p95 has its own
narrow dispatch-to-frame scope and does not clear physical performance gates.
External `tauri-driver` has no macOS WKWebView driver; adding an embedded driver
plugin is outside this task. Compilation alone never clears these gates.

On a dedicated test desktop, record HEAD, binary SHA-256, OS build, architecture,
SDK/webview versions, desktop/WM, tray host and measured CPU/RAM/load:

1. Launch the release binary disconnected. Tab through controls; activate
   Show demo with Enter. Use keyboard to name/add/remove a mix, change sends,
   master and targets. Confirm labels, focus outlines, unavailable controls and
   640 px layout. Repeat with the platform screen reader (Orca/NVDA/VoiceOver),
   capturing accessible names, values, disabled states and live notices. DOM
   labels alone are not screen-reader proof.
2. With a visible tray, exercise Open Mixer, Close mixer window and Quit UI
   from the real menu. Linux/Windows close exits only the UI, then fresh launch
   restores it. macOS close hides; Dock click and tray Open Mixer must restore,
   unminimize and focus the same window. Quit must exit. Repeat with the tray
   absent, unavailable or hidden; macOS Dock remains the recovery path. Record
   failures; do not interpret tray object creation as visible recovery.
3. Exercise titlebar close, UI Close window and UI Quit; confirm UI process
   exit/recovery as above. A second launch while already running may open a
   second UI: no single-instance guarantee is claimed. No hide/relaunch
   recovery is assumed on Linux/Windows.
4. Repeat native-webview timing with >=60 real key events under stated idle
   and contended loads, preserving raw samples and the exact dispatch-to-frame
   definition. For Windows use the matching Edge/WebView2 native driver; for
   macOS use a separately approved native tooling path. Until exercised, leave
   those results BLOCKED. Do not substitute Chromium browser timings.
5. Audio-engine survival remains BLOCKED: the headless lifecycle daemon reports
   backend unavailable. Once a real audio backend exists, start it separately
   on an owned test audio setup, prove process/endpoint ownership, then repeat
   UI close/Quit and demonstrate uninterrupted audio operation. Neither a
   sleeping sentinel nor passing lifecycle IPC qualifies as audio evidence.
   Never change the operator's display/audio to run this protocol.

The checked-in `linux-container-evidence.json` distinguishes historical shell-only
results from this connection qualification. A prepared harness or successful
build does not imply an executed pass. Source hashes now include the root
Cargo graph, client/runtime sources, focused tests and native harness/workflows;
all three actual executed binaries receive SHA-256 fingerprints. Raw logs,
screenshots and caches stay outside the checkout.
