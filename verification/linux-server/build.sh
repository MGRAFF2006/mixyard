#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
reserve=10737418240
check_disk() { local free; free=$(df -B1 --output=avail . | tail -1); (( free >= reserve )) || { echo "BLOCKED: $free bytes free; require $reserve" >&2; exit 2; }; }
check_disk
trap check_disk EXIT
mkdir -p verification/linux-server/build
commit=8fa27cabdc6c0c1350c69c026af5850ef0af1e26
archive=verification/linux-server/build/pipewire.tar.gz
if [[ ! -f $archive ]]; then curl --fail --location "https://codeload.github.com/PipeWire/pipewire/tar.gz/$commit" -o "$archive"; fi
echo "67929f63e6d75d7d058d94a5c7e3595478ecbc4034c64187b41683d300c22107  $archive" | sha256sum --check
if [[ ! -d verification/linux-server/build/source ]]; then
    tar -xf "$archive" -C verification/linux-server/build
    mv "verification/linux-server/build/pipewire-$commit" verification/linux-server/build/source
fi
docker build --tag mixyard-server-proof:local verification/linux-server
docker run --rm --network none --cap-drop ALL --security-opt no-new-privileges --cpus 2 --memory 2g \
    --user "$(id -u):$(id -g)" -v "$PWD:/work" mixyard-server-proof:local bash verification/linux-server/compile.sh
docker image inspect mixyard-server-proof:local --format '{{.Id}}' > verification/linux-server/build/image-id
