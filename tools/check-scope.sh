#!/usr/bin/env bash
# check-scope: what tools/check.sh runs, chosen from what changed. Sourced by check.sh, which has `crates_in` and `branch`.
#
# Everything the selection depends on is in this file, once: the registration guards, the tiers of tests that launch the app, and the rules that say which
# changes run what. A name here that no test has any more fails the check before anything is queued (`scope_names_missing`), so a rename can't quietly
# drop a guard or move a test to another tier. tools/check-test.sh proves the rules without a build.

# --- Registration guards -------------------------------------------------------------------------------------------------------------------------------
# The tests that exist so a feature can't forget to register itself: every command has a table row, rows are documented and their examples parse, every
# emitted event is listed, every row is wired up in `recto` and enforced at every level, every menu id has an enable rule, every person-originated call
# is frozen during a hand-off, the known `qa state` paths are sorted, the slow stage's names exist. They run in `--quick` whichever crate changed, and in a
# selective `--full`, because a change in one crate breaks the list in another (recto-session's edit showed up in recto-agent's table, 2 minutes into a
# full check). `test(~x)` is a substring of the test's name (its module path included), and `test(~module::)` is a whole module's tests.
# A new guard goes here, with the failure it catches in its own doc comment.
GUARD_TESTS='test(~every_thread_the_code_starts_is_named) | test(~table::tests::) | test(~every_command_has_a_row_and_every_session_row_has_a_command) | test(~every_error_says_the_next_step) | test(~every_edit_through_the_agent_interface_is_an_agents) | test(~each_level_says_what_its_commands_do) | test(~every_row_that_runs_here_is_wired_up) | test(~every_row_answers_in_plain_words_and_in_versioned_json) | test(~every_row_is_enforced_at_every_level_over_the_socket) | test(~the_known_paths_and_exceptions_are_sorted_with_no_duplicates) | test(~every_test_the_slow_stage_names_is_there) | test(~every_menu_id_has_an_enable_rule) | test(~every_person_originated_call_is_frozen_during_a_hand_off) | test(~a_frozen_queue_takes_nothing_a_person_does)'

# --- Tiers of the tests that launch the app --------------------------------------------------------------------------------------------------------------
# Every test in these binaries (recto-cli's) starts the real app in a quiet run. APP_BINARIES is all of them: `scope_names_missing` fails when a test file
# that drives the app is not named here, so a new one can't run in no tier.
APP_BINARIES='app restore compare long_session registrations'
# Lifecycle: the app starting, updating, being killed and found by Launch Services: every update test end to end, the quiet-run and Launch Services
# tests, long sessions, a thousand tabs, tabs leaving in bulk, comparisons that leave no views behind, and what is registered afterwards.
LIFECYCLE_TESTS='(binary(=restore) & test(~update::)) | (binary(=app) & test(~quiet::)) | binary(=long_session) | binary(=registrations) | (binary(=compare) & test(~comparisons_leave_no_views_behind)) | (binary(=restore) & test(~a_thousand_tabs_come_back_cold))'
# Window: everything else in the app binaries: drawing, AppKit, the bridge, reading and restoring state. The rest by construction, so a test is in exactly
# one tier: the one its name puts it in, or this.
WINDOW_TESTS="(binary(=app) | binary(=restore) | binary(=compare)) & !($LIFECYCLE_TESTS)"
# --- Tiers by time ---------------------------------------------------------------------------------------------------------------------------------------
# The test system's four tiers (docs/reviews/2026-11-12-development-speed.md §5), as `check.sh --slow` reports them against a budget. Tier 1 (core) is every
# test that is in none of the lists below, in a single process; tier 3 is the window tier and tier 4 the lifecycle tier above; tier 2 (process) is these
# binaries, whose tests spawn `recto`, the render worker, the helpers or the installer. A binary is `crate::file` (a test file, or a folder with a main.rs) as
# nextest names it; `scope_names_missing` fails when one is not there, so a rename can't quietly move its tests into tier 1.
PROCESS_BINARIES='recto-cli::corpus recto-cli::dev_build recto-cli::export_svg recto-cli::extract recto-cli::find recto-cli::generated recto-cli::index recto-cli::levels recto-cli::memory recto-cli::missing_marks recto-cli::notices recto-cli::print recto-cli::render recto-cli::save recto-cli::select recto-cli::session recto-cli::socket recto-update::installer recto-platform::helper recto-platform::walls recto-reader::engine recto-render-service::corpus recto-render-service::hostile recto-render-service::memory recto-render-service::sandbox recto-render-service::sequence recto-render-service::surfaces recto-render-service::worker'
# Wall-clock budget in seconds for each tier, the one place to edit them. Over budget is reported by `check.sh --slow`, plainly: it is read in a process review.
TIER_BUDGETS='1:60 2:60 3:90 4:240'

# The crates whose change (or whose dependencies' change) can change what the window shows or does.
WINDOW_CRATES='recto-ffi recto-reader recto-session recto-agent recto-control'
# The changes that run the lifecycle tier: a crate it is about (recto-update, recto-platform), the app, the record code, the bridge's modules for updates and
# sleeping, and the tests themselves. Not recto-core, recto-session in general or recto-agent: those reach the window tier.
LIFECYCLE_PATHS='^app/|^crates/(recto-update|recto-platform)/|^crates/recto-session/src/record\.rs$|^crates/recto-control/src/record_store\.rs$|^crates/recto-ffi/src/(updates|sleeper|saver)(\.rs|/)|^crates/recto-app/src/sleep_queue\.rs$|^crates/recto-cli/tests/(restore|long_session|registrations|compare|common)|^crates/recto-cli/tests/app/quiet\.rs$'

# Whether a line is in a list. Not `printf | grep -q`: grep quits at the first hit, printf dies of SIGPIPE, and under pipefail that reads as a miss.
has_line() { case $'\n'"$1"$'\n' in *$'\n'"$2"$'\n'*) return 0 ;; *) return 1 ;; esac; }

# The names above that no test or module has. Prints one per line, with where it is named. Reads the sources under $1 (the repository).
scope_names_missing() {
  local root=${1:-.} wanted name kind
  local fns mods
  fns=$(grep -rhoE --include='*.rs' '(^|[^a-zA-Z0-9_])fn [a-z_0-9]+' "$root/crates" 2>/dev/null | sed -E 's/.*fn //' | sort -u)
  mods=$(grep -rhoE --include='*.rs' '(^|[^a-zA-Z0-9_])mod [a-z_0-9]+' "$root/crates" 2>/dev/null | sed -E 's/.*mod //' | sort -u)
  # test(~name) in a filterset: a function, or a module when it ends in `::`, or a module path ending in `tests::`.
  wanted=$(printf '%s\n%s\n' "$GUARD_TESTS" "$LIFECYCLE_TESTS" | grep -oE 'test\(~[A-Za-z0-9_:]+\)' | sed -E 's/^test\(~//; s/\)$//' | sort -u)
  for name in $wanted; do
    case "$name" in
      *::)
        for kind in $(printf '%s' "$name" | tr -s ':' ' '); do
          has_line "$mods" "$kind" || echo "module $name"
        done
        ;;
      *) has_line "$fns" "$name" || echo "test $name" ;;
    esac
  done
  # binary(=x): a test file or folder of recto-cli.
  for name in $(printf '%s\n%s\n' "$WINDOW_TESTS" "$LIFECYCLE_TESTS" | grep -oE 'binary\(=[a-z_0-9]+\)' | sed -E 's/^binary\(=//; s/\)$//' | sort -u); do
    [ -f "$root/crates/recto-cli/tests/$name.rs" ] || echo "test binary $name"
  done
  for name in $APP_BINARIES; do
    [ -f "$root/crates/recto-cli/tests/$name.rs" ] || echo "app binary $name"
  done
  # crate::file for tier 2: a file, or a folder with a main.rs, in that crate's tests.
  for name in $PROCESS_BINARIES; do
    [ -f "$root/crates/${name%%::*}/tests/${name##*::}.rs" ] || [ -f "$root/crates/${name%%::*}/tests/${name##*::}/main.rs" ] || echo "process binary $name"
  done
  # A test file that starts the app and isn't in APP_BINARIES runs in no tier.
  local file bin
  for file in "$root"/crates/recto-cli/tests/*.rs "$root"/crates/recto-cli/tests/*/*.rs; do
    [ -f "$file" ] || continue
    grep -qE 'common::app|app_path' "$file" || continue
    bin=$(basename "$file" .rs)
    [ "$(dirname "$file")" = "$root/crates/recto-cli/tests" ] || bin=$(basename "$(dirname "$file")")
    case "$bin" in common | app_*) continue ;; esac
    case " $APP_BINARIES " in *" $bin "*) ;; *) echo "starts the app, but is in no tier: tests/${file#"$root"/crates/recto-cli/tests/} (add $bin to APP_BINARIES)" ;; esac
  done
}

# Every workspace crate that depends on the crates on stdin, however indirectly, with those crates. Path dependencies of every kind (dev ones too: a test
# can use a crate). Fails if cargo or jq can't say, and the caller then runs everything.
affected_crates() {
  local edges
  command -v jq >/dev/null 2>&1 || return 1
  edges=$(cargo metadata --no-deps --format-version 1 --locked 2>/dev/null | jq -r '.packages[] | .name as $n | .dependencies[] | select(.path != null) | "\($n) \(.name)"') || return 1
  awk 'NR == FNR { seed[$1] = 1; next }
       { p[FNR] = $1; d[FNR] = $2; n = FNR }
       END { do { grew = 0; for (i = 1; i <= n; i++) if ((d[i] in seed) && !(p[i] in seed)) { seed[p[i]] = 1; grew = 1 } } while (grew); for (k in seed) print k }' \
    /dev/stdin <(printf '%s\n' "$edges") | sort -u
}

# What the tools' own tests (tools/check-test.sh, tools/ratchet-test.sh) are about: the tools, the hooks in tools/hooks, nextest's profiles and CI's workflow.
# A change to any of these since the base runs them in a full run, here and on CI; nothing else does.
TOOLS_PATHS='^(tools|\.config|\.github)/'

# The commit CI compares with, for what the tools' tests are about: on main, the commit the push came after, which CI checked last; on any other branch,
# where it left origin/main, so every push of a branch that changed the tools runs them. Prints it, or fails when it can't be worked out (a manual run, a
# first push, a `before` that isn't in HEAD's history, history not fetched, no jq), and the caller then runs what it would have skipped.
ci_base() {
  local base=""
  if [ "${GITHUB_REF_NAME:-}" = main ]; then
    [ "${GITHUB_EVENT_NAME:-}" = push ] && [ -f "${GITHUB_EVENT_PATH:-}" ] || return 1
    base=$(jq -r '.before // empty' "$GITHUB_EVENT_PATH" 2>/dev/null) || return 1
  else
    base=$(git merge-base origin/main HEAD 2>/dev/null) || return 1
  fi
  case "$base" in '' | *[!0-9a-f]* | 0000000000000000000000000000000000000000) return 1 ;; esac
  git cat-file -e "$base^{commit}" 2>/dev/null && git merge-base --is-ancestor "$base" HEAD 2>/dev/null || return 1
  printf '%s\n' "$base"
}

# Chooses what a run covers, from what changed since the branch left main (or $CHECK_BASE, or on CI `ci_base`) and what isn't committed. Arguments: the mode
# (--quick or --full). Sets: scope_tools (1 when the tools' tests run) and scope_tools_why (why, or why not), scope_all (1 when everything runs),
# scope_changed (crates changed), scope_affected (those and their dependents), scope_window, scope_lifecycle (1 when the tier runs), scope_app (1 when app/
# changed), scope_key ("all" or a short hash of the selection, for the memory of a pass) and scope_report (the lines the run prints at its start, saying what
# it selected and why, and what it skipped).
select_scope() {
  local mode=$1 base files file c all_crates affected="" reasons="" skipped=""
  scope_tools=0 scope_tools_why="" scope_all=0 scope_changed="" scope_affected="" scope_window=0 scope_lifecycle=0 scope_app=0 scope_key="" scope_report=""
  all_crates=$(for folder in crates/*/; do echo "${folder}x"; done | crates_in)
  base=${CHECK_BASE:-}
  if [ -z "$base" ] && [ -n "${CI:-}" ]; then
    base=$(ci_base) || { base="" scope_tools=1 scope_tools_why="CI's base couldn't be worked out, so they run"; }
  fi
  [ -n "$base" ] || base=$(git merge-base main HEAD 2>/dev/null || true)
  if [ "$mode" = --full ]; then
    if [ -n "${CI:-}" ]; then scope_all=1 reasons="CI runs everything"
    elif [ "${CHECK_ALL:-}" = 1 ]; then scope_all=1 reasons="CHECK_ALL=1"
    elif [ "$branch" = main ]; then scope_all=1 reasons="main runs everything, as it is what gets pushed"
    elif [ -z "$base" ]; then scope_all=1 reasons="no main to compare with"
    fi
  fi
  [ -n "$base" ] || base=HEAD
  files=$( { git diff --name-only "$base" 2>/dev/null; git ls-files --others --exclude-standard 2>/dev/null; } | sort -u | grep -vE '^docs/|\.md$' || true)
  local headings=0 other=""
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    if [ "$scope_tools" = 0 ] && [[ $file =~ $TOOLS_PATHS ]]; then
      scope_tools=1 scope_tools_why="$file changed since $(printf '%s' "$base" | cut -c1-12)"
    fi
    case "$file" in
      crates/recto-headings/*) headings=1 ;;
      crates/*/*)
        c=${file#crates/}
        c=${c%%/*}
        if printf '%s\n' "$all_crates" | grep -Fxq "$c"; then
          printf '%s\n' "$scope_changed" | grep -Fxq "$c" || scope_changed="$scope_changed${scope_changed:+ }$c"
        else
          other="$other $file"
        fi
        ;;
      app/*) scope_app=1 ;;
      *) other="$other $file" ;;
    esac
    if printf '%s\n' "$file" | grep -qE "$LIFECYCLE_PATHS"; then scope_lifecycle=1; fi
  done <<< "$files"
  [ "$scope_tools" = 1 ] || scope_tools_why="nothing under tools/, .config/ or .github/ changed since $(printf '%s' "$base" | cut -c1-12)"
  if [ -n "$other" ] && [ "$scope_all" = 0 ]; then
    scope_all=1
    reasons="changed outside crates/ and app/, so it can affect anything:$(printf '%s' "$other" | tr ' ' '\n' | head -3 | tr '\n' ' ')"
  fi
  if [ "$headings" = 1 ]; then scope_changed="$scope_changed${scope_changed:+ }recto-cli"; scope_window=1; fi
  if [ -n "$scope_changed" ]; then
    affected=$(printf '%s\n' $scope_changed | affected_crates) || { scope_all=1; reasons="the crates' dependencies couldn't be read (cargo metadata, jq)"; affected=""; }
  fi
  scope_affected=$(printf '%s\n' $affected | sort -u | tr '\n' ' ' | sed 's/ $//')
  if [ "$scope_all" = 1 ]; then
    scope_window=1 scope_lifecycle=1 scope_key=all
    scope_report="check: everything runs: $reasons."
    return 0
  fi
  for c in $WINDOW_CRATES recto-cli; do
    printf '%s\n' $scope_affected | grep -Fxq "$c" && scope_window=1
  done
  [ "$scope_app" = 0 ] || scope_window=1
  scope_key="sel:$(printf '%s|%s|%s|%s' "$scope_affected" "$scope_window" "$scope_lifecycle" "$scope_app" | shasum | cut -c1-12)"
  [ "$scope_window" = 1 ] || skipped="$skipped window tier (drawing, AppKit, the bridge);"
  [ "$scope_lifecycle" = 1 ] || skipped="$skipped lifecycle tier (updates, Launch Services, long sessions);"
  {
    echo "check: selective, by what changed since $(printf '%s' "$base" | cut -c1-12)${CHECK_BASE:+ (CHECK_BASE)}. CHECK_ALL=1 runs everything; CI and main do."
    echo "check:   crates changed: ${scope_changed:-none}$([ "$scope_app" = 1 ] && echo '; app/ changed')"
    echo "check:   tests of: ${scope_affected:-none}, and the registration guards"
    echo "check:   app tiers: $([ "$scope_window" = 1 ] && echo -n 'window ')$([ "$scope_lifecycle" = 1 ] && echo -n 'lifecycle')$([ "$scope_window$scope_lifecycle" = 00 ] && echo none)"
    [ -z "$skipped" ] || echo "check:   skipped:$skipped"
  } > "${TMPDIR:-/tmp}/check-scope.$$"
  scope_report=$(cat "${TMPDIR:-/tmp}/check-scope.$$"); rm -f "${TMPDIR:-/tmp}/check-scope.$$"
}

# The nextest filterset for the crates on stdin, as `package(=a) | package(=b)`.
packages_filter() {
  local out="" c
  while IFS= read -r c; do [ -z "$c" ] || out="$out${out:+ | }package(=$c)"; done
  printf '%s' "$out"
}
