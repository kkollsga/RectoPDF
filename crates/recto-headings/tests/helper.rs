//! The real helper process, on generated PDFs, the way the session runs it: the document on descriptor 3, a job on stdin, a line a page and one end back on stdout. Run bare, from `target/`: the walled one is `tests/sandbox.rs`, and what the session does with the answer (the check for hidden text) is `recto-control`'s `tests/structure.rs`.

use recto_index::corpus::CorpusKey;
use recto_index::generated::visible::MAX_LINE_BYTES;
use recto_platform::helper::{self, Helper, Priority};
use recto_testpdf::TempPdf;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::ExitStatus;
#[path = "support/diagnostics.rs"]
mod diagnostics;
#[path = "support/hello.rs"]
mod hello;
use hello::after_hello;
use std::time::{Duration, Instant};

/// What the helper said after its hello, one job.
struct Run {
    /// Each line, as JSON.
    lines: Vec<Value>,
    elapsed: Duration,
}

impl Run {
    /// The last thing it said: the end line, a refusal, or nothing.
    fn last(&self) -> &Value {
        self.lines.last().unwrap_or(&Value::Null)
    }
}

/// The `headings` job as the session sends it, on the document given on descriptor 3.
fn run(path: &std::path::Path, close_stdin: bool) -> Run {
    let started = Instant::now();
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
    writeln!(stdin, "{}", json!({ "kind": "headings", "memory_limit": 512u64 << 20 })).unwrap();
    let held_open = (!close_stdin).then_some(stdin);
    let mut said = String::new();
    stdout.read_to_string(&mut said).unwrap();
    let _ = child.wait();
    drop(held_open);
    let lines = after_hello(&said)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or(Value::Null))
        .collect();
    Run {
        lines,
        elapsed: started.elapsed(),
    }
}

/// Failure: something that isn't a PDF, or one distillPDF's bounded reader refuses, makes the helper panic or hang, rather than say it couldn't be read.
#[test]
fn what_cant_be_read_is_refused_plainly() {
    for (name, bytes) in [("junk.pdf", b"%PDF-1.7 this is not really a PDF".to_vec()), ("empty.pdf", Vec::new())] {
        let pdf = TempPdf::new(name, &bytes);
        let run = run(&pdf.0, false);
        assert_eq!((run.lines.len(), run.last()["state"].as_str()), (1, Some("refused")), "{name}: {:?}", run.lines);
        assert!(run.elapsed < Duration::from_secs(30), "{:?}", run.elapsed);
    }
}

/// Failure: a hostile page, a form that draws itself over and over, holds the helper without end, or takes its memory past any limit.
#[test]
fn a_form_bomb_ends() {
    let pdf = TempPdf::new("bomb.pdf", &recto_testpdf::form_bomb());
    let run = run(&pdf.0, false);
    let report = run.last();
    assert!(matches!(report["state"].as_str(), Some("ready" | "refused")), "{report}");
    assert!(run.elapsed < Duration::from_secs(60), "{:?}", run.elapsed);
    assert!(report["helper_peak_bytes"].as_u64().is_none_or(|peak| peak < 512 << 20), "{report}");
}

/// Failure: the helper outlives whoever started it. Its stdin closing is its sign to go, whatever it's doing. On a file it works on for seconds (the
/// form bomb, 5.3 s here with stdin open, 2026-11-12), it goes before it has anything to say. On a quick file, as this was until then, it finished
/// before the close mattered, so the test passed with the watch on stdin taken out.
#[test]
fn it_goes_when_its_stdin_closes() {
    let pdf = TempPdf::new("bomb.pdf", &recto_testpdf::form_bomb());
    let run = run(&pdf.0, true);
    assert!(
        run.lines.is_empty(),
        "it worked on after its stdin closed, for {:?}: {:?}",
        run.elapsed,
        run.lines
    );
    assert!(run.elapsed < Duration::from_secs(10), "{:?}", run.elapsed);
}

/// Failure: the headings job says its answer in a shape the session doesn't read: lines out of page order, a line longer than the cap, an end line whose `found` isn't what the page lines held, or anything said after the end. On the six-page headings document: a line a page, pages strictly increasing, and one end.
#[test]
fn the_headings_job_says_a_line_a_page_and_one_end() {
    let pdf = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    let run = run(&pdf.0, false);
    let (end, pages) = run.lines.split_last().expect("it said something");
    assert_eq!(end["state"], "ready", "{:?}", run.lines);
    assert_eq!(
        (end["pages"].as_u64(), end["pages_failed"].as_u64(), end["truncated"].as_bool()),
        (Some(6), Some(0), Some(false)),
        "{end}"
    );
    let mut last = None;
    let mut sent = 0;
    for line in pages {
        let page = line["page"].as_u64().unwrap_or_else(|| panic!("a page line: {line}"));
        assert!(last.is_none_or(|before| page > before), "pages out of order at {page} after {last:?}");
        last = Some(page);
        assert_eq!(line.as_object().unwrap().len(), 2, "exactly a page and what was found: {line}");
        sent += line["found"].as_array().unwrap_or_else(|| panic!("{line}")).len();
        assert!(line.to_string().len() <= MAX_LINE_BYTES);
    }
    assert_eq!(pages.len(), 6, "a line for each page with a heading: {:?}", run.lines);
    assert_eq!(end["found"].as_u64(), Some(sent as u64), "the end counts what the page lines held");
}

/// Failure: a bare helper, which has no walls to refuse a path with, opens a file by the name a job gives it, so a job that names one reads outside the document the host chose. It answers
/// `unsupported_job` whatever the build, for the old shape (`file` and `key` alone) and for a `headings` or `blocks` job that also carries a `file`.
#[test]
fn a_job_that_names_a_path_is_refused_by_a_bare_helper_too() {
    let pdf = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    let key = CorpusKey::of_file(&pdf.0).unwrap();
    for job in [
        json!({ "file": pdf.0, "key": key.hex(), "memory_limit": 512u64 << 20 }),
        json!({ "kind": "headings", "file": pdf.0, "key": key.hex() }),
        json!({ "kind": "blocks", "file": pdf.0, "key": key.hex() }),
        json!({ "kind": "something else" }),
    ] {
        let (said, status) = converse(&job, Some(&std::fs::File::open(&pdf.0).unwrap()));
        assert_eq!(said.len(), 2, "{job}: {said:?}");
        assert_eq!(said[1], json!({ "state": "failed", "code": "unsupported_job" }), "{job}");
        assert!(status.is_some_and(|status| status.success()), "{status:?}");
    }
}

/// How long the helper has to be gone once it has a reason to go (stdin closed, memory limit passed), whatever its main thread is doing. It's a rule, not a measure of speed: the faults below hold the helper for 15 s or 30 s, so only going at once passes, and a starved machine has to be 10 times slower than this one to fail it by itself.
const GONE_WITHIN: Duration = Duration::from_secs(5);

/// What the helper did, said so a failure explains itself.
struct Went {
    status: Option<ExitStatus>,
    said: String,
    elapsed: Duration,
    evidence: Option<String>,
}

impl Went {
    fn describe(&self) -> String {
        match self.status {
            Some(status) => format!("exited {status} after {:?}, said {:?}", self.elapsed, self.said),
            None => format!(
                "was still alive after {:?} and was killed, said {:?}; {}",
                self.elapsed,
                self.said,
                self.evidence.as_deref().unwrap_or("evidence unavailable")
            ),
        }
    }
}

fn wait_for(child: &mut Helper, bound: Duration) -> Option<ExitStatus> {
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
    let document = std::fs::File::open(&pdf.0).unwrap();
    let mut job = json!({ "kind": "headings" });
    job.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    let started = Instant::now();
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
    writeln!(stdin, "{job}").unwrap();
    let held_open = (!close_stdin).then_some(stdin);
    let status = wait_for(&mut child, bound);
    let elapsed = started.elapsed();
    let evidence = status.is_none().then(|| diagnostics::capture(child.id(), elapsed, &job));
    if status.is_none() {
        let _ = child.kill();
    }
    let mut said = String::new();
    let _ = stdout.read_to_string(&mut said);
    let _ = child.wait();
    drop(held_open);
    Went {
        status,
        said,
        elapsed,
        evidence,
    }
}

/// Failure: a helper whose stdin closed is kept by something in its own process, as `std::process::exit` is by an exit handler or a destructor waiting on a lock (the 2026-11-13 failure: it worked 5.3 s after its stdin closed). Here its main thread is busy for 30 s and an exit handler stalls for 15 s, and it still has to go, quietly, within the bound.
#[test]
fn it_goes_at_once_when_its_stdin_closes_whatever_it_is_doing() {
    let went = start_and_wait(json!({ "test_busy_ms": 30_000, "test_stall_exit_ms": 15_000 }), true, GONE_WITHIN);
    assert!(
        went.status.is_some_and(|status| status.success()) && after_hello(&went.said).is_empty(),
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
        went.status.is_some_and(|status| status.code() == Some(3)) && after_hello(&went.said).is_empty(),
        "past its memory limit it didn't stop: it {}",
        went.describe()
    );
}

/// Failure: a helper that couldn't start its watchers (a thread that wouldn't spawn) runs the job without them, so nothing makes it go when its parent has, or when it grows. It has to refuse the job with a plain code and do none of it.
#[test]
fn it_refuses_the_job_when_it_cant_start_its_watchers() {
    let went = start_and_wait(json!({ "memory_limit": 512u64 << 20, "test_spawn_fails": true }), false, GONE_WITHIN);
    let report: Value = serde_json::from_str(after_hello(&went.said).lines().next().unwrap_or("null")).unwrap_or(Value::Null);
    assert!(
        went.status.is_some() && report["state"] == "failed" && report["code"] == "no_watch",
        "it did the job without its watchers: it {}",
        went.describe()
    );
    assert_eq!(after_hello(&went.said).lines().count(), 1, "{}", went.describe());
}

/// Failure: keeping stdin open for a completed job leaks a parent descriptor on every request.
#[test]
fn completed_jobs_do_not_leak_stdin() {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--ignored", "--exact", "stdin_fd_probe", "--nocapture"]);
    let (_, output) = recto_platform::test_command::output(&mut command, Duration::from_secs(10)).unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "isolated descriptor-count entry for completed_jobs_do_not_leak_stdin"]
fn stdin_fd_probe() {
    let count = || std::fs::read_dir("/dev/fd").unwrap().count();
    let pdf = TempPdf::new("invalid.pdf", b"not a PDF");
    let before = count();
    let _ = run(&pdf.0, false);
    assert_eq!(count(), before, "completed helper leaked a parent descriptor");
}

/// The helper's whole conversation for one job line: each line it said, as JSON, and how it ended. `document`, if there is one, is put on its descriptor 3.
fn converse(job: &Value, document: Option<&std::fs::File>) -> (Vec<Value>, Option<ExitStatus>) {
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_recto-headings"));
    let env = [("MallocLargeCache", std::ffi::OsStr::new("0"))];
    let spawned = match document {
        Some(file) => helper::spawn_with_document(exe, &env, Priority::Inherited, file),
        None => helper::spawn(exe, &env, Priority::Inherited),
    };
    let helper::Spawned {
        mut child,
        mut stdin,
        mut stdout,
    } = spawned.unwrap();
    writeln!(stdin, "{job}").unwrap();
    let mut said = String::new();
    stdout.read_to_string(&mut said).unwrap();
    drop(stdin);
    let status = child.wait().ok();
    (said.lines().map(|line| serde_json::from_str(line).unwrap_or(Value::Null)).collect(), status)
}

/// Failure: a bare helper (as every development and test run is) claims walls it doesn't have, so the session believes a document was read behind them.
#[test]
fn a_bare_helper_says_it_has_no_walls() {
    let (said, _) = converse(&json!({ "kind": "count" }), None);
    assert_eq!(said[0], json!({ "state": "hello", "walled": false }), "{said:?}");
}

/// Failure: `count` reads the file by a name, or from the wrong descriptor, or counts another file's pages: its answer is the pages of the document it was handed, and the hello still comes first.
#[test]
fn count_gives_the_pages_of_the_document_it_was_handed() {
    let pages: Vec<recto_testpdf::Page> = (1..=3)
        .map(|number| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("page {number}")])))
        .collect();
    let pdf = TempPdf::new("three.pdf", &recto_testpdf::pages(&pages));
    let file = std::fs::File::open(&pdf.0).unwrap();
    let (said, status) = converse(&json!({ "kind": "count" }), Some(&file));
    assert_eq!(said.len(), 2, "{said:?}");
    assert_eq!((said[1]["state"].as_str(), said[1]["pages"].as_u64()), (Some("counted"), Some(3)), "{said:?}");
    assert!(said[1]["helper_peak_bytes"].as_u64().is_some_and(|peak| peak > 0), "{said:?}");
    assert!(status.is_some_and(|status| status.success()), "{status:?}");
}

/// Failure: a descriptor 3 that isn't a regular file (a pipe the host left there) is read as a document, or the job hangs on it. It's refused as no document, and read from by nothing.
#[test]
fn count_with_a_pipe_on_3_says_there_is_no_document() {
    let (reader, writer) = std::io::pipe().unwrap();
    // The writer end stays open in this process and unwritten: a read of it would hang.
    let as_file: std::fs::File = std::os::fd::OwnedFd::from(reader).into();
    let (said, _) = converse(&json!({ "kind": "count" }), Some(&as_file));
    drop(writer);
    assert_eq!(
        (said[1]["state"].as_str(), said[1]["code"].as_str()),
        (Some("refused"), Some("no_document")),
        "{said:?}"
    );
}

/// Failure: with nothing on descriptor 3 the job opens some other file or counts nothing and says it counted. It says there is no document.
#[test]
fn count_with_no_descriptor_says_there_is_no_document() {
    let (said, _) = converse(&json!({ "kind": "count" }), None);
    assert_eq!(
        (said[1]["state"].as_str(), said[1]["code"].as_str()),
        (Some("refused"), Some("no_document")),
        "{said:?}"
    );
}
