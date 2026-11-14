#!/usr/bin/env bash
# The whole app at a glance: each feature once, in the real app, light and dark, laid out as one labelled contact sheet per look. Quiet runs only: nothing
# reaches a screen, nothing is posted (the `qa` verbs call what the window's own controls call), state is temp, and the run ends with the script.
#
#   app/qa/contact-sheet.sh OUT_DIR
#
# OUT_DIR gets sheet-light.png and sheet-dark.png, the shots they're made of in shots/, and notes.txt (a shot that didn't settle is said there).
# Needs the debug app (app/build.sh, through tools/sysguard/heavy.sh), qpdf, and the system's bash manuals. Not part of the gate: 37 s at load 6 to 14 (2026-11-14).
# A feature's own script shows it in depth (app/qa/*-shots.sh); this shows one state of each, so a change anywhere is seen everywhere.
set -uo pipefail
out="$1"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF}"
started=$(date +%s)
mkdir -p "$out/shots"
sheets="$out"
root="$(mktemp -d)"
# Every `recto` this script runs, the trap's included, goes to a state folder of its own under the temp root: never the user's own app or session.
export RECTO_STATE_DIR="$root/state-none"
trap 'quiet_files_out 2>/dev/null || true; "$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
out="$out/shots"
. "$(dirname "$0")/quiet-files.sh"
export RECTO_QA_OWNER_PID=$$
# A step that fails stops the run and names itself, so no shot after it stands for a state that never was.
fail() { note "FAILED ($look, after shot $n): $*"; exit 1; }
r() { "$recto" "$@" >/dev/null || fail "recto $*"; }
settle() { r qa wait-settled --timeout 60s; }
note() { echo "$*" | tee -a "$sheets/notes.txt"; }
n=0 look=none
# A shot waits for the window to settle, and says when it gave up: a shot not to trust.
shot() {
    n=$((n + 1)); local name; name="$(printf %02d "$n")-$1-$look"; rm -f "$out/$name.png"
    local said; said=$("$recto" qa shot --out "$out/$name.png" --json) || fail "recto qa shot ($name)"
    case "$said" in *'"settled":true'*) ;; *) note "not settled: $name" ;; esac
}
until_state() {
    for _ in $(seq 1 300); do
        "$recto" qa state --json | /usr/bin/python3 -I -c "import json,sys; d=json.load(sys.stdin)['data']; a=d['app']; sys.exit(0 if ($1) else 1)" && return 0
        sleep 0.1 # ratchet: poll
    done
    fail "never held: $1"
}
# A point on a line of text, as `recto select` places it, in the form `qa mouse-*` takes: PAGE RECT SHARE.
place() {
    "$recto" select "$1" --json | /usr/bin/python3 -I -c "
import json,sys
r=json.load(sys.stdin)['data']['pages'][0]['rects'][$2]
print('$1:%.2f,%.2f' % (r[0]+r[2]*$3, r[1]+r[3]/2))"
}

# The documents, all in the run's own folder: a text manual, a picture PDF with no contents of its own (for generated contents), a protected copy and a
# copy-restricted one.
mkdir -p "$root/in"
for f in bash article; do cp "/usr/share/doc/bash/$f.pdf" "$root/in/$f.pdf"; done
chmod u+w "$root"/in/*.pdf
qpdf --encrypt shots owner 256 -- "$root/in/article.pdf" "$root/in/locked.pdf"
qpdf --encrypt "" owner 256 --extract=n -- "$root/in/article.pdf" "$root/in/restricted.pdf"
bashp="$root/in/bash.pdf"; article="$root/in/article.pdf"
note "load at start: $(uptime)"

for look in light dark; do
    n=0
    # The last look's run, on its own state folder (the first time, none).
    "$recto" stop >/dev/null 2>&1
    for _ in $(seq 1 100); do pgrep -f "$app" >/dev/null || break; sleep 0.1; done # ratchet: poll
    export RECTO_STATE_DIR="$root/state-$look"
    r qa start --app "$app" --time-limit 10m
    r qa scroller-style overlay
    r qa appearance "$look"
    settle; shot home

    # Reading: the sidebar, and hidden; the go-to field.
    r open "$bashp"; r open "$article"; r tap "#1"; r goto 12; settle; shot reader
    r view --sidebar off; settle; shot sidebar-hidden; r view --sidebar on
    r qa button page; settle; shot go-to-field; r qa page-escape

    # Paper.
    r view --paper dark; settle; shot paper-dark
    r view --paper warm; settle; shot paper-warm; r view --paper auto

    # Find, at a hit.
    r find "history"; r find --hit 5; settle; shot find; r find --close

    # Selection: a drag over a few lines.
    r goto 3; settle
    r qa mouse-down "$(place 3 2 0.2)"; r qa mouse-drag "$(place 3 6 0.7)"; r qa mouse-up; settle; shot selection
    r qa mouse-down "$(place 3 2 0.2)"; r qa mouse-up

    # The comparison, side by side, and the panel.
    r compare --tabs "#1,#2"; settle; shot compare; r compare --off; settle
    r qa panel toggle; settle; shot panel; r qa panel toggle

    # ⌘K's palette, with words typed.
    r qa menu palette.open; r qa palette-type "zoom"; settle; shot palette; r qa palette-key escape

    # Edit mode, an edit and the Save icon, and the close prompt (cancelled), then undone.
    r edit mode on; settle; shot edit-mode; r edit mode off
    r qa as-person "page rotate 1 --by 90"; until_state "a['document_edited']"; settle; shot edited-save-icon
    r qa menu file.close; until_state "a['sheet'].get('kind') == 'close_prompt'"; settle; shot close-prompt
    r qa sheet-answer second; until_state "not a['sheet']"; r qa as-person "undo"

    # The snapshot tool, a box dragged.
    r qa menu tools.snapshot; r goto 3; settle
    r qa mouse-down "3:60,60"; r qa mouse-drag "3:400,260"; settle; shot snapshot-box; r qa mouse-up; settle; r qa menu tools.snapshot


    # Generated contents for a file with none (the manual's pages 2 to 13; its page 1 is blank): the offer, the list made, a section folded.
    qpdf --empty --pages "$bashp" 2-13 -- "$root/in/plain-$look.pdf"
    r open "$root/in/plain-$look.pdf"; settle; shot contents-offer
    r qa click-row note
    until "$recto" wait contents-generated --timeout 120s >/dev/null; do [ $? -eq 7 ] || break; done
    settle; shot contents-generated
    # The first section that has sections in it, by `recto contents`' depths, folded by the command the chevron sends.
    parent=$("$recto" contents --generated --json | /usr/bin/python3 -I -c "
import json,sys
e=json.load(sys.stdin)['data']['entries']
print(next(e[i]['entry'] for i in range(len(e)-1) if e[i+1]['depth']>e[i]['depth']))")
    r contents fold "$parent"; settle; shot contents-folded

    # A file that asks not to be copied: Copy's notice with "Copy anyway", and a print's with "Print anyway".
    r open "$root/in/restricted.pdf"; settle
    r qa menu edit.select_all; r qa menu edit.copy; until_state "a.get('copy_notice')"; settle; shot copy-restricted
    r qa mouse-down "1:300,300"; r qa mouse-up
    r qa print-to-pdf "$root/printed-$look.pdf"; settle; shot print-restricted

    # The password sheet.
    r qa as-person "open \"$root/in/locked.pdf\""; until_state "a['sheet'].get('kind') == 'password'"; settle; shot password
    printf 'shots' | r qa password; until_state "not a['sheet']"

    # The menus and Settings, drawn by the app at their places.
    n=$((n + 1)); r qa shot-menus --out "$out/$(printf %02d "$n")-menus-$look.png"
    r qa settings open; settle; n=$((n + 1)); r qa shot-settings --out "$out/$(printf %02d "$n")-settings-$look.png"; r qa settings close
done
r stop
quiet_files_out
for look in light dark; do
    swift "$here/app/qa/contact-sheet.swift" "$sheets/sheet-$look.png" "RectoPDF, $look window, $(git -C "$here" rev-parse --short HEAD)" "$sheets"/shots/*-"$look".png
done
note "load at end: $(uptime)"
note "sheets: $sheets/sheet-light.png $sheets/sheet-dark.png, in $(( $(date +%s) - started )) s"
