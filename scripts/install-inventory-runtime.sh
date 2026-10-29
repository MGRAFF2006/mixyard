#!/usr/bin/env bash
set -euo pipefail
# Explicit local installation of user-selected build artifacts. Nothing starts.
if [[ $(uname -s) != Darwin || $# != 3 || $3 != /* ]]; then
  echo 'Usage on macOS: install-inventory-runtime.sh /path/runtime /path/helper /absolute/new-install-directory' >&2
  exit 2
fi
runtime=$1
helper=$2
destination=$3
for source in "$runtime" "$helper"; do
  [[ -f $source && ! -L $source ]] || { echo 'Source must be a regular local build artifact' >&2; exit 2; }
done
umask 077
mkdir -m 700 -- "$destination" # Refuse every existing path, including symlinks.
chmod -N "$destination" # No inherited ACL grants beyond the mode bits.
cp -- "$runtime" "$destination/mixyard-runtime"
cp -- "$helper" "$destination/mixyard-macos-inventory"
chmod -N "$destination/mixyard-runtime" "$destination/mixyard-macos-inventory"
chmod 500 "$destination/mixyard-runtime" "$destination/mixyard-macos-inventory"
printf '%s\n' "$destination/mixyard-runtime"
