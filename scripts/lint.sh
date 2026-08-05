#!/usr/bin/env sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repository_root"

cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
