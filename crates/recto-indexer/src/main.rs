//! `recto-indexer`: builds one document's search corpus, and exits. Started by the session, one at a time. It takes a key bound to the one corpus it's to build (never the session's Find key: `recto_index::seal::grant_for`) on stdin, read with no buffer so nothing else holds a copy, then its job as one line, for that corpus, so a file's name stays out of `ps`, and it ends as soon as stdin closes, so it doesn't outlive whoever started it however they ended. It says how it went as one line of JSON on stdout. Anything else, such as being killed, is safe: `recto_index::corpus` goes on from where it stopped next time.
//!
//! It holds no PDFium. Pages come from a worker like any other, under limits of its own. It's a process of its own, rather than a thread in the session, because what it uses goes when it exits, and a page of two million characters takes tens of megabytes to read: the session's footprint stays where it was.

use recto_core::OpenError;
use recto_core::page_id::PageRead;
use recto_index::corpus::{self, BuildLimits, BuildOutcome, CorpusKey, DocMeta, FailReason, FedPage, FeedError, FileIdentities, IdentityInfo, PageFeed};
use recto_render_service::{DocRenderer, KillReason, MAX_BUSY, Policy, RenderFailure, RenderStats, ServiceConfig};
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Longest job line. A path or two and some numbers.
const MAX_JOB_BYTES: u64 = 128 * 1024;
/// A worker's limit while indexing. Half of what a reader's page gets. Only the check for hidden text needs to draw, and a page that can't be drawn under this is marked as not checked, and is drawn if a search ever finds something on it. By `docs/engineering/search-index.md`, 17 of 5,872 real pages pass this at 2x.
const MEMORY_LIMIT_BYTES: u64 = 256 * 1024 * 1024;
/// A page's time. Twice a reader's, as this runs at utility, below what the person is looking at.
const PAGE_DEADLINE: Duration = Duration::from_secs(10);
/// Hidden-text checks that may stop a worker before the rest of the document's pages go unchecked. Each costs a kill and a new worker, and kills are capped per document: spending them all on checks would leave pages unread.
const MAX_HIDDEN_CHECK_FAILURES: u32 = 16;

/// A build given up on because a worker wasn't given the time it needed, which says nothing about the file. Nothing's remembered, and whoever's still waiting asks again. Each of these has taken a whole deadline, so asking again can't spin.
const TOO_BUSY: &str = "too_busy";

/// Reads pages through a worker.
struct WorkerFeed {
    path: PathBuf,
    key: CorpusKey,
    renderer: Option<DocRenderer>,
    check_hidden: bool,
    hidden_check_failures: u32,
    /// The file's version as `main` saw it right after the key check, and lopdf's reading of exactly that version, opened when the first identity is asked for.
    identities: OpenedIdentities,
}

/// What lopdf reads of the file for the page identities. Opened once, and only if the build gets as far as asking.
struct OpenedIdentities {
    stamp: Option<recto_platform::FileStamp>,
    opened: Option<Option<FileIdentities>>,
}

impl OpenedIdentities {
    fn of(path: &Path) -> Self {
        Self {
            stamp: recto_platform::file_stamp(path).ok(),
            opened: None,
        }
    }

    /// `None` when the file can't be read by lopdf, or isn't the version that was stamped.
    fn files(&mut self, path: &Path) -> Option<&FileIdentities> {
        let stamp = self.stamp;
        self.opened
            .get_or_insert_with(|| stamp.and_then(|stamp| FileIdentities::open(path, stamp)))
            .as_ref()
    }
}

impl WorkerFeed {
    fn renderer(&mut self) -> Result<&mut DocRenderer, FeedError> {
        if self.renderer.is_none() {
            let config = ServiceConfig::discover().map_err(about_the_install)?;
            let policy = Policy {
                memory_limit_bytes: MEMORY_LIMIT_BYTES,
                // Not above the limit turns the step for one heavy page off. An index never asks for a gigabyte.
                heavy_limit_bytes: MEMORY_LIMIT_BYTES,
                page_deadline: PAGE_DEADLINE,
                ..Policy::default()
            };
            self.renderer = Some(DocRenderer::new(&self.path, None, config, policy).map_err(whole_file)?);
        }
        self.renderer.as_mut().ok_or(FeedError::Cant("no_renderer"))
    }

    fn stats(&self) -> RenderStats {
        self.renderer.as_ref().map(DocRenderer::stats).unwrap_or_default()
    }
}

/// Takes the renderer itself, not a closure that reports whether the count moved, so the count is read here before each try and a test can fail the bug of reading it once before the loop. Asks again of `renderer` while the answer is [`RenderFailure::TooBusyToStart`]: the service's own bound ([`MAX_BUSY`] in a row) decides inside this run, and a worker that hangs before hello ends as `Unavailable` rather than pausing the build for ever. `busy` reads the renderer's `too_busy` count, taken before each try, so a failure that isn't about the file and moved it during that try (not an earlier one) is a pause (`TOO_BUSY`); a page's own deadline stays with that page (a busy kill during a page read is that page's `Deadline` failure, a temporary failed page read again later, not a pause). At most `2 * MAX_BUSY` tries (starts and opens each have a run of their own), then the build pauses.
fn asked<R, T>(
    renderer: &mut R,
    busy: fn(&R) -> u32,
    mut ask: impl FnMut(&mut R) -> Result<T, RenderFailure>,
    meaning: fn(RenderFailure) -> FeedError,
) -> Result<T, FeedError> {
    for _ in 0..2 * u32::from(MAX_BUSY) {
        let before = busy(renderer);
        match ask(renderer) {
            Ok(value) => return Ok(value),
            Err(RenderFailure::TooBusyToStart) => {}
            Err(failure) => {
                return Err(match meaning(failure) {
                    FeedError::Cant(_) if busy(renderer) > before => FeedError::Cant(TOO_BUSY),
                    error => error,
                });
            }
        }
    }
    Err(FeedError::Cant(TOO_BUSY))
}

fn busy_count(renderer: &DocRenderer) -> u32 {
    renderer.stats().too_busy
}

fn about_the_install(failure: RenderFailure) -> FeedError {
    match failure {
        RenderFailure::LibraryMissing => FeedError::Cant("render_library_missing"),
        RenderFailure::WallsDown(_) | RenderFailure::StartRefused => FeedError::Cant(failure.code()),
        _ => FeedError::Cant("render_worker_missing"),
    }
}

/// What a failure means for a whole document. Only what's a fact about this file is remembered.
fn whole_file(failure: RenderFailure) -> FeedError {
    match failure {
        // Its text is never written anywhere: the agent report's §5.5, and nothing here has consent for it yet.
        RenderFailure::NeedsPassword => FeedError::Refused("needs_password"),
        RenderFailure::NotPdf => FeedError::Refused("not_pdf"),
        RenderFailure::Security => FeedError::Refused("security"),
        RenderFailure::WorkerMissing | RenderFailure::LibraryMissing | RenderFailure::WallsDown(_) | RenderFailure::StartRefused => about_the_install(failure),
        RenderFailure::NotFound => FeedError::Cant("not_found"),
        // The busy Mac gave the worker too little time to start: a paused build, as a start that timed out was.
        RenderFailure::TooBusyToStart => FeedError::Cant(TOO_BUSY),
        RenderFailure::SourceChanged => FeedError::FileChanged,
        _ => FeedError::Cant("couldnt_open"),
    }
}

/// What a failure to read one page means for the build. Only what's a fact about that page gets written down as one, and whether it might read another time goes with it. What's about this moment rather than any page stops the build where it is, with nothing written, so it goes on from that page later.
fn read_failure(failure: RenderFailure) -> FeedError {
    match failure {
        RenderFailure::Unavailable => FeedError::Cant("render_unavailable"),
        RenderFailure::TooBusyToStart => whole_file(failure),
        RenderFailure::PageReadFailed {
            reason: KillReason::Deadline, ..
        } => FeedError::Page(FailReason::Deadline),
        RenderFailure::PageReadFailed {
            reason: KillReason::Crashed, ..
        } => FeedError::Page(FailReason::Crashed),
        RenderFailure::DocumentStopped => FeedError::Page(FailReason::DocumentStopped),
        RenderFailure::SourceChanged => FeedError::FileChanged,
        RenderFailure::NeedsPassword
        | RenderFailure::NotPdf
        | RenderFailure::Security
        | RenderFailure::WorkerMissing
        | RenderFailure::LibraryMissing
        | RenderFailure::WallsDown(_)
        | RenderFailure::StartRefused
        | RenderFailure::NotFound
        | RenderFailure::OpenFailed { .. } => whole_file(failure),
        _ => FeedError::Page(FailReason::Stopped),
    }
}

impl PageFeed for WorkerFeed {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        asked(self.renderer()?, busy_count, DocRenderer::page_count, whole_file)
    }

    fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
        let check = self.check_hidden;
        let renderer = self.renderer()?;
        let read = asked(renderer, busy_count, |renderer| renderer.page_characters(page, true, check), read_failure);
        match read {
            Ok(characters) => {
                if check && !characters.hidden_checked {
                    self.hidden_check_failures += 1;
                    self.check_hidden = self.hidden_check_failures < MAX_HIDDEN_CHECK_FAILURES;
                }
                Ok(FedPage {
                    chars: characters.chars,
                    boxes: characters.boxes,
                    hidden_checked: characters.hidden_checked,
                })
            }
            Err(error) => Err(error),
        }
    }

    fn still_the_same_file(&mut self) -> bool {
        CorpusKey::of_file(&self.path).is_ok_and(|now| now == self.key)
    }

    fn forget_failures(&mut self) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.forget_failures();
        }
    }

    fn identities(&mut self) -> Option<IdentityInfo> {
        self.identities.files(&self.path).map(FileIdentities::info)
    }

    fn identity(&mut self, page: u32) -> PageRead {
        self.identities
            .files(&self.path)
            .map_or(PageRead { identity: None, bytes: 0 }, |files| files.read(page))
    }
}

/// What goes in the manifest beside the pages. A file only the renderer can read has none of it, and says so.
fn doc_meta(path: &Path) -> Result<DocMeta, OpenError> {
    let file_bytes = std::fs::metadata(path).map_or(0, |meta| meta.len());
    match recto_core::inspect(path) {
        Ok(facts) => {
            let outline = facts
                .toc
                .entries()
                .iter()
                .map(|entry| {
                    (
                        entry.depth,
                        (entry.page != recto_core::NONE).then_some(entry.page),
                        facts.toc.title(entry).to_string(),
                    )
                })
                .collect();
            Ok(DocMeta {
                title: facts.title.map(String::from),
                outline,
                renderer_only: facts.renderer_only,
                file_bytes,
            })
        }
        // Whether it can be read is the worker's to say, in a moment.
        Err(OpenError::NotPdf | OpenError::TooComplex) => Ok(DocMeta {
            renderer_only: true,
            file_bytes,
            ..DocMeta::default()
        }),
        Err(error) => Err(error),
    }
}

fn job() -> Option<Value> {
    let mut line = String::new();
    std::io::stdin().lock().take(MAX_JOB_BYTES).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn say(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

fn outcome_json(outcome: &BuildOutcome) -> Value {
    match outcome {
        BuildOutcome::Ready { summary, stats } => {
            json!({ "state": "ready", "pages": summary.pages, "pages_built": stats.pages_built, "pages_resumed": stats.pages_resumed, "elapsed_ms": stats.elapsed_ms, "corpus_bytes": summary.corpus_bytes, "pack_bytes": summary.pack_bytes, "failed_pages": summary.failed_pages, "no_text_pages": summary.no_text_pages, "hidden_flagged_pages": summary.hidden_flagged_pages, "hidden_unchecked_pages": summary.hidden_unchecked_pages })
        }
        BuildOutcome::AlreadyReady(summary) => {
            json!({ "state": "ready", "pages": summary.pages, "pages_built": 0, "corpus_bytes": summary.corpus_bytes, "pack_bytes": summary.pack_bytes })
        }
        BuildOutcome::Refused(code) => json!({ "state": "refused", "code": code }),
        BuildOutcome::Busy => json!({ "state": "failed", "code": "building_elsewhere" }),
        BuildOutcome::FileChanged => json!({ "state": "changed" }),
        BuildOutcome::Paused(code @ ("out_of_time" | TOO_BUSY)) => json!({ "state": "paused", "code": code }),
        BuildOutcome::Paused(code) => json!({ "state": "failed", "code": code }),
    }
}

fn main() {
    // It holds the key and pages' text, so a crash leaves no image of its memory.
    recto_platform::find_key::forbid_core_dumps();
    // First, before anything reads stdin through its buffer. Never in the arguments, the environment or a file.
    let Ok(find_key) = recto_platform::find_key::read_from_stdin() else {
        say(&json!({ "state": "failed", "code": "bad_job" }));
        return;
    };
    let Some(job) = job() else {
        say(&json!({ "state": "failed", "code": "bad_job" }));
        return;
    };
    let (Some(file), Some(root), Some(key)) = (
        job["file"].as_str().map(PathBuf::from),
        job["root"].as_str().map(PathBuf::from),
        job["key"].as_str().and_then(CorpusKey::from_hex),
    ) else {
        say(&json!({ "state": "failed", "code": "bad_job" }));
        return;
    };
    // The key it was handed opens one corpus: a job for another is a session's mistake, and building it could only fail.
    if find_key.binding() != Some(key.as_bytes()) {
        say(&json!({ "state": "failed", "code": "bad_job" }));
        return;
    }
    // Whoever started this has gone or wants it stopped. It ends at once: every step of a build can be killed.
    let _ = std::thread::Builder::new().name("stdin".into()).stack_size(64 * 1024).spawn(|| {
        let mut sink = [0u8; 64];
        while std::io::stdin().lock().read(&mut sink).is_ok_and(|count| count > 0) {}
        std::process::exit(0);
    });
    if !CorpusKey::of_file(&file).is_ok_and(|now| now == key) {
        say(&json!({ "state": "changed" }));
        return;
    }
    // Straight after the key check, so what lopdf reads for the identities is the version the key names, or nothing.
    let identities = OpenedIdentities::of(&file);
    let mut limits = BuildLimits::default();
    if let Some(bytes) = job["max_doc_bytes"].as_u64() {
        limits.max_doc_bytes = bytes;
    }
    let meta = match doc_meta(&file) {
        Ok(meta) => meta,
        Err(OpenError::PasswordNeeded) => {
            let mut feed = RefusedFeed("needs_password");
            report(
                corpus::build_for_file(&find_key, &file, &root, key, &mut feed, &DocMeta::default(), &limits),
                RenderStats::default(),
                0,
            );
            return;
        }
        Err(_) => DocMeta::default(),
    };
    let mut feed = WorkerFeed {
        path: file.clone(),
        key,
        renderer: None,
        check_hidden: true,
        hidden_check_failures: 0,
        identities,
    };
    let built = corpus::build_for_file(&find_key, &file, &root, key, &mut feed, &meta, &limits);
    report(built, feed.stats(), feed.hidden_check_failures);
}

/// A document already known not to be readable, so no worker need start to find out.
struct RefusedFeed(&'static str);

impl PageFeed for RefusedFeed {
    fn page_count(&mut self) -> Result<u32, FeedError> {
        Err(FeedError::Refused(self.0))
    }
    fn read(&mut self, _page: u32) -> Result<FedPage, FeedError> {
        Err(FeedError::Refused(self.0))
    }
    fn still_the_same_file(&mut self) -> bool {
        true
    }
}

fn report(built: std::io::Result<BuildOutcome>, stats: RenderStats, hidden_check_failures: u32) {
    let mut line = match built {
        Ok(outcome) => outcome_json(&outcome),
        // Most likely a full disk. What's written stays, and the next build goes on from there.
        Err(_) => json!({ "state": "failed", "code": "couldnt_write" }),
    };
    line["helper_peak_bytes"] = json!(recto_platform::isolate::process_memory(std::process::id()).map(|memory| memory.peak_footprint_bytes));
    line["worker_peak_bytes"] = json!(stats.peak_footprint_bytes);
    line["worker_spawns"] = json!(stats.spawns);
    line["worker_retirements"] = json!(stats.retirements);
    line["memory_kills"] = json!(stats.memory_kills);
    line["deadline_kills"] = json!(stats.deadline_kills);
    line["crashes"] = json!(stats.crashes);
    line["hidden_check_failures"] = json!(hidden_check_failures);
    say(&line);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: on a busy Mac a helper below the app's priority gets too little time to start a worker, and the index counts as failed: `recto wait indexing-done` says it couldn't be built, and it isn't tried for a minute. It's paused, as for running out of time, and goes on when asked. A failure that's about the install still says so.
    #[test]
    fn too_busy_pauses_a_build_rather_than_failing_it() {
        assert_eq!(outcome_json(&BuildOutcome::Paused(TOO_BUSY))["state"], "paused");
        assert_eq!(outcome_json(&BuildOutcome::Paused("out_of_time"))["state"], "paused");
        assert_eq!(outcome_json(&BuildOutcome::Paused("render_worker_missing"))["state"], "failed");
        assert_eq!(outcome_json(&BuildOutcome::Paused("render_unavailable"))["state"], "failed");
    }

    /// Failure: a worker that couldn't be started, or a failure that's remembered for only a moment, goes down as this page having failed, and so does every page in that moment. The corpus is then marked finished with those pages failed for good.
    #[test]
    fn a_failure_of_the_moment_writes_no_page_down_as_failed() {
        assert_eq!(read_failure(RenderFailure::Unavailable), FeedError::Cant("render_unavailable"));
        // A worker the busy Mac couldn't start in time is a paused build, not a page that failed.
        assert_eq!(read_failure(RenderFailure::TooBusyToStart), FeedError::Cant(TOO_BUSY));
        assert_eq!(whole_file(RenderFailure::TooBusyToStart), FeedError::Cant(TOO_BUSY));
        // These go down as failed, and are read again, because they may say more about the moment than the page.
        assert!(
            matches!(read_failure(RenderFailure::PageReadFailed { page: 1, reason: KillReason::Deadline }), FeedError::Page(reason) if reason.is_temporary())
        );
        assert!(
            matches!(read_failure(RenderFailure::PageReadFailed { page: 1, reason: KillReason::Crashed }), FeedError::Page(reason) if reason.is_temporary())
        );
        assert!(matches!(read_failure(RenderFailure::DocumentStopped), FeedError::Page(reason) if reason.is_temporary()));
        // This is the page's own doing, and stays.
        assert!(
            matches!(read_failure(RenderFailure::PageReadFailed { page: 1, reason: KillReason::MemoryLimit }), FeedError::Page(reason) if !reason.is_temporary())
        );
    }

    /// A renderer that answers from a script, counting `too_busy` as the real one does: each busy answer moves it and stays moved.
    struct Scripted {
        answers: std::vec::IntoIter<(Result<u32, RenderFailure>, bool)>,
        too_busy: u32,
        tries: u32,
    }

    fn scripted(answers: Vec<(Result<u32, RenderFailure>, bool)>) -> Scripted {
        Scripted {
            answers: answers.into_iter(),
            too_busy: 0,
            tries: 0,
        }
    }

    fn ask_scripted(renderer: &mut Scripted) -> Result<u32, FeedError> {
        ask_scripted_as(renderer, whole_file)
    }

    fn ask_scripted_as(renderer: &mut Scripted, meaning: fn(RenderFailure) -> FeedError) -> Result<u32, FeedError> {
        asked(
            renderer,
            |r| r.too_busy,
            |r| {
                r.tries += 1;
                let (answer, counted) = r.answers.next().unwrap_or((Err(RenderFailure::TooBusyToStart), true));
                r.too_busy += u32::from(counted);
                answer
            },
            meaning,
        )
    }

    /// Failure: a page read (not only the page count) whose worker hangs before hello ends as `render_unavailable` only if it reaches the outcome as failed; mapped as a pause it would hold the build for ever, as the page count did.
    #[test]
    fn a_read_whose_worker_never_says_hello_is_a_failed_build_too() {
        let busy = (Err(RenderFailure::TooBusyToStart), true);
        let mut renderer = scripted(vec![busy, busy, (Err(RenderFailure::Unavailable), false)]);
        let error = ask_scripted_as(&mut renderer, read_failure).unwrap_err();
        assert_eq!(error, FeedError::Cant("render_unavailable"));
        let FeedError::Cant(code) = error else { unreachable!() };
        assert_eq!(outcome_json(&BuildOutcome::Paused(code))["state"], "failed");
    }

    /// Failure: `WorkerFeed::read` goes back to asking once (or maps a failure its own way) and a worker that never says hello pauses every build for ever, whatever `asked` does. Through the feed itself, with a worker that has no time to say hello (a start timeout of nothing; the file is never opened by it, so any bytes will do): the build ends as failed, and no page is written down as failed.
    #[test]
    fn the_feeds_read_ends_a_worker_that_never_says_hello_as_a_failed_build() {
        let folder = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-indexer-hello-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("doc.pdf");
        std::fs::write(&path, b"not read: no worker starts").unwrap();
        let policy = Policy {
            start_timeout: Duration::ZERO,
            ..Policy::default()
        };
        let renderer = DocRenderer::new(&path, None, ServiceConfig::discover().unwrap(), policy).unwrap();
        let mut feed = WorkerFeed {
            path: path.clone(),
            key: CorpusKey::of_file(&path).unwrap(),
            renderer: Some(renderer),
            check_hidden: false,
            hidden_check_failures: 0,
            identities: OpenedIdentities::of(&path),
        };
        let error = feed.read(0).unwrap_err();
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(error, FeedError::Cant("render_unavailable"));
        let FeedError::Cant(code) = error else { unreachable!() };
        assert_eq!(outcome_json(&BuildOutcome::Paused(code))["state"], "failed");
    }

    /// Failure: a worker that hangs before hello pauses the build for ever (each run is a new process, so the service's count never gets past one start), and `recto wait indexing-done` never says it couldn't be built. The busy starts before it stay counted, and don't turn the `Unavailable` into a pause.
    #[test]
    fn a_worker_that_never_says_hello_is_a_failed_build_after_the_busy_starts() {
        let busy = (Err(RenderFailure::TooBusyToStart), true);
        let mut renderer = scripted(vec![busy, busy, (Err(RenderFailure::Unavailable), false)]);
        let error = ask_scripted(&mut renderer).unwrap_err();
        assert_eq!(error, FeedError::Cant("couldnt_open"));
        let FeedError::Cant(code) = error else { unreachable!() };
        assert_eq!(outcome_json(&BuildOutcome::Paused(code))["state"], "failed");
    }

    /// Failure: a busy Mac whose next start works loses the build, as it did when the first busy answer ended the run.
    #[test]
    fn a_start_that_works_after_a_busy_one_goes_on() {
        let mut renderer = scripted(vec![(Err(RenderFailure::TooBusyToStart), true), (Ok(7), false)]);
        assert_eq!(ask_scripted(&mut renderer), Ok(7));
    }

    /// Failure: a busy Mac is called a failure (a failure that moved the busy count must pause, not fail), or a Mac that is busy for ever makes the helper spin without end.
    #[test]
    fn a_busy_mac_pauses_the_build_and_the_asking_is_bounded() {
        let mut renderer = scripted(vec![(Err(RenderFailure::Unavailable), true)]);
        assert_eq!(ask_scripted(&mut renderer), Err(FeedError::Cant(TOO_BUSY)));
        assert_eq!(outcome_json(&BuildOutcome::Paused(TOO_BUSY))["state"], "paused");
        let mut renderer = scripted(vec![]);
        assert_eq!(ask_scripted(&mut renderer), Err(FeedError::Cant(TOO_BUSY)));
        assert_eq!(renderer.tries, 2 * u32::from(MAX_BUSY), "asks again no more than a start's and an open's bound");
    }

    /// Failure: the identities are read from whatever is at the path when the first one is asked for (a file swapped in after the key check is identified as the
    /// version the key names), or opened for a build that never asks (a cost for every build), or read twice.
    #[test]
    fn the_identities_open_the_stamped_version_once_and_only_when_asked() {
        let folder = recto_testpdf::TempDir::new("recto-indexer-ids-");
        let path = folder.join("doc.pdf");
        std::fs::write(&path, recto_testpdf::pages(&[recto_testpdf::Page::new("BT /F1 12 Tf 72 700 Td (A) Tj ET")])).unwrap();
        let mut identities = OpenedIdentities::of(&path);
        assert!(identities.opened.is_none(), "nothing is opened before the first is asked for");
        assert_eq!(identities.files(&path).map(|files| files.info().pages), Some(1));
        assert!(matches!(identities.opened, Some(Some(_))));
        // Another version put at the path after the stamp: nothing is identified for it.
        let mut stale = OpenedIdentities::of(&path);
        std::fs::write(
            &path,
            recto_testpdf::pages(&[recto_testpdf::Page::new("BT /F1 12 Tf 72 700 Td (B) Tj ET"), recto_testpdf::Page::new("")]),
        )
        .unwrap();
        assert!(stale.files(&path).is_none());
        assert!(matches!(stale.opened, Some(None)), "and it isn't tried again");
    }
}
