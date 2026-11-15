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
#   - Items planted where the app could meet them (the second security review, S2): for any application, by `security -A` and by
#     SecItemAdd with an open access list, in the keychain the app reads; in a keychain of the planter's own, ahead in the search list,
#     with and without its partition list set for the team, and as the default keychain. Each read by A and B with the app's guard,
#     by the default search list and by the app's own lookup of one named keychain. Reported, for the release's claim.
#   - What an item's access holds (Task A2), reported: the app's own item, made by A and inspected by A and B, and items planted by C
#     and by D trusting A alone, trusting A and the planter, open to any application, and with a partition entry forged at the add or
#     set after it. Each in a fresh keychain, read by the app's lookup of that keychain alone; each line says what the access lists and
#     the partition list hold (names and counts). Measured on the team's signature (run 43706807396): an item a planter makes trusting
#     A alone opens with no prompt for A and B and reads, through public API, exactly as the app's own does (three entries, Decrypt
#     naming A, no partition entry visible); a partition entry for the team forged at the add is taken and shows; set after the add,
#     it's refused. So no check of an item's access can tell the app's own from a planted one, and the app makes none.
#   - Last, after everything that must hold, in a fresh keychain: once A's item is there, what another signature can do to it. C and
#     D add an item of the same service and account, replace its data (SecItemUpdate) and set its access (SecKeychainItemSetAccess);
#     what B then reads is reported. Measured on the team's signature (run 43708234469): C, ad hoc, is refused a second item (-25299)
#     but replaces the app's item's data (0, in 8 ms: its access lets any application encrypt), and its change of access waited; D,
#     the team's with another identifier, is refused all three (-25293); and after C's update B can't read the item (-25293). So a
#     program of this user's can overwrite the app's item at any time, before or after the app makes its key: this keychain can't
#     hold the claim against a program that sets out to.
# What must hold, and fails the run: B, a later build, reads A's item with no prompt; C's and D's reads of it, a locked keychain, and
# C's delete are refused without waiting; and an item in a keychain of a planter's own, ahead in the search list or the default, is
# never found by the app's lookup of the one keychain. Everything else is reported.
# Each probe has a limit, by perl's alarm; one that doesn't answer in it waited, as on a prompt. Prints status codes and times, never
# the item. Uploads nothing. The keychain, the probes and the search list are put back on any exit.
set -euo pipefail
identity="${1:?usage: tools/release/keychain-probe.sh <signing identity>}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/keychain-probe.XXXXXX")"
pk="$dir/probe.keychain-db"
# A keychain of the planter's own, with its own password: what a same-user program can make.
pk2="$dir/planter.keychain-db"
orig_default="$(security default-keychain -d user | tr -d '" ')"
orig_list="$(security list-keychains -d user | tr -d '"')"
# shellcheck disable=SC2329 # run by the trap below
restore() {
  # shellcheck disable=SC2086 # the runner's keychain paths have no spaces
  security list-keychains -d user -s $orig_list >/dev/null 2>&1 || true
  security default-keychain -d user -s "$orig_default" >/dev/null 2>&1 || true
  security delete-keychain "$pk" >/dev/null 2>&1 || true
  security delete-keychain "$pk2" >/dev/null 2>&1 || true
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
  local want="$1" build="$2" mode="$3" interaction="$4" named="${5:-}" limit=20 out code=0 status
  local extra=("${@:6}")
  [ "$interaction" != none ] || limit=10
  out="$(perl -e 'alarm shift; exec @ARGV' "$limit" "$dir/$build" "$mode" "$interaction" ${named:+"$named"} ${extra[@]+"${extra[@]}"})" || code=$?
  if [ "$code" -eq 142 ]; then
    status=waited
  else
    status="$(printf '%s' "$out" | sed -n 's/^status=\(-*[0-9]*\) .*/\1/p')"
  fi
  local line="$build $mode ($interaction${named:+, $(basename "$named") alone}): ${status:-no answer} ${out#status=* }"
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

# Planted where the app could meet it (the second security review, S2). Each read by A and B with the app's guard, by the default search
# list and by the lookup the app makes, of the login keychain alone (here the probe's own keychain stands for it).
team="$(codesign -dv "$dir/a" 2>&1 | sed -n 's/^TeamIdentifier=//p')"
svc=io.github.kkollsga.RectoPDF.keychain-probe
reads() {
  for reader in a b; do
    step report "$reader" read global
    step report "$reader" read global "$pk"
  done
}
# For an item in a keychain of the planter's own: never found by the app's lookup of the one keychain.
reads_elsewhere() {
  for reader in a b; do
    step report "$reader" read global
    step gone "$reader" read global "$pk"
  done
}
echo "Planted for any application, in the keychain the app reads: by security -A"
security add-generic-password -A -s "$svc" -a probe -w "$(openssl rand -hex 16)" "$pk"
reads
security delete-generic-password -s "$svc" -a probe "$pk" >/dev/null
echo "Planted for any application, in the keychain the app reads: by C's SecItemAdd with an open access list"
step ok c add-open global "$pk"
reads
step ok c delete global "$pk"
echo "Planted in a keychain of the planter's own, ahead in the search list"
pk2_pass="$(openssl rand -hex 24)"
security create-keychain -p "$pk2_pass" "$pk2"
security set-keychain-settings "$pk2"
security unlock-keychain -p "$pk2_pass" "$pk2"
security add-generic-password -A -s "$svc" -a probe -w "$(openssl rand -hex 16)" "$pk2"
security list-keychains -d user -s "$pk2" "$pk"
reads_elsewhere
echo "The same, with its partition list set for the team"
security set-generic-password-partition-list -S "teamid:$team,apple-tool:,apple:" -s "$svc" -k "$pk2_pass" "$pk2" >/dev/null
reads_elsewhere
echo "The planter's keychain as the default"
security list-keychains -d user -s "$pk" "$pk2"
security default-keychain -d user -s "$pk2"
reads_elsewhere
security default-keychain -d user -s "$pk"
security list-keychains -d user -s "$pk"
security delete-keychain "$pk2"

# What an item's access holds, and what a rule over it would say (reported: see the header). Each case in a fresh keychain, so nothing of one is left for
# the next, read by the app's lookup of that keychain alone.
fresh() {
  security delete-keychain "$pk" >/dev/null 2>&1 || true
  security create-keychain -p "$pk_pass" "$pk"
  security set-keychain-settings "$pk"
  security unlock-keychain -p "$pk_pass" "$pk"
  security list-keychains -d user -s "$pk"
  security default-keychain -d user -s "$pk"
}
inspects() {
  for reader in a b; do step report "$reader" inspect global "$pk" "$team"; done
}
echo "The app's own item, made by A, as A and B read its access"
fresh
step ok a add global "$pk"
step report a inspect global "$pk" "$team"
step report b inspect global "$pk" "$team"
for planter in c d; do
  echo "Planted by $planter, trusting A alone"
  fresh
  step report "$planter" add-trusting global "$pk" "$team" "$dir/a"
  inspects
  echo "Planted by $planter, trusting A and itself"
  fresh
  step report "$planter" add-trusting global "$pk" "$team" "$dir/a" self
  inspects
  echo "Planted by $planter, trusting A alone, with a partition entry for the team forged at the add"
  fresh
  step report "$planter" add-forged global "$pk" "$team" "$dir/a"
  inspects
  echo "Planted by $planter, then its access set with a forged partition entry"
  fresh
  step report "$planter" add-then-forge global "$pk" "$team" "$dir/a"
  inspects
done
echo "Open to any application, by security -A"
fresh
security add-generic-password -A -s "$svc" -a probe -w "$(openssl rand -hex 16)" "$pk"
inspects
echo "Open to any application, by C's SecItemAdd with an open access list"
fresh
step ok c add-open global "$pk"
inspects
echo "Last: A's item is there; what C and D can do to it, and what B reads after (reported)"
fresh
step ok a add global "$pk"
for other in c d; do
  step report "$other" add global "$pk"
  step report "$other" update global "$pk"
  step report "$other" set-access global "$pk" "$team" self
done
step report b read global "$pk"
exit "$failed"
