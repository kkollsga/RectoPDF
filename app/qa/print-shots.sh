#!/usr/bin/env bash
# Real screenshots of printing, drawn inside the app in a quiet run, light and dark: the File menu with Print…, and the line by the page while a long print's pages are prepared. A quiet run shows no sheet, so the system's print panel isn't in them.
#
#   app/qa/print-shots.sh OUT_DIR LONG.pdf [LAST_PAGE]
#
# OUT_DIR gets menus-*, every menu side by side, and preparing-*, taken while pages 1 to LAST_PAGE (200 if left out) are being prepared, which takes longer than a shot waits.
set -euo pipefail
out="$1"; pdf="$2"; last="${3:-200}"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap '"$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
export RECTO_STATE_DIR="$root/s"
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 120s; }

r qa start --app "$app" --time-limit 10m
r qa scroller-style overlay
r open "$pdf"
settle
r qa resize 1440x1000
for look in light dark; do
    r qa appearance "$look"
    settle
    rm -f "$out/menus-$look.png"; r qa shot-menus --out "$out/menus-$look.png"
    rm -f "$root/printed-$look.pdf"
    r qa print-to-pdf "$root/printed-$look.pdf|1|$last"
    # The shot waits up to 10 s for settled, and preparing these pages takes longer, so it shows how far it's got.
    rm -f "$out/preparing-$look.png"; r qa shot --out "$out/preparing-$look.png"
    settle
done
echo "shots in $out"
