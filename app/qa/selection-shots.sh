#!/usr/bin/env bash
# Real screenshots of selected text, drawn inside the app in a quiet run, on every paper. Nothing reaches a screen, and nothing is posted: `qa mouse-down` and its like go through what a real press calls.
#
#   app/qa/selection-shots.sh OUT_DIR TWO_COLUMN.pdf [PAGE]
#
# OUT_DIR gets columns-*, a drag from one column into the next, boundary-*, a drag over a page break, and word-*, line-*, a double and a triple click, each on white, warm and dark paper. PAGE is a page with two columns of text that has a page after it. 2 if left out.
set -euo pipefail
out="$1"; pdf="$2"; page="${3:-2}"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap 'rm -rf "${root:?}"' EXIT
# A quiet run this starts ends with this script, however it ends.
export RECTO_QA_OWNER_PID=$$
export RECTO_STATE_DIR="$root/s"
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
shot() { rm -f "$out/$1.png"; r qa shot --out "$out/$1.png"; }
# Where things are on a page, as `recto select` says, in the form `qa mouse-*` takes. Which rectangle, what share of the way along it.
place() { # PAGE RECT SHARE
    "$recto" select "$1" --json | /usr/bin/python3 -c "
import json,sys
rects=json.load(sys.stdin)['data']['pages'][0]['rects']
wanted=$2
rect=rects[wanted if wanted>=0 else len(rects)+wanted]
print('$1:%.2f,%.2f' % (rect[0]+rect[2]*$3, rect[1]+rect[3]/2))"
}
# The rectangle starting furthest right among those on a page's first half, which is a line of the second column.
right_column_rect() { # PAGE
    "$recto" select "$1" --json | /usr/bin/python3 -c "
import json,sys
rects=json.load(sys.stdin)['data']['pages'][0]['rects']
middle=json.load(sys.stdin) if False else None
wide=[(i,r) for i,r in enumerate(rects) if r[2]>150]
columns=[i for i,r in wide if r[0]>250]
print(columns[len(columns)//3])"
}
left_column_rect() { # PAGE
    "$recto" select "$1" --json | /usr/bin/python3 -c "
import json,sys
rects=json.load(sys.stdin)['data']['pages'][0]['rects']
wide=[(i,r) for i,r in enumerate(rects) if r[2]>150 and r[0]<250]
print(wide[2*len(wide)//3][0])"
}

r qa start --app "$app" --time-limit 5m
r qa scroller-style overlay
r open "$pdf"
settle
# Fits the page in the window, so the whole of what's selected is in the picture.
r qa resize 1440x1100
r goto "$page"
settle
right=$(right_column_rect "$page"); left=$(left_column_rect "$page")
from=$(place "$page" "$left" 0.3); to=$(place "$page" "$right" 0.6)
last_line=$(place "$page" -1 0.4); next_first=$(place "$((page + 1))" 2 0.5)
word=$(place "$page" 4 0.5)

for spec in "light white" "light warm" "dark dark" "dark white" "light dark"; do
    look="${spec% *}"; paper="${spec#* }"
    name="$paper"; [ "$look" = dark ] && [ "$paper" = white ] && name=white-in-dark-window
    [ "$look" = light ] && [ "$paper" = dark ] && name=dark-in-light-window
    r qa appearance "$look"
    r view --paper "$paper"
    r goto "$page"
    settle
    r qa mouse-down "$from"
    r qa mouse-drag "$to"
    r qa mouse-up
    settle
    shot "columns-$name"
    r qa mouse-down "$word:2"
    r qa mouse-up
    settle
    shot "word-$name"
    r qa mouse-down "$word:3"
    r qa mouse-up
    settle
    shot "line-$name"
    # Over the break: the bottom of this page and the top of the next, with the view on the join.
    r qa zoom 0.5
    settle
    r qa mouse-down "$last_line"
    r qa mouse-drag "$next_first"
    r qa mouse-up
    settle
    shot "boundary-$name"
    r qa zoom 1
    settle
done
r stop
echo "shots are in $out"
