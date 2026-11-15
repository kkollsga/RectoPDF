#!/usr/bin/env bash
# release/keychain-probe: shows on the team's real signature what the app's Find key depends on (the coordinator's decision, 2026-11-15:
# until this has run, that a team-signed copy reads its item with no prompt, and a later one too, is only read, not measured).
# Three builds of tools/release/keychain-probe.swift with one identifier: A and B signed by the team (B is a later build, as an update
# is), C ad hoc (as a development build). A makes an item; B must read it; C must be refused; nothing may wait on a prompt. Each with
# interaction refused on each query alone, and with the whole process's switch too. Then A removes it.
#
#   tools/release/keychain-probe.sh <keychain path> <signing identity>
#
# Uses only the keychain it's given, the job's throwaway one. Prints status codes and times, never the item. Uploads nothing; removes
# the probes on any exit.
set -euo pipefail
kc="${1:?usage: tools/release/keychain-probe.sh <keychain path> <signing identity>}"
identity="${2:?usage: tools/release/keychain-probe.sh <keychain path> <signing identity>}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/keychain-probe.XXXXXX")"
trap 'rm -rf "$dir"' EXIT
id=io.github.kkollsga.RectoPDF.keychain-probe
for build in a b c; do
  sed "s/PROBE_BUILD/$build/" "$root/tools/release/keychain-probe.swift" > "$dir/$build.swift"
  swiftc -O "$dir/$build.swift" -o "$dir/$build" 2>/dev/null
done
# By its SHA-1, as codesign can't find a name with a letter outside ASCII in it (app/render-helper.sh, `key`).
sign_key="$("$root/app/render-helper.sh" --key "$identity")"
codesign --force --options runtime --sign "$sign_key" -i "$id" "$dir/a"
codesign --force --options runtime --sign "$sign_key" -i "$id" "$dir/b"
codesign -dv "$dir/a" 2>&1 | grep -E '^TeamIdentifier='
codesign --force --sign - -i "$id" "$dir/c"
[ "$(codesign -dvvv "$dir/a" 2>&1 | grep CDHash=)" != "$(codesign -dvvv "$dir/b" 2>&1 | grep CDHash=)" ] || { echo "::error::A and B are the same build"; exit 1; }

failed=0
# One step: what it must come to (ok or refused), and the probe's line. A probe that hasn't answered in 20 s is taken as waiting on a prompt.
step() {
  local want="$1"; shift
  local started=$SECONDS out code=0
  out="$(perl -e 'alarm shift; exec @ARGV' 20 "$@")" || code=$?
  local took=$((SECONDS - started))
  if [ "$code" -eq 142 ] || [ "$took" -ge 20 ]; then
    echo "::error::$(basename "$1") $2 ($4) didn't answer in 20 s: it waited, as on a prompt"; failed=1; return
  fi
  case "$want:$code" in
    ok:0 | refused:1) echo "as it should: $out" ;;
    *) echo "::error::wanted $want: $out (exit $code)"; failed=1 ;;
  esac
}
step ok "$dir/a" add "$kc" global
for interaction in per-call global; do
  step ok "$dir/b" read "$kc" "$interaction"
  step refused "$dir/c" read "$kc" "$interaction"
  step refused "$dir/c" delete "$kc" "$interaction"
done
step ok "$dir/a" delete "$kc" global
step refused "$dir/a" read "$kc" global
exit "$failed"
