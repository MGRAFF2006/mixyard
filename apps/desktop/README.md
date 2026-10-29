# Mixyard desktop

The Tauri 2 shell mounts the current `apps/web` Vue console and its routing,
profiles, DSP/channel/delay and situation workflows. There is no second desktop
mixer model. The desktop supplies a bounded native transport through `Api(fetcher)`;
the console owns its selected management target, session epochs, revisions and
stored-intent guards. Management targets are separate from audio destinations.

## Run

Build/launch the independent `mixyard-web` service separately, using the built web
or desktop assets, then run `pnpm tauri dev` or the packaged desktop executable.
The default native transport connects only to `127.0.0.1:8384`. To use another
explicit local service port, set `MIXYARD_WEB_PORT` to a nonzero numeric port before
starting the desktop. The app never starts or stops a runtime or service.
Browser-only Vite preview requires its own same-origin API; it does not substitute
fixture state when the service is absent.

The native bridge permits exact local console API routes, bounded JSON strings
and the local CSRF token. It rejects arbitrary origins, redirects, bearer headers,
remote-control routes, shell commands and filesystem paths. Request serialization
is preserved, including DSP integer precision. At most four blocking operations
are admitted, with bounded response/header sizes and a ten-second transport
deadline. A timeout or cancelled completion never causes an automatic write retry;
refresh and review the owning store first.

**Paired remote TLS transport is not implemented in this shell.** The shared
console supports trusted transport injection, but this native adapter must not
send `/control/*` credentials to the local CSRF service or invent device identity.
Legacy local sessions show identity unavailable. Live native audio mute is disabled;
all editable routing/profile levels are stored intent, not applied audio.

## Windows, macOS and Linux lifecycle

The tray/menu bar offers target/status labels, Open Mixer, a compact window,
Profiles (opening the same shared console), unavailable live mute, Close and Quit
UI. Compact/full use the same webview, preserving the selected target and draft.
Closing or quitting is checked against unsaved drafts before the UI exits.
A separate read-only local lifecycle diagnostics subscription never retargets
console edits or grants audio control.

On Windows and Linux, Close exits the UI, leaving services/runtime independent.
On macOS, Close hides the window; Dock reopen and Open Mixer restore it. Quit UI
exits the interface on all three platforms. Tray object creation does not prove
visibility, Explorer recovery, Dock behavior or assistive technology support.

Linux uses Tauri's StatusNotifier/AppIndicator menu where a host is available.
KDE, GNOME, Wayland and X11 need separate host qualification. No GNOME extension
is assumed or installed. Linux tray mouse events are not used for anchored panels;
Open compact controls opens the same ordinary window in compact mode. Without a tray,
full/compact windows and the headless CLI remain usable. Launch with
`MIXYARD_DESKTOP_NO_TRAY=1` to explicitly disable tray construction. There is no
autostart, OS-default, service installation or runtime shutdown side effect.

## Build and installer qualification

The hosted desktop workflows install ephemeral SDKs, build locked release shells,
run the std-only native HTTP boundary tests on each OS, then generate unsigned
packages. `tauri bundle --no-sign --no-binary-patching` preserves the previously
qualified executable while generating packages; see the official
[Tauri CLI](https://v2.tauri.app/reference/cli/). Each artifact is tied to HEAD,
runner/SDK inventory, file size and SHA-256 in `package-manifest.json`.
These are CI artifacts for qualification, not published releases.

| Runner | Generated formats | Execution gate | Still unverified |
| --- | --- | --- | --- |
| Ubuntu 24.04 | deb, AppImage | Owned Xvfb/WebKit shared-console and runtime-survival harness | Physical SNI hosts, Wayland/X11 desktops, installation/upgrade/rollback, assistive technology |
| Windows 2022 | MSI, NSIS exe | Native bridge tests; release compilation | Explorer/tray recovery, DPI/keyboard/accessibility, install/upgrade/uninstall |
| macOS 14 | app, DMG | Native bridge tests; release compilation | Menu bar/Dock lifecycle, accessibility, install/upgrade, signing/notarization |

A prepared workflow is not an executed pass. Current run results and raw evidence
belong on the PR. Local verification can reuse compatible read-only dependencies;
SDK/native builds must respect the task's disk budget. Native audio, hardware
capture/render, gap-free runtime survival, physical presentation, remote desktop
pairing and mobile support are separate gates. See
[the native harness](../../verification/desktop-native/README.md).
