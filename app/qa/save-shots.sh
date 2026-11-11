#!/usr/bin/env bash
# Real screenshots of saving, drawn inside the app in a quiet run, light and dark. Nothing reaches a screen, and nothing is posted: `qa button`, `qa menu` and `qa sheet-answer` go through what the window's own controls call.
#
#   app/qa/save-shots.sh OUT_DIR READER.pdf
#
# OUT_DIR gets, each -light and -dark: edited (page 1 turned, the red button's dot and the Save icon), saved (the same view after ⌘S's path, with the dot and the icon gone),
# close-prompt (File > Close with an unsaved turn), delete-edited and delete-saved (page 2 deleted, then saved by the rewrite), undo-past-delete (undone past that save,
# page 2 back from the history's copy), signed-save-off and certified-save-off (a change a Save would break the signature with: the Save icon is there and off),
# signed-copy-saved (Save As's unsigned copy, with its note) and usage-rights-saved (a delete on a file with usage rights alone, saved in place, with its note),
# damaged-save-off and damaged-close-prompt (a file lopdf and PDFium see differently, the agreement gate: Save off, and closing offers Cancel and Don't Save).
# READER.pdf is copied: the original is never written.
set -euo pipefail
out="$1"; reader="$2"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap '"$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
# A quiet run this starts ends with this script, however it ends.
export RECTO_QA_OWNER_PID=$$
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

# A signed copy of the reader's file, by an appended update: `certified` adds `/Perms /DocMDP`, `signed` a signed signature field, `usage_rights` `/Perms /UR3`.
sign() { # KIND IN OUT
    /usr/bin/python3 -I - "$1" "$2" "$3" <<'PY'
import re, sys
kind = sys.argv[1]
data = open(sys.argv[2], 'rb').read()
startxref = int(re.findall(rb'startxref\s+(\d+)', data)[-1])
root = re.findall(rb'/Root\s+(\d+)\s+(\d+)\s+R', data)[-1]
size = max(int(n) for n in re.findall(rb'/Size\s+(\d+)', data))
num, gen = int(root[0]), int(root[1])
body = re.findall(rb'(?<!\d)' + str(num).encode() + rb'\s+' + str(gen).encode() + rb'\s+obj\s*<<(.*?)>>\s*endobj', data, re.S)
if not body:
    sys.exit('the catalog is in an object stream; use another file')
cat = re.sub(rb'/Perms\s*<<.*?>>', b'', body[-1], flags=re.S)
sig, field = size, size + 1
out = bytearray(data if data.endswith(b'\n') else data + b'\n')
offsets = {}
offsets[sig] = len(out)
method = b'/UR3' if kind == 'usage_rights' else b'/DocMDP'
out += b'%d 0 obj\n<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] /Contents <00> /Reference [ << /Type /SigRef /TransformMethod %s /TransformParams << /Type /TransformParams /P 1 /V /1.2 >> >> ] >>\nendobj\n' % (sig, method)
extra = b''
if kind == 'signed':
    offsets[field] = len(out)
    out += b'%d 0 obj\n<< /FT /Sig /T (Tenant) /V %d 0 R >>\nendobj\n' % (field, sig)
    extra = b' /AcroForm << /Fields [%d 0 R] /SigFlags 3 >>' % field
elif kind == 'certified':
    extra = b' /Perms << /DocMDP %d 0 R >>' % sig
else:
    extra = b' /Perms << /UR3 %d 0 R >>' % sig
offsets[num] = len(out)
out += b'%d %d obj\n<<' % (num, gen) + cat + extra + b' >>\nendobj\n'
xref = len(out)
out += b'xref\n0 1\n0000000000 65535 f \n'
for n in sorted(offsets):
    out += b'%d 1\n%010d %05d n \n' % (n, offsets[n], gen if n == num else 0)
out += b'trailer\n<< /Size %d /Root %d %d R /Prev %d >>\nstartxref\n%d\n%%%%EOF\n' % (field + 1, num, gen, startxref, xref)
open(sys.argv[3], 'wb').write(out)
PY
}

# A file lopdf and PDFium walk differently: a /Type /Page with /Kids is a page to lopdf and a branch to PDFium, so each takes another page first.
damaged() { # OUT
    /usr/bin/python3 -I - "$1" <<'PY'
import sys
def text(words):
    return b'BT /F1 28 Tf 60 500 Td (%s) Tj ET' % words
objects = [
    b'<< /Type /Catalog /Pages 2 0 R >>',
    b'<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>',
    b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Kids [4 0 R] /Contents 6 0 R /Resources << /Font << /F1 9 0 R >> >> >>',
    b'<< /Type /Page /Parent 3 0 R /MediaBox [0 0 500 600] /Contents 7 0 R /Resources << /Font << /F1 9 0 R >> >> >>',
    b'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 8 0 R /Resources << /Font << /F1 9 0 R >> >> >>',
    text(b"lopdf's first page"),
    text(b"PDFium's first page"),
    text(b'The second page'),
    b'<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>',
]
out = bytearray(b'%PDF-1.7\n')
offsets = []
for number, body in enumerate(objects, 1):
    offsets.append(len(out))
    if body.startswith(b'BT'):
        out += b'%d 0 obj\n<< /Length %d >>\nstream\n' % (number, len(body)) + body + b'\nendstream\nendobj\n'
    else:
        out += b'%d 0 obj\n' % number + body + b'\nendobj\n'
xref = len(out)
out += b'xref\n0 %d\n0000000000 65535 f \n' % (len(objects) + 1)
for offset in offsets:
    out += b'%010d 00000 n \n' % offset
out += b'trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n' % (len(objects) + 1, xref)
open(sys.argv[1], 'wb').write(out)
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

    r close

    # A delete, saved by the rewrite, then undone past the save: page 2 comes back from the history's copy.
    copy="$root/delete-$look.pdf"
    cp "$reader" "$copy"
    r open "$copy"
    settle
    r qa owner-edit delete:2
    until_state "a['document_edited'] and a['save_button']['visible']"
    r goto 2
    settle
    shot "delete-edited-$look"
    r qa button save
    until_state "not a['document_edited'] and not a['save_button']['visible']"
    settle
    shot "delete-saved-$look"
    r qa owner-edit undo
    until_state "a['document_edited']"
    r goto 2
    settle
    shot "undo-past-delete-$look"
    r discard
    r close

    # A person's signature, and a certification: a change a Save over it would break turns Save off.
    for kind in signed certified; do
        signed="$root/$kind-$look.pdf"
        sign "$kind" "$reader" "$signed"
        r open "$signed"
        settle
        if [ "$kind" = certified ]; then r qa owner-edit rotate:1; else r qa owner-edit delete:2; fi
        until_state "a['save_button']['visible'] and not a['save_button']['enabled']"
        settle
        shot "$kind-save-off-$look"
        if [ "$kind" = signed ]; then
            r qa save-as "$root/signed unsigned copy-$look.pdf"
            until_state "not a['document_edited'] and a.get('copy_notice')"
            settle
            shot "signed-copy-saved-$look"
        else
            r discard
        fi
        r close
    done

    # Usage rights alone: a delete saves in place, with them removed, and says so quietly.
    rights="$root/usage-rights-$look.pdf"
    sign usage_rights "$reader" "$rights"
    r open "$rights"
    settle
    r qa owner-edit delete:2
    until_state "a['save_button']['visible'] and a['save_button']['enabled']"
    r qa button save
    until_state "not a['document_edited'] and a.get('copy_notice')"
    settle
    shot "usage-rights-saved-$look"
    r close

    # The agreement gate: a file its two readers see differently can't be saved, and says why.
    broken="$root/damaged-$look.pdf"
    damaged "$broken"
    r open "$broken"
    settle
    r qa owner-edit rotate:1
    until_state "a['save_button']['visible'] and not a['save_button']['enabled'] and 'differently' in a['save_button']['tooltip']"
    settle
    shot "damaged-save-off-$look"
    r qa menu file.close
    until_state "a['sheet'].get('kind') == 'close_prompt'"
    settle
    shot "damaged-close-prompt-$look"
    r qa sheet-answer first
    until_state "not a['sheet']"
    r discard
    r close
done
echo "shots are in $out"
