#!/usr/bin/env bash
set -euo pipefail
# Portable helper checks ONLY: no PipeWire header, SDK call, Rust build or host.
here=$(cd -- "$(dirname -- "$0")" && pwd)
root=$(cd -- "$here/../../.." && pwd)
output="/dev/shm/mixyard-private-helper-$$"
python3 -B "$here/resource_guard.py" --forecast 524288 --output "$output"
umask 077
mkdir -- "$output"
export TMPDIR="$output"
"${CC:-cc}" -std=c11 -O2 -g0 -Wall -Wextra -Werror -pedantic \
  -I"$root/adapters/engine/include" "$here/test_owner.c" -o "$output/owner-check"
"$output/owner-check"
if [[ ${1:-} == --owner-only ]]; then
  echo "json-c helper compile/run UNRUN (--owner-only); outputs retained: $output"
  exit 0
fi
pkg-config --exists json-c || { echo 'json-c helper compile/run UNRUN: explicit json-c development files unavailable' >&2; exit 2; }
"${CXX:-c++}" -std=c++17 -O2 -g0 -Wall -Wextra -Werror \
  $(pkg-config --cflags json-c) "$here/test_json.cpp" $(pkg-config --libs json-c) -o "$output/json-check"
"$output/json-check"
python3 -B - "$output" <<'PY'
from pathlib import Path
import sys
out = Path(sys.argv[1])
total = sum(p.stat().st_size for p in out.iterdir() if p.is_file())
if total > 524288:
    raise SystemExit(f"helper forecast exceeded: {total}")
print(f"helper outputs retained: {out}; {total} bytes; SDK compile/run UNRUN")
PY
