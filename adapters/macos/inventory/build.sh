#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ $(uname -s) != Darwin ]]; then
  echo 'native-inventory-build-requires-macos' >&2
  exit 2
fi
build_dir=${1:-build}
sdk_version=$(xcrun --sdk macosx --show-sdk-version)
sdk_major=${sdk_version%%.*}
sdk_tail=${sdk_version#*.}
sdk_minor=${sdk_tail%%.*}
if (( sdk_major < 14 || (sdk_major == 14 && sdk_minor < 2) )); then
  echo 'native-inventory-build-requires-sdk-14.2' >&2
  exit 2
fi
mkdir -p -- "$build_dir"
xcrun --sdk macosx clang++ -std=c++17 -O2 -Wall -Wextra -Werror \
  -Werror=unguarded-availability-new -fobjc-arc -mmacosx-version-min=13.0 \
  main.mm -framework CoreAudio -framework Foundation \
  -o "$build_dir/mixyard-macos-inventory"
# No capture usage description, entitlements, driver install or permission prompt.
