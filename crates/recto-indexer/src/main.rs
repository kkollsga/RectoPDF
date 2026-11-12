//! `recto-indexer`: builds one document's search corpus, and exits. Started by the session, one at a time. It takes its job as one line on stdin, so a file's name stays out of `ps`, and it ends as soon as stdin closes, so it doesn't outlive whoever started it however they ended. It says how it went as one line of JSON on stdout. Anything else, such as being killed, is safe: `recto_index::corpus` goes on from where it stopped next time.
//!
//! It holds no PDFium. Pages come from a worker like any other, under limits of its own. It's a process of its own, rather than a thread in the session, because what it uses goes when it exits, and a page of two million characters takes tens of megabytes to read: the session's footprint stays where it was.

use recto_core::OpenError;
use recto_index::corpus::{self, BuildLimits, BuildOutcome, CorpusKey, DocMeta, FailReason, FedPage, FeedError, PageFeed};
use recto_render_service::{DocRenderer, KillReason, Policy, RenderFailure, RenderStats, ServiceConfig};
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

    /// What a failure that isn't about the file means, given how many times the worker had been too busy before the step that failed. A page's own deadline stays with that page, which is read again later.
    fn unless_too_busy(&self, before: u32, error: FeedError) -> FeedError {
        match error {
            FeedError::Cant(_) if self.stats().too_busy > before => FeedError::Cant(TOO_BUSY),
            error => error,
        }
    }
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
        RenderFailure::SourceChanged => FeedError::FileChanged,
        _ => FeedError::Cant("couldnt_open"),
    }
}

/// What a failure to read one page means for the build. Only what's a fact about that page gets written down as one, and whether it might read another time goes with it. What's about this moment rather than any page stops the build where it is, with nothing written, so it goes on from that page later.
fn read_failure(failure: RenderFailure) -> FeedError {
    match failure {
        RenderFailure::Unavailable => FeedError::Cant("render_unavailable"),
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
        let busy = self.stats().too_busy;
        let counted = self.renderer()?.page_count();
        counted.map_err(|failure| self.unless_too_busy(busy, whole_file(failure)))
    }

    fn read(&mut self, page: u32) -> Result<FedPage, FeedError> {
        let check = self.check_hidden;
        let busy = self.stats().too_busy;
        let read = self.renderer()?.page_characters(page, true, check);
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
            Err(failure) => Err(self.unless_too_busy(busy, read_failure(failure))),
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
    let mut limits = BuildLimits::default();
    if let Some(bytes) = job["max_doc_bytes"].as_u64() {
        limits.max_doc_bytes = bytes;
    }
    let meta = match doc_meta(&file) {
        Ok(meta) => meta,
        Err(OpenError::PasswordNeeded) => {
            let mut feed = RefusedFeed("needs_password");
            report(
                corpus::build_for_file(&file, &root, key, &mut feed, &DocMeta::default(), &limits),
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
    };
    let built = corpus::build_for_file(&file, &root, key, &mut feed, &meta, &limits);
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
}
