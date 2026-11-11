//! The real helper process, on generated PDFs, the way the session runs it: a job on stdin, two lines back on stdout. The render worker has to be built beside it, as `tools/check.sh` does.

use recto_index::corpus::CorpusKey;
use recto_index::generated::Generated;
use recto_testpdf::TempPdf;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run(path: &std::path::Path, key: CorpusKey, close_stdin: bool) -> (Value, Option<Generated>, Duration) {
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_recto-headings"))
        .env_clear()
        .env("MallocLargeCache", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{}", json!({ "file": path, "key": key.hex(), "memory_limit": 512u64 << 20 })).unwrap();
    if close_stdin {
        drop(stdin);
    } else {
        std::mem::forget(stdin);
    }
    let mut said = String::new();
    child.stdout.take().unwrap().read_to_string(&mut said).unwrap();
    let _ = child.wait();
    let mut lines = said.lines();
    let report: Value = lines.next().and_then(|line| serde_json::from_str(line).ok()).unwrap_or(Value::Null);
    let generated = lines.next().map(|line| Generated::decode(line.as_bytes(), key).unwrap());
    (report, generated, started.elapsed())
}

/// Failure: headings in white text or in render mode 3, set as large as the real ones, show in the generated contents, where a reader or an agent would take them for part of the document. And the real ones must all be there, at their pages.
#[test]
fn hidden_headings_never_reach_the_contents_and_visible_ones_do() {
    let pdf = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let (report, generated, _) = run(&pdf.0, key, false);
    assert_eq!(report["state"], "ready", "{report}");
    let generated = generated.expect("contents");
    let titles: Vec<(&str, u32)> = generated.entries.iter().map(|entry| (entry.title.as_str(), entry.page)).collect();
    assert!(titles.iter().all(|(title, _)| !title.contains("HIDDEN")), "{titles:?}");
    for (page, title) in ["1 Introduction", "2 Background", "3 Method", "4 Results", "5 Discussion", "6 Conclusion"]
        .iter()
        .enumerate()
    {
        assert!(titles.contains(&(title, page as u32)), "{title} missing from {titles:?}");
    }
    // distillPDF 0.1.7 takes both hidden lines for headings, so they're found and then left out here. Should it stop finding them, nothing's left out, and this still holds.
    assert_eq!(
        report["found"].as_u64(),
        Some(titles.len() as u64 + u64::from(generated.hidden_dropped)),
        "{report}"
    );
    assert!(
        generated.entries.iter().all(|entry| entry.place < 200),
        "headings are near the top of their pages: {:?}",
        generated.entries
    );
}

/// Failure: a file replaced after it was asked for gets the old version's headings, or the new one's under the old key.
#[test]
fn a_file_that_isnt_the_one_asked_for_gets_nothing() {
    let pdf = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    std::fs::write(&pdf.0, recto_testpdf::text_lines(&["not even a PDF now"])).unwrap();
    let (report, generated, _) = run(&pdf.0, key, false);
    assert_eq!(report["state"], "changed", "{report}");
    assert!(generated.is_none());
}

/// Failure: something that isn't a PDF, or one distillPDF's bounded reader refuses, makes the helper panic or hang, rather than say it couldn't be read.
#[test]
fn what_cant_be_read_is_refused_plainly() {
    for (name, bytes) in [("junk.pdf", b"%PDF-1.7 this is not really a PDF".to_vec()), ("empty.pdf", Vec::new())] {
        let pdf = TempPdf::new(name, &bytes);
        let key = CorpusKey::of_file(&pdf.0).unwrap();
        let (report, generated, elapsed) = run(&pdf.0, key, false);
        assert_eq!(report["state"], "refused", "{name}: {report}");
        assert!(generated.is_none());
        assert!(elapsed < Duration::from_secs(30), "{elapsed:?}");
    }
}

/// Failure: a hostile page, a form that draws itself over and over, holds the helper without end, or takes its memory past any limit.
#[test]
fn a_form_bomb_ends() {
    let pdf = TempPdf::new("bomb.pdf", &recto_testpdf::form_bomb());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let (report, _, elapsed) = run(&pdf.0, key, false);
    assert!(matches!(report["state"].as_str(), Some("ready" | "refused")), "{report}");
    assert!(elapsed < Duration::from_secs(60), "{elapsed:?}");
    assert!(report["helper_peak_bytes"].as_u64().is_none_or(|peak| peak < 512 << 20), "{report}");
}

/// Failure: the helper outlives whoever started it. Its stdin closing is its sign to go, whatever it's doing.
#[test]
fn it_goes_when_its_stdin_closes() {
    let pdf = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let (_, _, elapsed) = run(&pdf.0, key, true);
    assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
}

/// Failure: a heading set so no one can see it reaches the generated contents. Every way in `recto_testpdf::HIDDEN_HEADINGS`, including the four a review got through: no width (`0 Tz`), invisible glyphs read as a space by `/ActualText`, white text over a paragraph, and white text whose words are also in a visible speck in a corner. And every visible heading stays.
#[test]
fn no_way_of_hiding_a_heading_gets_it_into_the_contents() {
    for (name, hidden) in recto_testpdf::HIDDEN_HEADINGS {
        let pdf = TempPdf::new(
            &format!("{name}.pdf"),
            &recto_testpdf::headings_document_with(hidden, [0.0, 0.0, 612.0, 792.0], ""),
        );
        let key = CorpusKey::of_file(&pdf.0).unwrap();
        let (report, generated, _) = run(&pdf.0, key, false);
        assert_eq!(report["state"], "ready", "{name}: {report}");
        let titles: Vec<String> = generated.expect("contents").entries.iter().map(|entry| entry.title.clone()).collect();
        assert!(titles.iter().all(|title| !title.contains("HIDDEN")), "{name}: {titles:?}");
        assert_eq!(
            titles,
            ["1 Introduction", "2 Background", "3 Method", "4 Results", "5 Discussion", "6 Conclusion"],
            "{name}"
        );
    }
}

/// Failure: on a page whose media box doesn't start at 0, every heading is put at the top of its page, so going to one lands in the wrong place.
#[test]
fn place_is_right_on_a_media_box_that_doesnt_start_at_zero() {
    let pdf = TempPdf::new("moved.pdf", &recto_testpdf::headings_document_with("", [0.0, 792.0, 612.0, 1584.0], ""));
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let (report, generated, _) = run(&pdf.0, key, false);
    let generated = generated.unwrap_or_else(|| panic!("{report}"));
    assert_eq!(generated.entries.len(), 6, "{generated:?}");
    // The headings' baseline is 72 points from the top of 792.
    assert!(
        generated.entries.iter().all(|entry| (60..140).contains(&entry.place)),
        "{:?}",
        generated.entries
    );
}

/// Failure: headings in heavy type, which cover more than half of each glyph's box, are taken for hidden text and left out, as a review's crafted file lost all six. Drawn with and without them, they add what anyone can see.
#[test]
fn headings_in_heavy_type_stay() {
    let pdf = TempPdf::new("bold.pdf", &recto_testpdf::bold_headings_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let (report, generated, _) = run(&pdf.0, key, false);
    let titles: Vec<String> = generated
        .unwrap_or_else(|| panic!("{report}"))
        .entries
        .iter()
        .map(|entry| entry.title.clone())
        .collect();
    assert_eq!(
        titles,
        ["1 Introduction", "2 Background", "3 Method", "4 Results", "5 Discussion", "6 Conclusion"],
        "{report}"
    );
}
