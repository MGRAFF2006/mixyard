#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
test "$(df --output=avail -BG . | tail -1 | tr -dc '0-9')" -ge 10
docker build --pull --tag mixyard-native-proof:local -f verification/linux-private/Dockerfile verification/linux-private
df -h .
