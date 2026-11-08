//! The seam against real PDFium, in this process. PDFium allows one document at a time here, so tests take turns.
//!
//! These need the library from `tools/pdfium.sh`. Missing it fails plainly rather than skipping, because a skipped renderer test looks
//! like a passing one.

use recto_render::{CharRecord, NeverCancel, Pdfium, PdfiumBackend, RenderBackend, TextOptions, TileKind, TileOutcome, TileRect};
use recto_proto::device_size;
use recto_testpdf::{Page, TempPdf, busy_page, grey_page, hidden_text_page, pages, text_lines};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn turn() -> MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn library_path() -> PathBuf {
    if let Some(path) = std::env::var_os("RECTO_PDFIUM_LIB") {
        return path.into();
    }
    let exe = std::env::current_exe().unwrap();
    let profile = exe.parent().unwrap().parent().unwrap();
    profile.parent().unwrap().join("pdfium").join(recto_platform::isolate::library_file_name("pdfium"))
}

fn pdfium() -> Pdfium {
    let path = library_path();
    Pdfium::load(&path).unwrap_or_else(|error| panic!("PDFium didn't load from {}: {error:?}. Run tools/pdfium.sh", path.display()))
}

fn open(file: &TempPdf) -> PdfiumBackend {
    PdfiumBackend::open(&pdfium(), std::fs::File::open(&file.0).unwrap(), None).unwrap()
}

fn draw(backend: &mut PdfiumBackend, page: u32, rect: TileRect, scale: f32) -> (Vec<u8>, TileKind) {
    let stride = rect.width * 4;
    let mut out = vec![0x55u8; (stride * rect.height) as usize];
    match backend.render_tile(page, rect, scale, &mut out, stride, &NeverCancel).unwrap() {
        TileOutcome::Done(kind) => (out, kind),
        TileOutcome::Cancelled => panic!("nothing asked to cancel"),
    }
}

fn whole_page(backend: &mut PdfiumBackend, page: u32, scale: f32) -> (Vec<u8>, u32, u32) {
    let size = backend.page_size(page).unwrap();
    let (width, height) = device_size(size.width, size.height, scale).unwrap();
    let (pixels, _) = draw(backend, page, TileRect { x: 0, y: 0, width, height }, scale);
    (pixels, width, height)
}

/// Draws the page as `edge` tiles and pastes them together.
fn stitched(backend: &mut PdfiumBackend, page: u32, scale: f32, edge: u32, width: u32, height: u32) -> Vec<u8> {
    let mut out = vec![0u8; (width * height * 4) as usize];
    for y in (0..height).step_by(edge as usize) {
        for x in (0..width).step_by(edge as usize) {
            let rect = TileRect { x, y, width: edge.min(width - x), height: edge.min(height - y) };
            let (tile, _) = draw(backend, page, rect, scale);
            for row in 0..rect.height {
                let from = (row * rect.width * 4) as usize;
                let to = (((y + row) * width + x) * 4) as usize;
                out[to..to + (rect.width * 4) as usize].copy_from_slice(&tile[from..from + (rect.width * 4) as usize]);
            }
        }
    }
    out
}

/// The share of pixels whose largest channel difference passes `tolerance`.
fn share_differing(a: &[u8], b: &[u8], tolerance: i32) -> f64 {
    let differing = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(x, y)| (0..3).any(|channel| (i32::from(x[channel]) - i32::from(y[channel])).abs() > tolerance))
        .count();
    differing as f64 / (a.len() / 4) as f64
}

/// The share of pixels within a pixel of a tile boundary whose largest channel difference passes `tolerance`. A whole-page share hides a seam, because a seam is only a few pixels in a million.
fn share_differing_at_seams(a: &[u8], b: &[u8], width: u32, height: u32, edge: u32, tolerance: i32) -> f64 {
    let at_seam = |value: u32, end: u32| value >= edge && (value.is_multiple_of(edge) || value % edge == edge - 1) && value < end;
    let (mut differing, mut total) = (0u64, 0u64);
    for y in 0..height {
        for x in 0..width {
            if !at_seam(x, width) && !at_seam(y, height) {
                continue;
            }
            total += 1;
            let at = ((y * width + x) * 4) as usize;
            if (0..3).any(|channel| (i32::from(a[at + channel]) - i32::from(b[at + channel])).abs() > tolerance) {
                differing += 1;
            }
        }
    }
    differing as f64 / total.max(1) as f64
}

/// Paints the rows around a horizontal tile boundary black, the way a wrongly placed or short tile would show. Not white: those rows may be white already.
fn with_a_seam_painted(pixels: &[u8], width: u32, edge: u32) -> Vec<u8> {
    let mut out = pixels.to_vec();
    let row = (width * 4) as usize;
    for line in [edge - 1, edge] {
        let start = line as usize * row;
        out[start..start + row].fill(0);
    }
    out
}

/// Failure: a seam the page-wide share can't see: a whole row wrong at a tile boundary is under 0.1% of a page. So this looks only at pixels beside a boundary, and proves it would notice by
/// painting one in.
#[test]
fn tile_boundaries_themselves_match_the_page_drawn_whole() {
    let _turn = turn();
    let file = TempPdf::new("seam-rows.pdf", &pages(&[Page::new(busy_page())]));
    let mut backend = open(&file);
    let (scale, edge) = (2.0, 512);
    let (whole, width, height) = whole_page(&mut backend, 0, scale);
    let tiles = stitched(&mut backend, 0, scale, edge, width, height);
    let measured = share_differing_at_seams(&whole, &tiles, width, height, edge, 16);
    assert!(measured < 0.02, "{:.2}% of the pixels at tile boundaries differ", measured * 100.0);
    let seam = share_differing_at_seams(&whole, &with_a_seam_painted(&tiles, width, edge), width, height, edge, 16);
    assert!(seam > measured * 3.0 && seam > 0.02, "a painted seam should show here: {seam} against {measured}");
    // And in regions, which meet each other on the same grid.
    let mut pasted = vec![0u8; (width * height * 4) as usize];
    for y in (0..height).step_by(edge as usize * 2) {
        let rect = TileRect { x: 0, y, width: width.min(1024), height: (edge * 2).min(height - y) };
        let (pixels, _) = draw_region(&mut backend, 0, rect, scale, edge);
        for row in 0..rect.height {
            let (from, to) = ((row * rect.width * 4) as usize, (((y + row) * width) * 4) as usize);
            pasted[to..to + (rect.width * 4) as usize].copy_from_slice(&pixels[from..from + (rect.width * 4) as usize]);
        }
    }
    let row_share = {
        let (mut differing, mut total) = (0u64, 0u64);
        for y in (edge * 2..height).step_by(edge as usize * 2) {
            for line in [y - 1, y] {
                for x in 0..width.min(1024) {
                    total += 1;
                    let at = ((line * width + x) * 4) as usize;
                    if (0..3).any(|channel| (i32::from(whole[at + channel]) - i32::from(pasted[at + channel])).abs() > 16) {
                        differing += 1;
                    }
                }
            }
        }
        differing as f64 / total.max(1) as f64
    };
    assert!(row_share < 0.02, "{:.2}% of the pixels where regions meet differ", row_share * 100.0);
}

/// Failure: tile edges don't line up with the page drawn whole, so a seam shows wherever two tiles meet. The shifted comparison proves the
/// tolerance is tight enough to catch a tile placed one pixel off.
#[test]
fn a_page_drawn_as_tiles_equals_the_page_drawn_whole() {
    let _turn = turn();
    let file = TempPdf::new("seam.pdf", &pages(&[Page::new(busy_page())]));
    let mut backend = open(&file);
    for (scale, edge) in [(2.0, 512), (1.0, 256), (1.37, 300)] {
        let (whole, width, height) = whole_page(&mut backend, 0, scale);
        let tiles = stitched(&mut backend, 0, scale, edge, width, height);
        let differing = share_differing(&whole, &tiles, 16);
        assert!(differing < 0.002, "scale {scale} tile {edge}: {:.3}% of pixels differ", differing * 100.0);
        // Negative control: the same comparison, with the picture moved one pixel.
        let row = (width * 4) as usize;
        let shifted = share_differing(&whole[row..], &tiles[..tiles.len() - row], 16);
        assert!(shifted > differing * 5.0 && shifted > 0.005, "a one-pixel shift should be visible: {shifted} against {differing}");
    }
}

/// Failure: a tile of an empty area isn't reported blank, so every empty tile costs a whole surface; or one is reported blank with ink on it.
#[test]
fn blank_tiles_are_reported_and_really_are_one_colour() {
    let _turn = turn();
    let file = TempPdf::new("blank.pdf", &pages(&[Page::new(text_lines(&["Only at the top of the page"]))]));
    let mut backend = open(&file);
    // Text sits near the top. The bottom of the page is empty.
    let (pixels, kind) = draw(&mut backend, 0, TileRect { x: 0, y: 1000, width: 512, height: 512 }, 2.0);
    assert_eq!(kind, TileKind::Blank { rgb: 0xFF_FFFF });
    assert!(pixels.as_chunks::<4>().0.iter().all(|px| px[..3] == [255, 255, 255]), "reported blank with ink on it");
    let (pixels, kind) = draw(&mut backend, 0, TileRect { x: 0, y: 0, width: 1224, height: 300 }, 2.0);
    assert!(!matches!(kind, TileKind::Blank { .. }), "a tile with text was reported blank");
    assert!(pixels.as_chunks::<4>().0.iter().any(|px| px[0] < 128), "nothing drew");
}

/// Failure: a grey text tile isn't reported grey and costs four times the memory, or a coloured one is and loses its colour.
#[test]
fn grey_and_colour_tiles_are_told_apart() {
    let _turn = turn();
    let file = TempPdf::new("grey.pdf", &pages(&[Page::new(grey_page()), Page::new(busy_page())]));
    let mut backend = open(&file);
    let (pixels, kind) = draw(&mut backend, 0, TileRect { x: 0, y: 0, width: 1024, height: 1024 }, 2.0);
    assert_eq!(kind, TileKind::Grey);
    assert!(pixels.as_chunks::<4>().0.iter().all(|px| px[0] == px[1] && px[1] == px[2]), "reported grey with chroma in it");
    let (_, kind) = draw(&mut backend, 1, TileRect { x: 0, y: 200, width: 1024, height: 1024 }, 2.0);
    assert_eq!(kind, TileKind::Colour);
}

/// Failure: a rotated page draws in its unrotated shape, or tiles of it land in the wrong place.
#[test]
fn rotated_pages_have_rotated_sizes_and_tile_correctly() {
    let _turn = turn();
    let page = Page { content: busy_page(), page_extra: "/Rotate 90".into(), media: (612.0, 792.0) };
    let file = TempPdf::new("rotated.pdf", &pages(&[page]));
    let mut backend = open(&file);
    let size = backend.page_size(0).unwrap();
    assert_eq!((size.width, size.height), (792.0, 612.0));
    let (whole, width, height) = whole_page(&mut backend, 0, 2.0);
    assert_eq!((width, height), (1584, 1224));
    let tiles = stitched(&mut backend, 0, 2.0, 512, width, height);
    assert!(share_differing(&whole, &tiles, 16) < 0.002);
}

/// Failure: the same input draws differently twice in one build, so nothing can be cached or compared.
#[test]
fn the_same_tile_draws_the_same_twice() {
    let _turn = turn();
    let file = TempPdf::new("determinism.pdf", &pages(&[Page::new(busy_page())]));
    let mut backend = open(&file);
    let rect = TileRect { x: 100, y: 100, width: 512, height: 512 };
    let (first, _) = draw(&mut backend, 0, rect, 2.0);
    let (second, _) = draw(&mut backend, 0, rect, 2.0);
    assert!(first == second);
}

/// Draws `rect` as one region with `edge` tiles, returning its pixels and kinds.
fn draw_region(backend: &mut PdfiumBackend, page: u32, rect: TileRect, scale: f32, edge: u32) -> (Vec<u8>, Vec<TileKind>) {
    let stride = rect.width * 4;
    let mut out = vec![0x55u8; (stride * rect.height) as usize];
    let mut kinds = Vec::new();
    let outcome = backend.render_region(page, rect, scale, edge, &mut out, stride, &NeverCancel, &mut kinds).unwrap();
    assert!(matches!(outcome, TileOutcome::Done(_)));
    (out, kinds)
}

/// Failure: tiles handed out of a region don't match the page drawn whole, so a seam shows where one region meets the next. Regions are drawn on the tile grid and pasted
/// together, then compared the way tiles are, with the same one-pixel negative control.
#[test]
fn a_page_drawn_as_regions_equals_the_page_drawn_whole() {
    let _turn = turn();
    let file = TempPdf::new("region-seam.pdf", &pages(&[Page::new(busy_page())]));
    let mut backend = open(&file);
    for (scale, block) in [(2.0, 1024), (1.37, 1024)] {
        let (whole, width, height) = whole_page(&mut backend, 0, scale);
        let mut pasted = vec![0u8; (width * height * 4) as usize];
        for y in (0..height).step_by(block as usize) {
            for x in (0..width).step_by(block as usize) {
                let rect = TileRect { x, y, width: block.min(width - x), height: block.min(height - y) };
                let (pixels, _) = draw_region(&mut backend, 0, rect, scale, 512);
                for row in 0..rect.height {
                    let from = (row * rect.width * 4) as usize;
                    let to = (((y + row) * width + x) * 4) as usize;
                    pasted[to..to + (rect.width * 4) as usize].copy_from_slice(&pixels[from..from + (rect.width * 4) as usize]);
                }
            }
        }
        let differing = share_differing(&whole, &pasted, 16);
        assert!(differing < 0.002, "scale {scale}: {:.3}% of pixels differ", differing * 100.0);
        let row = (width * 4) as usize;
        let shifted = share_differing(&whole[row..], &pasted[..pasted.len() - row], 16);
        assert!(shifted > differing * 5.0 && shifted > 0.005, "a one-pixel shift should be visible: {shifted} against {differing}");
    }
}

/// Failure: a tile cut from a region differs from the same tile drawn alone, so whichever way a tile was drawn shows on screen.
#[test]
fn a_tile_from_a_region_equals_the_tile_drawn_alone() {
    let _turn = turn();
    let file = TempPdf::new("region-tile.pdf", &pages(&[Page::new(busy_page())]));
    let mut backend = open(&file);
    let region = TileRect { x: 0, y: 512, width: 1024, height: 1024 };
    let (pixels, kinds) = draw_region(&mut backend, 0, region, 2.0, 512);
    for (tile, kind) in recto_proto::region_tiles(region, 512).zip(&kinds) {
        let (alone, alone_kind) = draw(&mut backend, 0, tile, 2.0);
        assert_eq!(*kind, alone_kind, "{tile:?}");
        let mut cut = Vec::new();
        for row in 0..tile.height {
            let at = ((tile.y - region.y + row) * region.width + (tile.x - region.x)) as usize * 4;
            cut.extend_from_slice(&pixels[at..at + tile.width as usize * 4]);
        }
        assert!(share_differing(&alone, &cut, 16) < 0.002, "{tile:?}");
    }
}

/// Failure: a region reports one kind for all its tiles, so an empty tile in a busy view costs a surface and a light page loses its savings. Each tile is classified by itself.
#[test]
fn each_tile_of_a_region_is_classified_by_itself() {
    let _turn = turn();
    let file = TempPdf::new("region-kinds.pdf", &pages(&[Page::new(text_lines(&["Only at the top of the page"]))]));
    let mut backend = open(&file);
    let size = backend.page_size(0).unwrap();
    let (width, height) = device_size(size.width, size.height, 2.0).unwrap();
    let region = TileRect { x: 0, y: 0, width: width.min(1024), height };
    let (_, kinds) = draw_region(&mut backend, 0, region, 2.0, 512);
    assert_eq!(kinds.len(), recto_proto::region_tiles(region, 512).count());
    assert!(kinds.iter().any(|kind| matches!(kind, TileKind::Grey)), "the tile with text: {kinds:?}");
    assert_eq!(kinds.iter().filter(|kind| matches!(kind, TileKind::Blank { rgb: 0xFF_FFFF })).count(), kinds.len() - 1, "the rest are empty: {kinds:?}");
}

/// Failure: a region cancelled up front still draws, or its tiles are reported.
#[test]
fn a_region_cancelled_up_front_draws_nothing() {
    let _turn = turn();
    let file = TempPdf::new("region-cancel.pdf", &pages(&[Page::new(busy_page())]));
    let mut backend = open(&file);
    let mut out = vec![0x55u8; 1024 * 1024 * 4];
    let mut kinds = Vec::new();
    let rect = TileRect { x: 0, y: 0, width: 1024, height: 1024 };
    let cancelled = || true;
    assert_eq!(backend.render_region(0, rect, 2.0, 512, &mut out, 4096, &cancelled, &mut kinds).unwrap(), TileOutcome::Cancelled);
    assert!(kinds.is_empty());
    assert!(out.iter().all(|&byte| byte == 0x55));
}

/// Failure: a tile off the page or past the document reaches PDFium and draws garbage, or a page count from a hostile file is believed.
#[test]
fn impossible_requests_are_refused_before_pdfium_sees_them() {
    let _turn = turn();
    let file = TempPdf::new("bad.pdf", &pages(&[Page::new(text_lines(&["x"]))]));
    let mut backend = open(&file);
    let mut out = vec![0u8; 512 * 512 * 4];
    let rect = TileRect { x: 0, y: 0, width: 512, height: 512 };
    assert!(backend.render_tile(1, rect, 2.0, &mut out, 2048, &NeverCancel).is_err(), "page past the end");
    assert!(backend.render_tile(0, TileRect { x: 99_999, ..rect }, 2.0, &mut out, 2048, &NeverCancel).is_err(), "off the page");
    assert!(backend.render_tile(0, TileRect { height: 513, ..rect }, 2.0, &mut out, 2048, &NeverCancel).is_err(), "buffer too small");
    assert!(backend.render_tile(0, rect, 2.0, &mut out, 100, &NeverCancel).is_err(), "stride shorter than a row");
    assert!(backend.page_text(9, TextOptions::default()).is_err());
    assert!(backend.page_size(9).is_err());
}

/// Failure: a cancelled job still draws, wasting a worker on a page nobody is looking at.
#[test]
fn a_job_cancelled_up_front_draws_nothing() {
    let _turn = turn();
    let file = TempPdf::new("cancel.pdf", &pages(&[Page::new(busy_page())]));
    let mut backend = open(&file);
    let mut out = vec![0x55u8; 512 * 512 * 4];
    let rect = TileRect { x: 0, y: 0, width: 512, height: 512 };
    let cancelled = || true;
    assert_eq!(backend.render_tile(0, rect, 2.0, &mut out, 2048, &cancelled).unwrap(), TileOutcome::Cancelled);
    assert!(out.iter().all(|&byte| byte == 0x55), "pixels were written for a cancelled job");
}

/// Failure: a second document opens in the same process and corrupts PDFium's global state.
#[test]
fn a_second_document_in_one_process_is_refused() {
    let _turn = turn();
    let file = TempPdf::new("one.pdf", &pages(&[Page::new(text_lines(&["x"]))]));
    let first = open(&file);
    assert!(PdfiumBackend::open(&pdfium(), std::fs::File::open(&file.0).unwrap(), None).is_err());
    drop(first);
    let again = open(&file);
    assert_eq!(again.page_count(), 1, "closing the first must free the slot");
}

fn text_of(chars: &[CharRecord]) -> String {
    chars.iter().map(|c| c.ch()).collect()
}

/// Failure: page text is empty, out of order, or its boxes don't line up with its characters, so highlights land on the wrong words.
#[test]
fn page_text_comes_with_a_box_per_character() {
    let _turn = turn();
    let file = TempPdf::new("text.pdf", &pages(&[Page::new(text_lines(&["First line here", "Second line here"]))]));
    let mut backend = open(&file);
    let page = backend.page_text(0, TextOptions { boxes: true, hidden_check: false }).unwrap();
    let text = text_of(&page.chars);
    assert!(text.contains("First line here") && text.contains("Second line here"), "{text:?}");
    assert!(text.find("First") < text.find("Second"));
    assert_eq!(page.boxes.len(), page.chars.len());
    let first = page.chars.iter().position(|c| c.ch() == 'F').unwrap();
    let [left, bottom, right, top] = page.boxes[first];
    assert!((71.0..90.0).contains(&left) && right > left, "{:?}", page.boxes[first]);
    assert!(top > bottom && (700.0..=740.0).contains(&bottom), "{:?}", page.boxes[first]);
    assert!(!page.hidden_checked);
    let plain = backend.page_text(0, TextOptions::default()).unwrap();
    assert!(plain.boxes.is_empty(), "boxes cost 16 bytes a character, so they're sent only when asked");
}

/// Failure: the hidden-text check misses a technique an attacker could use to reach an agent, or flags ordinary visible text. Nothing may
/// be dropped either way: every line stays in the text, marked.
#[test]
fn hidden_text_is_marked_and_nothing_is_dropped() {
    let _turn = turn();
    let file = TempPdf::new("hidden.pdf", &pages(&[Page::new(hidden_text_page())]));
    let mut backend = open(&file);
    let page = backend.page_text(0, TextOptions { boxes: false, hidden_check: true }).unwrap();
    assert!(page.hidden_checked);
    let mut lines: Vec<(String, bool)> = Vec::new();
    let mut current = (String::new(), false);
    for record in &page.chars {
        let ch = record.ch();
        if ch == '\r' || ch == '\n' {
            if !current.0.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.0.push(ch);
        current.1 |= record.has(CharRecord::NO_INK);
    }
    lines.push(current);
    for technique in ["HIDDEN-WHITE", "HIDDEN-MODE3", "HIDDEN-TINY", "HIDDEN-TMSCALE", "HIDDEN-OFFPAGE-RIGHT", "HIDDEN-OFFPAGE-BELOW", "HIDDEN-CLIPPED", "HIDDEN-BEHINDRECT", "HIDDEN-MODE7", "HIDDEN-SAMEASBG"] {
        let line = lines.iter().find(|(text, _)| text.contains(technique)).unwrap_or_else(|| panic!("{technique} was dropped: {lines:?}"));
        assert!(line.1, "{technique} wasn't flagged");
    }
    for visible in ["VISIBLE-SENTINEL", "VISIBLE-AFTER"] {
        let line = lines.iter().find(|(text, _)| text.contains(visible)).unwrap_or_else(|| panic!("{visible} is missing: {lines:?}"));
        assert!(!line.1, "{visible} was flagged as hidden");
    }
}

/// Failure: white text drawn over a paragraph of black text isn't flagged, because each of its boxes has the paragraph's ink in it, so `recto text`, a copy and generated contents all take words no one can see for the page's own. A review's crafted file, `white_over_body`. And white text on a dark box, which can be read, isn't flagged.
#[test]
fn white_text_over_a_paragraph_is_marked_hidden() {
    let _turn = turn();
    let mut content = String::from("BT /F1 10 Tf 12 TL 72 690 Td\n");
    for _ in 0..30 {
        content += "(This is ordinary body text on the page, written in a smaller size than any heading.) Tj T*\n";
    }
    content += "ET\nq 1 1 1 rg BT /F1 20 Tf 72 600 Td (HIDDENOVERBODY) Tj ET Q\n";
    content += "q 0 0 0 rg 60 90 400 30 re f Q q 1 1 1 rg BT /F1 20 Tf 72 100 Td (VISIBLEONDARK) Tj ET Q\n";
    let file = TempPdf::new("over.pdf", &pages(&[Page::new(content.into_bytes())]));
    let mut backend = open(&file);
    let page = backend.page_text(0, TextOptions { boxes: false, hidden_check: true }).unwrap();
    assert!(page.hidden_checked);
    let text: String = page.chars.iter().map(|record| record.ch()).collect();
    let marked = |word: &str| {
        let at = text.find(word).unwrap_or_else(|| panic!("{word} missing"));
        page.chars[at..at + word.len()].iter().filter(|record| record.has(CharRecord::NO_INK)).count()
    };
    assert_eq!(marked("HIDDENOVERBODY"), "HIDDENOVERBODY".len());
    assert_eq!(marked("VISIBLEONDARK"), 0);
    assert_eq!(marked("ordinary body"), 0, "the paragraph under it is still seen");
}

/// Failure: heavy type is marked hidden because it covers more than half its box, so its own colour is the box's main one: bold headings, display type, heavy numerals like an IRS form's "990", thin letters and marks that are mostly ink at a large size (`I l | .`), and solid symbols. They then go from `recto text`, a copy and generated contents. A review's crafted `bold_solid.pdf` and bench100's `forms_irs_f990.pdf`.
#[test]
fn heavy_type_is_never_marked_hidden() {
    let _turn = turn();
    let content = "BT /F2 28 Tf 72 720 Td (1 Introduction) Tj ET\n\
        BT /F2 48 Tf 72 640 Td (Will. Fill) Tj ET\n\
        BT /F2 96 Tf 72 520 Td (990) Tj ET\n\
        BT /F2 120 Tf 72 360 Td (I l | .) Tj ET\n\
        BT /F2 72 Tf 72 240 Td (\\225 \\267 #) Tj ET\n\
        BT /F1 10 Tf 72 120 Td (Body text in an ordinary size under them.) Tj ET\n\
        q 1 1 1 rg BT /F2 20 Tf 300 640 Td (HIDDENWHITE) Tj ET Q\n";
    let objects = vec![
        recto_testpdf::Object::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
        recto_testpdf::Object::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F2 6 0 R >> >> >>".into()),
        recto_testpdf::Object::Stream { dict: String::new(), data: content.as_bytes().to_vec() },
        recto_testpdf::Object::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into()),
        recto_testpdf::Object::Dict("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".into()),
    ];
    let file = TempPdf::new("heavy.pdf", &recto_testpdf::build(&objects));
    let mut backend = open(&file);
    let page = backend.page_text(0, TextOptions { boxes: false, hidden_check: true }).unwrap();
    assert!(page.hidden_checked);
    let text: String = page.chars.iter().map(|record| record.ch()).collect();
    let flagged: String = page.chars.iter().filter(|record| record.has(CharRecord::NO_INK)).map(|record| record.ch()).collect();
    assert_eq!(flagged, "HIDDENWHITE", "only the white line is hidden, in {text:?}");
}

/// Failure: an ordinary page reports hidden text, so the flag means nothing and an agent learns to ignore it.
#[test]
fn an_ordinary_page_has_no_hidden_text() {
    let _turn = turn();
    let file = TempPdf::new("plain.pdf", &pages(&[Page::new(busy_page())]));
    let mut backend = open(&file);
    let page = backend.page_text(0, TextOptions { boxes: false, hidden_check: true }).unwrap();
    assert!(page.chars.len() > 1000);
    let flagged = page.chars.iter().filter(|c| c.has(CharRecord::NO_INK)).count();
    assert_eq!(flagged, 0);
}

/// Failure: a page with no text reads as "not checked", so an agent is told nothing is known about a page where there's nothing to know.
#[test]
fn a_page_with_no_text_counts_as_checked() {
    let _turn = turn();
    let file = TempPdf::new("empty-page.pdf", &pages(&[Page::new(b"0.5 g 10 10 100 100 re f".to_vec())]));
    let mut backend = open(&file);
    let page = backend.page_text(0, TextOptions { boxes: false, hidden_check: true }).unwrap();
    assert!(page.chars.is_empty());
    assert!(page.hidden_checked);
}

/// Failure: the page can't be drawn for the hidden-text check and the page's text is lost with it, though ENGINEERING promises the text back, marked as not
/// checked. A page under a pixel across can't be drawn at any scale the check uses.
#[test]
fn text_survives_a_page_the_check_cant_draw() {
    let _turn = turn();
    let tiny = Page { content: text_lines(&["Text on a page too small to draw"]), page_extra: String::new(), media: (0.1, 0.1) };
    let file = TempPdf::new("tiny.pdf", &pages(&[tiny]));
    let mut backend = open(&file);
    let page = backend.page_text(0, TextOptions { boxes: false, hidden_check: true }).unwrap();
    assert!(!page.chars.is_empty(), "the text was lost");
    assert!(!page.hidden_checked, "it says it was checked when the drawing failed");
    assert!(page.chars.iter().all(|c| !c.has(CharRecord::NO_INK)), "nothing may be marked without a drawing");
}

/// Failure: tools/pdfium.sh installs a tarball whose digest is wrong, or leaves anything behind when it stops. A downloaded binary is untrusted.
#[test]
fn the_fetch_script_stops_on_a_bad_digest() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/pdfium.sh");
    let dir = std::env::temp_dir().join(format!("recto-pdfium-script-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("-not-the-tarball.tgz");
    std::fs::write(&bad, b"not the tarball").unwrap();
    let output = std::process::Command::new("bash")
        .arg(&script)
        .env("CARGO_TARGET_DIR", dir.join("target"))
        .env("RECTO_PDFIUM_TARBALL", &bad)
        .output()
        .unwrap();
    let installed = dir.join("target/pdfium").exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!output.status.success(), "it accepted a wrong digest");
    assert!(!installed, "it wrote something after a wrong digest");
    assert!(String::from_utf8_lossy(&output.stderr).contains("digest"), "it should say why");
}

/// Failure: garbage or an encrypted file is reported as something else, so the user is told the wrong thing.
#[test]
fn open_failures_say_why() {
    let _turn = turn();
    let junk = TempPdf::new("junk.pdf", b"this is not a pdf");
    let result = PdfiumBackend::open(&pdfium(), std::fs::File::open(&junk.0).unwrap(), None);
    assert_eq!(result.err(), Some(recto_render::FailCode::NotPdf));
    let empty = TempPdf::new("empty.pdf", b"");
    assert!(PdfiumBackend::open(&pdfium(), std::fs::File::open(&empty.0).unwrap(), None).is_err());
    let nul = TempPdf::new("ok.pdf", &pages(&[Page::new(text_lines(&["x"]))]));
    assert!(PdfiumBackend::open(&pdfium(), std::fs::File::open(&nul.0).unwrap(), Some(b"a\0b")).is_err(), "a password with a NUL");
}
