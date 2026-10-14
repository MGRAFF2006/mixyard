#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
test "$(df --output=avail -BG . | tail -1 | tr -dc '0-9')" -ge 10
proof_evidence="${1:?usage: run.sh NEW-EVIDENCE-DIRECTORY}"
mkdir -p "$(dirname "$proof_evidence")"
mkdir -m 700 "$proof_evidence"
shift
proof_evidence="$(realpath "$proof_evidence")"
docker image inspect mixyard-native-proof:local --format '{{.Id}}' > "$proof_evidence/image-id.txt"
proof_status=0
docker run --rm --name mixyard-native-proof-run --network none --cpus 2 --memory 1g \
  --pids-limit 128 --cap-drop ALL --security-opt no-new-privileges --read-only \
  --tmpfs /tmp:rw,nosuid,nodev,size=256m \
  --mount "type=bind,src=$PWD,dst=/work,readonly" \
  --mount "type=bind,src=$proof_evidence,dst=/evidence" \
  mixyard-native-proof:local python3 -B verification/linux_private_graph.py /evidence "$@" || proof_status=$?
df -h .
test "$(df --output=avail -BG . | tail -1 | tr -dc '0-9')" -ge 10
exit "$proof_status"
