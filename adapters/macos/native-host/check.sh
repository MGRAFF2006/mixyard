#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ $(uname -s) != Darwin || ${GITHUB_ACTIONS:-} != true || -z ${RUNNER_TEMP:-} ]]; then
  echo 'Native-host SDK checks require an isolated hosted Apple runner; no native audio is executed.' >&2
  exit 2
fi
if [[ $# != 2 ]]; then
  echo 'Usage: check.sh SHARED_HOST_CHECKOUT SHARED_HOST_LIBRARY' >&2
  exit 2
fi
host_checkout=$1
host_library=$2
test -f "$host_checkout/adapters/engine/include/mixyard_host.h"
test -f "$host_checkout/adapters/engine/fixtures/owned-common-clock.json"
test -f "$host_library"
python3 - "$RUNNER_TEMP" <<'PY'
import shutil, sys
for path in ('.', sys.argv[1]):
    if shutil.disk_usage(path).free < 10 * 2**30:
        raise SystemExit(f'BLOCKED: preserve 10 GiB on isolated hosted {path}')
PY
scratch=$(mktemp -d "$RUNNER_TEMP/mixyard-macos-host.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
export TMPDIR="$scratch"
flags=(-std=c++17 -O1 -g0 -Wall -Wextra -Werror -fblocks -pthread -mmacosx-version-min=14.2
  -I "$host_checkout/adapters/engine/include")
frameworks=(-framework AudioUnit -framework CoreAudio -framework CoreFoundation -framework Foundation
  -Wl,-rpath,"$(dirname "$host_library")")
xcrun --sdk macosx clang++ --version
# Real production symbols are linked but never executed. No production test
# override exists, and physical clock admission always rejects.
for file in host_link.cpp capture.mm ../output-only/output_only.mm; do
  name=$(basename "$file"); name=${name%.*}
  xcrun --sdk macosx clang++ "${flags[@]}" -c "$file" -o "$scratch/$name.o"
done
xcrun --sdk macosx ar rcs "$scratch/libmixyard-macos-host.a" "$scratch/host_link.o" "$scratch/capture.o" "$scratch/output_only.o"
xcrun --sdk macosx clang++ "${flags[@]}" link_check.cpp \
  -Wl,-force_load,"$scratch/libmixyard-macos-host.a" "$host_library" "${frameworks[@]}" -o "$scratch/link-check"
rm -- "$scratch/host_link.o" "$scratch/capture.o" "$scratch/output_only.o" "$scratch/libmixyard-macos-host.a" "$scratch/link-check"
# Actual Rust policy/mixer/DSP/clock; only native audio SDK functions are replaced.
xcrun --sdk macosx clang++ "${flags[@]}" -DMIXYARD_NATIVE_HOST_TEST -DMIXYARD_OUTPUT_API_TEST \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  host_link.cpp capture.mm ../output-only/output_only.mm selfcheck.mm \
  "$host_library" "${frameworks[@]}" -o "$scratch/selfcheck"
used=$(du -sk "$scratch" | awk '{print $1}')
if (( used > 16 * 1024 )); then
  echo 'BLOCKED: private macOS attachment artifacts exceed 16 MiB' >&2
  exit 2
fi
ASAN_OPTIONS=detect_leaks=0 "$scratch/selfcheck" "$host_checkout/adapters/engine/fixtures/owned-common-clock.json"
echo "Native-host linked SDK/API-double tests passed; measured private peak ${used} KiB; artifacts removed. No native audio."
