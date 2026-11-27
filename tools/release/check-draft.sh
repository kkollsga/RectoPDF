#!/usr/bin/env bash
# release/check-draft: the draft's checks, before it's published. Read-only: it unpacks, asks codesign, spctl and stapler, and runs the
# bundle's programs for their answers (no window), all in a temp folder; it signs, uploads and changes nothing.
#
#   tools/release/check-draft.sh <dir> --previous <RectoPDF-P.zip> [--version X.Y.Z]
#
# <dir> holds the draft's three assets, as `gh release download vX.Y.Z --dir <dir>` leaves them: RectoPDF-X.Y.Z.zip, SHA256SUMS, latest.json.
# --previous is the release before it, as published (the zip its users run); --version is the tag's version (default: the zip's name's).
# It checks, and says each, then all that failed:
#   assets    the three files and no other zip; SHA256SUMS is one line, for that zip, and `shasum -a 256 -c` agrees;
#   feed      latest.json as a release writes it: this version, its tag, its zip, the zip's sha256, the app's LSMinimumSystemVersion;
#   unpack    the previous release's own `recto-updater check-zip` takes the zip (the reader every copy of it will use) into an empty folder;
#   bundle    on what it unpacked: the bundle id, CFBundleShortVersionString is the version, no development version, the licences' first
#             line names it; codesign --verify --deep --strict; codesign -dv: Developer ID Application authority, TeamIdentifier, the
#             hardened runtime flag and a secure Timestamp, for the app and for its renderer helper (and from 0.1.4 its structure helper); the helper's sandbox
#             (app/render-helper.sh --check); stapler validate; spctl: accepted, source=Notarized Developer ID; from 0.1.1, the embedded
#             provisioning profile and the entitlements on the main executable alone (tools/release/app-entitlements.sh --check);
#   release   no override and no QA hook: `recto-updater source` with every override set says release, the real repo, developer_id and
#             no test hooks; `RectoPDF --build-kind` says a release build that takes no test hooks; no test- file in Resources; the bundled
#             `recto --version` names the version;
#   verdict   the previous release's own updater code on the feed and the zip (tools/release/verdict.sh): parsed, on offer to the previous
#             version on this Mac, the size and checksum, and stage::verify_app with the team and Gatekeeper.
# Exit 0 when all pass, 1 when any fails (each named, with its reason), 2 on wrong use.
set -uo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
usage() { sed -n '2,7p' "$0" >&2; exit 2; }
dir="" previous="" version=""
while [ $# -gt 0 ]; do
  case "$1" in
    --previous) previous="${2:-}"; shift 2 || usage ;;
    --version) version="${2:-}"; shift 2 || usage ;;
    -h | --help) usage ;;
    -*) usage ;;
    *) [ -z "$dir" ] || usage; dir="$1"; shift ;;
  esac
done
[ -n "$dir" ] && [ -d "$dir" ] && [ -f "$previous" ] || usage
dir="$(cd "$dir" && pwd)"
TEAM_ID="$(sed -n 's/^pub const TEAM_ID: &str = "\([A-Z0-9]*\)";$/\1/p' "$REPO/crates/recto-update/src/lib.rs")"
REPO_URL="$(sed -n 's/^pub const REPO: &str = "\(.*\)";$/\1/p' "$REPO/crates/recto-update/src/lib.rs")"
BUNDLE_ID="$(sed -n 's/^pub const BUNDLE_ID: &str = "\(.*\)";$/\1/p' "$REPO/crates/recto-update/src/lib.rs")"
W=$(mktemp -d "${TMPDIR:-/tmp}/check-draft.XXXXXX")
# The copies it unpacked are forgotten by Launch Services before they go, as asking Gatekeeper and running their programs registers them.
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
trap '"$LSREGISTER" -u "$W/unpacked/RectoPDF.app" 2>/dev/null; "$LSREGISTER" -u "$W/previous/RectoPDF.app" 2>/dev/null; rm -rf "$W"' EXIT
failed=()
pass() { echo "ok    $1"; }
fail() { echo "FAIL  $1: $2"; failed+=("$1: $2"); }
finish() {
  if [ ${#failed[@]} -gt 0 ]; then
    echo "check-draft: ${#failed[@]} check(s) failed, so this draft is not to be published:"
    printf '  %s\n' "${failed[@]}"
    exit 1
  fi
  echo "check-draft: every check passed for RectoPDF $version."
  exit 0
}

# ---- assets ----
zips=("$dir"/RectoPDF-*.zip)
if [ ${#zips[@]} != 1 ] || [ ! -f "${zips[0]}" ]; then fail assets "there should be one RectoPDF-X.Y.Z.zip in $dir"; finish; fi
zip="${zips[0]}" zip_name="$(basename "${zips[0]}")"
named="${zip_name#RectoPDF-}"; named="${named%.zip}"
[ -n "$version" ] || version="$named"
[ "$named" = "$version" ] || fail assets "the zip is $zip_name, not RectoPDF-$version.zip"
for f in SHA256SUMS latest.json; do [ -f "$dir/$f" ] || fail assets "no $f"; done
[ ${#failed[@]} = 0 ] || finish
sum="$(shasum -a 256 "$zip" | cut -d' ' -f1)"
if [ "$(grep -c . "$dir/SHA256SUMS")" != 1 ] || ! grep -qxE "[0-9a-f]{64}  $zip_name" "$dir/SHA256SUMS"; then
  fail assets "SHA256SUMS should be one line naming $zip_name: $(head -c 200 "$dir/SHA256SUMS")"
elif ! (cd "$dir" && shasum -a 256 -c SHA256SUMS >/dev/null 2>&1); then
  fail assets "the zip's checksum is $sum, not SHA256SUMS's $(cut -d' ' -f1 "$dir/SHA256SUMS")"
else pass "assets: $zip_name, SHA256SUMS agrees ($sum)"; fi

# ---- unpack, with the previous release's own reader ----
mkdir -p "$W/previous" "$W/unpacked"
ditto -x -k "$previous" "$W/previous" 2>/dev/null || { fail unpack "the previous release's zip doesn't unpack: $previous"; finish; }
OLD="$W/previous/RectoPDF.app"
old_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$OLD/Contents/Info.plist" 2>/dev/null)"
if out=$("$OLD/Contents/MacOS/recto-updater" check-zip "$zip" "$W/unpacked" 2>&1); then pass "unpack: $old_version's own recto-updater check-zip takes it"
else fail unpack "$old_version's own reader refuses the zip: $out"; finish; fi
APP="$W/unpacked/RectoPDF.app"
plist() { /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist" 2>/dev/null; }

# ---- feed ----
min_macos="$(plist "$APP" LSMinimumSystemVersion)"
feed_check() { # latest.json, version, sha256, minimum macOS: says what's wrong, exit 1, or nothing
  python3 -I - "$@" <<'PY'
import json, sys
path, version, sha, min_macos = sys.argv[1:5]
raw = open(path, "rb").read()
feed = json.loads(raw)
want = {"version": version, "tag": f"v{version}", "minimum_macos": min_macos, "zip": f"RectoPDF-{version}.zip", "sha256": sha}
wrong = [f"{k} is {feed.get(k)!r}, not {v!r}" for k, v in want.items() if feed.get(k) != v]
extra = sorted(set(feed) - set(want))
if extra: wrong.append(f"fields a release doesn't write: {extra}")
if wrong: sys.exit("; ".join(wrong))
PY
}
if out=$(feed_check "$dir/latest.json" "$version" "$sum" "$min_macos" 2>&1); then pass "feed: latest.json names $version, v$version, $zip_name, its sha256 and macOS $min_macos"
else fail feed "${out:-latest.json does not parse}"; fi

# ---- bundle ----
[ "$(plist "$APP" CFBundleIdentifier)" = "$BUNDLE_ID" ] && pass "bundle: id $BUNDLE_ID" || fail bundle "its id is $(plist "$APP" CFBundleIdentifier), not $BUNDLE_ID"
[ "$(plist "$APP" CFBundleShortVersionString)" = "$version" ] && pass "bundle: version $version" || fail bundle "its version is $(plist "$APP" CFBundleShortVersionString), not the tag's $version"
[ -z "$(plist "$APP" RectoDevVersion)" ] && pass "bundle: no development version" || fail bundle "it carries a development version, $(plist "$APP" RectoDevVersion)"
first="$(head -1 "$APP/Contents/Resources/Licenses.txt" 2>/dev/null)"
[ "$first" = "RectoPDF $version: third-party software notices" ] && pass "bundle: the licences name $version" || fail bundle "its Licenses.txt says \"$first\" (THIRD-PARTY-NOTICES.txt not bumped?)"
codesign --verify --deep --strict "$APP" 2>"$W/v" && pass "bundle: codesign --verify --deep --strict" || fail bundle "the signature doesn't verify: $(tr '\n' ' ' < "$W/v")"
signed_by() { # what, path
  local d; d="$(codesign -dv --verbose=4 "$2" 2>&1)"
  local why=""
  printf '%s\n' "$d" | grep -q '^Authority=Developer ID Application: .*('"$TEAM_ID"')$' || why="$why no Developer ID Application authority of $TEAM_ID;"
  printf '%s\n' "$d" | grep -qx "TeamIdentifier=$TEAM_ID" || why="$why TeamIdentifier is $(printf '%s\n' "$d" | sed -n 's/^TeamIdentifier=//p');"
  printf '%s\n' "$d" | grep -E '^CodeDirectory .*flags=' | grep -q '(runtime)\|,runtime\|runtime,' || why="$why no hardened runtime;"
  printf '%s\n' "$d" | grep -q '^Timestamp=' || why="$why no secure timestamp;"
  if [ -z "$why" ]; then pass "bundle: $1 is Developer ID signed by $TEAM_ID, hardened runtime, timestamped"; else fail bundle "$1:$why"; fi
}
signed_by "the app" "$APP"
signed_by "the renderer helper" "$APP/Contents/Helpers/RectoRender.app"
# From 0.1.4 the structure helper is in the bundle too (S6b), signed by the same team.
if [ "$(printf '%s\n' 0.1.4 "$version" | sort -V | head -1)" = 0.1.4 ]; then signed_by "the structure helper" "$APP/Contents/Helpers/RectoStructure.app"; fi
# From 0.1.1, the Find key lives in the data-protection keychain in the app's own group (the owner's decision, 2026-11-15): the bundle embeds its
# provisioning profile and its main executable alone claims the App ID, the team and the group. 0.1.0 had neither.
if [ "$(printf '%s\n' 0.1.1 "$version" | sort -V | head -1)" = 0.1.1 ]; then
  out=$("$REPO/tools/release/app-entitlements.sh" --check "$APP" 2>&1) && pass "bundle: $(printf '%s' "$out" | tail -1)" || fail bundle "$(printf '%s' "$out" | tr '\n' ' ')"
fi
out=$("$REPO/app/render-helper.sh" --check "$APP" 2>&1) && pass "bundle: the renderer helper is sandboxed as it should be" || fail bundle "render-helper.sh --check: $(printf '%s' "$out" | tail -2 | tr '\n' ' ')"
out=$(xcrun stapler validate "$APP" 2>&1) && pass "bundle: notarization ticket stapled" || fail bundle "stapler validate: $(printf '%s' "$out" | tail -1)"
out=$(spctl --assess --type execute -vv "$APP" 2>&1)
case "$out" in *": accepted"*"source=Notarized Developer ID"*) pass "bundle: Gatekeeper accepts it, source=Notarized Developer ID" ;; *) fail bundle "Gatekeeper: $(printf '%s' "$out" | tr '\n' ' ')" ;; esac

# ---- release: no override, no QA hook ----
out=$(RECTO_UPDATE_REPO=http://127.0.0.1:1 RECTO_UPDATE_TRUST=any RECTO_UPDATER_DIE_AT=swapped "$APP/Contents/MacOS/recto-updater" source 2>&1)
case "$out" in
  *"\"mode\":\"release\""*"\"repo\":\"$REPO_URL\""*"\"release_build\":true"*"\"trust\":\"developer_id\""*"\"takes_test_hooks\":false"*) pass "release: the installer reads no override ($out)" ;;
  *) fail release "the installer would use an override: $out" ;;
esac
out=$("$APP/Contents/MacOS/RectoPDF" --build-kind 2>&1)
case "$out" in *"\"release_build\":true"*"\"takes_test_hooks\":false"*) pass "release: the app launches, with its entitlements, and is a release build with no test hooks" ;; *) fail release "the app didn't launch or isn't a release build: $out" ;; esac
tests_in=$(ls "$APP/Contents/Resources/" | grep '^test-' | tr '\n' ' ')
[ -z "$tests_in" ] && pass "release: no test file in Resources" || fail release "a test's file is in Resources: $tests_in"
out=$("$APP/Contents/MacOS/recto" --version 2>&1)
case "$out" in "recto $version, API version "*) pass "release: the bundled recto answers \"$out\"" ;; *) fail release "the bundled recto --version says \"$out\"" ;; esac

# ---- verdict: the previous release's own updater code ----
out=$("$REPO/tools/release/verdict.sh" "v$old_version" "$dir/latest.json" "$zip" "$W/unpacked" "$old_version" 2>&1); rc=$?
last="$(printf '%s\n' "$out" | tail -1)"
if [ $rc = 0 ]; then pass "verdict: $old_version's own updater takes it ($(printf '%s\n' "$out" | grep '"step":"offer"' | sed 's/.*"detail":"\([^"]*\)".*/\1/'))"
else fail verdict "$old_version's own updater refuses it: $last"; fi
finish
