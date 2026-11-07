#!/usr/bin/env bash
# heavy: the build queue. Runs one heavy command (cargo build/test, a bench, a VM) when its turn comes and the Mac has
# room, across every agent and shell. Quick jobs go first, and it refuses to rerun what just failed on the same code.
#
#   tools/sysguard/heavy.sh cargo test --release -p recto-core
#   HEAVY_SOLO=1 tools/sysguard/heavy.sh <command>        Runs with nothing else running: for timing and memory measurements.
#   HEAVY_PRIORITY=high tools/sysguard/heavy.sh <command> Goes ahead of everything, for a short job someone waits on.
#   HEAVY_CLASS=quick|normal tools/sysguard/heavy.sh ...  Overrides the guess at whether a job is quick.
#   HEAVY_RETRY=1 tools/sysguard/heavy.sh <command>       Runs it even though it just failed on this same code: only to
#                                                         investigate a failure that comes and goes, which still gets a fix.
#   tools/sysguard/heavy.sh --status                      Slots, queue, memory.
#   tools/sysguard/heavy.sh --report [hours]              Waits, run times, failures and refusals. Default 24 hours.
#   tools/sysguard/heavy.sh --slots N                     Sets how many jobs may run at once, 1 to 4. Default 2.
#   tools/sysguard/heavy.sh --last-failure                Path of the saved output of the last failure in this folder.
#
# Order: high, then quick (a compile check, one test binary, a filtered run), then the rest, each first come first served.
# A normal job that has waited 10 minutes goes ahead of quick ones, so it can't be starved.
#
# Solo: it holds nothing while it waits, and takes every slot at once when nothing is running. Normal jobs that came after
# it wait for it, so what's running drains. Quick jobs go past it for as long as a normal job is running, so nobody's small
# job waits on someone's long one because of a measurement. Once it has waited 10 minutes and only quick jobs are left,
# new quick jobs wait too, so it can't be starved.
#
# Room: each running job raises the memory a new one needs, MIN_FREE (default 40 %, from kern.memorystatus_level) plus
# 10 % per job already running, so a Mac short of memory falls back to fewer builds by itself. That's checked at the
# start only: sysguard.sh is what acts if memory falls later. Together the jobs get about 12 of 10 cores
# (CARGO_BUILD_JOBS 6 at two slots, 4 at three) unless the caller set it. Three agents' release builds at higher counts
# have rebooted this Mac, so run sysguard.sh whenever more than one slot is open.
#
# Older copies of this script, in other checkouts and repos, know only the first slot and don't queue. They still count,
# so the total never passes the limit. Jobs here take the first slot last, so those copies aren't starved.
set -uo pipefail
MIN_FREE="${MIN_FREE:-40}"
AGE_LIMIT="${HEAVY_AGE_LIMIT:-600}"
FAILURE_MEMORY=21600
# Machine-specific settings (untracked, optional): HEAVY_LOCK, SYSGUARD_LOG, SYSGUARD_EXTRA.
# A git worktree has no copy of the untracked file: it uses the main checkout's, so every worktree shares one queue.
LOCAL_ENV="$(dirname "$0")/local.env"
if [ ! -f "$LOCAL_ENV" ]; then
  common="$(git -C "$(dirname "$0")" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" &&
    LOCAL_ENV="$(dirname "$common")/tools/sysguard/local.env"
fi
# shellcheck source=/dev/null
[ -f "$LOCAL_ENV" ] && . "$LOCAL_ENV"
# Not $TMPDIR: a sandboxed session gets its own, and the queue must be one for every agent and shell of this user.
LOCK="${HEAVY_LOCK:-/tmp/seedwell-heavy-$(id -u).lock}"
QUEUE="$LOCK.queue"
STATS="$LOCK.stats"
SLOTS_FILE="$LOCK.slots"
FAILS="$LOCK.fails"
MAX_SLOTS=4

slot_limit() {
  local n
  n=$(cat "$SLOTS_FILE" 2>/dev/null)
  case "$n" in [1-4]) echo "$n" ;; *) echo 2 ;; esac
}
slot_path() { if [ "$1" = 1 ]; then echo "$LOCK"; else echo "$LOCK.$1"; fi; }
now() { date +%s; }
hash_of() { shasum | cut -d' ' -f1; }

# A slot or ticket left by a process that no longer exists is stale.
clear_stale() {
  local i path owner pid ticket
  for i in 1 2 3 4; do
    path=$(slot_path "$i")
    [ -d "$path" ] || continue
    owner=$(cat "$path/owner" 2>/dev/null)
    pid=${owner%% *}
    if [ -n "$pid" ] && ! kill -0 "$pid" 2>/dev/null; then rm -rf "$path"; fi
  done
  for ticket in "$QUEUE"/*; do
    [ -f "$ticket" ] || continue
    pid=${ticket##*-}
    kill -0 "$pid" 2>/dev/null || rm -f "$ticket"
  done
}
slots_in_use() {
  local i count=0
  for i in 1 2 3 4; do [ -d "$(slot_path "$i")" ] && count=$((count + 1)); done
  echo "$count"
}
swap_used() { sysctl -n vm.swapusage | sed -E 's/.*used = ([0-9.,]+)M.*/\1/' | tr ',' '.' | cut -d. -f1; }
folder_key() { pwd | hash_of; }

case "${1:-}" in
  --status)
    clear_stale
    printf 'Slots: %s in use of %s. Memory %s%% free, swap %s MiB used. Load%s\n' "$(slots_in_use)" "$(slot_limit)" \
      "$(sysctl -n kern.memorystatus_level)" "$(swap_used)" "$(uptime | sed -E 's/.*load averages?:/ /')"
    for i in 1 2 3 4; do
      path=$(slot_path "$i")
      [ -f "$path/owner" ] && printf '  running  %s\n' "$(cut -c1-110 "$path/owner")"
    done
    position=0
    for ticket in "$QUEUE"/*; do
      [ -f "$ticket" ] || continue
      position=$((position + 1))
      started=$(basename "$ticket" | cut -d- -f2)
      printf '  %3d. waited %5ss  %s\n' "$position" "$(($(now) - started))" "$(cut -c1-100 "$ticket")"
    done
    exit 0
    ;;
  --report)
    hours="${2:-24}"
    awk -F'\t' -v since="$(($(now) - hours * 3600))" '
      $2 == "refused" && $1 >= since { refused++ }
      $2 == "end" && $1 >= since {
        n++; wait_s[n] = $3; run_s[n] = $4; if ($5 != 0) { failed++; failed_s += $4 }
        if (min_free == "" || $6 < min_free) min_free = $6
        if ($7 > peak) peak = $7
        if ($8 > swap) swap = $8
        busy += $4; if (first == "" || $1 - $4 < first) first = $1 - $4; if ($1 > last) last = $1
      }
      function sorted(a, count,    i, j, t) { for (i = 1; i <= count; i++) s[i] = a[i]
        for (i = 2; i <= count; i++) { t = s[i]; for (j = i - 1; j > 0 && s[j] > t; j--) s[j + 1] = s[j]; s[j + 1] = t } }
      function at(count, p) { return s[int((count - 1) * p) + 1] }
      END {
        if (n == 0) { print "No finished jobs in that time."; exit }
        printf "%d jobs, %d failed using %d s of %d s, %d reruns refused. Most running at once: %d. Lowest memory free when one started: %s%%. Highest swap: %s MiB.\n", n, failed, failed_s, busy, refused, peak, min_free, swap
        sorted(wait_s, n); printf "Wait in queue, seconds: median %d, p90 %d, longest %d.\n", at(n, .5), at(n, .9), s[n]
        sorted(run_s, n); printf "Run time, seconds: median %d, p90 %d, longest %d.\n", at(n, .5), at(n, .9), s[n]
        if (last > first) printf "Average jobs running: %.2f.\n", busy / (last - first)
      }' "$STATS" 2>/dev/null
    exit 0
    ;;
  --slots)
    case "${2:-}" in
      [1-4]) echo "$2" > "$SLOTS_FILE"; echo "Slots set to $2: that many jobs may run at once." ;;
      *) echo "usage: heavy.sh --slots <1 to $MAX_SLOTS>" >&2; exit 2 ;;
    esac
    exit 0
    ;;
  --last-failure)
    last="$FAILS/last-$(folder_key)"
    if [ -f "$last" ] && [ -f "$(cat "$last")" ]; then cat "$last"; exit 0; fi
    echo "No failure saved for this folder." >&2
    exit 1
    ;;
esac
[ $# -gt 0 ] || { echo "usage: heavy.sh <command...>  (or --status, --report, --slots N, --last-failure)" >&2; exit 2; }

mkdir -p "$QUEUE" "$FAILS"
CMD="$*"
QUEUED_AT=$(now)
find "$FAILS" -type f -mmin +1440 -delete 2>/dev/null

# What the command is about to run on: the command and the code, including work not yet committed. Not in a git
# checkout, there's no telling, so nothing is remembered.
fingerprint() {
  git rev-parse --is-inside-work-tree >/dev/null 2>&1 || return 0
  {
    pwd
    printf '%s\n%s\n' "$CMD" "${tests_joined:-}"
    git rev-parse HEAD 2>/dev/null
    git diff HEAD 2>/dev/null
    git ls-files --others --exclude-standard -z 2>/dev/null | xargs -0 shasum 2>/dev/null
  } | hash_of
}
FP=$(fingerprint)
if [ -n "$FP" ] && [ -f "$FAILS/$FP" ] && [ "${HEAVY_RETRY:-}" != 1 ]; then
  read -r failed_code failed_at < "$FAILS/$FP"
  if [ "$(($(now) - failed_at))" -lt "$FAILURE_MEMORY" ]; then
    {
      echo "heavy: refused. This exact command failed $((($(now) - failed_at) / 60)) minutes ago on this same code (exit $failed_code), and nothing has changed since."
      echo "heavy: saved output: $FAILS/$FP.log. The end of it:"
      tail -30 "$FAILS/$FP.log" 2>/dev/null
      echo "heavy: fix the code and run it again. HEAVY_RETRY=1 runs it anyway, to investigate a failure that comes and goes."
    } >&2
    printf '%s\trefused\t%s\n' "$(now)" "$(printf '%s' "$CMD" | tr '\t\n' '  ' | cut -c1-100)" >> "$STATS" 2>/dev/null
    exit "$failed_code"
  fi
fi

job_class() {
  case "${HEAVY_CLASS:-}" in quick) echo 1; return ;; normal) echo 2; return ;; esac
  case "$CMD" in
    *--release*|*--workspace*|*rebuild*|*regression-gate*) echo 2 ;;
    *"cargo check"*|*"cargo clippy"*|*"cargo fmt"*) echo 1 ;;
    *nextest*" -E "*|*"cargo test"*"--test "*|*"cargo test"*"--lib"*|*nextest*"--test-filter"*) echo 1 ;;
    *) echo 2 ;;
  esac
}
PRIORITY=$(job_class)
[ "${HEAVY_PRIORITY:-}" = high ] && PRIORITY=0
# Sorted by name, so by priority, then arrival. A process's id ends the name, which is how a dead one is noticed.
SOLO_MARK=""
[ "${HEAVY_SOLO:-}" = 1 ] && SOLO_MARK="solo-"
TICKET="$QUEUE/$PRIORITY-$QUEUED_AT-$SOLO_MARK$$"
printf '%s  [%s]\n' "$CMD" "$(pwd)" > "$TICKET"
HELD=()
OUTPUT="$FAILS/run-$$"
TEES=""
release() {
  if [ -n "$TEES" ]; then kill $TEES 2>/dev/null; TEES=""; fi
  if [ ${#HELD[@]} -gt 0 ]; then rm -rf "${HELD[@]}"; fi
  HELD=()
  rm -f "$TICKET" "$OUTPUT" "$OUTPUT.out" "$OUTPUT.err"
}
trap release EXIT
trap 'release; exit 130' INT
trap 'release; exit 143' TERM

take_slot() {
  local i path
  # The first slot last: other copies of this script can only use that one.
  for i in 2 3 4 1; do
    [ "$i" -le "$1" ] || continue
    path=$(slot_path "$i")
    if mkdir "$path" 2>/dev/null; then
      echo "$$ $(date +%T) $CMD" > "$path/owner"
      echo "$PRIORITY" > "$path/class"
      HELD+=("$path")
      return 0
    fi
  done
  return 1
}
# Whether a job that isn't quick is running. One with no class is an older copy of this script's, which only big jobs use.
normal_job_running() {
  local i path
  for i in 1 2 3 4; do
    path=$(slot_path "$i")
    [ -d "$path" ] || continue
    case "$(cat "$path/class" 2>/dev/null)" in 0|1) ;; *) return 0 ;; esac
  done
  return 1
}
# One order that every waiting job agrees on, so two can never each wait for the other: high, normal jobs that have waited
# too long, quick, the rest, each by arrival. A solo job waiting on a normal job doesn't count as having waited too long.
queue_key() {
  local name=$1 class time rank
  class=${name%%-*}
  time=$(echo "$name" | cut -d- -f2)
  case "$class" in
    0) rank=0 ;;
    1) rank=2 ;;
    *)
      rank=3
      if [ $((TIME_NOW - time)) -gt "$AGE_LIMIT" ]; then
        case "$name" in
          *-solo-*) [ "$NORMAL_RUNNING" = 1 ] || rank=1 ;;
          *) rank=1 ;;
        esac
      fi
      ;;
  esac
  echo "$rank-$time-$name"
}
# How many are ahead of this job, and whether a solo job is what it must wait for.
ahead_of_me() {
  local count=0 behind_solo=0 ticket name mine my_key their_key their_time my_time
  TIME_NOW=$(now)
  NORMAL_RUNNING=0
  normal_job_running && NORMAL_RUNNING=1
  mine=$(basename "$TICKET")
  my_key=$(queue_key "$mine")
  my_time=$(echo "$mine" | cut -d- -f2)
  for ticket in "$QUEUE"/*; do
    [ -f "$ticket" ] || continue
    name=$(basename "$ticket")
    [ "$name" = "$mine" ] && continue
    their_key=$(queue_key "$name")
    their_time=$(echo "$name" | cut -d- -f2)
    if [[ "$their_key" < "$my_key" ]]; then
      count=$((count + 1))
      case "$name" in *-solo-*) behind_solo=1 ;; esac
    fi
    # Later normal jobs wait for it however long they've waited, or they'd go on being what it's waiting for.
    case "$name" in *-solo-*) [ "$PRIORITY" = 2 ] && [ "$their_time" -lt "$my_time" ] && behind_solo=1 ;; esac
  done
  echo "$count $behind_solo"
}

said=""
while true; do
  clear_stale
  limit=$(slot_limit)
  in_use=$(slots_in_use)
  free=$(sysctl -n kern.memorystatus_level)
  read -r ahead behind_solo <<< "$(ahead_of_me)"
  open=$((limit - in_use))
  need=$((MIN_FREE + 10 * in_use))
  why=""
  if [ "${HEAVY_SOLO:-}" = 1 ]; then
    # It holds nothing until it can hold everything: slots kept while waiting are what others wait for.
    if [ "$ahead" -gt 0 ]; then
      why="for its turn, $ahead ahead"
    elif [ "$in_use" -gt 0 ]; then
      why="for what's running to end"
    else
      while take_slot "$MAX_SLOTS"; do :; done
      if [ ${#HELD[@]} -ge "$MAX_SLOTS" ]; then break; fi
      # Someone took one at that same moment.
      rm -rf "${HELD[@]}"
      HELD=()
      why="for every slot to run alone"
    fi
  elif [ "$behind_solo" = 1 ]; then
    why="behind a job that runs alone"
  elif [ "$ahead" -ge "$open" ]; then
    why="in the queue, $ahead ahead, $in_use of $limit running"
  elif [ "$free" -le "$need" ]; then
    why="for memory: ${free}% free, $in_use running, needs more than ${need}%"
  elif take_slot "$limit"; then
    break
  else
    why="for a slot"
  fi
  [ "$why" != "$said" ] && echo "heavy: waiting $why" >&2 && said="$why"
  sleep 2
done
rm -f "$TICKET"
RUN_STARTED=$(now)
START_FREE=$free
RUNNING=$(slots_in_use)
if [ -z "${CARGO_BUILD_JOBS:-}" ]; then
  jobs_each=$((12 / $(slot_limit)))
  [ "$jobs_each" -lt 3 ] && jobs_each=3
  export CARGO_BUILD_JOBS="$jobs_each"
fi

# The output goes where it always went, and into a file to keep if this fails. Through named pipes rather than a shell
# pipeline, so a daemon the command leaves running, holding its output open, can't keep this job's slot for ever.
mkfifo "$OUTPUT.out" "$OUTPUT.err"
tee -a "$OUTPUT" < "$OUTPUT.out" &
tee_out=$!
tee -a "$OUTPUT" < "$OUTPUT.err" >&2 &
tee_err=$!
TEES="$tee_out $tee_err"
"$@" > "$OUTPUT.out" 2> "$OUTPUT.err"
status=$?
waited=0
while [ "$waited" -lt 20 ] && { kill -0 "$tee_out" 2>/dev/null || kill -0 "$tee_err" 2>/dev/null; }; do
  sleep 0.1
  waited=$((waited + 1))
done
kill $TEES 2>/dev/null
TEES=""

if [ -n "$FP" ]; then
  case "$status" in
    0) rm -f "$FAILS/$FP" "$FAILS/$FP.log" ;;
    # Stopped from outside says nothing about the code, so it isn't held against it.
    130|137|143) ;;
    *)
      echo "$status $(now)" > "$FAILS/$FP"
      tail -400 "$OUTPUT" > "$FAILS/$FP.log" 2>/dev/null
      echo "$FAILS/$FP.log" > "$FAILS/last-$(folder_key)"
      ;;
  esac
fi
# A line per finished job: time, end, seconds waited, seconds run, exit, memory free at start, jobs running, swap MiB, command.
printf '%s\tend\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(now)" "$((RUN_STARTED - QUEUED_AT))" "$(($(now) - RUN_STARTED))" \
  "$status" "$START_FREE" "$RUNNING" "$(swap_used)" "$(printf '%s' "$CMD" | tr '\t\n' '  ' | cut -c1-100)" >> "$STATS" 2>/dev/null
exit "$status"
