//! Writes small PDFs for tests, so expected values come from the bytes rather than from another parser. Test-only: nothing that ships
//! depends on this crate. No third-party file is committed: the hostile cases here are generated too.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// A body for one object: either a dictionary or a stream with its own dictionary entries.
pub enum Object {
    Dict(String),
    Stream { dict: String, data: Vec<u8> },
}

/// Builds a classic-xref PDF. Object numbers start at 1, in the given order. Object 1 must be the catalog.
pub fn build(objects: &[Object]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).as_bytes());
        match object {
            Object::Dict(body) => out.extend(body.as_bytes()),
            Object::Stream { dict, data } => {
                out.extend(format!("<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes());
                out.extend(data);
                out.extend(b"\nendstream");
            }
        }
        out.extend(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

/// One page of a [`pages`] document.
pub struct Page {
    pub content: Vec<u8>,
    /// Extra page dictionary entries, such as `/Rotate 90`.
    pub page_extra: String,
    pub media: (u32, u32),
}

impl Page {
    pub fn new(content: impl Into<Vec<u8>>) -> Self {
        Self { content: content.into(), page_extra: String::new(), media: (612, 792) }
    }
}

/// A document of the given pages, sharing Helvetica as `/F1`. Each page's resources are `/Font`, plus anything in `extra_resources`,
/// and `extra_objects` are numbered from `first_extra_object(pages.len())`.
pub fn pages_with(pages: &[Page], extra_resources: &str, extra_objects: Vec<Object>) -> Vec<u8> {
    let page_object = |i: usize| 4 + i * 2;
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", page_object(i))).collect();
    let mut objects = vec![
        Object::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        Object::Dict(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len())),
        Object::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
    ];
    for (i, page) in pages.iter().enumerate() {
        objects.push(Object::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> {extra_resources} >> {} >>",
            page.media.0,
            page.media.1,
            page_object(i) + 1,
            page.page_extra
        )));
        objects.push(Object::Stream { dict: String::new(), data: page.content.clone() });
    }
    objects.extend(extra_objects);
    build(&objects)
}

/// The number the first of `extra_objects` gets in [`pages_with`].
pub fn first_extra_object(page_count: usize) -> usize {
    4 + page_count * 2
}

pub fn pages(pages: &[Page]) -> Vec<u8> {
    pages_with(pages, "", Vec::new())
}

/// Content for lines of black 12 pt text, from near the top of a page.
pub fn text_lines(lines: &[&str]) -> Vec<u8> {
    let mut out = String::from("BT /F1 12 Tf 14 TL 72 720 Td\n");
    for line in lines {
        out.push_str(&format!("({}) Tj T*\n", escape(line)));
    }
    out.push_str("ET\n");
    out.into_bytes()
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)")
}

/// A page with ink in every region: lines of text, a coloured band, rules, a circle-ish curve. For comparing tiles to the whole.
pub fn busy_page() -> Vec<u8> {
    let mut out = String::new();
    out.push_str("0.2 0.4 0.8 rg 40 600 532 60 re f\n0.9 0.3 0.1 rg 100 300 200 200 re f\n");
    out.push_str("0 0 0 RG 0.7 w\n");
    for i in 0..40 {
        out.push_str(&format!("20 {y} m 590 {y2} l S\n", y = 20 + i * 19, y2 = 40 + i * 18));
    }
    out.push_str("0.1 0.6 0.2 RG 3 w 300 400 m 420 560 500 240 300 200 c S\n");
    out.push_str("0 g BT /F1 11 Tf 13 TL 36 760 Td\n");
    for i in 0..56 {
        out.push_str(&format!("(Line {i} of a page that crosses every tile boundary, seams and all, with room to spare) Tj T*\n"));
    }
    out.push_str("ET\n");
    out.into_bytes()
}

/// Grey text only, for the tile that must be reported grey.
pub fn grey_page() -> Vec<u8> {
    let mut out = String::from("0.3 g BT /F1 14 Tf 16 TL 72 720 Td\n");
    for i in 0..40 {
        out.push_str(&format!("(Grey text only, line {i}, no colour anywhere on this page) Tj T*\n"));
    }
    out.push_str("ET\n");
    out.into_bytes()
}

/// A page that keeps drawing long enough to cancel, cooperatively: the pause callback runs between these objects.
pub fn slow_page(rectangles: usize) -> Vec<u8> {
    let mut out = String::with_capacity(rectangles * 32);
    for i in 0..rectangles {
        let (x, y) = (i % 500, (i / 500) % 700);
        out.push_str(&format!("0.{} 0.5 0.2 rg {x} {y} 40 40 re f\n", i % 9 + 1));
    }
    out.into_bytes()
}

/// The spike's hostile `form_bomb`: a form that draws itself twice, with its own resources naming itself. PDFium took this to 4 GiB in
/// 6.3 s (spike §8), with no way to interrupt it. The second page is ordinary, so a test can tell a page failing from a file failing.
pub fn form_bomb() -> Vec<u8> {
    let bomb_page = Page { content: b"q /X Do Q".to_vec(), page_extra: String::new(), media: (200, 200) };
    let plain = Page::new(text_lines(&["An ordinary page after the bomb"]));
    let form_object = first_extra_object(2);
    let resources = format!("/XObject << /X {form_object} 0 R >>");
    pages_with(
        &[bomb_page, plain],
        &resources,
        vec![Object::Stream {
            dict: format!("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /FormType 1 /Resources << {resources} >>"),
            data: b"q 1 0 0 1 0 0 cm /X Do Q q 1 0 0 1 5 0 cm /X Do Q".to_vec(),
        }],
    )
}

/// Hidden-text techniques, ported from the spike's `gen_hidden.py`. Lines starting `HIDDEN-` must never show; `VISIBLE-` must.
pub fn hidden_text_page() -> Vec<u8> {
    let line = |x: i32, y: i32, text: &str, pre: &str, post: &str| format!("q BT {pre} /F1 12 Tf {x} {y} Td ({text}) Tj ET {post} Q\n");
    let mut out = String::new();
    out += &line(72, 740, "VISIBLE-SENTINEL normal black text on the page", "", "");
    out += &line(72, 700, "HIDDEN-WHITE white text on the white page", "1 1 1 rg", "");
    out += &line(72, 680, "HIDDEN-MODE3 text render mode three", "3 Tr", "");
    out += "q BT /F1 0.01 Tf 72 640 Td (HIDDEN-TINY font size one hundredth) Tj ET Q\n";
    out += "q BT /F1 12 Tf 0.001 0 0 0.001 72 620 Tm (HIDDEN-TMSCALE matrix scale) Tj ET Q\n";
    out += &line(10000, 600, "HIDDEN-OFFPAGE-RIGHT beyond the right edge", "", "");
    out += &line(72, -5000, "HIDDEN-OFFPAGE-BELOW below the bottom edge", "", "");
    out += &format!("q 0 0 1 1 re W n {}Q\n", line(72, 580, "HIDDEN-CLIPPED outside an active clip", "", ""));
    out += &line(72, 560, "HIDDEN-BEHINDRECT covered by an opaque rectangle", "", "");
    out += "q 1 1 1 rg 60 550 480 22 re f Q\n";
    out += &line(72, 500, "HIDDEN-MODE7 render mode seven adds to clip only", "7 Tr", "");
    out += "q 0.9 0.9 0.9 rg 60 470 480 24 re f Q\n";
    out += &line(72, 478, "HIDDEN-SAMEASBG same colour as a filled box", "0.9 0.9 0.9 rg", "");
    out += &line(72, 440, "VISIBLE-AFTER last visible sentence", "", "");
    out.into_bytes()
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A unique temp file holding the bytes, removed on drop.
pub struct TempPdf(pub PathBuf);

impl TempPdf {
    pub fn new(name: &str, bytes: &[u8]) -> Self {
        let dir = std::env::temp_dir().join(format!("recto-render-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        Self(path)
    }
}

impl Drop for TempPdf {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        if let Some(dir) = self.0.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}
