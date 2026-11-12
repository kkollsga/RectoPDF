#!/usr/bin/env bash
# target-hygiene-test: proves tools/target-hygiene.sh on a throwaway folder. Seconds, nothing builds.
#
#   tools/target-hygiene-test.sh
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
X=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/hygiene-test.XXXXXX")" && pwd)
trap 'rm -rf "$X"' EXIT
fails=0
check() { if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi; }
has() { case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$3]"; fails=$((fails + 1)) ;; esac; }
H() { HYGIENE_MAX_ENTRIES=10 "$SOURCE/tools/target-hygiene.sh" "$X/target" 2>&1; }
fresh() { rm -rf "$X/target"; mkdir -p "$X/target/debug/deps" "$X/target/release/deps"; }
count() { find "$X/target/$1/deps" -maxdepth 1 -mindepth 1 -name "$2" | wc -l | tr -d ' '; }

fresh; for i in 1 2 3; do : > "$X/target/debug/deps/a$i.o"; done; : > "$X/target/debug/deps/bin-1"
out=$(H)
check "a small folder keeps its objects" "3" "$(count debug '*.o')"
has "and reports its size for the trend" "holds 4 entries" "$out"

fresh; for i in $(seq 1 30); do : > "$X/target/debug/deps/a$i.rcgu.o"; done
: > "$X/target/debug/deps/bin-1"; : > "$X/target/debug/deps/bin-1.d"; : > "$X/target/debug/deps/libx.rlib"; : > "$X/target/debug/deps/libx.rmeta"; mkdir "$X/target/debug/deps/dir.o"
out=$(H)
check "a big folder loses its object files" "0" "$(count debug '*.rcgu.o')"
check "the binary stays" "1" "$(count debug 'bin-1')"
check "so do libraries and metadata" "2" "$(count debug 'libx.*')"
check "and a folder named like an object" "1" "$(count debug 'dir.o')"
has "it says what it did, with the count" "held 35 entries" "$out"
has "and why" "first IOSurface" "$out"

fresh; for i in $(seq 1 30); do : > "$X/target/release/deps/a$i.o"; done; : > "$X/target/debug/deps/k.o"
out=$(H)
check "each profile's folder is judged on its own" "0 1" "$(count release '*.o') $(count debug '*.o')"

fresh; for i in $(seq 1 30); do : > "$X/target/debug/deps/a$i.o"; done
out=$("$SOURCE/tools/target-hygiene.sh" "$X/missing" 2>&1); rc=$?
check "a missing target folder is fine" "0 " "$rc $out"
HYGIENE_MAX_ENTRIES=100 "$SOURCE/tools/target-hygiene.sh" "$X/target" >/dev/null
check "the limit is the setting's" "30" "$(count debug '*.o')"
fresh; for i in $(seq 1 30); do : > "$X/target/debug/deps/a$i.o"; done
# A build in this target folder: a process whose command line names it.
bash -c "sleep 30; :" --out-dir "$X/target/debug/deps/" & busy=$!; sleep 0.3 # ratchet: poll
out=$(H)
{ kill "$busy"; wait "$busy"; } 2>/dev/null
check "nothing is swept while a build runs in the folder" "30" "$(count debug '*.o')"
has "and it says so" "a build is running" "$out"

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
