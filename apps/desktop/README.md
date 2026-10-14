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
pnpm tauri build         # Native binary; installer bundling deliberately disabled
```

On a qualifying desktop, detach longer native builds in tmux and report their
result. `pnpm tauri dev` starts its own frontend server; stop an existing server
on port 1420 before using it. The shell package has its own Cargo workspace and
lockfile under `src-tauri/`; it does not alter the root Cargo package.

## Implemented versus simulated

The default state is **Not connected**. It never enumerates devices, opens a mic,
changes system defaults, starts an audio service, or invokes the headless CLI.
Send, master, target, source and media-policy mutations are locked. Connection
checking only reads shell availability; the live control runtime is not
implemented. Missing IPC or an unsupported shell response leaves controls locked.
No runtime revision, accepted command or applied route is invented.

**Show demo** opens a visibly labelled, in-memory preview. Leaving demo resets
its edits. Fixtures are neither discovery results nor schema-2 runtime config.
There is no import/export or live apply operation.

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

The Rust shell constructs a native tray/menu with Open Mixer, disconnected
status, Close mixer window, Quit UI, and a disabled Stop runtime item. It has
no pretend mute/recording actions. Window closure hides the UI; Quit UI exits
only the interface. It neither spawns nor owns a runtime/sidecar and cannot stop
one. Tray creation failure is diagnosed without preventing app startup.
Tray creation success is not proof that the desktop host displays it.
On macOS, a Dock reopen event restores the window. Without a visible tray on
Linux/Windows, a hidden prototype window may require relaunching the UI; native
lifecycle/fallback behavior must be qualified before shipping.

Linux uses the native context-menu Open Mixer fallback; Tauri tray mouse events
are unsupported there. Rich anchored panels are not implemented or promised.
See [Tauri tray support](https://v2.tauri.app/learn/system-tray/).

Only three local commands are exposed through an explicit main-window
capability: `shell_status`, `hide_mixer`, `quit_ui`. No shell, filesystem,
device-control, remote-origin or generic runtime permissions are granted.
Release CSP excludes network control endpoints; the development CSP permits
only the local frontend/HMR origin. See
[Tauri capability rules](https://v2.tauri.app/security/capabilities/).

## Evidence and remaining gates

Verified 2026-10-14 on `homelab-dev` (Debian 13, headless): pnpm install, TypeScript,
seven model tests, production frontend build, Rust formatting, Tauri config
inspection and standalone Cargo metadata. These checks do **not** compile or
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
No system packages were installed and no heavy native build was attempted.
Linux needs its GTK/WebKit/AppIndicator libraries and a qualifying desktop/tray
host. Windows needs the MSVC toolchain and WebView2. macOS needs Apple development
tools. Follow [official prerequisites](https://v2.tauri.app/start/prerequisites/)
on actual test hosts.

Still required per supported platform/host: native compilation, window close /
reopen / Quit UI lifecycle, visible tray/menu behavior and no-tray fallback,
keyboard/screen-reader and compact-window checks, and measured native-webview
local interaction p95 <=100 ms under defined normal/contended load. This is a
proposed gate, not a result. No audio runtime or IPC meter transport exists here,
so UI-induced audio gaps, runtime survival during UI exit, bounded/coalesced
meter delivery, live accepted-versus-applied revisions, stale edits, reconnects,
virtual-input transport, device latency and sound quality remain unproved.
No UI screenshot or browser timing substitutes for those gates.
