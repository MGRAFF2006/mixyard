#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ $(uname -s) != Linux || $# != 1 || $1 != *.a || ! -f $1 ]]; then
  echo 'Usage on Linux: portable-check.sh READ_ONLY_ACTUAL_HOST_STATIC_LIBRARY' >&2
  exit 2
fi
host_library=$1
python3 - <<'PY'
import shutil
for path, minimum in (('/', 2 * 2**30), ('/tmp', 10 * 2**30), ('/dev/shm', 10 * 2**30)):
    free = shutil.disk_usage(path).free
    if free < minimum:
        raise SystemExit(f'BLOCKED: {path} has {free} bytes free; require {minimum}')
PY
scratch=$(mktemp -d /dev/shm/mixyard-macos-host.XXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
export TMPDIR="$scratch"
peak=0
measure() {
  local used
  used=$(du -sk "$scratch" | awk '{print $1}')
  (( used <= peak )) || peak=$used
  if (( used > 16 * 1024 )); then
    echo 'BLOCKED: private platform artifacts exceed 16 MiB' >&2
    exit 2
  fi
}
flags=(-std=c++17 -O1 -g0 -pipe -ffunction-sections -fdata-sections -Wall -Wextra -Werror
  -DMIXYARD_NATIVE_HOST_TEST -I ../../engine/include)
g++ "${flags[@]}" -c host_link.cpp -o "$scratch/host_link.o"
measure
g++ "${flags[@]}" -c portable_selfcheck.cpp -o "$scratch/selfcheck.o"
measure
g++ -s -Wl,--gc-sections "$scratch/host_link.o" "$scratch/selfcheck.o" "$host_library" \
  -pthread -ldl -lm -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free -o "$scratch/selfcheck"
measure
"$scratch/selfcheck" ../../engine/fixtures/owned-common-clock.json
echo "Measured generated artifact peak ${peak} KiB; read-only host library unchanged; private artifacts removed."
