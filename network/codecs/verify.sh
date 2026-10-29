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
links=()
for codec in ZSTD OPUS; do
    kind_var="MIXYARD_${codec}_LINK_KIND"
    name_var="MIXYARD_${codec}_LIB_NAME"
    dir_var="MIXYARD_${codec}_LIB_DIR"
    kind=${!kind_var-dylib}
    default_name=zstd
    if [[ $codec == OPUS ]]; then default_name=opus; fi
    name=${!name_var-$default_name}
    [[ $kind == dylib || $kind == static ]] || { echo "Invalid $kind_var" >&2; exit 1; }
    [[ $name =~ ^[a-zA-Z0-9_-]+$ ]] || { echo "Invalid $name_var" >&2; exit 1; }
    if [[ -n ${!dir_var+x} ]]; then
        [[ ${!dir_var} == /* && -d ${!dir_var} ]] || { echo "Invalid $dir_var" >&2; exit 1; }
        [[ ${!dir_var} != *$'\n'* && ${!dir_var} != *$'\r'* ]] || { echo "Invalid $dir_var" >&2; exit 1; }
        file="lib$name.so"
        if [[ $(uname -s) == Darwin ]]; then file="lib$name.dylib"; fi
        if [[ $kind == static ]]; then file="lib$name.a"; fi
        [[ -f ${!dir_var}/$file ]] || { echo "$dir_var is missing selected library $file" >&2; exit 1; }
        links+=(-L "native=${!dir_var}")
    fi
    links+=(-l "$kind=$name")
done
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
