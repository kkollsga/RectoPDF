#!/usr/bin/env bash
# Highlights in the real app (R4.3): made from a selection by Mark Up > Highlight, unsaved on white, warm and dark paper, then saved, the file closed
# and opened again, on white and dark paper as it really looks; then the file's own taken out (gone before a Save, back on Undo, gone once saved and
# opened again), and one on a page an edit turned, saved and opened again (a saved highlight is part of the page PDFium draws, so dark paper turns it over with
# the page: the coordinator's Q8, a follow-up). Light and dark windows. Quiet runs only: nothing reaches a screen, nothing is posted (the `qa` verbs
# call what the window's own controls call), state is temp, and the run ends with the script.
#
#   app/qa/highlight-shots.sh OUT_DIR
#
# OUT_DIR gets NN-<state>-<look>.png and notes.txt. Needs the debug app (app/build.sh, through tools/sysguard/heavy.sh) and the system's bash manual.
set -uo pipefail
out="$1"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
export RECTO_STATE_DIR="$root/state-none"
trap 'quiet_files_out 2>/dev/null || true; "$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
. "$(dirname "$0")/quiet-files.sh"
export RECTO_QA_OWNER_PID=$$
fail() { note "FAILED ($look, after shot $n): $*"; exit 1; }
r() { "$recto" "$@" >/dev/null || fail "recto $*"; }
settle() { r qa wait-settled --timeout 60s; }
note() { echo "$*" | tee -a "$out/notes.txt"; }
n=0 look=none
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
place() {
    "$recto" select "$1" --json | /usr/bin/python3 -I -c "
import json,sys
r=json.load(sys.stdin)['data']['pages'][0]['rects'][$2]
print('$1:%.2f,%.2f' % (r[0]+r[2]*$3, r[1]+r[3]/2))"
}

mkdir -p "$root/in"
note "load at start: $(uptime)"
for look in light dark; do
    n=0
    "$recto" stop >/dev/null 2>&1
    for _ in $(seq 1 100); do pgrep -f "$app" >/dev/null || break; sleep 0.1; done # ratchet: poll
    export RECTO_STATE_DIR="$root/state-$look"
    doc="$root/in/manual-$look.pdf"
    cp /usr/share/doc/bash/bash.pdf "$doc"; chmod u+w "$doc"
    r qa start --app "$app" --time-limit 10m
    r qa scroller-style overlay
    r qa appearance "$look"
    r open "$doc"; r goto 3; settle

    # Made: a drag over a few lines, Mark Up > Highlight, the selection cleared by a click.
    r qa mouse-down "$(place 3 2 0.2)"; r qa mouse-drag "$(place 3 6 0.7)"; r qa mouse-up
    r qa menu markup.highlight; until_state "a['document_edited']"
    r qa mouse-down "$(place 3 2 0.2)"; r qa mouse-up; settle
    r view --paper white; settle; shot unsaved-white
    r view --paper warm; settle; shot unsaved-warm
    r view --paper dark; settle; shot unsaved-dark
    r view --paper white

    # Saved, closed and opened again: the highlight is the file's now, drawn with the page.
    r qa menu file.save; until_state "not a['document_edited']"
    r qa menu file.close; settle
    r open "$doc"; r goto 3; settle; shot saved-reopened-white
    r view --paper dark; settle; shot saved-reopened-dark
    r view --paper white; settle

    # The file's own highlight taken out (review of R4.3, S2): the same lines selected and Mark Up > Remove Highlight, gone before a Save (the
    # worker leaves it out), back on Undo, and gone from the file once saved, closed and opened again.
    r qa mouse-down "$(place 3 2 0.2)"; r qa mouse-drag "$(place 3 6 0.7)"; r qa mouse-up
    r qa menu markup.remove_highlight; until_state "a['document_edited']"
    r qa mouse-down "$(place 3 2 0.2)"; r qa mouse-up; settle; shot removed-unsaved
    r qa menu edit.undo; until_state "not a['document_edited']"; settle; shot removal-undone
    r qa menu edit.redo; until_state "a['document_edited']"
    r qa menu file.save; until_state "not a['document_edited']"
    r qa menu file.close; settle
    r open "$doc"; r goto 3; settle; shot removed-saved-reopened

    # A highlight on a page an edit turned, saved, closed and opened again: on its text, where the turned page now has it (S3).
    r page rotate --pages 3 --degrees 90; settle
    r highlight --page 3 --chars 0-160; until_state "a['document_edited']"; settle; shot turned-unsaved
    r qa menu file.save; until_state "not a['document_edited']"
    r qa menu file.close; settle
    r open "$doc"; r goto 3; settle; shot turned-saved-reopened
    r view --paper auto
done
r stop
quiet_files_out
note "load at end: $(uptime)"
