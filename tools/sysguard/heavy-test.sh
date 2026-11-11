#!/usr/bin/env bash
# heavy-test: proves the build queue's rules on a throwaway lock, with no effect on real jobs. About a minute.
#
#   tools/sysguard/heavy-test.sh [path to heavy.sh]
set -uo pipefail
HEAVY="$(cd "$(dirname "${1:-$0}")" && pwd)/$(basename "${1:-heavy.sh}")"
X=$(mktemp -d "${TMPDIR:-/tmp}/heavy-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
export HEAVY_LOCK="$X/lock"
# local.env sends other copies to the main checkout's script, which would test the wrong file.
export HEAVY_TESTING=1
unset CARGO_BUILD_JOBS HEAVY_SOLO HEAVY_PRIORITY HEAVY_CLASS HEAVY_RETRY tests_joined
mkdir "$X/repo"
cd "$X/repo" || exit 1
git init -q . && git -c user.name=t -c user.email=t@t commit -q --allow-empty -m start
fails=0
check() { # name, expected, actual
  if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi
}
pause() { perl -e "select(undef,undef,undef,$1)"; }
job() { # name, seconds, [exit code]
  bash "$HEAVY" bash -c "echo $1 >> '$X/log'; sleep $2; exit ${3:-0}" 2>>"$X/err"
}
order() { tr '\n' ' ' < "$X/log" | sed 's/ $//'; : > "$X/log"; }

# Failure: jobs overtaking one another, so a full check started long ago never gets its turn.
bash "$HEAVY" --slots 1 >/dev/null
for n in 1 2 3 4; do job "$n" 2 & pause 1.1; done
wait
check "first come, first served" "1 2 3 4" "$(order)"

# Failure: more jobs than slots at once, which is how this Mac has been rebooted.
bash "$HEAVY" --slots 2 >/dev/null
for n in a b c d; do
  bash "$HEAVY" bash -c "mkdir '$X/run.$n'; ls -d '$X'/run.* | wc -l | tr -d ' ' >> '$X/peak'; sleep 2; rmdir '$X/run.$n'" 2>>"$X/err" &
done
wait
check "never more than the slots" "2" "$(sort -n "$X/peak" | tail -1)"
rm -f "$X/peak"

# Failure: a cargo job spawning more compilers per slot as slots rise, past what the Mac can take.
bash "$HEAVY" --slots 3 >/dev/null
check "fewer compilers per job at three slots" "4" "$(bash "$HEAVY" bash -c 'echo $CARGO_BUILD_JOBS' 2>/dev/null)"
bash "$HEAVY" --slots 2 >/dev/null
check "more per job at two" "6" "$(bash "$HEAVY" bash -c 'echo $CARGO_BUILD_JOBS' 2>/dev/null)"

# Failure: a one-line fix waiting behind a long build.
bash "$HEAVY" --slots 1 >/dev/null
job first 3 & pause 0.8
job long 1 & pause 0.8
( HEAVY_CLASS=quick job quick 1 ) & pause 0.8
( HEAVY_PRIORITY=high job high 1 ) &
wait
check "high, then quick, then the rest" "first high quick long" "$(order)"

# Failure: a job refused for good by a stream of quick ones.
mkdir -p "$X/lock.queue"
aged="$X/lock.queue/2-$(($(date +%s) - 700))-$$"
echo "an old job" > "$aged"
bash "$HEAVY" --slots 1 >/dev/null
( HEAVY_CLASS=quick job young 1 ) & quick_pid=$!
pause 4
check "a normal job that waited too long goes before quick ones" "" "$(order)"
rm -f "$aged"
wait "$quick_pid"
check "and the quick job then runs" "young" "$(order)"

# Failure: a measurement taken while other builds run.
bash "$HEAVY" --slots 2 >/dev/null
job n1 3 & pause 0.8
( HEAVY_SOLO=1 job solo 1 ) & pause 0.8
job n2 1 &
wait
check "solo waits for all, and runs before later jobs" "n1 solo n2" "$(order)"

# Waits for this many lines in the log, then ends whatever's still there, so a job stuck for ever fails its check and doesn't hang this.
settle() {
  for _ in $(seq 1 "${2:-40}"); do [ "$(wc -l < "$X/log" | tr -d ' ')" -ge "$1" ] && break; pause 0.5; done
  pkill -f "$X/log" 2>/dev/null
  wait
}

# Failure: a quick job comes while solo is collecting slots. It sorts ahead of solo, which waits for its turn holding the slots that job needs, and neither ever moves: everything on the Mac waited over an hour behind this. And short of that, a measurement waiting for someone's long build holds up every small job for as long as the build lasts: 25 minutes, one time.
bash "$HEAVY" --slots 2 >/dev/null
bash "$HEAVY" bash -c "echo long >> '$X/log'; sleep 4; echo long-done >> '$X/log'" 2>>"$X/err" & pause 0.8
( HEAVY_SOLO=1 job solo 1 ) & pause 0.8
( HEAVY_CLASS=quick job quick 0 ) &
settle 4
check "quick jobs go past solo while it waits for a long job, and it holds nothing" "long quick long-done solo" "$(order)"

# The same once solo has waited too long: it's the long job that's holding it up, and quick jobs can't make that any better.
bash "$HEAVY" bash -c "echo long >> '$X/log'; sleep 4; echo long-done >> '$X/log'" 2>>"$X/err" & pause 0.8
( HEAVY_AGE_LIMIT=0 HEAVY_SOLO=1 job solo 1 ) & pause 1.5
( HEAVY_AGE_LIMIT=0 HEAVY_CLASS=quick job quick 0 ) &
settle 4
check "that holds however long solo has waited" "long quick long-done solo" "$(order)"

# Failure: a stream of quick jobs keeps solo out for good.
bash "$HEAVY" --slots 2 >/dev/null
( HEAVY_AGE_LIMIT=0 HEAVY_CLASS=quick job q1 3 ) & pause 0.8
( HEAVY_AGE_LIMIT=0 HEAVY_SOLO=1 job solo 1 ) & pause 1.5
( HEAVY_AGE_LIMIT=0 HEAVY_CLASS=quick job q2 0 ) &
settle 3
check "solo that has waited too long goes before new quick jobs" "q1 solo q2" "$(order)"

# Failure: a normal job that has waited too long and a quick job each count the other as ahead, and with one slot neither ever goes.
bash "$HEAVY" --slots 1 >/dev/null
job first 3 & pause 0.8
( HEAVY_AGE_LIMIT=0 job old 0 ) & pause 1.2
( HEAVY_AGE_LIMIT=0 HEAVY_CLASS=quick job young 0 ) &
settle 3
check "a job that waited too long and a quick one can't wait for each other" "first old young" "$(order)"

# Failure: the command's exit code lost, so a failed build reads as a pass.
bash "$HEAVY" bash -c 'exit 7' 2>/dev/null
check "exit code comes through" "7" "$?"

# Failure: the same failing job run again and again, on code nobody changed.
: > "$X/ran"
bash "$HEAVY" bash -c "echo run >> '$X/ran'; echo the-reason-it-failed; exit 3" >/dev/null 2>&1
first=$?
out=$(bash "$HEAVY" bash -c "echo run >> '$X/ran'; echo the-reason-it-failed; exit 3" 2>&1 >/dev/null)
second=$?
check "first failure runs" "3" "$first"
check "rerun on the same code is refused with the same exit" "3" "$second"
check "and doesn't run" "1" "$(wc -l < "$X/ran" | tr -d ' ')"
case "$out" in *the-reason-it-failed*) r=shown ;; *) r=missing ;; esac
check "the refusal shows why it failed" "shown" "$r"
check "the saved failure is found by folder" "1" "$(bash "$HEAVY" --last-failure | xargs grep -c the-reason-it-failed)"

# Failure: a fixed bug refused because the command's text is the same.
echo changed > work.txt
bash "$HEAVY" bash -c "echo run >> '$X/ran'; echo the-reason-it-failed; exit 3" >/dev/null 2>&1
check "new work in the folder may run again" "2" "$(wc -l < "$X/ran" | tr -d ' ')"
HEAVY_RETRY=1 bash "$HEAVY" bash -c "echo run >> '$X/ran'; echo the-reason-it-failed; exit 3" >/dev/null 2>&1
check "HEAVY_RETRY runs it anyway" "3" "$(wc -l < "$X/ran" | tr -d ' ')"
# A pass forgets the failure.
: > "$X/flag"
bash "$HEAVY" bash -c "[ -s '$X/flag' ] || { echo x >> '$X/flag'; exit 5; }" >/dev/null 2>&1
HEAVY_RETRY=1 bash "$HEAVY" bash -c "[ -s '$X/flag' ] || { echo x >> '$X/flag'; exit 5; }" >/dev/null 2>&1
bash "$HEAVY" bash -c "[ -s '$X/flag' ] || { echo x >> '$X/flag'; exit 5; }" >/dev/null 2>&1
check "a pass forgets the failure" "0" "$?"

# Failure (PLAN phase 7): a library installed where git doesn't look, such as PDFium in target/, and the job that failed
# without it refused as "nothing has changed". What `.heavy-inputs` lists counts as code; a path out of the checkout doesn't.
printf 'lib/\n' > .gitignore
printf '# untracked inputs\nlib\n../outside\n' > .heavy-inputs
git add .gitignore .heavy-inputs && git -c user.name=t -c user.email=t@t commit -q -m inputs
: > "$X/ran3"
needs_lib() { bash "$HEAVY" bash -c "echo run >> '$X/ran3'; [ -f lib/lib.dylib ]" >/dev/null 2>&1; }
needs_lib
needs_lib
check "without the library, the rerun is refused" "1" "$(wc -l < "$X/ran3" | tr -d ' ')"
mkdir -p lib ../outside && echo v1 > lib/lib.dylib
needs_lib
check "installing a listed input lets it run again" "0 2" "$? $(wc -l < "$X/ran3" | tr -d ' ')"
rm lib/lib.dylib
needs_lib
check "taking it away again is the state that failed, so it's refused" "2" "$(wc -l < "$X/ran3" | tr -d ' ')"
echo changed > ../outside/x
needs_lib
check "and a path out of the checkout doesn't count" "2" "$(wc -l < "$X/ran3" | tr -d ' ')"
echo v2 > lib/lib.dylib
needs_lib
check "a changed input counts" "0 3" "$? $(wc -l < "$X/ran3" | tr -d ' ')"
rm -rf lib ../outside

# Failure (2026-11-11): the same test command run with another setting that changes what it does, a slow mode in its environment, refused as "this exact
# command failed": the failure was another setting's. What `.heavy-inputs` names with `env`, by name or by a prefix and `*`, counts; nothing else in the
# environment does, so a shell's own changes don't undo the refusal.
printf 'env SLOWISH_*\nenv EXACTLY\n' >> .heavy-inputs
git add .heavy-inputs && git -c user.name=t -c user.email=t@t commit -q -m env-inputs
: > "$X/ran4"
fails_with_env() { env "$@" bash "$HEAVY" bash -c "echo run >> '$X/ran4'; exit 4" >/dev/null 2>&1; }
fails_with_env SLOWISH_A=1
fails_with_env SLOWISH_A=1
check "the same setting again is refused" "1" "$(wc -l < "$X/ran4" | tr -d ' ')"
fails_with_env SLOWISH_A=2
check "another value of a listed setting runs" "2" "$(wc -l < "$X/ran4" | tr -d ' ')"
fails_with_env SLOWISH_A=2 EXACTLY=x
check "and so does a setting listed by its name" "3" "$(wc -l < "$X/ran4" | tr -d ' ')"
fails_with_env SLOWISH_A=2 EXACTLY=x UNLISTED=y
check "one that isn't listed doesn't count" "3" "$(wc -l < "$X/ran4" | tr -d ' ')"

# Failure: a job stopped from outside held against the code.
bash "$HEAVY" bash -c "echo run >> '$X/ran2'; kill -TERM \$\$" >/dev/null 2>&1
bash "$HEAVY" bash -c "echo run >> '$X/ran2'; kill -TERM \$\$" >/dev/null 2>&1
check "a job killed from outside isn't remembered" "2" "$(wc -l < "$X/ran2" | tr -d ' ')"

# Failure: a daemon left holding the output open keeps the slot for ever.
started=$(date +%s)
bash "$HEAVY" bash -c "sleep 30 & echo \$! > '$X/daemon'; exit 0" >/dev/null 2>&1
took=$(($(date +%s) - started))
check "a daemon holding the output doesn't hold the slot" "yes" "$([ "$took" -lt 10 ] && echo yes || echo "no, $took s")"
kill "$(cat "$X/daemon")" 2>/dev/null

# Failure: separate streams merged, so a caller parsing stdout reads error messages.
check "stdout stays stdout" "out" "$(bash "$HEAVY" bash -c 'echo out; echo err >&2' 2>/dev/null)"
check "stderr stays stderr" "err" "$(bash "$HEAVY" bash -c 'echo out; echo err >&2' 2>&1 >/dev/null)"

# Failure: dead jobs holding the queue, and memory rules ignored.
touch "$X/lock.queue/1-1-999999"
mkdir "$X/lock"
echo "999999 x dead" > "$X/lock/owner"
bash "$HEAVY" --slots 1 >/dev/null
job after-dead 0
check "dead slots and tickets are cleared" "after-dead" "$(order)"
bash "$HEAVY" --slots 2 >/dev/null
MIN_FREE=100 bash "$HEAVY" bash -c "echo no-room >> '$X/log'" 2>/dev/null & p=$!
pause 3.5
kill "$p" 2>/dev/null
wait "$p" 2>/dev/null
check "no memory, no start" "" "$(order)"
check "a job killed while waiting leaves no ticket" "0" "$(ls "$X/lock.queue" | wc -l | tr -d ' ')"

# The watchdog. Failure: the queue stops, nobody notices, and everything waits for over an hour.
watch() { rm -f "$X/watch.rc"; ( HEAVY_WATCH_EVERY=1 "$@" bash "$HEAVY" --watch > "$X/watch.out" 2>&1; echo $? > "$X/watch.rc" ) & W=$!; }
ended_within() { # seconds
  local n=0
  while [ "$n" -lt "$(( $1 * 5 ))" ]; do [ -f "$X/watch.rc" ] && { echo "ended $(cat "$X/watch.rc")"; return; }; pause 0.2; n=$((n + 1)); done
  pkill -P "$W" 2>/dev/null; echo "still watching"
}
clean_queue() { rm -rf "$X/lock" "$X/lock.2" "$X/lock.3" "$X/lock.4"; rm -f "$X"/lock.queue/*; }
bash "$HEAVY" --slots 1 >/dev/null
clean_queue
sleep 60 & idle=$!
mkdir "$X/lock"; echo "$idle 00:00:00 idle holder" > "$X/lock/owner"
echo "a waiting job" > "$X/lock.queue/2-$(date +%s)-$$"
watch env HEAVY_STALL_SECS=2
check "a holder doing nothing with jobs waiting wakes the watchdog" "ended 1" "$(ended_within 15)"
case "$(cat "$X/watch.out")" in *"NEEDS ATTENTION"*"idle holder"*) r=says ;; *) r=silent ;; esac
check "and it says what's holding the queue" "says" "$r"
{ kill "$idle"; wait "$idle"; } 2>/dev/null
clean_queue
yes > /dev/null & busy=$!
mkdir "$X/lock"; echo "$busy 00:00:00 busy holder" > "$X/lock/owner"
echo "a waiting job" > "$X/lock.queue/2-$(date +%s)-$$"
pause 1
watch env HEAVY_STALL_SECS=2
check "a long job really working isn't called stuck" "still watching" "$(ended_within 6)"
{ kill "$busy"; wait "$busy"; } 2>/dev/null
clean_queue
bash "$HEAVY" --slots 2 >/dev/null
echo "a waiting job" > "$X/lock.queue/2-$(date +%s)-$$"
watch env HEAVY_FREE_SLOT_SECS=2
check "a free slot nobody takes wakes it" "ended 1" "$(ended_within 15)"
clean_queue
watch env HEAVY_STALL_SECS=1 HEAVY_FREE_SLOT_SECS=1
check "an empty queue doesn't" "still watching" "$(ended_within 4)"
yes > /dev/null & busy=$!
mkdir "$X/lock"; echo "$busy 00:00:00 busy holder" > "$X/lock/owner"
echo "an old job" > "$X/lock.queue/2-$(($(date +%s) - 100))-$$"
watch env HEAVY_LONG_WAIT_SECS=50 HEAVY_FREE_SLOT_SECS=999
check "a job waiting far too long wakes it, even with work running" "ended 1" "$(ended_within 15)"
{ kill "$busy"; wait "$busy"; } 2>/dev/null
clean_queue
bash "$HEAVY" --slots 2 >/dev/null
case "$(bash "$HEAVY" --status)" in *"Watchdog: NOT RUNNING"*) r=says ;; *) r=silent ;; esac
check "status says when no watchdog is running" "says" "$r"

# The stats line. Failure: scripts under one long path all read the same, and nobody can tell which project or branch ran what.
long="/private/tmp/$(printf 'a-long-scratch-path/%.0s' 1 2 3 4 5 6 7 8)"
git checkout -q -b build/stats
bash "$HEAVY" bash -c 'exit 0' "${long}first.sh" 2>/dev/null
bash "$HEAVY" bash -c 'exit 0' "${long}second.sh" 2>/dev/null
stat_field() { tail -"${2:-1}" "$X/lock.stats" | head -1 | cut -f"$1"; }
case "$(stat_field 9 2)" in *first.sh) r=kept ;; *) r=lost ;; esac
check "the end of a long command stays" "kept" "$r"
case "$(stat_field 9)" in *second.sh) r=kept ;; *) r=lost ;; esac
check "so two scripts under one path differ" "kept" "$r"
case "$(stat_field 9)" in "bash -c exit 0 "*) r=kept ;; *) r=lost ;; esac
check "and its start stays" "kept" "$r"
check "and it stays 100 characters" "100" "$(stat_field 9 | tr -d '\n' | wc -c | tr -d ' ')"
bash "$HEAVY" bash -c 'exit 0' 2>/dev/null
check "a short command is kept whole" "bash -c exit 0" "$(stat_field 9)"
check "the old columns stay where they were" "end 0" "$(stat_field 2) $(stat_field 5)"
check "the repo is recorded" "repo" "$(stat_field 10)"
check "and the branch" "build/stats" "$(stat_field 11)"
git worktree add -q -b build/tree "$X/tree"
( cd "$X/tree" && bash "$HEAVY" bash -c 'exit 0' 2>/dev/null )
check "a worktree counts as its main checkout's repo" "repo" "$(stat_field 10)"
check "on its own branch" "build/tree" "$(stat_field 11)"
mkdir "$X/plain"
( cd "$X/plain" && bash "$HEAVY" bash -c 'exit 0' 2>/dev/null )
check "outside a repo, the columns say so and still line up" "- -" "$(stat_field 10) $(stat_field 11)"
bash "$HEAVY" bash -c 'exit 9' >/dev/null 2>&1
bash "$HEAVY" bash -c 'exit 9' >/dev/null 2>&1
check "a refusal is recorded with the repo and branch" "refused repo build/stats" "$(stat_field 2) $(stat_field 4) $(stat_field 5)"
git checkout -q -

# --report. Failure: a change to the line stops older lines parsing, or a small run called a whole suite, or whole suites can't be told apart by branch.
now_s=$(date +%s)
end_line() { printf '%s\tend\t0\t%s\t%s\t50\t1\t0\t%s' "$now_s" "$1" "$2" "$3"; [ -z "${4:-}" ] || printf '\t%s\t%s' "$4" "$5"; printf '\n'; }
{
  end_line 100 0 'bash -c tests=($tests_joined); run'
  end_line 90 100 'bash -c tests=($tests_joined); run'
  end_line 60 0 'bash -c tests=($tests_joined); run check.sh--full' proj build/a
  end_line 40 100 'bash -c tests=($tests_joined); run check.sh--full' proj build/a
  end_line 30 0 'cargo nextest run --locked --workspace' proj build/b
  end_line 20 0 'scripts/regression-gate.sh check --full --run /x' other main
  end_line 5 0 'bash -c tests=($tests_joined); run check.sh--quick' proj build/a
  end_line 5 0 'cargo clippy --workspace --all-targets' proj build/a
  end_line 5 0 'cargo nextest run --workspace -E test(=x)' proj build/a
  end_line 5 0 'cargo test -p x --test y --workspace' proj build/a
  end_line 5 0 'scripts/regression-gate.sh check --fast' other main
  end_line 5 0 '/x/regression-gate/venv/bin/maturin build' other main
  printf '%s\trefused\tan old refusal\n' "$now_s"
  printf '%s\trefused\ta new refusal\tproj\tbuild/a\n' "$now_s"
} > "$X/report.stats"
report=$(HEAVY_LOCK="$X/report" bash "$HEAVY" --report)
has_line() { case "$report" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$report]"; fails=$((fails + 1)) ;; esac; }
has_line "older and newer lines both count" "12 jobs, 2 failed"
has_line "older and newer refusals both count" "2 reruns refused"
has_line "whole suites are grouped by repo and branch" "  2 runs,   1 failed,    100 s  proj build/a"
has_line "a whole workspace's tests count" "  1 runs,   0 failed,     30 s  proj build/b"
has_line "a project's gate counts" "  1 runs,   0 failed,     20 s  other main"
has_line "older lines have no branch, and say so" "  2 runs,   1 failed,    190 s  (branch not recorded)"
check "and nothing small is in that section" "4" "$(printf '%s\n' "$report" | sed -n '/^Whole suites/,$p' | grep -c ' runs, ')"
printf '%s\tend\t0\t5\t0\t50\t1\t0\tcargo check\n' "$now_s" > "$X/none.stats"
case "$(HEAVY_LOCK="$X/none" bash "$HEAVY" --report)" in *"Whole suites by branch:"*"none."*) r=says ;; *) r=silent ;; esac
check "no whole suites, and it says so" "says" "$r"

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
