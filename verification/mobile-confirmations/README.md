# Native confirmation qualification

The shipping Android WebChromeClient and iOS WKUIDelegate answer confirmation
requests from the pinned bundled document. Android exposes a requesting URL,
not frame identity: the shell pins top-level navigation and its controller CSP
forbids frames. WebKit additionally exposes and checks isMainFrame and the exact
bundled file URL. Native dialogs require an active presentation and are bounded
to one pending answer; navigation, background, authority epoch changes and UI
disposal cancel it. Approval rechecks the document/generation/epoch. Cancel never
replaces a draft or invokes a store write.

The dedicated workflow builds the ACTUAL shared App with a visibly labelled,
read-only synthetic service boundary, then loads it in the SHIPPING native shells.
It is not a fake replacement editor or a native transport/authentication test.
The iOS Simulator XCUITest and SDK-only Android Instrumentation exercise dirty
routing replacement and codec-draft removal through real native Cancel/Confirm
buttons. The Android actor also checks pause cancellation and an untrusted URL.
The iOS actor backgrounds the owned app while a dialog is pending. A probe/build
is not a pass: exact-head hosted execution remains required.

The Android UI lane is intentionally SKIPPED unless root explicitly dispatches
`android_emulator: true`, authorizing its owned hosted SDK system-image download
and isolated no-audio emulator. No local SDK/dependency installs occur. It stages
an instrumentation class and the fixture assets only into the disposable debug
build; release sources/assets and Maven dependencies are unchanged. The iOS
lane uses a preinstalled simulator runtime and creates/deletes only its owned
simulator. Neither lane contacts a management service, enables audio/capture,
changes a personal phone/desktop, or qualifies physical device behavior.

Current-source SDK/dialog execution is pending. Native permissions, network
trust, secret storage and full shipping controller-to-service proof remain in
the separate native-control qualification lane; never infer them from this UI
fixture. Root owns final review and the Android image gate.

The preceding daf1dbc iOS lane FAILED before UI tests: MobileCore was built for
arm64-simulator while the application also emitted x86_64. The runner now requires
an installed runtime matching the selected SDK major/minor, verifies the owned
destination advertises the native runner architecture, and uses that explicit
architecture consistently for the actual app, package and test. A newer arbitrary
runtime is not selected; no architecture is excluded to hide an unsupported target.
The SDK/runtime/native-architecture selection is logged. Selection/cleanup units
are not SDK or dialog proof; fresh hosted execution remains required.

A separate94c738 iOS run timed out in bootstatus180s and both owned cleanup
commands30s each before any build/test. That is not the observed78/93 module
architecture failure. Fixed phase/outcome evidence now preserves the first
boot/build/test failure while independently attempting shutdown and delete.
Those180s/30s/30s limits and all native UI assertions are unchanged; cleanup
failure never becomes successful qualification.

The new862/2b/a306 family runs also failed at an EARLIER boundary: CoreSimulator
runtime discovery itself exceeded30s before creating a simulator. These are the
[78 job](https://github.com/MGRAFF2006/mixyard/actions/runs/40989290922/job/123847664523),
[92 job](https://github.com/MGRAFF2006/mixyard/actions/runs/40989400914/job/123848008540)
and [93 job](https://github.com/MGRAFF2006/mixyard/actions/runs/40989429466/job/123848090287).
No architecture build or native dialog test ran. Labeled capture now records the
selected toolchain before discovery and bounded read-only CoreSimulator/tool
process names and IDs on failure. No argv/environment/unrelated process data is
published; no service is killed/restarted and discovery is not retried or given
a longer deadline. The actual service-initialization cause remains unknown.

The authorized initial Android dispatch at8b7 compiled instrumentation but stopped
before any emulator/image or dialog assertion: its KVM gate lacked diagnostic
metadata. The gate now reports fixed existence/type, numeric UID/GID/mode and
current-user read/write state, verifies the node against the kernel KVM misc
registration, and only on the disposable approved GitHub Linux VM grants that
runner UID access using existing noninteractive setfacl (or chown if unavailable).
It rechecks the same device identity and access; absent/non-KVM nodes stay failed.
No world-writable permissions, software-emulator fallback or personal device is used.
Seven mocked boundary tests do not establish actual runner KVM/UI availability.

Two later iOS runs reached the ACTUAL app after successful matching18.5 discovery,
creation and boot: [78@91d](https://github.com/MGRAFF2006/mixyard/actions/runs/40992859342/job/123859251994)
and [93@eade](https://github.com/MGRAFF2006/mixyard/actions/runs/40993195702/job/123860322843).
Both failed at the first missing `Fixture load example` accessible button before
any native dialog assertion. This is distinct from the cold-discovery30s timeout:
the architecture/build gate was exercised here, but page rendering/bootstrap or
accessibility was not established. No native confirmation success is credited.

The qualification build alone now enables OWNED_CONFIRMATION_EVIDENCE. It records
fixed document ready/navigation classes, asset/marker/control counts and bounded
script/CSP error classes, never raw HTML, error messages, URLs, credentials or paths.
A test-only read-status action samples WebKit without bootstrapping Vue, changing
CSP or answering a dialog. The existing10s startup allowance covers both WebView
creation and real observed fixture/control readiness; all actual Cancel/Continue
and background-cancellation assertions remain. Production builds omit this probe.
Read-only first-launch status, resolved selected-Xcode simctl identity and numeric
CoreSimulator versions supplement the unchanged30s discovery capture. No service
initialization, install, retry, deadline extension or SDK fallback is performed.
These diagnostics require fresh actual hosted execution, not source-only credit.
