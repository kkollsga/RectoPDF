#!/usr/bin/env bash
# Real screenshots of a page zoomed in, drawn inside the app in a quiet run, light and dark: at the fit, zoomed wider than the room the fit keeps clear
# but no wider than the window (centred, equal margins, nothing to scroll across: owner, 2026-11-16, "The page should always be centered (if there is
# no scrollbar)"), and zoomed wider than the window (from the left edge, the thumb across showing); and two side by side at the middle zoom. Nothing
# reaches a screen, and nothing is posted.
#
#   app/qa/zoom-shots.sh OUT_DIR FILE.pdf [MIDDLE] [WIDE] [TWO_UP]
#
# MIDDLE, WIDE and TWO_UP are magnifications, where 1 is the fit: 1.3, 2.0 and 1.1 by default, for a letter page in a 1440 by 1000 window with the
# sidebar showing. Each shot's page gutters, the document's width and the view's are printed beside it, so the rule can be read off.
set -euo pipefail
out="$1"; pdf="$2"; middle="${3:-1.3}"; wide="${4:-2.0}"; two_up="${5:-1.1}"
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
across() {
    "$recto" qa state --json | /usr/bin/python3 -I -c "
import json,sys
pages=json.load(sys.stdin)['data']['app']['pages']
gutters=', '.join('page %d: %.1f | %.1f' % (g['page'] + 1, g['left'], g['right']) for g in pages['page_gutters'])
print('%s  document %.1f, view %.1f, across %.1f, thumb across %s; %s' % (sys.argv[1], pages['document_width'], pages['view_width'], pages['scroll_x'], pages['thumb_across']['shown'], gutters))" "$1"
}
shot() { rm -f "$out/$1.png"; r qa shot --out "$out/$1.png"; across "$1"; }

r qa start --app "$app" --time-limit 5m
r qa scroller-style overlay
r open "$(inside "$pdf")"
settle
r qa resize 1440x1000
r goto 2
settle
for look in light dark; do
    r qa appearance "$look"
    r view --paper auto
    r view --layout single
    for spec in "fit 1" "middle $middle" "wide $wide"; do
        name="${spec% *}"; zoom="${spec#* }"
        r view --zoom "$zoom"
        settle
        shot "$name-$look"
    done
    r view --layout two-up
    r view --zoom "$two_up"
    settle
    shot "two-up-middle-$look"
done
echo "shots in $final_out"
