#!/usr/bin/env bash
# Real screenshots of links, Back and Forward and page labels, drawn inside the app in a quiet run, light and dark. Nothing reaches a screen, and nothing is posted: `qa pointer`, `qa mouse-down`, `qa button` and `qa sheet-answer` go through what the window's own pointer and controls call.
#
#   app/qa/links-shots.sh OUT_DIR
#
# OUT_DIR gets, each -light and -dark: hover (the pointer on a link to a page, its hint in the lower left and the label in the corner), followed (after clicking it, on page "5"),
# web-prompt (a web link clicked: the sheet with the real address) and go-to (the go-to field open over the page number). The PDF is recto_testpdf::linked_document.
set -euo pipefail
out="$1"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap '"$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
# A quiet run this starts ends with this script, however it ends.
export RECTO_QA_OWNER_PID=$$
export RECTO_STATE_DIR="$root/s"
"$here/tools/sysguard/heavy.sh" cargo run -q --locked -p recto-testpdf --example write_linked -- "$root/linked.pdf"
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
shot() { rm -f "$out/$1.png"; "$recto" qa shot --out "$out/$1.png" --json | grep -q '"settled":true' || echo "not settled: $1" >&2; }
until_state() {
    for _ in $(seq 1 300); do
        if "$recto" qa state --json | /usr/bin/python3 -I -c "import json,sys; d=json.load(sys.stdin)['data']; a=d['app']; sys.exit(0 if ($1) else 1)"; then return 0; fi
        sleep 0.1
    done
    echo "never held: $1" >&2; return 1
}
# The middle of the first link (to page 9) and the third (a web address), as LINKS_ON_PAGE_ONE has them.
page_link="1:186,120"; web_link="1:186,180"

"$recto" qa start --app "$app" --time-limit 10m >/dev/null
r qa scroller-style overlay
r open "$root/linked.pdf"
for look in light dark; do
    r qa appearance "$look"
    r goto 1
    settle
    r qa pointer "$page_link"; settle; r qa pointer "$page_link"
    until_state "a['pages']['cursor'] == 'link'"
    settle
    shot "hover-$look"
    r qa mouse-down "$page_link:1"; r qa mouse-up
    until_state "a['place_label'] == '5 (9 of 12)'"
    settle
    shot "followed-$look"
    r back
    until_state "a['place_label'] == 'i (1 of 12)'"
    settle
    r qa pointer "$web_link"; settle; r qa pointer "$web_link"
    until_state "a['pages']['cursor'] == 'link'"
    r qa mouse-down "$web_link:1"; r qa mouse-up
    until_state "a['sheet'].get('kind') == 'link_prompt'"
    settle
    shot "web-prompt-$look"
    r qa sheet-answer second
    r qa button page
    settle
    shot "go-to-$look"
    r qa page-escape
done
