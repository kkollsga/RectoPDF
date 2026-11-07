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

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
