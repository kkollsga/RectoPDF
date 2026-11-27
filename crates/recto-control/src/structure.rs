//! The runner for any job of the structure helper (`recto-headings`, laid out as `RectoStructure.app`): starts it walled, checks it from outside, gives it its job, and reads what it says
//! as hostile. Generated contents is the first job ([`headings`]); a table of contents of tables would be the second, and use this as it is.
//!
//! **Never an unwalled one.** [`Structure::installed`] is the only finder, and it looks for the helper in its app, beside this program's own app and nowhere else: no fallback to a bare
//! `recto-headings`, whatever is in the environment. A helper that says it isn't walled, or that the kernel doesn't show in a sandbox (`structure_not_sandboxed`), or can't be confirmed (`structure_unconfirmed`), or that may read the document by its name, is
//! killed before it is given its job: [`talk`] writes the job only after [`outside_check`] says yes. The document's descriptor 3 is the helper's from the spawn (it's inherited, and can't be
//! given later), so what's withheld until the check passes is the job: until then the helper has the descriptor and no instruction, and a helper that fails the check is killed holding it.
//!
//! **The document is a descriptor, not a path.** It's opened here, read-only, by the file wall (`open_regular`), its key checked against the one asked for, and handed over as the helper's
//! descriptor 3 (`recto_platform::helper::spawn_with_document`); the key is looked at again when the helper has ended, so a file that changed while it was read is `Changed`.
//!
//! **What it says is bounded on every side:** a line over [`MAX_LINE_BYTES`], an answer over [`MAX_ANSWER_BYTES`], a line after the end, or a bad line kills it at once
//! (`bad_answer`); its footprint is read every [`WATCH_EVERY`] and it's killed past [`MEMORY_LIMIT`], past its time, or when it's no longer wanted. After the end line it has [`END_GRACE`]
//! to exit. The one thread here, [`READER_THREAD`], lives as long as the helper does.

pub mod blocks;
pub mod headings;

use recto_index::corpus::CorpusKey;
use recto_index::generated::visible::MAX_LINE_BYTES;
use recto_platform::helper::{self, Helper, Priority, Spawned};
use recto_platform::wall;
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::time::{Duration, Instant};

/// The helper's own footprint, which is distillPDF reading one page at a time. With `MallocLargeCache=0`, all 141 PDFs of bench100 peaked under 100 MiB: see `docs/engineering/generated-contents.md`.
pub const MEMORY_LIMIT: u64 = 512 * 1024 * 1024;
/// A job's time: three minutes, and a second for each page, up to half an hour, so one file can't hold the only slot for long. It's there for a helper that's stuck, not to hurry one: a busy Mac slows it, and a debug build is many times slower. The 383-page NASA report took about 15 s in a release build at normal priority.
pub fn time_limit(pages: u32) -> Duration {
    (Duration::from_secs(180) + Duration::from_secs(1) * pages).min(Duration::from_secs(30 * 60))
}
/// How often the helper is looked at while it runs.
const WATCH_EVERY: Duration = Duration::from_millis(25);
/// How long a helper has to say hello. It reads nothing before it does, so this is only starting a process, which a loaded Mac can make slow.
const HELLO_WAIT: Duration = Duration::from_secs(30);
/// How long a helper that has said its end has to exit, before it's killed and its answer stands.
const END_GRACE: Duration = Duration::from_secs(10);
/// Everything it may say, in all: [`headings`]'s answer is at most [`recto_index::generated::MAX_ENTRIES`] of the longest titles, with their JSON, and this is well past that.
pub const MAX_ANSWER_BYTES: u64 = 4 * 1024 * 1024;
/// The reader thread's stack. serde_json refuses nesting past 128, which takes more than 64 KiB in a debug build, so this is what a 64 KiB line of `[[[` is read on.
pub const READER_STACK: usize = 256 * 1024;
/// The reader thread's name, for the thread table that shows nothing runs when nothing is asked.
pub const READER_THREAD: &str = "recto-headings-out";
/// The names of the numbers this session measures itself and reports last; an answer that uses one is a bad answer.
pub const SESSION_NUMBERS: [&str; 3] = ["walled", "elapsed_ms", "helper_peak_watched_bytes"];
/// What the helper is started with, and nothing else ever: not `spawn_helper`, which passes the render worker's and PDFium's locations on, and never `APP_SANDBOX_CONTAINER_ID`.
/// `MallocLargeCache=0` must be there from the start, as the allocator reads it once: without it, freed large blocks stay in the footprint.
pub const HELPER_ENV: [(&str, &str); 1] = [("MallocLargeCache", "0")];

/// Why a structure helper wasn't used. Rust's words, as codes, and each means the job was never given to it (the document's descriptor is inherited at the spawn, before the check).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallsDown {
    /// It isn't in its sandbox, or doesn't say it is: this copy is signed wrong.
    NotSandboxed,
    /// It was killed on starting, and this process is in a sandbox of its own: the one it's in has no room for another.
    InsideAnotherSandbox,
    /// It couldn't put its walls up on this macOS.
    WallsFailed,
    /// The system wouldn't start it.
    StartRefused,
    /// Its walls couldn't be confirmed: a hello that isn't one, a probe that couldn't tell, or a document its sandbox lets it read by name. Not that the copy is signed wrong: ask again.
    Unconfirmed,
}

impl WallsDown {
    pub fn code(self) -> &'static str {
        match self {
            Self::NotSandboxed => "structure_not_sandboxed",
            Self::InsideAnotherSandbox => "structure_inside_another_sandbox",
            Self::WallsFailed => "structure_walls_failed",
            Self::StartRefused => "structure_start_refused",
            Self::Unconfirmed => "structure_unconfirmed",
        }
    }
}

/// What a line of an answer comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Said {
    /// Something that goes on.
    More,
    /// The end of a good answer: nothing may follow.
    End,
    Refused(String),
    Changed,
    Failed(String),
}

/// The job's own reader of what the helper says after its hello: each line, one at a time, believing nothing. A line it won't have is `Err`, and ends the helper.
pub trait Answer: Send + 'static {
    fn take(&mut self, line: &[u8]) -> Result<Said, &'static str>;
    /// The most one line may be, hello included. Over it the helper is killed.
    fn max_line_bytes(&self) -> usize {
        MAX_LINE_BYTES
    }
    /// The most it may say in all.
    fn max_answer_bytes(&self) -> u64 {
        MAX_ANSWER_BYTES
    }
    /// Why the answer itself stopped the job: it refused a line (`take` was `Err`) for a reason of its own, or something it was handed (a store it writes to) said stop. It wins over `bad_answer`, which its `Err` would otherwise be reported as.
    fn stopped_for(&self) -> Option<&'static str> {
        None
    }
    /// The numbers and bools of what it said, for measuring: never words.
    fn numbers(&self) -> Value {
        Value::Null
    }
}

/// How a job ended.
#[derive(Debug)]
pub enum Ended<A> {
    Answered(A),
    /// This version of the file won't have an answer, for this reason.
    Refused(String),
    Changed,
    /// Not about the file, or stopped at a limit.
    Failed(String),
}

/// The structure helper: where it is, and at what level it runs.
#[derive(Clone, Debug)]
pub struct Structure {
    exe: std::path::PathBuf,
    priority: Priority,
}

impl Structure {
    /// The helper in this program's own app: `Contents/Helpers/RectoStructure.app` beside the `MacOS` folder this program is in, through any link to it. Nowhere else, and no fallback.
    pub fn installed(priority: Priority) -> Option<Self> {
        let this = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
        let exe = recto_platform::helper_app::structure_helper(this.parent()?)?;
        exe.is_file().then_some(Self { exe, priority })
    }

    /// A helper at `exe`, for a test: the one laid out in `target/Helpers`, or a stand-in.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn at(exe: std::path::PathBuf, priority: Priority) -> Self {
        Self { exe, priority }
    }

    /// Runs one job on `document`, whose key was `key` when it was asked for. `job` is the line it's given; `answer` reads what it says. The numbers are for measuring.
    pub fn run<A: Answer>(&self, document: &Path, key: CorpusKey, job: &Value, answer: A, deadline: Instant, stop: &AtomicBool) -> (Ended<A>, Value) {
        let failed = |code: &str| (Ended::Failed(code.to_string()), Value::Null);
        if document.to_str().is_none() {
            return (Ended::Refused("couldnt_read".into()), Value::Null);
        }
        // Opened fresh, by the file wall, and read-only: this is the descriptor the helper is given, so it's the one whose key is looked at, before and after.
        let Ok(file) = recto_platform::document_file::open_regular(document) else {
            return (Ended::Changed, Value::Null);
        };
        if CorpusKey::of_open(&file).ok() != Some(key) {
            return (Ended::Changed, Value::Null);
        }
        let spawned = match spawn(&self.exe, self.priority, &file) {
            Ok(spawned) => spawned,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return failed(WallsDown::StartRefused.code()),
            Err(_) => return failed("helper_missing"),
        };
        let (ended, numbers) = talk(spawned, job, answer, deadline, stop, |pid, walled| outside_check(pid, walled, document));
        let moved = matches!(ended, Ended::Answered(_) | Ended::Refused(_)) && CorpusKey::of_open(&file).ok() != Some(key);
        if moved { (Ended::Changed, numbers) } else { (ended, numbers) }
    }
}

/// Starts the helper with `document` on its descriptor 3, at `priority` and with [`HELPER_ENV`].
pub(crate) fn spawn(exe: &Path, priority: Priority, document: &std::fs::File) -> std::io::Result<Spawned> {
    let env: Vec<(&str, &OsStr)> = HELPER_ENV.iter().map(|(name, value)| (*name, OsStr::new(value))).collect();
    helper::spawn_with_document(exe, &env, priority, document)
}

/// Whether a helper that said hello may be given its job: only if it says it's walled, the kernel has it in a sandbox, and that sandbox refuses it the document by its name, both as
/// given and as it really is. Anything else is not. The kernel showing no sandbox at all is [`WallsDown::NotSandboxed`] (this copy is signed wrong); a hello that isn't a hello or says
/// it isn't walled, a probe that couldn't tell, or a document the sandbox lets it read by name is [`WallsDown::Unconfirmed`]: not a reason to reinstall.
fn admit(walled: Option<bool>, sandboxed: i32, may_read_by_name: &[i32]) -> Result<(), WallsDown> {
    if sandboxed == 0 {
        Err(WallsDown::NotSandboxed)
    } else if walled == Some(true) && sandboxed == 1 && may_read_by_name.iter().all(|denied| *denied == 1) {
        Ok(())
    } else {
        Err(WallsDown::Unconfirmed)
    }
}

/// The check from outside, by the helper's process id, before it's given a job: the kernel's word against the helper's own.
fn outside_check(pid: u32, walled: Option<bool>, document: &Path) -> Result<(), WallsDown> {
    let real = std::fs::canonicalize(document).unwrap_or_else(|_| document.to_path_buf());
    admit(
        walled,
        wall::pid_sandboxed(pid),
        &[wall::pid_may_read(pid, document), wall::pid_may_read(pid, &real)],
    )
}

/// What a helper that ended before it said hello says by how it ended: the exit code of a wall that wouldn't go up, or a kill by the system on a start inside another sandbox.
fn why_it_ended(code: Option<i32>, in_a_sandbox: bool) -> &'static str {
    use recto_platform::helper_app::exit;
    match code {
        Some(exit::NO_WALL) => WallsDown::NotSandboxed.code(),
        Some(exit::NO_RIGHTS_TO_DROP | exit::LAUNCHD_REACHABLE) => WallsDown::WallsFailed.code(),
        None if in_a_sandbox => WallsDown::InsideAnotherSandbox.code(),
        _ => "helper_stopped",
    }
}

/// Reads one line, without its newline, into `buf`. `Ok(false)` at the end of input. Over `cap` (the answer's own: [`MAX_LINE_BYTES`] unless it says more), over what's left of `budget` (everything it may say in all), or a last line
/// that never ended, is bad, and nothing past the cap is ever held.
fn next_line(input: &mut impl BufRead, buf: &mut Vec<u8>, budget: &mut u64, cap: usize) -> Result<bool, &'static str> {
    buf.clear();
    loop {
        let Ok(available) = input.fill_buf() else { return Ok(false) };
        if available.is_empty() {
            return if buf.is_empty() { Ok(false) } else { Err("bad_answer") };
        }
        let (used, done) = match available.iter().position(|byte| *byte == b'\n') {
            Some(at) => (at + 1, true),
            None => (available.len(), false),
        };
        let body = if done { used - 1 } else { used };
        if buf.len() + body > cap || used as u64 > *budget {
            return Err("bad_answer");
        }
        *budget -= used as u64;
        buf.extend_from_slice(&available[..body]);
        input.consume(used);
        if done {
            return Ok(true);
        }
    }
}

/// What the reader and the watcher both look at.
#[derive(Default)]
struct Flags {
    /// A line it won't have: the helper is killed.
    bad: AtomicBool,
    /// A terminal line was read: the helper has [`END_GRACE`] to go.
    ended: AtomicBool,
}

/// The first line is the hello, and the rest go to `answer`; each is read as hostile. Returns the answer and how it ended, if it did.
fn read_lines<A: Answer>(mut input: impl BufRead, mut answer: A, hello: SyncSender<Option<bool>>, flags: &Flags) -> (A, Option<Said>) {
    let (mut buf, mut budget, mut terminal) = (Vec::new(), answer.max_answer_bytes(), None);
    let cap = answer.max_line_bytes();
    match next_line(&mut input, &mut buf, &mut budget, cap) {
        // Nothing before the end of input: the sender is dropped, which the other side reads as a helper that stopped.
        Ok(false) => return (answer, None),
        Ok(true) => {
            let walled = serde_json::from_slice::<Value>(&buf)
                .ok()
                .filter(|line| line.as_object().is_some_and(|object| object.len() == 2) && line["state"] == "hello")
                .and_then(|line| line["walled"].as_bool());
            if walled.is_none() {
                flags.bad.store(true, Ordering::SeqCst);
            }
            let _ = hello.send(walled);
            if walled.is_none() {
                return (answer, None);
            }
        }
        Err(_) => {
            flags.bad.store(true, Ordering::SeqCst);
            let _ = hello.send(None);
            return (answer, None);
        }
    }
    loop {
        match next_line(&mut input, &mut buf, &mut budget, cap) {
            Ok(false) => return (answer, terminal),
            // Anything after the end, or a line that isn't one.
            Ok(true) if terminal.is_some() => break,
            Ok(true) => match answer.take(&buf) {
                Ok(Said::More) => {}
                Ok(said) => {
                    flags.ended.store(true, Ordering::SeqCst);
                    terminal = Some(said);
                }
                Err(_) => break,
            },
            Err(_) => break,
        }
    }
    flags.bad.store(true, Ordering::SeqCst);
    (answer, terminal)
}

/// A refusal, a change or a failure: the only things a helper says instead of an answer, and only as the first line (`first`). Its code is one of the known ones, never its own words.
pub(crate) fn instead(first: bool, line: &serde_json::Map<String, Value>) -> Result<Said, &'static str> {
    if !first {
        return Err("bad_answer");
    }
    match line.get("state").and_then(Value::as_str) {
        Some("refused") => {
            let code = line.get("code").and_then(Value::as_str).unwrap_or("");
            let known = ["needs_password", "couldnt_read", "no_pages"].into_iter().find(|known| *known == code);
            Ok(Said::Refused(known.unwrap_or("couldnt_read").to_string()))
        }
        Some("changed") => Ok(Said::Changed),
        // What it failed at is its own word for it, never repeated.
        Some("failed") => Ok(Said::Failed("helper_stopped".into())),
        _ => Err("bad_answer"),
    }
}

/// The reader over bytes already in hand: for the fuzz target and the tests. The answer, how it ended if it did, and whether a line was refused.
pub(crate) fn read_bytes<A: Answer>(said: &[u8], answer: A) -> (A, Option<Said>, bool) {
    let flags = Flags::default();
    let (hello, _heard) = sync_channel(1);
    let (answer, terminal) = read_lines(said, answer, hello, &flags);
    (answer, terminal, flags.bad.load(Ordering::SeqCst))
}

/// What the watch saw.
struct Watched {
    peak: u64,
    stopped_for: Option<&'static str>,
}

/// Waits for the helper to end, looking at it every [`WATCH_EVERY`]: past its memory, past its time, no longer wanted, a line it won't have, or past its grace after its end.
fn watch(child: &mut Helper, pid: u32, deadline: Instant, stop: &AtomicBool, flags: &Flags) -> Watched {
    let mut watched = Watched { peak: 0, stopped_for: None };
    let mut ended_at = None;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return watched,
            Ok(None) => {}
            // `try_wait` failed: the helper isn't this process's to signal any more, and `Helper::kill` knows it.
            Err(_) => {
                watched.stopped_for = Some("helper_stopped");
                return watched;
            }
        }
        let footprint = recto_platform::isolate::process_memory(pid).map_or(0, |memory| memory.footprint_bytes);
        watched.peak = watched.peak.max(footprint);
        if flags.ended.load(Ordering::SeqCst) {
            ended_at.get_or_insert_with(Instant::now);
        }
        watched.stopped_for = if flags.bad.load(Ordering::SeqCst) {
            Some("bad_answer")
        } else if footprint > MEMORY_LIMIT {
            Some("over_memory")
        } else if Instant::now() > deadline {
            Some("out_of_time")
        } else if stop.load(Ordering::SeqCst) {
            Some("cancelled")
        } else {
            None
        };
        // A helper that has said its end and won't go is killed, and what it said stands.
        if watched.stopped_for.is_some() || ended_at.is_some_and(|at: Instant| at.elapsed() > END_GRACE) {
            return watched;
        }
        pace();
    }
}

/// The one wait between looks at a helper.
fn pace() {
    std::thread::sleep(WATCH_EVERY);
}

/// Kills a helper that's still ours, and collects it.
fn put_down(child: &mut Helper) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Waits for the hello. `Ok` is what it said of itself; `Err` is a code, with the helper already put down.
fn await_hello(child: &mut Helper, hello: &std::sync::mpsc::Receiver<Option<bool>>, started: Instant, stop: &AtomicBool) -> Result<Option<bool>, &'static str> {
    loop {
        // The channel first, then the child: a helper that said hello and was then collected would be a pid that's no longer ours.
        match hello.recv_timeout(WATCH_EVERY) {
            Ok(walled) => return Ok(walled),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => pace(),
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(why_it_ended(status.code(), wall::in_a_sandbox()));
        }
        let code = if stop.load(Ordering::SeqCst) {
            "cancelled"
        } else if started.elapsed() > HELLO_WAIT {
            "helper_stopped"
        } else {
            continue;
        };
        put_down(child);
        return Err(code);
    }
}

/// One job on a helper that's been started: waits for its hello, **checks it from outside before anything is written to it**, gives it `job`, then watches it and reads what it says
/// until it ends. `check` is told the helper's process id and what it said of its walls.
pub(crate) fn talk<A: Answer>(
    spawned: Spawned,
    job: &Value,
    answer: A,
    deadline: Instant,
    stop: &AtomicBool,
    check: impl FnOnce(u32, Option<bool>) -> Result<(), WallsDown>,
) -> (Ended<A>, Value) {
    let Spawned { mut child, mut stdin, stdout } = spawned;
    let (started, pid, flags) = (Instant::now(), child.id(), Arc::new(Flags::default()));
    let (hello, heard) = sync_channel(1);
    let reading = {
        let flags = Arc::clone(&flags);
        std::thread::Builder::new()
            .name(READER_THREAD.into())
            .stack_size(READER_STACK)
            .spawn(move || read_lines(std::io::BufReader::new(stdout), answer, hello, &flags))
    };
    let Ok(reading) = reading else {
        put_down(&mut child);
        return (Ended::Failed("helper_stopped".into()), Value::Null);
    };
    let admitted = await_hello(&mut child, &heard, started, stop).and_then(|walled| check(pid, walled).map_err(WallsDown::code));
    let numbers = |peak: u64, walled: bool| json!({ "elapsed_ms": started.elapsed().as_millis() as u64, "helper_peak_watched_bytes": peak, "walled": walled.then_some(true) });
    let watched = match admitted {
        Err(code) => {
            put_down(&mut child);
            drop(stdin);
            let _ = reading.join();
            return (Ended::Failed(code.to_string()), numbers(0, false));
        }
        // Only now: the job goes to a helper the kernel has shown walled.
        Ok(_) if writeln!(stdin, "{job}").and_then(|()| stdin.flush()).is_err() => {
            put_down(&mut child);
            drop(stdin);
            let _ = reading.join();
            return (Ended::Failed("helper_stopped".into()), numbers(0, true));
        }
        Ok(_) => watch(&mut child, pid, deadline, stop, &flags),
    };
    // A helper that ended is collected, so this signals nothing; one that is still there is past a limit, wasn't wanted, or won't go after its end.
    let _ = child.kill();
    drop(stdin);
    let _ = child.wait();
    let mut numbers = numbers(watched.peak, true);
    let Ok((answer, said)) = reading.join() else {
        return (Ended::Failed("helper_stopped".into()), numbers);
    };
    // The answer's numbers go first and the session's own over them: nothing the helper says replaces what was measured here.
    if let (Some(own), Some(more)) = (numbers.as_object().cloned(), answer.numbers().as_object()) {
        let mut all = more.clone();
        all.extend(own);
        numbers = Value::Object(all);
    }
    // The answer's own reason first (what its `Err` would otherwise be reported as `bad_answer`), then a limit's code, which wins over a helper that stopped, and a line it wouldn't have over what it said before.
    let code = answer
        .stopped_for()
        .or(watched.stopped_for)
        .or_else(|| flags.bad.load(Ordering::SeqCst).then_some("bad_answer"));
    let ended = match (code, said) {
        (Some(code), _) => Ended::Failed(code.to_string()),
        (None, Some(Said::End)) => Ended::Answered(answer),
        (None, Some(Said::Refused(code))) => Ended::Refused(code),
        (None, Some(Said::Changed)) => Ended::Changed,
        (None, Some(Said::Failed(code))) => Ended::Failed(code),
        (None, _) => Ended::Failed("helper_stopped".into()),
    };
    (ended, numbers)
}

#[cfg(test)]
mod tests;
