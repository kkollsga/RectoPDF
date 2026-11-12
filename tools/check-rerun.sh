#!/usr/bin/env bash
# check-rerun: a failure measured the first time (PLAN R0.7c). Sourced by tools/check.sh, whose `nt` calls `rerun_failed` after a nextest stage fails.
#
# On CI a test that failed cost a rerun of the whole job to learn whether it fails every time or one time in three (owner, 2026-11-12: "What's the solution to
# ensure we are able to debug and fix issues after one failure?"). So a failed stage runs just its failed tests again, CHECK_RERUN times (CI: 4, here: 0
# unless asked), each round with its own evidence folder (crates/recto-cli/tests/common/evidence.rs), and prints "test, failed N of M" counting the first
# run. The stage still fails, whatever the reruns say: a test that passes four times of five is a bug, not a pass.
#
# Bounds, so a failing part can't run on: at most CHECK_RERUN_TESTS tests (8) are rerun, and a round starts only while the reruns have taken less than
# CHECK_RERUN_SECONDS (240 s). A round can't take longer than the slowest test's nextest limit (.config/nextest.toml: 60 s, ended at 300 s), so the most a
# failing stage adds is 240 + 300 s, 9 minutes; a failed test of 20 s adds four rounds of about 25 s, under 2 minutes.

# The failed tests in a nextest JUnit file, as `<binary id> <test name>` a line: a testcase with a <failure> or <error> in it.
junit_failed() {
  LC_ALL=C awk '
    function attr(line, name) {
      if (!match(line, " " name "=\"[^\"]*\"")) return ""
      return substr(line, RSTART + length(name) + 3, RLENGTH - length(name) - 4)
    }
    /<testcase / { name = attr($0, "name"); class = attr($0, "classname"); open = ($0 !~ /\/>[[:space:]]*$/); next }
    open && /<(failure|error)[ >]/ { if (!((class, name) in said)) { print class "\t" name; said[class, name] = 1 } }
    /<\/testcase>/ { open = 0 }' "$1"
}

# The arguments of a nextest run without its filter (-E and what follows) and without --no-tests, so a rerun can name its own tests.
args_without_filter() {
  local skip=0 a
  for a in "$@"; do
    if [ "$skip" = 1 ]; then skip=0; continue; fi
    case "$a" in
      -E | --filterset | --filter-expr) skip=1 ;;
      -E* | --filterset=* | --filter-expr=* | --no-tests=*) ;;
      *) printf '%s\0' "$a" ;;
    esac
  done
}

# Runs the failed tests of $1 (a JUnit file) again, with the nextest arguments after it, and prints the table. Never changes the stage's result.
rerun_failed() {
  local junit=$1 rounds=${CHECK_RERUN:-0} cap=${CHECK_RERUN_TESTS:-8} budget=${CHECK_RERUN_SECONDS:-240}
  shift
  [ "$rounds" -gt 0 ] 2>/dev/null || return 0
  [ -f "$junit" ] || { echo "check: no JUnit from the failed stage, so its failures can't be run again."; return 0; }
  local failed total filter="" class name round began elapsed results
  failed=$(junit_failed "$junit")
  [ -n "$failed" ] || { echo "check: the stage failed with no failed test in its JUnit (a build, or a test binary that didn't list), so nothing is run again."; return 0; }
  total=$(printf '%s\n' "$failed" | wc -l | tr -d ' ')
  if [ "$total" -gt "$cap" ]; then
    echo "check: $total tests failed; the first $cap are run again (CHECK_RERUN_TESTS). So many at once is usually one cause: look at the first."
    failed=$(printf '%s\n' "$failed" | head -n "$cap")
  fi
  while IFS=$'\t' read -r class name; do
    filter="$filter${filter:+ | }(binary_id(=$class) & test(=$name))"
  done <<< "$failed"
  local args=()
  while IFS= read -r -d '' a; do args+=("$a"); done < <(args_without_filter "$@")
  local evidence="${CHECK_EVIDENCE_ROOT:-${CARGO_TARGET_DIR:-target}/recto-evidence}" stage=${stage_name:-run}
  mkdir -p "$evidence"
  results="$evidence/$stage-reruns.tsv"
  : > "$results"
  echo "check: running the $(printf '%s\n' "$failed" | wc -l | tr -d ' ') failed test(s) again, up to $rounds times, with their evidence (CHECK_RERUN=$rounds; at most ${budget} s of rounds)."
  began=$(date +%s)
  local out_junit="${CARGO_TARGET_DIR:-target}/nextest/${NEXTEST_PROFILE:-default}/junit.xml" done_rounds=0
  for round in $(seq 1 "$rounds"); do
    elapsed=$(($(date +%s) - began))
    if [ "$elapsed" -ge "$budget" ]; then
      echo "check: reruns stopped after $done_rounds of $rounds rounds: ${elapsed} s, past CHECK_RERUN_SECONDS=$budget."
      break
    fi
    rm -f "$out_junit"
    mkdir -p "$evidence/$stage-rerun-$round"
    RECTO_EVIDENCE="$evidence/$stage-rerun-$round" CHECK_STAGE="$stage-rerun-$round" CHECK_STAGE_JOBS="$(nt_jobs "${args[@]}")" \
      cargo nextest run "${args[@]}" -E "$filter" > "$evidence/$stage-rerun-$round/nextest.log" 2>&1 || true
    done_rounds=$round
    while IFS=$'\t' read -r class name; do
      if [ ! -f "$out_junit" ]; then
        printf '%s\t%s\tnot run\n' "$class" "$name" >> "$results"
      elif junit_failed "$out_junit" | grep -Fxq "$(printf '%s\t%s' "$class" "$name")"; then
        printf '%s\t%s\tfail\n' "$class" "$name" >> "$results"
      else
        printf '%s\t%s\tpass\n' "$class" "$name" >> "$results"
      fi
    done <<< "$failed"
    rm -f "$out_junit"
  done
  rerun_table "$failed" "$results" "$done_rounds"
  rerun_table "$failed" "$results" "$done_rounds" > "$evidence/$stage-reruns.txt"
  echo "check: each round's output and evidence are in $evidence/$stage-rerun-<round>/, the table in $evidence/$stage-reruns.txt. The stage still fails."
}

# The table: each failed test, and how often it failed counting the first run. $1 the tests, $2 the results file, $3 the rounds run.
rerun_table() {
  local class name fails runs
  echo "check: test, failed N of M (the first run and $3 more):"
  while IFS=$'\t' read -r class name; do
    fails=$(( 1 + $(awk -F'\t' -v c="$class" -v n="$name" '$1 == c && $2 == n && $3 == "fail"' "$2" | wc -l) ))
    runs=$(( 1 + $(awk -F'\t' -v c="$class" -v n="$name" '$1 == c && $2 == n && $3 != "not run"' "$2" | wc -l) ))
    printf 'check:   %-90s failed %d of %d\n' "$class $name" "$fails" "$runs"
  done <<< "$1"
}

# At the end of a failed run: where each failed test's evidence is, one folder a test, and the command that runs it again (its rerun.txt).
evidence_report() {
  [ "$1" != 0 ] && [ -d "${CHECK_EVIDENCE_ROOT:-}" ] || return 0
  local folders folder n
  folders=$(find "$CHECK_EVIDENCE_ROOT" -mindepth 2 -maxdepth 2 -type d -name '*--*' 2>/dev/null | sort)
  [ -n "$folders" ] || return 0
  n=$(printf '%s\n' "$folders" | wc -l | tr -d ' ')
  echo "check: evidence of $n failed test run(s), one folder each, in $CHECK_EVIDENCE_ROOT:" >&2
  printf '%s\n' "$folders" | head -20 | while IFS= read -r folder; do
    echo "  ${folder#"$CHECK_EVIDENCE_ROOT"/}" >&2
    [ ! -f "$folder/rerun.txt" ] || echo "    run it again here as it ran: $(cat "$folder/rerun.txt")" >&2
  done
  [ "$n" -le 20 ] || echo "  and $((n - 20)) more" >&2
}
