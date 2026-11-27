//! Generated contents through the real thing: `recto-headings` walled in `RectoStructure.app`, started by the session's runner, and the check for hidden text in the session. What only
//! the real helper and the real render worker show; what the runner decides alone is `src/structure/tests.rs`.
//!
//! The helper is laid out where `tools/check.sh` lays it (`target/Helpers`), and a test that can't find it says so and stops: there is no fallback to a bare one, in a test or anywhere.
#![cfg(target_os = "macos")]

use recto_agent::generated::{GenState, Generating};
use recto_control::generating::{GenJob, Generator, Outcome, Runner, Walled};
use recto_index::corpus::CorpusKey;
use recto_index::generated::{self, Generated};
use recto_testpdf::TempPdf;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// `target`: three folders up from this test binary (`target/debug/deps/structure-…`).
fn target() -> PathBuf {
    let this = std::env::current_exe().unwrap().canonicalize().unwrap();
    this.ancestors().nth(3).expect("a test binary in target/debug/deps").to_path_buf()
}

/// The walled structure helper, laid out by `tools/check.sh`.
fn walled_helper() -> PathBuf {
    let exe = target().join("Helpers/RectoStructure.app/Contents/MacOS/recto-headings");
    assert!(
        exe.is_file(),
        "run tools/check.sh: it lays the structure helper out at target/Helpers ({})",
        exe.display()
    );
    exe
}

fn job_for(pdf: &TempPdf, pages: u32) -> GenJob {
    GenJob {
        path: pdf.0.clone(),
        key: CorpusKey::of_file(&pdf.0).unwrap(),
        pages,
    }
}

/// The runner on a document, to its end.
fn make(pdf: &TempPdf, pages: u32) -> (Outcome, Value) {
    Walled::at(walled_helper()).run(&job_for(pdf, pages), &AtomicBool::new(false))
}

fn ready(outcome: Outcome, report: &Value) -> Generated {
    match outcome {
        Outcome::Ready(made) => *made,
        other => panic!("not ready: {other:?} {report}"),
    }
}

const SIX: [&str; 6] = ["1 Introduction", "2 Background", "3 Method", "4 Results", "5 Discussion", "6 Conclusion"];

/// Failure: a heading set so no one can see it reaches the generated contents. Every way in `recto_testpdf::HIDDEN_HEADINGS`, including the four a review got through: no width (`0 Tz`),
/// invisible glyphs read as a space by `/ActualText`, white text over a paragraph, and white text whose words are also in a visible speck in a corner. Through the walled helper and the
/// check here, which since S6b is where the render worker runs. And every visible heading stays, and what was found is what's kept and what was left out as hidden.
#[test]
fn no_way_of_hiding_a_heading_gets_it_into_the_contents() {
    for (name, hidden) in recto_testpdf::HIDDEN_HEADINGS {
        let pdf = TempPdf::new(
            &format!("{name}.pdf"),
            &recto_testpdf::headings_document_with(hidden, [0.0, 0.0, 612.0, 792.0], ""),
        );
        let (outcome, report) = make(&pdf, 6);
        let generated = ready(outcome, &report);
        let titles: Vec<&str> = generated.entries.iter().map(|entry| entry.title.as_str()).collect();
        assert!(titles.iter().all(|title| !title.contains("HIDDEN")), "{name}: {titles:?}");
        assert_eq!(titles, SIX, "{name}");
        let pages: Vec<u32> = generated.entries.iter().map(|entry| entry.page).collect();
        assert_eq!(pages, [0, 1, 2, 3, 4, 5], "{name}");
        assert!(generated.entries.iter().all(|entry| entry.place < 200), "{name}: {:?}", generated.entries);
        // distillPDF may take a hidden line for a heading, and then it's left out; should it stop finding them, nothing's left out, and this still holds.
        assert_eq!(
            report["found"].as_u64(),
            Some(titles.len() as u64 + u64::from(generated.hidden_dropped)),
            "{name}: {report}"
        );
        assert_eq!(report["walled"], true, "{name}: the helper was shown walled before it had its job: {report}");
    }
}

/// Failure: on a page whose media box doesn't start at 0, every heading is put at the top of its page, so going to one lands in the wrong place.
#[test]
fn place_is_right_on_a_media_box_that_doesnt_start_at_zero() {
    let pdf = TempPdf::new("moved.pdf", &recto_testpdf::headings_document_with("", [0.0, 792.0, 612.0, 1584.0], ""));
    let (outcome, report) = make(&pdf, 6);
    let generated = ready(outcome, &report);
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
    let (outcome, report) = make(&pdf, 6);
    let titles: Vec<String> = ready(outcome, &report).entries.into_iter().map(|entry| entry.title).collect();
    assert_eq!(titles, SIX, "{report}");
}

/// Failure: a file replaced after it was asked for gets the old version's headings, or the new one's under the old key. The runner opens the document itself and compares its key with the one
/// it was asked for, so nothing is started for a file that isn't that version.
#[test]
fn a_file_changed_after_it_was_asked_for_gets_nothing() {
    let pdf = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    let job = job_for(&pdf, 6);
    std::fs::write(&pdf.0, recto_testpdf::text_lines(&["not even a PDF now"])).unwrap();
    let (outcome, report) = Walled::at(walled_helper()).run(&job, &AtomicBool::new(false));
    assert!(matches!(outcome, Outcome::Changed), "{outcome:?}");
    assert_eq!(
        report,
        Value::Null,
        "a helper was started on a file that wasn't the version asked for: {report}"
    );
}

/// Failure: the session falls back to an unwalled helper, or accepts one, so a hostile file is read with no walls. The bare `recto-headings` of `target/debug` says it isn't walled (it has no
/// bundle to take walls from), and is killed before it has its job: the answer is `structure_not_sandboxed`, in a debug build and a release one alike.
#[test]
fn an_unwalled_helper_is_refused_and_reads_nothing() {
    let bare = target().join("debug/recto-headings");
    assert!(
        bare.is_file(),
        "run tools/check.sh: it builds the bare helper at target/debug ({})",
        bare.display()
    );
    let pdf = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    let (outcome, report) = Walled::at(bare).run(&job_for(&pdf, 6), &AtomicBool::new(false));
    assert!(matches!(&outcome, Outcome::Failed(code) if code == "structure_not_sandboxed"), "{outcome:?}");
    assert_eq!(report["walled"], Value::Null, "never admitted: {report}");
}

const INSIDE: &str = "RECTO_STRUCTURE_INSIDE_ANOTHER";

/// Failure: `recto` run inside an agent's sandbox can't start the structure helper, and says this copy is signed wrong, sending the user to reinstall for nothing. This test runs itself again
/// under `sandbox-exec` with a profile that allows everything, which is sandboxed all the same, and there the walled helper can't start: the system ends it before its first line. The runner says
/// it's the sandbox it's in.
#[test]
fn inside_another_sandbox_it_says_so() {
    if let Some(helper) = std::env::var_os(INSIDE) {
        assert!(recto_platform::wall::in_a_sandbox(), "sandbox-exec didn't sandbox this run");
        let pdf = TempPdf::new("inside.pdf", &recto_testpdf::headings_document());
        let (outcome, _) = Walled::at(helper.into()).run(&job_for(&pdf, 6), &AtomicBool::new(false));
        let Outcome::Failed(code) = outcome else {
            panic!("not a failure: {outcome:?}")
        };
        assert_eq!(code, "structure_inside_another_sandbox");
        let words = recto_agent::generated::failure_words(&code);
        assert!(words.contains("inside another sandbox") && !words.contains("signed wrong"), "{words}");
        return;
    }
    let output = std::process::Command::new("/usr/bin/sandbox-exec")
        .args(["-p", "(version 1)(allow default)"])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", "inside_another_sandbox_it_says_so", "--nocapture", "--test-threads=1"])
        .env(INSIDE, walled_helper())
        .output()
        .unwrap();
    let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success() && said.contains("1 passed"), "inside the seatbelt: {said}");
}

/// Failure: a job cancelled part way writes what it had, or leaves the helper running to the end of its time. The form bomb keeps the helper busy for seconds; the generator is asked, the
/// job is cancelled once it's running, and nothing is on disk for it and the process has no child left.
#[test]
fn cancelled_part_way_nothing_is_written() {
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-structure-cancel-");
    let pdf = TempPdf::new("bomb.pdf", &recto_testpdf::form_bomb());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let generator = Generator::with_runner(&root, Box::new(Walled::at(walled_helper())));
    generator.want(&pdf.0, key, 1);
    let until = Instant::now() + Duration::from_secs(30);
    while generator.state(key) != GenState::Running {
        assert!(Instant::now() < until, "it never started");
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
    // Cancelled with the helper started, so it's the kill of a live helper that's seen.
    let children = || {
        String::from_utf8_lossy(
            &std::process::Command::new("/usr/bin/pgrep")
                .args(["-P", &std::process::id().to_string()])
                .output()
                .unwrap()
                .stdout,
        )
        .trim()
        .to_string()
    };
    while children().is_empty() {
        assert!(Instant::now() < until, "the helper never started");
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
    generator.cancel(Some(key));
    while generator.making(None) {
        assert!(Instant::now() < until, "it didn't stop when cancelled");
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
    assert!(generated::read(&root, &pdf.0, key).is_none(), "something was written for a cancelled job");
    assert!(generator.held(key).is_none());
    // Its child is collected: the runner never leaves a helper behind.
    assert_eq!(children(), "", "children left");
}
