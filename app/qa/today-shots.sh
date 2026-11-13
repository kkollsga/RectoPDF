#!/usr/bin/env bash
# Looks at today's visible behaviour in the real app, in quiet runs that never touch the screen, light and dark, with notes of what `qa state` said beside each shot.
#
#   app/qa/today-shots.sh OUT_DIR [PHASE...]
#
# Phases (all by default): baseline wake early dark backfwd changed find close agent; also darkimg, wake2, restore, drawn and findwrap when named. OUT_DIR gets `<nn>-<what>-<light|dark>.png` and notes.txt.
# The PDFs are made or taken from the system here (pics.pdf: 12 pages cycling a large colour photo, text with a small photo, and a white-backed scan; the system's bash manuals),
# copied into the run's temp folder, and never opened in place.
set -uo pipefail
out="$1"; shift
phases="${*:-baseline wake early dark backfwd changed find close agent}"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="${RECTO_BIN:-$here/target/debug/recto}"
app="${RECTO_APP:-$here/app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF}"
mkdir -p "$out"
root="$(mktemp -d)"
trap 'quiet_files_out 2>/dev/null || true; "$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
. "$(dirname "$0")/quiet-files.sh"
export RECTO_QA_OWNER_PID=$$
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
note() { echo "$*" | tee -a "$out/notes.txt"; }
look=light
shot() { rm -f "$out/$1-$look.png"; "$recto" qa shot --out "$out/$1-$look.png" --json | grep -q '"settled":true' || note "NOT SETTLED: $1-$look"; }
# What `qa state` says of the place, the last wake and the window, in one line.
pst() {
    "$recto" qa state --json | /usr/bin/python3 -I -c "
import json,sys
d=json.load(sys.stdin)['data']; a=d['app']; rd=d['reader']
t=[x for x in d['session']['tabs'] if x['id']==d['session']['active']]
pl=t[0]['place'] if t else {}
w=rd['last_wake'] or {}; s=rd['sleep_draws']
print('  state $1: page',rd['page'],'scroll_y',round(rd['scroll_y']),'place',{k:round(v,2) if isinstance(v,float) else v for k,v in pl.items() if k in ('page','within','y')},'settled',rd['settled'],'| wake had=%s from_sleep=%s first_frame_us=%s bare_with_picture=%s sleep_pictures=%s sleep_in_view=%s'%(w.get('had'),w.get('from_sleep'),w.get('first_frame_us'),w.get('bare_with_picture'),w.get('sleep_pictures'),w.get('sleep_in_view')),'| sleep_draws jobs=%s pictures=%s leaves_pictured=%s'%(s['jobs'],s['pictures'],s['leaves_pictured']),'| note=%r sheet=%r find=%r'%(a.get('note'),a.get('sheet'),a['find'].get('status')),'gutters',a['pages'].get('page_gutters'))
" | tee -a "$out/notes.txt"
}
until_state() {
    for _ in $(seq 1 300); do
        if "$recto" qa state --json | /usr/bin/python3 -I -c "import json,sys; d=json.load(sys.stdin)['data']; a=d['app']; sys.exit(0 if ($1) else 1)"; then return 0; fi
        sleep 0.1 # ratchet: poll
    done
    note "never held: $1"; return 1
}
fresh() { # a new quiet run on a state folder of its own
    "$recto" stop >/dev/null 2>&1
    for _ in $(seq 1 100); do pgrep -f "$app" >/dev/null || break; sleep 0.1; done # ratchet: poll
    export RECTO_STATE_DIR="$root/state-$1"
    r qa start --app "$app" --time-limit 10m
    r qa scroller-style overlay
}
sleep_tab() { # TAB: puts a tab not on screen to sleep, trying a few times
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        "$recto" session sleep --tab "$1" --json 2>&1 | grep -q '"dormant":true' && return 0
        sleep 0.3 # ratchet: poll
    done
    note "could not put $1 to sleep"; return 1
}
mkdir -p "$root/in"
/usr/bin/python3 -I - "$root/in/pics.pdf" 12 <<'PY'
import sys, zlib, random
random.seed(7)
def photo(w, h):
    px = bytearray()
    for y in range(h):
        for x in range(w):
            t = y / h
            if t < 0.55:  # sky
                c = (int(70 + 120 * t), int(130 + 110 * t), int(230 - 40 * t))
                if (x - w * 0.75) ** 2 + (y - h * 0.15) ** 2 < (w * 0.08) ** 2: c = (255, 220, 60)
            else:  # hills and house
                g = 90 + int(70 * ((x * 3) % w) / w)
                c = (30, g, 40)
                if w * 0.3 < x < w * 0.55 and h * 0.5 < y < h * 0.75: c = (190, 40, 40)
                if w * 0.38 < x < w * 0.46 and h * 0.6 < y < h * 0.75: c = (60, 40, 20)
            px += bytes(c)
    return bytes(px)
def scan(w, h):
    rows = bytearray()
    for y in range(h):
        line = (y // 14) % 2 == 0 and (y % 14) < 8 and 90 < y < h - 90
        for x in range(w):
            v = 247 + random.randint(-3, 3)
            if line and 60 < x < w - 60 - ((y // 14) * 37 % 200) and (x // 9) % 7 != 6: v = 25 + random.randint(0, 30)
            rows.append(v)
    return bytes(rows)
objs = []
def add(b): objs.append(b); return len(objs)
def stream(d, data): return b"<< %s /Length %d /Filter /FlateDecode >>\nstream\n" % (d.encode(), len(zlib.compress(data))) + zlib.compress(data) + b"\nendstream"
cat = add(b""); pages_id = add(b""); font = add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
big = add(stream("/Type /XObject /Subtype /Image /Width 360 /Height 420 /ColorSpace /DeviceRGB /BitsPerComponent 8", photo(360, 420)))
small = add(stream("/Type /XObject /Subtype /Image /Width 110 /Height 80 /ColorSpace /DeviceRGB /BitsPerComponent 8", photo(110, 80)))
sc = add(stream("/Type /XObject /Subtype /Image /Width 306 /Height 396 /ColorSpace /DeviceGray /BitsPerComponent 8", scan(306, 396)))
n = int(sys.argv[2]); kids = []
for i in range(n):
    k = i % 3
    t = lambda y, s, size=14: b"BT /F1 %d Tf 72 %d Td (%s) Tj ET\n" % (size, y, s.encode())
    if k == 0:
        c = t(740, "Page %d: large colour photo" % (i + 1), 20) + b"q 468 0 0 546 72 150 cm /Big Do Q\n" + t(120, "Figure %d. A landscape with a red house." % (i + 1))
    elif k == 1:
        c = t(740, "Page %d: text with a small photo" % (i + 1), 20)
        for j in range(30): c += t(700 - j * 15, "Line %d of page %d: the quick brown fox jumps over the lazy dog, and the dog sleeps." % (j + 1, i + 1), 11)
        c += b"q 220 0 0 160 190 150 cm /Small Do Q\n"
    else:
        c = b"q 612 0 0 792 0 0 cm /Scan Do Q\n"
    cid = add(stream("", c))
    kids.append(add(b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] /Contents %d 0 R /Resources << /Font << /F1 %d 0 R >> /XObject << /Big %d 0 R /Small %d 0 R /Scan %d 0 R >> >> >>" % (pages_id, cid, font, big, small, sc)))
objs[cat - 1] = b"<< /Type /Catalog /Pages %d 0 R >>" % pages_id
objs[pages_id - 1] = b"<< /Type /Pages /Count %d /Kids [%s] >>" % (n, b" ".join(b"%d 0 R" % k for k in kids))
out = bytearray(b"%PDF-1.5\n"); offs = []
for i, o in enumerate(objs):
    offs.append(len(out)); out += b"%d 0 obj\n" % (i + 1) + o + b"\nendobj\n"
x = len(out); out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for o in offs: out += b"%010d 00000 n \n" % o
out += b"trailer\n<< /Size %d /Root %d 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, cat, x)
open(sys.argv[1], "wb").write(out)
PY
for f in bash article rose94 bashbug; do cp -c "/usr/share/doc/bash/$f.pdf" "$root/in/$f.pdf" 2>/dev/null || cp "/usr/share/doc/bash/$f.pdf" "$root/in/$f.pdf"; done
chmod u+w "$root"/in/*.pdf
pics="$root/in/pics.pdf"; bashp="$root/in/bash.pdf"
want() { case " $phases " in *" $1 "*) return 0;; esac; return 1; }
: > "$out/notes.txt"
note "load at start: $(uptime)"

if want baseline; then
    note "== 1 baseline"
    fresh base
    for look in light dark; do r qa appearance "$look"; settle; shot 01-home; pst "home $look"; done
    r open "$pics"; r open "$bashp"; r tap "#2"; settle
    for look in light dark; do
        r qa appearance "$look"; r view --paper auto
        r goto 12 --tab "#2"; r view --sidebar on; settle
        shot 01-reader-sidebar-open; pst "reader-sidebar-open $look"
        r view --sidebar off; settle
        shot 01-reader-sidebar-hidden; pst "reader-sidebar-hidden $look"
        r view --sidebar on; settle
    done
fi

if want wake; then
    note "== 2 wake after sleep (pics, page 7 at 0.4, other tab shown, slept, shown again)"
    fresh wake
    r open "$pics"; r open "$bashp"
    for look in light dark; do
        r qa appearance "$look"; r view --paper auto
        r tap "#1"; r goto 7 --within 0.4; settle
        shot 02-before-sleep; pst "before-sleep $look"
        r tap "#2"; settle
        sleep_tab "#1"
        r qa hold-drawing on
        r tap "#1"
        shot 02-wake-at-once; pst "wake-at-once(drawing held) $look"
        r qa hold-drawing off; settle
        shot 02-wake-settled; pst "wake-settled $look"
    done
fi

if want early; then
    note "== 3 left before its place was drawn (goto 10 far, switch tab at once, slept, woken)"
    fresh early
    r open "$pics"; r open "$bashp"
    for look in light dark; do
        r qa appearance "$look"; r view --paper auto
        r tap "#1"; settle; r goto 1; settle
        r goto 10 --within 0.3 --tab "#1"
        r tap "#2"
        pst "just-left $look"
        sleep_tab "#1"
        pst "after-sleep $look"
        r qa hold-drawing on
        r tap "#1"
        shot 03-early-wake-at-once; pst "early-wake-at-once $look"
        r qa hold-drawing off; settle
        shot 03-early-wake-settled; pst "early-wake-settled $look"
        r tap "#2"; settle
    done
fi

if want dark; then
    note "== 4 dark paper: p1 large photo, p2 small photo (text page), p3 white-backed scan; fresh, then after sleep and wake"
    fresh dark
    r open "$pics"; r open "$bashp"
    for look in light dark; do
        r qa appearance "$look"; r view --paper dark
        r tap "#1"; settle
        for p in 1 2 3; do r goto "$p"; settle; shot "04-dark-p$p-fresh"; pst "dark-p$p-fresh $look"; done
        r tap "#2"; settle; sleep_tab "#1"; r tap "#1"; settle
        for p in 1 2 3; do r goto "$p"; settle; shot "04-dark-p$p-woken"; pst "dark-p$p-woken $look"; done
        r tap "#2"; settle
    done
fi

if want backfwd; then
    note "== 5 back and forward"
    fresh backfwd
    r open "$bashp"
    for look in light dark; do
        r qa appearance "$look"
        r goto 4 --within 0.1; settle
        r goto 4 --within 0.8; settle
        shot 05-a-after-jump; pst "a-after-jump(within 0.8) $look"
        r qa scroll -400; settle
        shot 05-b-scrolled-up; pst "b-scrolled-up $look"
        r back; settle
        shot 05-c-back; pst "c-back $look"
        r forward; settle
        shot 05-d-forward; pst "d-forward $look"
        r goto 1; settle
    done
fi

if want changed; then
    note "== 6 file changed on disk under unsaved edits"
    for look in light dark; do
        for other in bashbug rose94; do
            fresh "changed-$look-$other"
            r qa appearance "$look"
            cp "$root/in/article.pdf" "$root/in/ed.pdf"; chmod u+w "$root/in/ed.pdf"; cp -p "$root/in/ed.pdf" "$root/in/ed-first.pdf"
            r open "$root/in/ed.pdf"; settle
            r goto 11; settle
            r qa as-person "page rotate 2 --by 90"
            until_state "a['document_edited']"; settle
            shot "06-$other-a-edited"; pst "edited $look"
            r qa frontmost off
            cp "$root/in/$other.pdf" "$root/in/ed.pdf"; chmod u+w "$root/in/ed.pdf"
            r qa frontmost on
            until_state "'changed on disk' in (a.get('note') or '')"; settle
            shot "06-$other-b-notice"; pst "notice($other) $look"
            note "  note words: $("$recto" qa state --json | /usr/bin/python3 -I -c "import json,sys; print(json.load(sys.stdin)['data']['app'].get('note'))")"
            r find --open; settle; shot "06-$other-c-find-open"
            r qa find-type the; until_state "a['find'].get('status')"; settle; shot "06-$other-c-find-typed"; pst "find-typed($other) $look"
            note "  find (as an agent) says: $("$recto" find the --json 2>&1 | head -c 400)"
            r find --close
            r qa menu file.save; until_state "a.get('sheet')"; settle
            shot "06-$other-d-save-sheet"; pst "save-sheet($other) $look"
            r qa sheet-answer first; settle
            r goto 1 >/dev/null 2>&1; settle; shot "06-$other-e-after-goto"; pst "after-goto1($other) $look"
            r qa frontmost off
            cp "$root/in/ed-first.pdf" "$root/in/ed.pdf"; touch -r "$root/in/ed-first.pdf" "$root/in/ed.pdf"
            r qa frontmost on
            until_state "not a.get('note')"; settle
            shot "06-$other-f-original-back"; pst "original-back($other) $look"
            r qa menu file.save; settle; shot "06-$other-g-saved"; pst "saved($other) $look"
            r close >/dev/null 2>&1
        done
    done
fi


if want darkimg; then
    note "== 4b dark paper on real embedded images: p1 photo 39% of the page with text, p2 photo 53%, p3 a JPEG photo 45%, p4 a vector drawing 56% with text, p5 small photo, p6 white-backed scan, p7 photo 75% with a caption, p8 a full-bleed photo, p9 a photo 39% on a coloured background; then p2, p4, p7, p8 and p9 again from the sleep picture as the tab wakes, and settled"
    /usr/bin/python3 -I - "$root/in/photo.bmp" <<'BMP'
import sys, struct
w, h = 360, 300
rows = []
for y in range(h):
    t = y / h
    row = bytearray()
    for x in range(w):
        if t < 0.55: c = (int(70 + 120 * t), int(130 + 110 * t), int(230 - 40 * t))
        else: c = (30, 90 + int(70 * ((x * 3) % w) / w), 40)
        if (x - w * 0.3) ** 2 + (y - h * 0.2) ** 2 < (w * 0.07) ** 2: c = (255, 220, 60)
        row += bytes((c[2], c[1], c[0]))
    row += b"\0" * ((4 - len(row) % 4) % 4)
    rows.append(bytes(row))
data = b"".join(reversed(rows))
open(sys.argv[1], "wb").write(b"BM" + struct.pack("<IHHI", 54 + len(data), 0, 0, 54) + struct.pack("<IiiHHIIiiII", 40, w, h, 1, 24, 0, len(data), 2835, 2835, 0, 0) + data)
BMP
    sips -s format jpeg "$root/in/photo.bmp" --out "$root/in/photo.jpg" >/dev/null
    /usr/bin/python3 -I - "$root/in/imgs.pdf" "$root/in/photo.jpg" <<'PY2'
import sys, zlib, random
random.seed(7)
def photo(w, h):
    px = bytearray()
    for y in range(h):
        for x in range(w):
            t = y / h
            if t < 0.55: c = (int(70 + 120 * t), int(130 + 110 * t), int(230 - 40 * t))
            else:
                c = (30, 90 + int(70 * ((x * 3) % w) / w), 40)
                if w * 0.3 < x < w * 0.55 and h * 0.5 < y < h * 0.75: c = (190, 40, 40)
            px += bytes(c)
    return bytes(px)
def scan(w, h):
    rows = bytearray()
    for y in range(h):
        line = (y // 14) % 2 == 0 and (y % 14) < 8 and 90 < y < h - 90
        for x in range(w):
            v = 247 + random.randint(-3, 3)
            if line and 60 < x < w - 60 - ((y // 14) * 37 % 200) and (x // 9) % 7 != 6: v = 25 + random.randint(0, 30)
            rows.append(v)
    return bytes(rows)
objs = []
def add(b): objs.append(b); return len(objs)
def stream(d, data): return b"<< %s /Length %d /Filter /FlateDecode >>\nstream\n" % (d.encode(), len(zlib.compress(data))) + zlib.compress(data) + b"\nendstream"
jpeg = open(sys.argv[2], "rb").read()
cat = add(b""); pages_id = add(b""); font = add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
big = add(stream("/Type /XObject /Subtype /Image /Width 360 /Height 420 /ColorSpace /DeviceRGB /BitsPerComponent 8", photo(360, 420)))
jp = add(b"<< /Type /XObject /Subtype /Image /Width 360 /Height 300 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length %d >>\nstream\n" % len(jpeg) + jpeg + b"\nendstream")
sc = add(stream("/Type /XObject /Subtype /Image /Width 306 /Height 396 /ColorSpace /DeviceGray /BitsPerComponent 8", scan(306, 396)))
t = lambda y, s, size=14: b"BT /F1 %d Tf 72 %d Td (%s) Tj ET\n" % (size, y, s.encode())
lines = lambda top, n: b"".join(t(top - j * 15, "Line %d: the quick brown fox jumps over the lazy dog, and the dog sleeps." % (j + 1), 11) for j in range(n))
def vector():
    c = b"0.30 0.55 0.90 rg 72 400 468 300 re f\n"
    for k in range(12): c += b"%.2f %.2f %.2f rg 72 %d 468 25 re f\n" % (0.35 + k * 0.04, 0.6 + k * 0.025, 0.92 - k * 0.02, 400 + k * 25)
    c += b"0.15 0.50 0.20 rg 72 220 468 180 re f\n0.75 0.15 0.15 rg 200 250 120 110 re f\n0.25 0.15 0.08 rg 245 250 30 60 re f\n0.98 0.85 0.25 rg 420 600 m 460 600 l 460 640 l 420 640 l f\n"
    return c
contents = [
    t(740, "Page 1: a photo, 39% of the page, with text", 18) + b"q 468 0 0 400 72 280 cm /Big Do Q\n" + t(255, "Figure 1. A landscape.") + lines(230, 8),
    t(740, "Page 2: a photo, 53% of the page", 18) + b"q 468 0 0 546 72 150 cm /Big Do Q\n" + t(120, "Figure 2. The same landscape, larger."),
    t(740, "Page 3: a JPEG photo, 45% of the page, with text", 18) + b"q 468 0 0 468 72 230 cm /Jp Do Q\n" + t(205, "Figure 3. A JPEG.") + lines(180, 6),
    t(740, "Page 4: a vector drawing, 56% of the page, with text", 18) + vector() + b"0 g\n" + t(195, "Figure 4. Drawn in vectors, no image.") + lines(170, 6),
    t(740, "Page 5: text with a small photo", 18) + lines(700, 30) + b"q 220 0 0 160 190 150 cm /Big Do Q\n",
    b"q 612 0 0 792 0 0 cm /Scan Do Q\n",
    t(760, "Page 7: a photo, 75% of the page, with a caption", 18) + b"q 540 0 0 672 36 72 cm /Big Do Q\n" + t(50, "Figure 7. The landscape over three quarters of the page."),
    b"q 612 0 0 792 0 0 cm /Big Do Q\n",
    b"0.13 0.23 0.42 rg 0 0 612 792 re f\n0.95 g\n" + t(740, "Page 9: a photo, 39% of the page, on a coloured background", 18) + b"q 468 0 0 400 72 280 cm /Big Do Q\n" + t(255, "Figure 9. A landscape on a blue page.") + lines(230, 8),
]
kids = []
for c in contents:
    cid = add(stream("", c))
    kids.append(add(b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] /Contents %d 0 R /Resources << /Font << /F1 %d 0 R >> /XObject << /Big %d 0 R /Jp %d 0 R /Scan %d 0 R >> >> >>" % (pages_id, cid, font, big, jp, sc)))
objs[cat - 1] = b"<< /Type /Catalog /Pages %d 0 R >>" % pages_id
objs[pages_id - 1] = b"<< /Type /Pages /Count %d /Kids [%s] >>" % (len(kids), b" ".join(b"%d 0 R" % k for k in kids))
out = bytearray(b"%PDF-1.5\n"); offs = []
for i, o in enumerate(objs):
    offs.append(len(out)); out += b"%d 0 obj\n" % (i + 1) + o + b"\nendobj\n"
x = len(out); out += b"xref\n0 %d\n0000000000 65535 f \n" % (len(objs) + 1)
for o in offs: out += b"%010d 00000 n \n" % o
out += b"trailer\n<< /Size %d /Root %d 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (len(objs) + 1, cat, x)
open(sys.argv[1], "wb").write(out)
PY2
    fresh darkimg
    r open "$root/in/imgs.pdf"
    for look in light dark; do
        r qa appearance "$look"; r view --paper dark; settle
        for p in 1 2 3 4 5 6 7 8 9; do r goto "$p"; settle; shot "04b-img-p$p"; pst "img-p$p $look"; done
    done
    # The picture a tab sleeps as is made with the page's one look: each page left, the tab slept, and shown again, first from its sleep picture with drawing held, then settled.
    r open "$bashp"
    for look in light dark; do
        r qa appearance "$look"; r view --paper dark
        for p in ${WOKEN_PAGES:-2 4 7 8 9}; do
            r tap "#1"; r goto "$p"; settle
            r tap "#2"; settle
            sleep_tab "#1"
            r qa hold-drawing on
            r tap "#1"
            shot "04b-img-p$p-woken-at-once"; pst "img-p$p-woken-at-once(drawing held) $look"
            r qa hold-drawing off; settle
            shot "04b-img-p$p-woken"; pst "img-p$p-woken $look"
        done
    done
    r tap "#1"
    # The same pages on white paper, for what they are.
    look=light; r qa appearance light; r view --paper white; settle
    for p in 2 4 7 8 9; do r goto "$p"; settle; shot "04b-img-p$p-white-paper"; done
fi

if want findwrap; then
    note "== 6d Find's line under the field, a refusal's sentence wrapped, at the default width and the narrowest"
    for look in light dark; do
        fresh "findwrap-$look"
        r qa appearance "$look"
        cp "$root/in/article.pdf" "$root/in/fw.pdf"; chmod u+w "$root/in/fw.pdf"
        r open "$root/in/fw.pdf"; settle
        r qa as-person "page rotate 2 --by 90"; until_state "a['document_edited']"
        r qa frontmost off; cp "$root/in/rose94.pdf" "$root/in/fw.pdf"; r qa frontmost on
        until_state "'changed on disk' in (a.get('note') or '')"
        r find --open; r qa find-type the; until_state "'search' in a['find']['status']"; settle
        for width in 272 200; do
            r qa sidebar-width "$width"; settle
            shot "06d-find-wrapped-$width"
            "$recto" qa state --json | /usr/bin/python3 -I -c 'import json,sys; f=json.load(sys.stdin)["data"]["app"]["find"]; print("  find at", sys.argv[1], {k: f.get(k) for k in ("status", "status_lines", "status_cut", "list_height", "block_height")})' "$look $width" | tee -a "$out/notes.txt"
        done
    done
fi

if want wake2; then
    note "== 2b wake at once with the looks in the other order (dark first), and that the zoom level and page number fade"
    fresh wake2
    r open "$pics"; r open "$bashp"
    for look in dark light; do
        r qa appearance "$look"; r view --paper auto
        r tap "#1"; r goto 7 --within 0.4; settle
        r tap "#2"; settle
        sleep_tab "#1"
        r qa hold-drawing on
        r tap "#1"
        shot 02b-wake-at-once-order; pst "wake-at-once dark-first-order $look"
        r qa hold-drawing off; settle
        shot 02b-wake-settled-order
    done
    for look in light dark; do
        r qa appearance "$look"; settle
        until_state "not a['chrome']['place_shown'] and not a['chrome']['zoom_shown']"
        shot 10-after-4s; "$recto" qa state --json | /usr/bin/python3 -I -c "
import json,sys
c=json.load(sys.stdin)['data']['app']['chrome']; print('  chrome once faded $look:',{k:c[k] for k in ('place_shown','place_alpha','zoom_shown','zoom_alpha','window_buttons_shown')})" | tee -a "$out/notes.txt"
    done
fi

if want restore; then
    note "== 6b the first version put back (same bytes, same modification time): does the notice go, and does Save work"
    fresh restore
    cp "$root/in/article.pdf" "$root/in/ed.pdf"; chmod u+w "$root/in/ed.pdf"; cp -p "$root/in/ed.pdf" "$root/in/ed-first.pdf"
    r open "$root/in/ed.pdf"; settle
    r qa as-person "page rotate 2 --by 90"; until_state "a['document_edited']"
    r qa frontmost off; cp "$root/in/bashbug.pdf" "$root/in/ed.pdf"; r qa frontmost on
    until_state "'changed on disk' in (a.get('note') or '')"
    note "  tab file facts while changed: $("$recto" qa state --json | /usr/bin/python3 -I -c "import json,sys; t=json.load(sys.stdin)['data']['session']['tabs'][0]; print(t['file'], t['notice'])")"
    r qa frontmost off; cp "$root/in/ed-first.pdf" "$root/in/ed.pdf"; touch -r "$root/in/ed-first.pdf" "$root/in/ed.pdf"
    note "  on disk now: $(stat -f 'size %z mtime %m' "$root/in/ed.pdf") first: $(stat -f 'size %z mtime %m' "$root/in/ed-first.pdf")"
    r qa frontmost on
    until_state "not a.get('note')"; pst "restored"
    r tap "#1"; settle; pst "after-tap"
    r qa menu file.save; until_state "not a['document_edited'] or a.get('sheet')"; pst "after-save"
    "$recto" tabs --json | head -c 400 | tee -a "$out/notes.txt"; echo >> "$out/notes.txt"
fi

if want drawn; then
    note "== 6c what the page view draws while the edits don't fit: article (11 pages) edited, replaced by rose94 (13 pages), by rename and written over in place"
    for how in rename inplace; do
        fresh "drawn-$how"
        look=light; r qa appearance light
        cp "$root/in/article.pdf" "$root/in/dr.pdf"; chmod u+w "$root/in/dr.pdf"
        r open "$root/in/dr.pdf"; r open "$bashp"; r tap "#1"; settle
        r goto 3; settle
        r qa as-person "page rotate 2 --by 90"; until_state "a['document_edited']"; settle
        shot "06c-$how-a-edited"; pst "edited($how)"
        r qa frontmost off
        if [ "$how" = rename ]; then cp "$root/in/rose94.pdf" "$root/in/dr.incoming"; mv "$root/in/dr.incoming" "$root/in/dr.pdf"; else cp "$root/in/rose94.pdf" "$root/in/dr.pdf"; fi
        r qa frontmost on
        until_state "'changed on disk' in (a.get('note') or '')"; settle
        shot "06c-$how-b-notice"; pst "notice($how)"
        r goto 9; settle; shot "06c-$how-c-goto9"; pst "goto9($how)"
        r goto 3; settle; shot "06c-$how-d-back-to-3"; pst "back-to-3($how)"
        r tap "#2"; settle; r tap "#1"; settle; shot "06c-$how-e-tab-back"; pst "tab-back($how)"
        r tap "#2"; settle; sleep_tab "#1"; r tap "#1"; settle; shot "06c-$how-f-woken"; pst "woken($how)"
        "$recto" tabs --json | /usr/bin/python3 -I -c 'import json,sys; t=json.load(sys.stdin)["data"]["tabs"][0]; print("  tab:", {k: t.get(k) for k in ("pages", "file_pages", "edited")})' | tee -a "$out/notes.txt"
    done
fi

if want find; then
    note "== 7 find"
    fresh find
    r open "$bashp"
    for look in light dark; do
        r qa appearance "$look"
        r find "history" >/dev/null; settle
        r find --hit 5 >/dev/null; settle
        shot 07-find-hit; pst "find-hit $look"
        r find --close; settle
    done
fi

if want close; then
    note "== 8 close and quit with unsaved edits"
    for look in light dark; do
        fresh "close-$look"
        r qa appearance "$look"
        for f in article rose94 bash; do cp "$root/in/$f.pdf" "$root/in/c-$f.pdf"; chmod u+w "$root/in/c-$f.pdf"; r open "$root/in/c-$f.pdf"; done
        r tap "#1"; settle
        r qa as-person "page rotate 2 --by 90"; until_state "a['document_edited']"; settle
        r qa menu file.close; until_state "a['sheet'].get('kind') == 'close_prompt'"; settle
        shot 08-close-prompt; pst "close-prompt $look"
        r qa sheet-answer second; until_state "not a['sheet']"
        r tap "#2"; settle
        r qa as-person "page rotate 3 --by 90"; until_state "a['document_edited']"
        r qa as-person "undo"; settle
        r tap "#3"; settle
        sleep_tab "#1"; sleep_tab "#2"
        "$recto" tabs --json | /usr/bin/python3 -I -c "
import json,sys
for t in json.load(sys.stdin)['data']['tabs']: print('  tab',t['sel'],t['untrusted']['name'],'edited',t['edited'],'undone',t['user_edits_undone'])" | tee -a "$out/notes.txt"
        note "  recto stop with an edited tab and one with only undone steps, both asleep: $("$recto" stop --json 2>&1 | head -c 700)"
        settle; shot 08-after-stop-asked; pst "after-stop-asked $look"
    done
fi

if want agent; then
    note "== 9 an agent's refused commands leave the window as it was"
    qpdf --encrypt "" owner 256 --extract=n -- "$root/in/article.pdf" "$root/in/protected.pdf" 2>/dev/null
    for look in light dark; do
        fresh "agent-$look"
        r qa appearance "$look"
        r open "$root/in/protected.pdf"; settle
        shot 09-before-agent; pst "before $look"
        note "  agent palette copy: $("$recto" palette copy --run --json 2>&1 | head -c 500)"
        note "  agent copy: $("$recto" copy --page 1 --json 2>&1 | head -c 300)"
        note "  agent permission request: $("$recto" permission request control --json 2>&1 | head -c 400)"
        note "  agent select all: $("$recto" select --page 1 --all --json 2>&1 | head -c 200)"
        settle
        shot 09-after-agent; pst "after $look"
        cmp -s "$out/09-before-agent-$look.png" "$out/09-after-agent-$look.png" && note "  window unchanged (png identical) $look" || note "  window DIFFERS $look"
    done
fi
note "load at end: $(uptime)"
