#!/usr/bin/env bash
set -euo pipefail

# Caller owns this private directory and its build/resource authorization.
target_dir=${1:?Usage: verify.sh /tmp/your-private-codec-target}
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
mkdir -p -- "$target_dir"
target_dir=$(cd -- "$target_dir" && pwd)
cd -- "$source_dir"

pkg-config --atleast-version=1.4.0 libzstd
pkg-config --atleast-version=1.3.0 opus
features=(--cfg 'feature="system-zstd"' --cfg 'feature="system-opus"')
rustc --edition 2024 --test src/lib.rs "${features[@]}" -D warnings -C debuginfo=0 -o "$target_dir/tests"
"$target_dir/tests" --test-threads=1
rustc --edition 2024 --test src/lib.rs -D warnings -C debuginfo=0 -o "$target_dir/tests"
"$target_dir/tests" --test-threads=1
rustc --edition 2024 --crate-type rlib --crate-name mixyard_network_codecs src/lib.rs \
    "${features[@]}" -D warnings -C debuginfo=0 -C opt-level=2 -o "$target_dir/libmixyard_network_codecs.rlib"
rustc --edition 2024 examples/loopback.rs "${features[@]}" -D warnings -C debuginfo=0 -C opt-level=2 \
    --extern "mixyard_network_codecs=$target_dir/libmixyard_network_codecs.rlib" -o "$target_dir/loopback"
"$target_dir/loopback"
du -sh -- "$target_dir"
