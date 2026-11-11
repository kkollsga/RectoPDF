#!/usr/bin/env bash
# The app icon, built from its source (app/icon/icons.swift) into <out-dir>:
#
#   app/icon/build.sh <out-dir>
#
#   AppIcon.icns   every size a bundle needs, 16 to 512 at 1x and 2x, through iconutil. CFBundleIconFile names it: what macOS 15 shows,
#                  and what any Mac falls back to.
#   Assets.car     the Icon Composer version through actool, when Xcode's actool is there, for macOS 26's dark and tinted icons.
#                  CFBundleIconName names it. Without actool there's no Assets.car, and the build says so: the app still has its icon.
#
# Nothing generated is committed: a Mac with Swift has iconutil, and CI's macOS runners have Xcode. The outputs are kept with a stamp
# of the sources, so a build where the icon hasn't changed spends nothing on it.
set -euo pipefail

[ $# -eq 1 ] || { sed -n '2,4p' "$0"; exit 2; }
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$1"
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
has_actool() { xcrun --find actool >/dev/null 2>&1; }
# What decides the outputs: the source, this script, and whether actool is there to make Assets.car.
stamp="$(cat "$HERE/icons.swift" "$0" | shasum | cut -d' ' -f1) actool:$(has_actool && echo yes || echo no)"
if [ -f "$OUT/AppIcon.icns" ] && [ "$(cat "$OUT/stamp" 2>/dev/null)" = "$stamp" ]; then
    exit 0
fi
rm -f "$OUT/stamp" "$OUT/AppIcon.icns" "$OUT/Assets.car"

swift "$HERE/icons.swift" "$OUT"
iconutil -c icns "$OUT/AppIcon.iconset" -o "$OUT/AppIcon.icns"
if has_actool; then
    rm -rf "$OUT/actool" && mkdir -p "$OUT/actool"
    # actool writes its own AppIcon.icns too; the bundle takes iconutil's, whose 16 and 32 px are drawn for the pixel grid.
    xcrun actool "$OUT/AppIcon.icon" --compile "$OUT/actool" --platform macosx --minimum-deployment-target 15.0 --app-icon AppIcon \
        --output-partial-info-plist "$OUT/actool/partial.plist" --errors --warnings >/dev/null
    mv "$OUT/actool/Assets.car" "$OUT/Assets.car"
    rm -rf "$OUT/actool"
else
    echo "icon: Xcode's actool isn't here, so there's no Assets.car: the app gets AppIcon.icns only, without macOS 26's dark and tinted icons." >&2
fi
echo "$stamp" > "$OUT/stamp"
