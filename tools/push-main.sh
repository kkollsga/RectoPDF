#!/usr/bin/env bash
# push-main: pushes `main` and watches that commit's CI to its end (PLAN R0.7e). On 2026-11-12 `main` was red on CI for four pushes, and one red went
# unseen because nothing watched main's CI after a push.
#
#   tools/push-main.sh                 git push origin main (the pre-push hook still decides), then watch the pushed commit's CI run
#   tools/push-main.sh --watch [sha]   watch a commit's run on main (default: origin/main)
#   tools/push-main.sh --status        one line on the last pushed main commit's CI, for tools/check.sh: exit 0 green, running or marked known, 1 red, 2 unknown, 3 never ran
#   tools/push-main.sh --known <sha> --until YYYY-MM-DD --why "what is known"
#                                      marks that commit's red CI as known until the date, at most 7 days ahead, so `check.sh --full` on main stamps again
#
# A finished watch prints the result, the failed jobs, the failed tests with their "failed N of M" lines (tools/check-rerun.sh) and the artifacts with how
# to fetch them, and keeps the result in the git common dir (recto-ci-main). Every call to `gh` is bounded (PUSH_MAIN_GH_TIMEOUT, 20 s), and `gh` missing,
# signed out or offline is said plainly, never waited on. The watch gives up after PUSH_MAIN_LIMIT seconds (45 min), looking every PUSH_MAIN_POLL (30 s).
set -uo pipefail
cd "$(dirname "$0")/.."
GH=${PUSH_MAIN_GH:-gh}
GH_TIMEOUT=${PUSH_MAIN_GH_TIMEOUT:-20}
POLL=${PUSH_MAIN_POLL:-30}
LIMIT=${PUSH_MAIN_LIMIT:-2700}
WORKFLOW=CI
common=$(git rev-parse --git-common-dir)
results="$common/recto-ci-main"
known="$common/recto-ci-known"

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

# The last pushed main commit's CI: the newest run on main that wasn't cancelled (a newer push cancels an older run) and whose commit is origin/main or
# in its history. Prints `<state> <sha> <run id> <url>`, state one of green, red, running, none. Fails with gh's code when it can't be read.
last_main_run() {
  local tip runs rc=0 line sha status conclusion
  tip=$(git rev-parse -q --verify refs/remotes/origin/main) || { echo "none - - -"; return 0; }
  runs=$(gh_bounded run list --workflow "$WORKFLOW" --branch main -L 10 --json databaseId,headSha,status,conclusion,url) || rc=$?
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
  done < <(printf '%s' "$runs" | objects)
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

watch() {
  sha=$(git rev-parse "$1") || { echo "push-main: $1 isn't a commit here." >&2; return 2; }
  began=$(date +%s) reruns=0
  echo "push-main: watching CI on main for $(printf '%s' "$sha" | cut -c1-12), up to $((LIMIT / 60)) min."
  watch_loop
}

# Follows $sha's run to its end, and says how it ended. Shares sha, began and reruns with `watch`.
watch_loop() {
  local rc line id="" url="" status conclusion
  while :; do
    rc=0
    line=$(gh_bounded run list --workflow "$WORKFLOW" --branch main --commit "$sha" -L 1 --json databaseId,status,conclusion,url | objects | head -1) || rc=$?
    if [ "$rc" != 0 ]; then echo "push-main: $(gh_trouble "$rc"). Look again with tools/push-main.sh --watch $sha." >&2; return 2; fi
    if [ -n "$line" ]; then
      id=$(field "$line" databaseId); url=$(field "$line" url); status=$(field "$line" status); conclusion=$(field "$line" conclusion)
      [ "$status" != completed ] || break
    elif [ $(($(date +%s) - began)) -ge 180 ]; then
      if git rev-parse -q --verify "$sha^" >/dev/null && [ -z "$(git diff --name-only "$sha^" "$sha" | grep -vE '^docs/|\.md$')" ]; then
        echo "push-main: no CI run for $(printf '%s' "$sha" | cut -c1-12) after 3 min: it changed only documents, which CI skips."
        return 0
      fi
      echo "push-main: no CI run for $(printf '%s' "$sha" | cut -c1-12) after 3 min. Look on GitHub, or again with tools/push-main.sh --watch $sha." >&2
      return 2
    fi
    if [ $(($(date +%s) - began)) -ge "$LIMIT" ]; then
      echo "push-main: CI for $(printf '%s' "$sha" | cut -c1-12) hadn't finished after $((LIMIT / 60)) min${url:+: $url}. Look again with tools/push-main.sh --watch $sha." >&2
      return 3
    fi
    sleep "$POLL"
  done
  if [ "$conclusion" = success ]; then
    printf '%s\t%s\t%s\t%s\t%s\n' "$(date '+%Y-%m-%d %H:%M')" "$sha" "$id" success "$(($(date +%s) - began))" >> "$results"
    echo "push-main: CI is green on main $(printf '%s' "$sha" | cut -c1-12): $url"
    return 0
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
    git push origin main || exit $?
    watch "$(git rev-parse refs/heads/main)"
    ;;
  --watch) watch "${2:-refs/remotes/origin/main}" ;;
  --status) status ;;
  --known) shift; mark_known "$@" ;;
  *) sed -n '2,13p' "$0" >&2; exit 2 ;;
esac
