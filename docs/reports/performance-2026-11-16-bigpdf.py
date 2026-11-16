#!/usr/bin/env python3
"""The made 2 GiB, 10,000-page PDF of run B (docs/reports/budgets-2026-11-08.md and performance-2026-11-16.md).

As the 2026-11-08 report describes it: a classic xref, each page a line of text and a 210 KiB stream beside it, so the
page objects are scattered over the whole file. That report didn't keep its generator; this one makes a file of the same
shape (2,155,0xx,xxx bytes against its 2,155,039,645).

    python3 -I docs/reports/performance-2026-11-16-bigpdf.py OUT.pdf [PAGES]
"""
import sys

out = sys.argv[1]
pages = int(sys.argv[2]) if len(sys.argv) > 2 else 10_000
FILLER = 210 * 1024
# Bytes that look like data (not all one byte), the same for every page, so the file writes fast.
filler = bytes((i * 131 + 7) & 0xFF for i in range(FILLER))

# Objects: 1 catalog, 2 pages, 3 font; then per page n (0-based): page 4+3n, contents 5+3n, filler 6+3n.
count = 3 + 3 * pages
offsets = [0] * (count + 1)
with open(out, "wb") as f:
    pos = 0

    def put(data: bytes):
        global pos
        f.write(data)
        pos += len(data)

    def obj(num: int, body: bytes):
        offsets[num] = pos
        put(b"%d 0 obj\n" % num + body + b"\nendobj\n")

    put(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
    kids = b" ".join(b"%d 0 R" % (4 + 3 * n) for n in range(pages))
    obj(2, b"<< /Type /Pages /Count %d /Kids [ " % pages + kids + b" ] >>")
    obj(3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    for n in range(pages):
        page, contents, big = 4 + 3 * n, 5 + 3 * n, 6 + 3 * n
        obj(page, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents %d 0 R >>" % contents)
        text = b"BT /F1 24 Tf 72 700 Td (Page %d of the made file) Tj ET" % (n + 1)
        obj(contents, b"<< /Length %d >>\nstream\n" % len(text) + text + b"\nendstream")
        obj(big, b"<< /Length %d >>\nstream\n" % FILLER + filler + b"\nendstream")
    xref = pos
    put(b"xref\n0 %d\n0000000000 65535 f \n" % (count + 1))
    for num in range(1, count + 1):
        put(b"%010d 00000 n \n" % offsets[num])
    put(b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (count + 1, xref))
print(out, pos)
