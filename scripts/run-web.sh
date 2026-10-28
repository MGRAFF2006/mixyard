#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$project_root"

if ! command -v pnpm >/dev/null 2>&1; then
  echo 'Mixyard web requires Node.js 24 and pnpm 11 to build the interface.' >&2
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo 'Mixyard web requires the Rust toolchain to build the local service.' >&2
  exit 1
fi

pnpm --dir apps/web install --frozen-lockfile
pnpm --dir apps/web build
exec cargo run --locked --package mixyard --bin mixyard-web -- "$@"
