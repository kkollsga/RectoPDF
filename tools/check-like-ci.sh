#!/usr/bin/env bash
# check-like-ci: CI's conditions here, by limits on the process (PLAN R0.7d). Sourced by tools/check.sh for `--like-ci`.
#
# Owner, 2026-11-12, asked to choose between a macOS virtual machine and limits on the process: "Lets do option 2". So nothing is installed: a part or a
# test filter runs with CI's nextest profile, CI's concurrency for it, and its tests held by what macOS gives any process (taskpolicy), as CI's runner, a
# virtual M1 with 3 cores and 7 GB, is slower than this Mac (the lifecycle part: about 4.5 min of tests there, 95 s here alone, 2026-11-12).
#
#   tools/check.sh --like-ci core|window|lifecycle             that part, as CI runs it (CHECK_PART), with the limits
#   tools/check.sh --like-ci '<nextest filter>' [--times N]    those tests, N times, with "failed N of M" (a failure's rerun.txt gives this line)
#       [--profile ci] [--jobs N]                              CI's profile and concurrency unless given: 4 for the app's tests, 3 for the rest
#
# The limits (LIKE_CI_LIMITS, calibrated in docs/engineering/failures.md): the tests, and every app and helper they start, run under `taskpolicy -c background`,
# efficiency cores at background priority. A utility clamp, and throughput or latency tiers, changed a test's time by under 15 %; background took three
# lifecycle tests from about 9.5 s to 36 s (CI: 24.6 s), and the lifecycle tier to 353 s of wall against CI's 250 s, a factor of 1.41 (2026-11-12, load 4 to
# 8). Its disk cost, which AGENTS warns of, measured 4 to 5 times on fsynced writes: real, not starving, and only this run's. Builds aren't held: they run
# first, at full speed, as CI's build time isn't what a test sees. LIKE_CI_LIMITS='' runs with none, to tell a slow machine's failure from any other.
#
# What it can't give, and says so every time: a clean user (this Mac's defaults, fonts, scroll and appearance settings, and keychain), a clean Launch
# Services database (every copy registered here before is still known), a virtual machine's timers, disk and network, and a runner where nothing else runs.
# A failure that depends on those rests on the evidence (R0.7a), the rerun line (R0.7b) and CI's own reruns (R0.7c). Never a pass for the push guard.

LIKE_CI_LIMITS_DEFAULT='-c background'

like_ci_cant() {
  cat <<'SAID'
like-ci: what this can't give, so a pass here isn't CI's pass: a clean user (this Mac's defaults, fonts, appearance and keychain), a clean Launch Services
like-ci: database (every copy registered here before is still known), a virtual machine's timers, disk and network, and a machine with nothing else on it.
SAID
}

# Runs `tools/check.sh --like-ci <args>`. Exits with the tests' result.
like_ci_main() {
  local target="" profile=ci jobs="" times=1
  while [ $# -gt 0 ]; do
    case "$1" in
      --profile) profile=${2:?--profile needs a name}; shift 2 ;;
      --jobs) jobs=${2:?--jobs needs a number}; shift 2 ;;
      --times) times=${2:?--times needs a number}; shift 2 ;;
      -*) echo "like-ci: '$1' isn't an option here: --profile, --jobs, --times." >&2; return 2 ;;
      *) [ -z "$target" ] || { echo "like-ci: one part or one filter (quote it)." >&2; return 2; }; target=$1; shift ;;
    esac
  done
  [ -n "$target" ] || { echo "like-ci: name a part (core, window, lifecycle) or a nextest filter." >&2; return 2; }
  case "$times$jobs" in *[!0-9]*) echo "like-ci: --times and --jobs are numbers." >&2; return 2 ;; esac
  local limits=${LIKE_CI_LIMITS-$LIKE_CI_LIMITS_DEFAULT}
  export CHECK_LIKE_CI_WRAP=""
  [ -z "$limits" ] || CHECK_LIKE_CI_WRAP="taskpolicy $limits"
  export CHECK_LIKE_CI=1 NEXTEST_PROFILE=$profile
  case "$target" in
    core | window | lifecycle)
      [ "$times" = 1 ] || { echo "like-ci: --times goes with a filter; a part runs once." >&2; return 2; }
      [ -n "$jobs" ] || { [ "$target" = core ] && jobs=3; }
      local shown=${jobs:-4}
      echo "like-ci: the $target part as CI runs it: profile $profile, $shown at once, tests held by: ${CHECK_LIKE_CI_WRAP:-nothing}."
      like_ci_cant
      [ -z "$jobs" ] || export NEXTEST_TEST_THREADS=$jobs
      # A part's run, never remembered or stamped; everything in it, as on CI; the slow stage left out, as on CI.
      exec env CHECK_PART="$target" CHECK_ALL=1 HEAVY_RETRY=1 tools/check.sh --full
      ;;
  esac
  local listed app=0 b
  listed=$(HEAVY_CLASS=quick tools/sysguard/heavy.sh cargo nextest list --locked --workspace -E "$target" -T oneline 2>/dev/null) || {
    echo "like-ci: nextest couldn't list '$target' (a filter it can't read, or a build that fails): cargo nextest list --workspace -E '$target'" >&2
    return 2
  }
  listed=$(printf '%s\n' "$listed" | grep -vE '^(info|warning|error)' | grep -E '^[^ ]+ [^ ]+$' || true)
  [ -n "$listed" ] || { echo "like-ci: no test matches '$target'." >&2; return 2; }
  for b in $APP_BINARIES; do
    printf '%s\n' "$listed" | grep -q "^recto-cli::$b " && app=1
  done
  [ -n "$jobs" ] || { [ "$app" = 1 ] && jobs=4 || jobs=3; }
  echo "like-ci: $(printf '%s\n' "$listed" | wc -l | tr -d ' ') test(s), $times time(s): profile $profile, $jobs at once, tests held by: ${CHECK_LIKE_CI_WRAP:-nothing}."
  like_ci_cant
  tools/pdfium.sh >/dev/null
  if [ "$app" = 1 ]; then
    HEAVY_CLASS=normal tools/sysguard/heavy.sh app/build.sh > "${TMPDIR:-/tmp}/like-ci-build.$$" 2>&1 || {
      tail -20 "${TMPDIR:-/tmp}/like-ci-build.$$" >&2; rm -f "${TMPDIR:-/tmp}/like-ci-build.$$"
      echo "like-ci: the app didn't build." >&2; return 1
    }
    rm -f "${TMPDIR:-/tmp}/like-ci-build.$$"
  fi
  local status=0
  export LIKE_TARGET=$target LIKE_JOBS=$jobs LIKE_TIMES=$times LIKE_APP=$app
  HEAVY_RETRY=1 tools/sysguard/heavy.sh bash -c 'like_ci_rounds' "check.sh --like-ci" || status=$?
  return "$status"
}

# Inside the queue's job: the rounds, each with its own evidence, and the table when there was more than one or a failure.
like_ci_rounds() {
  local round status=0 rc junit results tests began
  check_mark=$(mktemp "${TMPDIR:-/tmp}/recto-like-ci.XXXXXX")
  # This run's evidence only, in the tree's own root (CI's, given in RECTO_EVIDENCE, isn't emptied).
  [ -n "${RECTO_EVIDENCE:-}" ] || rm -rf "$CHECK_EVIDENCE_ROOT"
  mkdir -p "$CHECK_EVIDENCE_ROOT"
  results="$CHECK_EVIDENCE_ROOT/like-ci-results.tsv"
  : > "$results"
  for round in $(seq 1 "$LIKE_TIMES"); do
    stage_name="like-ci-round-$round"
    rc=0
    began=$(date +%s)
    if [ "$LIKE_APP" = 1 ]; then
      RECTO_REQUIRE_APP=1 nt --locked --no-fail-fast --workspace -j "$LIKE_JOBS" -E "$LIKE_TARGET" || rc=$?
    else
      nt --locked --no-fail-fast --workspace -j "$LIKE_JOBS" -E "$LIKE_TARGET" || rc=$?
    fi
    echo "like-ci: round $round of $LIKE_TIMES: $([ "$rc" = 0 ] && echo passed || echo failed) in $(($(date +%s) - began)) s."
    [ "$rc" = 0 ] || status=$rc
    junit=$(find "$check_mark.junit" -name "*-like-ci-round-$round.xml" 2>/dev/null | head -1)
    [ -z "$junit" ] || like_ci_note "$junit" "$results"
  done
  if [ "$LIKE_TIMES" -gt 1 ] || [ "$status" != 0 ]; then
    tests=$(awk -F'\t' '$3 == "fail" { print $1 "\t" $2 }' "$results" | sort -u)
    if [ -z "$tests" ] && [ "$status" != 0 ]; then
      echo "like-ci: a round failed with no test failing in it (a build, or the runner): its output is above."
    elif [ -z "$tests" ]; then
      echo "like-ci: every test passed every round ($LIKE_TIMES)."
    else
      echo "like-ci: test, failed N of M:"
      while IFS=$'\t' read -r class name; do
        printf 'like-ci:   %-90s failed %d of %d\n' "$class $name" \
          "$(awk -F'\t' -v c="$class" -v n="$name" '$1 == c && $2 == n && $3 == "fail"' "$results" | wc -l)" \
          "$(awk -F'\t' -v c="$class" -v n="$name" '$1 == c && $2 == n' "$results" | wc -l)"
      done <<< "$tests"
    fi
  fi
  rm -rf "$check_mark" "$check_mark.junit"
  return "$status"
}

# Each test of a round's JUnit, passed or failed, as `class name pass|fail` lines.
like_ci_note() {
  local failed
  failed=$(junit_failed "$1")
  LC_ALL=C awk '
    function attr(line, name) {
      if (!match(line, " " name "=\"[^\"]*\"")) return ""
      return substr(line, RSTART + length(name) + 3, RLENGTH - length(name) - 4)
    }
    /<testcase / { print attr($0, "classname") "\t" attr($0, "name") }' "$1" | while IFS=$'\t' read -r class name; do
    if printf '%s\n' "$failed" | grep -Fxq "$(printf '%s\t%s' "$class" "$name")"; then echo "$class	$name	fail"; else echo "$class	$name	pass"; fi
  done >> "$2"
}
