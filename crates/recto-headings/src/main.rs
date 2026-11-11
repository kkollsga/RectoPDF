//! `recto-headings`: finds one document's headings for generated contents, and exits. Started by the session, one at a time, and only when someone asks: never on open.
//!
//! It takes its job as one line on stdin, so a file's name stays out of `ps`, and it ends as soon as stdin closes, so it can't outlive whoever started it. It says how it went as two lines on stdout: a report, then the contents in `recto_index::generated`'s form, which the session reads as hostile and writes to disk itself. It writes nothing.
//!
//! **Limits, from outside:** the session reads this process's footprint and kills it past its limit, and past its time. It's started with `MallocLargeCache=0`, which took distillPDF's page path on the 383-page NASA report from 281 MiB to 39 MiB of footprint, since the system allocator otherwise keeps freed large blocks dirty. As a second line it watches itself, and leaves past the limit it was given. The render worker it uses for the check for hidden text has limits of its own, half a reader's.

use recto_headings::{Failure, InkSource, PageInk, Scan, find_headings, keep_visible};
use recto_index::corpus::CorpusKey;
use recto_render_service::{DocRenderer, Policy, RenderStats, ServiceConfig};
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Longest job line.
const MAX_JOB_BYTES: u64 = 128 * 1024;
/// The render worker's limit, as the indexer's: only the check for hidden text draws.
const WORKER_MEMORY_LIMIT: u64 = 512 * 1024 * 1024;
const WORKER_PAGE_DEADLINE: Duration = Duration::from_secs(10);
/// How often this process looks at its own footprint.
const SELF_WATCH_EVERY: Duration = Duration::from_millis(50);
/// The exit code for having passed its own limit. The session kills it first, as a rule.
const EXIT_OVER_LIMIT: i32 = 3;

/// The render worker, started on the first page that has headings.
struct WorkerInk {
    path: PathBuf,
    renderer: Option<DocRenderer>,
    /// It couldn't be started: no page can be checked, so none of their headings are kept.
    unavailable: bool,
}

impl WorkerInk {
    fn stats(&self) -> RenderStats {
        self.renderer.as_ref().map(DocRenderer::stats).unwrap_or_default()
    }
}

impl InkSource for WorkerInk {
    /// A page the worker couldn't answer for, past its deadline on a starved machine or with the worker gone, gets one more go with a fresh worker: otherwise every heading on it is dropped unseen, as one that can't be checked isn't kept (CI, 2026-11-11: five of six headings, on a loaded virtual machine).
    fn page(&mut self, page: u32) -> Option<PageInk> {
        if let Some(read) = self.page_once(page).filter(|read| read.checked) {
            return Some(read);
        }
        self.renderer = None;
        self.page_once(page)
    }
}

impl WorkerInk {
    fn page_once(&mut self, page: u32) -> Option<PageInk> {
        if self.unavailable {
            return None;
        }
        if self.renderer.is_none() {
            let policy = Policy {
                memory_limit_bytes: WORKER_MEMORY_LIMIT,
                heavy_limit_bytes: WORKER_MEMORY_LIMIT,
                page_deadline: WORKER_PAGE_DEADLINE,
                ..Policy::default()
            };
            match ServiceConfig::discover().and_then(|config| DocRenderer::new(&self.path, None, config, policy)) {
                Ok(renderer) => self.renderer = Some(renderer),
                Err(_) => {
                    self.unavailable = true;
                    return None;
                }
            }
        }
        let characters = self.renderer.as_mut()?.page_characters(page, true, true).ok()?;
        Some(PageInk {
            chars: characters.chars,
            boxes: characters.boxes,
            frame: characters.frame,
            checked: characters.hidden_checked,
        })
    }
}

fn job() -> Option<Value> {
    let mut line = String::new();
    std::io::stdin().lock().take(MAX_JOB_BYTES).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn say(lines: &[&str]) {
    let mut out = std::io::stdout().lock();
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
    let _ = out.flush();
}

fn failed(code: &str) {
    say(&[&json!({ "state": "failed", "code": code }).to_string()]);
}

fn peak() -> Option<u64> {
    recto_platform::isolate::process_memory(std::process::id()).map(|memory| memory.peak_footprint_bytes)
}

fn main() {
    let Some(job) = job() else { return failed("bad_job") };
    let (Some(file), Some(key)) = (job["file"].as_str().map(PathBuf::from), job["key"].as_str().and_then(CorpusKey::from_hex)) else {
        return failed("bad_job");
    };
    if job["background"] != false {
        recto_platform::run_in_background();
    }
    // Whoever started this has gone or wants it stopped.
    let _ = std::thread::Builder::new().name("stdin".into()).stack_size(64 * 1024).spawn(|| {
        let mut sink = [0u8; 64];
        while std::io::stdin().lock().read(&mut sink).is_ok_and(|count| count > 0) {}
        std::process::exit(0);
    });
    if let Some(limit) = job["memory_limit"].as_u64() {
        let _ = std::thread::Builder::new().name("watch".into()).stack_size(64 * 1024).spawn(move || {
            loop {
                if recto_platform::footprint_bytes().is_some_and(|bytes| bytes > limit) {
                    std::process::exit(EXIT_OVER_LIMIT);
                }
                std::thread::sleep(SELF_WATCH_EVERY);
            }
        });
    }
    let scan = if job["scan"] == "typography" { Scan::Typography } else { Scan::Full };
    let started = Instant::now();
    report(&file, key, scan, started);
}

fn same_file(file: &Path, key: CorpusKey) -> bool {
    CorpusKey::of_file(file).is_ok_and(|now| now == key)
}

fn report(file: &Path, key: CorpusKey, scan: Scan, started: Instant) {
    if !same_file(file, key) {
        return say(&[&json!({ "state": "changed" }).to_string()]);
    }
    let headings = match find_headings(file, scan, None, &mut |_, _| {}) {
        Ok(headings) => headings,
        Err(Failure::Changed) => return say(&[&json!({ "state": "changed" }).to_string()]),
        Err(failure) => return say(&[&json!({ "state": "refused", "code": failure.code(), "helper_peak_bytes": peak() }).to_string()]),
    };
    let distilled_ms = started.elapsed().as_millis() as u64;
    let found = headings.found.len();
    let mut ink = WorkerInk {
        path: file.to_path_buf(),
        renderer: None,
        unavailable: false,
    };
    let generated = keep_visible(headings, &mut ink);
    // Read to the end on another version of the file, it's about neither.
    if !same_file(file, key) {
        return say(&[&json!({ "state": "changed" }).to_string()]);
    }
    let stats = ink.stats();
    let line = json!({
        "state": "ready",
        "found": found,
        "kept": generated.entries.len(),
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "distilled_ms": distilled_ms,
        "helper_peak_bytes": peak(),
        "worker_peak_bytes": stats.peak_footprint_bytes,
        "worker_spawns": stats.spawns,
        "worker_unavailable": ink.unavailable,
    });
    say(&[&line.to_string(), &generated.encode(key)]);
}
