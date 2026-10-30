#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
task_build="$task_root/verification/mobile-native/swift/.build"
mkdir -p "$task_build"
task_target="$(uname -m)-apple-macosx13.0"
swiftc -target "$task_target" -parse-as-library -emit-library -emit-module -module-name MobileCore \
  "$task_root"/apps/mobile/ios/Sources/MobileCore/*.swift \
  -emit-module-path "$task_build/MobileCore.swiftmodule" -o "$task_build/libMobileCore.dylib"
swiftc -target "$task_target" -parse-as-library -I "$task_build" -L "$task_build" -lMobileCore \
  -Xlinker -rpath -Xlinker "$task_build" \
  "$task_root/apps/mobile/ios/App/CredentialStore.swift" \
  "$task_root/apps/mobile/ios/App/ControlClient.swift" \
  "$task_root/verification/mobile-native/swift/Qualification.swift" \
  -o "$task_build/native-control-qualification"
