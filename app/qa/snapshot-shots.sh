#!/usr/bin/env bash
# Real screenshots of the snapshot tool, drawn inside the app in a quiet run, light and dark. Nothing reaches a screen, and nothing is posted: `qa mouse-down` and its like go through what a real press calls.
#
#   app/qa/snapshot-shots.sh OUT_DIR FILE.pdf [PAGE] [FROM] [TO] [IMAGE]
#
# OUT_DIR gets box-*, the box being dragged, copied-*, the notice once it's on the clipboard, and image-*, the notice after a click on an image, each light and dark. FROM and TO are where the drag goes, and IMAGE where the click is, as `X,Y` in points on the page, as `recto snapshot --rect` and `recto images` give them.
set -euo pipefail
out="$1"; pdf="$2"; page="${3:-1}"; from="${4:-40,20}"; to="${5:-560,150}"; image="${6:-500,70}"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap '"$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
# A quiet run this starts ends with this script, however it ends.
export RECTO_QA_OWNER_PID=$$
export RECTO_STATE_DIR="$root/s"
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
shot() { rm -f "$out/$1.png"; r qa shot --out "$out/$1.png"; }

r qa start --app "$app" --time-limit 5m
r qa scroller-style overlay
r open "$pdf"
settle
r qa resize 1440x1000
r goto "$page"
settle
r qa menu tools.snapshot
r qa panel toggle
for look in light dark; do
    r qa appearance "$look"
    settle
    r qa mouse-down "$page:$from"
    r qa mouse-drag "$page:$to"
    settle
    shot "box-$look"
    r qa mouse-up
    settle
    shot "copied-$look"
    r qa mouse-down "$page:$image"
    r qa mouse-up
    settle
    shot "image-$look"
done
"$recto" qa copied --json | /usr/bin/python3 -c "import json,sys; d=json.load(sys.stdin)['data']; print('clipboard:', [t['type'] for t in d['types'] if t['type'].startswith('public.')])"
echo "shots in $out"
