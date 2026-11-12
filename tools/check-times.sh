#!/usr/bin/env bash
# check-times: how long the check's tests take. Sourced by tools/check.sh, which has `common_dir`, `check_mark`, `stage_name` and the lists in
# tools/check-scope.sh. Nothing here decides whether a run passes.
#
# Every nextest stage writes JUnit (.config/nextest.toml), which the next stage would overwrite, so `nt` copies it after each stage. When a full run ends,
# `times_record` keeps its durations in one small file in `$common_dir/recto-test-times/` (next to the ledger `recto-full-runs`), the newest 20 runs.
# A file is tab-separated lines:
#   run    <commit> <date and time> <all|selective> <part> <result>
#   stage  <name> <seconds>                         the stage's wall time
#   tier   <1|2|3|4> <tests> <test-seconds> <wall>  wall: first start to last end of the tier's tests, per stage, added up. Tiers that share a nextest run overlap.
#   slow   <tests> <test-seconds>                   the slow stage's tests, kept apart: they are the same tests as tiers 3 and 4 run late
#   test   <tier|slow> <stage> <binary> <seconds> <name>    the 40 slowest tests
# `tools/check.sh --slow` reads the newest and prints it against the budgets (TIER_BUDGETS, tools/check-scope.sh).
TIMES_KEEP=20
TIMES_TOP=40

times_dir() { echo "$common_dir/recto-test-times"; }

# Runs nextest and keeps the JUnit it wrote for the stage that is running, whether it passed or not.
nt() {
  local rc=0 junit n
  cargo nextest run "$@" || rc=$?
  junit="${CARGO_TARGET_DIR:-target}/nextest/${NEXTEST_PROFILE:-default}/junit.xml"
  if [ -f "$junit" ]; then
    mkdir -p "$check_mark.junit"
    n=$(find "$check_mark.junit" -type f | wc -l | tr -d ' ')
    cp "$junit" "$check_mark.junit/$(printf '%02d' "$n")-${stage_name:-run}.xml"
    rm -f "$junit"
  fi
  return "$rc"
}

# The tests of the lifecycle tier among the app binaries, one `binary name` a line, from nextest's own filterset so the tier is the one that ran.
times_lifecycle_tests() {
  local args="" b
  for b in $APP_BINARIES; do args="$args --test $b"; done
  # shellcheck disable=SC2086
  cargo nextest list --locked -p recto-cli $args -E "$LIFECYCLE_TESTS" -T oneline 2>/dev/null || true
}

# JUnit files (named NN-stage.xml) to `stage seconds start end binary name` lines. Start and end in seconds, from the clock part of the timestamps.
times_extract() {
  LC_ALL=C awk '
    function epoch(ts,   y, m, d, era, yoe, doy, doe) {
      y = substr(ts, 1, 4) + 0; m = substr(ts, 6, 2) + 0; d = substr(ts, 9, 2) + 0
      if (m <= 2) y--
      era = int(y / 400); yoe = y - era * 400
      doy = int((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1
      doe = yoe * 365 + int(yoe / 4) - int(yoe / 100) + doy
      return (era * 146097 + doe - 719468) * 86400 + substr(ts, 12, 2) * 3600 + substr(ts, 15, 2) * 60 + substr(ts, 18, 6)
    }
    function attr(line, name) {
      if (!match(line, " " name "=\"[^\"]*\"")) return ""
      return substr(line, RSTART + length(name) + 3, RLENGTH - length(name) - 4)
    }
    FNR == 1 { n = split(FILENAME, parts, "/"); stage = parts[n]; sub(/^[0-9]+-/, "", stage); sub(/\.xml$/, "", stage) }
    /<testcase / {
      t = attr($0, "time") + 0; ts = attr($0, "timestamp"); start = (ts == "") ? 0 : epoch(ts)
      printf "%s\t%.3f\t%.3f\t%.3f\t%s\t%s\n", stage, t, start, start + t, attr($0, "classname"), attr($0, "name")
    }' "$@"
}

# The run file's tier, slow and test lines from the extracted tests (stdin). $1: a file of the lifecycle tests, `binary name` a line.
times_summarise() {
  LC_ALL=C awk -F'\t' -v life="$1" -v process_bins="$PROCESS_BINARIES" -v app_bins="$APP_BINARIES" -v top="$TIMES_TOP" '
    BEGIN {
      while ((getline line < life) > 0) is_life[line] = 1
      n = split(process_bins, a, " "); for (i = 1; i <= n; i++) is_process[a[i]] = 1
      n = split(app_bins, a, " "); for (i = 1; i <= n; i++) is_app["recto-cli::" a[i]] = 1
    }
    # A test can be in several stages JUnit files: the app tests are in the workspace stage too, where RECTO_SKIP_APP makes them skip in no time, and the notices
    # are in the bundle stage and the workspace one. It counts once, in the stage where it took longest, i.e. where it really ran. The slow stage tests are
    # the same tests run late: kept apart, never added to a tier.
    {
      stage = $1; secs = $2; start = $3; end = $4; bin = $5; name = $6
      if (stage == "slow") { slow_n++; slow_s += secs; rows++; r_tier[rows] = "slow"; r_stage[rows] = stage; r_bin[rows] = bin; r_secs[rows] = secs; r_name[rows] = name; next }
      key = bin SUBSEP name
      if (!(key in seen)) { seen[key] = ++uniq; order[uniq] = key }
      u = seen[key]
      if (!(u in u_secs) || secs > u_secs[u]) { u_stage[u] = stage; u_secs[u] = secs; u_start[u] = start; u_end[u] = end; u_bin[u] = bin; u_name[u] = name }
    }
    END {
      for (u = 1; u <= uniq; u++) {
        bin = u_bin[u]; name = u_name[u]; stage = u_stage[u]
        if (bin in is_app) tier = ((bin " " name) in is_life) ? 4 : 3
        else if (bin in is_process) tier = 2
        else tier = 1
        n_t[tier]++; s_t[tier] += u_secs[u]
        tk = tier SUBSEP stage
        if (!(tk in lo) || u_start[u] < lo[tk]) lo[tk] = u_start[u]
        if (!(tk in hi) || u_end[u] > hi[tk]) hi[tk] = u_end[u]
        rows++; r_tier[rows] = tier; r_stage[rows] = stage; r_bin[rows] = bin; r_secs[rows] = u_secs[u]; r_name[rows] = name
      }
      for (tier = 1; tier <= 4; tier++) {
        wall = 0
        for (key in lo) { split(key, k, SUBSEP); if (k[1] == tier) wall += hi[key] - lo[key] }
        printf "tier\t%d\t%d\t%.1f\t%.1f\n", tier, n_t[tier], s_t[tier], wall
      }
      printf "slow\t%d\t%.1f\n", slow_n, slow_s
      for (j = 0; j < top && j < rows; j++) {
        best = 0
        for (i = 1; i <= rows; i++) if (!used[i] && (best == 0 || r_secs[i] > r_secs[best])) best = i
        used[best] = 1
        printf "test\t%s\t%s\t%s\t%.2f\t%s\n", r_tier[best], r_stage[best], r_bin[best], r_secs[best], r_name[best]
      }
    }'
}

# Keeps this run's durations. Args: commit, "all" or "selective", the part, the result. Needs `$check_mark.junit` (from `nt`) and `$check_mark.stages`.
times_record() {
  local dir file life files
  [ -d "$check_mark.junit" ] || return 0
  dir=$(times_dir)
  mkdir -p "$dir"
  file="$dir/$(date '+%Y%m%dT%H%M%S')-$(printf '%s' "$1" | cut -c1-12).tsv"
  life=$(mktemp "${TMPDIR:-/tmp}/recto-times.XXXXXX")
  if [ -n "$(find "$check_mark.junit" -name '*app-tests*' -o -name '*registrations*' -o -name '*slow*')" ]; then times_lifecycle_tests > "$life"; fi
  {
    printf 'run\t%s\t%s\t%s\t%s\t%s\n' "$1" "$(date '+%Y-%m-%d %H:%M')" "$2" "$3" "$4"
    [ ! -f "$check_mark.stages" ] || LC_ALL=C awk '{ printf "stage\t%s\t%s\n", $1, $2 }' "$check_mark.stages"
    # shellcheck disable=SC2046
    times_extract $(find "$check_mark.junit" -name '*.xml' | sort) | times_summarise "$life"
  } > "$file.part" && mv "$file.part" "$file" || rm -f "$file.part"
  rm -f "$life"
  # Only the newest TIMES_KEEP runs are kept.
  files=$(ls -1 "$dir"/*.tsv 2>/dev/null | sort -r | tail -n +$((TIMES_KEEP + 1)))
  [ -z "$files" ] || printf '%s\n' "$files" | while IFS= read -r old; do rm -f "$old"; done
}

# `check.sh --slow`: the newest kept run against the budgets, then how the latest run of everything compares with the one before it.
times_report() {
  local dir files latest all two f
  dir=$(times_dir)
  files=$(ls -1 "$dir"/*.tsv 2>/dev/null | sort -r || true)
  if [ -z "$files" ]; then
    echo "No test durations kept yet. A full run (tools/check.sh --full) keeps them in $dir."
    return 0
  fi
  latest=$(printf '%s\n' "$files" | head -1)
  LC_ALL=C awk -F'\t' -v budgets="$TIER_BUDGETS" '
    BEGIN {
      n = split(budgets, b, " "); for (i = 1; i <= n; i++) { split(b[i], kv, ":"); budget[kv[1]] = kv[2] }
      name[1] = "core"; name[2] = "process"; name[3] = "window"; name[4] = "lifecycle"
    }
    $1 == "run" { printf "Latest kept run: commit %s, %s, %s, part %s, %s\n\n", substr($2, 1, 12), $3, ($4 == "all" ? "everything" : "selective"), $5, $6 }
    $1 == "stage" { stagewall[$2] = $3; stages = stages sprintf("  %-14s %6d s\n", $2, $3); if ($2 == "slow") slow_stage = $3 }
    $1 == "tier" { tests[$2] = $3; secs[$2] = $4; wall[$2] = $5 }
    $1 == "slow" { slow_tests = $2; slow_secs = $3 }
    $1 == "test" { top = top sprintf("  %7.1f s  %-5s %s  %s\n", $5, $2, $4, $6) }
    END {
      print "Slowest tests (seconds, tier, binary, name):"
      printf "%s\n", top
      print "Tiers (a tier that shares a nextest run with another has no wall of its own here; the pair gets the run'"'"'s wall against both budgets added):"
      printf "  %-4s %-10s %6s %12s %8s\n", "tier", "", "tests", "test-seconds", "budget s"
      pair_of[1] = 2; pair_of[2] = 1; pair_of[3] = 4; pair_of[4] = 3
      for (t = 1; t <= 4; t++) {
        shared = (tests[t] > 0 && tests[pair_of[t]] > 0)
        note = ""
        if (tests[t] == 0) note = "  (no tests in this run)"
        else if (shared) note = "  (shares its run with tier " pair_of[t] ")"
        else if (wall[t] > budget[t]) note = sprintf("  wall %d s: OVER by %d s", wall[t], wall[t] - budget[t])
        else note = sprintf("  wall %d s", wall[t])
        printf "  %-4d %-10s %6d %12.0f %8d%s\n", t, name[t], tests[t], secs[t], budget[t], note
      }
      for (p = 1; p <= 2; p++) {
        ta = (p == 1) ? 1 : 3; tb = ta + 1
        if (tests[ta] > 0 && tests[tb] > 0) {
          label = (p == 1) ? "core + process" : "window + lifecycle"
          split((p == 1) ? "changed-tests other-tests" : "app-tests", sn, " ")
          pw = 0; for (k in sn) pw += stagewall[sn[k]]
          pb = budget[ta] + budget[tb]
          printf "  tiers %d+%d (%s): %d s wall against %d s (%d + %d)%s\n", ta, tb, label, pw, pb, budget[ta], budget[tb], (pw > pb) ? sprintf(", OVER by %d s", pw - pb) : ""
        }
      }
      print "  The spans of tiers that share a run overlap when run locally; CI runs them as separate jobs and gives each tier'"'"'s own wall."
      printf "\nSlow stage (the same tests as tiers 3 and 4, with every waiting point late): %d tests, %.0f test-seconds", slow_tests, slow_secs
      if (slow_stage != "") printf ", %d s wall", slow_stage
      printf "\n\nStages of that run:\n%s", stages
    }' "$latest"
  # Runs of everything: scope all and part all. The newest two, stage by stage.
  all=""
  for f in $files; do
    if LC_ALL=C awk -F'\t' '$1 == "run" && $4 == "all" && $5 == "all" { found = 1 } END { exit !found }' "$f"; then all="$all $f"; fi
    [ "$(printf '%s\n' $all | grep -c .)" -lt 2 ] || break
  done
  two=$(printf '%s\n' $all | grep -c . || true)
  echo
  if [ "$two" -lt 2 ]; then
    echo "Stage times against the run of everything before it: only $([ "$two" = 0 ] && echo no || echo one) run of everything is kept so far."
    return 0
  fi
  echo "Stage times, the latest run of everything against the one before it:"
  # shellcheck disable=SC2086
  LC_ALL=C awk -F'\t' '
    FNR == 1 { fileno++ }
    $1 == "run" { commit[fileno] = substr($2, 1, 12) }
    $1 == "stage" { sec[fileno, $2] = $3; if (!($2 in seen)) { seen[$2] = 1; order[++n] = $2 } }
    END {
      printf "  %-14s %8s %8s %8s     (%s against %s)\n", "stage", "latest", "before", "change", commit[1], commit[2]
      for (i = 1; i <= n; i++) {
        s = order[i]; a = ((1, s) in sec) ? sec[1, s] : "-"; p = ((2, s) in sec) ? sec[2, s] : "-"
        d = (a != "-" && p != "-") ? sprintf("%+d", a - p) : "-"
        printf "  %-14s %8s %8s %8s\n", s, a, p, d
      }
    }' $all
}
