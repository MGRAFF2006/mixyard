#!/usr/bin/env bash
set -euo pipefail
# Only disposable hosted runners install SDKs. This script never installs any.
: "${MIXYARD_NATIVE_BUILD:?set a private build directory}"
root=$(cd -- "$(dirname -- "$0")/../../.." && pwd)
cc -std=gnu11 -Wall -Wextra -Werror -O2 -g0 \
  $(pkg-config --cflags libpipewire-0.3 json-c) \
  -I"$root/adapters/engine/include" \
  -c "$root/adapters/linux/native-host/graph.c" \
  -o "$MIXYARD_NATIVE_BUILD/native-graph.o"
export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
export CARGO_TARGET_DIR="$MIXYARD_NATIVE_BUILD/rust"
cargo build --locked --manifest-path "$root/adapters/engine/Cargo.toml"
cc "$MIXYARD_NATIVE_BUILD/native-graph.o" \
  "$CARGO_TARGET_DIR/debug/libmixyard_host.a" \
  -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free,--wrap=posix_memalign,--wrap=aligned_alloc \
  $(pkg-config --libs libpipewire-0.3 json-c) -lpthread -ldl -lm -lrt -lutil \
  -o "$MIXYARD_NATIVE_BUILD/native-graph"
