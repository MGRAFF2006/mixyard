#!/usr/bin/env bash
set -euo pipefail
# Only disposable hosted runners install SDKs. This script never installs any.
: "${MIXYARD_NATIVE_BUILD:?set a private build directory}"
root=$(cd -- "$(dirname -- "$0")/../../.." && pwd)
if [[ -n "${MIXYARD_COMPARISON_SDK:-}" ]]; then
  python3 -B "$root/adapters/linux/native-host/comparison_sdk.py" validate-build "$MIXYARD_COMPARISON_SDK"
fi
pkg-config --modversion libpipewire-0.3
cc -std=c11 -Wall -Wextra -Werror -pedantic -O2 -g0 \
  -I"$root/adapters/engine/include" "$root/adapters/linux/native-host/consumer_copy_check.c" \
  -o "$MIXYARD_NATIVE_BUILD/consumer-copy-check"
"$MIXYARD_NATIVE_BUILD/consumer-copy-check"
# Exercise the actual SPA request/parser contract before creating any native context.
cc -std=gnu11 -Wall -Wextra -Werror -O2 -g0 \
  $(pkg-config --cflags libpipewire-0.3) \
  "$root/adapters/linux/native-host/native_format_check.c" \
  -o "$MIXYARD_NATIVE_BUILD/native-format-check"
"$MIXYARD_NATIVE_BUILD/native-format-check"
# Public mapping offset contract: fresh owned file, read-only mappings, pread oracle.
# An offset mismatch fails visibly before graph execution; it is never skipped.
cc -std=gnu11 -Wall -Wextra -Werror -O2 -g0 \
  $(pkg-config --cflags libpipewire-0.3) \
  "$root/adapters/linux/native-host/native_map_check.c" \
  $(pkg-config --libs libpipewire-0.3) -ldl \
  -o "$MIXYARD_NATIVE_BUILD/native-map-check"
if "$MIXYARD_NATIVE_BUILD/native-map-check" >"$MIXYARD_NATIVE_BUILD/native-map-check.json"; then
  cat "$MIXYARD_NATIVE_BUILD/native-map-check.json"
else
  result=$?
  cat "$MIXYARD_NATIVE_BUILD/native-map-check.json"
  exit "$result"
fi
if [[ -n "${MIXYARD_COMPARISON_SDK:-}" ]]; then
  python3 -B "$root/adapters/linux/native-host/comparison_sdk.py" verify-api \
    "$MIXYARD_COMPARISON_SDK" "$MIXYARD_NATIVE_BUILD/native-map-check.json"
fi
# Real plugin pool/alias counterexample; no context, server connection or devices.
cc -std=gnu11 -Wall -Wextra -Werror -O2 -g0 \
  $(pkg-config --cflags libpipewire-0.3) \
  -I"$root/adapters/engine/include" \
  "$root/adapters/linux/native-host/native_pool_check.c" \
  $(pkg-config --libs libpipewire-0.3) \
  -o "$MIXYARD_NATIVE_BUILD/native-pool-check"
"$MIXYARD_NATIVE_BUILD/native-pool-check"
cc -std=gnu11 -Wall -Wextra -Werror -O2 -g0 \
  $(pkg-config --cflags libpipewire-0.3 json-c) \
  -I"$root/adapters/engine/include" \
  -c "$root/adapters/linux/native-host/graph.c" \
  -o "$MIXYARD_NATIVE_BUILD/native-graph.o"
export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
export CARGO_TARGET_DIR="$MIXYARD_NATIVE_BUILD/rust"
cargo build --locked --manifest-path "$root/adapters/engine/Cargo.toml" --release
cc "$MIXYARD_NATIVE_BUILD/native-graph.o" \
  "$CARGO_TARGET_DIR/release/libmixyard_host.a" \
  -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free,--wrap=posix_memalign,--wrap=aligned_alloc \
  $(pkg-config --libs libpipewire-0.3 json-c) -lpthread -ldl -lm -lrt -lutil \
  -o "$MIXYARD_NATIVE_BUILD/native-graph"

cc -std=c11 -Wall -Wextra -Werror -O2 -g0 \
  -I"$root/adapters/engine/include" "$root/adapters/linux/native-host/host_oracle.c" \
  "$CARGO_TARGET_DIR/release/libmixyard_host.a" \
  -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free \
  -lpthread -ldl -lm -lrt -lutil -o "$MIXYARD_NATIVE_BUILD/host-oracle"
