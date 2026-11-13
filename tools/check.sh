#!/usr/bin/env bash
# check: the one local check, cheapest step first, stopping at the first failure.
#
#   tools/check.sh --quick    The compile gate, then the tests of crates changed since the branch left main (and uncommitted) and the registration guards (while working)
#   tools/check.sh --corpus   The index against the pages over every PDF in RECTO_BENCH100, about a quarter of an hour
#   tools/check.sh --failed   Only the tests that failed last time in this folder (while fixing them)
#   tools/check.sh --full     The compile gate, then the tests of what the change can affect (changed crates and their dependents, the guards) and the app
#                             tiers it can affect, as tools/check-scope.sh says; everything on CI, on `main` and with CHECK_ALL=1 (before handing back).
#                             Only a pass of everything lets `main` be pushed.
#   tools/check.sh --full --why "what this run decides"    A branch gets two full runs. A third needs a stated reason, which goes in the ledger.
#   tools/check.sh --runs     Full runs per branch, from the ledger, and what counts against this branch's budget.
#   tools/check.sh --slow     The newest kept run's 20 slowest tests, each tier's time against its budget, the slow stage, and the stages against the run before.
#   CHECK_PART=core|window|lifecycle tools/check.sh --full    Just that part of what a full run covers, as CI splits it into jobs; never recorded as a pass.
#   tools/check.sh --like-ci <part or filter> [--times N]   CI's profile, concurrency and slower machine, by limits on the process (tools/check-like-ci.sh); never a pass.
#
# A pass counts for identical code: --full on a clean tree whose code has already passed says so, lets the push guard accept this commit and runs
# nothing. Documents don't count as code, as in tools/hooks/pre-push. CHECK_AGAIN=1 runs it for real anyway. A dirty tree always runs and is never recorded.
# docs/agent-instructions/worker-testing.md says which check answers which question.
#
# The compile gate is what runs first, before any test is queued: the layout (rustfmt, outside the queue, as it builds nothing), then clippy and the Windows
# compile check as a quick job of their own. An error there costs seconds and no test job is ever queued for it, nor a full run counted.
# The queue (tools/sysguard/heavy.sh) refuses to run a check again on code that hasn't changed since it failed.
#
# The app tier is held by one check at a time, machine-wide (2026-11-13: three full runs reached their app tests together, each launched the real app
# four at a time, the load went to 139 on 10 cores and lifecycle tests failed from starvation; three in a row are faster than three at once). The stages
# that launch the real app (app-tests, slow, registrations, and a --like-ci round of an app test) take the lock `recto-app-tier.lock` in the git common
# dir, which every worktree of this repository shares: a directory made with mkdir, holding the owner's pids, worktree, stage and start time. Builds and
# in-process tests are not held. A waiter says who holds it and for how long, then what it waited. A dead owner (or a pid that isn't a check any more)
# is taken over, with a line saying so; the holder's exit, failed or interrupted, removes it. The time waited is said apart and is not in the stage's
# seconds (`check.sh --slow`, the budgets), nor in the ledger's. On CI (CI set) the lock is taken over and never waited on.
#   CHECK_APP_TIER=free        no lock at all: the coordinator's solo measurements.
#   CHECK_APP_TIER_POLL=<s>    seconds between looks (default 2, a fraction is fine); CHECK_APP_TIER_NOOWNER=<s> how old an owner-less lock is before it is taken over (default 10).
#   CHECK_APP_TIER_WAIT=<s>    how long a waiter waits before it stops (default 2700): exit 5, nothing tested, never a run, a failure or a ledger line.
set -euo pipefail
cd "$(dirname "$0")/.."
# A check never signs with the owner's identity or raises a keychain prompt: every build and test in it signs ad hoc, whatever this shell has set.
unset RECTO_SIGN_IDENTITY
usage() { sed -n '2,17p' "$0"; exit 2; }
mode=""
why=""
why_given=""
like_args=()
while [ $# -gt 0 ]; do
  case "$1" in
    --why)
      [ $# -ge 2 ] || usage
      why=$2
      why_given=1
      shift 2
      ;;
    *)
      if [ "$mode" = --like-ci ]; then like_args+=("$1"); shift; continue; fi
      [ -z "$mode" ] || usage
      mode=$1
      shift
      ;;
  esac
done
mode="${mode:---quick}"
[ -z "$why_given" ] || [ "$mode" = --full ] || usage
# A named part of a full run (CI runs the parts as jobs of their own): core is the gate, the workspace tests and the tools' tests, window and lifecycle the
# app's two tiers (tools/check-scope.sh). The parts together are everything. A part is never remembered or counted as a run.
part=${CHECK_PART:-all}
case "$part" in
  all) ;;
  core | window | lifecycle)
    [ "$mode" = --full ] || { echo "check: CHECK_PART=$part only goes with --full." >&2; exit 2; }
    ;;
  *) echo "check: CHECK_PART is core, window or lifecycle, not '$part'." >&2; exit 2 ;;
esac
# One line, because the ledger is one line per run.
why=$(printf '%s' "$why" | tr '\t\n' '  ' | sed -E 's/^ +| +$//g' | cut -c1-200)
if [ -n "$why_given" ] && [ -z "$why" ]; then
  echo 'check: --why needs the question this run decides, for example --why "the fix for the review'\''s race holds across the app tests".' >&2
  exit 2
fi

common_dir=$(git rev-parse --git-common-dir)
app_tier_dir="$(cd "$common_dir" && pwd)/recto-app-tier.lock"
# The check this is, for the lock's owner line: the queue's job is a child of it, and says its own pid there.
CHECK_TOP_PID=$$
export app_tier_dir CHECK_TOP_PID
checked="$common_dir/recto-checked"
# fingerprint, commit, time, scope: a line per pass on a clean tree. Scope is `all` (a pass of everything), or `sel:` and a hash of the selection (tools/check-scope.sh).
# A line without a scope is from before selection, and was a pass of everything.
passes="$common_dir/recto-passes"
# time, branch, commit, result, seconds, reason, scope (all or selective): a line per real --full run. Result is pass, fail:<stage>, or stopped, meaning from outside, which says nothing about the code.
ledger="$common_dir/recto-full-runs"
BUDGET=2

# What decides a result. The tracked files, by content, except documents: this filter is the one in tools/hooks/pre-push. Then what isn't tracked and can
# change a result on the same code: the tools, whether the Windows compile check can run at all, and the settings that change what the tests do.
# Not covered: the real PDFs some tests read if they're there, in another repository. docs/agent-instructions/worker-testing.md lists them.
tracked_code() { git ls-files -s | awk -F'\t' '$2 !~ /^docs\/|\.md$/'; }
code_fingerprint() {
  {
    tracked_code
    cargo nextest --version 2>/dev/null || true
    swift --version 2>/dev/null || true
    sw_vers -productVersion 2>/dev/null || true
    rustup target list --installed 2>/dev/null | grep -c x86_64-pc-windows-gnu || true
    env | grep -E '^(RECTO_|NEXTEST_|RUSTFLAGS=|RUSTDOCFLAGS=|CARGO_PROFILE_)' | sort || true
  } | shasum | cut -d' ' -f1
}
# What's untracked and could still change a result: a file anywhere cargo, the app build or the tools read, such as a test file, a new crate or a
# `.cargo/config.toml`, and the build's own files at the top. Not documents, the filter below, and not other files at the top, such as someone's notes:
# nothing reads those, and counting them stopped a clean checkout's pass from being remembered at all.
untracked_that_counts() {
  git ls-files --others --exclude-standard | grep -vE '^docs/|\.md$' | grep -E '/|^(Cargo\.|rust-toolchain|clippy\.toml|rustfmt\.toml|\.heavy-inputs$)' || true
}
tree_is_clean() { [ -z "$(git status --porcelain --untracked-files=no)" ] && [ -z "$(untracked_that_counts)" ]; }
# The commit a run tested goes to the push guard, not whatever HEAD is by the end.
stamp_checked() {
  local commit=${1:-$(git rev-parse HEAD)}
  [ "$(tail -1 "$checked" 2>/dev/null)" = "$commit" ] || echo "$commit" >> "$checked"
}
# Only documents (the guard's filter) changed from a commit to HEAD, which descends from it.
docs_only_since() {
  git merge-base --is-ancestor "$1" HEAD 2>/dev/null && [ -z "$(git diff --name-only "$1" HEAD | grep -vE '^docs/|\.md$')" ]
}
branch=$(git symbolic-ref --short -q HEAD || echo detached)
# On `main`, a pass is stamped for the push guard only while the last pushed main commit's CI isn't red, unless that red is marked known with a date
# (tools/push-main.sh --known, at most 7 days; PLAN R0.7e): merges went on over a red main four times, and one red went unseen (2026-11-12). The pass
# is still remembered, so once CI is green or the red is marked, the same code stamps without running again. gh missing, signed out or offline is a
# warning, never a wait: push-main.sh bounds every call.
# Every run that builds or tests ends with this, on any branch: while main's CI is over its time budget, each agent is told so, sternly, as the last thing
# it reads (owner, 2026-11-12: "It should be a stern warning so the agents knows it's something they need to take seriously and look at"). It reads a
# local file and asks GitHub nothing; within the budget it says nothing.
ci_budget_warning() { [ ! -x tools/push-main.sh ] || tools/push-main.sh --budget || true; }

main_ci_allows_stamp() {
  [ "$branch" = main ] && [ -x tools/push-main.sh ] || return 0
  local said rc=0
  said=$(tools/push-main.sh --status 2>&1) || rc=$?
  said=${said#push-main: }
  case "$rc" in
    0) echo "check: $said" ;;
    1)
      {
        echo "check: not stamped for the push guard: $said"
        echo "Fix main first (tools/push-main.sh --watch shows the failure, its rates and its evidence), or, if the failure is understood and tracked, mark it"
        echo "known for at most a week: tools/push-main.sh --known <sha> --until YYYY-MM-DD --why \"what is known\". The pass is remembered: run this again then."
      } >&2
      return 1
      ;;
    3) echo "check: warning: $said The pass is stamped: a run that never ran says nothing of the code." >&2 ;;
    *) echo "check: warning: $said The pass is stamped without knowing it." >&2 ;;
  esac
}

# Whether a ledger line's commit still counts against this branch's budget. Only a run of code still in HEAD's history counts: a merge taken back off, or
# commits set aside, aren't what's being checked now (on main, a merge identical to a passed branch was refused, its count made of runs of two merges that
# were no longer there). On `main`, a budget per range of commits not yet pushed: every merge is new code and needs its check, so a count for all time
# would mean nothing, and a push starts the count again. Elsewhere, runs since the branch left main: a branch cut again under an old name, or one that has
# taken in main, has new code under test.
counts_now() {
  local commit=${1%+dirty} since
  git merge-base --is-ancestor "$commit" HEAD 2>/dev/null || return 1
  if [ "$branch" = main ]; then
    git rev-parse -q --verify refs/remotes/origin/main >/dev/null || return 0
    ! git merge-base --is-ancestor "$commit" refs/remotes/origin/main 2>/dev/null
  else
    since=$(git merge-base main HEAD 2>/dev/null) || return 0
    git merge-base --is-ancestor "$since" "$commit" 2>/dev/null
  fi
}
# The ledger's lines that count against this branch now, oldest first. A run stopped from outside doesn't.
counted_runs() {
  [ -f "$ledger" ] || return 0
  local time run_branch commit result seconds reason scope
  while IFS=$'\t' read -r time run_branch commit result seconds reason scope; do
    [ "$run_branch" = "$branch" ] && [ "$result" != stopped ] || continue
    if counts_now "$commit"; then printf '  %s  %s  %s  %s s  %s%s\n' "$time" "$(printf '%s' "$commit" | cut -c1-12)" "$result" "$seconds" "$reason" "${scope:+ ($scope)}"; fi
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
# What the rules in tools/check-scope.sh select and the names it relies on. A name no test has any more stops the check here, before anything is queued.
# shellcheck source=tools/check-scope.sh
. tools/check-scope.sh
# shellcheck source=tools/check-times.sh
. tools/check-times.sh
# shellcheck source=tools/check-rerun.sh
. tools/check-rerun.sh
# shellcheck source=tools/check-like-ci.sh
. tools/check-like-ci.sh
# ---- The app tier lock (see the header). ----
# The owner file: pid (the job taking it), top (the check.sh it belongs to), start time, stage, worktree.
app_tier_owner() { cat "$app_tier_dir/owner" 2>/dev/null || true; }
# Why the lock's owner no longer holds it, or nothing if it still does: a lock with no owner line for 10 s is one whose taker died between mkdir and writing.
app_tier_stale() {
  local pid top rest
  read -r pid top rest < <(app_tier_owner) || true
  if [ -z "${pid:-}" ]; then
    local made
    made=$(stat -f %m "$app_tier_dir" 2>/dev/null || date +%s)
    [ $(($(date +%s) - made)) -ge "${CHECK_APP_TIER_NOOWNER:-10}" ] && echo "it has no owner" || true
    return 0
  fi
  if ! kill -0 "$pid" 2>/dev/null; then echo "its owner (process $pid) is gone"; return 0; fi
  case "$(ps -p "$top" -o command= 2>/dev/null || true)" in
    *check.sh*) ;;
    *) echo "process $top is no check any more" ;;
  esac
}
app_tier_held_by() {
  local pid top began stage tree
  read -r pid top began stage tree < <(app_tier_owner) || true
  echo "${tree:-an unknown tree} (${stage:-?}, $(($(date +%s) - ${began:-$(date +%s)})) s)"
}
# Takes the lock for a stage, waiting for it. Sets app_tier_waited to the seconds waited and adds them to the run's wait file. A tree-wide failure to write
# anywhere never fails the check: only the bound does (exit 5).
app_tier_take() {
  app_tier_waited=0
  [ "${CHECK_APP_TIER:-}" != free ] || return 0
  local began now said="" why moved
  began=$(date +%s)
  if [ -n "${CI:-}" ]; then
    # One check at a time on CI, so nothing to wait for; what a cancelled run left on a reused runner is taken over.
    rm -rf "$app_tier_dir" 2>/dev/null || true
  fi
  while ! mkdir "$app_tier_dir" 2>/dev/null; do
    if [ ! -d "$app_tier_dir" ]; then
      # mkdir failed and nothing's there: the folder can't be made (read-only, say). The check goes on without the lock rather than fail on it.
      echo "check: the app tier lock can't be made at $app_tier_dir, so this run goes on without it."
      return 0
    fi
    why=$(app_tier_stale)
    if [ -n "$why" ]; then
      echo "check: the app tier lock is stale ($why; held by $(app_tier_held_by)), so this run takes it over."
      moved="$app_tier_dir.stale.$$"
      # One mv wins when two waiters take over together. If what was moved turns out to be a live owner's (a new one made in between), it goes back.
      if mv "$app_tier_dir" "$moved" 2>/dev/null; then
        if [ -n "$(app_tier_stale_of "$moved")" ]; then rm -rf "$moved"; else mv "$moved" "$app_tier_dir" 2>/dev/null || rm -rf "$moved"; fi
      fi
      continue
    fi
    now=$(date +%s)
    if [ $((now - began)) -ge "${CHECK_APP_TIER_WAIT:-2700}" ]; then
      echo "check: stopped after waiting $((now - began)) s for the app tier, held by $(app_tier_held_by). Nothing was tested. Run it again when that check is done (CHECK_APP_TIER_WAIT=<s> sets how long to wait, CHECK_APP_TIER=free skips the lock)." >&2
      # Marked for the check that started the job, which exits 5 for it: the queue would hold a failure against unchanged code and refuse the next run.
      : > "$app_tier_dir.bound.$CHECK_TOP_PID" 2>/dev/null || true
      return 143
    fi
    if [ -z "$said" ]; then said=1; echo "check: waiting for the app tier, held by $(app_tier_held_by)"; fi
    sleep "${CHECK_APP_TIER_POLL:-2}"
  done
  printf '%s %s %s %s %s\n' "$$" "$CHECK_TOP_PID" "$(date +%s)" "$1" "$PWD" > "$app_tier_dir/owner" 2>/dev/null || true
  app_tier_waited=$(($(date +%s) - began))
  if [ -n "$said" ]; then echo "check: got the app tier after waiting $app_tier_waited s"; fi
  [ -z "${check_mark:-}" ] || echo "$app_tier_waited" >> "$check_mark.waited" 2>/dev/null || true
}
app_tier_stale_of() { local d=$app_tier_dir r; app_tier_dir=$1; r=$(app_tier_stale); app_tier_dir=$d; echo "$r"; }
# After the queue's job: it gave up waiting for the app tier (its message is above). That is not a result of any code: exit 5, nothing recorded.
app_tier_stopped() {
  [ -e "$app_tier_dir.bound.$CHECK_TOP_PID" ] || return 0
  rm -f "$app_tier_dir.bound.$CHECK_TOP_PID"
  exit 5
}
# A --like-ci round of an app test launches the app, so it holds the tier for its rounds. tools/check-like-ci.sh's own function runs inside; the lock is
# around it so that file stays as it is.
eval "app_tier_like_inner() $(declare -f like_ci_rounds | tail -n +2)"
like_ci_rounds() {
  local rc=0
  if [ "${LIKE_APP:-0}" = 1 ]; then
    trap app_tier_release EXIT
    app_tier_take like-ci || return $?
  fi
  app_tier_like_inner || rc=$?
  app_tier_release
  return "$rc"
}
# Removes the lock if it is this job's or this check's. Safe to call twice, and from the outer exit trap.
app_tier_release() {
  local pid top rest
  [ -d "$app_tier_dir" ] || return 0
  read -r pid top rest < <(app_tier_owner) || true
  if [ "${pid:-}" = "$$" ] || [ "${top:-}" = "$CHECK_TOP_PID" ]; then rm -rf "$app_tier_dir" 2>/dev/null || true; fi
}
# Where a failing test that started the app leaves its evidence, one folder a test under its stage's name (crates/recto-cli/tests/common/evidence.rs): the tree's
# own, emptied at each run's start, or RECTO_EVIDENCE's, as CI gives it to upload. And how often a failed stage's tests run again to measure the failure:
# CI 4 times (tools/check-rerun.sh has the bounds), here only when asked.
CHECK_EVIDENCE_ROOT=${RECTO_EVIDENCE:-$PWD/${CARGO_TARGET_DIR:-target}/recto-evidence}
case "$CHECK_EVIDENCE_ROOT" in /*) ;; *) CHECK_EVIDENCE_ROOT=$PWD/$CHECK_EVIDENCE_ROOT ;; esac
CHECK_RERUN=${CHECK_RERUN:-$([ -n "${CI:-}" ] && echo 4 || echo 0)}
case "${CHECK_SLOW_SEED:-0}" in '' | *[!0-9]*) echo "check: CHECK_SLOW_SEED is a number, the slow mode's jitter seed, not '$CHECK_SLOW_SEED'." >&2; exit 2 ;; esac
export CHECK_EVIDENCE_ROOT CHECK_RERUN
if [ "$mode" = --slow ]; then
  times_report
  exit 0
fi
names_or_stop() {
  local missing
  missing=$(scope_names_missing .)
  [ -z "$missing" ] || {
    echo "check: tools/check-scope.sh names tests that aren't there any more (renamed or moved?). Fix the name there, or the tier, before anything runs:" >&2
    printf '%s\n' "$missing" | sed 's/^/  /' >&2
    exit 2
  }
}
# What the slow stage runs, with every waiting point late (`RECTO_QA_SLOW=ci`, docs/engineering/slow-mode.md): what only the real app shows when every seam is
# late at once. Since R1.4 (2026-11-13) what Rust decides of a late arrival is tested in process, each seam held and let through in order by the headless
# kit (`recto_platform::slow::gate`); a window made again from a late state is the window tier's `everything_comes_back_when_the_state_is_read_late`, which
# says the remake happened (`record.remade`); a tab woken from its sleep file late is `slow::a_slow_run_waits_at_every_seam_it_passes`. Here: the one
# whole-state test with every seam late, the first frame drawn through late workers, the switch's own check, and a protected tab asked again on a late
# state, whose sheet and its Cancel are on the window that went up without the state and the one made again from it, which the kit has no window to see.
SLOW_TESTS='test(=slow_stage::everything_comes_back_after_a_quit_when_every_wait_is_late) | test(=a_protected_tab_asks_again_after_a_quit) | test(=the_first_frame_is_the_right_one) | test(=slow::a_quiet_run_with_no_switch_is_never_slow)'
export SLOW_TESTS
# How a run is shaped, set below from the mode and what changed, and read by `run`:
#   tests_first    the crates whose tests go first (what changed is most likely to fail)
#   other_mode     all: every other test; filter: the tests of other_filter; none
#   app_mode       both, window, lifecycle or none: which tiers of tests that launch the app
tests_first=""
tests=()
other_mode=none
other_filter=""
app_mode=none
slow_filter=$SLOW_TESTS
case "$mode" in
  --quick)
    names_or_stop
    select_scope --quick
    if [ "$scope_all" = 1 ]; then
      other_mode=all
      tests=(--workspace)
      echo "check: $scope_report Quick runs every test, as something outside crates/ and app/ changed."
    else
      other_mode=filter
      other_filter="$GUARD_TESTS"
      [ -z "$scope_changed" ] || other_filter="($(printf '%s\n' $scope_changed | packages_filter)) | $GUARD_TESTS"
      echo "check: quick: the tests of ${scope_changed:-no crate} (changed since the branch left main, or uncommitted), and the registration guards."
    fi
    ;;
  --corpus)
    # Every kind of search from the index and from the pages, over a folder of real PDFs. Slow, so not part of --full, which always runs a small version.
    : "${RECTO_BENCH100:?Set RECTO_BENCH100 to a folder of PDFs}"
    tools/pdfium.sh >/dev/null
    RECTO_INDEX_ALL=1 exec tools/sysguard/heavy.sh cargo test --release --locked -p recto-cli --test index real_pages -- --nocapture
    ;;
  --full)
    names_or_stop
    select_scope --full
    echo "$scope_report"
    if [ "${CHECK_AGAIN:-}" != 1 ] && [ "$part" = all ] && tree_is_clean; then
      # A pass of everything counts for any scope; a selective one only for the same selection (a different change, or main having moved, may need other tests).
      known=$(awk -F'\t' -v fp="$(code_fingerprint)" -v sc="$scope_key" '$1 == fp && ($4 == "" || $4 == "all" || $4 == sc) { line = $0 } END { print line }' "$passes" 2>/dev/null || true)
      if [ -n "$known" ]; then
        passed_commit=$(printf '%s' "$known" | cut -f2 | cut -c1-12)
        passed_at=$(printf '%s' "$known" | cut -f3)
        known_scope=$(printf '%s' "$known" | cut -f4)
        if [ -z "$known_scope" ] || [ "$known_scope" = all ]; then
          if ! main_ci_allows_stamp; then
            echo "check: this code already passed, on commit $passed_commit at $passed_at, so nothing ran, but this commit isn't stamped." >&2
            exit 4
          fi
          stamp_checked
          echo "check: this code already passed, on commit $passed_commit at $passed_at. Only documents, if anything, have changed since, so nothing ran and this commit counts as checked. CHECK_AGAIN=1 runs it for real."
        else
          echo "check: this code already passed the same selection, on commit $passed_commit at $passed_at, so nothing ran. That was not a pass of everything, so it doesn't let \`main\` be pushed: CHECK_ALL=1 runs everything."
        fi
        exit 0
      fi
    fi
    counted=$(counted_runs)
    count=0
    [ -z "$counted" ] || count=$(printf '%s\n' "$counted" | wc -l | tr -d ' ')
    if [ "$part" = all ] && [ "$count" -ge "$BUDGET" ] && [ -z "$why" ]; then
      {
        echo "check: not started. $branch has had $count full runs $(scope_words), and the budget is $BUDGET: one for a hand-back, one after a review's fixes."
        printf '%s\n' "$counted"
        echo "A smaller question is usually cheaper:"
        echo "  tools/check.sh --failed     only the tests that failed last time"
        echo "  tools/check.sh --quick      the compile gate, changed crates' tests, the registration guards"
        echo "  HEAVY_CLASS=quick tools/sysguard/heavy.sh cargo nextest run --locked -p <crate> -E 'test(=<name>)'     one named test"
        echo "If a whole run is what's needed, nothing stops it. Say what it decides, and it starts at once:"
        echo "  tools/check.sh --full --why \"what this run decides\""
      } >&2
      exit 3
    fi
    # What this work changed is what's most likely to fail, so it runs first, and the rest skip it.
    for c in $scope_changed; do tests_first="$tests_first -p $c"; done
    if [ "$scope_all" = 1 ]; then
      other_mode=all
      app_mode=both
      tests=(--workspace)
      for c in $scope_changed; do tests+=(--exclude "$c"); done
      # Every crate changed, as a rustfmt pass does: the changed ones are all of them, and nothing's left for the rest. `--workspace` with every member
      # left out is an error, not an empty run.
      all=$(for folder in crates/*/; do echo "${folder}x"; done | crates_in)
      if [ -n "$scope_changed" ] && [ "$(printf '%s\n' $scope_changed | sort -u)" = "$all" ]; then
        tests=()
        other_mode=none
      fi
    else
      other_mode=filter
      others=$(printf '%s\n' $scope_affected | grep -vFxf <(printf '%s\n' $scope_changed) || true)
      other_filter="$GUARD_TESTS"
      [ -z "$others" ] || other_filter="($(printf '%s\n' "$others" | packages_filter)) | $GUARD_TESTS"
      if [ "$scope_window$scope_lifecycle" = 11 ]; then app_mode=both
      elif [ "$scope_window" = 1 ]; then app_mode=window
      elif [ "$scope_lifecycle" = 1 ]; then app_mode=lifecycle
      fi
    fi
    # A named part leaves out the other part's work: core has no app tests, and an app part has no workspace tests and no gate.
    case "$part" in
      core) app_mode=none ;;
      window | lifecycle)
        tests_first="" other_mode=none
        case "$app_mode" in both | "$part") app_mode=$part ;; *) app_mode=none ;; esac
        ;;
    esac
    # The slow stage's tests (SLOW_TESTS) that are in a tier this run covers.
    slow_filter=$SLOW_TESTS
    case "$app_mode" in
      window) slow_filter="($SLOW_TESTS) & ($WINDOW_TESTS)" ;;
      lifecycle) slow_filter="($SLOW_TESTS) & ($LIFECYCLE_TESTS)" ;;
    esac
    ;;
  --like-ci)
    export -f nt nt_jobs junit_failed args_without_filter rerun_failed rerun_table like_ci_rounds app_tier_like_inner like_ci_note app_tier_owner app_tier_stale app_tier_held_by app_tier_take app_tier_stale_of app_tier_release
    status=0
    like_ci_main "${like_args[@]}" || status=$?
    app_tier_stopped
    evidence_report "$status"
    exit "$status"
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
code_state() { { git rev-parse HEAD; git status --porcelain --untracked-files=no; untracked_that_counts; } | shasum | cut -d' ' -f1; }
# Every tracked file but documents, as the index has it and as it is on disk: modification time to the nanosecond, size and inode. A file edited during a
# run and put back before it ends is the same in `code_state`, but not here (process review, 2026-11-11: a test edited mid-run). Documents are left out,
# as a commit of them during a run is allowed. A line a file, so the end can say which changed.
tracked_tree() {
  tracked_code
  git -c core.quotePath=false ls-files | grep -vE '^docs/|\.md$' | tr '\n' '\0' | xargs -0 stat -f '%Fm %z %i %N' 2>/dev/null || true
}

# Prints how long the last stage took, and leaves the name of the one now running where the caller can see it.
stage() {
  local at
  at=$(date +%s)
  [ -z "${stage_name:-}" ] || echo "check: $stage_name took $((at - stage_started)) s${stage_waited:+ (waited $stage_waited s for the app tier)}"
  stage_waited=""
  [ -z "${stage_name:-}" ] || echo "$stage_name $((at - stage_started))" >> "$check_mark.stages"
  stage_name=$1
  stage_started=$at
  [ "$1" = done ] || echo "$1" >> "$check_mark"
}
# The compile gate: what must build and pass the lints before a test job is queued. Clippy for the whole workspace, which is incremental and so costs
# seconds on what didn't change, then the headings helper's own lints, then the Windows compile check. A compile error is found here, as a quick job at
# the front of the queue, and no test is ever queued for it (46 of 156 failed jobs in 30 hours were compile, clippy or fmt errors, 2026-11-12).
gate() {
  set -e
  local at
  at=$(date +%s)
  cargo clippy --workspace --all-targets --locked -- -D warnings
  # The headings helper is a workspace of its own, kept apart so distillPDF's lopdf features never reach the app. Its own lints, then its tests below.
  cargo clippy --manifest-path crates/recto-headings/Cargo.toml --target-dir "${CARGO_TARGET_DIR:-target}" --all-targets --locked -- -D warnings
  # Keeps the core buildable for Windows from the first commit. Skipped only if the target isn't installed. A compile error, so it goes before anything runs.
  # Clippy at the level above, so a warning fails it: code only a Windows build compiles is seen nowhere else, and an unused variable in it passed a plain
  # `cargo check` (916efaf). Clippy shares the check's artifacts for dependencies, so nothing more is built than before.
  if rustup target list --installed | grep -q x86_64-pc-windows-gnu; then
    cargo clippy --workspace --locked --target x86_64-pc-windows-gnu -- -D warnings
  fi
  echo "check: clippy and the Windows compile check took $(($(date +%s) - at)) s"
  # The docs' links of the crates in doc_crates (DOC_LINK_CRATES that changed, tools/check-scope.sh): a link to nothing, or a public doc linking a private item.
  local c
  for c in ${doc_crates:-}; do
    at=$(date +%s)
    RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links -D rustdoc::private_intra_doc_links --document-private-items" cargo doc -p "$c" --no-deps --locked
    echo "check: $c's doc links took $(($(date +%s) - at)) s"
  done
}
run() {
  set -e
  # That it really started, past the queue, and on what. A refusal or a job stopped while waiting never gets here.
  echo "$(date +%s) $(git rev-parse HEAD) $(code_state)" > "$check_mark"
  # This run's evidence only: what the last run left in the tree's own folder goes (one given in RECTO_EVIDENCE, such as CI's, is never emptied).
  [ -n "${RECTO_EVIDENCE:-}" ] || rm -rf "$CHECK_EVIDENCE_ROOT"
  mkdir -p "$CHECK_EVIDENCE_ROOT"
  tracked_tree > "$check_mark.tree"
  # Tests start the worker, the indexer and the headings helper, and `-p` alone wouldn't build them.
  # Object files cargo leaves in deps make every test that makes a surface slow (tools/target-hygiene.sh, 2026-11-12). Skipped by a check-test stub tree.
  if [ -x tools/target-hygiene.sh ]; then tools/target-hygiene.sh; fi
  stage workers
  cargo build --locked -p recto-render-service -p recto-indexer
  cargo build --locked --manifest-path crates/recto-headings/Cargo.toml --target-dir "${CARGO_TARGET_DIR:-target}" --bin recto-headings
  stage headings-tests
  [ "$part" = window ] || [ "$part" = lifecycle ] || cargo test --locked --manifest-path crates/recto-headings/Cargo.toml --target-dir "${CARGO_TARGET_DIR:-target}"
  # The app's own tests run below, against an app built from this tree. An older one lying around says nothing.
  if [ -n "$tests_first" ]; then
    stage changed-tests
    # shellcheck disable=SC2086
    RECTO_SKIP_APP=1 nt --locked --no-fail-fast $tests_first
  fi
  if [ "$other_mode" = all ]; then
    stage other-tests
    RECTO_SKIP_APP=1 nt --locked --no-fail-fast --no-tests=pass "${tests[@]}"
  elif [ "$other_mode" = filter ]; then
    stage other-tests
    RECTO_SKIP_APP=1 nt --locked --no-fail-fast --no-tests=pass --workspace -E "$other_filter"
  fi
  # Only --full: no Xcode needed for --quick, and no minutes spent on Swift. And only when a tier of tests that launch the app is selected.
  if [ "$mode" = "--full" ] && [ "$app_mode" != none ]; then
    stage app-build
    app/build.sh
    # The bundle just built, as it ships: its notices are this tree's, and nothing in it has an extended attribute, which a zip carries as a file the
    # signature doesn't cover. They ran only with RECTO_SKIP_APP above, so no run held a built app to them (icon worker, 2026-11-11). Seconds.
    stage app-bundle
    [ "$part" = lifecycle ] || RECTO_REQUIRE_APP=1 nt --locked --no-fail-fast -p recto-cli --test notices -E 'test(=the_built_app_carries_the_notices) | test(=nothing_in_the_built_app_has_extended_attributes)'
    # At most four at once: each is a real app, and this Mac is in use. Each run is quiet, off every screen, on its own state folder and
    # pasteboard, so they don't meet. Four took this stage from 191 s to 118 to 140 s, passing three runs out of three at load 25 to 45
    # (docs/reports/build-performance-2026-11-11.md). The tiers (tools/check-scope.sh): window, and lifecycle. Both, as on CI, is every test in the binaries.
    # A failure here doesn't hide the registrations stage below, which is about what these tests left (a leak seen one run in three on CI, 2026-11-12):
    # it runs anyway, and the run fails with this stage's name. The slow stage, the same tests late, is left out after a failure here.
    stage app-tests
    # Only one check at a time launches the app (the header). The stage's clock starts when the lock is had, so the wait isn't in its seconds.
    trap app_tier_release EXIT
    app_tier_take app-tests || return $?
    if [ "$app_tier_waited" -gt 0 ]; then stage_waited=$app_tier_waited; stage_started=$(date +%s); fi
    local app_status=0
    case "$app_mode" in
      both) RECTO_REQUIRE_APP=1 nt --locked --no-fail-fast -p recto-cli --test app --test restore --test compare --test long_session --test evidence -j 4 || app_status=$? ;;
      window) RECTO_REQUIRE_APP=1 nt --locked --no-fail-fast -p recto-cli --test app --test restore --test compare --test evidence -j 4 -E "$WINDOW_TESTS" || app_status=$? ;;
      lifecycle) RECTO_REQUIRE_APP=1 nt --locked --no-fail-fast -p recto-cli --test app --test restore --test compare --test long_session -j 4 -E "$LIFECYCLE_TESTS" || app_status=$? ;;
    esac
    [ "$app_status" = 0 ] || echo app-tests > "$check_mark.failed"
    # The same app with every waiting point late, as a slow disk or a loaded machine has it: the state read past the window's second of patience, so the
    # window goes up without it and is made again from it, workers', the helpers' and the socket's answers late, drawing late, sleep files slow. Main went
    # red on such paths, unseen here, three pushes running (2026-11-11), and this mode's first run found five more.
    # Not on CI, which is the slow machine already: the profile on top of it doubled up, and two tests waited past their bounds there for slowness
    # alone, as they did here with the profile doubled (CI run 43080381613, 2026-11-11). The stage is how this Mac takes CI's paths.
    if [ -z "${CI:-}" ] && [ -z "${CHECK_LIKE_CI:-}" ] && [ "$app_status" = 0 ]; then
      stage slow
      # The ci profile exactly, so the gate fails the same way each run; a seed for the profile's jitter only when asked (CHECK_SLOW_SEED), said either
      # way, so a failure's seams and seed are in its rerun line and the same seed gives the same delays (PLAN R0.7b).
      local slow_switch="ci${CHECK_SLOW_SEED:+,jitter=$CHECK_SLOW_SEED}"
      echo "check: slow: RECTO_QA_SLOW=$slow_switch$([ -n "${CHECK_SLOW_SEED:-}" ] || echo ' (no jitter; CHECK_SLOW_SEED=<n> adds jitter=<n>)')"
      RECTO_QA_SLOW=$slow_switch RECTO_REQUIRE_APP=1 nt --locked --no-fail-fast -p recto-cli --test app --test restore -j 4 -E "$slow_filter" --no-tests=pass
    fi
    # After them, as it's about what they left: no copy of the app registered but the one install (owner, 2026-11-11: "We should only ever have one").
    if [ "$app_mode" = both ] || [ "$app_mode" = lifecycle ]; then
      stage registrations
      RECTO_REQUIRE_APP=1 nt --locked --no-fail-fast -p recto-cli --test registrations
    fi
    app_tier_release
    [ "$app_status" = 0 ] || { echo "check: app-tests failed (above), and the stages after it that don't depend on it ran." >&2; return "$app_status"; }
  fi
  stage done
}
# PDFium's digest-checked library, which the render tests need. Does nothing if it's already there.
tools/pdfium.sh >/dev/null
check_mark=$(mktemp "${TMPDIR:-/tmp}/recto-check.XXXXXX")
trap 'rm -rf "$check_mark" "$check_mark.tree" "$check_mark.stages" "$check_mark.junit" "$check_mark.failed" "$check_mark.waited"; app_tier_release; ci_budget_warning' EXIT
# Structure that may only shrink (tools/ratchet.sh, tools/ratchet/baseline.txt): seconds, builds nothing, so it fails first, before anything is queued.
ratchet_started=$(date +%s)
tools/ratchet.sh
echo "check: ratchet took $(($(date +%s) - ratchet_started)) s"
# The tools' own tests, when a change touched what they test since the base (TOOLS_PATHS, tools/check-scope.sh; a full run only, and on CI the same rule
# against CI's base, `ci_base`): the tool is held by the tool and not by someone running its test by hand. Each builds nothing and runs on a throwaway
# repository and lock, so nothing is queued for them. On CI they took 173 s of every run's core part whatever changed (run 43195226598).
if [ "$mode" = --full ] && [ "$part" != window ] && [ "$part" != lifecycle ] && [ "$scope_tools" = 0 ]; then
  echo "check: the tools' tests are skipped: $scope_tools_why."
fi
if [ "$mode" = --full ] && [ "$part" != window ] && [ "$part" != lifecycle ] && [ "$scope_tools" = 1 ]; then
  echo "check: the tools' tests run: $scope_tools_why."
  tools_started=$(date +%s)
  # Only the tests of the tools that changed (tool_tests_for, tools/check-scope.sh), side by side: each is a script on a throwaway repository and lock of its
  # own, and its output goes to a file of its own in one folder, so none shares a file with another. check-test.sh is the long one and runs its own parts side
  # by side. A failure prints that script's output whole (without its ok lines, last 40).
  tool_out=$(mktemp -d "${TMPDIR:-/tmp}/recto-tool-test.XXXXXX")
  tool_pids="" tool_launched=""
  for tool in ratchet hygiene check; do
    case " $scope_tools_which " in *" $tool "*) ;; *) continue ;; esac
    case "$tool" in ratchet) tool_test=tools/ratchet-test.sh ;; hygiene) tool_test=tools/target-hygiene-test.sh ;; *) tool_test=tools/check-test.sh ;; esac
    [ -f "$tool_test" ] || continue
    ( started=$(date +%s); tool_rc=0; "$tool_test" > "$tool_out/$tool.out" 2>&1 || tool_rc=$?; echo "$tool_rc $(($(date +%s) - started)) $tool_test" > "$tool_out/$tool.rc" ) &
    tool_pids="$tool_pids $!" tool_launched="$tool_launched $tool"
  done
  # shellcheck disable=SC2086
  [ -z "$tool_pids" ] || wait $tool_pids || true
  tool_failed=""
  for tool in $tool_launched; do
    # A script killed before it wrote its line is a failure, not a pass.
    read -r tool_rc tool_secs tool_test < "$tool_out/$tool.rc" 2>/dev/null || { tool_rc=killed tool_secs=0 tool_test="the $tool test"; echo "killed 0 the $tool test" > "$tool_out/$tool.rc"; }
    echo "check: $tool_test took $tool_secs s"
    [ "$tool_rc" = 0 ] || tool_failed="$tool_failed $tool"
  done
  if [ -n "$tool_failed" ]; then
    for tool in $tool_failed; do
      echo "check: $(cut -d' ' -f3- "$tool_out/$tool.rc") failed, so nothing was queued:" >&2
      grep -v '^ok' "$tool_out/$tool.out" | tail -40 >&2
    done
    rm -rf "$tool_out"
    exit 1
  fi
  rm -rf "$tool_out"
  echo "check: the tools' tests took $(($(date +%s) - tools_started)) s"
fi
# The compile gate, before any test is queued. The layout first, outside the queue as it builds nothing: one layout, so two branches that each add a field
# to a struct literal merge without a conflict (guide §2.8). `cargo fmt --all` fixes it. Then clippy and the Windows check, a quick job of their own: a
# failure is refused at once the next time on the same code, and no full run is counted for it.
if ! { cargo fmt --all --check && cargo fmt --manifest-path crates/recto-headings/Cargo.toml --all --check; }; then
  echo "check: the layout is off, so nothing was queued. \`cargo fmt --all\` fixes it." >&2
  exit 1
fi
# Whose doc links the gate checks: the DOC_LINK_CRATES that changed, or all of them when everything runs. None outside --quick and --full, which select.
doc_crates=""
for c in $DOC_LINK_CRATES; do
  if [ "${scope_all:-0}" = 1 ] || has_line "$(printf '%s\n' ${scope_changed:-})" "$c"; then doc_crates="$doc_crates${doc_crates:+ }$c"; fi
done
[ -n "${scope_key:-}" ] || doc_crates=""
export doc_crates
export -f gate
gate_status=0
if [ "$part" != window ] && [ "$part" != lifecycle ]; then
  HEAVY_CLASS=quick tools/sysguard/heavy.sh bash -c gate "check.sh$mode gate" || gate_status=$?
fi
if [ "$gate_status" -ne 0 ]; then
  echo "check: the compile gate failed, so no test was queued and no run was counted. Fix it, then run the check again." >&2
  exit "$gate_status"
fi
export -f app_tier_owner app_tier_stale app_tier_held_by app_tier_take app_tier_stale_of app_tier_stopped app_tier_release run stage nt code_state untracked_that_counts tracked_code tracked_tree junit_failed args_without_filter nt_jobs rerun_failed rerun_table
export app_tier_dir CHECK_TOP_PID check_mark part mode tests_first other_mode other_filter app_mode slow_filter WINDOW_TESTS LIFECYCLE_TESTS
export tests_joined="${tests[*]-}"
status=0
tools/sysguard/heavy.sh bash -c 'tests=($tests_joined); run' "check.sh$mode" || status=$?
app_tier_stopped
evidence_report "$status"
[ "$mode" = --full ] || exit "$status"
# Nothing ran: the queue refused it, or it was stopped while waiting. So there's no run to count.
read -r started started_commit started_state < "$check_mark" || exit "$status"
case "$status" in
  0) result=pass ;;
  # Stopped from outside says nothing about the code, so it isn't held against it.
  130|137|143) result=stopped ;;
  *) result="fail:$(cat "$check_mark.failed" 2>/dev/null || tail -1 "$check_mark")" ;;
esac
# The tests' durations are kept for every run that started, a part too (tools/check-times.sh, `check.sh --slow`). Not for one stopped from outside.
[ "$result" = stopped ] || ( times_record "$started_commit" "$([ "$scope_key" = all ] && echo all || echo selective)" "$part" "$result" ) || true
# A part is half a run: it is never remembered, stamped or counted.
[ "$part" = all ] || exit "$status"
# Only a commit that passed, with nothing uncommitted from start to end, counts for the push guard. A commit of documents only made while it ran (the
# coordinator writing up while a check runs) doesn't undo that: the code tested is still the started commit's, and that's the commit stamped.
ledger_commit="$started_commit+dirty"
started_clean=$(printf '%s\n' "$started_commit" | shasum | cut -d' ' -f1)
touched=""
if [ -f "$check_mark.tree" ]; then
  touched=$(tracked_tree | diff "$check_mark.tree" - | sed -n 's/^[<>] //p' | awk -F'\t' 'NF > 1 { print $2; next } { sub(/^[^ ]+ [^ ]+ [^ ]+ /, ""); print }' | sort -u || true)
fi
if [ -n "$started_state" ] && tree_is_clean && { [ "$started_state" = "$(code_state)" ] || { [ "$started_state" = "$started_clean" ] && docs_only_since "$started_commit"; }; }; then
  if [ -n "$touched" ]; then
    {
      echo "check: not recorded. Tracked files changed while it ran, so what it tested may not be commit $(printf '%s' "$started_commit" | cut -c1-12), even though they're back as they were:"
      printf '%s\n' "$touched" | head -10 | sed 's/^/  /'
      echo "Leave the tree alone while a full run is under way, or run it again: nothing was stamped for the push guard."
    } >&2
  else
    ledger_commit=$started_commit
  fi
fi
if [ "$ledger_commit" = "$started_commit" ]; then
  if [ "$result" = pass ]; then
    # The scope goes with the pass: only a pass of everything is remembered for any scope, and only it lets `main` be pushed. A selective pass is
    # remembered for the same selection and never stamps the push guard: the same code on `main`, or with other changes under it, still runs everything.
    printf '%s\t%s\t%s\t%s\n' "$(code_fingerprint)" "$started_commit" "$(date '+%Y-%m-%d %H:%M')" "$scope_key" >> "$passes"
    if [ "$scope_key" = all ]; then
      if main_ci_allows_stamp; then stamp_checked "$started_commit"; else status=4; fi
    fi
  fi
fi
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(date '+%Y-%m-%d %H:%M')" "$branch" "$ledger_commit" "$result" "$(($(date +%s) - started - $(awk '{ t += $1 } END { print t + 0 }' "$check_mark.waited" 2>/dev/null || echo 0)))" "${why:--}" "$([ "$scope_key" = all ] && echo all || echo selective)" >> "$ledger"
exit "$status"
