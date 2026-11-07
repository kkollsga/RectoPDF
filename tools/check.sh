#!/usr/bin/env bash
# check: the one local check, cheapest step first, stopping at the first failure.
#
#   tools/check.sh --quick    Changed crates' tests, clippy for the workspace, a Windows compile check (while working)
#   tools/check.sh --failed   Only the tests that failed last time in this folder (while fixing them)
#   tools/check.sh --full     Clippy, the tests of crates changed since main, every other test, a Windows compile check
#                             (before handing back). A pass on a clean tree is what lets `main` be pushed.
#
# The queue (tools/sysguard/heavy.sh) refuses to run a check again on code that hasn't changed since it failed.
set -euo pipefail
cd "$(dirname "$0")/.."
mode="${1:---quick}"
# A changed folder under crates/ that the workspace builds. A deleted crate or one kept out of the workspace would stop cargo.
crates_in() {
  local members
  members=$(cargo metadata --no-deps --format-version 1 --locked 2>/dev/null | grep -o '"name":"[^"]*"' | cut -d'"' -f4 | sort -u)
  sed -n 's#^crates/\([^/]*\)/.*#\1#p' | sort -u | grep -Fxf <(echo "$members") || true
}
tests_first=""
case "$mode" in
  --quick)
    changed=$( { git diff --name-only HEAD 2>/dev/null; git ls-files --others --exclude-standard 2>/dev/null; } | crates_in)
    tests=()
    if [ -n "$changed" ]; then for c in $changed; do tests+=(-p "$c"); done; else tests=(--workspace); fi
    ;;
  --full)
    tests=(--workspace)
    # What this work changed is what's most likely to fail, so it runs first, and the rest skip it.
    base=$(git merge-base main HEAD 2>/dev/null || true)
    if [ -n "$base" ]; then
      changed=$( { git diff --name-only "$base" 2>/dev/null; git ls-files --others --exclude-standard 2>/dev/null; } | crates_in)
      for c in $changed; do
        tests_first="$tests_first -p $c"
        tests+=(--exclude "$c")
      done
    fi
    ;;
  --failed)
    log=$(tools/sysguard/heavy.sh --last-failure) || exit 1
    names=$(sed -nE 's/^ *(FAIL|SIGKILL|SIGABRT|SIGSEGV|TIMEOUT|ABORT) +\[[^]]*\] +(\([^)]*\) +)?[^ ]+ +([^ ]+) *$/\3/p' "$log" | sort -u)
    if [ -z "$names" ]; then
      echo "That failure has no failed test in it, so there's nothing narrower to run. Saved output: $log" >&2
      exit 1
    fi
    filter=""
    for name in $names; do filter="$filter${filter:+ | }test(=$name)"; done
    tools/pdfium.sh >/dev/null
    exec env HEAVY_CLASS=quick tools/sysguard/heavy.sh cargo nextest run --locked --no-fail-fast --workspace -E "$filter"
    ;;
  *) sed -n '2,8p' "$0"; exit 2 ;;
esac
run() {
  set -e
  cargo clippy --workspace --all-targets --locked -- -D warnings
  # Tests start the worker and the indexer, and `-p` alone wouldn't build them.
  cargo build --locked -p recto-render-service -p recto-indexer
  if [ -n "$tests_first" ]; then
    # shellcheck disable=SC2086
    cargo nextest run --locked $tests_first
  fi
  cargo nextest run --locked --no-tests=pass "${tests[@]}"
  # Keeps the core buildable for Windows from the first commit. Skipped only if the target isn't installed.
  if rustup target list --installed | grep -q x86_64-pc-windows-gnu; then
    cargo check --workspace --locked --target x86_64-pc-windows-gnu
  fi
}
# PDFium's digest-checked library, which the render tests need. Does nothing if it's already there.
tools/pdfium.sh >/dev/null
export -f run
export tests_joined="${tests[*]}"
export tests_first
tools/sysguard/heavy.sh bash -c 'tests=($tests_joined); run'
# Only a commit that passed, with nothing uncommitted, counts for the push guard.
if [ "$mode" = --full ] && [ -z "$(git status --porcelain)" ]; then
  git rev-parse HEAD >> "$(git rev-parse --git-common-dir)/recto-checked"
fi
