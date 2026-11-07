#!/usr/bin/env python3
"""Writes a PDF that embeds no font at all, so every glyph comes from whatever the renderer finds on the system.

The case the sandbox could break without any error: a worker that can't read a font draws a substitute, or nothing, and still says "done".
Run with `python3 -I make_font_pdf.py out.pdf`. Page 1: the base 14 and common Mac names. Page 2: names only in Supplemental. Page 3: CJK by Adobe CMap, and a
made-up name, to see the fallback.
"""
import sys

BASE14 = ["Helvetica", "Helvetica-Bold", "Helvetica-Oblique", "Times-Roman", "Times-Bold", "Times-Italic", "Courier", "Courier-Bold", "Symbol", "ZapfDingbats"]
MAC = ["ArialMT", "Arial-BoldMT", "TimesNewRomanPSMT", "CourierNewPSMT", "Georgia", "Verdana", "TrebuchetMS", "Tahoma", "Impact", "Menlo-Regular", "AvenirNext-Regular", "Futura-Medium",
       "Baskerville", "Didot", "Palatino-Roman", "Optima-Regular", "GillSans", "HelveticaNeue", "Monaco", "ComicSansMS", "NoSuchFontAnywhere-Regular"]
CJK = [("STSong-Light", "UniGB-UTF16-H", "Adobe", "GB1", 5, "4E2D6587"), ("HeiseiKakuGo-W5", "UniJIS-UTF16-H", "Adobe", "Japan1", 5, "65E5672C8A9E"), ("HYGoThic-Medium", "UniKS-UTF16-H", "Adobe", "Korea1", 2, "D55CAE00"), ("PingFangSC-Regular", "UniGB-UTF16-H", "Adobe", "GB1", 5, "4E2D6587")]
SAMPLE = b"The quick brown fox jumps over 13 lazy dogs: fi fl AVA Wax."


def build(out):
    objects = {}
    next_id = [3]

    def add(body):
        n = next_id[0]
        next_id[0] += 1
        objects[n] = body
        return n

    def text_page(fonts, cjk=()):
        font_dict, ops, y = {}, [], 760
        for i, name in enumerate(fonts):
            font_dict[f"F{i}"] = add(f"<< /Type /Font /Subtype /TrueType /BaseFont /{name} /Encoding /WinAnsiEncoding >>".encode() if name not in BASE14 else f"<< /Type /Font /Subtype /Type1 /BaseFont /{name} >>".encode())
            ops.append(f"BT /F{i} 13 Tf 36 {y} Td ({SAMPLE.decode()}) Tj ET".encode())
            y -= 24
        for i, (name, cmap, reg, ordering, supp, hexs) in enumerate(cjk):
            descriptor = add(f"<< /Type /FontDescriptor /FontName /{name} /Flags 6 /FontBBox [-200 -300 1200 1000] /ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 700 /StemV 80 >>".encode())
            descendant = add(f"<< /Type /Font /Subtype /CIDFontType0 /BaseFont /{name} /CIDSystemInfo << /Registry ({reg}) /Ordering ({ordering}) /Supplement {supp} >> /FontDescriptor {descriptor} 0 R >>".encode())
            font_dict[f"C{i}"] = add(f"<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /{cmap} /DescendantFonts [{descendant} 0 R] >>".encode())
            ops.append(f"BT /C{i} 28 Tf 36 {y} Td <{hexs}> Tj ET".encode())
            y -= 48
        content = b"\n".join(ops)
        stream = add(b"<< /Length " + str(len(content)).encode() + b" >>\nstream\n" + content + b"\nendstream")
        fonts_text = " ".join(f"/{k} {v} 0 R" for k, v in font_dict.items())
        return add(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << {fonts_text} >> >> /Contents {stream} 0 R >>".encode())

    kids = [text_page(BASE14), text_page(MAC), text_page(["Helvetica"], CJK)]
    objects[1] = b"<< /Type /Catalog /Pages 2 0 R >>"
    objects[2] = f"<< /Type /Pages /Kids [{' '.join(f'{k} 0 R' for k in kids)}] /Count {len(kids)} >>".encode()
    data = bytearray(b"%PDF-1.7\n")
    offsets = {}
    for n in sorted(objects):
        offsets[n] = len(data)
        data += f"{n} 0 obj\n".encode() + objects[n] + b"\nendobj\n"
    xref = len(data)
    data += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    for n in sorted(objects):
        data += f"{offsets[n]:010d} 00000 n \n".encode()
    data += f"trailer << /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    with open(out, "wb") as f:
        f.write(data)


if __name__ == "__main__":
    build(sys.argv[1])
