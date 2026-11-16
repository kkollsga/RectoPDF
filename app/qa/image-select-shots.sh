#!/usr/bin/env bash
# Real screenshots of an image picked with a click in read mode, drawn inside the app in a quiet run, on every paper, light and dark. Nothing reaches a
# screen, and nothing is posted: `qa mouse-down` and `qa mouse-up` go through what a real press and release call.
#
#   app/qa/image-select-shots.sh OUT_DIR FILE.pdf [PAGE] [IMAGE]
#
# OUT_DIR gets picked-*, the image picked, its box drawn, on white, warm and dark paper in a light and a dark window, and copied-*, the notice after ⌘C,
# with what the clipboard holds printed. IMAGE is where the click goes, as `X,Y` in points on the page, as `recto images` gives its boxes; the middle of
# the page's first image if left out.
set -euo pipefail
out="$1"; pdf="$2"; page="${3:-1}"; image="${4:-}"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap 'quiet_files_out 2>/dev/null || true; "$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
. "$(dirname "$0")/quiet-files.sh"
# A quiet run this starts ends with this script, however it ends.
export RECTO_QA_OWNER_PID=$$
export RECTO_STATE_DIR="$root/s"
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
shot() { rm -f "$out/$1.png"; r qa shot --out "$out/$1.png"; }

r qa start --app "$app" --time-limit 5m
r qa scroller-style overlay
r open "$(inside "$pdf")"
settle
r qa resize 1440x1100
r goto "$page"
settle
if [ -z "$image" ]; then
    image="$("$recto" images "$page" --json | /usr/bin/python3 -c "
import json,sys
rect=json.load(sys.stdin)['data']['images'][0]['rect']
print('%.2f,%.2f' % (rect[0]+rect[2]/2, rect[1]+rect[3]/2))")"
fi
for spec in "light white" "light warm" "dark dark" "dark white" "light dark"; do
    look="${spec% *}"; paper="${spec#* }"
    name="$look-$paper"
    r qa appearance "$look"
    r view --paper "$paper"
    settle
    r qa mouse-down "$page:$image"
    r qa mouse-up
    settle
    shot "picked-$name"
done
r qa menu edit.copy
settle
shot "copied"
"$recto" qa state --json | /usr/bin/python3 -c "import json,sys; d=json.load(sys.stdin)['data']; print('picked:', d['reader']['selection']['image']['picked']); print('notice:', d['snapshot']['notice'])"
"$recto" qa copied --json | /usr/bin/python3 -c "import json,sys; d=json.load(sys.stdin)['data']; print('clipboard:', [t['type'] for t in d['types'] if t['type'].startswith('public.')])"
echo "shots in $out"
