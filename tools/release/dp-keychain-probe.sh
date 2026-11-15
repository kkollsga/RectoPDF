#!/usr/bin/env bash
# release/dp-keychain-probe: what the app's Find key in the data-protection keychain will depend on, on the team's real signature and the
# release's real provisioning profile, before the app depends on it (the owner's decision, 2026-11-15: the key goes in the data-protection
# keychain, in the team's access group). Run by the release preflight, on its runner only: the data-protection keychain has no file of its
# own to point a probe at, so nothing of this runs on a person's Mac.
#
#   tools/release/dp-keychain-probe.sh <signing identity> <provisioning profile>
#
# First the profile is checked, and the run stops with a plain sentence if it isn't the one the app needs: a macOS Developer ID profile
# (all devices) for the app's App ID and team, allowing the app's keychain group, expiring more than 60 days from now, and made for the
# certificate that signs (its DeveloperCertificates hold the signing identity's). Then tools/release/dp-keychain-probe.swift is built as:
#   A and B   app-like bundles of the app's bundle identifier with the profile embedded, signed by the team with the hardened runtime and
#             the entitlements on the main executable (the App ID, the team, the keychain group); B is a later build, as an update is.
#   C         B's bundle signed again ad hoc, without the entitlements: a development build, or any program of this user's.
#   C+        B's bundle signed ad hoc, claiming the entitlements: a planter pretending to be the app.
#   D         a plain program, ad hoc.
#   E         a program the team signed with another identifier and no profile: what a team-signed `recto` would be.
# What must hold, and fails the run, each a line with the OSStatus and the milliseconds:
#   1. A adds the item, and reads it.
#   2. B, a later build, reads it, the same bytes, with no prompt.
#   3. C, D and E can't read it, add one in the group, change its data or delete it (errSecMissingEntitlement -34018, or not found
#      -25300): the two things the login keychain let an ad hoc program do (runs 43706807396, 43708234469) among them.
#   4. C+ is stopped by the system at launch, or, if it runs, is refused as C is.
#   5. B still reads A's bytes after all of that.
#   6. A deletes it, and B then finds nothing.
# No step may wait: there's no switch to turn prompts off here, and one that waits past its limit is taken as a prompt and fails the run.
# Prints status codes, times and an 8-hex prefix of the probe's own random bytes' hash; never the profile, the certificate or the item.
# Removes the item and everything it made on any exit.
set -euo pipefail
umask 077
identity="${1:?usage: tools/release/dp-keychain-probe.sh <signing identity> <provisioning profile>}"
profile="${2:?usage: tools/release/dp-keychain-probe.sh <signing identity> <provisioning profile>}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
dir="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/dp-keychain-probe.XXXXXX")"
team=$(sed -n 's/^pub const TEAM_ID: &str = "\([A-Z0-9]*\)";$/\1/p' "$root/crates/recto-update/src/lib.rs")
[ -n "$team" ] || { echo "::error::the team's ID couldn't be read from crates/recto-update/src/lib.rs" >&2; exit 1; }
bundle_id=io.github.kkollsga.RectoPDF
app_id="$team.$bundle_id"
group="$app_id"
# shellcheck disable=SC2329 # run by the trap below
cleanup() {
  for build in A B; do
    [ -x "$dir/$build.app/Contents/MacOS/probe" ] && perl -e 'alarm shift; exec @ARGV' 20 "$dir/$build.app/Contents/MacOS/probe" delete "$group" >/dev/null 2>&1 && break
  done
  rm -rf "$dir"
}
trap cleanup EXIT

echo "The provisioning profile"
sign_key="$("$root/app/render-helper.sh" --key "$identity")"
security cms -D -i "$profile" > "$dir/profile.plist" 2>/dev/null \
  || { echo "::error::The provisioning profile can't be read as a signed profile: MACOS_PROVISION_PROFILE isn't a .provisionprofile, base64" >&2; exit 1; }
python3 -I - "$dir/profile.plist" "$app_id" "$team" "$sign_key" <<'EOF'
import datetime, hashlib, plistlib, sys
path, app_id, team, signing_sha1 = sys.argv[1:5]
with open(path, "rb") as file:
    profile = plistlib.load(file)
entitlements = profile.get("Entitlements", {})
groups = entitlements.get("keychain-access-groups", [])
expires = profile.get("ExpirationDate")
if expires is not None and expires.tzinfo is None:
    expires = expires.replace(tzinfo=datetime.timezone.utc)
now = datetime.datetime.now(datetime.timezone.utc)
certificates = {hashlib.sha1(der).hexdigest().upper() for der in profile.get("DeveloperCertificates", [])}
problems = []
if "OSX" not in profile.get("Platform", []):
    problems.append("it isn't a macOS profile")
if not profile.get("ProvisionsAllDevices") or profile.get("ProvisionedDevices"):
    problems.append("it isn't a Developer ID profile: it doesn't provision every Mac")
if entitlements.get("com.apple.application-identifier") != app_id:
    problems.append(f"its App ID isn't {app_id}")
if entitlements.get("com.apple.developer.team-identifier") != team or team not in profile.get("TeamIdentifier", []):
    problems.append(f"it isn't team {team}'s")
if app_id not in groups and f"{team}.*" not in groups:
    problems.append(f"it doesn't allow the keychain group {app_id}")
if expires is None or expires - now < datetime.timedelta(days=60):
    problems.append("it expires within 60 days, or says no date: make a new one in the developer account")
if signing_sha1.upper() not in certificates:
    problems.append("it wasn't made for the certificate that signs: the certificate in MACOS_CERT_P12 isn't one of the profile's")
for problem in problems:
    print(f"::error::The provisioning profile: {problem}.")
if problems:
    sys.exit(1)
print(f"  ok  macOS Developer ID, {app_id}, keychain group allowed, expires {expires:%Y-%m-%d}, made for the signing certificate")
EOF
rm -f "$dir/profile.plist"

# What tells apart why a signing would wait (run 43751288571 hung at its first team signing for the job's whole 20 minutes): the keychains
# searched and the default, the signing keychain's settings (asked under a limit, as a locked keychain may wait to be unlocked), and how many
# signing identities are found. Paths and counts only.
now_ms() { perl -MTime::HiRes=time -e 'printf "%d", time * 1000'; }
keychains() {
  echo "  keychains searched: $(security list-keychains -d user | tr -d '"' | tr -s ' \n' ' ')"
  echo "  default keychain: $(security default-keychain -d user | tr -d '" ')"
  if [ -n "${PREFLIGHT_KEYCHAIN:-}" ]; then
    case " $(security list-keychains -d user | tr -d '"' | tr -s ' \n' ' ') " in
      *" $PREFLIGHT_KEYCHAIN "*) echo "  the signing keychain is searched" ;;
      *) echo "  the signing keychain isn't searched" ;;
    esac
    echo "  the signing keychain's settings: $(perl -e 'alarm shift; exec @ARGV' 10 security show-keychain-info "$PREFLIGHT_KEYCHAIN" 2>&1 | tr '\n' ' ' || true)"
  fi
  echo "  signing identities: $(perl -e 'alarm shift; exec @ARGV' 10 security find-identity -v -p codesigning 2>/dev/null | tail -1 || true)"
}
# limited <seconds> <what> <command...>: the command under a limit, with its milliseconds; one that passes its limit hung, which is said with
# the keychains as they are then, and ends the run.
limited() {
  local limit="$1" what="$2" started code=0 ms
  shift 2
  started=$(now_ms)
  perl -e 'alarm shift; exec @ARGV' "$limit" "$@" || code=$?
  ms=$(($(now_ms) - started))
  if [ "$code" -eq 142 ]; then
    echo "::error::$what didn't finish in $limit s: it hung"
    keychains
    exit 1
  elif [ "$code" -ne 0 ]; then
    echo "::error::$what failed (exit $code, after $ms ms)"
    exit 1
  fi
  echo "  ok  $what ($ms ms)"
}

echo "The keychains, before any signing"
keychains
echo "The builds"
for build in a b d e; do
  sed "s/PROBE_BUILD/$build/" "$root/tools/release/dp-keychain-probe.swift" > "$dir/$build.swift"
  limited 120 "building $build" swiftc -O "$dir/$build.swift" -o "$dir/$build"
done
cat > "$dir/probe.entitlements" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.application-identifier</key><string>$app_id</string>
  <key>com.apple.developer.team-identifier</key><string>$team</string>
  <key>keychain-access-groups</key><array><string>$group</string></array>
</dict></plist>
EOF
bundle() {
  local name="$1" program="$2"
  mkdir -p "$dir/$name.app/Contents/MacOS"
  cp "$program" "$dir/$name.app/Contents/MacOS/probe"
  cp "$profile" "$dir/$name.app/Contents/embedded.provisionprofile"
  cat > "$dir/$name.app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
  <key>CFBundleExecutable</key><string>probe</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleName</key><string>DP keychain probe $name</string>
</dict></plist>
EOF
}
echo "The signings, one thing more each time, so the first that waits says what it was"
cp "$dir/a" "$dir/plain"
limited 60 "the team signing a plain program" codesign --force --options runtime --timestamp=none --sign "$sign_key" "$dir/plain"
cp "$dir/a" "$dir/plain-claims"
limited 60 "the team signing a plain program with the entitlements" \
  codesign --force --options runtime --timestamp=none --entitlements "$dir/probe.entitlements" --sign "$sign_key" "$dir/plain-claims"
bundle A "$dir/a"
bundle B "$dir/b"
for build in A B; do
  limited 60 "the team signing bundle $build, with the entitlements and its profile" \
    codesign --force --options runtime --timestamp=none --entitlements "$dir/probe.entitlements" --sign "$sign_key" "$dir/$build.app"
done
cp -R "$dir/B.app" "$dir/C.app"
limited 60 "signing C ad hoc" codesign --force --sign - "$dir/C.app"
cp -R "$dir/B.app" "$dir/Cplus.app"
limited 60 "signing C+ ad hoc, claiming the entitlements" codesign --force --sign - --entitlements "$dir/probe.entitlements" "$dir/Cplus.app"
limited 60 "signing D ad hoc" codesign --force --sign - -i "$bundle_id.dp-probe-d" "$dir/d"
limited 60 "the team signing E, another identifier" codesign --force --options runtime --timestamp=none --sign "$sign_key" -i "$bundle_id.other" "$dir/e"
cdhash() { codesign -dvvv "$1" 2>&1 | sed -n 's/^CDHash=//p'; }
[ "$(cdhash "$dir/A.app")" != "$(cdhash "$dir/B.app")" ] || { echo "::error::A and B are the same build" >&2; exit 1; }
for name in A B C Cplus; do
  claims=$(codesign -d --entitlements - --xml "$dir/$name.app" 2>/dev/null | grep -c "$app_id" || true)
  echo "  $name: $(codesign -dv "$dir/$name.app" 2>&1 | sed -n 's/^TeamIdentifier=/team /p'), claims the App ID: $([ "$claims" -gt 0 ] && echo yes || echo no)"
done

failed=0
sha_of_a=""
last_sha=""
# One step: what it must come to, which build, and what it does. `ok` is status 0; `refused` is errSecMissingEntitlement (-34018) or
# errSecItemNotFound (-25300); `gone` is -25300; `stopped` is the system killing the program at launch (no status line, exit 128 or more,
# not the limit's 142), or else `refused`; `report` asserts nothing. A step past its limit waited, as on a prompt: that fails whatever
# it wanted.
step() {
  local want="$1" name="$2" mode="$3" program out code=0 status line
  case "$name" in
    A | B | C | Cplus) program="$dir/$name.app/Contents/MacOS/probe" ;;
    D) program="$dir/d" ;;
    E) program="$dir/e" ;;
  esac
  out="$(perl -e 'alarm shift; exec @ARGV' 20 "$program" "$mode" "$group" 2>/dev/null)" || code=$?
  status="$(printf '%s' "$out" | sed -n 's/^status=\(-*[0-9]*\) .*/\1/p')"
  last_sha="$(printf '%s' "$out" | sed -n 's/.* sha=\([0-9a-f]*\)$/\1/p')"
  if [ "$code" -eq 142 ]; then
    status=waited
  elif [ -z "$status" ] && [ "$code" -ge 128 ]; then
    status="stopped (exit $code)"
  fi
  line="$name $mode: ${status:-no answer (exit $code)} ${out#status=* }"
  case "$want:$status" in
    *:waited) echo "::error::waited, as on a prompt: $line"; failed=1 ;;
    ok:0 | refused:-34018 | refused:-25300 | gone:-25300 | stopped:stopped* | stopped:-34018 | stopped:-25300 | report:*) echo "  $want  $line" ;;
    *) echo "::error::wanted $want: $line"; failed=1 ;;
  esac
}
same_bytes() {
  if [ -n "$sha_of_a" ] && [ "$last_sha" = "$sha_of_a" ]; then
    echo "  ok  $1 reads the bytes A made"
  else
    echo "::error::$1 doesn't read the bytes A made"
    failed=1
  fi
}

echo "1. A adds the item, and reads it"
step ok A add
step ok A read
sha_of_a="$last_sha"
echo "2. B, a later build, reads it"
step ok B read
same_bytes B
echo "3. A development build, a plain program and another of the team's are refused everything"
for name in C D E; do
  for mode in read add update delete; do step refused "$name" "$mode"; done
done
echo "4. A program claiming the app's entitlements without its profile is stopped"
for mode in read update delete; do step stopped Cplus "$mode"; done
echo "5. B still reads what A made"
step ok B read
same_bytes B
echo "6. A deletes it, and B then finds nothing"
step ok A delete
step gone B read
exit "$failed"
