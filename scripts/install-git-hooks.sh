#!/usr/bin/env sh
set -eu

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"

git config --local core.hooksPath .githooks

echo "Git hooks enabled from .githooks"
