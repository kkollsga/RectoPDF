#!/usr/bin/env bash
# release/keychain-probe: what the app's Find key depends on, on the team's real signature (the coordinator's decision, 2026-11-15:
# until this has run, that a team-signed copy reads its item with no prompt, and a later one too, is only read, not measured).
#
#   tools/release/keychain-probe.sh <signing identity>
#
# Builds tools/release/keychain-probe.swift four times: A and B signed by the team with one identifier (B a later build, as an update
# is), C ad hoc with that identifier (as a development build), D signed by the team with another identifier (in the team's partition
# and not on the item's list of trusted programs: what a team-signed `recto` would be). Then, in a keychain of its own made here,
# with its own password, made the default and the only one searched for as long as this runs (so the probe asks as the app does, by
# the default search list, and touches nothing else):
#   - A makes the item; B must read it, each way.
#   - C and D must be refused, and the keychain locked must refuse B, with the whole process's switch: the app's way. The per-query
#     flag alone, and no guard at all, are run too and reported, as Apple's header says the flag doesn't govern this keychain: with
#     no guard a case that would prompt is seen to wait (that shows it's real).
#   - An item planted by C, and one by D, under the same names: what A and B get is reported (whether a program of this user's could
#     plant a key it knows).
# Each probe has a limit, by perl's alarm; one that doesn't answer in it waited, as on a prompt. Prints status codes and times, never
# the item. Uploads nothing. The keychain, the probes and the search list are put back on any exit.
set -euo pipefail
identity="${1:?usage: tools/release/keychain-probe.sh <signing identity>}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/keychain-probe.XXXXXX")"
pk="$dir/probe.keychain-db"
orig_default="$(security default-keychain -d user | tr -d '" ')"
orig_list="$(security list-keychains -d user | tr -d '"')"
# shellcheck disable=SC2329 # run by the trap below
restore() {
  # shellcheck disable=SC2086 # the runner's keychain paths have no spaces
  security list-keychains -d user -s $orig_list >/dev/null 2>&1 || true
  security default-keychain -d user -s "$orig_default" >/dev/null 2>&1 || true
  security delete-keychain "$pk" >/dev/null 2>&1 || true
  rm -rf "$dir"
}
trap restore EXIT

id=io.github.kkollsga.RectoPDF.keychain-probe
for build in a b c d; do
  sed "s/PROBE_BUILD/$build/" "$root/tools/release/keychain-probe.swift" > "$dir/$build.swift"
  swiftc -O "$dir/$build.swift" -o "$dir/$build"
done
# By its SHA-1, as codesign can't find a name with a letter outside ASCII in it (app/render-helper.sh, `key`). Signed before the
# search list is changed, as the identity is in the job's own keychain.
sign_key="$("$root/app/render-helper.sh" --key "$identity")"
codesign --force --options runtime --sign "$sign_key" -i "$id" "$dir/a"
codesign --force --options runtime --sign "$sign_key" -i "$id" "$dir/b"
codesign --force --options runtime --sign "$sign_key" -i "$id.other" "$dir/d"
codesign --force --sign - -i "$id" "$dir/c"
codesign -dv "$dir/a" 2>&1 | grep -E '^TeamIdentifier='
[ "$(codesign -dvvv "$dir/a" 2>&1 | grep CDHash=)" != "$(codesign -dvvv "$dir/b" 2>&1 | grep CDHash=)" ] || { echo "::error::A and B are the same build"; exit 1; }

pk_pass="$(openssl rand -hex 24)"
security create-keychain -p "$pk_pass" "$pk"
security set-keychain-settings "$pk"
security unlock-keychain -p "$pk_pass" "$pk"
security list-keychains -d user -s "$pk"
security default-keychain -d user -s "$pk"

failed=0
# One probe: what it must come to, then the probe, its mode and how interaction is refused. `ok` is status 0; `refused` is
# errSecAuthFailed (-25293), errSecInteractionNotAllowed (-25308) or, for a delete, errSecInvalidOwnerEdit (-25244), and nothing
# else; `gone` is errSecItemNotFound (-25300); `report` asserts nothing and says what happened. A probe past its limit waited, as on a
# prompt, which is never what `refused` is.
step() {
  local want="$1" build="$2" mode="$3" interaction="$4" limit=20 out code=0 status
  [ "$interaction" != none ] || limit=10
  out="$(perl -e 'alarm shift; exec @ARGV' "$limit" "$dir/$build" "$mode" "$interaction")" || code=$?
  if [ "$code" -eq 142 ]; then
    status=waited
  else
    status="$(printf '%s' "$out" | sed -n 's/^status=\(-*[0-9]*\) .*/\1/p')"
  fi
  local line="$build $mode ($interaction): ${status:-no answer} ${out#status=* }"
  case "$want:$status" in
    ok:0 | refused:-25293 | refused:-25308 | refused:-25244 | gone:-25300 | report:*) echo "  $want  $line" ;;
    *) echo "::error::wanted $want: $line (exit $code)"; failed=1 ;;
  esac
}

echo "A makes the item; B, a later build, reads it"
step ok a add global
for interaction in global per-call none; do step ok b read "$interaction"; done
echo "Refused, with the app's guard; the other ways reported"
for build in c d; do
  step refused "$build" read global
  step report "$build" read per-call
  step report "$build" read none
done
step refused c delete global
echo "The keychain locked"
security lock-keychain "$pk"
step refused b read global
step report b read per-call
step report b read none
security unlock-keychain -p "$pk_pass" "$pk"
step ok a delete global
step gone a read global
for planter in c d; do
  echo "An item planted by $planter"
  step ok "$planter" add global
  for reader in a b; do
    for interaction in global per-call none; do step report "$reader" read "$interaction"; done
  done
  step ok "$planter" delete global
done
step gone a read global
exit "$failed"
