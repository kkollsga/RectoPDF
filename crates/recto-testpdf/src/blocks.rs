//! A three-page document for the structure helper's `blocks` job and the session's mapping of what it says: a heading over two columns of paragraphs, a heading over a list and a ruled table, and a page with a paragraph
//! drawn in white between two visible ones. [`BLOCKS_TEXT`] holds each page's lines exactly as they are drawn, so a test (and a fake corpus that serves the same text) can say what each block's words are.

use crate::{Page, pages};
use std::fmt::Write;

/// Each page's drawn strings, in the order they are drawn.
///
/// - Page 1: the heading, then the left column's eleven lines (three paragraphs of three lines, a blank line between), then the right column's eleven. distillPDF reads the left column's last paragraph and the right column's first as one block that runs on from column to column (measured 2026-11-27: five paragraphs, the third with a box over both columns), as a reader does.
/// - Page 2: the heading, four list items (each starts with a bullet), then the table's nine cells row by row.
/// - Page 3: three paragraphs, of three, two and three lines: the middle one is drawn in white.
pub const BLOCKS_TEXT: [&[&str]; 3] = [
    &[
        "Harbour Survey Overview",
        "The northern quay was measured twice",
        "during the first week of spring, once",
        "at low and at high tide.",
        "",
        "Sediment cores from the inner basin",
        "show a layer of fine silt, which the",
        "dredging starts in autumn.",
        "",
        "Lighting along the breakwater has been",
        "replaced with sealed lamps that resist",
        "salt spray and need little care.",
        "The eastern slipway carries the heaviest",
        "traffic, mostly small fishing boats that",
        "launch early and return.",
        "",
        "Fuel is stored in a tank farm behind the",
        "customs shed, and every tank is checked",
        "by a gauge each shift.",
        "",
        "Visitors may walk the full length of the",
        "outer wall when the weather is calm, but",
        "the gates close at dusk.",
    ],
    &[
        "Equipment Register",
        "\u{2022} Two mooring winches, serviced in March",
        "\u{2022} One floating crane with a lifting limit",
        "\u{2022} Six steel bollards along the east wall",
        "\u{2022} A spare generator kept in the cold store",
        "Item",
        "Count",
        "Status",
        "Winch",
        "Two",
        "Serviced",
        "Bollard",
        "Six",
        "Painted",
    ],
    &[
        "The final inspection covered every berth and",
        "found no damage that would stop a vessel from",
        "docking during the busy summer season ahead.",
        "HIDDEN secret instruction for the reader here",
        "that must never appear in any visible answer.",
        "The inspector signed the report on the last day",
        "of the month and filed one copy with the harbour",
        "master and one copy with the regional authority.",
    ],
];

/// Draws `lines` at `x`, from `y` down, 14 points apart, in 11 point type; an empty line is a gap.
fn column(out: &mut String, x: u32, y: u32, lines: &[&str]) {
    let _ = writeln!(out, "BT /F1 11 Tf 14 TL {x} {y} Td");
    for line in lines {
        if line.is_empty() {
            out.push_str("T*\n");
        } else {
            let _ = writeln!(out, "({}) Tj T*", pdf_text(line));
        }
    }
    out.push_str("ET\n");
}

/// A string for a literal in a content stream: Helvetica's own encoding has the bullet at octal 267, and parentheses and backslashes are escaped.
fn pdf_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)").replace('\u{2022}', "\\267")
}

fn heading(out: &mut String, text: &str) {
    let _ = writeln!(out, "BT /F1 24 Tf 72 740 Td ({}) Tj ET", pdf_text(text));
}

fn page_one() -> Vec<u8> {
    let text = BLOCKS_TEXT[0];
    let mut out = String::new();
    heading(&mut out, text[0]);
    column(&mut out, 72, 690, &text[1..12]);
    column(&mut out, 320, 690, &text[12..23]);
    out.into_bytes()
}

fn page_two() -> Vec<u8> {
    let text = BLOCKS_TEXT[1];
    let mut out = String::new();
    heading(&mut out, text[0]);
    for (row, item) in text[1..5].iter().enumerate() {
        column(&mut out, 90, 690 - 24 * row as u32, &[item]);
    }
    // A ruled 3 by 3 grid, 100 points wide a column and 30 high a row, from (72, 560) down.
    out.push_str("0 G 0.8 w\n");
    for step in 0..=3 {
        let (x, y) = (72 + 100 * step, 560 - 30 * step);
        let _ = writeln!(out, "{x} 560 m {x} 470 l S\n72 {y} m 372 {y} l S");
    }
    for (index, cell) in text[5..14].iter().enumerate() {
        let (row, col) = (index as u32 / 3, index as u32 % 3);
        column(&mut out, 80 + 100 * col, 540 - 30 * row, &[cell]);
    }
    out.into_bytes()
}

fn page_three() -> Vec<u8> {
    let text = BLOCKS_TEXT[2];
    let mut out = String::new();
    column(&mut out, 72, 720, &text[0..3]);
    out.push_str("q 1 1 1 rg\n");
    column(&mut out, 72, 640, &text[3..5]);
    out.push_str("Q\n");
    column(&mut out, 72, 580, &text[5..8]);
    out.into_bytes()
}

/// The document: see [`BLOCKS_TEXT`].
pub fn blocks_document() -> Vec<u8> {
    pages(&[Page::new(page_one()), Page::new(page_two()), Page::new(page_three())])
}

/// Lines a page of [`outlined_blocks_document`] holds that a document must not be able to turn into structure when it's read as Markdown: a line that starts as a heading and a code fence of three backticks.
pub const OUTLINED_FORGED: [&str; 3] = ["# Forged heading from the page", "```", "fence words after three backticks in a row"];

/// The outline's titles of [`outlined_blocks_document`], each with its depth and page (from 1).
pub const OUTLINE: [(&str, u8, u32); 3] = [("Harbour Survey Overview", 0, 1), ("Equipment Register", 0, 2), ("Final Inspection", 1, 3)];

/// [`blocks_document`] with the file's own contents (an `/Outlines` of [`OUTLINE`]: two chapters, the second with a section that starts page 3) and [`OUTLINED_FORGED`] drawn under page 3's paragraphs. Each destination's top is a little above
/// its heading's top, as a real file's is.
pub fn outlined_blocks_document() -> Vec<u8> {
    use crate::Object;
    let page_object = |index: usize| 4 + index * 2;
    let first_extra = 4 + 3 * 2;
    let item_object = |index: usize| first_extra + 1 + index;
    let mut third = page_three();
    let mut forged = String::new();
    column(&mut forged, 72, 480, &OUTLINED_FORGED);
    third.extend(forged.into_bytes());
    let contents = [page_one(), page_two(), third];
    let kids: Vec<String> = (0..3).map(|index| format!("{} 0 R", page_object(index))).collect();
    let mut objects = vec![
        Object::Dict(format!("<< /Type /Catalog /Pages 2 0 R /Outlines {first_extra} 0 R >>")),
        Object::Dict(format!("<< /Type /Pages /Kids [{}] /Count 3 >>", kids.join(" "))),
        Object::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into()),
    ];
    for (index, content) in contents.into_iter().enumerate() {
        objects.push(Object::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>",
            page_object(index) + 1
        )));
        objects.push(Object::Stream {
            dict: String::new(),
            data: content,
        });
    }
    // The outline root, then the items: the first two chapters under the root, the third under the second.
    objects.push(Object::Dict(format!(
        "<< /Type /Outlines /First {} 0 R /Last {} 0 R /Count 3 >>",
        item_object(0),
        item_object(1)
    )));
    let tops = [770, 770, 790];
    for (index, (title, _, page)) in OUTLINE.iter().enumerate() {
        let dest = format!("/Dest [{} 0 R /XYZ 0 {} 0]", page_object(*page as usize - 1), tops[index]);
        let body = match index {
            0 => format!("/Parent {first_extra} 0 R /Next {} 0 R", item_object(1)),
            1 => format!(
                "/Parent {first_extra} 0 R /Prev {} 0 R /First {} 0 R /Last {} 0 R /Count 1",
                item_object(0),
                item_object(2),
                item_object(2)
            ),
            _ => format!("/Parent {} 0 R", item_object(1)),
        };
        objects.push(Object::Dict(format!("<< /Title ({title}) {body} {dest} >>")));
    }
    crate::build(&objects)
}
