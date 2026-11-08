#!/usr/bin/env bash
# Real screenshots of opening, drawn inside the app in a quiet run, light and dark: the password sheet, a file replaced while the app was in the background, and File > Open Recent.
#
#   app/qa/opening-shots.sh OUT_DIR PLAIN.pdf
#
# OUT_DIR gets, each -light and -dark: password (the sheet a protected PDF asks with), wrong-password (after a wrong one), changed (the note after the file was replaced while the app was behind another), and menus (every menu side by side, as `qa shot-menus` draws them, File with Open Recent).
# PLAIN.pdf is copied; the protected copy is made with qpdf, with the user password `shots`.
set -euo pipefail
out="$1"; plain="$2"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap '"$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
export RECTO_STATE_DIR="$root/s"
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
"$recto" qa start --app "$app" --time-limit 10m >/dev/null
r qa scroller-style overlay
for look in light dark; do
    r qa appearance "$look"
    locked="$root/locked-$look.pdf"
    qpdf --encrypt shots owner 128 --use-aes=y -- "$plain" "$locked"
    r qa open-file "$locked"
    until_state "a['sheet'].get('kind') == 'password'"
    settle
    shot "password-$look"
    printf 'wrong' | r qa password
    until_state "'tries left' in a['sheet'].get('informative', '')"
    settle
    shot "wrong-password-$look"
    printf 'shots' | r qa password
    # The sheet goes while the password's tried, before the file opens: wait for its tab, or it opens after the next file and takes the front.
    until_state "a['sheet'] == {} and any(t['path'].endswith('locked-$look.pdf') for t in d['session']['tabs'])"
    copy="$root/report-$look.pdf"
    cp "$plain" "$copy"
    r open "$copy"
    settle
    r qa frontmost off
    qpdf --pages "$plain" 1 -- "$plain" "$copy.new" && mv "$copy.new" "$copy"
    r qa frontmost on
    until_state "a['note'] is not None and 'changed on disk' in a['note']"
    settle
    shot "changed-$look"
    rm -f "$out/menus-$look.png"
    r qa shot-menus --out "$out/menus-$look.png"
done
