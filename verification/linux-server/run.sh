#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
check_disk() { local free; free=$(df -B1 --output=avail . | tail -1); (( free >= 10737418240 )) || { echo "BLOCKED: $free bytes free" >&2; return 1; }; }
check_disk
trap 'proof_exit=$?; check_disk || proof_exit=2; exit "$proof_exit"' EXIT
evidence=${1:?usage: run.sh NEW-EVIDENCE-DIRECTORY}
mkdir -p "$(dirname "$evidence")"
mkdir -m700 "$evidence"
evidence=$(realpath "$evidence")
docker image inspect mixyard-server-proof:local --format '{{.Id}}' > "$evidence/image-id"
docker run --rm --name mixyard-server-proof-run --network none --cpus 2 --memory 1g \
  --pids-limit 128 --cap-drop ALL --security-opt no-new-privileges --read-only \
  --env "PROOF_SOURCE_REVISION=$(git rev-parse HEAD)" \
  --user "$(id -u):$(id -g)" --tmpfs /tmp:rw,nosuid,nodev,size=256m \
  --tmpfs /private:rw,nosuid,nodev,size=8m,mode=0700,uid="$(id -u)",gid="$(id -g)" \
  --mount "type=bind,src=$PWD,dst=/work,readonly" \
  --mount "type=bind,src=$evidence,dst=/evidence" \
  mixyard-server-proof:local python3 -B verification/linux_server_ingress.py /evidence
