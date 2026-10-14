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
writer_link=()
if [[ $(uname -s) == Linux ]]; then
  writer_link=(-Wl,--wrap=fwrite -Wl,--wrap=fflush -Wl,--wrap=fseek -Wl,--wrap=fclose)
fi
"${CXX:-c++}" -std=c++17 -O1 -g -Wall -Wextra -Werror -pthread \
  -fsanitize=address,undefined -fno-omit-frame-pointer writer_selfcheck.cpp \
  "${writer_link[@]}" -o "$scratch/writer-selfcheck"
python3 - "$scratch/writer-selfcheck" "$scratch" <<'PY'
import json
import pathlib
import platform
import struct
import subprocess
import sys
import time

binary, scratch = sys.argv[1:]
modes = ['normal', 'create-error']
if platform.system() == 'Linux':
    modes += ['write-error', 'flush-error', 'close-error', 'write-block',
              'flush-block', 'seek-block', 'header-block', 'close-block', 'destructor-block']
for mode in modes:
    wav = pathlib.Path(scratch) / (mode + '.wav')
    if mode == 'create-error':
        wav.write_bytes(b'existing file must survive')
    begin = time.monotonic()
    child = subprocess.run([binary, mode, str(wav)],
                           capture_output=True, text=True, timeout=5)
    elapsed = time.monotonic() - begin
    if mode.endswith('-block'):
        assert child.returncode == 1, (mode, child.returncode, child.stderr)
        result = json.loads(child.stdout)
        assert result['capabilities']['processCapture'] is False
        assert [d['code'] for d in result['diagnostics']] == ['disk-write-timeout', 'capture-unqualified']
        assert 1.9 <= elapsed < 5, (mode, elapsed)
        assert not child.stderr, child.stderr
        print(f'PASS: isolated {mode} reported unqualified failure and exited in {elapsed:.2f}s')
    else:
        assert child.returncode == 0, (mode, child.returncode, child.stderr)
        assert not child.stderr, child.stderr
        if mode == 'normal':
            data = wav.read_bytes()
            assert len(data) == 72 and struct.unpack_from('<I', data, 4)[0] == 64
            assert struct.unpack_from('<I', data, 44)[0] == 2
            assert struct.unpack_from('<I', data, 52)[0] == 16
            assert data[56:] == struct.pack('<4f', 0.25, -0.25, 0.5, -0.5)
        if mode == 'create-error':
            assert wav.read_bytes() == b'existing file must survive'
        print(child.stdout, end='')
if platform.system() != 'Linux':
    print('UNAVAILABLE: libc stall injection uses the Linux linker; normal portable writer check passed')
PY
python3 - <<'PY'
import plistlib
with open('Info.plist', 'rb') as f:
    p = plistlib.load(f)
assert p['CFBundleIdentifier'] == 'org.mixyard.coreaudio-probe'
assert p['NSAudioCaptureUsageDescription']
print('PASS: capture usage-description plist parses; macOS privacy behavior remains pending')
PY
bash -n build.sh check.sh
