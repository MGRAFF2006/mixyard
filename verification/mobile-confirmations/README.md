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

The actual [92@f3c iOS run](https://github.com/MGRAFF2006/mixyard/actions/runs/40997623894/job/123874474202)
now narrows the first-control failure: document complete/nav finished/index exists,
marker true, one script asset, runtime error, no CSP event, zero app children or
fixture controls. Vue never mounted in this observation; this is not just a late
WebView or accessible-button selector. It still does not identify the operation.
The qualification-only observer now emits only an allowlisted error NAME and
bounded numeric line/column, fixed module-load/fixture-boot phases, and fixed API
capability booleans. No error message/stack/URL/HTML is logged. This diagnoses the
actual failing boot boundary; it does not add a polyfill, change CSP or force Vue
or confirmation. All production/native dialog assertions remain unchanged.

The same runtime/app-not-mounted fields were independently observed in
[78@fe57](https://github.com/MGRAFF2006/mixyard/actions/runs/40997578211/job/123874329843)
and [93@6c07](https://github.com/MGRAFF2006/mixyard/actions/runs/40997677253/job/123875047689).
All three runs completed owned simulator shutdown/delete; no native alert
assertion ran. Embedded actual observer/query privacy tests execute only JavaScript
classifications in an isolated VM and do not stand in for WebKit SDK execution.

Repeated current-family16.4 discovery failures sometimes precede the prepared
bootstrap diagnostic. One explicitly named COMPARISON job therefore uses only
preinstalled macos14/Xcode16.2/SDK+runtime18.2. Exact toolchain/SDK/runtime/native
architecture inputs are required or the lane fails, with no install or fallback.
The same shipping source, qualification flag, UI assertions,10s readiness and all
original command/boot/test/cleanup limits run unchanged. The separate16.4 lane
remains. A comparison pass is not causal repair or current-runtime eligibility.

The first actual16.2 comparison [92@dbd run](https://github.com/MGRAFF2006/mixyard/actions/runs/41001387504/job/123886466019)
passed discovery/create but failed native-architecture advertisement before build
or UI. Captured destination stdout was not published, so the exact owned-entry
format is still unknown. A bounded diagnostic now emits only that UUID's parsed
platform/architecture/availability/error-presence fields, never names/UUIDs/raw
stdout; the architecture gate is unchanged. Independent delete completed after
shutdown149/alreadyShutdown. Exit149 now requires exact unique owned-UUID Shutdown
readback within the SAME30s shutdown budget to establish the desired cleanup
postcondition; it is never a blanket allowed status. The original architecture
failure stays primary. No compilation/native-dialog success is inferred.

Actual comparison destination evidence now confirms that Xcode16.2 omits the
architecture FIELD for its available, error-free, exact-owned simulator record;
it does not report a conflicting architecture. Missing text remains UNKNOWN.
Destination UUID/platform/availability/error/duplicate checks remain strict, but
positive architecture proof moves to executable artifacts. The same native ARCHS,
ONLY_ACTIVE_ARCH, exact SDK/runtime/UUID and diagnostic flag build-for-testing,
then preinstalled lipo must verify the app, UI-test bundle/runner, compiled
MobileCore module+objects and packaged Mach-O dependencies before any UI execution.
Own compiled products must be native-only; universal SDK dependencies must contain
that native arch. Missing/wrong/ambiguous proof fails. Artifact role counts and
app/test SHA256 are logged without paths; no Apple tool is mocked in hosted proof.
Build, artifact verification and test-without-building share the EXISTING900s
build/test budget. Native UI assertions/readiness are unchanged. Local proof-gate
units use clearly fake bytes/tool boundaries, not SDK compilation or UI evidence.
