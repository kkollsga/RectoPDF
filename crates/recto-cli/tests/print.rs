//! `recto print --to-pdf` end to end, through the real `recto` and a real worker, and the result checked page by page against the source by drawing both.

mod common;

use common::State;
use recto_agent::Level;
use recto_svg::verify::{Embedded, read_png};
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn run(file: &Path, args: &[&str]) -> (Value, i32) {
    let state = State::at(Level::Full);
    let mut all = vec!["local", file.to_str().unwrap()];
    all.extend_from_slice(args);
    state.json(&all)
}

fn ok(file: &Path, args: &[&str]) -> Value {
    let (reply, code) = run(file, args);
    assert_eq!(code, 0, "{args:?}: {reply}");
    reply["data"].clone()
}

fn drawn(file: &Path, page: u32, out: &Path) -> (u32, u32, Vec<u8>) {
    ok(file, &["render", &page.to_string(), "--scale", "1", "--out", out.to_str().unwrap()]);
    let Embedded::Png { width, height, pixels, .. } = read_png(&std::fs::read(out).unwrap()) else { panic!() };
    std::fs::remove_file(out).unwrap();
    (width, height, pixels)
}

fn out_path(file: &TempPdf, name: &str) -> PathBuf {
    file.0.with_file_name(format!("{}-{name}", std::process::id()))
}

/// Three pages, each with its own words and a coloured box in its own place, so a page out of order or drawn wrong shows.
fn source() -> TempPdf {
    let page = |n: usize, colour: &str| Page::new([text_lines(&[&format!("Page {n} of the source")]), format!("{colour} rg {} 300 120 80 re f\n", 60 + n * 100).into_bytes()].concat());
    TempPdf::new("source.pdf", &pages(&[page(1, "1 0 0"), page(2, "0 0.6 0"), page(3, "0 0 1")]))
}

/// Failure: a print isn't the document: pages missing, out of order, the wrong size, or drawn differently, as from the screen's tiles or with paper colour in them. Each page of the print, drawn, must match the same page of the source, drawn the same way.
#[test]
fn a_print_matches_its_source_page_by_page() {
    let file = source();
    let out = out_path(&file, "printed.pdf");
    let printed = ok(&file.0, &["print", "--dpi", "150", "--to-pdf", out.to_str().unwrap()]);
    assert_eq!((printed["pages"].as_u64(), printed["dpi"].as_u64()), (Some(3), Some(150)), "{printed}");
    assert_eq!(ok(&out, &["facts"])["pages"], 3);
    let png = out_path(&file, "page.png");
    for page in 1..=3 {
        let (sw, sh, source) = drawn(&file.0, page, &png);
        let (pw, ph, print) = drawn(&out, page, &png);
        assert_eq!((pw, ph), (sw, sh), "page {page} is its own size");
        let differing = source.iter().zip(&print).filter(|(a, b)| a.abs_diff(**b) > 48).count();
        assert!(differing * 200 < source.len(), "page {page}: {differing} of {} channels differ by more than 48", source.len());
    }
    std::fs::remove_file(&out).unwrap();
    // A range takes those pages, in order.
    let some = ok(&file.0, &["print", "--pages", "2-3", "--dpi", "72", "--to-pdf", out.to_str().unwrap()]);
    assert_eq!(some["pages"], 2);
    let (_, _, second) = drawn(&out, 1, &png);
    let (_, _, source_second) = drawn(&file.0, 2, &png);
    let differing = second.iter().zip(&source_second).filter(|(a, b)| a.abs_diff(**b) > 64).count();
    assert!(differing * 100 < second.len(), "the print's first page is the source's second");
    std::fs::remove_file(&out).unwrap();
}

/// Failure: a print writes over a file that's there, the PDF it prints among them, or somewhere a review of Save found no file should go; or asks for what it can't do and leaves something behind.
#[test]
fn a_print_never_writes_over_anything() {
    let file = source();
    let before = std::fs::read(&file.0).unwrap();
    let (reply, code) = run(&file.0, &["print", "--to-pdf", file.0.to_str().unwrap()]);
    assert_eq!((code, reply["error"]["code"].as_str()), (1, Some("output_exists")), "{reply}");
    assert_eq!(std::fs::read(&file.0).unwrap(), before, "the source is untouched");
    let (reply, _) = run(&file.0, &["print", "--to-pdf", out_path(&file, "x.png").to_str().unwrap()]);
    assert_eq!(reply["error"]["code"], "bad_output", "{reply}");
    let (reply, _) = run(&file.0, &["print", "--pages", "9", "--to-pdf", out_path(&file, "x.pdf").to_str().unwrap()]);
    assert_eq!(reply["error"]["code"], "no_such_page", "{reply}");
    let (reply, _) = run(&file.0, &["print", "--dpi", "20", "--to-pdf", out_path(&file, "y.pdf").to_str().unwrap()]);
    assert_eq!(reply["error"]["code"], "bad_dpi", "{reply}");
    assert!(!out_path(&file, "x.pdf").exists() && !out_path(&file, "y.pdf").exists());
}
