#!/usr/bin/env bash
# heavy: the build queue. Runs one heavy command (cargo build/test, a bench, a VM) when its turn comes and the Mac has
# room, first come first served, across every agent and shell.
#
#   tools/sysguard/heavy.sh cargo test --release -p recto-core
#   HEAVY_SOLO=1 tools/sysguard/heavy.sh <command>       Holds every slot: for timing and memory measurements.
#   HEAVY_PRIORITY=high tools/sysguard/heavy.sh <command> Goes ahead of normal jobs, for a short job someone waits on.
#   tools/sysguard/heavy.sh --status                     Slots, queue, memory.
#   tools/sysguard/heavy.sh --report [hours]             Waits, run times and memory from the log. Default 24 hours.
#   tools/sysguard/heavy.sh --slots N                    Sets how many jobs may run at once, 1 to 4. Default 2.
#
# Room: each running job raises the memory a new one needs, MIN_FREE (default 40 %, from kern.memorystatus_level) plus
# 10 % per job already running, so a Mac short of memory falls back to fewer builds by itself. Each runs with
# CARGO_BUILD_JOBS=6 of 10 cores unless the caller set it.
#
# Older copies of this script, in other checkouts and repos, know only the first slot and don't queue. They still count,
# so the total never passes the limit. Jobs here take the first slot last, so those copies aren't starved.
set -uo pipefail
MIN_FREE="${MIN_FREE:-40}"
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
MAX_SLOTS=4
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-6}"

slot_limit() {
  local n
  n=$(cat "$SLOTS_FILE" 2>/dev/null)
  case "$n" in [1-4]) echo "$n" ;; *) echo 2 ;; esac
}
# Every slot, the first one last.
slot_path() { if [ "$1" = 1 ]; then echo "$LOCK"; else echo "$LOCK.$1"; fi; }
now() { date +%s; }

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
      $2 == "end" && $1 >= since {
        n++; wait_s[n] = $3; run_s[n] = $4; if ($5 != 0) failed++
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
        printf "%d jobs, %d failed. Most running at once: %d. Lowest memory free when one started: %s%%. Highest swap: %s MiB.\n", n, failed, peak, min_free, swap
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
esac
[ $# -gt 0 ] || { echo "usage: heavy.sh <command...>  (or --status, --report, --slots N)" >&2; exit 2; }

mkdir -p "$QUEUE"
CMD="$*"
QUEUED_AT=$(now)
PRIORITY=1
[ "${HEAVY_PRIORITY:-}" = high ] && PRIORITY=0
# Sorted by name, so by priority, then arrival. A process's id ends the name, which is how a dead one is noticed.
TICKET="$QUEUE/$PRIORITY-$QUEUED_AT-$$"
printf '%s  [%s]\n' "$CMD" "$(pwd)" > "$TICKET"
HELD=()
RUN_STARTED=""
release() {
  if [ ${#HELD[@]} -gt 0 ]; then rm -rf "${HELD[@]}"; fi
  HELD=()
  rm -f "$TICKET"
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
      HELD+=("$path")
      return 0
    fi
  done
  return 1
}
ahead_of_me() {
  local count=0 ticket
  for ticket in "$QUEUE"/*; do
    [ -f "$ticket" ] || continue
    [ "$ticket" = "$TICKET" ] && { echo "$count"; return; }
    count=$((count + 1))
  done
  echo "$count"
}

said=""
while true; do
  clear_stale
  limit=$(slot_limit)
  in_use=$(slots_in_use)
  free=$(sysctl -n kern.memorystatus_level)
  ahead=$(ahead_of_me)
  open=$((limit - in_use))
  need=$((MIN_FREE + 10 * in_use))
  why=""
  if [ "${HEAVY_SOLO:-}" = 1 ]; then
    # At the front it keeps each slot as it frees, so it can't be starved, and those behind it wait.
    if [ "$ahead" -gt 0 ]; then
      why="for its turn, $ahead ahead"
    else
      while take_slot "$MAX_SLOTS"; do :; done
      if [ ${#HELD[@]} -ge "$MAX_SLOTS" ]; then break; fi
      why="for every slot to run alone"
    fi
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
  sleep 3
done
rm -f "$TICKET"
RUN_STARTED=$(now)
START_FREE=$free
RUNNING=$(slots_in_use)
"$@"
status=$?
# A line per finished job: time, end, seconds waited, seconds run, exit, memory free at start, jobs running, swap MiB, command.
printf '%s\tend\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(now)" "$((RUN_STARTED - QUEUED_AT))" "$(($(now) - RUN_STARTED))" \
  "$status" "$START_FREE" "$RUNNING" "$(swap_used)" "$(printf '%s' "$CMD" | tr '\t\n' '  ' | cut -c1-100)" >> "$STATS" 2>/dev/null
exit "$status"
