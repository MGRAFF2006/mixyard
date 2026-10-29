#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

host=$(uname -s)
if [[ ${GITHUB_ACTIONS:-} == true && -n ${RUNNER_TEMP:-} ]]; then
  # Hosted Linux /dev/shm is small. Use that runner's isolated scratch, never the
  # shared development host's root or /tmp as an alternate local build cache.
  python3 - "$RUNNER_TEMP" <<'PY'
import shutil, sys
for path in ('.', sys.argv[1]):
    if shutil.disk_usage(path).free < 10 * 2**30:
        raise SystemExit(f'BLOCKED: keep 10 GiB free on hosted {path}')
PY
  scratch=$(mktemp -d "$RUNNER_TEMP/mixyard-native-macos.XXXXXX")
elif [[ $host == Linux ]]; then
  # This lane's explicit root exception is 2 GiB; scratch files require 10 GiB.
  python3 - <<'PY'
import shutil
for path, reserve in [('/', 2 * 2**30), ('/tmp', 10 * 2**30), ('/dev/shm', 10 * 2**30)]:
    free = shutil.disk_usage(path).free
    if free < reserve:
        raise SystemExit(f'BLOCKED: {path} free={free} reserve={reserve}')
PY
  scratch=$(mktemp -d /dev/shm/mixyard-native-macos.XXXXXX)
elif [[ $host == Darwin ]]; then
  if ! command -v xcrun >/dev/null; then
    echo 'BLOCKED: Apple macOS SDK/xcrun unavailable.' >&2
    exit 2
  fi
  python3 - <<'PY'
import shutil, tempfile
for path in ('.', tempfile.gettempdir()):
    if shutil.disk_usage(path).free < 10 * 2**30:
        raise SystemExit(f'BLOCKED: keep 10 GiB free on {path}')
PY
  scratch=$(mktemp -d "${TMPDIR:-/tmp}/mixyard-native-macos.XXXXXX")
else
  echo 'BLOCKED: worker fixture supports Linux and macOS toolchains only.' >&2
  exit 2
fi
trap 'rm -rf -- "$scratch"' EXIT
export TMPDIR="$scratch"
flags=(-std=c++17 -O1 -g0 -Wall -Wextra -Werror -pthread
       -fsanitize=address,undefined -fno-omit-frame-pointer)
frameworks=()
compiler=("${CXX:-c++}")
if [[ $host == Darwin ]]; then
  compiler=(xcrun --sdk macosx clang++)
  flags+=(-mmacosx-version-min=13.0)
  frameworks=(-framework CoreAudio -framework CoreFoundation)
fi
git rev-parse HEAD
"${compiler[@]}" --version
"${compiler[@]}" "${flags[@]}" worker_selfcheck.cpp "${frameworks[@]}" -o "$scratch/worker-selfcheck"
# One serial compile and one small executable; a hard per-leaf budget, not a cache.
used=$(du -sk "$scratch" | awk '{print $1}')
if (( used > 50 * 1024 )); then
  echo 'BLOCKED: macOS leaf artifact budget exceeded 50 MiB.' >&2
  exit 2
fi
if [[ $host == Darwin ]]; then
  # Existing hosted Apple sanitizer leak mode aborts before main; keep ASan/UBSan.
  ASAN_OPTIONS=detect_leaks=0 "$scratch/worker-selfcheck"
else
  "$scratch/worker-selfcheck"
fi
echo "Worker fixtures used ${used} KiB temporary artifacts; removed on exit. No physical audio executed."
