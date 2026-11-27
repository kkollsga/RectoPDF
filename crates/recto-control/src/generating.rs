//! Makes generated contents on demand, one document at a time, whatever's open. **What starts it is someone asking:** `recto contents --generated`, `recto wait contents-generated`, or the reader tapping the sidebar's offer. Never opening a tab or switching to one (SPEC decision 22's rule).
//!
//! The work is in a helper process, `recto-headings`, laid out as `RectoStructure.app` and run **only walled** ([`crate::structure`]): given the document as a descriptor, checked from outside before it sees its job, killed past [`MEMORY_LIMIT`], past its time, or when it's no longer wanted. A hang, a panic or a blow-up there is a failed job here, never anything worse. When the walled helper can't start, the answer says so and there is no unwalled one to fall back to. What it says (a line a page, as [`crate::structure::headings`] reads it) is believed only as far as it's checked, **then the check for hidden text runs here** ([`ink`]), with a render worker, and only then is the result written to disk, by this process. The one thread here exists only while there's something to make.

use recto_agent::Event;
use recto_agent::generated::{GenState, Generating};
use recto_index::corpus::{self, CorpusKey};
use recto_index::generated::visible::keep_visible;
use recto_index::generated::{self, Generated, MAX_ENTRIES};
use recto_platform::helper::Priority;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::indexing::Notifier;
use crate::structure::headings::{self, HeadingsAnswer};
use crate::structure::{Ended, Structure};

mod ink;

pub use crate::structure::{MEMORY_LIMIT, time_limit};

/// Documents waiting their turn. Past this the oldest ask gives way: whoever still wants them asks again.
pub const MAX_WAITING: usize = 8;
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
    running: Option<(CorpusKey, Arc<AtomicBool>, PathBuf)>,
    failures: VecDeque<(CorpusKey, String, Instant)>,
    refused: VecDeque<(CorpusKey, String)>,
    held: VecDeque<(CorpusKey, Arc<Generated>)>,
    working: bool,
    stopped: bool,
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
        Self::with_runner(root, Box::new(Walled::new(crate::helpers::HELPER_PRIORITY)))
    }

    pub fn with_runner(root: &Path, runner: Box<dyn Runner>) -> Self {
        Self(Arc::new(Shared {
            root: root.to_path_buf(),
            runner,
            inner: Mutex::new(Inner::default()),
            notify: OnceLock::new(),
        }))
    }

    pub fn attach(&self, notify: Notifier) {
        let _ = self.0.notify.set(notify);
    }

    /// Stops what's running and forgets what's waiting.
    pub fn shut_down(&self) {
        let mut inner = self.0.lock();
        inner.stopped = true;
        inner.waiting.clear();
        if let Some((_, stop, _)) = &inner.running {
            stop.store(true, Ordering::SeqCst);
        }
    }

    fn start_working(&self, inner: &mut Inner) {
        if inner.working {
            return;
        }
        let shared = Arc::clone(&self.0);
        let spawned = std::thread::Builder::new()
            .name("recto-headings".into())
            .stack_size(256 * 1024)
            .spawn(move || shared.work());
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
                inner.running = Some((job.key, Arc::clone(&stop), job.path.clone()));
                (job, stop)
            };
            self.tell(Event::ContentsGenerating {
                path: job.path.clone(),
                file: job.key.short(),
            });
            let (outcome, report) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.runner.run(&job, &stop)))
                .unwrap_or_else(|_| (Outcome::Failed("helper_stopped".into()), Value::Null));
            recto_platform::slow::pause(recto_platform::slow::Seam::Headings);
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
                    // Past a limit is this version's own doing, so it isn't run again for it, as a refusal isn't: a hostile file could otherwise take a gigabyte every minute.
                    Outcome::Failed(code) if code == "over_memory" || code == "out_of_time" => {
                        remember(&mut inner.refused, (job.key, code.clone()));
                        "failed"
                    }
                    Outcome::Failed(code) => {
                        inner.failures.retain(|(key, ..)| *key != job.key);
                        remember(&mut inner.failures, (job.key, code.clone(), Instant::now()));
                        "failed"
                    }
                }
            };
            // What it took, for whoever's watching: numbers only.
            self.tell(Event::ContentsGenerated {
                path: job.path.clone(),
                file: job.key.short(),
                state,
                code: outcome.code().map(str::to_string),
                elapsed_ms: report["elapsed_ms"].clone(),
                helper_peak_bytes: report["helper_peak_watched_bytes"].clone(),
                walled: report["walled"].clone(),
            });
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
        if inner.stopped
            || inner.running.as_ref().is_some_and(|(running, ..)| *running == key)
            || inner.refused.iter().any(|(refused, _)| *refused == key)
            || failure(&mut inner, key).is_some()
        {
            return;
        }
        inner.waiting.retain(|waiting| waiting.key != key);
        inner.waiting.push_front(GenJob {
            path: path.to_path_buf(),
            key,
            pages,
        });
        inner.waiting.truncate(MAX_WAITING);
        self.start_working(&mut inner);
    }

    fn state(&self, key: CorpusKey) -> GenState {
        let mut inner = self.0.lock();
        if inner.running.as_ref().is_some_and(|(running, ..)| *running == key) {
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
        if let Some((running, stop, _)) = &inner.running
            && key.is_none_or(|key| key == *running)
        {
            stop.store(true, Ordering::SeqCst);
        }
    }

    fn cancel_files(&self, paths: Option<&[PathBuf]>) {
        match paths {
            None => self.cancel(None),
            Some(paths) => paths.iter().for_each(|path| self.cancel_path(path)),
        }
    }

    fn making(&self, paths: Option<&[PathBuf]>) -> bool {
        self.running_for(paths)
    }
}

impl Generator {
    /// Whether a job for one of these files, or any, is running.
    fn running_for(&self, paths: Option<&[PathBuf]>) -> bool {
        self.0
            .lock()
            .running
            .as_ref()
            .is_some_and(|(_, _, running)| paths.is_none_or(|paths| paths.contains(running)))
    }

    /// Stops what's on its way for a file, by its path: a tab closed, or the reader saying stop. Nothing's read for it.
    pub fn cancel_path(&self, path: &Path) {
        let mut inner = self.0.lock();
        inner.waiting.retain(|job| job.path != path);
        if let Some((_, stop, running)) = &inner.running
            && running == path
        {
            stop.store(true, Ordering::SeqCst);
        }
    }
}

fn failure(inner: &mut Inner, key: CorpusKey) -> Option<String> {
    inner.failures.retain(|(_, _, at)| at.elapsed() < FAILURE_PAUSE);
    inner.failures.iter().find(|(failed, ..)| *failed == key).map(|(_, code, _)| code.clone())
}

/// The real thing: the walled structure helper, then the check for hidden text here.
#[derive(Clone, Debug)]
pub struct Walled {
    /// [`crate::helpers::HELPER_PRIORITY`]. Anything else only to measure the work apart from the waiting.
    pub priority: Priority,
    /// A helper to use in place of the installed one: the one laid out in `target/Helpers`, in a test.
    #[cfg(any(test, feature = "test-seams"))]
    exe: Option<PathBuf>,
}

impl Walled {
    pub fn new(priority: Priority) -> Self {
        Self {
            priority,
            #[cfg(any(test, feature = "test-seams"))]
            exe: None,
        }
    }

    /// For a test: the helper at `exe`, at this process's own level.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn at(exe: PathBuf) -> Self {
        Self {
            priority: Priority::Inherited,
            exe: Some(exe),
        }
    }

    fn structure(&self) -> Option<Structure> {
        #[cfg(any(test, feature = "test-seams"))]
        if let Some(exe) = &self.exe {
            return Some(Structure::at(exe.clone(), self.priority));
        }
        Structure::installed(self.priority)
    }
}

impl Runner for Walled {
    fn run(&self, job: &GenJob, stop: &AtomicBool) -> (Outcome, Value) {
        let deadline = Instant::now() + time_limit(job.pages);
        let Some(structure) = self.structure() else {
            return (Outcome::Failed("helper_missing".into()), Value::Null);
        };
        let (ended, mut report) = structure.run(&job.path, job.key, &headings::job(), HeadingsAnswer::new(MAX_ENTRIES), deadline, stop);
        let outcome = match ended {
            Ended::Answered(answer) => check_what_was_found(job, answer, deadline, stop, &mut report),
            Ended::Refused(code) => Outcome::Refused(code),
            Ended::Changed => Outcome::Changed,
            Ended::Failed(code) => Outcome::Failed(code),
        };
        (outcome, report)
    }
}

/// What the helper found, kept only where it can be seen: each page that has headings is drawn by a render worker and its characters with no ink under them are marked, and a heading is
/// kept only if its text is found among those with ink ([`keep_visible`]). The file is looked at again before and after, so what's kept is of the version that was asked for.
fn check_what_was_found(job: &GenJob, answer: HeadingsAnswer, deadline: Instant, stop: &AtomicBool, report: &mut Value) -> Outcome {
    let same = || CorpusKey::of_file(&job.path).is_ok_and(|now| now == job.key);
    if !same() {
        return Outcome::Changed;
    }
    let found = answer.into_headings();
    let (count, started) = (found.found.len(), Instant::now());
    let mut ink = ink::Ink::new(job.path.clone(), deadline, stop);
    let made = keep_visible(found, &mut ink);
    let stats = ink.stats();
    if !report.is_object() {
        *report = json!({});
    }
    if let Some(numbers) = report.as_object_mut() {
        numbers.insert("found".into(), json!(count));
        numbers.insert("kept".into(), json!(made.entries.len()));
        numbers.insert("ink_ms".into(), json!(started.elapsed().as_millis() as u64));
        numbers.insert("worker_peak_bytes".into(), json!(stats.peak_footprint_bytes));
        numbers.insert("worker_spawns".into(), json!(stats.spawns));
        numbers.insert("worker_unavailable".into(), json!(ink.unavailable));
    }
    if let Some(code) = ink.stopped_for {
        return Outcome::Failed(code.into());
    }
    if same() { Outcome::Ready(Box::new(made)) } else { Outcome::Changed }
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
        Generated {
            pages: 3,
            entries: vec![GeneratedEntry {
                depth: 0,
                page: 1,
                place: 10,
                title: "One".into(),
            }],
            ..Generated::default()
        }
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

    /// Failure: a file distillPDF refuses, or a helper killed at its limit, is started again by every ask, or every minute, so a hostile file keeps a gigabyte busy. A limit is that version's own, as a refusal is. And a refusal of one version stands for another.
    #[test]
    fn a_refusal_and_a_failure_are_remembered() {
        struct Says(&'static str);
        impl Runner for Says {
            fn run(&self, _job: &GenJob, _stop: &AtomicBool) -> (Outcome, Value) {
                (
                    if self.0 == "refused" {
                        Outcome::Refused("couldnt_read".into())
                    } else {
                        Outcome::Failed("over_memory".into())
                    },
                    Value::Null,
                )
            }
        }
        for (says, expected) in [
            ("refused", GenState::Refused("couldnt_read".into())),
            ("failed", GenState::Refused("over_memory".into())),
        ] {
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

    /// Failure: closing a tab, or the reader saying stop, leaves its helper running to the end of its time, or leaves a job that hadn't started on the queue.
    #[test]
    fn a_closed_tabs_job_stops() {
        let stand = Arc::new(Stand::default());
        let generator = Generator::with_runner(Path::new("/nowhere"), Box::new(Runs(Arc::clone(&stand))));
        // `a` is running before `b` is asked for. Asking for both at once raced: the newest goes first only if the thread hasn't already taken the other, and the stand holds whatever it took until it's cancelled.
        generator.want(Path::new("/nowhere/a.pdf"), key(1), 10);
        let deadline = Instant::now() + Duration::from_secs(10);
        while generator.state(key(1)) != GenState::Running {
            assert!(Instant::now() < deadline, "the first job never started");
            std::thread::yield_now();
        }
        generator.want(Path::new("/nowhere/b.pdf"), key(2), 10);
        assert_eq!(generator.state(key(2)), GenState::Queued);
        generator.cancel_path(Path::new("/nowhere/b.pdf"));
        assert_eq!(generator.state(key(2)), GenState::Idle, "taken off the queue");
        generator.cancel_path(Path::new("/nowhere/a.pdf"));
        settle(&generator);
        assert!(generator.held(key(1)).is_none(), "a cancelled job's result isn't kept");
        assert_eq!(stand.running.load(Ordering::SeqCst), 0, "the running job ended");
    }

    /// Failure: the time a job gets doesn't grow with the document, so a long one is always killed, or has no ceiling.
    #[test]
    fn the_time_allowed_grows_with_pages_and_has_a_ceiling() {
        assert!(time_limit(383) > time_limit(10));
        assert_eq!(time_limit(u32::MAX), Duration::from_secs(30 * 60));
    }
}
