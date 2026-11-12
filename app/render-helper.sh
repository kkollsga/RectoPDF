#!/usr/bin/env bash
# The renderer helper: PDFium's worker in a small app of its own inside RectoPDF.app, signed with the App Sandbox. One place for its layout, its signing and its check, so the
# app build, the release and the tests can't lay it out three ways. docs/engineering/sandbox.md says why it's built this way.
#
#   app/render-helper.sh --layout <Contents> <worker> <libpdfium>   Puts the helper in <Contents>/Helpers/RectoRender.app and signs it ad hoc, always.
#   app/render-helper.sh --sign <Contents>                          Signs it again: with RECTO_SIGN_IDENTITY if it's set, else ad hoc.
#   app/render-helper.sh --sign-dev <Contents>                      Signs it ad hoc, whatever's set, with a designated requirement of its identifier alone: a development build's,
#                                                                   so macOS takes each one as the same helper. Never the keychain (owner, 2026-11-11).
#   app/render-helper.sh --check <RectoPDF.app or RectoRender.app>  Fails unless the helper is sandboxed and signed as it should be, and nothing can run unsandboxed in its place.
#   app/render-helper.sh --key <identity>                           Prints the keychain's SHA-1 for a signing identity's name, or the SHA-1 itself, for codesign.
#
#   RectoPDF.app/Contents/
#     MacOS/RectoPDF, recto, recto-indexer, ...    Not sandboxed: the app has to reach rg and claude (SPEC decision 3). No worker or PDFium here.
#     Helpers/RectoRender.app/Contents/
#       Info.plist                                 CFBundleIdentifier, which names the sandbox's container. LSBackgroundOnly.
#       MacOS/recto-render-service                 Signed with RectoRender.entitlements, and the hardened runtime when signed by a team.
#       Frameworks/libpdfium.dylib                 Signed by the same team, so library validation passes. It arrives signed ad hoc.
#
# Signed from the inside out, never with --deep, which would sign the helper again without its entitlements.
#
# The identifier depends on the signature. A Developer ID build is io.github.kkollsga.RectoPDF.Render. One signed ad hoc, every development build and test, is
# io.github.kkollsga.RectoPDF.Render.adhoc, so the container a development build makes on a Mac is never the one a signed release then finds there: whether the system takes a
# container over from another signer, or asks the user first, isn't tested.
set -euo pipefail

APP_DIR="$(cd "$(dirname "$0")" && pwd)"
ENTITLEMENTS="$APP_DIR/RectoRender.entitlements"
HELPER_NAME=RectoRender.app
WORKER=recto-render-service
LIBRARY=libpdfium.dylib
RELEASE_ID=io.github.kkollsga.RectoPDF.Render
ADHOC_ID=$RELEASE_ID.adhoc

fail() { echo "render-helper: $*" >&2; exit 1; }

# key <identity>: the SHA-1 codesign should be given. codesign reads a name on its command line as Mac Roman, whatever the locale, so a name with a letter outside ASCII, such
# as the owner's "å", is never found ("no identity found", 2026-11-08). The name is matched here, byte for byte, against the keychain search list, the same list codesign and
# the release workflow search. Two valid certificates of one name are refused rather than one taken at random, and an expired or otherwise invalid one says so.
key() {
    local identity="$1" hashes count reason
    if [[ "$identity" =~ ^[0-9A-Fa-f]{40}$ ]]; then echo "$identity"; return; fi
    # `-v` lists valid identities only: `  1) <SHA-1> "<name>"`.
    hashes="$(security find-identity -v -p codesigning | awk -F'"' -v want="$identity" '$2 == want && $1 ~ /^ *[0-9]+\) [0-9A-F]{40} *$/ { split($1, f, " "); print f[2] }' | sort -u)"
    count="$(printf '%s' "$hashes" | grep -c . || true)"
    if [ "$count" -gt 1 ]; then
        fail "$count valid certificates are called \"$identity\" ($(echo $hashes)): remove the one that shouldn't sign from the keychain, or set RECTO_SIGN_IDENTITY to the SHA-1 of the one that should"
    fi
    if [ "$count" -eq 1 ]; then echo "$hashes"; return; fi
    # Without `-v`, invalid ones too, each with the reason: `(CSSMERR_TP_CERT_EXPIRED)`.
    reason="$(security find-identity -p codesigning | awk -F'"' -v want="$identity" '$2 == want { print $3 }' | head -1)"
    case "$reason" in
        *EXPIRED*) fail "the certificate \"$identity\" has expired: make a new Developer ID Application certificate (docs/RELEASING.md)" ;;
        *[A-Za-z]*) fail "the certificate \"$identity\" isn't valid for signing:$reason" ;;
    esac
    fail "the identity isn't in the keychain: $identity"
}

# sign <Contents> [stable]: writes the identifier this signature calls for, then signs the library and the helper. With `stable`, ad hoc with the identifier as the
# designated requirement, where ad hoc's own is the code's hash, which every build changes.
sign() {
    local helper="$1/Helpers/$HELPER_NAME" identity="${RECTO_SIGN_IDENTITY:--}" id args requirement=()
    [ "${2:-}" != stable ] || identity=-
    [ -x "$helper/Contents/MacOS/$WORKER" ] || fail "no helper to sign in $1"
    if [ "$identity" = - ]; then id=$ADHOC_ID; args=(--force --sign -); else id=$RELEASE_ID; args=(--force --sign "$(key "$identity")" --options runtime --timestamp); fi
    local version
    version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$1/Info.plist" 2>/dev/null || echo 1)"
    cat > "$helper/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>$id</string>
    <key>CFBundleName</key><string>RectoRender</string>
    <key>CFBundleExecutable</key><string>$WORKER</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>15.0</string>
    <key>LSBackgroundOnly</key><true/>
</dict>
</plist>
PLIST
    [ "${2:-}" != stable ] || requirement=(-r="designated => identifier \"$id\"")
    codesign "${args[@]}" "$helper/Contents/Frameworks/$LIBRARY"
    codesign "${args[@]}" "${requirement[@]+"${requirement[@]}"}" --entitlements "$ENTITLEMENTS" "$helper"
}

# What `codesign -d` says of a file: the value of one of its lines.
signature_field() { codesign -dv --verbose=2 "$1" 2>&1 | sed -n "s/^$2=//p" | head -1; }

entitlements_of() { codesign -d --entitlements - --xml "$1" 2>/dev/null || true; }

check() {
    local target="$1" helper app=""
    case "$target" in
        */"$HELPER_NAME"|*/"$HELPER_NAME"/) helper="${target%/}" ;;
        *.app|*.app/) app="${target%/}"; helper="$app/Contents/Helpers/$HELPER_NAME" ;;
        *) fail "--check takes RectoPDF.app or $HELPER_NAME: $target" ;;
    esac
    local worker="$helper/Contents/MacOS/$WORKER" library="$helper/Contents/Frameworks/$LIBRARY"
    [ -x "$worker" ] || fail "no worker at $worker"
    [ -f "$library" ] || fail "no $LIBRARY at $library"
    codesign --verify --deep --strict "$helper" || fail "the helper's signature doesn't verify: $helper"
    # Exactly the one entitlement: the sandbox on, and nothing that opens it up.
    entitlements_of "$worker" | python3 -I -c '
import plistlib, sys
data = sys.stdin.buffer.read()
got = plistlib.loads(data) if data.strip() else {}
want = {"com.apple.security.app-sandbox": True}
if got != want:
    sys.exit("render-helper: the worker must have exactly %r as its entitlements, and has %r" % (want, got))
'
    local id executable team library_team runtime
    id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$helper/Contents/Info.plist")"
    executable="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$helper/Contents/Info.plist")"
    [ "$executable" = "$WORKER" ] || fail "the helper's executable is $executable, not $WORKER"
    [ "$(/usr/libexec/PlistBuddy -c 'Print :LSBackgroundOnly' "$helper/Contents/Info.plist")" = true ] || fail "the helper isn't background only"
    team="$(signature_field "$worker" TeamIdentifier)"
    library_team="$(signature_field "$library" TeamIdentifier)"
    [ "$team" = "$library_team" ] || fail "the worker is signed by team '$team' and $LIBRARY by '$library_team': library validation would refuse it"
    if [ "$team" = "not set" ] || [ -z "$team" ]; then
        [ "$id" = "$ADHOC_ID" ] || fail "an ad hoc helper must be $ADHOC_ID, and is $id"
    else
        [ "$id" = "$RELEASE_ID" ] || fail "a team-signed helper must be $RELEASE_ID, and is $id"
        runtime="$(codesign -dv --verbose=2 "$worker" 2>&1 | sed -n 's/^CodeDirectory .*flags=\(0x[0-9a-f]*\).*/\1/p')"
        [ $(( ${runtime:-0} & 0x10000 )) -ne 0 ] || fail "a team-signed worker must run under the hardened runtime"
    fi
    if [ -n "$app" ]; then
        # Nothing the app could start in the helper's place: a worker or PDFium found beside the app's own programs would run unsandboxed.
        for stray in "$app/Contents/MacOS/$WORKER" "$app/Contents/MacOS/$LIBRARY"; do
            [ ! -e "$stray" ] || fail "$stray would run outside the sandbox: the worker and its library belong in the helper only"
        done
        # The app itself isn't sandboxed, and neither is anything else of it.
        local program entitlements
        for program in "$app" "$app/Contents/MacOS/"*; do
            entitlements="$(entitlements_of "$program")"
            case "$entitlements" in *com.apple.security.app-sandbox*) fail "$program is sandboxed: only the renderer helper may be (SPEC decision 3)" ;; esac
        done
    fi
    echo "render-helper: $helper is sandboxed and signed ($id, team ${team:-not set})"
}

case "${1:-}" in
    --layout)
        [ $# -eq 4 ] || fail "--layout <Contents> <worker> <libpdfium>"
        helper="$2/Helpers/$HELPER_NAME"
        rm -rf "$helper"
        mkdir -p "$helper/Contents/MacOS" "$helper/Contents/Frameworks"
        cp "$3" "$helper/Contents/MacOS/$WORKER"
        cp "$4" "$helper/Contents/Frameworks/$LIBRARY"
        chmod 755 "$helper/Contents/MacOS/$WORKER"
        (unset RECTO_SIGN_IDENTITY; sign "$2")
        ;;
    --sign)
        [ $# -eq 2 ] || fail "--sign <Contents>"
        sign "$2"
        ;;
    --sign-dev)
        [ $# -eq 2 ] || fail "--sign-dev <Contents>"
        sign "$2" stable
        ;;
    --check)
        [ $# -eq 2 ] || fail "--check <RectoPDF.app or $HELPER_NAME>"
        check "$2"
        ;;
    --key)
        [ $# -eq 2 ] || fail "--key <identity>"
        key "$2"
        ;;
    *) sed -n '2,11p' "$0"; exit 2 ;;
esac
