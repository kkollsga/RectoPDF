//! Whether a mark on the page is missing from the export, the way the eye sees it: dark in `recto render`, with nothing dark anywhere near it in the same SVG drawn by a real browser. Block-by-block figures can't see this: they once passed 12 pages of 274 with an equation's big brackets gone.
//!
//! The browser makes this opt-in, so it's ignored by default and asks for one by name:
//!
//! `RECTO_CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" tools/sysguard/heavy.sh cargo nextest run --locked -p recto-cli --test missing_marks --run-ignored all`
//!
//! Headless, so it never takes the screen or the keyboard. `RECTO_MARKS_PAGES="/path/a.pdf:3,/path/b.pdf:1"` adds real pages.

mod common;

use common::State;
use recto_agent::Level;
use recto_svg::verify::{Embedded, read_png};
use recto_testpdf::{Object, Page, TempPdf, first_extra_object, pages, pages_with, text_lines};
use std::path::Path;
use std::process::{Command, Stdio};

/// A mark this many pixels at 2x is one the eye would miss. Glyphs' edges differ by far fewer, between two renderers, and the thin bar of a big bracket is well over.
const SMALLEST_MARK: usize = 25;
/// How much darker it has to be to count.
const DARKER: i32 = 96;

struct Grey {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

fn grey(image: &Embedded) -> Grey {
    let Embedded::Png { width, height, channels, pixels } = image else { panic!("not a PNG") };
    let colours = (*channels).clamp(1, 3);
    let grey = pixels.chunks(*channels).map(|pixel| (pixel[..colours].iter().map(|value| u32::from(*value)).sum::<u32>() / colours as u32) as u8).collect();
    Grey { width: *width as usize, height: *height as usize, pixels: grey }
}

/// The size of the largest connected mark in `page` with nothing as dark within a pixel of it in `other`, a shift of a pixel between renderers being no loss.
fn largest_lost_mark(page: &Grey, other: &Grey) -> (usize, usize, usize) {
    let (width, height) = (page.width.min(other.width), page.height.min(other.height));
    let at = |x: usize, y: usize| y * width + x;
    let mut lost = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut nearest_dark = 255i32;
            for near_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for near_x in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    nearest_dark = nearest_dark.min(i32::from(other.pixels[near_y * other.width + near_x]));
                }
            }
            lost[at(x, y)] = nearest_dark - i32::from(page.pixels[y * page.width + x]) > DARKER;
        }
    }
    let mut largest = (0, 0, 0);
    let mut stack = Vec::new();
    for start in 0..lost.len() {
        if !lost[start] {
            continue;
        }
        lost[start] = false;
        stack.push(start);
        let mut size = 0;
        while let Some(here) = stack.pop() {
            size += 1;
            let (x, y) = (here % width, here / width);
            for near_y in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for near_x in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    if lost[at(near_x, near_y)] {
                        lost[at(near_x, near_y)] = false;
                        stack.push(at(near_x, near_y));
                    }
                }
            }
        }
        if size > largest.0 {
            largest = (size, start % width, start / width);
        }
    }
    largest
}

fn browser_draws(chrome: &str, svg: &Path, shot: &Path, profile: &Path, size: (f64, f64)) {
    let status = Command::new(chrome)
        .args(["--headless=new", "--disable-gpu", "--hide-scrollbars", "--mute-audio", "--run-all-compositor-stages-before-draw", "--virtual-time-budget=10000", "--force-device-scale-factor=2"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--window-size={},{}", size.0.ceil(), size.1.ceil()))
        .arg(format!("--screenshot={}", shot.display()))
        .arg(format!("file://{}", svg.display()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("the browser runs");
    assert!(status.success(), "the browser failed");
}

/// Exports one page, draws it both ways and gives the largest mark the export lost, with where it is.
fn lost_from(chrome: &str, what: &str, pdf: &Path, page: &str) -> (usize, usize, usize) {
    let state = State::at(Level::Full);
    let folder = pdf.parent().unwrap().join(format!("marks-{}-{what}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let (render, svg, shot) = (folder.join("page.png"), folder.join("page.svg"), folder.join("shot.png"));
    let path = pdf.to_str().unwrap();
    let (reply, code) = state.json(&["local", path, "render", page, "--scale", "2", "--out", render.to_str().unwrap()]);
    assert_eq!(code, 0, "{what}: {reply}");
    let (reply, code) = state.json(&["local", path, "export", "svg", page, "--out", svg.to_str().unwrap()]);
    assert_eq!(code, 0, "{what}: {reply}");
    let size = (reply["data"]["width_pt"].as_f64().unwrap(), reply["data"]["height_pt"].as_f64().unwrap());
    browser_draws(chrome, &svg, &shot, &folder.join("profile"), size);
    let drawn = grey(&read_png(&std::fs::read(&shot).unwrap()));
    let rendered = grey(&read_png(&std::fs::read(&render).unwrap()));
    std::fs::remove_dir_all(&folder).unwrap();
    largest_lost_mark(&rendered, &drawn)
}

/// A page with a glyph at the code for a space, as TeX's big brackets are.
fn bracket_page() -> Vec<u8> {
    let (font, glyph) = (first_extra_object(1), first_extra_object(1) + 1);
    let dict = format!("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 3000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /space {glyph} 0 R >> /Encoding << /Type /Encoding /Differences [32 /space] >> /FirstChar 32 /LastChar 32 /Widths [1000] >>");
    let drawn = Object::Stream { dict: String::new(), data: b"1000 0 0 0 800 3000 d1 100 0 500 3000 re f".to_vec() };
    pages_with(&[Page::new("BT /T3 10 Tf 100 500 Td ( ) Tj ET BT /F1 12 Tf 130 500 Td (next to it) Tj ET\n")], &format!("/Font << /T3 {font} 0 R >>"), vec![Object::Dict(dict), drawn])
}

/// A stamp's look, on a page whose last object has a clip that doesn't reach it.
fn stamp_page() -> Vec<u8> {
    let (annot, look) = (first_extra_object(1), first_extra_object(1) + 1);
    let appearance = Object::Stream { dict: "/Type /XObject /Subtype /Form /BBox [0 0 100 100]".into(), data: b"0 0 0 rg 0 0 100 100 re f".to_vec() };
    let stamp = Object::Dict(format!("<< /Type /Annot /Subtype /Stamp /Rect [300 300 400 400] /AP << /N {look} 0 R >> /F 4 >>"));
    pages_with(&[Page { page_extra: format!("/Annots [{annot} 0 R]"), ..Page::new("q 10 10 m 20 10 l 15 20 l h W n 0 g 0 0 100 100 re f Q\n") }], "", vec![stamp, appearance])
}

/// Failure: this check is blind, or sees marks that aren't lost: it has to find a real gap whatever its shape, and let through a pixel's shift and a smudge. It takes no browser, so it always runs.
#[test]
fn the_check_finds_a_lost_mark_and_lets_a_shift_through() {
    let page = |marks: &[(usize, usize, usize, usize)]| {
        let (width, height) = (60, 60);
        let mut pixels = vec![255u8; width * height];
        for &(left, top, right, bottom) in marks {
            for y in top..bottom {
                for x in left..right {
                    pixels[y * width + x] = 0;
                }
            }
        }
        Grey { width, height, pixels }
    };
    let bar = (20, 5, 22, 50);
    assert!(largest_lost_mark(&page(&[bar]), &page(&[])).0 >= SMALLEST_MARK, "a thin bar gone altogether");
    assert_eq!(largest_lost_mark(&page(&[bar]), &page(&[bar])).0, 0, "the same");
    assert_eq!(largest_lost_mark(&page(&[bar]), &page(&[(21, 5, 23, 50)])).0, 0, "a pixel across");
    assert!(largest_lost_mark(&page(&[(1, 1, 3, 3)]), &page(&[])).0 < SMALLEST_MARK, "a speck");
    let extra_is_not_lost = largest_lost_mark(&page(&[]), &page(&[bar])).0;
    assert_eq!(extra_is_not_lost, 0, "something extra isn't something lost");
}

/// Failure: a mark on the page is gone from the export with nothing said, as big brackets and stamps were, while every figure looks fine. Pages built to have the kinds of mark that have gone missing before, and any real ones given.
#[test]
#[ignore = "needs a browser: set RECTO_CHROME, and see this file's top"]
fn nothing_the_page_shows_is_missing_from_the_export() {
    let chrome = std::env::var("RECTO_CHROME").expect("RECTO_CHROME is where the browser is");
    let plain = pages(&[Page::new(text_lines(&["Ordinary words on an ordinary page", "and a second line of them"]))]);
    for (what, bytes) in [("text", plain), ("bracket", bracket_page()), ("stamp", stamp_page())] {
        let file = TempPdf::new("doc.pdf", &bytes);
        let (size, x, y) = lost_from(&chrome, what, &file.0, "1");
        assert!(size < SMALLEST_MARK, "{what}: a mark of {size} pixels at {x},{y} (at 2x) is gone");
    }
    if let Ok(list) = std::env::var("RECTO_MARKS_PAGES") {
        for one in list.split(',').filter(|one| !one.is_empty()) {
            let (path, page) = one.rsplit_once(':').expect("a file and its page");
            let (size, x, y) = lost_from(&chrome, &page.replace('/', "_"), Path::new(path), page);
            println!("{one}: largest lost mark {size} pixels at {x},{y}");
        }
    }
}
