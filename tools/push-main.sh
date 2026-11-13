#!/usr/bin/env bash
# push-main: pushes `main` and watches that commit's CI to its end (PLAN R0.7e). On 2026-11-12 `main` was red on CI for four pushes, and one red went
# unseen because nothing watched main's CI after a push.
#
#   tools/push-main.sh                 git push origin main (the pre-push hook still decides), then watch the pushed commit's CI run
#   tools/push-main.sh --watch [sha]   watch a commit's run on main (default: origin/main)
#   tools/push-main.sh --status        one line on the last pushed main commit's CI, for tools/check.sh: exit 0 green, running or marked known, 1 red, 2 unknown, 3 never ran
#   tools/push-main.sh --budget        the warning when CI's check is over its time budget, from the kept times alone; `tools/check.sh` ends every run with it
#   tools/push-main.sh --known <sha> --until YYYY-MM-DD --why "what is known"
#                                      marks that commit's red CI as known until the date, at most 7 days ahead, so `check.sh --full` on main stamps again
#
# A finished watch prints the result, the failed jobs, the failed tests with their "failed N of M" lines (tools/check-rerun.sh) and the artifacts with how
# to fetch them, and keeps the result in the git common dir (recto-ci-main). Every call to `gh` is bounded (PUSH_MAIN_GH_TIMEOUT, 20 s), and `gh` missing,
# signed out or offline is said plainly, never waited on. A green run's check time is kept (recto-ci-times) and said against CI's budget (CI_BUDGET). A run cancelled by a newer push is "superseded", not red, and the watch follows the newer run. The watch gives up after PUSH_MAIN_LIMIT seconds (45 min), looking every PUSH_MAIN_POLL (30 s).
#
# Which runs it sees (2026-11-13: a push of 91 files was called "documents only" and given up on at 3 min, while its run sat queued): GitHub's run list
# leaves a run that is still queued out of an answer filtered by --branch or --commit, though the unfiltered list shows it. So the list is asked
# unfiltered (newest 30 of the CI workflow) and the script picks the branch and the commit, and a queued, waiting or running run is a run: it is waited
# for, and one that gets no runner in the whole limit is "never got a runner", neither green nor red, and kept as neither. "Documents only" is decided
# from the whole pushed range against the workflow's own paths-ignore (read from the workflow file, so the two can't drift), as GitHub does; a range with
# code in it and no run after PUSH_MAIN_NORUN seconds (10 min) is "no CI run at all", said with the range's file count.
#
# One watcher a repository (recto-ci-watcher in the git common dir: its process and commit). A watch that starts while another is alive says so in one line,
# and the older one stops when it sees the newer one has taken its commit or a later one over. A run's time is kept once, whoever sees it go green.
set -uo pipefail
cd "$(dirname "$0")/.."
GH=${PUSH_MAIN_GH:-gh}
GH_TIMEOUT=${PUSH_MAIN_GH_TIMEOUT:-20}
POLL=${PUSH_MAIN_POLL:-30}
LIMIT=${PUSH_MAIN_LIMIT:-2700}
NORUN=${PUSH_MAIN_NORUN:-600}
WORKFLOW_FILE=${PUSH_MAIN_WORKFLOW_FILE:-.github/workflows/ci.yml}
WORKFLOW=CI
common=$(git rev-parse --git-common-dir)
results="$common/recto-ci-main"
known="$common/recto-ci-known"
# How long CI's check job may take, in seconds, and where each green run's time is kept. Owner, 2026-11-12: "Add a rule so the ci doesnt regress in the
# future." The one place to edit the budget, and it only goes down: lower it when a merge makes CI faster (25 min on 2026-11-12, when runs took 18 to 33;
# 21 min on 2026-11-13, when five green runs in a row took 15 min 56 s to 19 min 35 s).
CI_BUDGET=${PUSH_MAIN_CI_BUDGET:-1260}
times="$common/recto-ci-times"
watcher_file=${PUSH_MAIN_WATCHER_FILE:-$common/recto-ci-watcher}

# Runs gh with a bound. Its output on stdout; 124 if it took too long, 127 if there's no gh.
gh_bounded() {
  command -v "$GH" >/dev/null 2>&1 || return 127
  local out rc=0 pid waited=0
  out=$(mktemp "${TMPDIR:-/tmp}/push-main.XXXXXX")
  "$GH" "$@" > "$out" 2>/dev/null &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$waited" -ge $((GH_TIMEOUT * 10)) ]; then
      kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
      rm -f "$out"
      return 124
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  wait "$pid" || rc=$?
  cat "$out"
  rm -f "$out"
  return "$rc"
}

gh_trouble() { # the code from gh_bounded
  case "$1" in
    127) echo "gh isn't installed, so main's CI can't be read" ;;
    124) echo "gh didn't answer within ${GH_TIMEOUT} s (offline?), so main's CI can't be read" ;;
    *) echo "gh failed ($1: signed out, offline, or no access?), so main's CI can't be read" ;;
  esac
}

# A field of a one-line JSON object, without jq: the runs' fields are plain strings and numbers.
field() { printf '%s' "$1" | sed -nE "s/.*\"$2\":(\"([^\"]*)\"|([0-9]+)).*/\2\3/p" | head -1; }
# The objects of a JSON array of flat objects, one a line.
objects() { tr -d '\n' | sed -E 's/^\[//; s/\]$//; s/\},\{/}\n{/g'; echo; }

# Today, and a date's validity.
today() { date '+%Y-%m-%d'; }
is_date() { [[ $1 =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] && date -j -f '%Y-%m-%d' "$1" '+%Y-%m-%d' >/dev/null 2>&1; }

# A commit's red marked known and not yet due: prints its line (sha, until, why, when marked).
known_for() {
  [ -f "$known" ] || return 1
  local sha until why at
  while IFS=$'\t' read -r sha until why at; do
    [ -n "$sha" ] || continue
    case "$1" in "$sha"*) ;; *) continue ;; esac
    [[ ! "$(today)" > "$until" ]] || continue
    printf '%s\t%s\t%s\t%s\n' "$sha" "$until" "$why" "$at"
    return 0
  done < "$known"
  return 1
}

# How a finished run that didn't pass ended: `failed` when one of its parts ran a step that failed or was cut off (cancelled, timed out), `never` when
# none did: its jobs got no runner, or no step (run 43248252599, 2026-11-12: three macos-26 jobs never acquired a runner in 15 minutes and were
# cancelled, and the aggregate `check` of the three-part layout failed for it). Steps that say nothing of the code are left out: the runner's own `Set up
# job`, and that aggregate's `All three parts passed`, which failed whenever a part didn't pass (the one `check` job since then is a real one, and its
# `Full check` counts). When gh can't say, `failed`, the safe side. Also prints, after a tab, the jobs that never ran with GitHub's annotation for each.
run_outcome() {
  local jobs rc=0 bad never="" name conclusion steps failed jid note
  jobs=$(gh_bounded run view "$1" --json jobs --jq '.jobs[] | "\(.name)\t\(.conclusion)\t\(.steps | length)\t\([.steps[] | select(.name != "Set up job" and .name != "All three parts passed") | select(.conclusion == "failure" or .conclusion == "cancelled" or .conclusion == "timed_out")] | length)\t\(.databaseId)"') || rc=$?
  [ "$rc" = 0 ] || { echo failed; return 0; }
  bad=$(printf '%s\n' "$jobs" | awk -F'\t' '$4 > 0' | head -1)
  if [ -n "$bad" ]; then echo failed; return 0; fi
  while IFS=$'\t' read -r name conclusion steps failed jid; do
    [ -n "$name" ] && [ "$conclusion" != success ] || continue
    note=$(gh_bounded api "repos/{owner}/{repo}/check-runs/$jid/annotations" --jq '.[0].message // empty' 2>/dev/null | head -1 || true)
    never="$never${never:+; }$name ($conclusion, $steps steps${note:+: $note})"
  done <<< "$jobs"
  printf 'never\t%s\n' "${never:-no job of it failed}"
}

# The CI workflow's newest runs on main, one JSON object a line, queued ones included. Not asked with --branch or --commit: those leave a queued run out
# (run 43469313717, 2026-11-13: --workflow CI --branch main -L 2 and --commit <sha> both answered without it, the plain list had it first).
runs_on_main() {
  local raw rc=0 line
  raw=$(gh_bounded run list --workflow "$WORKFLOW" -L 30 --json databaseId,headSha,headBranch,status,conclusion,url) || rc=$?
  [ "$rc" = 0 ] || return "$rc"
  printf '%s' "$raw" | objects | while IFS= read -r line; do
    if [ -n "$line" ] && [ "$(field "$line" headBranch)" = main ]; then printf '%s\n' "$line"; fi
  done
  return 0
}

# The last pushed main commit's CI: the newest run on main that wasn't cancelled (a newer push cancels an older run) and whose commit is origin/main or
# in its history. Prints `<state> <sha> <run id> <url>`, state one of green, red, running, none. Fails with gh's code when it can't be read.
last_main_run() {
  local tip runs rc=0 line sha status conclusion
  tip=$(git rev-parse -q --verify refs/remotes/origin/main) || { echo "none - - -"; return 0; }
  runs=$(runs_on_main) || rc=$?
  [ "$rc" = 0 ] || return "$rc"
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    sha=$(field "$line" headSha); status=$(field "$line" status); conclusion=$(field "$line" conclusion)
    [ "$conclusion" != cancelled ] || continue
    git merge-base --is-ancestor "$sha" "$tip" 2>/dev/null || continue
    if [ "$status" != completed ]; then echo "running $sha $(field "$line" databaseId) $(field "$line" url)"
    elif [ "$conclusion" = success ]; then echo "green $sha $(field "$line" databaseId) $(field "$line" url)"
    else
      case "$(run_outcome "$(field "$line" databaseId)" | cut -f1)" in
        never) echo "never $sha $(field "$line" databaseId) $(field "$line" url)" ;;
        *) echo "red $sha $(field "$line" databaseId) $(field "$line" url)" ;;
      esac
    fi
    return 0
  done <<< "$runs"
  echo "none - - -"
}

status() {
  local got rc=0 state sha id url mark
  got=$(last_main_run) || rc=$?
  if [ "$rc" != 0 ]; then echo "push-main: $(gh_trouble "$rc")."; return 2; fi
  read -r state sha id url <<< "$got"
  case "$state" in
    green) echo "push-main: main's CI is green on $(printf '%s' "$sha" | cut -c1-12) ($url)." ;;
    running) echo "push-main: main's CI is still running on $(printf '%s' "$sha" | cut -c1-12) ($url)." ;;
    none) echo "push-main: no finished CI run on main's pushed commits to go by." ;;
    never)
      echo "push-main: main's CI never ran on $(printf '%s' "$sha" | cut -c1-12) ($url): no part got a runner or ran a step, so it's neither red nor green. tools/push-main.sh --watch $(printf '%s' "$sha" | cut -c1-12) runs it again once."
      return 3
      ;;
    red)
      if mark=$(known_for "$sha"); then
        echo "push-main: main's CI is red on $(printf '%s' "$sha" | cut -c1-12) ($url), marked known until $(printf '%s' "$mark" | cut -f2): $(printf '%s' "$mark" | cut -f3)."
        return 0
      fi
      echo "push-main: main's CI is red on $(printf '%s' "$sha" | cut -c1-12) ($url)."
      return 1
      ;;
  esac
  return 0
}

# After a failed run: the failed jobs, the failed tests and their rates from the log, and the artifacts.
report_failure() {
  local id=$1 jobs log rc=0
  jobs=$(gh_bounded run view "$id" --json jobs --jq '.jobs[] | select(.conclusion == "failure") | .name') || rc=$?
  [ "$rc" = 0 ] && [ -n "$jobs" ] && { echo "push-main: failed jobs:"; printf '%s\n' "$jobs" | sed 's/^/  /'; }
  rc=0
  log=$(PUSH_MAIN_GH_TIMEOUT=$((GH_TIMEOUT * 3)) gh_bounded run view "$id" --log-failed) || rc=$?
  if [ "$rc" = 0 ]; then
    # nextest's FAIL lines and the reruns' table, without the log's job, step and time columns.
    # The log is coloured (CARGO_TERM_COLOR=always): the colours go too.
    local failed rates plain
    plain=$(printf '%s\n' "$log" | sed -E "s/$(printf '\033')\[[0-9;]*m//g; s/^([^$(printf '\t')]*)$(printf '\t')[^$(printf '\t')]*$(printf '\t')[^ ]+ /\1: /")
    failed=$(printf '%s\n' "$plain" | grep -E ': +(FAIL|SIGKILL|SIGABRT|SIGSEGV|TIMEOUT) +\[' | sed -E 's/\[ *[0-9.]+s\] \([0-9]+\/[0-9]+\) //' | tr -s ' ' | sort -u | head -30)
    rates=$(printf '%s\n' "$plain" | grep -E 'failed [0-9]+ of [0-9]+$|check: test, failed N of M|check: reruns stopped' | tr -s ' ' | head -30)
    [ -z "$failed" ] || { echo "push-main: failed tests:"; printf '%s\n' "$failed"; }
    [ -z "$rates" ] || { echo "push-main: how often each failed (the first run and the reruns):"; printf '%s\n' "$rates"; }
  else
    echo "push-main: the failed log couldn't be read: $(gh_trouble "$rc")."
  fi
  rc=0
  local artifacts
  artifacts=$(gh_bounded api "repos/{owner}/{repo}/actions/runs/$id/artifacts" --jq '.artifacts[] | "\(.name) \(.size_in_bytes)"') || rc=$?
  if [ "$rc" = 0 ] && [ -n "$artifacts" ]; then
    echo "push-main: artifacts (each failed test's evidence is under evidence/ in check-logs-<attempt>):"
    printf '%s\n' "$artifacts" | while read -r name size; do echo "  $name ($((size / 1024)) KiB): gh run download $id -n $name -D target/ci-$id/$name"; done
  fi
}

# The commit of the newest run on main that isn't cancelled and whose commit has $1 in its history and isn't $1: the run that replaced $1's cancelled
# one. Nothing when there is none, or when gh can't say (the cancelled run is then reported as it is).
newer_run_than() {
  local runs line other
  runs=$(runs_on_main) || return 0
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    other=$(field "$line" headSha)
    [ -n "$other" ] && [ "$other" != "$1" ] && [ "$(field "$line" conclusion)" != cancelled ] || continue
    git merge-base --is-ancestor "$1" "$other" 2>/dev/null || continue
    printf '%s\n' "$other"
    return 0
  done <<< "$runs"
}

mins() { echo "$(($1 / 60)) min $(($1 % 60)) s"; }

# The last five distinct green runs' lines: a run id kept twice (two watchers saw the same run go green, 2026-11-13) counts once, whatever the file holds.
recent_times() { [ -s "$times" ] && awk -F'\t' '!seen[$3]++' "$times" | tail -5; return 0; }

# Keeps a run's time unless that run is already kept. Two watchers can be here at once: the check and the write are one step under a lock directory (mkdir
# is atomic, as in tools/check.sh's app-tier lock), the file is rewritten whole and moved into place, and a lock still there after 5 s is a dead taker's.
keep_time() { # run id, sha, seconds
  local i=0
  until mkdir "$times.lock" 2>/dev/null; do
    i=$((i + 1))
    if [ "$i" -gt 50 ]; then rmdir "$times.lock" 2>/dev/null; i=0; fi
    sleep 0.1
  done
  if ! awk -F'\t' -v id="$1" '$3 == id { found = 1 } END { exit !found }' "$times" 2>/dev/null; then
    { cat "$times" 2>/dev/null; printf '%s\t%s\t%s\t%s\n' "$(date '+%Y-%m-%d %H:%M')" "$2" "$1" "$3"; } > "$times.new.$$" && mv "$times.new.$$" "$times"
  fi
  rmdir "$times.lock" 2>/dev/null
  return 0
}

# A green run's check job against the budget: its seconds are kept, and the median of the last five green runs is what counts, as one slow runner says
# little. Over the budget is said loudly and fails nothing: a runner's speed isn't the commit's fault, but a median over it is the next task (AGENTS.md).
ci_time() {
  local secs median n
  secs=$(gh_bounded run view "$1" --json jobs --jq '[.jobs[] | select(.name == "check") | ((.completedAt | fromdate) - (.startedAt | fromdate))] | max // empty') || return 0
  [[ "$secs" =~ ^[0-9]+$ ]] && [ "$secs" -gt 0 ] || return 0
  keep_time "$1" "$2" "$secs"
  n=$(recent_times | wc -l | tr -d ' ')
  median=$(recent_times | cut -f4 | sort -n | sed -n "$(((n + 1) / 2))p")
  echo "push-main: CI's check took $(mins "$secs"); the median of the last $n green runs is $(mins "$median"), and the budget is $(mins "$CI_BUDGET")."
  budget_warning
}

# The warning every agent sees while CI is over its budget: from the kept times alone, so it asks GitHub nothing and costs nothing. `tools/check.sh` ends
# every run with it. Owner, 2026-11-12: "It should be a stern warning so the agents knows it's something they need to take seriously and look at".
# Nothing when CI is within its budget or no time is kept. Never fails: it is for reading and acting on.
budget_warning() {
  [ -s "$times" ] || return 0
  local n median
  n=$(recent_times | wc -l | tr -d ' ')
  median=$(recent_times | cut -f4 | sort -n | sed -n "$(((n + 1) / 2))p")
  [[ "$median" =~ ^[0-9]+$ ]] && [ "$median" -gt "$CI_BUDGET" ] || return 0
  {
    echo "==================== WARNING: CI IS OVER ITS TIME BUDGET ===================="
    echo "main's CI check took a median of $(mins "$median") over its last $n green runs. The budget is $(mins "$CI_BUDGET"): over by $(mins $((median - CI_BUDGET)))."
    echo "This is not background noise and it is not someone else's job. Read it, and act on it in this task:"
    echo "  1. Add no test that launches the app, sleeps, or waits out a default. Test what Rust decides in process (recto-app's testkit)."
    echo "  2. If your branch adds test seconds, take them out, or say how many and why in your hand-back. Silence about it is a defect."
    echo "  3. If you see a test that costs more than its concern needs, say so in your hand-back with its name and seconds."
    echo "  4. Coordinator: bringing CI back under its budget is the next task, before any other feature work."
    echo "Where the time is: tools/check.sh --slow, and the 'check: ... took' lines in the last CI run's log. The rule: AGENTS.md, \"CI doesn't get slower\"."
    echo "============================================================================="
  } >&2
}

# The paths the CI workflow ignores on a push, one a line, read from its `paths-ignore: [...]` line so the two can't drift. Nothing when it can't be read.
ignore_globs() {
  [ -f "$WORKFLOW_FILE" ] || return 0
  sed -nE "s/^[[:space:]]*paths-ignore:[[:space:]]*\[(.*)\].*/\1/p" "$WORKFLOW_FILE" | head -1 | tr ',' '\n' | sed -E "s/^[[:space:]]*['\"]?//; s/['\"]?[[:space:]]*\$//" | grep -v '^$'
}

# Whether every file changed from $1 to $2 is one the workflow ignores, as GitHub decides for a push (the whole pushed range, not its last commit). False
# for an empty or unreadable range, and when the ignore list can't be read: never "documents only" on a guess.
range_ignored() {
  local files globs f g all
  globs=$(ignore_globs); [ -n "$globs" ] || return 1
  files=$(git diff --name-only "$1" "$2" 2>/dev/null) || return 1
  [ -n "$files" ] || return 1
  while IFS= read -r f; do
    all=1
    while IFS= read -r g; do
      # GitHub's `**` crosses folders, as a bare `*` does in a shell pattern.
      g=${g//\*\*/\*}
      # shellcheck disable=SC2254
      case "$f" in $g) all=0; break ;; esac
    done <<< "$globs"
    [ "$all" = 0 ] || return 1
  done <<< "$files"
  return 0
}

# One watcher a repository: whether process $1 is alive and is a push-main.
watcher_alive() { kill -0 "$1" 2>/dev/null && ps -p "$1" -o command= 2>/dev/null | grep -q push-main; }

register_watcher() {
  local opid="" osha=""
  [ ! -f "$watcher_file" ] || read -r opid osha < "$watcher_file" || true
  if [ -n "$opid" ] && [ "$opid" != "$$" ] && watcher_alive "$opid"; then
    echo "push-main: another watcher is running here (process $opid, on $(printf '%s' "$osha" | cut -c1-12)); it stops once it sees this one has taken over, and this one reports the runs."
  fi
  printf '%s %s\n' "$$" "$sha" > "$watcher_file.new.$$" && mv "$watcher_file.new.$$" "$watcher_file"
  trap 'unregister_watcher' EXIT
}
unregister_watcher() {
  local opid=""
  [ ! -f "$watcher_file" ] || read -r opid _ < "$watcher_file" || true
  [ "$opid" != "$$" ] || rm -f "$watcher_file"
}

# Whether a newer watcher of this repository, on $sha or a commit after it, has taken over from this one: this one then has nothing left to say.
taken_over() {
  local opid="" osha=""
  [ -f "$watcher_file" ] && read -r opid osha < "$watcher_file" || return 1
  [ -n "$opid" ] && [ "$opid" != "$$" ] && watcher_alive "$opid" || return 1
  git merge-base --is-ancestor "$sha" "$osha" 2>/dev/null || return 1
  echo "push-main: stopping the watch of $(printf '%s' "$sha" | cut -c1-12): a newer watcher (process $opid, on $(printf '%s' "$osha" | cut -c1-12)) has taken over and reports the runs."
}

# watch <commit> [<base>]: base is the main the push replaced, so the pushed range is base..commit. Without it: origin/main's previous place when the
# commit is origin/main (the push that put it there), else the commit's parent.
watch() {
  sha=$(git rev-parse "$1") || { echo "push-main: $1 isn't a commit here." >&2; return 2; }
  range_base=""
  if [ -n "${2:-}" ]; then range_base=$(git rev-parse -q --verify "$2^{commit}" || true)
  elif [ "$sha" = "$(git rev-parse -q --verify refs/remotes/origin/main || true)" ]; then range_base=$(git rev-parse -q --verify "refs/remotes/origin/main@{1}" || true)
  fi
  [ -n "$range_base" ] || range_base=$(git rev-parse -q --verify "$sha^" || true)
  began=$(date +%s) reruns=0
  register_watcher
  echo "push-main: watching CI on main for $(printf '%s' "$sha" | cut -c1-12), up to $((LIMIT / 60)) min."
  watch_loop
}

# Follows $sha's run to its end, and says how it ended. Shares sha, began, range_base and reruns with `watch`.
watch_loop() {
  local rc line id="" url="" status conclusion runs seen_running=0
  while :; do
    if taken_over; then return 0; fi
    rc=0
    runs=$(runs_on_main) || rc=$?
    if [ "$rc" != 0 ]; then echo "push-main: $(gh_trouble "$rc"). Look again with tools/push-main.sh --watch $sha." >&2; return 2; fi
    line=$(printf '%s\n' "$runs" | while IFS= read -r l; do [ "$(field "$l" headSha)" = "$sha" ] && { printf '%s\n' "$l"; break; }; done)
    if [ -n "$line" ]; then
      id=$(field "$line" databaseId); url=$(field "$line" url); status=$(field "$line" status); conclusion=$(field "$line" conclusion)
      [ "$status" != completed ] || break
      [ "$status" != in_progress ] || seen_running=1
    elif [ -n "$range_base" ] && range_ignored "$range_base" "$sha"; then
      echo "push-main: no CI run for $(printf '%s' "$sha" | cut -c1-12): everything pushed with it ($(git diff --name-only "$range_base" "$sha" | wc -l | tr -d ' ') files, $(printf '%s' "$range_base" | cut -c1-12)..$(printf '%s' "$sha" | cut -c1-12)) is documents, which CI's workflow skips."
      return 0
    elif [ $(($(date +%s) - began)) -ge "$NORUN" ]; then
      echo "push-main: no CI run at all for $(printf '%s' "$sha" | cut -c1-12) after $((NORUN / 60)) min, and the pushed range ($(git diff --name-only "${range_base:-$sha}" "$sha" 2>/dev/null | wc -l | tr -d ' ') files) isn't only documents. Look on GitHub, or again with tools/push-main.sh --watch $sha." >&2
      return 2
    fi
    if [ $(($(date +%s) - began)) -ge "$LIMIT" ]; then
      if [ "$seen_running" = 0 ] && [ -n "$line" ]; then
        echo "push-main: CI for $(printf '%s' "$sha" | cut -c1-12) never got a runner: still $status after $((LIMIT / 60)) min ($url). That's GitHub's runners, not the code: neither green nor red, and nothing is kept. Look again with tools/push-main.sh --watch $sha." >&2
      else
        echo "push-main: CI for $(printf '%s' "$sha" | cut -c1-12) hadn't finished after $((LIMIT / 60)) min${url:+: $url}. Look again with tools/push-main.sh --watch $sha." >&2
      fi
      return 3
    fi
    sleep "$POLL"
  done
  if [ "$conclusion" = success ]; then
    printf '%s\t%s\t%s\t%s\t%s\n' "$(date '+%Y-%m-%d %H:%M')" "$sha" "$id" success "$(($(date +%s) - began))" >> "$results"
    echo "push-main: CI is green on main $(printf '%s' "$sha" | cut -c1-12): $url"
    ci_time "$id" "$sha"
    return 0
  fi
  # A newer push cancels an older commit's run. That says nothing of the older commit: the verdict on main is the newer run's, so the watch follows it.
  if [ "$conclusion" = cancelled ]; then
    local newer
    newer=$(newer_run_than "$sha")
    if taken_over; then return 0; fi
    if [ -n "$newer" ]; then
      printf '%s\t%s\t%s\t%s\t%s\n' "$(date '+%Y-%m-%d %H:%M')" "$sha" "$id" superseded "$(($(date +%s) - began))" >> "$results"
      echo "push-main: CI on main $(printf '%s' "$sha" | cut -c1-12) was superseded: a newer push ($(printf '%s' "$newer" | cut -c1-12)) cancelled its run ($url). Watching the newer run."
      sha=$newer
      watch_loop
      return $?
    fi
  fi
  local outcome
  outcome=$(run_outcome "$id")
  if [ "$(printf '%s' "$outcome" | cut -f1)" = never ]; then
    printf '%s\t%s\t%s\t%s\t%s\n' "$(date '+%Y-%m-%d %H:%M')" "$sha" "$id" never-ran "$(($(date +%s) - began))" >> "$results"
    echo "push-main: CI NEVER RAN on main $(printf '%s' "$sha" | cut -c1-12) ($conclusion, but no test ran): $url"
    echo "push-main:   $(printf '%s' "$outcome" | cut -f2-)"
    echo "push-main: that's GitHub's runners, not the code: main is neither red nor green until a run completes, and check.sh --full on main says so but stamps."
    if [ "$reruns" -ge 1 ] || [ "${PUSH_MAIN_RERUN:-1}" = 0 ]; then
      echo "push-main: run it again: gh run rerun $id --failed, then tools/push-main.sh --watch $(printf '%s' "$sha" | cut -c1-12)." >&2
      return 2
    fi
    rc=0
    gh_bounded run rerun "$id" --failed > /dev/null || rc=$?
    if [ "$rc" != 0 ]; then
      echo "push-main: the rerun was refused ($(gh_trouble "$rc")): gh run rerun $id --failed, then tools/push-main.sh --watch $(printf '%s' "$sha" | cut -c1-12)." >&2
      return 2
    fi
    reruns=1
    echo "push-main: ran its failed jobs again, once (gh run rerun $id --failed); watching again."
    watch_loop
    return $?
  fi
  printf '%s\t%s\t%s\t%s\t%s\n' "$(date '+%Y-%m-%d %H:%M')" "$sha" "$id" "$conclusion" "$(($(date +%s) - began))" >> "$results"
  echo "push-main: CI is RED on main $(printf '%s' "$sha" | cut -c1-12) ($conclusion): $url"
  report_failure "$id"
  echo "push-main: until it's green or marked known (tools/push-main.sh --known $(printf '%s' "$sha" | cut -c1-12) --until <date> --why \"...\"), tools/check.sh --full on main won't stamp a commit for the push guard."
  return 1
}

mark_known() {
  local sha="" until="" why=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --until) until=${2:-}; shift 2 ;;
      --why) why=${2:-}; shift 2 ;;
      *) [ -z "$sha" ] || { echo "push-main: one commit at a time." >&2; return 2; }; sha=$1; shift ;;
    esac
  done
  [ -n "$sha" ] && [ -n "$until" ] && [ -n "$why" ] || { sed -n '2,13p' "$0" >&2; return 2; }
  sha=$(git rev-parse -q --verify "$sha^{commit}") || { echo "push-main: $sha isn't a commit here." >&2; return 2; }
  is_date "$until" || { echo "push-main: --until is a date, YYYY-MM-DD, not '$until'." >&2; return 2; }
  [[ ! "$until" < "$(today)" ]] || { echo "push-main: $until has passed." >&2; return 2; }
  local latest
  latest=$(date -j -v+7d '+%Y-%m-%d')
  [[ ! "$until" > "$latest" ]] || { echo "push-main: a known red lasts at most 7 days (until $latest): fix it, or mark it again then." >&2; return 2; }
  why=$(printf '%s' "$why" | tr '\t\n' '  ' | cut -c1-200)
  printf '%s\t%s\t%s\t%s\n' "$sha" "$until" "$why" "$(date '+%Y-%m-%d %H:%M')" >> "$known"
  echo "push-main: $(printf '%s' "$sha" | cut -c1-12)'s red CI is marked known until $until: $why"
}

case "${1:-}" in
  "")
    before=$(git rev-parse -q --verify refs/remotes/origin/main || true)
    git push origin main || exit $?
    watch "$(git rev-parse refs/heads/main)" "$before"
    ;;
  --watch) watch "${2:-refs/remotes/origin/main}" ;;
  --status) status ;;
  --budget) budget_warning ;;
  --ignored) ignore_globs ;;
  --known) shift; mark_known "$@" ;;
  *) sed -n '2,13p' "$0" >&2; exit 2 ;;
esac
