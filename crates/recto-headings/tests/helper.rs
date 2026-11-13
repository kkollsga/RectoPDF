//! The real helper process, on generated PDFs, the way the session runs it: a job on stdin, two lines back on stdout. The render worker has to be built beside it, as `tools/check.sh` does.

use recto_index::corpus::CorpusKey;
use recto_index::generated::Generated;
use recto_testpdf::TempPdf;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
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

/// Failure: the helper outlives whoever started it. Its stdin closing is its sign to go, whatever it's doing. On a file it works on for seconds (the
/// form bomb, 5.3 s here with stdin open, 2026-11-12), it goes before it has anything to say. On a quick file, as this was until then, it finished
/// before the close mattered, so the test passed with the watch on stdin taken out.
#[test]
fn it_goes_when_its_stdin_closes() {
    let pdf = TempPdf::new("bomb.pdf", &recto_testpdf::form_bomb());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let (report, generated, elapsed) = run(&pdf.0, key, true);
    assert_eq!((report, generated), (Value::Null, None), "it worked on after its stdin closed, for {elapsed:?}");
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
        let generated = generated.expect("contents");
        let titles: Vec<String> = generated.entries.iter().map(|entry| entry.title.clone()).collect();
        assert!(titles.iter().all(|title| !title.contains("HIDDEN")), "{name}: {titles:?}");
        assert_eq!(
            titles,
            ["1 Introduction", "2 Background", "3 Method", "4 Results", "5 Discussion", "6 Conclusion"],
            "{name}"
        );
        // Each at its page, near its top, and what was found is what's kept and what was left out as hidden: distillPDF may take a hidden line for a
        // heading, and then it's left out here; should it stop finding them, nothing's left out, and this still holds.
        let pages: Vec<u32> = generated.entries.iter().map(|entry| entry.page).collect();
        assert_eq!(pages, [0, 1, 2, 3, 4, 5], "{name}");
        assert!(generated.entries.iter().all(|entry| entry.place < 200), "{name}: {:?}", generated.entries);
        assert_eq!(
            report["found"].as_u64(),
            Some(titles.len() as u64 + u64::from(generated.hidden_dropped)),
            "{name}: {report}"
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

/// How long the helper has to be gone once it has a reason to go (stdin closed, memory limit passed), whatever its main thread is doing. It's a rule, not a measure of speed: the faults below hold the helper for 15 s or 30 s, so only going at once passes, and a starved machine has to be 10 times slower than this one to fail it by itself.
const GONE_WITHIN: Duration = Duration::from_secs(5);

/// What the helper did, said so a failure explains itself.
struct Went {
    status: Option<ExitStatus>,
    said: String,
    elapsed: Duration,
}

impl Went {
    fn describe(&self) -> String {
        match self.status {
            Some(status) => format!("exited {status} after {:?}, said {:?}", self.elapsed, self.said),
            None => format!("was still alive after {:?} and was killed, said {:?}", self.elapsed, self.said),
        }
    }
}

fn wait_for(child: &mut Child, bound: Duration) -> Option<ExitStatus> {
    let until = Instant::now() + bound;
    while Instant::now() < until {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(20)); // ratchet: poll
    }
    None
}

/// Starts the helper on a small valid PDF with `job`'s extra fields (a debug build reads the `test_` ones), closes its stdin or keeps it open, and waits at most `bound` for it to go.
fn start_and_wait(extra: Value, close_stdin: bool, bound: Duration) -> Went {
    let pdf = TempPdf::new("small.pdf", &recto_testpdf::headings_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    let mut job = json!({ "file": pdf.0, "key": key.hex() });
    job.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
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
    writeln!(stdin, "{job}").unwrap();
    let held_open = (!close_stdin).then_some(stdin);
    let status = wait_for(&mut child, bound);
    let elapsed = started.elapsed();
    if status.is_none() {
        let _ = child.kill();
    }
    let mut said = String::new();
    let _ = child.stdout.take().unwrap().read_to_string(&mut said);
    let _ = child.wait();
    drop(held_open);
    Went { status, said, elapsed }
}

/// Failure: a helper whose stdin closed is kept by something in its own process, as `std::process::exit` is by an exit handler or a destructor waiting on a lock (the 2026-11-13 failure: it worked 5.3 s after its stdin closed). Here its main thread is busy for 30 s and an exit handler stalls for 15 s, and it still has to go, quietly, within the bound.
#[test]
fn it_goes_at_once_when_its_stdin_closes_whatever_it_is_doing() {
    let went = start_and_wait(json!({ "test_busy_ms": 30_000, "test_stall_exit_ms": 15_000 }), true, GONE_WITHIN);
    assert!(
        went.status.is_some_and(|status| status.success()) && went.said.is_empty(),
        "closing its stdin didn't stop it: it {}",
        went.describe()
    );
}

/// Failure: a helper past its memory limit stays, because its way out waits on something, so a hostile file keeps a gigabyte until the session's own watch notices. It has to go with the over-the-limit code (3) and no report.
#[test]
fn it_goes_at_once_when_it_passes_its_memory_limit() {
    let extra = json!({ "memory_limit": 100u64 << 20, "test_hold_mib": 300, "test_busy_ms": 30_000, "test_stall_exit_ms": 15_000 });
    let went = start_and_wait(extra, false, GONE_WITHIN);
    assert!(
        went.status.is_some_and(|status| status.code() == Some(3)) && went.said.is_empty(),
        "past its memory limit it didn't stop: it {}",
        went.describe()
    );
}

/// Failure: a helper that couldn't start its watchers (a thread that wouldn't spawn) runs the job without them, so nothing makes it go when its parent has, or when it grows. It has to refuse the job with a plain code and do none of it.
#[test]
fn it_refuses_the_job_when_it_cant_start_its_watchers() {
    let went = start_and_wait(json!({ "memory_limit": 512u64 << 20, "test_spawn_fails": true }), false, GONE_WITHIN);
    let report: Value = serde_json::from_str(went.said.lines().next().unwrap_or("null")).unwrap_or(Value::Null);
    assert!(
        went.status.is_some() && report["state"] == "failed" && report["code"] == "no_watch",
        "it did the job without its watchers: it {}",
        went.describe()
    );
    assert_eq!(went.said.lines().count(), 1, "{}", went.describe());
}
