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
# Use the same selected-artifact validation and private link names as Cargo.
# Parsing directives as data avoids eval or a second, divergent linker policy.
mkdir -p -- "$target_dir/selected-libraries"
rustc --edition 2024 build.rs -D warnings -C debuginfo=0 -C opt-level=s \
    -C strip=symbols -o "$target_dir/link-setup"
target_os=linux
if [[ $(uname -s) == Darwin ]]; then target_os=macos; fi
CARGO_FEATURE_SYSTEM_ZSTD=1 CARGO_FEATURE_SYSTEM_OPUS=1 \
    CARGO_CFG_TARGET_OS=$target_os CARGO_CFG_TARGET_ENV= \
    OUT_DIR="$target_dir/selected-libraries" "$target_dir/link-setup" > "$target_dir/link-directives.txt"
links=()
while IFS= read -r directive; do
    case $directive in
        cargo:rustc-link-search=*) links+=(-L "${directive#cargo:rustc-link-search=}") ;;
        cargo:rustc-link-lib=*) links+=(-l "${directive#cargo:rustc-link-lib=}") ;;
    esac
done < "$target_dir/link-directives.txt"
rustc --edition 2024 --test src/lib.rs "${features[@]}" "${links[@]}" -D warnings -C debuginfo=0 -o "$target_dir/tests"
"$target_dir/tests" --test-threads=1
rustc --edition 2024 --test src/lib.rs -D warnings -C debuginfo=0 -o "$target_dir/tests"
"$target_dir/tests" --test-threads=1
rustc --edition 2024 --crate-type rlib --crate-name mixyard_network_codecs src/lib.rs \
    "${features[@]}" "${links[@]}" -D warnings -C debuginfo=0 -C opt-level=2 -o "$target_dir/libmixyard_network_codecs.rlib"
rustc --edition 2024 examples/loopback.rs "${features[@]}" -D warnings -C debuginfo=0 -C opt-level=2 \
    "${links[@]}" --extern "mixyard_network_codecs=$target_dir/libmixyard_network_codecs.rlib" -o "$target_dir/loopback"
"$target_dir/loopback"
du -sh -- "$target_dir"
