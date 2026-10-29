#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
if [[ $(uname -s) != Darwin ]]; then
  echo 'Apple SDK tests require a hosted macOS runner; no SDK installation or audio execution.' >&2
  exit 2
fi
if [[ ${GITHUB_ACTIONS:-} != true || -z ${RUNNER_TEMP:-} ]]; then
  echo 'This lane is authorized only on an isolated hosted Apple SDK runner.' >&2
  exit 2
fi
python3 - "$RUNNER_TEMP" <<'PY'
import shutil, sys
for p in ('.', sys.argv[1]):
    if shutil.disk_usage(p).free < 10 * 2**30:
        raise SystemExit(f'BLOCKED: keep 10 GiB free on hosted {p}')
PY
scratch=$(mktemp -d "$RUNNER_TEMP/mixyard-output-only.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
export TMPDIR="$scratch"
flags=(-std=c++17 -O1 -g0 -Wall -Wextra -Werror -fblocks -pthread -mmacosx-version-min=13.0)
frameworks=(-framework AudioUnit -framework CoreAudio -framework CoreFoundation)
xcrun --sdk macosx clang++ --version
# Compile the production library with real SDK declarations and real symbols.
xcrun --sdk macosx clang++ "${flags[@]}" -c output_only.mm -o "$scratch/output_only.o"
xcrun --sdk macosx ar rcs "$scratch/libmixyard-output-only.a" "$scratch/output_only.o"
xcrun --sdk macosx clang++ "${flags[@]}" link_check.cpp "$scratch/libmixyard-output-only.a" \
  "${frameworks[@]}" -o "$scratch/link-check"
# Linking proves symbol resolution. The production binary is deliberately NOT run.
rm -- "$scratch/output_only.o" "$scratch/libmixyard-output-only.a" "$scratch/link-check"
# Same implementation; only the public API boundary is substituted. No test calls
# a native audio device/AudioUnit. libdispatch and HostClock are real Apple APIs.
xcrun --sdk macosx clang++ "${flags[@]}" -DMIXYARD_OUTPUT_API_TEST \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  output_only.mm api_selfcheck.mm "${frameworks[@]}" -o "$scratch/api-selfcheck"
used=$(du -sk "$scratch" | awk '{print $1}')
if (( used > 20 * 1024 )); then
  echo 'BLOCKED: private output-only artifacts exceed 20 MiB' >&2
  exit 2
fi
ASAN_OPTIONS=detect_leaks=0 "$scratch/api-selfcheck"
echo "Output-only SDK link and injected API tests passed; ${used} KiB artifacts removed. No native audio execution."
