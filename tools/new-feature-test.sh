#!/usr/bin/env bash
# new-feature-test: proves tools/new-feature.sh on a copy of the tree, and tools/doc-paths.sh on a page made to fail. Run after changing either; check.sh
# runs it in a full run when they change (tool_tests_for, tools/check-scope.sh).
#
#   tools/new-feature-test.sh            with the build: clippy on the two crates the scaffold touches, through the queue (about a minute warm)
#   NEW_FEATURE_TEST_BUILD=0 tools/new-feature-test.sh    everything but the build, in a few seconds
#
# The copy is the committed tree with the working copies of the scripts under test, at one fixed place under target/ (so cargo's artifacts for it are
# reused run to run, not made anew under a new name each time), and goes when the test ends.
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
target_dir=${CARGO_TARGET_DIR:-target}
case "$target_dir" in /*) ;; *) target_dir="$SOURCE/$target_dir" ;; esac
place="$target_dir/new-feature-test"
tree="$place/tree"
trap 'rm -rf "$tree"' EXIT
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t GIT_CONFIG_GLOBAL=/dev/null
fails=0
check() { # name, expected, actual
  if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi
}

rm -rf "$tree" && mkdir -p "$tree"
git -C "$SOURCE" archive HEAD | tar -x -C "$tree"
for script in new-feature.sh doc-paths.sh doc-paths.py; do cp "$SOURCE/tools/$script" "$tree/tools/$script"; done
cd "$tree" || exit 1
git init -q && git add -A && git commit -qm base

# Failure: the scaffold leaves a tree that doesn't build, isn't in rustfmt's layout, or touches more than its two files and two marked lines.
out=$(tools/new-feature.sh demo_feature 2>&1); rc=$?
check "it makes a feature" "0" "$rc"
check "its two files" "yes yes" "$([ -f crates/recto-agent/src/commands/demo_feature.rs ] && echo yes) $([ -f crates/recto-app/src/core/headless/demo_feature.rs ] && echo yes)"
check "and one marked line in each of two shared files, nothing else" \
  "crates/recto-agent/src/commands/mod.rs crates/recto-app/src/core/headless/mod.rs" "$(git diff --name-only | tr '\n' ' ' | sed 's/ $//')"
check "each a mod line" "1 1" "$(git diff -U0 crates/recto-agent/src/commands/mod.rs | grep -c '^+pub mod demo_feature; // new-feature: demo_feature') $(git diff -U0 crates/recto-app/src/core/headless/mod.rs | grep -c '^+mod demo_feature; // new-feature: demo_feature')"
check "the tree is in rustfmt's layout" "0" "$(cargo fmt --all --check >/dev/null 2>&1; echo $?)"
check "a test at each tier says what to fill in" "4" "$(cat crates/recto-agent/src/commands/demo_feature.rs crates/recto-app/src/core/headless/demo_feature.rs | grep -c 'fn fill_me_in_demo_feature_')"
case "$out" in *"docs/FEATURES.md"*"tools/new-feature.sh --remove demo_feature"*) check "it prints the checklist" ok ok ;; *) check "it prints the checklist" ok "$out" ;; esac
check "a second time is refused, changing nothing" "1 4" "$(tools/new-feature.sh demo_feature >/dev/null 2>&1; echo $?) $(git status --porcelain | grep -c .)"
check "a name that isn't a tag is refused" "2" "$(tools/new-feature.sh Demo-Feature >/dev/null 2>&1; echo $?)"
if [ "${NEW_FEATURE_TEST_BUILD:-1}" != 0 ]; then
  # The two crates it touches, their tests included, with the gate's lints. Through the queue, into the checkout's target so its dependencies are reused.
  built=$(CARGO_TARGET_DIR="$target_dir" HEAVY_CLASS=quick "$SOURCE/tools/sysguard/heavy.sh" \
    cargo clippy --locked -p recto-agent -p recto-app --all-targets -- -D warnings 2>&1); rc=$?
  check "it builds, its tests too, with no lint" "0" "$rc"
  [ "$rc" = 0 ] || printf '%s\n' "$built" | tail -30
fi
tools/new-feature.sh --remove demo_feature >/dev/null
check "--remove leaves the tree as it was" "" "$(git status --porcelain)"

# Failure: a how-to page names a file or a test that a rename took away, and the check says nothing.
printf 'See `crates/recto-agent/src/lib.rs`, `execute` and `tools/check.sh --quick`.\n' > good.md
printf 'See `crates/recto-agent/src/gone.rs` and `a_test_that_was_renamed`.\n' > bad.md
check "doc-paths passes a page that names what's there" "0" "$(tools/doc-paths.sh good.md >/dev/null 2>&1; echo $?)"
said=$(tools/doc-paths.sh bad.md 2>&1); rc=$?
check "and fails one that doesn't" "1" "$rc"
case "$said" in *"bad.md:1: no such file: crates/recto-agent/src/gone.rs"*"no such name in the sources: a_test_that_was_renamed"*) check "naming each, with its line" ok ok ;; *) check "naming each, with its line" ok "$said" ;; esac
check "the pages it holds pass" "0" "$(tools/doc-paths.sh >/dev/null 2>&1; echo $?)"

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
