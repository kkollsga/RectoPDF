//! Selecting text through `recto select` and `recto copy`, on real PDFs drawn by the real worker: what an agent gets, and what the app is built on.

mod common;

use common::State;
use lopdf::encryption::{EncryptionState, EncryptionVersion, Permissions};
use recto_index::select::{PAGE_END, PageText, Rect, Unit};
use recto_platform::surface::PixelSurface;
use recto_proto::TileRect;
use recto_render_service::{DocRenderer, Policy, ServiceConfig, TileRequest, TileResult};
use recto_testpdf::{Page, TempPdf, hidden_text_page, pages, text_lines};
use serde_json::Value;
use std::path::Path;

/// One command on one file, in the in-process route. That route opens the file first, which is what needs Control. The levels have a test of their own, every row included.
fn recto(file: &TempPdf, args: &[&str]) -> (Value, i32) {
    let state = State::at(recto_agent::Level::Control);
    let mut line = vec!["local", file.0.to_str().unwrap()];
    line.extend_from_slice(args);
    state.json(&line)
}

fn data(file: &TempPdf, args: &[&str]) -> Value {
    let (reply, code) = recto(file, args);
    assert_eq!(code, 0, "{args:?}: {reply}");
    reply["data"].clone()
}

fn text(answer: &Value) -> &str {
    answer["untrusted"]["text"].as_str().unwrap_or_else(|| panic!("no text in {answer}"))
}

fn rects(answer: &Value, page: usize) -> Vec<[f64; 4]> {
    answer["pages"][page]["rects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rect| {
            [
                rect[0].as_f64().unwrap(),
                rect[1].as_f64().unwrap(),
                rect[2].as_f64().unwrap(),
                rect[3].as_f64().unwrap(),
            ]
        })
        .collect()
}

/// A page with two columns of text, the left one drawn first.
fn two_columns() -> Vec<u8> {
    let mut content = String::from("BT /F1 12 Tf 14 TL 72 720 Td\n");
    for line in ["Left column first line", "Left column second line", "Left column third line"] {
        content.push_str(&format!("({line}) Tj T*\n"));
    }
    content.push_str("ET\nBT /F1 12 Tf 14 TL 340 720 Td\n");
    for line in ["Right column first line", "Right column second line", "Right column third line"] {
        content.push_str(&format!("({line}) Tj T*\n"));
    }
    content.push_str("ET\n");
    content.into_bytes()
}

fn midpoint(rect: [f64; 4]) -> String {
    format!("{},{}", rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0)
}

/// Failure: selection goes by position on the page, so a drag from one column into the other takes lines from both by height, and a pasted page reads across the gutter.
#[test]
fn a_two_column_page_copies_in_reading_order() {
    let file = TempPdf::new("columns.pdf", &pages(&[Page::new(two_columns())]));
    let page = data(&file, &["select", "1"]);
    assert_eq!(
        text(&page),
        "Left column first line\nLeft column second line\nLeft column third line\nRight column first line\nRight column second line\nRight column third line"
    );
    let all = rects(&page, 0);
    assert_eq!(all.len(), 6, "one a line: {all:?}");
    assert!(
        all.iter().all(|rect| rect[0] + rect[2] < 300.0 || rect[0] > 330.0),
        "a highlight crosses the gutter: {all:?}"
    );
    // From the left column's second line to the right column's second: what lies between them in reading order, not what lies between them on the page.
    let from = rects(&page, 0)[1];
    let to = rects(&page, 0)[4];
    let drag = data(
        &file,
        &[
            "select",
            "--page",
            "1",
            "--at",
            &format!("{},{}", from[0], from[1] + from[3] / 2.0),
            "--to",
            &format!("{},{}", to[0] + to[2], to[1] + to[3] / 2.0),
        ],
    );
    assert_eq!(
        text(&drag),
        "Left column second line\nLeft column third line\nRight column first line\nRight column second line"
    );
    let by_copy = data(
        &file,
        &[
            "copy",
            "--page",
            "1",
            "--at",
            &format!("{},{}", from[0], from[1] + from[3] / 2.0),
            "--to",
            &format!("{},{}", to[0] + to[2], to[1] + to[3] / 2.0),
        ],
    );
    assert_eq!(text(&by_copy), text(&drag), "`copy` and `select` take the same");
}

/// Failure: a drag over a page break loses a line at the join, runs the pages together, or needs the pages to be the same one.
#[test]
fn a_selection_crosses_a_page_boundary() {
    let file = TempPdf::new(
        "two.pdf",
        &pages(&[
            Page::new(text_lines(&["First page top", "First page bottom"])),
            Page::new(text_lines(&["Second page top", "Second page bottom"])),
            Page::new(text_lines(&["Third page"])),
        ]),
    );
    let first = data(&file, &["select", "1"]);
    let second = data(&file, &["select", "2"]);
    let (from, to) = (rects(&first, 0)[1], rects(&second, 0)[0]);
    let drag = data(
        &file,
        &[
            "select",
            "--page",
            "1",
            "--at",
            &midpoint([from[0], from[1], 0.0, from[3]]),
            "--to-page",
            "2",
            "--to",
            &format!("{},{}", to[0] + to[2], to[1] + to[3] / 2.0),
        ],
    );
    assert_eq!(text(&drag), "First page bottom\nSecond page top");
    assert_eq!(drag["pages"].as_array().unwrap().len(), 2);
    assert_eq!(rects(&drag, 0).len(), 1, "only the second line of the first page");
    let whole = data(&file, &["copy", "--page", "2", "--to-page", "3"]);
    assert_eq!(text(&whole), "Second page top\nSecond page bottom\nThird page");
    assert_eq!(
        text(&data(&file, &["copy", "--all"])),
        "First page top\nFirst page bottom\nSecond page top\nSecond page bottom\nThird page"
    );
    assert_eq!(text(&data(&file, &["copy", "--page", "1", "--chars", "6-10"])), "page");
}

/// A report's pages, drawn: a running head, two lines of body, and the page number at the foot.
fn report(count: u32) -> Vec<u8> {
    let list: Vec<Page> = (1..=count)
        .map(|page| Page::new(format!("BT /F1 9 Tf 72 750 Td (Annual report 2024) Tj ET\nBT /F1 11 Tf 14 TL 72 690 Td (Body of page {page}, first line.) Tj T* (Body of page {page}, second line.) Tj ET\nBT /F1 9 Tf 540 30 Td ({page}) Tj ET\n").into_bytes()))
        .collect();
    pages(&list)
}

/// Failure: a copy across pages has the page number and the running head in the middle of the body text, as the owner found (2026-11-07: "When selecting across pages the page number is copied. Both footer and header should be ignored when coppying body text."), or a copy of a whole page has them, or `recto text` changes, which still reads everything on the page.
#[test]
fn a_copy_of_body_text_leaves_out_running_heads_and_page_numbers() {
    let file = TempPdf::new("report.pdf", &report(4));
    let across = data(&file, &["copy", "--page", "2", "--to-page", "3"]);
    assert_eq!(
        text(&across),
        "Body of page 2, first line.\nBody of page 2, second line.\nBody of page 3, first line.\nBody of page 3, second line.",
        "{across}"
    );
    assert!(
        across["chrome_left_out"].as_u64().unwrap() > 0 && across["note"].as_str().unwrap().contains("running heads"),
        "{across}"
    );
    let all = data(&file, &["copy", "--all"]);
    assert!(
        !text(&all).contains("Annual") && !text(&all).lines().any(|line| line.trim().parse::<u32>().is_ok()),
        "{all}"
    );
    let page = data(&file, &["text", "2"]);
    let read = page["untrusted"]["text"].as_str().unwrap();
    assert!(
        read.contains("Annual report 2024") && read.lines().any(|line| line.trim() == "2"),
        "recto text is unchanged: {read:?}"
    );
    // It marks them, by the same rule, in characters of its text.
    let chars: Vec<char> = read.chars().collect();
    let marked: Vec<String> = page["chrome_spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|span| {
            chars[span[0].as_u64().unwrap() as usize..span[1].as_u64().unwrap() as usize]
                .iter()
                .collect::<String>()
        })
        .collect();
    assert_eq!(marked, vec!["Annual report 2024".to_string(), "2".to_string()], "{page}");
    let (shown, _, _) = State::at(recto_agent::Level::Control).text(&["local", file.0.to_str().unwrap(), "text", "2"]);
    assert!(shown.contains("2 lines are marked as running heads, footers or page numbers"), "{shown}");
    // Picking only the running head copies it.
    let head = read.find("Annual").unwrap();
    let only = data(&file, &["copy", "--page", "2", "--chars", &format!("{head}-{}", head + 18)]);
    assert_eq!(text(&only), "Annual report 2024");
}

/// A legal text's pages, drawn: an article's heading at the top of some, two lines of body, and a footer whose number counts with the pages.
fn articles() -> Vec<u8> {
    let list: Vec<Page> = [Some(12), Some(13), None, Some(14)]
        .iter()
        .enumerate()
        .map(|(at, article)| {
            let page = at + 1;
            let heading = article.map_or(String::new(), |number| format!("BT /F1 11 Tf 72 760 Td (Article {number}) Tj ET\n"));
            Page::new(
                format!("{heading}BT /F1 11 Tf 14 TL 72 700 Td (Body of page {page}, first line.) Tj T* (Body of page {page}, second line.) Tj ET\nBT /F1 9 Tf 72 30 Td (Official Journal of the Union L 119/{page}) Tj ET\n")
                    .into_bytes(),
            )
        })
        .collect();
    pages(&list)
}

/// Failure: numbered headings at the top of pages, "Article 12" on one and "Article 13" on the next, are the same line once their digits are masked, so a copy leaves real headings out as a running head; or the footer whose number counts with the pages stays.
#[test]
fn numbered_headings_at_the_top_of_pages_stay_in_a_copy() {
    let file = TempPdf::new("articles.pdf", &articles());
    let all = data(&file, &["copy", "--all"]);
    let copied = text(&all);
    for heading in ["Article 12", "Article 13", "Article 14"] {
        assert!(copied.contains(heading), "{heading}: {copied:?}");
    }
    assert!(!copied.contains("Official Journal"), "{copied:?}");
}

/// Failure: `--at` means something else from what `rects` said, so a word found at a point isn't the one drawn there.
#[test]
fn a_click_takes_the_word_or_the_line_it_lands_on() {
    let file = TempPdf::new("one.pdf", &pages(&[Page::new(text_lines(&["The quick brown fox", "jumps over it"]))]));
    let quick = data(&file, &["select", "--page", "1", "--chars", "4-9"]);
    assert_eq!(text(&quick), "quick");
    let at = midpoint(rects(&quick, 0)[0]);
    assert_eq!(
        text(&data(&file, &["select", "--page", "1", "--at", &at])),
        "quick",
        "by default, as a double-click does"
    );
    assert_eq!(
        text(&data(&file, &["select", "--page", "1", "--at", &at, "--unit", "line"])),
        "The quick brown fox"
    );
    assert_eq!(
        text(&data(&file, &["select", "--page", "1", "--at", &at, "--unit", "char"])),
        "",
        "a place, and nothing selected"
    );
}

/// A page's picture at 1x, and where its ink is, as it's drawn, by what the reader's tiles come from.
fn ink_bounds(doc: &mut DocRenderer, page: u32) -> Vec<(u32, u32)> {
    let (width, height) = doc.page_size(page).unwrap();
    let (width, height) = (width.ceil() as u32, height.ceil() as u32);
    let surface = PixelSurface::create(width, height).unwrap();
    let rect = TileRect { x: 0, y: 0, width, height };
    let result = doc
        .render_tile(
            &TileRequest {
                page,
                rect,
                scale: 1.0,
                stride: surface.stride(),
                generation: 1,
            },
            &surface,
        )
        .unwrap();
    assert!(matches!(result, TileResult::Done(_)));
    let stride = surface.stride() as usize;
    surface
        .with_pixels(|pixels| {
            let mut ink = Vec::new();
            for y in 0..height as usize {
                for x in 0..width as usize {
                    if pixels[y * stride + x * 4 + 1] < 128 {
                        ink.push((x as u32, y as u32));
                    }
                }
            }
            ink
        })
        .unwrap()
}

fn covers(rects: &[Rect], x: f32, y: f32, slack: f32) -> bool {
    rects
        .iter()
        .any(|rect| x >= rect.x - slack && x <= rect.x + rect.width + slack && y >= rect.y - slack && y <= rect.y + rect.height + slack)
}

/// Failure: on a page with a rotation or a crop, the highlight lands away from the words, because the worker's boxes are in the page's own coordinates and the reader sees them moved and turned. Checked against what's really drawn, so a sum that's wrong in the same way as this test's sum can't pass: every dark pixel is under a rectangle, and the rectangles don't reach far past the ink.
#[test]
fn on_a_rotated_or_cropped_page_the_highlight_covers_the_ink() {
    let lines = text_lines(&["Rotated words on a page", "with a second line under it"]);
    for extra in [
        "",
        "/Rotate 90",
        "/Rotate 180",
        "/Rotate 270",
        "/CropBox [50 300 500 760]",
        "/CropBox [50 300 500 760] /Rotate 90",
        "/CropBox [50 300 500 760] /Rotate 270",
        "/Rotate -90",
        "/Rotate 450",
    ] {
        let page = Page {
            content: lines.clone(),
            page_extra: extra.to_string(),
            media: (612.0, 792.0),
        };
        let file = TempPdf::new("turned.pdf", &pages(&[page]));
        let mut doc = DocRenderer::new(
            &file.0,
            None,
            ServiceConfig::for_worker(Path::new(common::bin()).with_file_name("recto-render-service")).unwrap(),
            Policy::default(),
        )
        .unwrap();
        let characters = doc.page_characters(0, true, true).unwrap();
        let text = PageText::build(
            &characters.chars,
            &characters.boxes,
            characters.frame,
            characters.hidden_checked,
            characters.copy_restricted,
        );
        let (width, height) = doc.page_size(0).unwrap();
        assert_eq!(text.size(), (width, height), "`{extra}`: the frame says the size the page is drawn at");
        let shapes = text.rects(0, PAGE_END);
        assert!(!shapes.is_empty(), "`{extra}`");
        let ink = ink_bounds(&mut doc, 0);
        assert!(ink.len() > 100, "`{extra}`: the page drew nothing");
        for &(x, y) in &ink {
            assert!(covers(&shapes, x as f32, y as f32, 2.0), "`{extra}`: ink at {x},{y} is outside {shapes:?}");
        }
        for rect in &shapes {
            let near = |x: f32, y: f32| {
                ink.iter()
                    .any(|&(ink_x, ink_y)| (ink_x as f32 - x).abs() < 12.0 && (ink_y as f32 - y).abs() < 12.0)
            };
            assert!(
                near(rect.x, rect.y + rect.height / 2.0) && near(rect.x + rect.width, rect.y + rect.height / 2.0)
                    || near(rect.x + rect.width / 2.0, rect.y) && near(rect.x + rect.width / 2.0, rect.y + rect.height),
                "`{extra}`: {rect:?} is away from the ink"
            );
        }
        // A click on a rectangle's middle takes the line that rectangle is for, whichever comes first in the file.
        let mut found = Vec::new();
        for shape in &shapes {
            let (start, end) = text.unit_at(shape.x + shape.width / 2.0, shape.y + shape.height / 2.0, Unit::Line).unwrap();
            assert_eq!(text.rects(start, end), [*shape], "`{extra}`");
            found.push(text.text_of(start, end));
        }
        found.sort();
        assert_eq!(found, ["Rotated words on a page", "with a second line under it"], "`{extra}`");
    }
}

/// Failure: a scan, with no text layer, is an error, or a click on it finds something, or the whole-document copy stops at it.
#[test]
fn a_page_with_no_text_selects_nothing_without_an_error() {
    let scan = Page::new(b"0.5 g 100 100 300 300 re f".to_vec());
    let file = TempPdf::new(
        "scan.pdf",
        &pages(&[Page::new(text_lines(&["Before the scan"])), scan, Page::new(text_lines(&["After the scan"]))]),
    );
    let page = data(&file, &["select", "2"]);
    assert_eq!(text(&page), "");
    assert_eq!(rects(&page, 0).len(), 0);
    assert!(page["note"].as_str().unwrap().contains("no text"), "{page}");
    let click = data(&file, &["select", "--page", "2", "--at", "200,300"]);
    assert_eq!(text(&click), "");
    // A drag that starts on the scan and ends past it.
    let after = rects(&data(&file, &["select", "3"]), 0)[0];
    let drag = data(
        &file,
        &[
            "select",
            "--page",
            "2",
            "--at",
            "200,300",
            "--to-page",
            "3",
            "--to",
            &format!("{},{}", after[0] + after[2], after[1] + after[3] / 2.0),
        ],
    );
    assert_eq!(text(&drag), "After the scan");
    assert_eq!(text(&data(&file, &["copy", "--all"])), "Before the scan\nAfter the scan");
}

/// Failure: text nobody can see goes to the clipboard. And the answer has to say so, in a count of what went, for the owner's "truthful" to hold.
#[test]
fn hidden_text_is_left_out_of_a_copy_and_counted() {
    let file = TempPdf::new("hidden.pdf", &pages(&[Page::new(hidden_text_page())]));
    let copied = data(&file, &["copy", "1"]);
    assert!(
        text(&copied).contains("VISIBLE-SENTINEL") && text(&copied).contains("VISIBLE-AFTER"),
        "{copied}"
    );
    assert!(!text(&copied).contains("HIDDEN-"), "{copied}");
    assert!(copied["hidden_left_out"].as_u64().unwrap() > 50, "{copied}");
    assert_eq!(copied["pages_unchecked"], 0);
    // The same words as `recto text`, which keeps them and marks them.
    let read = data(&file, &["text", "1"]);
    assert!(text(&read).contains("HIDDEN-WHITE"), "`text` still shows them: {read}");
}

/// A file anyone can read and that asks not to be copied.
#[allow(clippy::disallowed_methods)] // A test fixture built with lopdf's eager writer, which alone encrypts or rebuilds; never over a user's file.
fn restricted() -> Vec<u8> {
    let bytes = pages(&[Page::new(text_lines(&["Words that ask not to be copied"]))]);
    let mut document = lopdf::Document::load_mem(&bytes).unwrap();
    document.trailer.set(
        "ID",
        lopdf::Object::Array(vec![
            lopdf::Object::string_literal("0123456789abcdef"),
            lopdf::Object::string_literal("0123456789abcdef"),
        ]),
    );
    let state = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &document,
        owner_password: "owner",
        user_password: "",
        key_length: 128,
        permissions: Permissions::all() - Permissions::COPYABLE,
    })
    .unwrap();
    document.encrypt(&state).unwrap();
    let mut out = Vec::new();
    document.save_to(&mut out).unwrap();
    out
}

/// Failure: the permission is ignored, or a refusal takes `select`'s rectangles with it, or `--anyway` doesn't get past it. The worker's own reading of the flag, on a really encrypted file.
#[test]
fn a_file_that_asks_not_to_be_copied_says_so_and_can_be_copied_anyway() {
    let file = TempPdf::new("restricted.pdf", &restricted());
    let (reply, code) = recto(&file, &["copy", "1"]);
    assert_eq!(code, recto_agent::exit::PROTECTED, "{reply}");
    assert_eq!(reply["error"]["code"], "copy_restricted");
    let shown = data(&file, &["select", "1"]);
    assert_eq!(shown["restricted"], true, "{shown}");
    assert!(shown["untrusted"]["text"].is_null(), "{shown}");
    assert_eq!(rects(&shown, 0).len(), 1, "where it shows is still said");
    let anyway = data(&file, &["copy", "1", "--anyway"]);
    assert_eq!(text(&anyway), "Words that ask not to be copied");
    let plain = TempPdf::new("plain.pdf", &pages(&[Page::new(text_lines(&["Ordinary words"]))]));
    assert_eq!(data(&plain, &["copy", "1"])["restricted"], false, "a file with no encryption isn't");
}

/// Failure: a wrong pick does something other than refuse, as an end before the start or a page off the end would.
#[test]
fn a_pick_that_makes_no_sense_is_refused() {
    let file = TempPdf::new("one.pdf", &pages(&[Page::new(text_lines(&["Words"]))]));
    for (args, code) in [
        (&["select"][..], recto_agent::exit::USAGE),
        (&["select", "--page", "2"][..], recto_agent::exit::NOT_FOUND),
        (&["select", "--page", "1", "--to-page", "9"][..], recto_agent::exit::NOT_FOUND),
        (&["select", "--page", "1", "--chars", "5-2"][..], recto_agent::exit::USAGE),
        (&["select", "--page", "1", "--at", "nan,1"][..], recto_agent::exit::USAGE),
        (&["select", "--page", "1", "--at", "1"][..], recto_agent::exit::USAGE),
        (&["select", "--page", "1", "--to", "1,1"][..], recto_agent::exit::USAGE),
        (&["select", "--page", "1", "--chars", "1-2", "--at", "1,1"][..], recto_agent::exit::USAGE),
        (&["copy", "--all", "--page", "1"][..], recto_agent::exit::USAGE),
    ] {
        let (reply, got) = recto(&file, args);
        assert_eq!(got, code, "{args:?}: {reply}");
    }
    assert_eq!(
        text(&data(&file, &["select", "--page", "1", "--chars", "3-999"])),
        "ds",
        "an end past the text is the end of the text"
    );
}

/// Failure: the end of a whole page reads as character 4,294,967,294 in an answer an agent goes by.
#[test]
fn a_whole_page_ends_at_its_own_length() {
    let file = TempPdf::new("one.pdf", &pages(&[Page::new(text_lines(&["Words"])), Page::new(text_lines(&["More words"]))]));
    let answer = data(&file, &["select", "2"]);
    assert_eq!(answer["end"]["page"], 2);
    assert_eq!(answer["end"]["at"], answer["pages"][0]["end"], "{answer}");
    assert!(answer["end"]["at"].as_u64().unwrap() < 100, "{answer}");
    let copied = data(&file, &["copy", "--all"]);
    assert!(copied["end"]["at"].as_u64().unwrap() < 100, "{copied}");
}
