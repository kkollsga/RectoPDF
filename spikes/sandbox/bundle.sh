#!/usr/bin/env bash
# Lays out every way of running the worker, each ad-hoc signed, in a folder of the caller's choice. Run after building the main workspace and this crate, in release.
#
#   spikes/sandbox/bundle.sh <out>
#
#   <out>/plain/       recto, the worker as built, libpdfium.dylib. Today's arrangement.
#   <out>/entitled/    Spike.app: the same worker, signed with `com.apple.security.app-sandbox`. recto sits beside it, unsandboxed, because a release `recto` looks there.
#   <out>/dropmach/   Spike.app again, with the worker variant that also lets go of its Mach send rights once PDFium has loaded.
#   <out>/selfbox/     recto, and the worker variant that calls sandbox_init after loading PDFium.
#   <out>/xpc/         A host app with the worker as an XPC service inside.
#
# Every sandboxed one uses the bundle ID no.rectospike.render, so no new container appears under ~/Library/Containers.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="$1"
id=no.rectospike.render
ent="$here/sandbox.entitlements"
built="$here/target/release"
main="$root/target/release"
lib="$root/target/pdfium/libpdfium.dylib"
rm -rf "$out/plain" "$out/entitled" "$out/dropmach" "$out/selfbox" "$out/xpc"

info() { # identifier executable package-type
  cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$1</string>
<key>CFBundleExecutable</key><string>$2</string>
<key>CFBundlePackageType</key><string>$3</string>
<key>CFBundleName</key><string>$2</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>1</string>
<key>LSBackgroundOnly</key><true/>
$4
</dict></plist>
PLIST
}

mkdir -p "$out/plain"
cp "$main/recto" "$main/recto-render-service" "$lib" "$out/plain/"

sandboxed_app() { # folder worker-binary
  local macos="$out/$1/Spike.app/Contents/MacOS"
  mkdir -p "$macos"
  cp "$main/recto" "$lib" "$macos/"
  cp "$2" "$macos/recto-render-service"
  info "$id" recto-render-service APPL "" > "$out/$1/Spike.app/Contents/Info.plist"
  codesign -f -s - "$macos/recto" 2>/dev/null
  codesign -f -s - "$macos/libpdfium.dylib" 2>/dev/null
  codesign -f -s - --entitlements "$ent" "$out/$1/Spike.app" 2>/dev/null
}
sandboxed_app entitled "$main/recto-render-service"
sandboxed_app dropmach "$built/worker-dropmach"

mkdir -p "$out/selfbox"
cp "$main/recto" "$lib" "$out/selfbox/"
cp "$built/selfbox-sbpl" "$out/selfbox/recto-render-service"

app="$out/xpc/RectoXpcHost.app/Contents"
service="$app/XPCServices/$id.xpc/Contents"
mkdir -p "$app/MacOS" "$service/MacOS" "$service/Frameworks"
cp "$built/xpc-host" "$app/MacOS/"
cp "$built/xpc-service" "$service/MacOS/"
cp "$lib" "$service/Frameworks/"
info no.rectospike.xpchost xpc-host APPL "" > "$app/Info.plist"
info "$id" xpc-service 'XPC!' "<key>XPCService</key><dict><key>ServiceType</key><string>Application</string></dict>" > "$service/Info.plist"
codesign -f -s - "$service/Frameworks/libpdfium.dylib" 2>/dev/null
codesign -f -s - --entitlements "$ent" "$app/XPCServices/$id.xpc" 2>/dev/null
codesign -f -s - "$out/xpc/RectoXpcHost.app" 2>/dev/null
