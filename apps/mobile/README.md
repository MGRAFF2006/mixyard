# Native mobile controller and endpoint foundation

Android (Kotlin, API 28+) and iPhone/iPad (Swift, iOS 16+) embed the **same Vue
controller** from `apps/web`, injected through `Api.forControl`. These are SDK
projects, not a second mixer. Read [the contract and qualification gates](../../docs/mobile-client.md).

Pairing uses a native form: exact HTTPS origin, separately verified device ID and
leaf-certificate SHA-256 fingerprint, host-issued one-time invitation and controller
name. Ordinary certificate chain/hostname validation still applies. Self-signed
certificates must be trusted through the platform's supported trust provisioning;
the app does not bypass PKI. Credentials remain in Android Keystore-backed encrypted
storage or iOS Keychain, never WebView storage. The bundle contains no configured
endpoint or credentials. Hosted builds do not connect to any personal service.

Android uses the shipping `android/http` module (OkHttp 4.12.0) unchanged in JVM
qualification: TLS 1.3, platform PKI plus the exact leaf pin and hostname,
one-shot POST bodies, no retries/redirects/proxy/authentication follow-ups/cookies,
and bounded calls/raw UTF-8 replies. It preserves real HTTP 401 JSON without
buffered-POST replay. Android SDK compilation and JVM execution are separate from
Android runtime/Keystore qualification. Platforms without TLS 1.3 fail closed.

Backgrounding, disconnect/forget and pairing changes invalidate native authority,
unmount the old controller and discard pending completions. Reopen explicitly;
requests are never automatically replayed. Forget removes the local credential;
revoke it at the host to remove server authority as well. Closing the UI does not
change another device's graph.

## Portable checks

```sh
pnpm --dir apps/web install --frozen-lockfile
node --test apps/mobile/controller/src/bridge.test.ts
```

This runs the actual shared `Api` through the mobile adapter. It needs Node 24
and the locked sibling web dependencies because `Api` uses Vue for observable
lifecycle state. Existing compatible dependencies can be reused read-only. It does
not qualify native TLS, a mobile WebView or audio.

## Build with existing platform SDKs or hosted CI

From the repository root:

```sh
pnpm --dir apps/web install --frozen-lockfile
pnpm --dir apps/mobile/controller install --frozen-lockfile
pnpm --dir apps/mobile/controller build
node apps/mobile/scripts/stage-assets.mjs
cd apps/mobile/android
# Gradle 8.11.1, Java 17 and Android SDK 35; hosted setup-gradle supplies Gradle.
gradle --no-daemon :core:test :app:assembleDebug :app:lintDebug
```

On a Mac with Xcode:

```sh
cd apps/mobile/ios
swift test
xcodebuild -project Mixyard.xcodeproj -scheme Mixyard -configuration Debug \
  -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath /path/to/owned/DerivedData CODE_SIGNING_ALLOWED=NO build
```

Open `android/` in Android Studio or `ios/Mixyard.xcodeproj` in Xcode after asset
staging. No SDK installer, Gradle binary or signing credential is checked in.
[Mobile CI](../../.github/workflows/mobile-client.yml) builds the controller,
runs native synthetic byte/state tests, Android SDK compilation/lint and an unsigned
iPhone/iPad simulator SDK build. Signing, deployment and stores are separate gates.

## Audio boundary

`core/` on Android and the Swift `MobileCore` package contain native exact-format
raw queues, finite generated/raw-file sources, provenance admission and session
fences. Their tests open no audio device. All public audio roles currently return
`shared-core-and-transport-not-linked`; both applied/backend flags remain false.
No microphone permission, audio session, focus request, background service, output,
network audio peer or OS route is activated. There is no generic virtual microphone
or call-capture implementation. These limits are visible in native status as well
as the controller. SDK success must not become a physical audio badge.
