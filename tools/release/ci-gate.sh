#!/usr/bin/env bash
# release/ci-gate: the release's gate on CI (release.yml's *Tag and CI* job): did CI pass on main for the code this tag has? Read-only: git, and
# `gh run list`. Run in a checkout with main's history (release.yml's has every branch, fetch-depth 0); a tag pushed before main is refused.
#
#   tools/release/ci-gate.sh <commit> [--repo owner/name]   The gate: asks, and waits for a run that is still going. Exit 0 passed, 1 refused.
#   tools/release/ci-gate.sh --commits <commit>             Only the commits it would ask about, one a line, or the refusal. Exit 0 or 1.
#   tools/release/ci-gate.sh --decide <runs.json> <seconds waited> <commit>...
#                                                           Only the decision, on `gh run list --json` output as the gate asks for it (a list
#                                                           of runs with headSha, headBranch, event, status, conclusion, createdAt, url).
#                                                           Exit 0 passed, 1 refused, 3 wait and ask again. tools/release/ci-gate-test.sh holds it.
#
# Which commits: CI skips a push that changes only documents, and runs once on a push's newest commit, whatever the push carried. So the gate
# asks about the tagged commit back to the newest on main's own line (first parents) that changes anything but documents: between those two
# only documents differ, and a run on any of them checked the code this tag has. (Until 0.1.1 it asked about one commit, the newest that changed
# code on any line of history: for v0.1.1 that was a branch's commit under a merge, which no push ever ended on, and the gate waited its hour.)
# The tag's commit has to be on main's own line: one that isn't on main at all, or is there only as a branch's commit under a merge, never had
# a run on main, so it's refused at once rather than waited for (since 0.1.2).
#
# Which runs count (since 0.1.2): only ci.yml's runs on main from a push. A branch's green run doesn't: a build/* branch is checked before its
# merge, and what's released is main. Among those runs on these commits, newest first, the first that wasn't cancelled decides: a run a newer
# push cancelled says nothing either way, so the gate looks past it to an older run on the same commit, or on the next commit down. A run that
# went green on a re-run counts as green: a run is listed once, as its latest attempt is, and its latest attempt passed on this code (two of
# main's runs on 2026-11-16 never got a runner on their first attempt and went green on a re-run). Anything else completed (failure,
# timed out, startup failure, ...) is refused.
#
# How long: a run still queued or going is waited for, at most an hour in all; with no run on main for these commits (or only cancelled ones),
# ten minutes, as CI starts within seconds of a push, so none is coming. Either way it gives up with what it saw, and release.yml withdraws the tag.
set -euo pipefail

FIELDS=databaseId,headSha,headBranch,event,status,conclusion,attempt,createdAt,url
NO_RUN_LIMIT=600   # seconds with no run on main for these commits before none is coming
RUN_LIMIT=3600     # seconds waited in all for a run that is going

refuse() { echo "::error::$*"; exit 1; }

# commits_for <commit>: the commits to ask about, newest first, or a refusal (exit 1) that says why. In the current directory's repository.
commits_for() {
  local c="$1" full code
  full=$(git rev-parse --verify --quiet "$c^{commit}") || refuse "$c isn't a commit here"
  git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null \
    || refuse "main's history isn't here (no origin/main), so whether $full is on main can't be said: fetch main first"
  git merge-base --is-ancestor "$full" origin/main \
    || refuse "the tag's commit $full isn't on main: a release is tagged on a commit of main, after main is pushed (tools/push-main.sh)"
  # grep reads to the end (no -q): under pipefail, rev-list ended early by a closed pipe would read as "not on main's line".
  git rev-list --first-parent origin/main | grep -x "$full" >/dev/null \
    || refuse "the tag's commit $full is on main only as a branch's commit under a merge, which CI on main never ran on: tag the merge on main's own line"
  code=$(git log -1 --first-parent --format=%H "$full" -- . ':(exclude)docs/**' ':(exclude,glob)**/*.md')
  [ -n "$code" ] || refuse "no commit at or before $full changes anything but documents"
  # Up to and with $code; awk reads to the end, as sed's q would close the pipe on rev-list, which pipefail reads as a failure.
  git rev-list --first-parent "$full" | awk -v code="$code" '!past { print } $0 == code { past = 1 }'
}

# decide <runs.json> <seconds waited> <commit>...: says what the runs mean for these commits; exit 0 passed, 1 refused, 3 wait.
decide() {
  local file="$1" waited="$2" lines first="" cancelled="" status conclusion sha url
  shift 2
  # Only main's runs from a push, on these commits, newest first, as "status conclusion sha url" ("none" for a conclusion not yet had).
  lines=$(jq -r --arg cs "$*" '
      ($cs | split(" ")) as $c
      | map(select(.headBranch == "main" and .event == "push" and (.headSha as $s | $c | any(. == $s))))
      | unique_by(.databaseId) | sort_by(.createdAt) | reverse | .[]
      | "\(.status) \(if (.conclusion // "") == "" then "none" else .conclusion end) \(.headSha) \(.url)"' "$file") \
    || refuse "the list of CI's runs couldn't be read: $(head -c 300 "$file")"
  while read -r status conclusion sha url; do
    [ -n "${status:-}" ] || continue
    if [ "$status" = completed ] && [ "$conclusion" = cancelled ]; then
      cancelled="${cancelled:-$url}"
      continue
    fi
    first="$status $conclusion $sha $url"
    break
  done <<< "$lines"
  if [ -n "$first" ]; then
    read -r status conclusion sha url <<< "$first"
    if [ "$status" = completed ]; then
      [ "$conclusion" = success ] && { echo "CI passed on main on $sha: $url"; exit 0; }
      refuse "CI on main on $sha ended with $conclusion: $url"
    fi
    [ "$waited" -lt "$RUN_LIMIT" ] || refuse "CI on main on $sha is still $status after an hour: $url"
    echo "CI on main on $sha: $status; waiting ($url)"
    exit 3
  fi
  if [ "$waited" -ge "$NO_RUN_LIMIT" ]; then
    [ -z "$cancelled" ] || refuse "CI's only runs on main for these commits were cancelled ($cancelled), and no other started in ten minutes: re-run it (gh run rerun), then tag again. Commits: $*"
    refuse "no CI run on main on any of these after ten minutes, so none is coming: $*"
  fi
  if [ -n "$cancelled" ]; then echo "CI on main: only a cancelled run so far ($cancelled); waiting"
  else echo "CI on main on $*: not started yet; waiting"; fi
  exit 3
}

# gate <commit> <repo>: asks gh every 30 s (CI_GATE_POLL) until the runs decide.
gate() {
  local c="$1" repo="$2" commits started json rc one
  commits=$(commits_for "$c") || { echo "$commits"; exit 1; }
  commits=$(tr '\n' ' ' <<< "$commits")
  commits=${commits% }
  echo "The tag's code, as CI would have checked it, is on: $commits"
  started=$(date +%s)
  json=$(mktemp "${TMPDIR:-/tmp}/ci-gate.XXXXXX")
  trap 'rm -f "$json" "$json".*' EXIT
  while :; do
    : > "$json.all"
    for one in $commits; do
      gh run list --repo "$repo" --workflow ci.yml --branch main --event push --commit "$one" --limit 20 --json "$FIELDS" >> "$json.all" 2> "$json.err" \
        || echo "gh couldn't list CI's runs on $one: $(head -c 300 "$json.err")"
    done
    jq -s 'add // []' "$json.all" > "$json"
    rc=0
    # shellcheck disable=SC2086 # the commits, one word each
    (decide "$json" "$(($(date +%s) - started))" $commits) || rc=$?
    [ "$rc" = 3 ] || exit "$rc"
    sleep "${CI_GATE_POLL:-30}"
  done
}

case "${1:-}" in
  --commits)
    [ $# = 2 ] || { sed -n '5p' "$0" >&2; exit 2; }
    commits_for "$2"
    ;;
  --decide)
    [ $# -ge 4 ] && [ -f "$2" ] && [[ "$3" =~ ^[0-9]+$ ]] || { sed -n '6,7p' "$0" >&2; exit 2; }
    file="$2" waited="$3"
    shift 3
    decide "$file" "$waited" "$@"
    ;;
  '' | -*)
    sed -n '4,9p' "$0" >&2
    exit 2
    ;;
  *)
    c="$1" repo="${GITHUB_REPOSITORY:-}"
    if [ "${2:-}" = --repo ]; then repo="${3:-}"; fi
    [ -n "$repo" ] || { echo "ci-gate: which repository? --repo owner/name, or GITHUB_REPOSITORY" >&2; exit 2; }
    gate "$c" "$repo"
    ;;
esac
