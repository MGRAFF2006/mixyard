#!/usr/bin/env bash
set -euo pipefail
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_OS:-}" == macOS ]] || { echo 'Owned hosted macOS qualifier required' >&2; exit 1; }
task_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
: "${MIXYARD_NATIVE_FIXTURE:?Owned fixture directory is required}"
# No Security CLI, sudo, trust settings, keychain or search-list preparation.
# Public per-challenge anchors are the unchanged production ControlClient path.
exec "$task_root/swift/.build/native-control-qualification"
