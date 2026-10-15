#!/usr/bin/env bash
set -euo pipefail
# Run ONLY in the coordinated build container, after the matching server build.
source_dir=${1:?exact PipeWire 1.6.9 source directory required}
output_dir=${2:?module output directory required}
adapter_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
[[ $(df -PB1 "$output_dir" | awk 'NR == 2 {print $4}') -ge 10737418240 ]]
[[ -f "$source_dir/build/config.h" && -f "$source_dir/build/src/pipewire/version.h" ]]
grep -q "version : '1.6.9'" "$source_dir/meson.build"
# Pin the internal layout and the five code paths on which this proof depends.
(cd "$source_dir" && sha256sum --check <<'HASHES'
68bcb44b55a560347fd39422577e2ffa4454ef8513a054ada7eab2d029976931  src/pipewire/private.h
ac4ae3f974239842ea125f61f5666c11db6549f5933252be204058f474478eba  src/pipewire/global.c
e193b599c362376d68e52cb55b4799b66e0e7547f1d16b208918c0ab80b0ad72  src/pipewire/impl-node.c
a54fa55de1c4dd6b1f376ae85058a63c8b7633e79ae73ce8b8754f8b1a06f70a  src/pipewire/impl-port.c
ccd918ac0d4904b36425c918d95b86d0998b2dded6033b471553e9b1c7e80e04  src/pipewire/impl-link.c
HASHES
)
cc -std=gnu11 -D_GNU_SOURCE -O2 -g -fPIC -shared -Wall -Wextra -Werror \
  -Wno-unused-parameter -Wno-missing-field-initializers \
  -I"$source_dir/src" -I"$source_dir/spa/include" \
  -I"$source_dir/build" -I"$source_dir/build/src" \
  "$adapter_dir/module.c" -o "$output_dir/libpipewire-module-mixyard-server-proof.so"
# Compile success permits hidden daemon symbols. Resolve every relocation now
# against the matching build, without invoking pipewire__module_init or a daemon.
python3 - "$source_dir/build/src/pipewire/libpipewire-0.3.so" \
  "$output_dir/libpipewire-module-mixyard-server-proof.so" <<'PY'
import ctypes
import os
import sys
from pathlib import Path

library, module = (str(Path(arg).resolve(strict=True)) for arg in sys.argv[1:])
server = ctypes.CDLL(library, mode=os.RTLD_NOW | os.RTLD_GLOBAL)
server.pw_get_library_version.restype = ctypes.c_char_p
if server.pw_get_library_version() != b"1.6.9":
    raise SystemExit("module load audit requires the matching PipeWire 1.6.9 library")
ctypes.CDLL(module, mode=os.RTLD_NOW | os.RTLD_LOCAL)
print("module load audit: all relocations resolved against matching PipeWire 1.6.9")
PY
