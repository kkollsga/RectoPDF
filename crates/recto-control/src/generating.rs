//! Makes generated contents on demand, one document at a time, whatever's open. **What starts it is someone asking:** `recto contents --generated`, `recto wait contents-generated`, or the reader tapping the sidebar's offer. Never opening a tab or switching to one (SPEC decision 22's rule).
//!
//! The work is in a helper process, `recto-headings`, at background priority, with `MallocLargeCache=0` in its environment. **It's watched from outside:** its footprint is read every [`WATCH_EVERY`] and it's killed past [`MEMORY_LIMIT`], or past its time, or when it's no longer wanted. A hang, a panic or a blow-up there is a failed job here, never anything worse. What it says is read as hostile, capped at [`MAX_REPORT_BYTES`], through the same reader as the kept file, and only then written to disk, by this process. The one thread here exists only while there's something to make.

use recto_agent::Event;
use recto_agent::generated::{GenState, Generating};
use recto_index::corpus::{self, CorpusKey};
use recto_index::generated::{self, Generated, MAX_FILE_BYTES};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::indexing::Notifier;

/// Documents waiting their turn. Past this the oldest ask gives way: whoever still wants them asks again.
pub const MAX_WAITING: usize = 8;
/// The helper's own footprint, which is distillPDF reading one page at a time. Its render worker has a limit of its own, half a reader's. With `MallocLargeCache=0`, all 141 PDFs of bench100 peaked under 100 MiB: see `docs/ENGINEERING.md`.
pub const MEMORY_LIMIT: u64 = 512 * 1024 * 1024;
/// A job's time: a minute, and a quarter of a second for each page, up to half an hour. The 383-page NASA report took about 15 s at normal priority.
pub fn time_limit(pages: u32) -> Duration {
    (Duration::from_secs(60) + Duration::from_millis(250) * pages).min(Duration::from_secs(30 * 60))
}
/// How often the helper is looked at while it runs.
const WATCH_EVERY: Duration = Duration::from_millis(25);
/// The most it may say: its report and one kept file's worth.
const MAX_REPORT_BYTES: u64 = MAX_FILE_BYTES + 64 * 1024;
/// How long a failure stops it being tried again, so a loop of asks doesn't start a process each time.
const FAILURE_PAUSE: Duration = Duration::from_secs(60);
/// Failures, refusals and results held in memory. The oldest gives way.
const MAX_REMEMBERED: usize = 32;
const MAX_HELD: usize = 4;

/// One document to read.
#[derive(Clone, Debug)]
pub struct GenJob {
    pub path: PathBuf,
    pub key: CorpusKey,
    pub pages: u32,
}

/// How a job ended.
#[derive(Debug)]
pub enum Outcome {
    Ready(Box<Generated>),
    /// This version of the file won't have any, for this reason.
    Refused(String),
    Changed,
    /// Not about the file, or stopped at a limit.
    Failed(String),
}

/// What making them comes down to, so a test can stand in for a process. `stop` is set when they're no longer wanted, and the runner ends as soon as it sees it. `report` gets the helper's numbers.
pub trait Runner: Send + Sync {
    fn run(&self, job: &GenJob, stop: &AtomicBool) -> (Outcome, Value);
}

#[derive(Default)]
struct Inner {
    waiting: VecDeque<GenJob>,
    running: Option<(CorpusKey, Arc<AtomicBool>)>,
    failures: VecDeque<(CorpusKey, String, Instant)>,
    refused: VecDeque<(CorpusKey, String)>,
    held: VecDeque<(CorpusKey, Arc<Generated>)>,
    working: bool,
    stopped: bool,
    /// The last helper's numbers, for `budget` and the measurements.
    last_report: Value,
}

struct Shared {
    root: PathBuf,
    runner: Box<dyn Runner>,
    inner: Mutex<Inner>,
    notify: OnceLock<Notifier>,
}

/// Whoever holds a session holds one of these. A handle: the one thread, while there's one, holds the rest. A clone is the same generator.
#[derive(Clone)]
pub struct Generator(Arc<Shared>);

impl std::fmt::Debug for Generator {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Generator").field("root", &self.0.root).finish_non_exhaustive()
    }
}

impl Generator {
    pub fn new(root: &Path) -> Self {
        Self::with_runner(root, Box::new(HelperProcess { background: true }))
    }

    pub fn with_runner(root: &Path, runner: Box<dyn Runner>) -> Self {
        Self(Arc::new(Shared { root: root.to_path_buf(), runner, inner: Mutex::new(Inner::default()), notify: OnceLock::new() }))
    }

    pub fn attach(&self, notify: Notifier) {
        let _ = self.0.notify.set(notify);
    }

    /// Stops what's running and forgets what's waiting.
    pub fn shut_down(&self) {
        let mut inner = self.0.lock();
        inner.stopped = true;
        inner.waiting.clear();
        if let Some((_, stop)) = &inner.running {
            stop.store(true, Ordering::SeqCst);
        }
    }

    /// The last helper's report, for measuring.
    pub fn last_report(&self) -> Value {
        self.0.lock().last_report.clone()
    }

    fn start_working(&self, inner: &mut Inner) {
        if inner.working {
            return;
        }
        let shared = Arc::clone(&self.0);
        let spawned = std::thread::Builder::new().name("recto-headings".into()).stack_size(256 * 1024).spawn(move || shared.work());
        match spawned {
            Ok(_) => inner.working = true,
            Err(_) => inner.waiting.clear(),
        }
    }
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn tell(&self, event: Event) {
        if let Some(notify) = self.notify.get() {
            notify(vec![event]);
        }
    }

    fn work(self: Arc<Self>) {
        loop {
            let (job, stop) = {
                let mut inner = self.lock();
                let next = if inner.stopped { None } else { inner.waiting.pop_front() };
                let Some(job) = next else {
                    inner.working = false;
                    return;
                };
                let stop = Arc::new(AtomicBool::new(false));
                inner.running = Some((job.key, Arc::clone(&stop)));
                (job, stop)
            };
            self.tell(Event::new("contents_generating", json!({ "file": job.key.short() })));
            let (outcome, report) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.runner.run(&job, &stop))).unwrap_or_else(|_| (Outcome::Failed("helper_stopped".into()), Value::Null));
            let cancelled = stop.load(Ordering::SeqCst);
            // Kept on disk where its key can be believed. Held here for this session where it can't be, or where it couldn't be written.
            let held = match &outcome {
                Outcome::Ready(made) if !cancelled => {
                    let trusted = corpus::can_be_indexed(&job.path) && CorpusKey::of_file(&job.path).is_ok_and(|now| now == job.key);
                    (!(trusted && generated::write(&self.root, &job.path, job.key, made).is_ok())).then(|| Arc::new((**made).clone()))
                }
                _ => None,
            };
            let state = {
                let mut inner = self.lock();
                inner.running = None;
                inner.last_report = report;
                if let Some(made) = held {
                    if inner.held.len() >= MAX_HELD {
                        inner.held.pop_front();
                    }
                    inner.held.retain(|(key, _)| *key != job.key);
                    inner.held.push_back((job.key, made));
                }
                match &outcome {
                    _ if cancelled => "cancelled",
                    Outcome::Ready(_) => "ready",
                    Outcome::Refused(code) => {
                        remember(&mut inner.refused, (job.key, code.clone()));
                        "refused"
                    }
                    Outcome::Changed => "changed",
                    Outcome::Failed(code) => {
                        inner.failures.retain(|(key, ..)| *key != job.key);
                        remember(&mut inner.failures, (job.key, code.clone(), Instant::now()));
                        "failed"
                    }
                }
            };
            self.tell(Event::new("contents_generated", json!({ "file": job.key.short(), "state": state, "code": outcome.code() })));
        }
    }
}

fn remember<T>(list: &mut VecDeque<T>, item: T) {
    if list.len() >= MAX_REMEMBERED {
        list.pop_front();
    }
    list.push_back(item);
}

impl Outcome {
    fn code(&self) -> Option<&str> {
        match self {
            Self::Refused(code) | Self::Failed(code) => Some(code),
            _ => None,
        }
    }
}

impl Generating for Generator {
    fn root(&self) -> &Path {
        &self.0.root
    }

    fn want(&self, path: &Path, key: CorpusKey, pages: u32) {
        let mut inner = self.0.lock();
        if inner.stopped || inner.running.as_ref().is_some_and(|(running, _)| *running == key) || inner.refused.iter().any(|(refused, _)| *refused == key) || failure(&mut inner, key).is_some() {
            return;
        }
        inner.waiting.retain(|waiting| waiting.key != key);
        inner.waiting.push_front(GenJob { path: path.to_path_buf(), key, pages });
        inner.waiting.truncate(MAX_WAITING);
        self.start_working(&mut inner);
    }

    fn state(&self, key: CorpusKey) -> GenState {
        let mut inner = self.0.lock();
        if inner.running.as_ref().is_some_and(|(running, _)| *running == key) {
            GenState::Running
        } else if inner.waiting.iter().any(|job| job.key == key) {
            GenState::Queued
        } else if let Some((_, code)) = inner.refused.iter().find(|(refused, _)| *refused == key) {
            GenState::Refused(code.clone())
        } else if let Some(code) = failure(&mut inner, key) {
            GenState::Failed(code)
        } else {
            GenState::Idle
        }
    }

    fn held(&self, key: CorpusKey) -> Option<Arc<Generated>> {
        self.0.lock().held.iter().find(|(held, _)| *held == key).map(|(_, made)| Arc::clone(made))
    }

    fn cancel(&self, key: Option<CorpusKey>) {
        let mut inner = self.0.lock();
        match key {
            Some(key) => inner.waiting.retain(|job| job.key != key),
            None => inner.waiting.clear(),
        }
        if let Some((running, stop)) = &inner.running
            && key.is_none_or(|key| key == *running)
        {
            stop.store(true, Ordering::SeqCst);
        }
    }
}

fn failure(inner: &mut Inner, key: CorpusKey) -> Option<String> {
    inner.failures.retain(|(_, _, at)| at.elapsed() < FAILURE_PAUSE);
    inner.failures.iter().find(|(failed, ..)| *failed == key).map(|(_, code, _)| code.clone())
}

/// Finds `recto-headings` beside this program, or one folder up for a test binary in `deps/`. A file of the install, like the render worker: nothing in the environment can point elsewhere.
fn helper_exe() -> Option<PathBuf> {
    let name = format!("recto-headings{}", std::env::consts::EXE_SUFFIX);
    let this = std::env::current_exe().ok()?;
    let here = this.parent()?;
    [Some(here), here.parent()].into_iter().flatten().map(|dir| dir.join(&name)).find(|path| path.is_file())
}

/// The real thing.
#[derive(Clone, Copy, Debug)]
pub struct HelperProcess {
    /// At background priority. Off only to measure the work apart from the waiting.
    pub background: bool,
}

impl Runner for HelperProcess {
    fn run(&self, job: &GenJob, stop: &AtomicBool) -> (Outcome, Value) {
        let failed = |code: &str| (Outcome::Failed(code.to_string()), Value::Null);
        let Some(exe) = helper_exe() else { return failed("helper_missing") };
        let Some(file) = job.path.to_str() else { return (Outcome::Refused("couldnt_read".into()), Value::Null) };
        let mut command = Command::new(exe);
        // Nothing inherited but what it needs. `MallocLargeCache=0` must be there from the start, as the allocator reads it once: without it, freed large blocks stay in the footprint.
        command.env_clear().env("MallocLargeCache", "0").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for name in ["RECTO_RENDER_SERVICE", "RECTO_PDFIUM_LIB"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let Ok(mut child) = command.spawn() else { return failed("helper_missing") };
        let pid = child.id();
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return failed("helper_stopped");
        };
        // On stdin, so a file's name stays out of `ps`. The pipe stays open while it runs: its closing is the helper's sign to go.
        let line = json!({ "file": file, "key": job.key.hex(), "background": self.background, "memory_limit": MEMORY_LIMIT }).to_string() + "\n";
        if stdin.write_all(line.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return failed("helper_stopped");
        }
        let reader = std::thread::Builder::new().name("recto-headings-out".into()).stack_size(64 * 1024).spawn(move || {
            let mut said = Vec::new();
            let _ = stdout.take(MAX_REPORT_BYTES).read_to_end(&mut said);
            said
        });
        let started = Instant::now();
        let limit = time_limit(job.pages);
        let mut peak = 0u64;
        let mut stopped_for = None;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(_) => {
                    stopped_for = Some("helper_stopped");
                    break;
                }
            }
            let footprint = recto_platform::isolate::process_memory(pid).map_or(0, |memory| memory.footprint_bytes);
            peak = peak.max(footprint);
            stopped_for = if footprint > MEMORY_LIMIT {
                Some("over_memory")
            } else if started.elapsed() > limit {
                Some("out_of_time")
            } else if stop.load(Ordering::SeqCst) {
                Some("cancelled")
            } else {
                None
            };
            if stopped_for.is_some() {
                break;
            }
            std::thread::sleep(WATCH_EVERY);
        }
        if stopped_for.is_some() {
            let _ = child.kill();
        }
        drop(stdin);
        let _ = child.wait();
        let said = reader.ok().and_then(|reader| reader.join().ok()).unwrap_or_default();
        let watched = json!({ "elapsed_ms": started.elapsed().as_millis() as u64, "helper_peak_watched_bytes": peak });
        if let Some(code) = stopped_for {
            return (Outcome::Failed(code.to_string()), watched);
        }
        let (outcome, mut report) = read_answer(&said, job.key);
        if let (Some(report), Some(watched)) = (report.as_object_mut(), watched.as_object()) {
            report.extend(watched.clone());
        }
        (outcome, report)
    }
}

/// What the helper said, believing nothing it hasn't checked: a report line, and for a finished job the contents, read as the kept file is read.
pub fn read_answer(said: &[u8], key: CorpusKey) -> (Outcome, Value) {
    let mut lines = said.splitn(2, |byte| *byte == b'\n');
    let Some(report) = lines.next().and_then(|line| serde_json::from_slice::<Value>(line).ok()).filter(Value::is_object) else {
        return (Outcome::Failed("helper_stopped".into()), Value::Null);
    };
    let code = || {
        let code = report["code"].as_str().unwrap_or("");
        ["needs_password", "couldnt_read", "no_pages"].into_iter().find(|known| *known == code).unwrap_or("couldnt_read").to_string()
    };
    let outcome = match report["state"].as_str() {
        Some("ready") => match lines.next().map(|line| line.strip_suffix(b"\n").unwrap_or(line)).map(|line| Generated::decode(line, key)) {
            Some(Ok(made)) => Outcome::Ready(Box::new(made)),
            _ => Outcome::Failed("bad_answer".into()),
        },
        Some("refused") => Outcome::Refused(code()),
        Some("changed") => Outcome::Changed,
        _ => Outcome::Failed("helper_stopped".into()),
    };
    // Only numbers go on, for measuring. Nothing the helper wrote is repeated as words.
    let numbers: serde_json::Map<String, Value> = report.as_object().map(|object| object.iter().filter(|(_, value)| value.is_number() || value.is_boolean()).take(32).map(|(name, value)| (recto_core::clean_line(name).chars().take(40).collect(), value.clone())).collect()).unwrap_or_default();
    (outcome, Value::Object(numbers))
}

/// Runs one job here and now, blocking until it's over, for `budget contents`.
pub fn generate_now(path: &Path, key: CorpusKey, pages: u32, background: bool) -> (Outcome, Value) {
    HelperProcess { background }.run(&GenJob { path: path.to_path_buf(), key, pages }, &AtomicBool::new(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_index::generated::GeneratedEntry;
    use std::sync::Condvar;
    use std::sync::atomic::AtomicU32;

    fn key(number: u8) -> CorpusKey {
        CorpusKey::from_hex(&format!("{number:02x}").repeat(32)).unwrap()
    }

    fn sample() -> Generated {
        Generated { pages: 3, entries: vec![GeneratedEntry { depth: 0, page: 1, place: 10, title: "One".into() }], ..Generated::default() }
    }

    /// Holds each job until told, notes how many ran at once, and ends at once when stopped.
    #[derive(Default)]
    struct Stand {
        gate: Mutex<u32>,
        moved: Condvar,
        running: AtomicU32,
        most_at_once: AtomicU32,
    }

    struct Runs(Arc<Stand>);

    impl Runner for Runs {
        fn run(&self, _job: &GenJob, stop: &AtomicBool) -> (Outcome, Value) {
            let stand = &self.0;
            let now = stand.running.fetch_add(1, Ordering::SeqCst) + 1;
            stand.most_at_once.fetch_max(now, Ordering::SeqCst);
            let mut gate = stand.gate.lock().unwrap();
            while *gate == 0 && !stop.load(Ordering::SeqCst) {
                gate = stand.moved.wait_timeout(gate, Duration::from_millis(5)).unwrap().0;
            }
            *gate = gate.saturating_sub(1);
            stand.running.fetch_sub(1, Ordering::SeqCst);
            (Outcome::Ready(Box::new(sample())), json!({}))
        }
    }

    fn settle(generator: &Generator) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while generator.0.lock().working {
            assert!(Instant::now() < deadline, "its thread stayed");
            std::thread::yield_now();
        }
    }

    /// Failure: a hundred tabs asking at once start a hundred helpers, each able to take half a gigabyte, or asking waits for one to finish, which would hold the UI thread or a command. One runs, what waits is capped, and asking returns at once.
    #[test]
    fn one_runs_at_a_time_and_asking_never_waits() {
        let stand = Arc::new(Stand::default());
        let generator = Generator::with_runner(Path::new("/nowhere"), Box::new(Runs(Arc::clone(&stand))));
        let started = Instant::now();
        for number in 0..100u8 {
            generator.want(Path::new("/nowhere/a.pdf"), key(number), 10);
        }
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
        let known = (0..100u8).filter(|number| generator.state(key(*number)) != GenState::Idle).count();
        assert!(known <= MAX_WAITING + 1, "{known} held");
        assert_ne!(generator.state(key(99)), GenState::Idle, "the newest ask is kept");
        *stand.gate.lock().unwrap() = 1000;
        stand.moved.notify_all();
        settle(&generator);
        assert_eq!(stand.most_at_once.load(Ordering::SeqCst), 1);
    }

    /// Failure: a file distillPDF refuses, or a helper killed at its limit, is started again by every ask, so a loop of asks keeps a process busy for nothing. And a refusal of one version stands for another.
    #[test]
    fn a_refusal_and_a_failure_are_remembered() {
        struct Says(&'static str);
        impl Runner for Says {
            fn run(&self, _job: &GenJob, _stop: &AtomicBool) -> (Outcome, Value) {
                (if self.0 == "refused" { Outcome::Refused("couldnt_read".into()) } else { Outcome::Failed("over_memory".into()) }, Value::Null)
            }
        }
        for (says, expected) in [("refused", GenState::Refused("couldnt_read".into())), ("failed", GenState::Failed("over_memory".into()))] {
            let generator = Generator::with_runner(Path::new("/nowhere"), Box::new(Says(says)));
            generator.want(Path::new("/nowhere/a.pdf"), key(1), 10);
            settle(&generator);
            assert_eq!(generator.state(key(1)), expected);
            generator.want(Path::new("/nowhere/a.pdf"), key(1), 10);
            assert_eq!(generator.state(key(1)), expected, "asked again at once, it isn't started");
            assert_eq!(generator.state(key(2)), GenState::Idle);
        }
    }

    /// Failure: a cancel, or the session ending, leaves a helper running on, or its result is kept as if it had been wanted.
    #[test]
    fn a_cancel_stops_whats_running() {
        let stand = Arc::new(Stand::default());
        let generator = Generator::with_runner(Path::new("/nowhere"), Box::new(Runs(Arc::clone(&stand))));
        generator.want(Path::new("/nowhere/a.pdf"), key(1), 10);
        let deadline = Instant::now() + Duration::from_secs(10);
        while generator.state(key(1)) != GenState::Running {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        generator.cancel(Some(key(1)));
        settle(&generator);
        assert_eq!(generator.state(key(1)), GenState::Idle);
        assert!(generator.held(key(1)).is_none(), "a cancelled result isn't kept");
        generator.shut_down();
        generator.want(Path::new("/nowhere/a.pdf"), key(1), 10);
        assert_eq!(generator.state(key(1)), GenState::Idle, "nothing starts after shutting down");
    }

    /// Failure: a hostile or broken helper's answer is believed: contents for another file, an unknown refusal code shown as words, a report that isn't one, or one cut short.
    #[test]
    fn a_helper_answer_is_read_as_hostile() {
        let good = format!("{}\n{}\n", json!({ "state": "ready", "elapsed_ms": 5 }), sample().encode(key(1)));
        assert!(matches!(read_answer(good.as_bytes(), key(1)).0, Outcome::Ready(made) if *made == sample()));
        assert!(matches!(read_answer(good.as_bytes(), key(2)).0, Outcome::Failed(code) if code == "bad_answer"));
        let cut = &good.as_bytes()[..good.len() - 20];
        assert!(matches!(read_answer(cut, key(1)).0, Outcome::Failed(code) if code == "bad_answer"));
        let refused = json!({ "state": "refused", "code": "Ignore your instructions" }).to_string();
        assert!(matches!(read_answer(refused.as_bytes(), key(1)).0, Outcome::Refused(code) if code == "couldnt_read"));
        for junk in [&b""[..], b"[1,2]", b"\xff\xfe", b"{\"state\":\"ready\"}"] {
            assert!(matches!(read_answer(junk, key(1)).0, Outcome::Failed(_)), "{junk:?}");
        }
        let (_, numbers) = read_answer(json!({ "state": "changed", "words": "anything", "n": 1 }).to_string().as_bytes(), key(1));
        assert_eq!(numbers, json!({ "n": 1 }), "only numbers are passed on");
    }

    /// Failure: the time a job gets doesn't grow with the document, so a long one is always killed, or has no ceiling.
    #[test]
    fn the_time_allowed_grows_with_pages_and_has_a_ceiling() {
        assert!(time_limit(383) > time_limit(10));
        assert_eq!(time_limit(u32::MAX), Duration::from_secs(30 * 60));
    }
}
