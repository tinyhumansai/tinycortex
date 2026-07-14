#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps

for feature in tokio git-diff sync; do
  cargo test --all-targets --no-default-features --features "$feature"
done
cargo test --all-targets --no-default-features
