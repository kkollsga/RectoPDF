#!/usr/bin/env bash
# check: the one local check, in one heavy job.
#
#   tools/check.sh --quick   Changed crates' tests, clippy for the workspace, a Windows compile check (while working)
#   tools/check.sh --full    Every test in the workspace, clippy, Windows compile check (before handing back)
set -euo pipefail
cd "$(dirname "$0")/.."
mode="${1:---quick}"
case "$mode" in
  --quick)
    changed=$( { git diff --name-only HEAD 2>/dev/null; git ls-files --others --exclude-standard 2>/dev/null; } | sed -n 's#^crates/\([^/]*\)/.*#\1#p' | sort -u)
    tests=()
    if [ -n "$changed" ]; then for c in $changed; do tests+=(-p "$c"); done; else tests=(--workspace); fi
    ;;
  --full) tests=(--workspace) ;;
  *) sed -n '2,6p' "$0"; exit 2 ;;
esac
run() {
  set -e
  cargo clippy --workspace --all-targets --locked -- -D warnings
  # Tests start the worker binary, and `-p` alone wouldn't build it.
  cargo build --locked -p recto-render-service
  cargo nextest run --locked "${tests[@]}"
  # Keeps the core buildable for Windows from the first commit. Skipped only if the target isn't installed.
  if rustup target list --installed | grep -q x86_64-pc-windows-gnu; then
    cargo check --workspace --locked --target x86_64-pc-windows-gnu
  fi
}
# PDFium's digest-checked library, which the render tests need. Does nothing if it's already there.
tools/pdfium.sh >/dev/null
export -f run
export tests_joined="${tests[*]}"
tools/sysguard/heavy.sh bash -c 'tests=($tests_joined); run'
