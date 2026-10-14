# Mixyard desktop prototype

Vue 3 + Vite + TypeScript UI, with an independent Tauri 2 desktop/tray shell.
All package files and dependencies stay in this directory. The root Rust policy
CLI has no dependency on this shell, Tauri, WebKit or a display server.

## Run and check

Use Node 24 and pnpm (the current host provides these through mise).
Keep at least 10 GiB free before dependency installation or native builds.

```sh
cd apps/desktop
pnpm install --frozen-lockfile
pnpm dev                 # http://127.0.0.1:1420
pnpm check               # TypeScript + focused model tests
pnpm build               # TypeScript + production frontend
pnpm preview             # Serve the production frontend
pnpm tauri info          # Inspect native prerequisites; does not build the app
pnpm tauri dev           # Requires native prerequisites and a desktop session
pnpm tauri build --no-bundle -- --locked --jobs 2 # Release binary; no installer
```

On a qualifying desktop, detach longer native builds in tmux and report their
result. `pnpm tauri dev` starts its own frontend server; stop an existing server
on port 1420 before using it. The shell package has its own Cargo workspace and
lockfile under `src-tauri/`. Its only core dependency points desktop → root library; root Cargo remains GUI/display-free.

## Implemented versus simulated

The default state is **Not connected**. It never enumerates devices, opens a mic,
changes system defaults, starts an audio service, or invokes the headless CLI.
Send, master, target, source and media-policy mutations stay locked even when connected.
Start the existing runtime separately (`cargo run --bin mixyard-runtime -- serve
--instance default`), then use **Connect** with that local instance name. The
read-only subscription uses the same Unix peer-UID / Windows SID-checked
transport as the CLI. There is no second transport or CLI subprocess bridge.
It displays IPC v1 snapshot/sequence/lifecycle, `backendAvailable: false`, and
null audio/applied/desired revisions. Connected means lifecycle IPC only;
it never means working audio. Missing IPC, capability/version mismatch, stopped
runtime and unexpected disconnect fail closed. Reconnect is explicit and resyncs.
No runtime revision, accepted audio command or applied route is invented.

One worker owns at most one subscription; there are no per-connect threads,
queued commands, automatic retry loops or event history. All transport work stays
off the UI thread, startup is bounded to five seconds, partial frames expire,
and status polling has one outstanding request at most. **Disconnect**, window
close and Quit UI drop only the UI connection. Runtime startup/shutdown remain
explicit CLI operations; the app never starts, owns or stops the process.

**Show demo** opens a visibly labelled, in-memory preview. Leaving demo resets
its edits. Fixtures are neither discovery results nor schema-2 runtime config.
The demo does not import/export or apply routes. The separate read-only snapshot
inspection below uses real schema-2 inputs and the Rust core.

## Named local profiles

Open **Named local profiles**, import schema-2 inventory content, and explicitly
**Refresh profiles**. Select a stable profile ID to show its saved desired config
and **Preview selected profile** against the store's current draft. Existing
[`mixyard profiles`](../../docs/profiles.md) CLI commands create/edit these drafts;
this panel uses the native process's platform default current-user store only.
No arbitrary frontend path, CLI subprocess, store write, migration or repair is
exposed. Missing, empty, corrupt, unsafe and busy/recovery states stay distinct.

Preview rereads/validates the store and rejects a changed store revision. Results
bind store revision, profile ID and imported inventory revision, and display the
core's source/send/mix/master/output/media changes, affected sensitive paths on
both sides and unavailable identities. Refresh, inventory input changes, selection
and errors clear prior context; late replies are discarded. Apply stays disabled
independently of runtime connection; `confirmedWorkingSetup` remains null. This
is desired-state comparison, with imported evidence unverified and no live-audio
claim. See [focused bounds, tests and native evidence](../../verification/desktop-profiles/README.md).

## Why silent / what is sharing

Open **Why silent · What is sharing**, select config and inventory JSON files or
paste their contents, then **Inspect snapshots**. This calls the existing
`diagnostics::inspect(config, inventory)` directly, independently of the runtime
connection and demo. No filesystem path is sent to Rust. Browser preview reports
that native inspection is unavailable; it never substitutes a fixture.

The panel shows imported config/inventory revisions, platform and declared
inventory evidence, with freshness unverified and snapshot age/current native
revisions unknown. `native-discovery` in an imported file is not authenticated
live discovery. Core preflight detail, reason codes, capacity, accepted-plan
headroom, selectors/candidates/formats and every source/output path are shown.
Configured potential sharing includes muted/zero and invalid excluded sends;
accepted dry-run coefficients alone are displayed as numbers. Rejected preflight
leaves all coefficients unavailable. Permission, consumer selection, applied
sharing, capture/native/pause state and live levels remain unknown.

Imports are content-only and bounded: 128 KiB UTF-8 per input, 32 configured
sources/mixes/outputs each, 256 sends, 64 media policies and source/output
observations each, 128 sessions, and a 2 MiB reply. These are desktop work limits,
not product/backend capacities. Oversized snapshots are rejected whole; CLI
`inspect` is available for larger inputs. Revisions beyond JavaScript's exact
integer range (9007199254740991) are explicitly rejected. Rust admits one worker
at a time off the UI thread; the bridge has a two-second response timeout. Editing,
selecting another file, rejection or Clear removes the previous report; generation
checks discard late results. Inputs/results stay in window memory and are not saved.

The focused [boundary tests and native inspection harness](../../verification/desktop-inspection/README.md)
are separate from the existing lifecycle/three-OS shell compile gates. No routing,
profile writes, permission granting, backend enablement or audio processing is added.

- Mixes have stable IDs and editable names; add/remove is dynamic with no fixed
  mix-count limit. Call, Record and Monitor are starter templates only.
- Each named source has independent gain/mute per mix. The same mic starts at
  100% in Call, 50% in Record and 0% in Monitor. Send gain is linear amplitude;
  its dB conversion is displayed beside the percentage.
- A mix routes to multiple targets, each with its own stable ID, editable label,
  gain and mute. Its master changes every output. The current policy prototype
  allows one mix per target; converging mixes are not offered by this preview.
  The fixture device list is finite; the model imposes no output-count ceiling.
- Call-return exclusion follows the mix policy and call-target identity, never
  the visible name. A physical mic-output route requires explicit monitoring
  consent, retained regardless of mute/zero gain. New mixes start with silent,
  unselected sends; selecting a mic send retains route intent until removal.
- Static illustrative levels are labelled fixture samples, never current meters.
  Missing sources show unavailable levels. Conservative headroom includes missing
  selected sources, and no other send is silently normalized. Invalid previews
  and unconsented monitoring show concrete action status.
- Media policy is configured per fixture device: normal OS session, associated
  primary player, or all associated routed players. Software **Preview play** /
  **Preview pause** set explicit coherent player states. Session associations
  survive pausing; no stream discovery or native player operation exists.
  Pausing Spotify from A stops Spotify everywhere it is routed, including B.
  Output mute quiets only its own branch. Hardware origin and dispatch suppression
  are unverified, so no Bluetooth/media/hardware-mute/call events are dispatched.
  Codec/profile and call answer/end capability remain unknown.

## Native shell and tray

The Rust shell constructs a native tray/menu with Open Mixer, an instruction
to inspect read-only runtime state in the mixer, Close mixer window, Quit UI, and a disabled Stop runtime item. It has
no pretend mute/recording actions. On Linux/Windows, titlebar close, Close window
and the tray Close mixer window action exit only the UI. A fresh launch opens
a new window. Tray construction cannot establish visible host recovery, so
these platforms conservatively avoid hiding an unreachable UI. There is no
single-instance plugin: launching while already running may create another UI.
On macOS, close hides and a Dock reopen event restores the window independently
of tray availability; that physical Dock path still needs qualification.
Quit UI exits only the interface on every platform. The shell neither spawns
nor owns a runtime/sidecar and cannot stop one. Tray creation failure is
diagnosed without preventing app startup. `trayCreated` means an object was
constructed, not that a host displayed its icon or menu.

Linux uses the native context-menu Open Mixer fallback; Tauri tray mouse events
are unsupported there. Rich anchored panels are not implemented or promised.
See [Tauri tray support](https://v2.tauri.app/learn/system-tray/).

Only six local commands are exposed through an explicit main-window
capability: `shell_status`, `connect_runtime`, `disconnect_runtime`, `hide_mixer`,
`quit_ui`, `inspect_snapshot`. Inspection accepts bounded JSON content, not paths.
Connect accepts only a validated local instance name. No shell, filesystem,
device-control, remote-origin or generic runtime permissions are granted.
The existing `hide_mixer` command name is retained, with the platform-specific
close behavior above; native qualification adds no test command or plugin.
Release CSP excludes network control endpoints; the development CSP permits
only the local frontend/HMR origin. See
[Tauri capability rules](https://v2.tauri.app/security/capabilities/).

## Evidence and remaining gates

Earlier host-only verification on 2026-10-14 on `homelab-dev` (Debian 13, headless): pnpm install, TypeScript,
seven model tests, production frontend build, Rust formatting, Tauri config
inspection and standalone Cargo metadata. Those checks did **not** compile or
execute the native shell. Production JS is about 34 KiB compressed.

The T3 collaborative browser was used for real rendered desktop and compact
layouts, including 1280 px desktop, 640 px compact and 360 px narrow layouts.
Checks covered disconnected mutation locks/unavailable levels, labelled demo,
keyboard Tab/Enter mix creation, keyboard renaming, native range ArrowRight
updates with 2 px focus outlines, multiple outputs and independent controls,
missing mic / monitoring consent / renamed exclusions, media-policy previews,
and demo exit/reset. No horizontal overflow or browser console errors remained.
Browser evaluation is UI evidence, not native webview performance certification.

Native prerequisites are absent here: `tauri info` reports missing
`webkit2gtk-4.1` and `rsvg2`; AppIndicator development detection is also absent.
No native SDK packages were installed on the operator host.
Linux needs its GTK/WebKit/AppIndicator libraries and a qualifying desktop/tray
host. Windows needs the MSVC toolchain and WebView2. macOS needs Apple development
tools. Follow [official prerequisites](https://v2.tauri.app/start/prerequisites/)
on actual test hosts. The new
[native shell workflow](../../.github/workflows/desktop-native.yml) supplies
ephemeral Linux SDK dependencies and builds actual locked release shells on
Ubuntu 24.04, Windows 2022 and macOS 14. Each OS uploads labelled binaries,
SHA-256/source hashes, exact OS/SDK/tool versions and executed command logs.
Build success is compilation evidence only; Windows/macOS UI, accessibility
and native-webview timing remain **BLOCKED** until separately exercised.

The focused [native harness and physical protocol](../../verification/desktop-native/README.md)
use the release Tauri/WebKit shell in an owned Linux Xvfb/Openbox/session bus,
without a tray host. They cover trusted keyboard input, compact layout,
close/exit/relaunch and actual-webview keydown-to-second-frame p95 under stated
loads. No frontend browser timing is reused as native evidence. Read each
run's `linux-webview.json`; failure or absence leaves its execution gates blocked.

On 2026-10-14, an owned Ubuntu **24.04.5** x86_64 SDK container on `homelab-dev`
successfully built the locked **release** Tauri 2.12.3/Wry 0.57.0 shell with
Rust 1.99.0, Node 24.13.1 and pnpm 11.10.0. WebKitGTK/driver were 2.52.6,
GTK 3.24.41, librsvg 2.58.0 and AppIndicator package 0.5.93-1build3.
The network-disabled Xvfb/Openbox run exercised the actual `tauri://localhost`
webview, real shell IPC, Tab/Enter demo, trusted arrow keys/focus outlines,
640 px compact layout, native Alt+F4, UI Close window, Quit UI and fresh-launch
recovery. `trayCreated` was true without a tray host; all three exit paths
terminated their UI PID. With 60 samples per case, local keydown-to-second-frame
p95 was **17 ms** with no added load and **15 ms** with one owned busy CPU worker
on this six-vCPU, shared-load, software-rendered host only. These measurements
exclude pre-dispatch OS queuing and physical display presentation.
See [exact scoped container evidence and raw timings](../../verification/desktop-native/linux-container-evidence.json).
Windows/macOS builds are prepared in the matrix but have not run at this handoff.

Still required on unexercised platforms/hosts: native compilation, window close /
reopen / Quit UI lifecycle, visible tray/menu behavior and no-tray fallback,
keyboard/screen-reader and compact-window checks, and measured native-webview
local interaction p95 <=100 ms under defined normal/contended load. The scoped
container result above predates the lifecycle bridge and does not certify those
shipping gates. The extended native harness starts a private real headless
runtime explicitly, connects through trusted keyboard input, and checks the
same runtime PID with CLI status after each UI close/Quit before explicit CLI
shutdown. Only a passing current `linux-webview.json` result with same-runtime survival steps proves that scope;
it is lifecycle process/IPC survival, with audio still unavailable.
UI-induced audio gaps, meter delivery, live accepted-versus-applied audio
revisions, stale edits, virtual-input transport, device latency and sound quality
remain unproved.
No UI screenshot or browser timing substitutes for those gates.
