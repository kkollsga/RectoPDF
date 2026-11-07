#!/usr/bin/env bash
# The Mac's release assets: full-LTO build, signed app, zip, SHA256SUMS, latest.json and the release notes. Everything
# lands in app/build/release/, and nothing is published.
#
#   app/release-zip.sh [--allow-dirty]            A local check: signed ad hoc, not notarized, not distributable.
#   app/release-zip.sh --signed [--allow-dirty]   The release: Developer ID signed and notarized. Builds, then signs.
#   app/release-zip.sh --built-only               Builds a release, to be signed by what comes next, with no keys about.
#   app/release-zip.sh --signed --already-built   Signs what --built-only built. It's what release.yml does, so no dependency's build script runs anywhere near a signing key.
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
ALLOW_DIRTY=0; SIGNED=0; BUILT_ONLY=0; ALREADY_BUILT=0
for a in "$@"; do
    case "$a" in
        --allow-dirty) ALLOW_DIRTY=1 ;;
        --signed) SIGNED=1 ;;
        --built-only) BUILT_ONLY=1 ;;
        --already-built) ALREADY_BUILT=1 ;;
        -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
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
echo "==> RectoPDF $VERSION ($TAG), commit $(git -C "$REPO" rev-parse --short HEAD)"

# Compiled in: this is what makes it a release, and it can't be turned on or off afterwards.
export RECTO_RELEASE=1
APP="$APP_DIR/build/RectoPDF.app"
if [ "$ALREADY_BUILT" = 1 ]; then
    [ -x "$APP/Contents/MacOS/recto-updater" ] || { echo "there's no built app to sign" >&2; exit 1; }
    # Built here by --built-only, from this very commit, so the bundle says the version it's about to be released as.
    "$APP_DIR/build.sh" --sign-only
else
    if [ "$BUILT_ONLY" = 1 ]; then unset RECTO_SIGN_IDENTITY; fi
    "$APP_DIR/build.sh" --release
fi
if [ "$BUILT_ONLY" = 1 ]; then
    echo "==> Built, not signed: app/release-zip.sh --signed --already-built does the rest."
    exit 0
fi

OUT="$APP_DIR/build/release"
ZIP="RectoPDF-$VERSION.zip"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
rm -rf "$OUT"; mkdir -p "$OUT"

echo "==> Verify the app"
codesign --verify --deep --strict "$APP"
for f in "$APP/Contents/MacOS/"*; do
    lipo -archs "$f" | grep -qx arm64 || { echo "unexpected architectures in $f: $(lipo -archs "$f")" >&2; exit 1; }
done
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")" = "$VERSION" ] \
    || { echo "the app's version isn't $VERSION" >&2; exit 1; }

# The real binaries, with every override set. Owner's bar: a test's way in can never reach a release.
echo "==> Check a release reads no override"
answer="$(RECTO_UPDATE_REPO=http://127.0.0.1:1 RECTO_UPDATE_TRUST=any RECTO_UPDATER_DIE_AT=swapped "$APP/Contents/MacOS/recto-updater" source)"
case "$answer" in
    *"\"mode\":\"release\""*"\"repo\":\"$REPO_URL\""*"\"release_build\":true"*"\"trust\":\"developer_id\""*"\"takes_test_hooks\":false"*) echo "    $answer" ;;
    *) echo "this build would use an override: $answer" >&2; exit 1 ;;
esac

# The real bundle, as signed, in a quiet run, which is the only thing that shows a renderer loading under the hardened
# runtime. It's what every copy would move to: a first frame on its own isn't enough. With the overrides set, the app
# has to say it's a release too, and refuse a test's clock.
echo "==> Run the signed app: it starts, opens a PDF and draws it"
SMOKE="$(mktemp -d "${TMPDIR:-/tmp}/recto-release-XXXXXX")"
chmod 700 "$SMOKE"
smoke_cleanup() { RECTO_STATE_DIR="$SMOKE/s" "$APP/Contents/MacOS/recto" qa quit now >/dev/null 2>&1 || true; rm -rf "$SMOKE"; }
trap 'smoke_cleanup; rm -rf "$WORK"' EXIT
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
assert state["memory"]["worker_bytes"] > 0 or state["reader"]["tiles"].get("attached", 0) > 0, "no worker drew it"
assert state["update"]["build"] == "release", "the app reads an override: %r" % state["update"]
' || { echo "the signed app didn't draw" >&2; exit 1; }
if "$APP/Contents/MacOS/recto" qa update-clock 1000 >/dev/null 2>&1; then echo "a release has a test's clock" >&2; exit 1; fi
"$APP/Contents/MacOS/recto" qa quit now >/dev/null
unset RECTO_STATE_DIR

team_of() { codesign -dv --verbose=2 "$1" 2>&1 | sed -n 's/^TeamIdentifier=//p'; }

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

if [ "$SIGNED" = 1 ]; then
    echo "==> Developer ID: team $(team_of "$APP")"
    [ "$(team_of "$APP")" = "$TEAM_ID" ] || { echo "the app isn't signed by team $TEAM_ID" >&2; exit 1; }
    echo "==> Notarize and staple the app (waits for Apple)"
    ditto -c -k --sequesterRsrc --keepParent "$APP" "$WORK/notarize.zip"
    notarize "$WORK/notarize.zip"
    xcrun stapler staple "$APP"
    xcrun stapler validate "$APP"
    spctl --assess --type execute --verbose=2 "$APP"
fi

echo "==> Zip"
# ditto keeps the bundle's permissions, signature and stapled ticket intact, which zip(1) can break.
ditto -c -k --sequesterRsrc --keepParent "$APP" "$OUT/$ZIP"
(cd "$OUT" && shasum -a 256 "$ZIP" > SHA256SUMS)
# latest.json: what the app's updater reads. Every value comes from this script.
ZIP_SHA="$(cd "$OUT" && shasum -a 256 "$ZIP" | cut -d' ' -f1)"
MIN_MACOS="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$APP/Contents/Info.plist")"
[ -n "$MIN_MACOS" ] || { echo "no LSMinimumSystemVersion in the app" >&2; exit 1; }
printf '{"version":"%s","tag":"%s","minimum_macos":"%s","zip":"%s","sha256":"%s"}\n' \
    "$VERSION" "$TAG" "$MIN_MACOS" "$ZIP" "$ZIP_SHA" > "$OUT/latest.json"

echo "==> Check the zip as the updater will: its own reader, not the system's"
CHECK="$WORK/check"; mkdir -p "$CHECK"
(cd "$OUT" && shasum -a 256 -c SHA256SUMS >/dev/null)
"$APP/Contents/MacOS/recto-updater" check-zip "$OUT/$ZIP" "$CHECK"
codesign --verify --deep --strict "$CHECK/RectoPDF.app"
[ -x "$CHECK/RectoPDF.app/Contents/MacOS/recto-updater" ] || { echo "the zipped app has no installer, so it could never update again" >&2; exit 1; }
if [ "$SIGNED" = 1 ]; then
    [ "$(team_of "$CHECK/RectoPDF.app")" = "$TEAM_ID" ] || { echo "the zipped app lost its signature" >&2; exit 1; }
    xcrun stapler validate "$CHECK/RectoPDF.app"
    spctl --assess --type execute "$CHECK/RectoPDF.app"
fi
python3 -I -c 'import json,sys; json.load(open(sys.argv[1]))' "$OUT/latest.json"

# The notes: each commit since the tag before this one, by its subject.
PREV="$(git -C "$REPO" describe --tags --abbrev=0 "HEAD^" 2>/dev/null || true)"
{
    echo "## Install"
    echo
    echo "macOS 15 or later, Apple silicon. Download [$ZIP]($REPO_URL/releases/download/$TAG/$ZIP), open it and drag RectoPDF to Applications."
    echo
    echo "RectoPDF updates itself from here on (RectoPDF menu, Settings)."
    echo
    echo "## Changes"
    echo
    if [ -n "$PREV" ]; then git -C "$REPO" log --format='- %s' "$PREV..HEAD" | cut -c1-200; else echo "- First release."; fi
    echo
    echo "## Checksums"
    echo
    echo '```'
    cat "$OUT/SHA256SUMS"
    echo '```'
} > "$OUT/NOTES.md"

echo "==> Assets in $OUT"
ls -l "$OUT" | tail -n +2 | awk '{printf "    %-28s %s\n", $NF, $5}'
echo "    A release is made only by the release workflow (tag vX.Y.Z): docs/RELEASING.md."
