//! `recto-headings`: finds one document's headings for generated contents, and exits. Started by the session, one at a time, and only when someone asks: never on open.
//!
//! **The document comes on descriptor 3**, read-only, opened by the host after it checked the file's key, and read here as `/dev/fd/3` (`recto_platform::helper`). No job names a path, and a job that does is refused (`unsupported_job`): a walled helper has nothing to open by a name. It takes its job as one line on stdin, so nothing of it is in `ps`, and it ends as soon as stdin closes, so it can't outlive whoever started it.
//!
//! **What it says,** after a hello line (`{"state":"hello","walled":…}`, before it reads the job, so the host can check it from outside first): for the `headings` job, one line a page that has headings and one end line (`answer`); for `count`, the page count. It finds headings and says them; **whether they can be seen is the session's check,** which needs a render worker that a walled helper can't start (`recto_index::generated::visible`). It writes nothing.
//!
//! **Walls:** laid out as `RectoStructure.app` it puts the renderer's walls up before any job is read, and in a release build it always must. A bare debug build, from `target/`, has none to put up and says `walled: false`, which the host refuses.
//!
//! **Limits, from outside:** the session reads this process's footprint and kills it past its limit, and past its time. It's started with `MallocLargeCache=0`, which took distillPDF's page path on the 383-page NASA report from 281 MiB to 39 MiB of footprint, since the system allocator otherwise keeps freed large blocks dirty. As a second line it watches itself, and leaves past the limit it was given.

mod hooks;

use recto_headings::answer::{end_line, page_line};
use recto_headings::{Failure, Scan, find_headings};
use recto_index::generated::MAX_ENTRIES;
use recto_platform::helper;
use recto_platform::helper_app::{exit, walls_required};
use recto_platform::wall;
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::time::Instant;

/// Longest job line.
const MAX_JOB_BYTES: u64 = 128 * 1024;
/// How often this process looks at its own footprint.
const SELF_WATCH_EVERY: std::time::Duration = std::time::Duration::from_millis(50);
/// The exit code for having passed its own limit. The session kills it first, as a rule.
const EXIT_OVER_LIMIT: i32 = 3;

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

/// Puts the walls up when this is the structure helper laid out in an app (`RectoStructure.app`, `docs/engineering/sandbox.md`) or any release build, and says whether it did. Before any job is
/// read, so no byte of a document reaches a helper whose walls aren't up: it leaves with the code that says which wall was missing. A bare debug helper, run from `target/` or by a test,
/// has none to put up, and says so.
fn put_up_walls() -> bool {
    if !walls_required(std::env::current_exe().ok().as_deref(), recto_platform::RELEASE_BUILD) {
        return false;
    }
    recto_platform::isolate::harden_this_process();
    match wall::put_up() {
        Ok(_) => true,
        Err(wall::NoWall::NotSandboxed) => recto_platform::leave::leave_now(exit::NO_WALL),
        Err(wall::NoWall::NothingToDrop) => recto_platform::leave::leave_now(exit::NO_RIGHTS_TO_DROP),
        Err(wall::NoWall::LaunchdReachable) => recto_platform::leave::leave_now(exit::LAUNCHD_REACHABLE),
    }
}

fn main() {
    let walled = put_up_walls();
    say(&[&json!({ "state": "hello", "walled": walled }).to_string()]);
    let Some(job) = job() else { return failed("bad_job") };
    // Two jobs, both on the document the host put on descriptor 3. Anything else, a job that names a file included, is refused whether this helper is walled or not.
    let kind = match job["kind"].as_str() {
        Some(kind @ ("count" | "headings" | "blocks")) if job.get("file").is_none() => kind,
        _ => return failed("unsupported_job"),
    };
    let faults = hooks::apply(&job);
    // A helper that can't start its watchers doesn't run without them: nothing would stop it when its parent has gone, or when it grows.
    let stdin_watch = start_watcher("stdin", &faults, || {
        let mut sink = [0u8; 64];
        while std::io::stdin().lock().read(&mut sink).is_ok_and(|count| count > 0) {}
        // Whoever started this has gone or wants it stopped.
        recto_platform::leave::leave_now(0)
    });
    let memory_watch = job["memory_limit"].as_u64().is_none_or(|limit| {
        start_watcher("watch", &faults, move || {
            loop {
                if recto_platform::footprint_bytes().is_some_and(|bytes| bytes > limit) {
                    recto_platform::leave::leave_now(EXIT_OVER_LIMIT);
                }
                std::thread::sleep(SELF_WATCH_EVERY);
            }
        })
    });
    if !(stdin_watch && memory_watch) {
        return failed("no_watch");
    }
    hooks::busy(&job);
    let Ok(document) = helper::inherited_document() else {
        return say(&[&json!({ "state": "refused", "code": "no_document" }).to_string()]);
    };
    if kind == "count" {
        count();
    } else if kind == "blocks" {
        blocks(recto_headings::blocks::scan_of(&job));
    } else {
        let scan = if job["scan"] == "typography" { Scan::Typography } else { Scan::Full };
        headings(scan, Instant::now());
    }
    // Held to the end: the open file is the descriptor the path above names.
    drop(document);
}

/// The `count` job: the pages of the document on descriptor 3, opened by `/dev/fd/3`. Nothing is opened by a name, so a walled helper can do it, and the number is read from the same bytes
/// the host chose, not from whatever is at a path now.
fn count() {
    match recto_headings::open(Path::new(helper::DOCUMENT_PATH)) {
        Ok(opened) => say(&[&json!({ "state": "counted", "pages": opened.page_count(), "helper_peak_bytes": peak() }).to_string()]),
        Err(failure) => say(&[&json!({ "state": "refused", "code": failure.code(), "helper_peak_bytes": peak() }).to_string()]),
    }
}

/// The `blocks` job: a line a page and one end line (`recto_headings::blocks`). A page that couldn't be read is a line with no blocks, never the end of the job.
fn blocks(scan: Scan) {
    let done = recto_headings::blocks::run(Path::new(helper::DOCUMENT_PATH), scan, &mut std::io::stdout().lock(), Instant::now());
    match done {
        Ok(()) => {}
        Err(Failure::Changed) => say(&[&json!({ "state": "changed" }).to_string()]),
        Err(failure) => say(&[&json!({ "state": "refused", "code": failure.code(), "helper_peak_bytes": peak() }).to_string()]),
    }
}

/// The `headings` job: what distillPDF finds, a line a page that has any and then one end line. The page lines come in page order; a page's line is cut to the cap, which is said in the end.
fn headings(scan: Scan, started: Instant) {
    let found = match find_headings(Path::new(helper::DOCUMENT_PATH), scan, None, &mut |_, _| {}) {
        Ok(found) => found,
        Err(Failure::Changed) => return say(&[&json!({ "state": "changed" }).to_string()]),
        Err(failure) => return say(&[&json!({ "state": "refused", "code": failure.code(), "helper_peak_bytes": peak() }).to_string()]),
    };
    let distilled_ms = started.elapsed().as_millis() as u64;
    let (mut sent, mut truncated) = (0, found.truncated);
    let mut at = 0;
    while at < found.found.len() {
        let page = found.found[at].page;
        let end = at + found.found[at..].iter().take_while(|entry| entry.page == page).count();
        let (line, kept) = page_line(page, &found.found[at..end]);
        if kept < end - at {
            truncated = true;
        }
        if kept > 0 {
            say(&[&line]);
            sent += kept;
        }
        at = end;
    }
    debug_assert!(sent <= MAX_ENTRIES);
    say(&[&end_line(&found, sent, truncated, distilled_ms, peak())]);
}

/// Starts one of the helper's two watchers, which leave the process at once (`leave_now`, not `exit`: an exit handler or a destructor waiting on what the busy main thread holds would keep it). False if the thread couldn't be made.
fn start_watcher(name: &str, faults: &hooks::Faults, body: impl FnOnce() + Send + 'static) -> bool {
    !faults.spawn_fails && std::thread::Builder::new().name(name.into()).stack_size(64 * 1024).spawn(body).is_ok()
}
