#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
available_kib=$(df -Pk . | awk 'NR == 2 {print $4}')
if (( available_kib < 10 * 1024 * 1024 )); then
  echo 'Keep at least 10 GiB free before compiling checks.' >&2
  exit 2
fi
mkdir -p build
scratch=$(mktemp -d build/selfcheck.XXXXXX)
# Only regenerable artifacts created by this invocation.
trap 'rm -rf -- "$scratch"' EXIT
"${CXX:-c++}" -std=c++17 -O1 -g -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer selfcheck.cpp -o "$scratch/selfcheck"
"$scratch/selfcheck" "$scratch/synthetic.wav"
python3 - <<'PY'
import plistlib
with open('Info.plist', 'rb') as f:
    p = plistlib.load(f)
assert p['CFBundleIdentifier'] == 'org.mixyard.coreaudio-probe'
assert p['NSAudioCaptureUsageDescription']
print('PASS: capture usage-description plist parses; macOS privacy behavior remains pending')
PY
bash -n build.sh check.sh
