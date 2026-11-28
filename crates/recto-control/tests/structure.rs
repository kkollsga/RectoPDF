//! Generated contents through the real thing: `recto-headings` walled in `RectoStructure.app`, started by the session's runner, and the check for hidden text in the session. What only
//! the real helper and the real render worker show; what the runner decides alone is `src/structure/tests.rs`.
//!
//! The helper is laid out where `tools/check.sh` lays it (`target/Helpers`), and a test that can't find it says so and stops: there is no fallback to a bare one, in a test or anywhere.
#![cfg(target_os = "macos")]

use recto_agent::blocks as agent_blocks;
use recto_agent::blocks::BlocksAsk;
use recto_agent::generated::{GenState, Generating};
use recto_control::generating::{GenJob, Generator, Outcome, Runner, Walled};
use recto_control::structure::blocks::{self, BlocksAnswer, PageStore};
use recto_control::structure::{Answer, Ended, Said, Structure, headings};
use recto_index::blocks::PageBlocks;
use recto_index::blocks::map::Shape;
use recto_index::corpus::{CorpusKey, ReadyCorpus, StoredBlocks, build_text_corpus};
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

/// How a file is changed while the helper is reading it.
fn change_while_it_runs(pdf: &TempPdf, how: impl FnOnce(&std::path::Path)) -> Ended<headings::HeadingsAnswer> {
    let job = job_for(pdf, 6);
    let mut line = headings::job();
    // Debug builds of the helper only: it spins this long before it reads, which is the time to change the file in.
    line["test_busy_ms"] = 1500.into();
    let (path, key) = (pdf.0.clone(), job.key);
    let running = std::thread::spawn(move || {
        let stop = AtomicBool::new(false);
        let answer = headings::HeadingsAnswer::new(generated::MAX_ENTRIES, 6);
        Structure::at(walled_helper(), recto_platform::helper::Priority::Inherited)
            .run(&path, key, &line, answer, Instant::now() + Duration::from_secs(120), &stop)
            .0
    });
    // The helper is there once this process has a child; it's busy for longer than this takes, and the key is looked at when it ends.
    let until = Instant::now() + Duration::from_secs(30);
    while !std::process::Command::new("/usr/bin/pgrep")
        .args(["-P", &std::process::id().to_string()])
        .output()
        .unwrap()
        .status
        .success()
    {
        assert!(Instant::now() < until, "the helper never started");
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
    how(&pdf.0);
    running.join().unwrap()
}

/// Failure: a file written in place while the helper reads it is answered for as the version that was asked for, and its headings are kept under the old key. The key is looked at again
/// when the helper has ended.
#[test]
fn a_file_written_in_place_while_it_is_read_is_changed() {
    let pdf = TempPdf::new("inplace.pdf", &recto_testpdf::headings_document());
    let ended = change_while_it_runs(&pdf, |path| {
        let mut bytes = std::fs::read(path).unwrap();
        bytes.extend_from_slice(b"\n% appended\n");
        std::fs::write(path, bytes).unwrap();
    });
    assert!(matches!(ended, Ended::Changed), "{ended:?}");
}

/// Failure: a file replaced by renaming another over it while the helper reads is answered for as unchanged, because the descriptor it was given still shows the old file.
#[test]
fn a_file_swapped_by_a_rename_while_it_is_read_is_changed() {
    let pdf = TempPdf::new("swap.pdf", &recto_testpdf::headings_document());
    let ended = change_while_it_runs(&pdf, |path| {
        let other = path.with_extension("new");
        std::fs::write(&other, recto_testpdf::text_lines(&["a different file"])).unwrap();
        std::fs::rename(&other, path).unwrap();
    });
    assert!(matches!(ended, Ended::Changed), "{ended:?}");
}

/// A finished corpus under `key` whose pages' text is `texts`, in a folder of its own, and the key it's sealed under.
fn corpus_of(key: CorpusKey, texts: &[Option<String>]) -> (recto_testpdf::TempDir, recto_platform::find_key::FindKey) {
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-structure-blocks-");
    recto_platform::local::ensure_private_dir(&root).unwrap();
    let find_key = recto_platform::find_key::FindKey::for_tests(3);
    build_text_corpus(&find_key, &root, (key, None), texts).unwrap();
    (root, find_key)
}

fn blocks_ask(pdf: &TempPdf, key: CorpusKey, root: &std::path::Path, find_key: &recto_platform::find_key::FindKey) -> BlocksAsk {
    BlocksAsk {
        path: pdf.0.clone(),
        key,
        corpus_root: root.to_path_buf(),
        grant: recto_index::seal::grant_for(find_key, key).unwrap(),
    }
}

/// A generator whose blocks run in the real walled helper.
fn blocks_generator(root: &std::path::Path) -> Generator {
    Generator::with_runners(
        root,
        Box::new(Walled::at(walled_helper())),
        Box::new(recto_control::blocking::Walled::at(walled_helper())),
    )
}

/// Waits for the job to be over: it's queued the moment it's asked for, so idle again is its end.
fn until_idle(generator: &Generator, key: CorpusKey) {
    let until = Instant::now() + Duration::from_secs(120);
    while agent_blocks::Blocking::state(generator, key) != GenState::Idle {
        assert!(
            Instant::now() < until,
            "blocks never ended: {:?}",
            agent_blocks::Blocking::state(generator, key)
        );
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
}

/// Failure: the real helper's blocks aren't stored (the header never written, or the answer not mapped onto the corpus's own text), tables are stored as if they were text (a page that is a table answers
/// as blocks it hasn't), or some page has no blocks though its text has paragraphs. On the three-page blocks document, whose corpus is made from its drawn text: paragraphs, headings and list items are stored,
/// no table is, and every page has some.
#[test]
fn blocks_of_a_made_document_are_stored_and_tables_never() {
    let pdf = TempPdf::new("blocks.pdf", &recto_testpdf::blocks::blocks_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let texts: Vec<Option<String>> = recto_testpdf::blocks::BLOCKS_TEXT.iter().map(|lines| Some(lines.join("\n"))).collect();
    let (root, find_key) = corpus_of(key, &texts);
    let generator = blocks_generator(&root);
    agent_blocks::Blocking::want(&generator, blocks_ask(&pdf, key, &root, &find_key));
    until_idle(&generator, key);
    let stored = StoredBlocks::open(&find_key, &root, key).unwrap().expect("blocks were made");
    let made = stored.made();
    for shape in [Shape::Paragraph, Shape::Heading, Shape::ListItem] {
        assert!(made.stored[shape.index()] > 0, "{} stored in {made:?}", shape.name());
    }
    assert_eq!((made.stored[Shape::Table.index()], made.stored[Shape::Figure.index()]), (0, 0), "{made:?}");
    assert!(made.dropped[Shape::Table.index()] >= 1, "the table was seen and not kept: {made:?}");
    assert!(made.bytes > 0 && made.pages == 3);
    let corpus = ReadyCorpus::open(&find_key, &root, key).unwrap().unwrap();
    for page in 0..3 {
        let mut buffer = Vec::new();
        let chars = corpus.page(page, &mut buffer).unwrap().text.chars().count() as u32;
        let blocks = stored.page(page, chars).unwrap_or_else(|error| panic!("page {page}: {error:?}"));
        assert!(!blocks.blocks.is_empty(), "page {page} has no blocks");
    }
}

/// The helper's own child, if any, of this test process.
fn children() -> String {
    String::from_utf8_lossy(
        &std::process::Command::new("/usr/bin/pgrep")
            .args(["-P", &std::process::id().to_string()])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string()
}

/// Waits until the blocks job runs and its helper is there.
fn helper_running(generator: &Generator, key: CorpusKey) {
    let until = Instant::now() + Duration::from_secs(30);
    while agent_blocks::Blocking::state(generator, key) != GenState::Running || children().is_empty() {
        assert!(Instant::now() < until, "the helper never started");
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
}

/// Failure: a blocks job cancelled part way leaves a bin that's believed, or its helper running to the end of its time. The form bomb keeps the helper busy for seconds; the job is cancelled once the helper is
/// there, and no file of blocks is left and the process has no child.
#[test]
fn blocks_cancelled_part_way_nothing_is_written() {
    let pdf = TempPdf::new("bomb.pdf", &recto_testpdf::form_bomb());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let (root, find_key) = corpus_of(key, &[None]);
    let generator = blocks_generator(&root);
    agent_blocks::Blocking::want(&generator, blocks_ask(&pdf, key, &root, &find_key));
    helper_running(&generator, key);
    agent_blocks::Blocking::cancel_files(&generator, Some(std::slice::from_ref(&pdf.0)));
    let until = Instant::now() + Duration::from_secs(30);
    while agent_blocks::Blocking::making(&generator, None) {
        assert!(Instant::now() < until, "it didn't stop when cancelled");
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
    assert!(
        StoredBlocks::open(&find_key, &root, key).unwrap().is_none(),
        "blocks were made for a cancelled job"
    );
    assert!(!root.join(key.hex()).join("v4/app/blocks.bin").exists(), "a bin was left");
    assert_eq!(children(), "", "children left");
}

/// Failure: a file swapped by a rename while its blocks are made gets blocks made of the old file's text (the helper holds the old file, so only a look at the path shows the swap): blocks are stored under the
/// key of a file that is no longer there. The helper is kept busy before it reads (debug builds), the file is swapped once it's there, and the job ends changed with nothing stored.
#[test]
fn a_file_rewritten_while_blocks_are_made_gets_none() {
    let pdf = TempPdf::new("swap.pdf", &recto_testpdf::blocks::blocks_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let texts: Vec<Option<String>> = recto_testpdf::blocks::BLOCKS_TEXT.iter().map(|lines| Some(lines.join("\n"))).collect();
    let (root, find_key) = corpus_of(key, &texts);
    let generator = Generator::with_runners(
        &root,
        Box::new(Walled::at(walled_helper())),
        Box::new(recto_control::blocking::Walled::at(walled_helper()).busy_for(1500)),
    );
    agent_blocks::Blocking::want(&generator, blocks_ask(&pdf, key, &root, &find_key));
    helper_running(&generator, key);
    let other = pdf.0.with_extension("new");
    std::fs::write(&other, recto_testpdf::text_lines(&["a different file"])).unwrap();
    std::fs::rename(&other, &pdf.0).unwrap();
    until_idle(&generator, key);
    assert!(
        StoredBlocks::open(&find_key, &root, key).unwrap().is_none(),
        "blocks were made for a file that was swapped"
    );
    assert!(!root.join(key.hex()).join("v4/app/blocks.bin").exists(), "a bin was left");
}

/// A store with no text for any page, that keeps the pages it was handed.
#[derive(Default)]
struct NoText(Vec<(u32, usize)>);

impl PageStore for NoText {
    fn text(&mut self, _page: u32, _chars: &mut Vec<char>) -> bool {
        false
    }

    fn put(&mut self, page: &PageBlocks) -> Result<(), &'static str> {
        self.0.push((page.page, page.blocks.len()));
        Ok(())
    }
}

/// Failure: the walled helper's blocks job, as the real runner reads it, says a page twice or none, gives candidates the session can't read (so every page maps nothing), counts a table as stored, or isn't the walled one. On the three-page blocks document with a store that has no text: every page is put once, in order, with no blocks stored (there is no text to find them in); candidates are counted by kind with the table among those not stored; and the numbers say walled.
#[test]
fn the_walled_helper_gives_blocks_for_every_page() {
    let pdf = TempPdf::new("blocks.pdf", &recto_testpdf::blocks::blocks_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let stop = AtomicBool::new(false);
    let (ended, numbers) = Structure::at(walled_helper(), recto_platform::helper::Priority::Inherited).run(
        &pdf.0,
        key,
        &blocks::job(),
        BlocksAnswer::new(NoText::default(), 3),
        Instant::now() + Duration::from_secs(120),
        &stop,
    );
    let Ended::Answered(answer) = ended else {
        panic!("not answered: {ended:?} {numbers}")
    };
    let (store, tally) = answer.into_parts();
    assert_eq!(
        store.0.iter().map(|(page, _)| *page).collect::<Vec<_>>(),
        [0, 1, 2],
        "every page, once, in order"
    );
    assert!(store.0.iter().all(|(_, stored)| *stored == 0), "no text, nothing stored: {:?}", store.0);
    assert_eq!((tally.pages, tally.pages_without_text), (3, 3));
    assert!(numbers["candidates"].as_u64().is_some_and(|n| n > 0), "{numbers}");
    assert!(
        numbers["dropped_table"].as_u64().is_some_and(|n| n >= 1),
        "the table is counted, never stored: {numbers}"
    );
    assert_eq!(numbers["stored"], 0);
    assert_eq!(numbers["walled"], true, "{numbers}");
    assert_eq!(numbers["pages_failed"], 0, "{numbers}");
    for shape in [Shape::Paragraph, Shape::Heading, Shape::ListItem] {
        assert!(
            numbers[format!("dropped_{}", shape.name())].as_u64().is_some_and(|n| n > 0),
            "{} among {numbers}",
            shape.name()
        );
    }
}

/// The pages of a document, from the helper's own `count` job.
struct Pages(Option<u32>);

impl Answer for Pages {
    fn take(&mut self, line: &[u8]) -> Result<Said, &'static str> {
        let line: Value = serde_json::from_slice(line).map_err(|_| "bad_answer")?;
        self.0 = line["pages"].as_u64().and_then(|pages| u32::try_from(pages).ok());
        if line["state"] == "counted" && self.0.is_some() {
            Ok(Said::End)
        } else {
            Err("bad_answer")
        }
    }
}

/// A measurement, not a test: the blocks job on the files named by `RECTO_BLOCKS_FILES` (separated by `:`), through the walled helper, with a store that has no text (so the mapping costs nothing and
/// the helper's own work and the answer's size are what's measured). Prints, a file a line: wall, the helper's peak as the session watched it, the answer's bytes a page, the candidates by kind, what was cut
/// and failed, and the time the answer spent. At utility priority, and again at normal when `RECTO_BLOCKS_NORMAL` is set.
///
/// `cargo test --release -p recto-control --test structure blocks_measure -- --ignored --nocapture`
#[test]
#[ignore = "a measurement"]
fn blocks_measure() {
    let files = std::env::var("RECTO_BLOCKS_FILES").expect("RECTO_BLOCKS_FILES: files separated by ':'");
    let levels = if std::env::var_os("RECTO_BLOCKS_NORMAL").is_some() {
        vec![
            ("utility", recto_platform::helper::Priority::Utility),
            ("normal", recto_platform::helper::Priority::Inherited),
        ]
    } else {
        vec![("utility", recto_platform::helper::Priority::Utility)]
    };
    for file in files.split(':').filter(|file| !file.is_empty()) {
        let path = PathBuf::from(file);
        let key = CorpusKey::of_file(&path).unwrap();
        for (name, priority) in &levels {
            let structure = Structure::at(walled_helper(), *priority);
            let stop = AtomicBool::new(false);
            let until = || Instant::now() + Duration::from_secs(1800);
            let count = serde_json::json!({ "kind": "count", "memory_limit": recto_control::structure::MEMORY_LIMIT });
            let Ended::Answered(Pages(Some(pages))) = structure.run(&path, key, &count, Pages(None), until(), &stop).0 else {
                panic!("no page count for {file}")
            };
            let started = Instant::now();
            let (ended, numbers) = structure.run(&path, key, &blocks::job(), BlocksAnswer::new(NoText::default(), pages), until(), &stop);
            let wall = started.elapsed();
            let Ended::Answered(_) = ended else { panic!("{file}: {ended:?} {numbers}") };
            let by_kind: Vec<String> = Shape::ALL
                .iter()
                .map(|shape| format!("{}={}", shape.name(), numbers[format!("dropped_{}", shape.name())]))
                .collect();
            let per_page = numbers["answer_bytes"].as_u64().unwrap_or(0) as f64 / f64::from(pages);
            println!(
                "BLOCKS {} [{name}] pages={pages} wall={:.2}s helper_peak_watched={:.1}MiB helper_peak_own={:.1}MiB answer_bytes/page={per_page:.0} candidates={} ({}) cut={} pages_failed={} map_ms={} distilled_ms={}",
                path.file_name().unwrap().to_string_lossy(),
                wall.as_secs_f64(),
                numbers["helper_peak_watched_bytes"].as_f64().unwrap_or(0.0) / 1048576.0,
                numbers["helper_peak_bytes"].as_f64().unwrap_or(0.0) / 1048576.0,
                numbers["candidates"],
                by_kind.join(" "),
                numbers["cut"],
                numbers["pages_failed"],
                numbers["map_ms"],
                numbers["distilled_ms"],
            );
        }
    }
}
