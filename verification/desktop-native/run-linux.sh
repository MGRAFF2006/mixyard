#!/usr/bin/env bash
set -euo pipefail

# Always acquire our own X server, WM, session bus and data directories.
# Never attach to the caller's desktop, bus, audio sockets or live runtime.
if [[ $# != 5 ]]; then
  echo "Usage: $0 DESKTOP_BINARY RUNTIME_BINARY CLI_BINARY WEB_BINARY EVIDENCE_DIRECTORY" >&2
  exit 2
fi
binary=$(realpath "$1")
runtime=$(realpath "$2")
cli=$(realpath "$3")
web=$(realpath "$4")
output=$(realpath -m "$5")
mkdir -p "$output"
root=$(cd "$(dirname "$0")" && pwd)
python3 "$root/evidence.py" --guard
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export HOME="$scratch" XDG_CONFIG_HOME="$scratch/config" XDG_DATA_HOME="$scratch/data"
export XDG_CACHE_HOME="$scratch/cache" XDG_RUNTIME_DIR="$scratch/run"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS PULSE_SERVER PIPEWIRE_REMOTE
export GDK_BACKEND=x11 LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1
export MIXYARD_NATIVE_ISOLATED=1 MIXYARD_DESKTOP_NO_TRAY=1
xvfb-run -a -s '-screen 0 1280x960x24 -nolisten tcp' dbus-run-session -- \
  python3 "$root/webdriver.py" "$binary" "$runtime" "$cli" "$web" "$output"
