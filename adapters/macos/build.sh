#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ $(uname -s) != Darwin ]]; then
  echo 'unavailable: native macOS build requires macOS and an Apple macOS SDK; no Linux stub is built.' >&2
  exit 2
fi
if ! command -v xcrun >/dev/null; then
  echo 'unavailable: xcrun/Apple developer tools are missing.' >&2
  exit 2
fi
available_kib=$(df -Pk . | awk 'NR == 2 {print $4}')
if (( available_kib < 10 * 1024 * 1024 )); then
  echo 'unavailable: keep at least 10 GiB free before building.' >&2
  exit 2
fi
sdk_version=$(xcrun --sdk macosx --show-sdk-version)
sdk_major=${sdk_version%%.*}
sdk_tail=${sdk_version#*.}
sdk_minor=${sdk_tail%%.*}
if (( sdk_major < 14 || (sdk_major == 14 && sdk_minor < 2) )); then
  echo "unsupported-sdk: found $sdk_version; Core Audio process taps require macOS SDK 14.2+." >&2
  exit 2
fi
mkdir -p build
# Keep the executable launchable on 13.x so capture can report unsupported-OS.
xcrun --sdk macosx clang++ -std=c++17 -O2 -Wall -Wextra -Werror \
  -Werror=unguarded-availability-new -fobjc-arc -mmacosx-version-min=13.0 \
  probe.mm -framework CoreAudio -framework Foundation -pthread \
  -Wl,-sectcreate,__TEXT,__info_plist,Info.plist -o build/mixyard-coreaudio-probe
echo "Built build/mixyard-coreaudio-probe with macOS SDK $sdk_version. Native audio is unqualified."
