#!/usr/bin/env bash
# The Mac's release assets: full-LTO build, signed app, zip, SHA256SUMS, latest.json and the release notes. Everything
# lands in app/build/release/, and nothing is published.
#
#   app/release-zip.sh [--allow-dirty]            A local check: built, run, signed ad hoc, zipped and checked. Not distributable.
#   app/release-zip.sh --signed [--allow-dirty]   The release on this Mac: all of that, Developer ID signed and notarized.
#
# What release.yml does, in jobs of their own, so no signing key is ever where the product is built or run:
#   app/release-zip.sh --built-only               Build, check and run, with no keys anywhere. The build job.
#   app/release-zip.sh --signed --already-built   Sign, notarize, staple and zip what was built, and run nothing of it. The signing job.
#   app/release-zip.sh --check-signed             Run the stapled app and check the zip, once the keys are gone again.
#
# --signed needs (docs/RELEASING.md, "What the owner has to add"):
#   RECTO_SIGN_IDENTITY    "Developer ID Application: <name> (<team>)", of TEAM_ID in crates/recto-update/src/lib.rs
#   RECTO_NOTARY_PROFILE   a notarytool keychain profile (xcrun notarytool store-credentials)
#   RECTO_NOTARY_KEYCHAIN  the keychain holding that profile, when it isn't the default one (CI's temporary one)
# Without them it fails. There's no unsigned release.
#
# It never creates a release: that's only the release workflow, when everything has passed.
# Wrap it in tools/sysguard/heavy.sh: the build is a full-LTO build.
set -euo pipefail

APP_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$APP_DIR/.." && pwd)"
ALLOW_DIRTY=0; SIGNED=0; BUILT_ONLY=0; ALREADY_BUILT=0; CHECK_SIGNED=0
for a in "$@"; do
    case "$a" in
        --allow-dirty) ALLOW_DIRTY=1 ;;
        --signed) SIGNED=1 ;;
        --built-only) BUILT_ONLY=1 ;;
        --already-built) ALREADY_BUILT=1 ;;
        --check-signed) CHECK_SIGNED=1 ;;
        -h|--help) sed -n '2,19p' "$0"; exit 0 ;;
        *) echo "unknown argument: $a" >&2; exit 2 ;;
    esac
done

if [ -n "$(git -C "$REPO" status --porcelain)" ]; then
    if [ "$ALLOW_DIRTY" = 1 ]; then echo "WARNING: dirty git tree (--allow-dirty): a test build, not a release."
    else echo "Refusing to release a dirty git tree. Commit first, or pass --allow-dirty for a test build." >&2; exit 1; fi
fi

# The one team a release is signed by, and the one repo. The app refuses any other, and this is where it says so.
UPDATE_SOURCE="$REPO/crates/recto-update/src/lib.rs"
TEAM_ID="$(sed -n 's/^pub const TEAM_ID: &str = "\([A-Z0-9]*\)";$/\1/p' "$UPDATE_SOURCE")"
REPO_URL="$(sed -n 's/^pub const REPO: &str = "\(.*\)";$/\1/p' "$UPDATE_SOURCE")"
[ -n "$TEAM_ID" ] && [ -n "$REPO_URL" ] || { echo "no TEAM_ID or REPO in $UPDATE_SOURCE" >&2; exit 1; }
if [ "$SIGNED" = 1 ]; then
    : "${RECTO_SIGN_IDENTITY:?--signed needs RECTO_SIGN_IDENTITY (docs/RELEASING.md)}"
    : "${RECTO_NOTARY_PROFILE:?--signed needs RECTO_NOTARY_PROFILE (docs/RELEASING.md)}"
    case "$RECTO_SIGN_IDENTITY" in
        "Developer ID Application: "*"($TEAM_ID)") ;;
        *) echo "RECTO_SIGN_IDENTITY must be a Developer ID Application identity of team $TEAM_ID: $RECTO_SIGN_IDENTITY" >&2; exit 1 ;;
    esac
    security find-identity -v -p codesigning | grep -qF "\"$RECTO_SIGN_IDENTITY\"" \
        || { echo "the identity isn't in the keychain: $RECTO_SIGN_IDENTITY" >&2; exit 1; }
else
    # A local check: ad hoc, whatever the environment says.
    unset RECTO_SIGN_IDENTITY
fi

VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p;}' "$REPO/Cargo.toml" | head -1)"
TAG="v$VERSION"

# Compiled in: this is what makes it a release, and it can't be turned on or off afterwards.
export RECTO_RELEASE=1
APP="$APP_DIR/build/RectoPDF.app"
OUT="$APP_DIR/build/release"
ZIP="RectoPDF-$VERSION.zip"
WORK="$(mktemp -d)"
cleanup() {
    if [ -n "${SMOKE:-}" ]; then RECTO_STATE_DIR="$SMOKE/s" "$APP/Contents/MacOS/recto" qa quit now >/dev/null 2>&1 || true; fi
    rm -rf "$WORK"
}
trap cleanup EXIT

team_of() { codesign -dv --verbose=2 "$1" 2>&1 | sed -n 's/^TeamIdentifier=//p'; }

check_built() {
    echo "==> Verify the app"
    codesign --verify --deep --strict "$APP"
    # The renderer is sandboxed, signed as it should be, and nothing could run in its place outside the sandbox.
    "$APP_DIR/render-helper.sh" --check "$APP"
    for f in "$APP/Contents/MacOS/"* "$APP/Contents/Helpers/RectoRender.app/Contents/MacOS/"* "$APP/Contents/Helpers/RectoRender.app/Contents/Frameworks/"*; do
        lipo -archs "$f" | grep -qx arm64 || { echo "unexpected architectures in $f: $(lipo -archs "$f")" >&2; exit 1; }
    done
    [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")" = "$VERSION" ] \
        || { echo "the app's version isn't $VERSION" >&2; exit 1; }
    cmp -s "$REPO/THIRD-PARTY-NOTICES.txt" "$APP/Contents/Resources/Licenses.txt" || { echo "the app's Licenses.txt isn't THIRD-PARTY-NOTICES.txt" >&2; exit 1; }
    # The real binaries, with every override set. Owner's bar: a test's way in can never reach a release.
    echo "==> Check a release reads no override"
    local answer
    answer="$(RECTO_UPDATE_REPO=http://127.0.0.1:1 RECTO_UPDATE_TRUST=any RECTO_UPDATER_DIE_AT=swapped "$APP/Contents/MacOS/recto-updater" source)"
    case "$answer" in
        *"\"mode\":\"release\""*"\"repo\":\"$REPO_URL\""*"\"release_build\":true"*"\"trust\":\"developer_id\""*"\"takes_test_hooks\":false"*) echo "    $answer" ;;
        *) echo "this build would use an override: $answer" >&2; exit 1 ;;
    esac
    # The app's own library too, not only the installer's: it's what reads a test's broken-release files.
    answer="$("$APP/Contents/MacOS/RectoPDF" --build-kind)"
    case "$answer" in
        *"\"release_build\":true"*"\"takes_test_hooks\":false"*) echo "    app: $answer" ;;
        *) echo "the app's library isn't a release build: $answer" >&2; exit 1 ;;
    esac
    # A test's broken-release file never ships, whatever a release build makes of it.
    if ls "$APP/Contents/Resources/" | grep -q '^test-'; then
        echo "a test's file is in the app's Resources: $(ls "$APP/Contents/Resources/" | grep '^test-' | tr '\n' ' ')" >&2
        exit 1
    fi
}
# The real bundle in a quiet run, which is the only thing that shows a renderer loading under the hardened runtime. It's
# what every copy would move to: a first frame on its own isn't enough. With the overrides set, the app has to say it's
# a release too, and refuse a test's clock.
run_it() {
echo "==> Run the app: it starts, opens a PDF and draws it"
SMOKE="$WORK/smoke"
rm -rf "$WORK/smoke"
mkdir -p "$SMOKE"; chmod 700 "$SMOKE"
cat > "$SMOKE/one.pdf" <<'PDF'
%PDF-1.4
1 0 obj
<< /Type /Catalog /Pages 2 0 R >>
endobj
2 0 obj
<< /Type /Pages /Kids [3 0 R] /Count 1 >>
endobj
3 0 obj
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>
endobj
4 0 obj
<< /Length 27 >>
stream
1 0 0 rg 72 72 400 400 re f
endstream
endobj
trailer
<< /Size 5 /Root 1 0 R >>
%%EOF
PDF
export RECTO_STATE_DIR="$SMOKE/s"
mkdir -p "$RECTO_STATE_DIR"; chmod 700 "$RECTO_STATE_DIR"
RECTO_UPDATE_REPO=http://127.0.0.1:1 RECTO_UPDATE_TRUST=any "$APP/Contents/MacOS/recto" qa start --app "$APP/Contents/MacOS/RectoPDF" --time-limit 2m >/dev/null
"$APP/Contents/MacOS/recto" open "$SMOKE/one.pdf" >/dev/null
"$APP/Contents/MacOS/recto" qa wait-settled --timeout 60s >/dev/null
STATE_JSON="$("$APP/Contents/MacOS/recto" qa state --json)"
printf '%s' "$STATE_JSON" | python3 -I -c '
import json, sys
state = json.load(sys.stdin)["data"]
pages = state["app"]["pages"]
assert state["reader"]["settled"] is True, "it never settled"
assert state["reader"]["pages"] == 1, "the PDF did not open"
assert pages["tile_layers"] > 0, "nothing was drawn: %r" % pages
# A tile in a layer came as an IOSurface over the Mach link: the app has no other way for one to arrive, so this is the tile path after the worker dropped its rights.
assert state["reader"]["tiles"].get("attached", 0) > 0, "no surface tile arrived: %r" % state["reader"]["tiles"]
# The release worker refuses to run without its walls, and its hello says it has them up.
assert state["memory"].get("workers_walled", 0) >= 1, "no live worker said it was sandboxed: %r" % state["memory"]
assert state["update"]["build"] == "release", "the app reads an override: %r" % state["update"]
' || { echo "the signed app didn't draw" >&2; exit 1; }
if "$APP/Contents/MacOS/recto" qa update-clock 1000 >/dev/null 2>&1; then echo "a release has a test's clock" >&2; exit 1; fi
"$APP/Contents/MacOS/recto" qa quit now >/dev/null
unset RECTO_STATE_DIR
}

# notarize <file>: submits it and waits for Apple's answer. Anything but Accepted fails with Apple's log.
notarize() {
    local answer status id
    answer="$(xcrun notarytool submit "$1" --keychain-profile "$RECTO_NOTARY_PROFILE" \
        ${RECTO_NOTARY_KEYCHAIN:+--keychain "$RECTO_NOTARY_KEYCHAIN"} --wait --timeout 45m --output-format json)" || true
    status="$(printf '%s' "$answer" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)"
    id="$(printf '%s' "$answer" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)"
    echo "    notarization of $(basename "$1"): ${status:-no answer} ${id:+(submission $id)}"
    if [ "$status" != Accepted ]; then
        [ -z "$id" ] || xcrun notarytool log "$id" --keychain-profile "$RECTO_NOTARY_PROFILE" \
            ${RECTO_NOTARY_KEYCHAIN:+--keychain "$RECTO_NOTARY_KEYCHAIN"} >&2 || true
        echo "notarization failed: ${answer:-no answer from notarytool}" >&2
        exit 1
    fi
}

# Signs with the Developer ID, notarizes and staples. Runs nothing of the app's.
sign_it() {
    "$APP_DIR/build.sh" --sign-only
    echo "==> Developer ID: team $(team_of "$APP")"
    [ "$(team_of "$APP")" = "$TEAM_ID" ] || { echo "the app isn't signed by team $TEAM_ID" >&2; exit 1; }
    [ "$(team_of "$APP/Contents/Helpers/RectoRender.app")" = "$TEAM_ID" ] || { echo "the renderer helper isn't signed by team $TEAM_ID" >&2; exit 1; }
    echo "==> Notarize and staple the app (waits for Apple)"
    ditto -c -k --sequesterRsrc --keepParent "$APP" "$WORK/notarize.zip"
    notarize "$WORK/notarize.zip"
    xcrun stapler staple "$APP"
    xcrun stapler validate "$APP"
    spctl --assess --type execute --verbose=2 "$APP"
}

# The zip, its checksums, latest.json and the notes. Runs nothing of the app's.
zip_it() {
    echo "==> Zip"
    rm -rf "$OUT"; mkdir -p "$OUT"
    # ditto keeps the bundle's permissions, signature and stapled ticket intact, which zip(1) can break.
    ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUT/$ZIP"
    (cd "$OUT" && shasum -a 256 "$ZIP" > SHA256SUMS)
    # latest.json: what the app's updater reads. Every value comes from this script.
    local zip_sha min_macos prev
    zip_sha="$(cd "$OUT" && shasum -a 256 "$ZIP" | cut -d' ' -f1)"
    min_macos="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$APP/Contents/Info.plist")"
    [ -n "$min_macos" ] || { echo "no LSMinimumSystemVersion in the app" >&2; exit 1; }
    printf '{"version":"%s","tag":"%s","minimum_macos":"%s","zip":"%s","sha256":"%s"}\n' \
        "$VERSION" "$TAG" "$min_macos" "$ZIP" "$zip_sha" > "$OUT/latest.json"
    python3 -I -c 'import json,sys; json.load(open(sys.argv[1]))' "$OUT/latest.json"
    # The notes: each commit since the tag before this one, by its subject.
    prev="$(git -C "$REPO" describe --tags --abbrev=0 "HEAD^" 2>/dev/null || true)"
    {
        echo "## Install"
        echo
        echo "macOS 15 or later, Apple silicon. Download [$ZIP]($REPO_URL/releases/download/$TAG/$ZIP), open it and drag RectoPDF to Applications."
        echo
        echo "RectoPDF updates itself from here on (RectoPDF menu, Settings)."
        echo
        echo "## Changes"
        echo
        if [ -n "$prev" ]; then git -C "$REPO" log --format='- %s' "$prev..HEAD" | cut -c1-200; else echo "- First release."; fi
        echo
        echo "## Checksums"
        echo
        echo '```'
        cat "$OUT/SHA256SUMS"
        echo '```'
    } > "$OUT/NOTES.md"
}

# The zip as the updater will take it: its own reader, not the system's.
check_zip() {
    echo "==> Check the zip as the updater will"
    local check="$WORK/check"
    mkdir -p "$check"
    (cd "$OUT" && shasum -a 256 -c SHA256SUMS >/dev/null)
    "$APP/Contents/MacOS/recto-updater" check-zip "$OUT/$ZIP" "$check"
    codesign --verify --deep --strict "$check/RectoPDF.app"
    "$APP_DIR/render-helper.sh" --check "$check/RectoPDF.app"
    [ -x "$check/RectoPDF.app/Contents/MacOS/recto-updater" ] || { echo "the zipped app has no installer, so it could never update again" >&2; exit 1; }
    cmp -s "$REPO/THIRD-PARTY-NOTICES.txt" "$check/RectoPDF.app/Contents/Resources/Licenses.txt" || { echo "the zipped app's Licenses.txt isn't THIRD-PARTY-NOTICES.txt" >&2; exit 1; }
    if [ "$1" = signed ]; then
        [ "$(team_of "$check/RectoPDF.app")" = "$TEAM_ID" ] || { echo "the zipped app lost its signature" >&2; exit 1; }
        xcrun stapler validate "$check/RectoPDF.app"
        spctl --assess --type execute "$check/RectoPDF.app"
    fi
}

if [ "$CHECK_SIGNED" = 1 ]; then
    # In the signing job, once the keys are gone: the stapled app runs and draws, and the zip is one the updater takes.
    [ "$(team_of "$APP")" = "$TEAM_ID" ] || { echo "the app isn't signed by team $TEAM_ID" >&2; exit 1; }
    xcrun stapler validate "$APP"
    "$APP_DIR/render-helper.sh" --check "$APP"
    run_it
    check_zip signed
elif [ "$ALREADY_BUILT" = 1 ]; then
    [ "$SIGNED" = 1 ] || { echo "--already-built goes with --signed" >&2; exit 2; }
    [ -x "$APP/Contents/MacOS/recto-updater" ] || { echo "there's no built app to sign" >&2; exit 1; }
    sign_it
    zip_it
    echo "==> Signed and zipped in $OUT. Nothing of the app has run here: --check-signed does that, once the keys are gone."
    exit 0
else
    # Built with no signing identity about, whatever the environment says: signing comes after.
    (unset RECTO_SIGN_IDENTITY; "$APP_DIR/build.sh" --release)
    check_built
    run_it
    if [ "$BUILT_ONLY" = 1 ]; then
        echo "==> Built, checked and run, not signed: --signed --already-built does the rest."
        exit 0
    fi
    if [ "$SIGNED" = 1 ]; then sign_it; run_it; fi
    zip_it
    if [ "$SIGNED" = 1 ]; then check_zip signed; else check_zip adhoc; fi
fi

echo "==> Assets in $OUT"
ls -l "$OUT" | tail -n +2 | awk '{printf "    %-28s %s\n", $NF, $5}'
echo "    A release is made only by the release workflow (tag vX.Y.Z): docs/RELEASING.md."
