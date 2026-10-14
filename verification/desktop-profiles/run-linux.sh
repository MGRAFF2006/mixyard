#!/usr/bin/env bash
set -euo pipefail
if [[ $# != 3 ]]; then
  echo "Usage: $0 RELEASE_DESKTOP_BINARY RELEASE_CLI EVIDENCE_DIRECTORY" >&2
  exit 2
fi
binary=$(realpath "$1")
cli=$(realpath "$2")
output=$(realpath -m "$3")
root=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$output"
python3 "$root/../desktop-native/evidence.py" --guard
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export XDG_CONFIG_HOME="$scratch/config" XDG_DATA_HOME="$scratch/data"
export XDG_CACHE_HOME="$scratch/cache" XDG_RUNTIME_DIR="$scratch/run"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS DBUS_SYSTEM_BUS_ADDRESS PULSE_SERVER PIPEWIRE_REMOTE
export GDK_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1
export MIXYARD_PROFILES_ISOLATED=1
xvfb-run -a -s '-screen 0 1280x960x24 -nolisten tcp' dbus-run-session -- \
  python3 "$root/webview.py" "$binary" "$cli" "$output"
