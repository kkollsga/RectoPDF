#!/usr/bin/env bash
# Real screenshots of saving, drawn inside the app in a quiet run, light and dark. Nothing reaches a screen, and nothing is posted: `qa button`, `qa menu` and `qa sheet-answer` go through what the window's own controls call.
#
#   app/qa/save-shots.sh OUT_DIR READER.pdf
#
# OUT_DIR gets, each -light and -dark: edited (page 1 turned, the red button's dot and the Save icon), saved (the same view after ⌘S's path, with the dot and the icon gone),
# close-prompt (File > Close with an unsaved turn) and certified-alert (Save on a certified copy, refused). READER.pdf is copied: the original is never written.
set -euo pipefail
out="$1"; reader="$2"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap '"$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
export RECTO_STATE_DIR="$root/s"
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
# A shot waits for the pages and the window's animations, and says if it gave up: that's a shot not to trust.
shot() { rm -f "$out/$1.png"; "$recto" qa shot --out "$out/$1.png" --json | grep -q '"settled":true' || echo "not settled: $1" >&2; }
# Waits until a Python expression over `qa state`'s data holds, for up to 30 s.
until_state() {
    for _ in $(seq 1 300); do
        if "$recto" qa state --json | /usr/bin/python3 -I -c "import json,sys; d=json.load(sys.stdin)['data']; a=d['app']; sys.exit(0 if ($1) else 1)"; then return 0; fi
        sleep 0.1
    done
    echo "never held: $1" >&2; return 1
}

# A certified copy: the reader's file with `/Perms /DocMDP` added to its catalog by an appended update, as a certification has it.
certify() { # IN OUT
    /usr/bin/python3 -I - "$1" "$2" <<'PY'
import re, sys
data = open(sys.argv[1], 'rb').read()
startxref = int(re.findall(rb'startxref\s+(\d+)', data)[-1])
root = re.findall(rb'/Root\s+(\d+)\s+(\d+)\s+R', data)[-1]
size = max(int(n) for n in re.findall(rb'/Size\s+(\d+)', data))
num, gen = int(root[0]), int(root[1])
# The catalog's dictionary as the file has it, last definition wins; its /Perms is replaced.
body = re.findall(rb'(?<!\d)' + str(num).encode() + rb'\s+' + str(gen).encode() + rb'\s+obj\s*<<(.*?)>>\s*endobj', data, re.S)
if not body:
    sys.exit('the catalog is in an object stream; use another file')
cat = re.sub(rb'/Perms\s*<<.*?>>', b'', body[-1], flags=re.S)
sig = size
out = bytearray(data if data.endswith(b'\n') else data + b'\n')
offsets = {}
offsets[sig] = len(out)
out += b'%d 0 obj\n<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /Reference [ << /Type /SigRef /TransformMethod /DocMDP /TransformParams << /Type /TransformParams /P 1 /V /1.2 >> >> ] >>\nendobj\n' % sig
offsets[num] = len(out)
out += b'%d %d obj\n<<' % (num, gen) + cat + b' /Perms << /DocMDP %d 0 R >> >>\nendobj\n' % sig
xref = len(out)
out += b'xref\n0 1\n0000000000 65535 f \n'
for n in sorted(offsets):
    out += b'%d 1\n%010d %05d n \n' % (n, offsets[n], gen if n == num else 0)
out += b'trailer\n<< /Size %d /Root %d %d R /Prev %d >>\nstartxref\n%d\n%%%%EOF\n' % (sig + 1, num, gen, startxref, xref)
open(sys.argv[2], 'wb').write(out)
PY
}

"$recto" qa start --app "$app" --time-limit 10m >/dev/null
r qa scroller-style overlay
for look in light dark; do
    r qa appearance "$look"
    copy="$root/report-$look.pdf"
    cp "$reader" "$copy"
    r open "$copy"
    settle
    r page rotate 1 --by 90
    until_state "a['document_edited'] and a['save_button']['visible']"
    settle
    shot "edited-$look"
    r qa button save
    until_state "not a['document_edited'] and not a['save_button']['visible']"
    settle
    shot "saved-$look"
    r page rotate 1 --by 90
    until_state "a['document_edited']"
    r qa menu file.close
    until_state "a['sheet'].get('kind') == 'close_prompt'"
    settle
    shot "close-prompt-$look"
    r qa sheet-answer cancel
    until_state "not a['sheet']"
    r undo
    until_state "not a['document_edited']"

    signed="$root/certified-$look.pdf"
    certify "$reader" "$signed"
    r open "$signed"
    settle
    r page rotate 1 --by 90
    until_state "a['document_edited']"
    r qa button save
    until_state "a['sheet'].get('kind') == 'save_problem'"
    settle
    shot "certified-alert-$look"
    r qa sheet-answer first
    until_state "not a['sheet']"
    r undo
    until_state "not a['document_edited']"
    r close
    r close
done
echo "shots are in $out"
