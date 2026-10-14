#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
if [[ $(uname -s) != Darwin ]] || ! command -v xcrun >/dev/null; then
  echo 'BLOCKED: real Apple macOS SDK/xcrun required; no portable HAL stub qualifies.' >&2
  exit 2
fi
artifact_dir=verification/macos-input/build
mkdir -p "$artifact_dir"
reserve() {
  python3 - "$artifact_dir" <<'PY'
import shutil, sys
free = shutil.disk_usage(sys.argv[1]).free
print(f'free_bytes={free} reserve_bytes=10737418240')
if free < 10737418240:
    raise SystemExit('BLOCKED: exact 10 GiB reserve unavailable')
PY
}
reserve
trap reserve EXIT
sdk_path=$(xcrun --sdk macosx --show-sdk-path)
{
  sw_vers
  uname -m
  xcrun --sdk macosx --show-sdk-version
  xcrun --sdk macosx clang++ --version
  git rev-parse HEAD
} > "$artifact_dir/toolchain.txt"
source_dir=adapters/macos/virtual-input
bundle="$artifact_dir/MixyardInput.driver"
mkdir -p "$bundle/Contents/MacOS"
cp "$source_dir/Info.plist" "$bundle/Contents/Info.plist"
flags=(-std=c++17 -O2 -g -Wall -Wextra -Werror -fvisibility=hidden
       -isysroot "$sdk_path" -mmacosx-version-min=12.0 -pthread)
frameworks=(-framework CoreAudio -framework CoreFoundation)
# Serial compile commands keep this small artifact below the jobs=2 limit.
xcrun --sdk macosx clang++ "${flags[@]}" -bundle \
  "$source_dir/input_driver.cpp" "${frameworks[@]}" \
  -Wl,-exported_symbol,_MixyardInput_Create -o "$bundle/Contents/MacOS/MixyardInput"
reserve
xcrun --sdk macosx clang++ "${flags[@]}" -DMIXYARD_INPUT_TESTING \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  "$source_dir/input_driver.cpp" verification/macos-input/harness.cpp \
  "${frameworks[@]}" -o "$artifact_dir/abi-harness"
xcrun --sdk macosx clang++ "${flags[@]}" verification/macos-input/consumer.cpp \
  "${frameworks[@]}" -Wl,-sectcreate,__TEXT,__info_plist,verification/macos-input/ConsumerInfo.plist \
  -o "$artifact_dir/mixyard-input-consumer"
codesign --force --sign - "$bundle"
codesign --verify --strict --verbose=2 "$bundle"
codesign --force --sign - "$artifact_dir/mixyard-input-consumer"
codesign --verify --strict --verbose=2 "$artifact_dir/mixyard-input-consumer"
# This is bundle verification, never HAL acceptance. No install, service reload or consumer execution.
nm -gU "$bundle/Contents/MacOS/MixyardInput" > "$artifact_dir/release-exports.txt"
python3 - "$artifact_dir/release-exports.txt" <<'PY'
import sys
symbols = [line.split()[-1] for line in open(sys.argv[1]) if line.strip()]
assert symbols == ['_MixyardInput_Create'], symbols
PY
ASAN_OPTIONS=detect_leaks=1 "$artifact_dir/abi-harness" "$bundle/Contents/MacOS/MixyardInput" > "$artifact_dir/harness.json"
python3 - "$artifact_dir" <<'PY'
import hashlib, json, pathlib, platform, subprocess, sys
root = pathlib.Path(sys.argv[1])
harness = json.loads((root / 'harness.json').read_text())
assert harness['evidence_kind'] == 'in_process_real_Apple_ABI'
assert harness['status'] == 'pass' and harness['native_HAL_PCM'] == 'BLOCKED'
files = [p for tree in ('adapters/macos/virtual-input', 'verification/macos-input')
         for p in pathlib.Path(tree).rglob('*') if p.is_file() and 'build' not in p.parts]
files += [p for p in root.rglob('*') if p.is_file() and p.name != 'manifest.json']
manifest = {
    'schema_version': 1, 'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'platform': platform.platform(), 'architecture': platform.machine(),
    'evidence_kind': 'artifact_compile_sign_and_in_process_ABI_only',
    'native_HAL_PCM': 'BLOCKED', 'producer_authentication': 'BLOCKED', 'production_apply': False,
    'sha256': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(files)},
}
(root / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
PY
echo 'PASS: compiled/ad-hoc-signed artifacts and in-process Apple ABI. Native HAL PCM/authentication BLOCKED.'
