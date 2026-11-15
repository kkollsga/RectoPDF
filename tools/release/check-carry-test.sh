#!/usr/bin/env bash
# check-carry-test: proves tools/release/check-carry.sh fails on a made fault, by name: a journal lost on the way, a tab not on its place,
# Find's text not carried (CHECK_CARRY_SEED, which only this sets), and passes on the real pair. Needs the real bundles, so it isn't part of
# release-test.sh: quiet runs, about a minute.
#
#   tools/release/check-carry-test.sh <RectoPDF-P.zip as published> <RectoPDF.app built for the new version>
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/../.." && pwd)"
[ $# = 2 ] && [ -f "$1" ] && [ -d "$2" ] || { sed -n '6p' "$0" >&2; exit 2; }
X=$(mktemp -d "${TMPDIR:-/tmp}/check-carry-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
fails=0
check() { if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi; }
has() { case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$3]"; fails=$((fails + 1)) ;; esac; }
run() { CHECK_CARRY_SEED="$1" "$SOURCE/tools/release/check-carry.sh" "$2" "$3" "$X/evidence-${1:-none}" 2>&1; }

# Failure: the check passes a pair whose session doesn't carry, as nothing it compares could differ.
out=$(run journal "$1" "$2"); rc=$?
check "a journal lost on the way fails the check" 1 "$rc"
has "naming the tab whose edits went" "tabs[0].edited: True then False" "$out"
out=$(run place "$1" "$2"); rc=$?
check "a tab off its place fails the check" 1 "$rc"
has "naming the place" "tabs[2].page: 4 then 2" "$out"
out=$(run find "$1" "$2"); rc=$?
check "Find's text not carried fails the check" 1 "$rc"
has "by the session's own answer" "FAIL  B: \`session show\`" "$out"
# Failure: the check fails the real pair, so its failures on a made fault mean nothing.
out=$(run "" "$1" "$2"); rc=$?
check "the real pair passes" 0 "$rc"
has "and says so" "did, and back again" "$out"
if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
