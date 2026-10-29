#!/usr/bin/env bash
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_OS:-}" == macOS ]] || { echo 'This qualification requires an owned hosted macOS runner' >&2; exit 1; }
[[ $# == 3 ]] || { echo 'Usage: run-swift.sh ENGINE_CHECKOUT ACTUAL_ENGINE_LIBRARY OWNED_FIXTURE_JSON' >&2; exit 1; }
task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
task_build="$RUNNER_TEMP/mixyard-mobile-host-swift"
mkdir -p "$task_build"
swiftc -parse-as-library -emit-library -emit-module -module-name MobileCore \
  "$task_root"/apps/mobile/ios/Sources/MobileCore/*.swift \
  -emit-module-path "$task_build/MobileCore.swiftmodule" -o "$task_build/libMobileCore.dylib"
swiftc -parse-as-library -emit-library -emit-module -module-name NativeHost \
  -I "$task_build" -I "$task_root/apps/mobile/native/swift/CHost" \
  -Xcc -I"$1/adapters/engine/include" -L "$task_build" -lMobileCore \
  -Xlinker "$2" -Xlinker -rpath -Xlinker "$task_build" \
  "$task_root/apps/mobile/native/swift/NativeHostWorker.swift" \
  -emit-module-path "$task_build/NativeHost.swiftmodule" -o "$task_build/libNativeHost.dylib"
swiftc -parse-as-library -I "$task_build" -I "$task_root/apps/mobile/native/swift/CHost" \
  -Xcc -I"$1/adapters/engine/include" -L "$task_build" -lMobileCore -lNativeHost \
  -Xlinker -rpath -Xlinker "$task_build" \
  "$task_root/verification/mobile-host/swift/HostQualification.swift" -o "$task_build/host-qualification"
"$task_build/host-qualification" "$3"
