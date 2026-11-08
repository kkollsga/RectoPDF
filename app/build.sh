#!/usr/bin/env bash
# One command from a clean checkout: Rust core -> Swift bindings -> Swift app -> app/build/RectoPDF.app, ad-hoc signed.
#
#   app/build.sh            A debug build, for QA while working.
#   app/build.sh --release  The build every budget is measured on: fat LTO, and it takes minutes.
#   app/build.sh --sign-only  Signs an app that's been built, with RECTO_SIGN_IDENTITY. The only mode that reads it: the others sign ad hoc. A release builds before there's a signing key anywhere on the machine, so no dependency's build script can reach one.
#
# Run it through tools/sysguard/heavy.sh.
set -euo pipefail

APP_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$APP_DIR/.." && pwd)"
mode="${1:---debug}"
case "$mode" in
    --sign-only) ;;
    --debug) CARGO_ARGS=(); RUST_OUT="$REPO/target/debug"; SWIFT_ARGS=(-c debug) ;;
    --release) CARGO_ARGS=(--release); RUST_OUT="$REPO/target/release"; SWIFT_ARGS=(-c release) ;;
    *) sed -n '2,8p' "$0"; exit 2 ;;
esac
BUILD="$APP_DIR/build"
APP="$BUILD/RectoPDF.app"
# One version, from one place. It's what the updater compares with, and it's in every copy's Info.plist.
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p;}' "$REPO/Cargo.toml" | head -1)"
# The identity a release is signed with, read only by --sign-only, which `app/release-zip.sh --signed` runs. Every build is ad hoc, whatever the shell has set, so a build or a
# test never signs with the owner's identity or raises a keychain prompt.
if [ "$mode" = --sign-only ]; then SIGN_IDENTITY="${RECTO_SIGN_IDENTITY:--}"; else SIGN_IDENTITY=-; fi
export RECTO_SIGN_IDENTITY="$SIGN_IDENTITY"
[ "$SIGN_IDENTITY" != - ] || unset RECTO_SIGN_IDENTITY
# By its SHA-1, as codesign can't find a name with a letter outside ASCII in it (render-helper.sh, `key`).
if [ "$SIGN_IDENTITY" = - ]; then SIGN_ARGS=(--force --sign -); else
    SIGN_KEY="$("$APP_DIR/render-helper.sh" --key "$SIGN_IDENTITY")"
    SIGN_ARGS=(--force --sign "$SIGN_KEY" --options runtime --timestamp)
fi

# From the inside out, and never --deep, which would sign the renderer helper again without its sandbox.
sign_app() {
    "$APP_DIR/render-helper.sh" --sign "$APP/Contents"
    codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/recto-indexer"
    codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/recto-headings"
    codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/recto-updater"
    codesign "${SIGN_ARGS[@]}" "$APP/Contents/MacOS/recto"
    codesign "${SIGN_ARGS[@]}" "$APP"
}

if [ "$mode" = --sign-only ]; then
    sign_app
    "$APP_DIR/render-helper.sh" --check "$APP"
    echo "==> signed $APP"
    exit 0
fi

"$REPO/tools/pdfium.sh" >/dev/null

echo "==> Rust ($mode)"
(cd "$REPO" && cargo build --locked -p recto-ffi -p recto-cli -p recto-render-service -p recto-indexer -p recto-update ${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"})
# The headings helper is a workspace of its own, so distillPDF's lopdf features stay out of the app: built apart, into the same folder.
(cd "$REPO" && cargo build --locked --manifest-path crates/recto-headings/Cargo.toml --target-dir "${CARGO_TARGET_DIR:-$REPO/target}" --bin recto-headings ${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"})
mkdir -p "$BUILD/rust"
cp "$RUST_OUT/librecto_ffi.a" "$BUILD/rust/librecto_ffi.a"

echo "==> Swift bindings"
GEN="$BUILD/generated"
rm -rf "$GEN" && mkdir -p "$GEN"
(cd "$REPO" && cargo run --locked -q -p recto-ffi --features bindgen --bin uniffi-bindgen -- \
    generate --library "$RUST_OUT/librecto_ffi.a" --language swift --out-dir "$GEN")
mkdir -p "$APP_DIR/Sources/recto_ffiFFI/include" "$APP_DIR/Sources/RectoPDF/Generated"
cp "$GEN/recto_ffiFFI.h" "$APP_DIR/Sources/recto_ffiFFI/include/recto_ffiFFI.h"
cp "$GEN/recto_ffiFFI.modulemap" "$APP_DIR/Sources/recto_ffiFFI/include/module.modulemap"
cp "$GEN/recto_ffi.swift" "$APP_DIR/Sources/RectoPDF/Generated/recto_ffi.swift"

echo "==> Swift app (${SWIFT_ARGS[*]})"
(cd "$APP_DIR" && swift build "${SWIFT_ARGS[@]}" --product RectoPDF)
BIN="$(cd "$APP_DIR" && swift build "${SWIFT_ARGS[@]}" --show-bin-path)/RectoPDF"

echo "==> $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
cp "$BIN" "$APP/Contents/MacOS/RectoPDF"
cp "$RUST_OUT/recto" "$APP/Contents/MacOS/recto"
cp "$RUST_OUT/recto-indexer" "$APP/Contents/MacOS/recto-indexer"
cp "$RUST_OUT/recto-headings" "$APP/Contents/MacOS/recto-headings"
cp "$RUST_OUT/recto-updater" "$APP/Contents/MacOS/recto-updater"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>io.github.kkollsga.RectoPDF</string>
    <key>CFBundleName</key><string>RectoPDF</string>
    <key>CFBundleDisplayName</key><string>RectoPDF</string>
    <key>CFBundleExecutable</key><string>RectoPDF</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>15.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <!-- Memory first: give freed pages back rather than keeping them in per-CPU magazines. -->
    <key>LSEnvironment</key>
    <dict>
        <key>MallocSpaceEfficient</key><string>1</string>
        <key>MallocMaxMagazines</key><string>1</string>
    </dict>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key><string>PDF document</string>
            <key>CFBundleTypeRole</key><string>Viewer</string>
            <key>LSItemContentTypes</key><array><string>com.adobe.pdf</string></array>
            <key>LSHandlerRank</key><string>Alternate</string>
        </dict>
    </array>
</dict>
</plist>
PLIST
# The renderer: the worker and the library `tools/pdfium.sh` checked, in their sandboxed helper, where `ServiceConfig::discover` looks first. Signed by sign_app below.
"$APP_DIR/render-helper.sh" --layout "$APP/Contents" "$RUST_OUT/recto-render-service" "$REPO/target/pdfium/libpdfium.dylib"
sign_app
"$APP_DIR/render-helper.sh" --check "$APP"
echo "==> built $APP"
