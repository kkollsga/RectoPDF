//! Where on its page a file's own contents entry is, read from its destination: the number the current entry is found by when a page holds several.

use recto_core::{TOP_SCALE, TocEntry, inspect};
use recto_testpdf::TempPdf;

/// A classic-xref PDF from object bodies, numbered from 1.
fn pdf(objects: &[String], trailer_extra: &str) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend(format!("trailer\n<< /Size {} {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out
}

/// A file of three pages (upright; turned a quarter; cropped at an offset) with one flat outline of the given `/Dest` bodies, each `[page ...]` written with its
/// page's object number (3, 4 or 5).
fn file_with(dests: &[&str]) -> TempPdf {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 595 842] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Rotate 90 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /CropBox [50 100 545 742] >>".to_string(),
    ];
    let count = dests.len();
    objects.push(format!("<< /Type /Outlines /First 7 0 R /Last {} 0 R /Count {count} >>", 6 + count));
    for (at, dest) in dests.iter().enumerate() {
        let (next, prev) = (
            if at + 1 < count { format!("/Next {} 0 R", 8 + at) } else { String::new() },
            if at > 0 { format!("/Prev {} 0 R", 6 + at) } else { String::new() },
        );
        objects.push(format!("<< /Title (Entry {at}) /Parent 6 0 R {next} {prev} /Dest {dest} >>"));
    }
    TempPdf::new("tops.pdf", &pdf(&objects, "/Root 1 0 R"))
}

fn read(dests: &[&str]) -> Vec<(u32, u16)> {
    let file = file_with(dests);
    inspect(&file.0).unwrap().toc.entries().iter().map(|entry| (entry.page, entry.top)).collect()
}

/// Failure: an entry's place down its page is read wrongly, so the current entry on a page with several is the wrong one: the file's `top` isn't taken in
/// the shown frame (from the page's top, within its crop), a fit with no top gets a place, or a turned page's `top`, a shown left, is taken for how far down.
#[test]
fn an_entrys_place_is_how_far_down_the_page_as_shown() {
    let tops = read(&[
        "[3 0 R /XYZ 0 842 0]",
        "[3 0 R /XYZ 0 421 0]",
        "[3 0 R /FitH 210.5]",
        "[3 0 R /Fit]",
        "[4 0 R /XYZ 0 400 0]",
        "[5 0 R /XYZ 0 642 0]",
    ]);
    assert_eq!(tops[0], (0, 0), "the top of the page");
    assert_eq!(tops[1], (0, TOP_SCALE / 2), "half way down");
    assert_eq!(tops[2], (0, 750), "FitH's top");
    assert_eq!(tops[3], (0, 0), "a fit that says none");
    assert_eq!(tops[4], (1, 0), "a quarter-turned page's top says nothing about how far down");
    assert_eq!(tops[5], (2, 156), "within the crop box: (742 - 642) / 642");
}

/// Failure: a destination from the file that is not a place (a string, null, missing, off the page or enormous) panics, drops the entry, or stores a place
/// outside 0 to [`TOP_SCALE`] that the current entry is then found by.
#[test]
fn a_hostile_places_is_none_or_held_to_the_page() {
    // Each alone: a number the file's syntax can't hold may cost its own entry, and must not cost the others.
    let alone = |dest: &str| read(&[dest]).first().copied();
    assert_eq!(alone("[3 0 R /XYZ 0 (text) 0]"), Some((0, 0)), "a string");
    assert_eq!(alone("[3 0 R /XYZ 0 null 0]"), Some((0, 0)), "null leaves the place as it is");
    assert_eq!(alone("[3 0 R /XYZ]"), Some((0, 0)), "no number at all");
    assert_eq!(alone("[3 0 R /XYZ 0 99999999999 0]"), Some((0, 0)), "above the page is its top");
    assert_eq!(alone("[3 0 R /XYZ 0 -99999999999 0]"), Some((0, TOP_SCALE)), "below the page is its bottom");
    for huge in [
        "[3 0 R /XYZ 0 99999999999999999999999999999999999999999999999999999999 0]",
        "[3 0 R /XYZ 0 -99999999999999999999999999999999999999999999999999999999 0]",
        "[3 0 R /XYZ 0 1.5.5 0]",
        "[3 0 R /XYZ 0 3.4e38 0]",
    ] {
        if let Some((page, top)) = alone(huge) {
            assert_eq!(page, 0, "{huge}");
            assert!(top <= TOP_SCALE, "{huge}: {top}");
        }
    }
}

/// Failure: an entry with no place changes which entry of a page is current, so a file whose outline says no places is worse than before: they are the last
/// entry of the page at any place on it, as `section_at` always was; with places, the last at or above.
#[test]
fn entries_without_places_keep_the_last_of_the_page_and_places_order_the_rest() {
    let file = file_with(&["[3 0 R /Fit]", "[3 0 R /Fit]", "[4 0 R /Fit]"]);
    let toc = inspect(&file.0).unwrap().toc;
    assert_eq!(toc.section_at_place(0, 0), Some(1), "the last of the page, as before");
    assert_eq!(toc.section_at_place(0, 900), Some(1));
    assert_eq!(toc.section_at(0), Some(1));
    let placed = file_with(&["[3 0 R /XYZ 0 842 0]", "[3 0 R /XYZ 0 421 0]", "[3 0 R /XYZ 0 100 0]", "[4 0 R /Fit]"]);
    let toc = inspect(&placed.0).unwrap().toc;
    assert_eq!(toc.section_at_place(0, 0), Some(0), "at the top of the page, the entry at its top");
    assert_eq!(toc.section_at_place(0, 499), Some(0));
    assert_eq!(toc.section_at_place(0, 500), Some(1));
    assert_eq!(toc.section_at_place(0, 1000), Some(2));
    assert_eq!(toc.section_at_place(1, 0), Some(3), "a later page's entry once the reader is there");
}

/// Failure: a field added to every contents entry of every warm tab grows each of them unnoticed (rule 1). Held at today's 20 bytes, so 10,000 entries are
/// 200,000 bytes before and after the place was added.
#[test]
fn an_entry_with_a_place_is_no_bigger() {
    assert_eq!(std::mem::size_of::<TocEntry>(), 20);
}
