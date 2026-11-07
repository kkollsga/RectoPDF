#!/usr/bin/env bash
# install: puts the repo's hooks in place. Shared by every worktree of this checkout.
set -euo pipefail
cd "$(dirname "$0")/../.."
hooks="$(git rev-parse --git-common-dir)/hooks"
mkdir -p "$hooks"
ln -sf "$(pwd)/tools/hooks/pre-push" "$hooks/pre-push"
echo "pre-push installed in $hooks"
