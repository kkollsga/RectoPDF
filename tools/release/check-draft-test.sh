#!/usr/bin/env bash
# check-draft-test: proves tools/release/check-draft.sh on real bundles: the published previous release passes as itself, and made faults
# are each refused with their reason. Needs what no throwaway folder can make: a release signed by the team (the published one) and a
# bundle built here (ad hoc), so it isn't part of release-test.sh. About a minute; read-only on its inputs, its copies in a temp folder.
#
#   tools/release/check-draft-test.sh <dir with RectoPDF-P.zip, SHA256SUMS, latest.json> <dir with a locally built RectoPDF-X.Y.Z.zip, SHA256SUMS, latest.json>
#
# The first is the previous release as published (gh release download vP --dir <dir>), the second app/release-zip.sh's ad hoc assets.
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/../.." && pwd)"
[ $# = 2 ] && [ -d "$1" ] && [ -d "$2" ] || { sed -n '6p' "$0" >&2; exit 2; }
REL="$(cd "$1" && pwd)" LOCAL="$(cd "$2" && pwd)"
PREV="$(ls "$REL"/RectoPDF-*.zip)"
X=$(mktemp -d "${TMPDIR:-/tmp}/check-draft-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
CHECK="$SOURCE/tools/release/check-draft.sh"
fails=0
check() { if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi; }
has() { case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$3]"; fails=$((fails + 1)) ;; esac; }
copy() { rm -rf "$X/$1"; mkdir -p "$X/$1"; cp "$2"/RectoPDF-*.zip "$2/SHA256SUMS" "$2/latest.json" "$X/$1/"; }
feed() { python3 -I -c 'import json,sys; f=json.load(open(sys.argv[1])); f.update(json.loads(sys.argv[2])); open(sys.argv[1],"w").write(json.dumps(f, separators=(",",":"))+"\n")' "$1" "$2"; }
resum() { (cd "$1" && shasum -a 256 RectoPDF-*.zip > SHA256SUMS) && feed "$1/latest.json" "{\"sha256\":\"$(cut -d' ' -f1 "$1/SHA256SUMS")\"}"; }
v_prev="$(basename "$PREV" .zip)"; v_prev="${v_prev#RectoPDF-}"
# The wrong-feed mutation must change even the current published fixture, including 0.1.2.
wrong_version=0.0.0
[ "$v_prev" != "$wrong_version" ] || wrong_version=0.0.1

# Failure: the script refuses the release that is out there and was checked by hand, so its refusals of a new one mean nothing.
out=$("$CHECK" "$REL" --previous "$PREV" 2>&1); rc=$?
check "the published $v_prev passes as itself" 0 "$rc"
has "with every check said" "every check passed for RectoPDF $v_prev" "$out"

# Failure: a draft whose zip isn't the file SHA256SUMS names (a re-upload, a truncated download) passes.
copy sums "$REL"
sed 's/^./0/' "$X/sums/SHA256SUMS" > "$X/t" && { cmp -s "$X/t" "$X/sums/SHA256SUMS" && sed 's/^./1/' "$X/sums/SHA256SUMS" > "$X/t"; mv "$X/t" "$X/sums/SHA256SUMS"; }
out=$("$CHECK" "$X/sums" --previous "$PREV" 2>&1); rc=$?
check "a wrong checksum is refused" 1 "$rc"
has "and named" "FAIL  assets: the zip's checksum is" "$out"

# Failure: latest.json names another version than the zip and the tag, so every copy is offered something it then refuses (or worse).
copy feed "$REL"
feed "$X/feed/latest.json" "{\"version\":\"$wrong_version\"}"
out=$("$CHECK" "$X/feed" --previous "$PREV" 2>&1); rc=$?
check "a feed naming another version is refused" 1 "$rc"
has "by the feed check" "FAIL  feed: version is '$wrong_version', not '$v_prev'" "$out"
has "and by the previous release's own reader of it" "FAIL  verdict: $v_prev's own updater refuses it" "$out"
copy feed2 "$REL"
feed "$X/feed2/latest.json" "{\"version\":\"$wrong_version\",\"tag\":\"v$wrong_version\",\"zip\":\"RectoPDF-$wrong_version.zip\"}"
out=$("$CHECK" "$X/feed2" --previous "$PREV" 2>&1); rc=$?
check "a well-formed feed of another version is refused too" 1 "$rc"
has "by the feed check" "FAIL  feed: version is '$wrong_version'" "$out"

# Failure: the draft is checked as some other version than the tag's.
out=$("$CHECK" "$REL" --previous "$PREV" --version 9.9.9 2>&1); rc=$?
check "a zip that isn't the tag's version is refused" 1 "$rc"
has "and named" "the zip is RectoPDF-$v_prev.zip, not RectoPDF-9.9.9.zip" "$out"

# Failure: a zip damaged after signing (and its checksums made to match) passes, as only the checksum was looked at.
copy broken "$REL"
python3 -I -c 'import sys; p=sys.argv[1]; b=bytearray(open(p,"rb").read()); i=len(b)//2; b[i]^=0xFF; open(p,"wb").write(b)' "$X/broken"/RectoPDF-*.zip
resum "$X/broken"
out=$("$CHECK" "$X/broken" --previous "$PREV" 2>&1); rc=$?
check "a zip damaged after signing is refused" 1 "$rc"
has "by the previous release's own reader" "FAIL  unpack: $v_prev's own reader refuses the zip" "$out"

# Failure: a bundle not signed by the team, not notarized and not stapled (built here, ad hoc) passes as a release.
copy adhoc "$LOCAL"
v_local="$(basename "$(ls "$X/adhoc"/RectoPDF-*.zip)" .zip)"; v_local="${v_local#RectoPDF-}"
out=$("$CHECK" "$X/adhoc" --previous "$PREV" 2>&1); rc=$?
check "an ad hoc bundle is refused" 1 "$rc"
has "its own reader still takes the zip" "ok    unpack: $v_prev's own recto-updater check-zip takes it" "$out"
has "the feed is a release's" "ok    feed: latest.json names $v_local" "$out"
has "no Developer ID" "FAIL  bundle: the app: no Developer ID Application authority" "$out"
has "no ticket" "FAIL  bundle: stapler validate" "$out"
has "Gatekeeper refuses it" "FAIL  bundle: Gatekeeper" "$out"
has "and the previous release's own updater refuses it for its team" "FAIL  verdict: $v_prev's own updater refuses it" "$out"
printf '%s\n' "$out" | grep '^FAIL  verdict' | sed 's/^/      /'

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
