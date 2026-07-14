#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

git submodule update --init --recursive

if command -v rustup >/dev/null 2>&1; then
  rustup toolchain install stable --profile minimal --component rustfmt,clippy
fi

cargo fetch --locked
echo "TinyCortex development dependencies are ready."
