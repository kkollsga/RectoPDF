//! The helper's `blocks` job: the lines it writes, read back as the session reads them (`recto_index::blocks::map::PageLines`), and the real helper process on generated documents, run bare as `tests/helper.rs` does. The walled run is `tests/sandbox.rs`.

use distillpdf::model::{Block, BlockKind};
use distillpdf::pages::BlockContinuation;
use recto_headings::blocks::{Out, end_line, kind_name, out_of, page_line, said_of, scan_of};
use recto_headings::{Failure, Scan};
use recto_index::blocks::map::{MAX_LINE_BLOCKS, MAX_LINE_BYTES, PageLines, Shape};
use recto_platform::helper::{self, Priority};
use recto_testpdf::TempPdf;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::ExitStatus;
#[path = "support/hello.rs"]
mod hello;
use hello::after_hello;

/// A block as distillPDF's own model reads it, so what the helper does to its text is what it does to the real ones.
fn block(kind: &str, text: &str, extra: Value) -> Block {
    let mut value = json!({ "id": "b1", "kind": kind, "text": text, "page": 1, "confidence": 1.0, "bbox": [10.0, 20.0, 30.0, 40.0] });
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

fn out(text: &str) -> Out {
    Out {
        kind: "para",
        level: 0,
        bbox: None,
        text: text.into(),
        continues: false,
    }
}

/// Failure: a kind is said under a name the session doesn't read (ListItem as `list`), a table or figure carries its cells or markup, a carrier of the page's own markup is sent as content, markup reaches the session as words, or a block that goes on to the next page loses that. Every kind goes through the line the helper writes and the reader the session uses, and comes back as sent.
#[test]
fn every_kind_reads_back_as_the_session_reads_it() {
    let continued = [BlockContinuation { block: 6, page: 2, bbox: None }];
    let cells = json!({ "cells": [["secret cell", "b"]], "el_html": "<svg>markup</svg>" });
    let blocks = [
        block("para", "Plain <b>bold</b> &amp; more", json!({})),
        block("heading", "<b>1. Intro</b>", json!({ "heading_level": 3 })),
        block("heading", "Deep", json!({ "heading_level": 40 })),
        block("list_item", "an item", json!({})),
        block("caption", "Figure 1. A caption", json!({})),
        block("code", "let x = 1;", json!({})),
        block("footnote", "1 a note", json!({})),
        block("para", "runs on", json!({})),
        block("table", "table text", cells.clone()),
        block("figure", "figure text", cells),
        block("header", "front matter", json!({})),
        block("dest_anchors", "anchors", json!({})),
        block("para", "no box", json!({ "bbox": null })),
    ];
    let outs: Vec<Out> = blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| out_of(block, 1, index, &continued))
        .collect();
    assert_eq!(outs.len(), 11, "header and dest_anchors are never sent: {outs:?}");
    let (line, sent, cut) = page_line(4, &outs);
    assert_eq!((sent, cut), (11, 0));
    let read = PageLines::new(5).read(line.as_bytes()).expect("the session reads what the helper wrote");
    assert_eq!((read.page, read.unreadable, read.candidates.len()), (4, 0, 11));
    let shapes: Vec<Shape> = read.candidates.iter().map(|candidate| candidate.shape).collect();
    assert_eq!(
        shapes,
        [
            Shape::Paragraph,
            Shape::Heading,
            Shape::Heading,
            Shape::ListItem,
            Shape::Caption,
            Shape::Code,
            Shape::Footnote,
            Shape::Paragraph,
            Shape::Table,
            Shape::Figure,
            Shape::Paragraph
        ]
    );
    let texts: Vec<&str> = read.candidates.iter().map(|candidate| candidate.text.as_str()).collect();
    assert_eq!(texts[0], "Plain bold & more");
    assert_eq!(texts[1], "1. Intro");
    assert_eq!((texts[8], texts[9]), ("", ""), "a table's cells and a figure's markup are never sent");
    assert_eq!(
        read.candidates.iter().map(|candidate| candidate.level).collect::<Vec<_>>(),
        [0, 3, 9, 0, 0, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(read.candidates[0].bbox, [10.0, 20.0, 30.0, 40.0]);
    assert_eq!(read.candidates[10].bbox, [0.0; 4], "no box is said as null and read as none");
    // Block 6 of the page (index in the list given to `out_of`) goes on to page 2: the footnote.
    let goes_on: Vec<usize> = read
        .candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.continues)
        .map(|(at, _)| at)
        .collect();
    assert_eq!(goes_on, [6]);
    assert_eq!((kind_name(BlockKind::Header), kind_name(BlockKind::DestAnchors)), (None, None));
    assert_eq!(kind_name(BlockKind::ListItem), Some("list_item"));
}

/// Failure: a box with a side that isn't a number is written into a line (which isn't JSON), or a continuation that points back is taken as one that goes on.
#[test]
fn a_box_that_isnt_finite_is_not_said_and_only_a_later_page_continues() {
    let mut odd = block("para", "x", json!({}));
    odd.bbox = Some([0.0, f32::NAN, 5.0, f32::INFINITY]);
    let back = [BlockContinuation { block: 0, page: 1, bbox: None }];
    let made = out_of(&odd, 1, 0, &back).unwrap();
    assert_eq!((made.bbox, made.continues), (None, false));
    let (line, _, _) = page_line(0, &[made]);
    assert!(serde_json::from_str::<Value>(&line).is_ok(), "{line}");
}

/// Failure: a page's line goes over the session's cap and is refused whole (so a busy page loses all its blocks), a block over half the cap stops the blocks after it, the closing `]}` isn't allowed for and the line is a byte over, or the count of what was left out is wrong. Two blocks and a third that fills the line exactly are kept; one byte more is cut; a block over half the cap is skipped and a later one still goes; 4097 small blocks are cut to 4096.
#[test]
fn a_page_line_is_cut_to_the_cap_and_counts_what_it_left_out() {
    let overhead = page_line(0, &[out("")]).0.len() - page_line(0, &[]).0.len();
    let two = page_line(0, &[out(&"a".repeat(200_000)), out(&"b".repeat(200_000))]).0.len();
    let room = MAX_LINE_BYTES - two - 1 - overhead;
    let fill = |extra: usize| [out(&"a".repeat(200_000)), out(&"b".repeat(200_000)), out(&"c".repeat(room + extra))];
    let (line, sent, cut) = page_line(0, &fill(0));
    assert_eq!((line.len(), sent, cut), (MAX_LINE_BYTES, 3, 0), "a block that fills the line exactly is kept");
    let (line, sent, cut) = page_line(0, &fill(1));
    assert_eq!((sent, cut), (2, 1), "one byte more is cut");
    assert!(line.len() <= MAX_LINE_BYTES);
    PageLines::new(1).read(line.as_bytes()).expect("what is sent is what the session takes");
    // Over half the cap: skipped, and the ones after it still go. The one after a cut-off block doesn't.
    let huge = out(&"h".repeat(MAX_LINE_BYTES / 2));
    let (line, sent, cut) = page_line(0, &[out("one"), huge, out("two")]);
    assert_eq!((sent, cut), (2, 1));
    assert!(line.contains("one") && line.contains("two") && !line.contains("hhhh"));
    let (_, sent, cut) = page_line(0, &(0..=MAX_LINE_BLOCKS).map(|_| out("x")).collect::<Vec<_>>());
    assert_eq!((sent, cut), (MAX_LINE_BLOCKS, 1));
    let given = MAX_LINE_BLOCKS + 50;
    let (_, sent, cut) = page_line(0, &(0..given).map(|_| out("x")).collect::<Vec<_>>());
    assert_eq!(sent + cut, given);
}

/// Failure: the end line carries words, or a name the session reads as its own, or loses a count the session checks the page lines against.
#[test]
fn the_end_line_is_numbers_the_session_checks() {
    let end: Value = serde_json::from_str(&end_line(3, 1, 11, 2, 40, Some(1 << 20))).unwrap();
    assert_eq!(
        end,
        json!({ "state": "ready", "pages": 3, "pages_failed": 1, "blocks": 11, "cut": 2, "distilled_ms": 40, "helper_peak_bytes": 1 << 20 })
    );
}

/// Failure: the blocks job gets the slower full scan unless asked, or the headings job's default changes with it.
#[test]
fn typography_is_the_blocks_default() {
    assert_eq!(scan_of(&json!({ "kind": "blocks" })), Scan::Typography);
    assert_eq!(scan_of(&json!({ "kind": "blocks", "scan": "typography" })), Scan::Typography);
    assert_eq!(scan_of(&json!({ "kind": "blocks", "scan": "full" })), Scan::Full);
    assert_eq!(scan_of(&json!({ "kind": "blocks", "scan": 5 })), Scan::Typography);
}

/// What the helper said to one `blocks` job after its hello, and how it ended.
struct Said {
    lines: Vec<Value>,
    raw: Vec<String>,
    status: Option<ExitStatus>,
}

fn run(path: &std::path::Path) -> Said {
    let document = std::fs::File::open(path).unwrap();
    let helper::Spawned {
        mut child,
        mut stdin,
        mut stdout,
    } = helper::spawn_with_document(
        std::path::Path::new(env!("CARGO_BIN_EXE_recto-headings")),
        &[("MallocLargeCache", std::ffi::OsStr::new("0"))],
        Priority::Inherited,
        &document,
    )
    .unwrap();
    writeln!(stdin, "{}", json!({ "kind": "blocks", "memory_limit": 512u64 << 20, "scan": "typography" })).unwrap();
    let mut said = String::new();
    stdout.read_to_string(&mut said).unwrap();
    drop(stdin);
    let status = child.wait().ok();
    let raw: Vec<String> = after_hello(&said).lines().map(str::to_owned).collect();
    let lines = raw.iter().map(|line| serde_json::from_str(line).unwrap_or(Value::Null)).collect();
    Said { lines, raw, status }
}

/// Failure: the job says a page twice or out of order, says a page the session can't read, gives an end that doesn't add up with its lines, says nothing about a kind (a document of paragraphs, a heading and a list gives none of them), or goes past the helper's memory limit. On the three-page blocks document: three page lines, `0,1,2`, each readable, and one end whose `blocks` is the sum.
#[test]
fn the_blocks_job_says_every_page_once_in_order_and_ends() {
    let pdf = TempPdf::new("blocks.pdf", &recto_testpdf::blocks::blocks_document());
    let said = run(&pdf.0);
    assert!(said.status.is_some_and(|status| status.success()), "{:?}", said.status);
    let (end, pages) = said.lines.split_last().expect("it said something");
    assert_eq!(pages.len(), 3, "{:?}", said.lines);
    let mut reader = PageLines::new(3);
    let mut sum = 0;
    let mut kinds = std::collections::BTreeSet::new();
    for (at, line) in said.raw[..3].iter().enumerate() {
        let read = reader.read(line.as_bytes()).unwrap_or_else(|error| panic!("page {at}: {error:?}: {line}"));
        assert_eq!(read.page, at as u32);
        assert_eq!(read.unreadable, 0);
        sum += read.candidates.len() as u64;
        kinds.extend(read.candidates.iter().map(|candidate| candidate.shape.name()));
    }
    let mut tables = 0;
    for line in pages {
        for block in line["blocks"].as_array().unwrap() {
            if matches!(block["kind"].as_str(), Some("table" | "figure")) {
                tables += 1;
                assert_eq!(block["text"], "", "a table's cells and a figure's markup are never sent: {block}");
            }
        }
    }
    assert!(tables > 0, "the document has a table");
    assert_eq!(end["state"], "ready", "{end}");
    assert_eq!(
        (end["pages"].as_u64(), end["pages_failed"].as_u64(), end["cut"].as_u64()),
        (Some(3), Some(0), Some(0)),
        "{end}"
    );
    assert_eq!(end["blocks"].as_u64(), Some(sum), "the end counts what the page lines held");
    for want in ["paragraph", "heading", "list_item"] {
        assert!(kinds.contains(want), "no {want} among {kinds:?}");
    }
    assert!(end["helper_peak_bytes"].as_u64().is_some_and(|peak| peak < 512 << 20), "{end}");
}

/// Failure: a hostile page ends the job instead of being said as a page with no blocks: a form that draws itself over and over, all fifteen ways of hiding a heading on one document, or a page of hidden text. Each ends `ready` and exits cleanly, with a line for every page.
#[test]
fn hostile_documents_end_with_an_answer() {
    let bomb = TempPdf::new("bomb.pdf", &recto_testpdf::form_bomb());
    let said = run(&bomb.0);
    assert!(said.status.is_some_and(|status| status.success()), "{:?}", said.status);
    assert_eq!(said.lines.len(), 3, "two page lines and an end: {:?}", said.lines);
    assert_eq!(said.lines[0], json!({ "page": 0, "blocks": [] }), "the bomb's page has no blocks");
    assert_eq!(said.lines[2]["state"], "ready");
    assert_eq!(said.lines[2]["pages"], 2);

    let every_way: String = recto_testpdf::HIDDEN_HEADINGS.iter().map(|(_, drawing)| *drawing).collect();
    let hidden = TempPdf::new(
        "hidden-headings.pdf",
        &recto_testpdf::headings_document_with(&every_way, [0.0, 0.0, 612.0, 792.0], ""),
    );
    let said = run(&hidden.0);
    assert!(said.status.is_some_and(|status| status.success()), "{:?}", said.status);
    assert_eq!(
        said.lines.last().map(|end| end["state"].clone()),
        Some(json!("ready")),
        "{:?}",
        said.lines.last()
    );
    assert_eq!(said.lines.len(), 7, "six pages and an end");

    let page = TempPdf::new(
        "hidden-text.pdf",
        &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::hidden_text_page())]),
    );
    let said = run(&page.0);
    assert_eq!(said.lines.last().map(|end| end["state"].clone()), Some(json!("ready")), "{:?}", said.lines);
}

/// Failure: a page distillPDF can't make ends the whole job (so one bad page in a book gives the book no blocks), is left out of the answer (so pages after it shift), or a document that changed under the read is carried on with. A page error is a line for that page with no blocks and a mark in the count; stale statistics or a cancelled walk end the job as changed.
#[test]
fn a_page_that_cant_be_made_is_an_empty_line_and_only_a_changed_file_ends_the_job() {
    let said = said_of(3, Err(distillpdf::Error::NoPage(Some(3)))).expect("a page never ends the job");
    assert_eq!(
        (said.line.as_str(), said.failed, said.sent, said.cut),
        ("{\"page\":2,\"blocks\":[]}", true, 0, 0)
    );
    for error in [distillpdf::Error::StaleStats("moved".into()), distillpdf::Error::Cancelled] {
        assert!(matches!(said_of(1, Err(error)), Err(Failure::Changed)));
    }
}

/// Failure: whether a block goes on is judged against the block's own `page` field, which distillPDF fills as it likes, instead of the page the line is for: a block whose field says another page, with a continuation to the next page, is read as not continuing (or a block on the last page as continuing).
#[test]
fn a_block_continues_by_the_page_the_line_is_for_not_by_its_own_field() {
    let mut odd = block("para", "x", json!({}));
    odd.page = 9;
    let on_to_three = [BlockContinuation { block: 0, page: 3, bbox: None }];
    assert!(
        out_of(&odd, 2, 0, &on_to_three).unwrap().continues,
        "page 2 goes on to page 3 whatever the block says it is on"
    );
    assert!(!out_of(&odd, 3, 0, &on_to_three).unwrap().continues, "page 3 doesn't go on to page 3");
}

/// Failure: when the helper can't read its own footprint the end line says `"helper_peak_bytes":null`, and the session, which reads only numbers and bools from it, refuses the whole answer for a number that was only for measuring. The key is left out, for the blocks job and the headings one alike.
#[test]
fn an_end_line_without_a_peak_has_no_peak() {
    let blocks: Value = serde_json::from_str(&end_line(3, 0, 5, 0, 40, None)).unwrap();
    assert!(blocks.get("helper_peak_bytes").is_none(), "{blocks}");
    assert_eq!(blocks["pages"], 3);
    let headings = recto_index::generated::visible::Headings {
        pages: 6,
        ..Default::default()
    };
    let said: Value = serde_json::from_str(&recto_headings::answer::end_line(&headings, 0, false, 40, None)).unwrap();
    assert!(said.get("helper_peak_bytes").is_none(), "{said}");
    assert_eq!(said["pages"], 6);
}
