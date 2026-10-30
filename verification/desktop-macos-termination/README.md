# Preventable macOS termination

Locked Tauri 2.12.3 / Muda 0.20.0 routes the default application-menu Quit
(and its Command-Q equivalent) to AppKit `terminate:`. Dock Quit reaches the same
application termination boundary. Tao 0.37.1 implements `applicationWillTerminate:`
but lacks `applicationShouldTerminate:`; its final LoopDestroyed event cannot be
cancelled by a Tauri ExitRequested handler.

The desktop adds only the missing optional method to the existing delegate class.
It retains/re-registers the same delegate instance so AppKit observes the method;
all Tao methods and state remain intact. Installation is main-thread-only and
fails closed if the delegate is absent or already owns a termination method. An
upstream policy change therefore requires explicit integration. No delegate proxy,
menu replacement, global swizzle or application/OS preference change is installed.

Every native termination attempt returns `NSTerminateCancel` immediately and
queues the same `quit` action as the desktop UI. The existing webview invokes the
shared aggregate retained-workspace draft guard. Cancel invokes no exit command;
approved `quit_ui` uses the existing Tauri explicit event-loop exit. It does not
call AppKit terminate again, stop a runtime, or grant persistent exit authority.
The process-lifetime callback holds one AppHandle; Rust panics cannot cross the
Objective-C boundary, and a callback failure still returns Cancel. macOS Close
continues to hide, Dock reopen restores, and Windows/Linux paths are unchanged.

The native implementation is `apps/desktop/src-tauri/src/macos_termination.m`,
compiled with the SDK by the existing locked `cc` crate, plus a thin macOS-only
Rust callback bridge. No new crate/version or GUI dependency enters the headless
runtime. Native SDK compilation/execution belongs on the hosted runner.

## Runnable evidence and limits

`.github/workflows/desktop-macos-termination.yml` runs on a disposable macOS VM.
The wrapper refuses local/non-GitHub execution. It does not use accessibility,
AppleEvents, key injection, screenshots, TCC suppression or permission changes.

1. `appkit.m` links the **production Objective-C seam** into a real NSApplication,
   NSWindow and NSMenu event loop. The no-guard control must perform native
   application termination before approval. The guarded probe dispatches a real
   menu item with the locked Muda Quit selector/key equivalent, then directly
   invokes `terminate:` as the Dock-equivalent boundary. Both must return through
   Cancel and retain the same visible native window/content marker. Explicit
   approval then stops the UI event loop and exits that UI process only.
2. `run.py` starts a separate real root runtime and CLI in a fresh private
   namespace. Darwin LOCAL_PEERPID must identify the same harness-owned runtime
   PID before and after each probe. Actual CLI state must keep audio unavailable
   and revisions null. Only explicit CLI shutdown may reap the runtime.
3. Actual shared Vue browser tests independently exercise dirty Processing and
   Situations child forms and hidden target retention. Existing `closeInterface`
   tests cover Cancel versus dispatching only `quit_ui`. Their inputs/native
   decision callback are not presented as a complete native webview integration.

These are explicitly **separate native AppKit and shared-browser composition
proofs**. The native content marker is not a fabricated Vue child editor. The
probe menu is an actual AppKit menu with the same selector, not a claim of
hit-testing the packaged Tauri menu, physical Dock or Command-Q keyboard delivery.
The regular native desktop workflow additionally compiles the real Tauri app and
its Rust callback wiring. Full macOS WKWebView/physical Dock, accessibility and
installer UX remain separately unqualified. A prepared workflow is not a pass;
use its exact-HEAD artifacts, raw outputs, SDK inventory and fingerprints.

The original Linux 5700 visibility failure remains UNKNOWN. This repair and its
macOS proof do not resolve it; the existing visible-title/real-WebKit/runtime-PID
qualification gates and diagnostics remain unchanged.

Primary contract sources:

- [Apple applicationShouldTerminate](https://developer.apple.com/documentation/appkit/nsapplicationdelegate/applicationshouldterminate(_:))
- [Apple terminate](https://developer.apple.com/documentation/appkit/nsapplication/terminate(_:))
- [Locked Tao delegate](https://github.com/tauri-apps/tao/blob/tao-v0.37.1/src/platform_impl/macos/app_delegate.rs)
- [Locked Tao explicit stop/exit](https://github.com/tauri-apps/tao/blob/tao-v0.37.1/src/platform_impl/macos/app_state.rs)
- [Darwin LOCAL_PEERPID](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/un.h)
