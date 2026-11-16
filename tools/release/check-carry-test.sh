#!/usr/bin/env bash
# check-carry-test: proves tools/release/check-carry.sh fails on a made fault, by name: a journal lost on the way, a tab not on its place,
# Find's text not carried, and for an older release before 0.1.1 the version-2 journal a later launch set aside gone, from 0.1.1 the highlight's
# journal lost before the older release runs again (CHECK_CARRY_SEED, which only this sets), and passes on the real pair. Run it with each
# previous release (docs/RELEASING.md); the real pair passes only once its tools/release/carry/<old>-to-<new>.txt says what differs by design.
# Needs the real bundles, so it isn't part of release-test.sh (tools/release/carry/expect_test.py, which is, holds the expectations with made
# states): quiet runs, about ten seconds each.
#
#   tools/release/check-carry-test.sh <RectoPDF-P.zip as published> <RectoPDF.app built for the new version>
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/../.." && pwd)"
[ $# = 2 ] && [ -f "$1" ] && [ -d "$2" ] || { sed -n '9p' "$0" >&2; exit 2; }
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
# What the older release can read decides what C, E and F expect (tools/release/carry/expect.py), so the seeds that only one side of that line can
# meet run for that side.
older=$(unzip -p "$1" RectoPDF.app/Contents/Info.plist | plutil -extract CFBundleShortVersionString raw -)
if [ "$(printf '%s\n' 0.1.1 "$older" | sort -V | head -1)" != 0.1.1 ]; then
  # Failure: a tab says its other list was kept in the state folder, and the check believes it without looking.
  out=$(run aside "$1" "$2"); rc=$?
  check "the set-aside list gone fails the check" 1 "$rc"
  has "naming the journal" "version-2 journal is on disk byte for byte, as None" "$out"
  has "and the notice it makes untrue" "FAIL  F: the hidden highlight is said on its tab" "$out"
else
  # Failure: an older release that reads highlights loses one on going back, and the check passes it as 0.1.0's way of leaving it alone.
  out=$(run highlight "$1" "$2"); rc=$?
  check "the highlight's journal lost on the way back fails the check" 1 "$rc"
  has "naming the tab in the older release" "FAIL  C: $older reads" "$out"
fi
# Failure: the check fails the real pair, so its failures on a made fault mean nothing.
out=$(run "" "$1" "$2"); rc=$?
check "the real pair passes" 0 "$rc"
has "and says so" "did, and back again" "$out"
if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
