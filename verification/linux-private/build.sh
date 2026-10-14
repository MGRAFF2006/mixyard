#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
check_disk() {
  local proof_available
  proof_available="$(df --output=avail -B1 . | tail -1)" || return
  if ! [[ "$proof_available" =~ ^[[:space:]]*[0-9]+[[:space:]]*$ ]] ||
     (( proof_available < 10 * 1024 * 1024 * 1024 )); then
    echo "Build requires at least 10737418240 available bytes; df reported: $proof_available" >&2
    return 1
  fi
}
trap 'proof_status=$?; check_disk || proof_status=1; exit "$proof_status"' EXIT
check_disk
docker build --pull --tag mixyard-native-proof:local -f verification/linux-private/Dockerfile verification/linux-private
