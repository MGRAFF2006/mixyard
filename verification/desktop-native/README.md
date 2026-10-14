# Desktop native qualification

This component builds and executes the independent Tauri shell. It never runs
the root CLI, a sidecar or audio runtime. `.github/workflows/desktop-native.yml`
builds locked release binaries on Ubuntu 24.04, Windows 2022 and macOS 14 with
Rust 1.99.0, Node 24.13.1 and pnpm 11.10.0. Hosted SDK packages are ephemeral.
Actual image/OS, architecture, SDK/package versions, command logs, source hashes,
binary SHA-256 and failures are uploaded in per-runner/HEAD evidence artifacts.
Binaries are separate, explicitly labelled artifacts; no build-cache upload.
Runner labels are not exact OS/SDK versions: use the recorded inventory.

## Isolated Linux release checks

Prerequisites: built release shell, Python 3.11+, `tauri-driver`, matching
`WebKitWebDriver`, Xvfb, xauth, Openbox, xdotool, dbus-x11. Do not install these
on the operator host for this task; use hosted runners or an owned container.
The included Dockerfile supplies an ephemeral Ubuntu 24.04 SDK/display image.
Mount only this checkout, a component-owned Cargo cache/target and build tools;
never mount host display, session bus, audio sockets or `/dev/snd`.
Use two Cargo jobs and check at least 10 GiB free before/after installs/builds.

```sh
# Inside a disposable SDK container or ephemeral hosted runner, from repo root:
cd apps/desktop
pnpm install --frozen-lockfile
pnpm tauri build --no-bundle -- --locked --jobs 2
cd ../..
cargo install tauri-driver --locked --jobs 2
bash verification/desktop-native/run-linux.sh \
  apps/desktop/src-tauri/target/release/mixyard-desktop /tmp/desktop-native-evidence
```

If `CARGO_TARGET_DIR` is shared, pass that directory's actual release binary.
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

The harness verifies disconnected locks/real shell IPC, Tab/Enter demo opening,
trusted native arrow-key range updates and >=2 px focus outlines, a 640 px
compact native window, native Alt+F4, the UI Close window button and Quit UI.
Each close must end that UI PID; a fresh real native window is then launched.
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
screen-reader accessibility on every platform, and audio/runtime survival.
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
5. Runtime survival is BLOCKED while the independent runtime is absent. Once
   available, start it separately on an owned test audio setup, prove it owns
   its process/endpoints, then repeat UI close/Quit and show uninterrupted
   operation. A sleeping sentinel process does not qualify as audio/runtime
   evidence. Never change the operator's display/audio to run this protocol.
