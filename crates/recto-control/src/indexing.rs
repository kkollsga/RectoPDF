//! Builds search indexes on demand, one at a time, whatever's open. **What triggers it is someone asking,** through `search`, `index` or `wait indexing-done`: never opening a tab or switching to one, so 200 open tabs cost nothing here. A tab holds nothing about its index: where a corpus stands is on disk under the file's key, and this holds only what's waiting and what's running.
//!
//! The work is in a helper process, `recto-indexer`, at background priority. The one thread here exists only while there's something to build. It starts the helper, sleeps until it's done, and takes the next. With nothing to build, there's no thread, no process and nothing to wake up for.

use recto_agent::Event;
use recto_agent::index::{Indexing, JobState};
use recto_index::corpus::{self, BuildLimits, CorpusKey};
use serde_json::{Value, json};
use std::any::Any;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// How the server hears of something that happened outside any command.
pub type Notifier = Arc<dyn Fn(Vec<Event>) + Send + Sync>;

/// Files waiting their turn. Past this the oldest ask gives way, and it costs nothing, because anyone who still wants it asks again.
pub const MAX_WAITING: usize = 16;
/// Failures remembered, so a broken install doesn't start a helper for every search. The oldest gives way.
const MAX_FAILURES: usize = 16;
/// How long a failure stops it being tried again. Long enough that a search loop doesn't start a process each time, and short enough that fixing it is soon noticed.
const FAILURE_PAUSE: Duration = Duration::from_secs(60);
/// The most a helper may say.
const MAX_REPORT_BYTES: u64 = 64 * 1024;

/// One file to index. Held only while it's waiting or being built, at most [`MAX_WAITING`] plus one.
#[derive(Clone, Debug)]
pub struct IndexJob {
    pub path: PathBuf,
    pub key: CorpusKey,
}

/// What building comes down to, so a test can stand in for a process. `started` is given something whose dropping stops the build.
pub trait Runner: Send + Sync {
    fn run(&self, job: &IndexJob, root: &Path, started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value;
}

#[derive(Default)]
struct Inner {
    waiting: VecDeque<IndexJob>,
    running: Option<CorpusKey>,
    /// Dropped to stop what's running.
    stop_running: Option<Box<dyn Any + Send>>,
    /// What's running has been told to stop, so its ending isn't a failure.
    cancelled: bool,
    failures: VecDeque<(CorpusKey, String, Instant)>,
    /// The thread that works through `waiting` is there.
    working: bool,
    stopped: bool,
    /// Nobody's looked at what's past its time in this session yet.
    sweep_due: bool,
}

struct Shared {
    root: PathBuf,
    limits: BuildLimits,
    runner: Box<dyn Runner>,
    inner: Mutex<Inner>,
    notify: OnceLock<Notifier>,
}

/// Whoever holds a session holds one of these. It's only a handle, and the one thread, while there's one, holds the rest. A clone is the same indexer.
#[derive(Clone)]
pub struct Indexer(Arc<Shared>);

impl std::fmt::Debug for Indexer {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Indexer").field("root", &self.0.root).finish_non_exhaustive()
    }
}

impl Indexer {
    pub fn new(root: &Path) -> Self {
        let indexer = Self::with_runner(root, Box::new(HelperProcess { background: true }));
        // Only if there's something to do: an ordinary start stays one thread, with one look at a folder.
        if corpus::old_folder_is_there(root) {
            indexer.start_working(&mut indexer.0.lock());
        }
        indexer
    }

    pub fn with_runner(root: &Path, runner: Box<dyn Runner>) -> Self {
        Self::with_limits(root, runner, BuildLimits::default())
    }

    fn with_limits(root: &Path, runner: Box<dyn Runner>, limits: BuildLimits) -> Self {
        Self(Arc::new(Shared {
            root: root.to_path_buf(),
            limits,
            runner,
            inner: Mutex::new(Inner {
                sweep_due: true,
                ..Inner::default()
            }),
            notify: OnceLock::new(),
        }))
    }

    pub fn attach(&self, notify: Notifier) {
        let _ = self.0.notify.set(notify);
    }

    /// Stops what's running and forgets what's waiting. What's been done so far stays on disk, and the next session's build goes on from there.
    pub fn shut_down(&self) {
        let mut inner = self.0.lock();
        inner.stopped = true;
        inner.waiting.clear();
        inner.stop_running = None;
    }

    /// Asks for a build. Never blocks: the newest ask goes first, since that's whatever someone's looking at now, and past the cap the oldest gives way.
    pub fn ask(&self, job: IndexJob) {
        let mut inner = self.0.lock();
        if inner.stopped || inner.running == Some(job.key) || Shared::failure(&mut inner, job.key).is_some() {
            return;
        }
        inner.waiting.retain(|waiting| waiting.key != job.key);
        inner.waiting.push_front(job);
        inner.waiting.truncate(MAX_WAITING);
        self.start_working(&mut inner);
    }

    /// Starts the thread if it isn't there. Only ever while there's something for it to do.
    fn start_working(&self, inner: &mut Inner) {
        if inner.working {
            return;
        }
        let shared = Arc::clone(&self.0);
        // It does no work of its own but a walk over a folder, so a small stack will do.
        let spawned = std::thread::Builder::new()
            .name("recto-indexer".into())
            .stack_size(256 * 1024)
            .spawn(move || shared.work());
        match spawned {
            Ok(_) => inner.working = true,
            // Asked again next time. A failed search would be worse than a late index.
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

    /// Works through what's waiting, one at a time, and goes when there's nothing left.
    fn work(self: Arc<Self>) {
        loop {
            // First, before anything's built, and on this thread because it walks a folder of any size.
            if std::mem::take(&mut self.lock().sweep_due) {
                corpus::remove_old_folder(&self.root);
                let _ = corpus::evict(&self.root, &self.limits, None);
            }
            let job = {
                let mut inner = self.lock();
                let next = if inner.stopped { None } else { inner.waiting.pop_front() };
                let Some(job) = next else {
                    inner.working = false;
                    return;
                };
                inner.running = Some(job.key);
                job
            };
            self.tell(Event::new("indexing_started", json!({ "corpus": job.key.short() })));
            let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                self.runner.run(&job, &self.root, &mut |stop| {
                    let mut inner = self.lock();
                    // Stopped while this was starting: not keeping it stops it now.
                    if !inner.stopped && !inner.cancelled {
                        inner.stop_running = Some(stop);
                    }
                })
            }))
            .unwrap_or_else(|_| json!({ "state": "failed", "code": "indexer_panicked" }));
            recto_platform::slow::pause(recto_platform::slow::Seam::Index);
            let mut state = report["state"].as_str().unwrap_or("failed").to_string();
            {
                let mut inner = self.lock();
                inner.running = None;
                inner.stop_running = None;
                if std::mem::take(&mut inner.cancelled) {
                    state = "cancelled".to_string();
                } else if state == "failed" {
                    if inner.failures.len() >= MAX_FAILURES {
                        inner.failures.pop_front();
                    }
                    inner.failures.retain(|(key, ..)| *key != job.key);
                    inner
                        .failures
                        .push_back((job.key, report["code"].as_str().unwrap_or("unknown").to_string(), Instant::now()));
                }
            }
            // However it ended. A helper that stopped before it got as far as a build, cancelled or finding its file changed, made nothing room for.
            let _ = corpus::evict(&self.root, &self.limits, Some(job.key));
            self.tell(Event::new(
                "indexing_finished",
                json!({ "corpus": job.key.short(), "state": state, "code": report["code"], "pages": report["pages"] }),
            ));
        }
    }

    fn failure(inner: &mut Inner, key: CorpusKey) -> Option<String> {
        inner.failures.retain(|(_, _, at)| at.elapsed() < FAILURE_PAUSE);
        inner.failures.iter().find(|(failed, ..)| *failed == key).map(|(_, code, _)| code.clone())
    }
}

impl Indexing for Indexer {
    fn root(&self) -> &Path {
        &self.0.root
    }

    fn want(&self, path: &Path, key: CorpusKey) {
        self.ask(IndexJob { path: path.to_path_buf(), key });
    }

    fn job(&self, key: CorpusKey) -> JobState {
        let mut inner = self.0.lock();
        if inner.running == Some(key) {
            JobState::Building
        } else if inner.waiting.iter().any(|job| job.key == key) {
            JobState::Queued
        } else if let Some(code) = Shared::failure(&mut inner, key) {
            JobState::Failed(code)
        } else {
            JobState::Idle
        }
    }

    fn load(&self) -> (u32, u32) {
        let inner = self.0.lock();
        (u32::from(inner.running.is_some()), inner.waiting.len() as u32)
    }

    fn cancel(&self, key: Option<CorpusKey>) {
        let mut inner = self.0.lock();
        match key {
            Some(key) => inner.waiting.retain(|job| job.key != key),
            None => inner.waiting.clear(),
        }
        if inner.running.is_some() && (key.is_none() || inner.running == key) {
            inner.cancelled = true;
            inner.stop_running = None;
        }
    }

    fn note_asked(&self) {
        let mut inner = self.0.lock();
        if inner.sweep_due && !inner.stopped {
            self.start_working(&mut inner);
        }
    }
}

/// Finds `recto-indexer` beside this program, through any link to it: [`crate::program_beside`].
fn helper_exe() -> Option<PathBuf> {
    crate::program_beside(&std::env::current_exe().ok()?, "recto-indexer")
}

/// The real thing.
#[derive(Clone, Copy, Debug)]
pub struct HelperProcess {
    /// At background priority. Off only to measure the work apart from the waiting.
    pub background: bool,
}

impl Runner for HelperProcess {
    fn run(&self, job: &IndexJob, root: &Path, started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value {
        let failed = |code: &str| json!({ "state": "failed", "code": code });
        let Some(exe) = helper_exe() else { return failed("indexer_missing") };
        let (Some(file), Some(root)) = (job.path.to_str(), root.to_str()) else {
            return failed("path_not_text");
        };
        let mut command = Command::new(exe);
        // It inherits nothing it doesn't need. These two only ever mean anything in a development build, as the worker's own rules say.
        command.env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for name in ["RECTO_RENDER_SERVICE", "RECTO_PDFIUM_LIB"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let Ok(mut child) = command.spawn() else {
            return failed("indexer_wouldnt_start");
        };
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return failed("indexer_wouldnt_start");
        };
        // On stdin, so a file's name stays out of `ps`.
        let line = json!({ "file": file, "root": root, "key": job.key.hex(), "background": self.background }).to_string() + "\n";
        if stdin.write_all(line.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return failed("indexer_wouldnt_start");
        }
        // Closing this is what stops it. And it stays open for as long as this process lives, so a helper can't outlive it.
        started(Box::new(stdin));
        let mut said = String::new();
        let _ = stdout.take(MAX_REPORT_BYTES).read_to_string(&mut said);
        let _ = child.wait();
        // It said nothing: it was killed, or it crashed. That counts as failing, and gets the pause, or a `wait` would start it again as fast as it could end.
        serde_json::from_str(said.trim()).unwrap_or_else(|_| failed("indexer_stopped"))
    }
}

/// Runs one build here and now, blocking until it's over, for `budget index`.
pub fn build_now(path: &Path, key: CorpusKey, root: &Path, background: bool) -> Value {
    let mut keep = None;
    HelperProcess { background }.run(&IndexJob { path: path.to_path_buf(), key }, root, &mut |stop| keep = Some(stop))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Stands in for the helper: it holds each build until told, and notes how many ran at once.
    #[derive(Default)]
    struct Stand {
        gate: Mutex<u32>,
        moved: Condvar,
        running: AtomicU32,
        most_at_once: AtomicU32,
        finished: AtomicU32,
    }

    struct Runs(Arc<Stand>);

    impl Runner for Runs {
        fn run(&self, _job: &IndexJob, _root: &Path, _started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value {
            let stand = &self.0;
            let now = stand.running.fetch_add(1, Ordering::SeqCst) + 1;
            stand.most_at_once.fetch_max(now, Ordering::SeqCst);
            let mut gate = stand.gate.lock().unwrap();
            while *gate == 0 {
                gate = stand.moved.wait(gate).unwrap();
            }
            *gate -= 1;
            stand.running.fetch_sub(1, Ordering::SeqCst);
            stand.finished.fetch_add(1, Ordering::SeqCst);
            json!({ "state": "ready" })
        }
    }

    fn key(number: u8) -> CorpusKey {
        CorpusKey::from_hex(&format!("{number:02x}").repeat(32)).unwrap()
    }

    fn job(number: u8) -> IndexJob {
        IndexJob {
            path: PathBuf::from(format!("/nowhere/{number}.pdf")),
            key: key(number),
        }
    }

    /// Failure: 200 open tabs that all get searched start 200 indexers, each with a worker that can take a gigabyte. One runs, whoever asks and however fast. Asking never blocks, and what's waiting is capped, with the newest ask first.
    #[test]
    fn one_runs_at_a_time_however_many_ask() {
        let stand = Arc::new(Stand::default());
        let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Runs(Arc::clone(&stand))));
        let started = Instant::now();
        for number in 0..200u8 {
            indexer.ask(job(number));
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "asking must not wait for building: {:?}",
            started.elapsed()
        );
        let (running, waiting) = indexer.load();
        assert!(running <= 1);
        assert!(waiting as usize <= MAX_WAITING, "{waiting}");
        assert_ne!(indexer.job(key(199)), JobState::Idle, "the newest ask is kept");
        let known = (0..200u8).filter(|number| indexer.job(key(*number)) != JobState::Idle).count();
        assert!(known <= MAX_WAITING + 1, "{known} held, against a cap of {MAX_WAITING} waiting and one running");
        *stand.gate.lock().unwrap() = 1000;
        stand.moved.notify_all();
        let deadline = Instant::now() + Duration::from_secs(10);
        while indexer.load() != (0, 0) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(indexer.load(), (0, 0));
        assert_eq!(stand.most_at_once.load(Ordering::SeqCst), 1, "more than one ran at once");
        let deadline = Instant::now() + Duration::from_secs(5);
        while indexer.0.lock().working && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(!indexer.0.lock().working, "its thread stays after there's nothing to do");
    }

    struct Fails;

    impl Runner for Fails {
        fn run(&self, _job: &IndexJob, _root: &Path, _started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value {
            json!({ "state": "failed", "code": "render_worker_missing" })
        }
    }

    /// Failure: on an incomplete install every search starts a process to fail again, and `wait` waits out its time on something that can't happen, or a panic in a build takes the session's thread and stays `building` for ever.
    #[test]
    fn a_failure_is_said_and_not_tried_again_at_once() {
        let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Fails));
        indexer.ask(job(1));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(indexer.job(key(1)), JobState::Failed(_)) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(indexer.job(key(1)), JobState::Failed("render_worker_missing".to_string()));
        indexer.ask(job(1));
        assert_eq!(indexer.load(), (0, 0), "not tried again during the pause");

        struct Panics;
        impl Runner for Panics {
            fn run(&self, _job: &IndexJob, _root: &Path, _started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value {
                panic!("a bug");
            }
        }
        let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Panics));
        indexer.ask(job(2));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(indexer.job(key(2)), JobState::Failed(_)) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(matches!(indexer.job(key(2)), JobState::Failed(_)));
    }

    /// Failure: a build paused because the Mac was too busy counts as failing, so `wait` ends with the index "couldn't be built" and nobody can ask again for a minute. It's taken up as soon as it's asked for.
    #[test]
    fn a_paused_build_isnt_a_failure_and_goes_on_when_asked() {
        struct Paused(Arc<AtomicU32>);
        impl Runner for Paused {
            fn run(&self, _job: &IndexJob, _root: &Path, _started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value {
                self.0.fetch_add(1, Ordering::SeqCst);
                json!({ "state": "paused", "code": "too_busy" })
            }
        }
        let runs = Arc::new(AtomicU32::new(0));
        let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Paused(Arc::clone(&runs))));
        for expected in 1..=2 {
            indexer.ask(job(1));
            let deadline = Instant::now() + Duration::from_secs(30);
            while runs.load(Ordering::SeqCst) < expected || indexer.load() != (0, 0) {
                assert!(Instant::now() < deadline, "it wasn't taken up");
                std::thread::yield_now();
            }
            assert_eq!(indexer.job(key(1)), JobState::Idle, "a pause isn't a failure");
        }
    }

    /// Failure: a cancel while a build is just starting comes before it can be stopped, so it runs on anyway. Or its ending counts as failing, and the next ask is turned away for a minute. And a cancel of one file takes another off the queue.
    #[test]
    fn a_cancel_stops_only_what_it_names_and_isnt_a_failure() {
        let stand = Arc::new(Stand::default());
        let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Runs(Arc::clone(&stand))));
        indexer.ask(job(1));
        indexer.ask(job(2));
        indexer.ask(job(3));
        let deadline = Instant::now() + Duration::from_secs(10);
        while indexer.load().0 == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        indexer.cancel(Some(key(2)));
        assert_ne!(indexer.job(key(2)), JobState::Queued);
        // Whichever of them it reached first. With the helper thread late to start, on a busy Mac, the one running can be the one cancelled, and it's still counted until it's stopped, so what's asked is about the others.
        for other in [1, 3] {
            assert_ne!(indexer.job(key(other)), JobState::Idle, "{other} is still there");
        }
        indexer.cancel(None);
        assert_eq!(indexer.load().1, 0);
        *stand.gate.lock().unwrap() = 1000;
        stand.moved.notify_all();
        let deadline = Instant::now() + Duration::from_secs(10);
        while indexer.load().0 > 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }
        for number in 1..=3 {
            assert_eq!(indexer.job(key(number)), JobState::Idle, "{number}");
        }
        indexer.ask(job(1));
        assert!(
            indexer.load().0 + indexer.load().1 >= 1 || stand.finished.load(Ordering::SeqCst) >= 2,
            "turned away, as if it had failed"
        );
    }

    /// A corpus of one page, with nothing to read but what's given.
    struct OnePage;

    impl corpus::PageFeed for OnePage {
        fn page_count(&mut self) -> Result<u32, corpus::FeedError> {
            Ok(1)
        }
        fn read(&mut self, _page: u32) -> Result<corpus::FedPage, corpus::FeedError> {
            let chars = "A page of text".chars().map(|c| recto_proto::CharRecord::new(c as u32, 0)).collect();
            Ok(corpus::FedPage {
                chars,
                boxes: Vec::new(),
                hidden_checked: true,
            })
        }
        fn still_the_same_file(&mut self) -> bool {
            true
        }
    }

    /// Failure: the owner's rule of 90 days and a maximum size only holds when a build succeeds, so a session that only reads what's there, or whose builds all fail, keeps text past both for good. The first ask of a session must apply each, with nothing being built.
    #[test]
    fn the_first_ask_of_a_session_applies_both_limits() {
        let temp = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-limits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        recto_platform::local::ensure_private_dir(&temp).unwrap();
        let (root, generous) = (temp.join("corpus.noindex"), BuildLimits::default());
        let build = |key: CorpusKey| corpus::build(&root, key, &mut OnePage, &corpus::DocMeta::default(), &generous).unwrap();
        let (old, recent) = (key(1), key(2));
        build(old);
        build(recent);
        let age = |key: CorpusKey, days: u64| {
            let used = corpus::CorpusPaths::new(&root, key).app.join("used");
            std::fs::File::options()
                .write(true)
                .open(used)
                .unwrap()
                .set_modified(std::time::SystemTime::now() - Duration::from_secs(days * 24 * 3600))
                .unwrap();
        };
        let wait_until_gone = |key: CorpusKey| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while root.join(key.hex()).exists() {
                assert!(Instant::now() < deadline, "{key:?} is still there");
                std::thread::yield_now();
            }
        };

        // Past 90 days, under the size.
        age(old, 91);
        age(recent, 1);
        let by_age = Indexer::with_limits(&root, Box::new(Fails), generous);
        by_age.note_asked();
        wait_until_gone(old);
        assert!(root.join(recent.hex()).exists(), "what's within its time stays");
        // 89 days is within it.
        build(old);
        age(old, 89);
        let within = Indexer::with_limits(&root, Box::new(Fails), generous);
        within.note_asked();
        let deadline = Instant::now() + Duration::from_secs(10);
        while within.0.lock().working && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(root.join(old.hex()).exists());

        // Past the size, within 90 days: the least recently used goes.
        let tight = BuildLimits {
            max_total_bytes: corpus::folder_bytes(&root.join(recent.hex())) + 1,
            ..BuildLimits::default()
        };
        let by_size = Indexer::with_limits(&root, Box::new(Fails), tight);
        by_size.note_asked();
        wait_until_gone(old);
        assert!(root.join(recent.hex()).exists(), "the most recently used goes last");
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Failure: a run that ends without a build, being cancelled or finding its file changed, doesn't look at the limits, so the total stays over them, though the rule is "however it ended".
    #[test]
    fn a_run_that_ends_any_way_makes_room() {
        struct Changed;
        impl Runner for Changed {
            fn run(&self, _job: &IndexJob, _root: &Path, _started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value {
                json!({ "state": "changed" })
            }
        }
        let temp = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-any-way-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        recto_platform::local::ensure_private_dir(&temp).unwrap();
        let root = temp.join("corpus.noindex");
        corpus::build(&root, key(1), &mut OnePage, &corpus::DocMeta::default(), &BuildLimits::default()).unwrap();
        let indexer = Indexer::with_limits(&root, Box::new(Changed), BuildLimits::default());
        // Past the first sweep, with room to spare, so it's the run's own look that matters.
        indexer.ask(job(2));
        let deadline = Instant::now() + Duration::from_secs(10);
        while indexer.0.lock().working && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(root.join(key(1).hex()).exists());
        let tight = Indexer::with_limits(
            &root,
            Box::new(Changed),
            BuildLimits {
                max_total_bytes: 1,
                ..BuildLimits::default()
            },
        );
        tight.0.lock().sweep_due = false;
        tight.ask(job(2));
        let deadline = Instant::now() + Duration::from_secs(10);
        while root.join(key(1).hex()).exists() {
            assert!(Instant::now() < deadline, "a run that ended with no build left the total over its limit");
            std::thread::yield_now();
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Failure: `stop` finds a build still queued and starts it after the session has gone, or asking afterwards starts one.
    #[test]
    fn nothing_starts_after_shut_down() {
        let stand = Arc::new(Stand::default());
        let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Runs(Arc::clone(&stand))));
        indexer.shut_down();
        indexer.ask(job(1));
        assert_eq!(indexer.load(), (0, 0));
        assert_eq!(stand.running.load(Ordering::SeqCst), 0);
    }
}
