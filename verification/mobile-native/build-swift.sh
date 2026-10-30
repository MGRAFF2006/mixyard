#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
task_build="${MIXYARD_SWIFT_BUILD_DIR:-$task_root/verification/mobile-native/swift/.build}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_OS:-}" == macOS ]] || { echo 'Disposable hosted Mac required'; exit 1; }
python3 -B "$task_root/verification/mobile-native/swift-package-lock.py"
mkdir -p "$task_build"
task_package="$task_root/apps/mobile/ios"
task_scratch="$task_build/package"
task_arch="$(uname -m)"
task_target="$task_arch-apple-macosx13.0"
task_lock_before="$(shasum -a 256 "$task_package/Package.resolved")"
# This product includes the exact same target/dependency closure Xcode links.
# No parser/client copy and no dynamic SwiftASN1/ControlTrust runtime library.
swift build --package-path "$task_package" --scratch-path "$task_scratch" --force-resolved-versions \
  --triple "$task_target" --product MobileControlTrust
swift test --package-path "$task_package" --scratch-path "$task_scratch" --force-resolved-versions --triple "$task_target"
task_bin="$(swift build --package-path "$task_package" --scratch-path "$task_scratch" --force-resolved-versions --triple "$task_target" --show-bin-path)"
swiftc -target "$task_target" -parse-as-library -emit-library -emit-module -module-name MobileCore \
  "$task_root"/apps/mobile/ios/Sources/MobileCore/*.swift \
  -emit-module-path "$task_build/MobileCore.swiftmodule" -o "$task_build/libMobileCore.dylib"
swiftc -target "$task_target" -parse-as-library -I "$task_build" -I "$task_bin/Modules" -L "$task_build" -lMobileCore \
  -Xlinker -rpath -Xlinker "$task_build" \
  "$task_root/apps/mobile/ios/App/CredentialStore.swift" \
  "$task_root/apps/mobile/ios/App/ControlClient.swift" \
  "$task_root/verification/mobile-native/swift/Qualification.swift" \
  "$task_bin/libMobileControlTrust.a" -framework Security -framework Foundation \
  -o "$task_build/native-control-qualification"
[[ "$task_lock_before" == "$(shasum -a 256 "$task_package/Package.resolved")" ]] || { echo 'Resolver changed committed lock'; exit 1; }
python3 -B "$task_root/verification/mobile-native/swift-link-evidence.py" "$task_build" "$task_bin" "$task_package"
