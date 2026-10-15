#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
check_disk() { local free; free=$(df -B1 --output=avail . | tail -1); (( free >= 10737418240 )) || { echo "BLOCKED: $free bytes free" >&2; return 1; }; }
check_disk
trap 'proof_exit=$?; check_disk || proof_exit=2; exit "$proof_exit"' EXIT
evidence=${1:?usage: run-diagnostic.sh NEW-EVIDENCE-DIRECTORY [--config PATH] [--capture]}
shift
mkdir -p "$(dirname "$evidence")"
mkdir -m700 "$evidence"
evidence=$(realpath "$evidence")
mkdir -m700 "$evidence/tmp" "$evidence/private"
mkdir "$evidence/inputs"
# Keep the actual source and binaries even when later diagnostic builds replace
# the shared retained build outputs. All paths are relative to this worktree.
cp -L --parents adapters/linux/server-proof/module.c verification/linux-server/{fixture.c,diagnostic.py,run-diagnostic.sh,pipewire.conf,compile.sh} \
  verification/linux_server_ingress.py verification/linux-server/build/{fixture,modules/libpipewire-module-mixyard-server-proof.so,install/bin/pipewire,install/bin/pw-dump,install/bin/pw-link,install/lib/libpipewire-0.3.so.0.1609.0,source/build/config.h} \
  "$evidence/inputs"
docker image inspect mixyard-server-proof:local --format '{{.Id}}' > "$evidence/image-id"
docker run --rm --name mixyard-server-diagnostic --network none --cpus 2 --memory 1g \
  --pids-limit 128 --cap-drop ALL --security-opt no-new-privileges --read-only \
  --env "PROOF_SOURCE_REVISION=$(git rev-parse HEAD)" --env TMPDIR=/tmp \
  --user "$(id -u):$(id -g)" \
  --mount "type=bind,src=$evidence/tmp,dst=/tmp" \
  --mount "type=bind,src=$evidence/private,dst=/private" \
  --mount "type=bind,src=$PWD,dst=/work,readonly" \
  --mount "type=bind,src=$evidence,dst=/evidence" \
  mixyard-server-proof:local python3 -B verification/linux-server/diagnostic.py /evidence "$@"
