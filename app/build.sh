#!/usr/bin/env bash
# One command from a clean checkout: Rust core -> Swift bindings -> Swift app -> app/build/RectoPDF.app, ad-hoc signed.
#
#   app/build.sh            A debug build, for QA while working.
#   app/build.sh --release  The build every budget is measured on: fat LTO, and it takes minutes.
#
# Run it through tools/sysguard/heavy.sh.
set -euo pipefail

APP_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$APP_DIR/.." && pwd)"
mode="${1:---debug}"
case "$mode" in
    --debug) CARGO_ARGS=(); RUST_OUT="$REPO/target/debug"; SWIFT_ARGS=(-c debug) ;;
    --release) CARGO_ARGS=(--release); RUST_OUT="$REPO/target/release"; SWIFT_ARGS=(-c release) ;;
    *) sed -n '2,7p' "$0"; exit 2 ;;
esac
BUILD="$APP_DIR/build"
APP="$BUILD/RectoPDF.app"

"$REPO/tools/pdfium.sh" >/dev/null

echo "==> Rust ($mode)"
(cd "$REPO" && cargo build --locked -p recto-ffi -p recto-cli -p recto-render-service ${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"})
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
# Worker and library beside the app, where `ServiceConfig::discover` looks. Each is what `tools/pdfium.sh` checked.
cp "$RUST_OUT/recto" "$APP/Contents/MacOS/recto"
cp "$RUST_OUT/recto-render-service" "$APP/Contents/MacOS/recto-render-service"
cp "$REPO/target/pdfium/libpdfium.dylib" "$APP/Contents/MacOS/libpdfium.dylib"
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
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
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
codesign --force --sign - "$APP/Contents/MacOS/libpdfium.dylib"
codesign --force --sign - "$APP/Contents/MacOS/recto-render-service"
codesign --force --sign - "$APP/Contents/MacOS/recto"
codesign --force --sign - "$APP"
echo "==> built $APP"
