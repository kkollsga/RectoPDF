#!/usr/bin/env bash
# check: the one local check, cheapest step first, stopping at the first failure.
#
#   tools/check.sh --quick    Changed crates' tests, clippy for the workspace, a Windows compile check (while working)
#   tools/check.sh --corpus   The index against the pages over every PDF in RECTO_BENCH100, about a quarter of an hour
#   tools/check.sh --failed   Only the tests that failed last time in this folder (while fixing them)
#   tools/check.sh --full     Clippy, a Windows compile check, the tests of crates changed since main, every other test,
#                             then the app built and driven in quiet runs (before handing back). A pass on a clean tree is what lets `main` be pushed.
#   tools/check.sh --full --why "what this run decides"    A branch gets two full runs. A third needs a stated reason, which goes in the ledger.
#   tools/check.sh --runs     Full runs per branch, from the ledger, and what counts against this branch's budget.
#
# A pass counts for identical code: --full on a clean tree whose code has already passed says so, lets the push guard accept this commit and runs
# nothing. Documents don't count as code, as in tools/hooks/pre-push. CHECK_AGAIN=1 runs it for real anyway. A dirty tree always runs and is never recorded.
# docs/agent-instructions/worker-testing.md says which check answers which question.
#
# The queue (tools/sysguard/heavy.sh) refuses to run a check again on code that hasn't changed since it failed.
set -euo pipefail
cd "$(dirname "$0")/.."
usage() { sed -n '2,10p' "$0"; exit 2; }
mode=""
why=""
why_given=""
while [ $# -gt 0 ]; do
  case "$1" in
    --why)
      [ $# -ge 2 ] || usage
      why=$2
      why_given=1
      shift 2
      ;;
    *)
      [ -z "$mode" ] || usage
      mode=$1
      shift
      ;;
  esac
done
mode="${mode:---quick}"
[ -z "$why_given" ] || [ "$mode" = --full ] || usage
# One line, because the ledger is one line per run.
why=$(printf '%s' "$why" | tr '\t\n' '  ' | sed -E 's/^ +| +$//g' | cut -c1-200)
if [ -n "$why_given" ] && [ -z "$why" ]; then
  echo 'check: --why needs the question this run decides, for example --why "the fix for the review'\''s race holds across the app tests".' >&2
  exit 2
fi

common_dir=$(git rev-parse --git-common-dir)
checked="$common_dir/recto-checked"
# fingerprint, commit, time: a line per pass on a clean tree.
passes="$common_dir/recto-passes"
# time, branch, commit, result, seconds, reason: a line per real --full run. Result is pass, fail:<stage>, or stopped, meaning from outside, which says nothing about the code.
ledger="$common_dir/recto-full-runs"
BUDGET=2

# What decides a result. The tracked files, by content, except documents: this filter is the one in tools/hooks/pre-push. Then what isn't tracked and can
# change a result on the same code: the tools, whether the Windows compile check can run at all, and the settings that change what the tests do.
# Not covered: the real PDFs some tests read if they're there, in another repository. docs/agent-instructions/worker-testing.md lists them.
code_fingerprint() {
  {
    git ls-files -s | awk -F'\t' '$2 !~ /^docs\/|\.md$/'
    cargo nextest --version 2>/dev/null || true
    swift --version 2>/dev/null || true
    sw_vers -productVersion 2>/dev/null || true
    rustup target list --installed 2>/dev/null | grep -c x86_64-pc-windows-gnu || true
    env | grep -E '^(RECTO_|NEXTEST_|RUSTFLAGS=|RUSTDOCFLAGS=|CARGO_PROFILE_)' | sort || true
  } | shasum | cut -d' ' -f1
}
tree_is_clean() { [ -z "$(git status --porcelain)" ]; }
stamp_checked() {
  local head
  head=$(git rev-parse HEAD)
  [ "$(tail -1 "$checked" 2>/dev/null)" = "$head" ] || echo "$head" >> "$checked"
}
branch=$(git symbolic-ref --short -q HEAD || echo detached)

# Whether a ledger line's commit still counts against this branch's budget. On `main`, a budget per range of commits not yet pushed: every merge is new code
# and needs its check, so a count for all time would mean nothing, and a push starts the count again. Elsewhere, runs since the branch left main: a branch
# cut again under an old name, or one that has taken in main, has new code under test.
counts_now() {
  local commit=${1%+dirty} since
  if [ "$branch" = main ]; then
    git rev-parse -q --verify refs/remotes/origin/main >/dev/null || return 0
    ! git merge-base --is-ancestor "$commit" refs/remotes/origin/main 2>/dev/null
  else
    since=$(git merge-base main HEAD 2>/dev/null) || return 0
    # A commit that's gone can't be placed, so it still counts.
    git cat-file -e "$commit" 2>/dev/null || return 0
    git merge-base --is-ancestor "$since" "$commit" 2>/dev/null
  fi
}
# The ledger's lines that count against this branch now, oldest first. A run stopped from outside doesn't.
counted_runs() {
  [ -f "$ledger" ] || return 0
  local time run_branch commit result seconds reason
  while IFS=$'\t' read -r time run_branch commit result seconds reason; do
    [ "$run_branch" = "$branch" ] && [ "$result" != stopped ] || continue
    if counts_now "$commit"; then printf '  %s  %s  %s  %s s  %s\n' "$time" "$(printf '%s' "$commit" | cut -c1-12)" "$result" "$seconds" "$reason"; fi
  done < "$ledger"
}
scope_words() {
  if [ "$branch" = main ]; then echo "on commits not yet pushed"; else echo "since it left main"; fi
}

if [ "$mode" = --runs ]; then
  if [ ! -s "$ledger" ]; then
    echo "No full runs recorded yet."
    exit 0
  fi
  echo "Full runs per branch:"
  awk -F'\t' '
    { n[$2]++; seconds[$2] += $5; if ($4 == "pass") pass[$2]++; else if ($4 == "stopped") stop[$2]++; else fail[$2]++; last[$2] = $1 }
    END { for (b in n) printf "%4d runs, %2d passed, %2d failed, %2d stopped, %6d s, last %s  %s\n", n[b], pass[b], fail[b], stop[b], seconds[b], last[b], b }
  ' "$ledger" | sort -rn
  counted=$(counted_runs)
  count=0
  [ -z "$counted" ] || count=$(printf '%s\n' "$counted" | wc -l | tr -d ' ')
  echo "$branch: $count of $BUDGET count against its budget, $(scope_words)."
  [ -z "$counted" ] || printf '%s\n' "$counted"
  exit 0
fi

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
  --corpus)
    # Every kind of search from the index and from the pages, over a folder of real PDFs. Slow, so not part of --full, which always runs a small version.
    : "${RECTO_BENCH100:?Set RECTO_BENCH100 to a folder of PDFs}"
    tools/pdfium.sh >/dev/null
    RECTO_INDEX_ALL=1 exec tools/sysguard/heavy.sh cargo test --release --locked -p recto-cli --test index real_pages -- --nocapture
    ;;
  --full)
    if [ "${CHECK_AGAIN:-}" != 1 ] && tree_is_clean; then
      known=$(grep "^$(code_fingerprint)"$'\t' "$passes" 2>/dev/null | tail -1 || true)
      if [ -n "$known" ]; then
        passed_commit=$(printf '%s' "$known" | cut -f2 | cut -c1-12)
        passed_at=$(printf '%s' "$known" | cut -f3)
        stamp_checked
        echo "check: this code already passed, on commit $passed_commit at $passed_at. Only documents, if anything, have changed since, so nothing ran and this commit counts as checked. CHECK_AGAIN=1 runs it for real."
        exit 0
      fi
    fi
    counted=$(counted_runs)
    count=0
    [ -z "$counted" ] || count=$(printf '%s\n' "$counted" | wc -l | tr -d ' ')
    if [ "$count" -ge "$BUDGET" ] && [ -z "$why" ]; then
      {
        echo "check: not started. $branch has had $count full runs $(scope_words), and the budget is $BUDGET: one for a hand-back, one after a review's fixes."
        printf '%s\n' "$counted"
        echo "A smaller question is usually cheaper:"
        echo "  tools/check.sh --failed     only the tests that failed last time"
        echo "  tools/check.sh --quick      clippy, changed crates' tests, the Windows compile check"
        echo "  HEAVY_CLASS=quick tools/sysguard/heavy.sh cargo nextest run --locked -p <crate> -E 'test(=<name>)'     one named test"
        echo "If a whole run is what's needed, nothing stops it. Say what it decides, and it starts at once:"
        echo "  tools/check.sh --full --why \"what this run decides\""
      } >&2
      exit 3
    fi
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
  *) usage ;;
esac
# HEAD and everything uncommitted, to tell whether the code under test stayed what it was.
code_state() { { git rev-parse HEAD; git status --porcelain; } | shasum | cut -d' ' -f1; }
# Prints how long the last stage took, and leaves the name of the one now running where the caller can see it.
stage() {
  local at
  at=$(date +%s)
  [ -z "${stage_name:-}" ] || echo "check: $stage_name took $((at - stage_started)) s"
  stage_name=$1
  stage_started=$at
  [ "$1" = done ] || echo "$1" >> "$check_mark"
}
run() {
  set -e
  # That it really started, past the queue, and on what. A refusal or a job stopped while waiting never gets here.
  echo "$(date +%s) $(git rev-parse HEAD) $(code_state)" > "$check_mark"
  stage clippy
  cargo clippy --workspace --all-targets --locked -- -D warnings
  # Keeps the core buildable for Windows from the first commit. Skipped only if the target isn't installed. A compile error, so it goes before anything runs.
  stage windows
  if rustup target list --installed | grep -q x86_64-pc-windows-gnu; then
    cargo check --workspace --locked --target x86_64-pc-windows-gnu
  fi
  # Tests start the worker and the indexer, and `-p` alone wouldn't build them.
  stage workers
  cargo build --locked -p recto-render-service -p recto-indexer
  # The app's own tests run below, against an app built from this tree. An older one lying around says nothing.
  if [ -n "$tests_first" ]; then
    stage changed-tests
    # shellcheck disable=SC2086
    RECTO_SKIP_APP=1 cargo nextest run --locked --no-fail-fast $tests_first
  fi
  stage other-tests
  RECTO_SKIP_APP=1 cargo nextest run --locked --no-fail-fast --no-tests=pass "${tests[@]}"
  # Only --full: no Xcode needed for --quick, and no minutes spent on Swift.
  if [ "$mode" = "--full" ]; then
    stage app-build
    app/build.sh
    # At most two at once: each is a real app, and this Mac is in use.
    stage app-tests
    RECTO_REQUIRE_APP=1 cargo nextest run --locked --no-fail-fast -p recto-cli --test app --test restore --test long_session -j 2
  fi
  stage done
}
# PDFium's digest-checked library, which the render tests need. Does nothing if it's already there.
tools/pdfium.sh >/dev/null
check_mark=$(mktemp "${TMPDIR:-/tmp}/recto-check.XXXXXX")
trap 'rm -f "$check_mark"' EXIT
export -f run stage code_state
export check_mark mode tests_first
export tests_joined="${tests[*]}"
status=0
tools/sysguard/heavy.sh bash -c 'tests=($tests_joined); run' "check.sh$mode" || status=$?
[ "$mode" = --full ] || exit "$status"
# Nothing ran: the queue refused it, or it was stopped while waiting. So there's no run to count.
read -r started started_commit started_state < "$check_mark" || exit "$status"
case "$status" in
  0) result=pass ;;
  # Stopped from outside says nothing about the code, so it isn't held against it.
  130|137|143) result=stopped ;;
  *) result="fail:$(tail -1 "$check_mark")" ;;
esac
# Only a commit that passed, with nothing uncommitted from start to end, counts for the push guard.
ledger_commit="$started_commit+dirty"
if [ -n "$started_state" ] && [ "$started_state" = "$(code_state)" ] && tree_is_clean; then
  ledger_commit=$started_commit
  if [ "$result" = pass ]; then
    printf '%s\t%s\t%s\n' "$(code_fingerprint)" "$started_commit" "$(date '+%Y-%m-%d %H:%M')" >> "$passes"
    stamp_checked
  fi
fi
printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$(date '+%Y-%m-%d %H:%M')" "$branch" "$ledger_commit" "$result" "$(($(date +%s) - started))" "${why:--}" >> "$ledger"
exit "$status"
