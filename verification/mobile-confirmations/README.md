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
