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

use crate::blocking;
use crate::indexing::Notifier;
use crate::structure::headings::{self, HeadingsAnswer};
use crate::structure::{Ended, Pick, numbers_of};
use recto_agent::blocks::BlocksAsk;

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

/// Which job a queue entry is. Generated contents and blocks share the one queue, the one helper, and [`MAX_WAITING`]; what's remembered of a file is remembered for each kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum What {
    Contents,
    Blocks,
}

impl What {
    /// Whether this kind's code is a limit of the version's own doing, which isn't run again for it (a refusal isn't either): a hostile file could otherwise take half a gigabyte every minute.
    fn is_limit(self, code: &str) -> bool {
        match self {
            Self::Contents => CONTENTS_LIMITS.contains(&code),
            Self::Blocks => blocking::is_limit(code),
        }
    }
}

/// The codes of a limit a generated-contents job can end at.
const CONTENTS_LIMITS: &[&str] = &["over_memory", "out_of_time"];

/// Whose ask a job is. A person's waiting ask is never given way for an agent's, and goes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum By {
    Person,
    Agent,
}

/// A job on the queue.
pub(crate) enum Job {
    Contents(GenJob),
    Blocks(BlocksAsk),
}

impl Job {
    pub(crate) fn key(&self) -> CorpusKey {
        match self {
            Self::Contents(job) => job.key,
            Self::Blocks(ask) => ask.key,
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Contents(job) => &job.path,
            Self::Blocks(ask) => &ask.path,
        }
    }

    pub(crate) fn what(&self) -> What {
        match self {
            Self::Contents(_) => What::Contents,
            Self::Blocks(_) => What::Blocks,
        }
    }
}

/// A job waiting its turn, and whose ask it is.
struct Waiting {
    by: By,
    job: Job,
}

/// The job running now: what it is, whose, how to stop it, and its file.
type Running = (What, CorpusKey, Arc<AtomicBool>, PathBuf);

#[derive(Default)]
struct Inner {
    /// The people's asks first, newest first, then the agents', newest first.
    waiting: VecDeque<Waiting>,
    running: Option<Running>,
    failures: VecDeque<(What, CorpusKey, String, Instant)>,
    refused: VecDeque<(What, CorpusKey, String)>,
    held: VecDeque<(CorpusKey, Arc<Generated>)>,
    working: bool,
    stopped: bool,
}

struct Shared {
    root: PathBuf,
    runner: Box<dyn Runner>,
    blocks: Box<dyn blocking::Runner>,
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
        Self::with_runners(root, runner, Box::new(blocking::Walled::new(crate::helpers::HELPER_PRIORITY)))
    }

    pub fn with_runners(root: &Path, contents: Box<dyn Runner>, blocks: Box<dyn blocking::Runner>) -> Self {
        Self(Arc::new(Shared {
            root: root.to_path_buf(),
            runner: contents,
            blocks,
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
        if let Some((_, _, stop, _)) = &inner.running {
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

    /// Someone wants this job done. Nothing happens if it's on its way, refused, or failed within the pause. The newest ask of a kind of asker goes first of theirs; a person's asks go before the agents', and
    /// past [`MAX_WAITING`] the agents' oldest gives way, a person's only when no agent's waits (and an agent's own ask is then the one that isn't kept).
    pub(crate) fn want_job(&self, by: By, job: Job) {
        let mut inner = self.0.lock();
        let (what, key) = (job.what(), job.key());
        if inner.stopped
            || inner
                .running
                .as_ref()
                .is_some_and(|(running, key_running, ..)| (*running, *key_running) == (what, key))
            || inner.refused.iter().any(|(kind, refused, _)| (*kind, *refused) == (what, key))
            || failure(&mut inner, what, key).is_some()
        {
            return;
        }
        // A person's ask for what an agent has waiting for becomes theirs; an agent's for what a person has waiting changes nothing.
        let same = |waiting: &Waiting| (waiting.job.what(), waiting.job.key()) == (what, key);
        if by == By::Agent && inner.waiting.iter().any(|waiting| same(waiting) && waiting.by == By::Person) {
            return;
        }
        inner.waiting.retain(|waiting| !same(waiting));
        let people = inner.waiting.iter().take_while(|waiting| waiting.by == By::Person).count();
        let at = if by == By::Person { 0 } else { people };
        inner.waiting.insert(at, Waiting { by, job });
        if inner.waiting.len() > MAX_WAITING {
            // The agents' oldest is at the back; with none, it's a person's oldest, which is the back too.
            inner.waiting.pop_back();
        }
        self.start_working(&mut inner);
    }

    pub(crate) fn state_of(&self, what: What, key: CorpusKey) -> GenState {
        let mut inner = self.0.lock();
        if inner
            .running
            .as_ref()
            .is_some_and(|(running, key_running, ..)| (*running, *key_running) == (what, key))
        {
            GenState::Running
        } else if inner.waiting.iter().any(|waiting| (waiting.job.what(), waiting.job.key()) == (what, key)) {
            GenState::Queued
        } else if let Some((.., code)) = inner.refused.iter().find(|(kind, refused, _)| (*kind, *refused) == (what, key)) {
            GenState::Refused(code.clone())
        } else if let Some(code) = failure(&mut inner, what, key) {
            GenState::Failed(code)
        } else {
            GenState::Idle
        }
    }

    /// Stops one kind of job for one file, or for all, whatever's waiting and what's running. The other kind isn't touched.
    pub(crate) fn cancel_key(&self, what: What, key: Option<CorpusKey>) {
        let mut inner = self.0.lock();
        inner
            .waiting
            .retain(|waiting| waiting.job.what() != what || key.is_some_and(|key| key != waiting.job.key()));
        if let Some((running, key_running, stop, _)) = &inner.running
            && *running == what
            && key.is_none_or(|key| key == *key_running)
        {
            stop.store(true, Ordering::SeqCst);
        }
    }

    /// Stops one kind of job for these files by path, every version, or for every file.
    pub(crate) fn cancel_paths(&self, what: What, paths: Option<&[PathBuf]>) {
        match paths {
            None => self.cancel_key(what, None),
            Some(paths) => {
                let mut inner = self.0.lock();
                inner
                    .waiting
                    .retain(|waiting| waiting.job.what() != what || !paths.iter().any(|path| path == waiting.job.path()));
                if let Some((running, _, stop, file)) = &inner.running
                    && *running == what
                    && paths.contains(file)
                {
                    stop.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    /// Whether a job of this kind for one of these files, or any, is running.
    pub(crate) fn making_paths(&self, what: What, paths: Option<&[PathBuf]>) -> bool {
        self.0
            .lock()
            .running
            .as_ref()
            .is_some_and(|(running, _, _, file)| *running == what && paths.is_none_or(|paths| paths.contains(file)))
    }

    /// Stops what's on its way for a file, by its path, of every kind: a tab closed, or the reader saying stop. Nothing's read for it.
    pub fn cancel_path(&self, path: &Path) {
        let paths = [path.to_path_buf()];
        for what in [What::Contents, What::Blocks] {
            self.cancel_paths(what, Some(&paths));
        }
    }
}

/// What a finished job comes to, for what's remembered of its file.
pub(crate) enum Verdict<'a> {
    Done,
    Refused(&'a str),
    Changed,
    Failed(&'a str),
}

/// Remembers how a job ended and says the state word for the news of it.
fn settle(inner: &mut Inner, (what, key): (What, CorpusKey), cancelled: bool, verdict: &Verdict) -> &'static str {
    match verdict {
        _ if cancelled => "cancelled",
        Verdict::Done => "ready",
        Verdict::Refused(code) => {
            remember(&mut inner.refused, |kept| kept.0, (what, key, (*code).to_string()));
            "refused"
        }
        Verdict::Changed => "changed",
        Verdict::Failed(code) if what.is_limit(code) => {
            remember(&mut inner.refused, |kept| kept.0, (what, key, (*code).to_string()));
            "failed"
        }
        Verdict::Failed(code) => {
            inner.failures.retain(|(kind, failed, ..)| (*kind, *failed) != (what, key));
            remember(&mut inner.failures, |kept| kept.0, (what, key, (*code).to_string(), Instant::now()));
            "failed"
        }
    }
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn tell(&self, event: Event) {
        if let Some(notify) = self.notify.get() {
            notify(vec![event]);
        }
    }

    fn work(self: Arc<Self>) {
        loop {
            let (job, stop) = {
                let mut inner = self.lock();
                let next = if inner.stopped { None } else { inner.waiting.pop_front() };
                let Some(Waiting { job, .. }) = next else {
                    inner.working = false;
                    return;
                };
                let stop = Arc::new(AtomicBool::new(false));
                inner.running = Some((job.what(), job.key(), Arc::clone(&stop), job.path().to_path_buf()));
                (job, stop)
            };
            match job {
                Job::Contents(job) => self.run_contents(&job, &stop),
                Job::Blocks(ask) => self.run_blocks(&ask, &stop),
            }
        }
    }

    /// One generated-contents job, from the news that it's started to the news of how it ended.
    fn run_contents(&self, job: &GenJob, stop: &AtomicBool) {
        self.tell(Event::ContentsGenerating {
            path: job.path.clone(),
            file: job.key.short(),
        });
        let (outcome, report) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.runner.run(job, stop)))
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
            let verdict = match &outcome {
                Outcome::Ready(_) => Verdict::Done,
                Outcome::Refused(code) => Verdict::Refused(code),
                Outcome::Changed => Verdict::Changed,
                Outcome::Failed(code) => Verdict::Failed(code),
            };
            settle(&mut inner, (What::Contents, job.key), cancelled, &verdict)
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

    /// One blocks job. Nothing of it is kept here, as the store has it on disk, and **nothing is announced for a job that changed nothing** (blocks already made): the runner says when it begins, and its end is told only if it did.
    fn run_blocks(&self, ask: &BlocksAsk, stop: &AtomicBool) {
        let begun = || self.tell(blocking::building(ask));
        let (outcome, report) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.blocks.run(ask, stop, &begun)))
            .unwrap_or_else(|_| (blocking::Outcome::Failed("helper_stopped".into()), Value::Null));
        recto_platform::slow::pause(recto_platform::slow::Seam::Headings);
        let cancelled = stop.load(Ordering::SeqCst);
        let state = {
            let mut inner = self.lock();
            inner.running = None;
            settle(&mut inner, (What::Blocks, ask.key), cancelled, &outcome.verdict())
        };
        if let Some(built) = blocking::built(ask, state, &outcome, &report) {
            self.tell(built);
        }
    }
}

/// Remembers one more of a kind, and past [`MAX_REMEMBERED`] of that kind lets its oldest go: a flood of one kind's refusals can't make the queue forget another's (a hostile file would then be run again, at half a gigabyte each time).
fn remember<T>(list: &mut VecDeque<T>, kind: impl Fn(&T) -> What, item: T) {
    let what = kind(&item);
    if list.iter().filter(|kept| kind(kept) == what).count() >= MAX_REMEMBERED
        && let Some(oldest) = list.iter().position(|kept| kind(kept) == what)
    {
        list.remove(oldest);
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
        self.want_job(
            By::Agent,
            Job::Contents(GenJob {
                path: path.to_path_buf(),
                key,
                pages,
            }),
        );
    }

    fn want_as_person(&self, path: &Path, key: CorpusKey, pages: u32) {
        self.want_job(
            By::Person,
            Job::Contents(GenJob {
                path: path.to_path_buf(),
                key,
                pages,
            }),
        );
    }

    fn state(&self, key: CorpusKey) -> GenState {
        self.state_of(What::Contents, key)
    }

    fn held(&self, key: CorpusKey) -> Option<Arc<Generated>> {
        self.0.lock().held.iter().find(|(held, _)| *held == key).map(|(_, made)| Arc::clone(made))
    }

    fn cancel(&self, key: Option<CorpusKey>) {
        self.cancel_key(What::Contents, key);
    }

    fn cancel_files(&self, paths: Option<&[PathBuf]>) {
        self.cancel_paths(What::Contents, paths);
    }

    fn making(&self, paths: Option<&[PathBuf]>) -> bool {
        self.making_paths(What::Contents, paths)
    }
}

fn failure(inner: &mut Inner, what: What, key: CorpusKey) -> Option<String> {
    inner.failures.retain(|(_, _, _, at)| at.elapsed() < FAILURE_PAUSE);
    inner
        .failures
        .iter()
        .find(|(kind, failed, ..)| (*kind, *failed) == (what, key))
        .map(|(_, _, code, _)| code.clone())
}

/// The real thing: the walled structure helper, then the check for hidden text here.
#[derive(Clone, Debug)]
pub struct Walled(Pick);

impl Walled {
    pub fn new(priority: Priority) -> Self {
        Self(Pick::new(priority))
    }

    /// For a test: the helper at `exe`, at this process's own level.
    #[cfg(any(test, feature = "test-seams"))]
    pub fn at(exe: PathBuf) -> Self {
        Self(Pick::at(exe))
    }
}

impl Runner for Walled {
    fn run(&self, job: &GenJob, stop: &AtomicBool) -> (Outcome, Value) {
        let deadline = Instant::now() + time_limit(job.pages);
        let Some(structure) = self.0.structure() else {
            return (Outcome::Failed("helper_missing".into()), Value::Null);
        };
        let (ended, mut report) = structure.run(
            &job.path,
            job.key,
            &headings::job(),
            HeadingsAnswer::new(MAX_ENTRIES, job.pages),
            deadline,
            stop,
        );
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
    let numbers = numbers_of(report);
    numbers.insert("found".into(), json!(count));
    numbers.insert("kept".into(), json!(made.entries.len()));
    numbers.insert("ink_ms".into(), json!(started.elapsed().as_millis() as u64));
    numbers.insert("worker_peak_bytes".into(), json!(stats.peak_footprint_bytes));
    numbers.insert("worker_spawns".into(), json!(stats.spawns));
    numbers.insert("worker_unavailable".into(), json!(ink.unavailable));
    if let Some(code) = ink.stopped_for {
        return Outcome::Failed(code.into());
    }
    if same() { Outcome::Ready(Box::new(made)) } else { Outcome::Changed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_agent::blocks;
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

    /// Holds each job until told, notes how many ran at once and in what order they started, and ends at once when stopped.
    #[derive(Default)]
    struct Stand {
        gate: Mutex<u32>,
        moved: Condvar,
        running: AtomicU32,
        most_at_once: AtomicU32,
        /// Each job as it started: what it was and the first byte of its key.
        started: Mutex<Vec<(What, u8)>>,
        /// Whether any job was stopped while it ran.
        saw_stop: AtomicBool,
    }

    impl Stand {
        /// What every stand-in does: starts, waits for the gate or a stop, and ends.
        fn hold(&self, what: What, key: CorpusKey, stop: &AtomicBool) {
            self.started.lock().unwrap().push((what, key.as_bytes()[0]));
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most_at_once.fetch_max(now, Ordering::SeqCst);
            let mut gate = self.gate.lock().unwrap();
            while *gate == 0 && !stop.load(Ordering::SeqCst) {
                gate = self.moved.wait_timeout(gate, Duration::from_millis(5)).unwrap().0;
            }
            if stop.load(Ordering::SeqCst) {
                self.saw_stop.store(true, Ordering::SeqCst);
            }
            *gate = gate.saturating_sub(1);
            self.running.fetch_sub(1, Ordering::SeqCst);
        }
    }

    struct Runs(Arc<Stand>);

    impl Runner for Runs {
        fn run(&self, job: &GenJob, stop: &AtomicBool) -> (Outcome, Value) {
            self.0.hold(What::Contents, job.key, stop);
            (Outcome::Ready(Box::new(sample())), json!({}))
        }
    }

    /// The blocks' stand-in, on the same stand.
    struct BlocksRuns(Arc<Stand>);

    impl blocking::Runner for BlocksRuns {
        fn run(&self, job: &BlocksAsk, stop: &AtomicBool, begun: &dyn Fn()) -> (blocking::Outcome, Value) {
            begun();
            self.0.hold(What::Blocks, job.key, stop);
            (blocking::Outcome::AlreadyMade(Default::default()), json!({}))
        }
    }

    fn ask(path: &str, number: u8) -> BlocksAsk {
        BlocksAsk {
            path: path.into(),
            key: key(number),
            corpus_root: "/nowhere".into(),
            grant: recto_index::seal::grant_for(&recto_platform::find_key::FindKey::for_tests(1), key(number)).unwrap(),
        }
    }

    fn both(stand: &Arc<Stand>) -> Generator {
        Generator::with_runners(
            Path::new("/nowhere"),
            Box::new(Runs(Arc::clone(stand))),
            Box::new(BlocksRuns(Arc::clone(stand))),
        )
    }

    /// Waits until `what`'s job for `number` runs.
    fn running(generator: &Generator, what: What, number: u8) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while generator.state_of(what, key(number)) != GenState::Running {
            assert!(Instant::now() < deadline, "{what:?} {number} never started");
            std::thread::yield_now();
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

    /// Failure: generated contents and blocks each start a helper, so two run at once and take twice the memory the one slot allows; or the newest ask isn't first across both kinds (an agent that asks
    /// for blocks waits behind older contents asks); or `MAX_WAITING` counts each kind apart and the queue holds twice what it may.
    #[test]
    fn contents_and_blocks_share_one_helper_and_the_newest_ask_goes_first() {
        let stand = Arc::new(Stand::default());
        let generator = both(&stand);
        generator.want(Path::new("/nowhere/a.pdf"), key(1), 10);
        running(&generator, What::Contents, 1);
        blocks::Blocking::want(&generator, ask("/nowhere/b.pdf", 2));
        generator.want(Path::new("/nowhere/c.pdf"), key(3), 10);
        blocks::Blocking::want(&generator, ask("/nowhere/d.pdf", 4));
        *stand.gate.lock().unwrap() = 1000;
        stand.moved.notify_all();
        settle(&generator);
        assert_eq!(
            *stand.started.lock().unwrap(),
            [(What::Contents, 1), (What::Blocks, 4), (What::Contents, 3), (What::Blocks, 2)],
            "the newest first, across both kinds"
        );
        assert_eq!(stand.most_at_once.load(Ordering::SeqCst), 1);

        let stand = Arc::new(Stand::default());
        let generator = both(&stand);
        for number in 0..40u8 {
            if number % 2 == 0 {
                generator.want(Path::new("/nowhere/a.pdf"), key(number), 10);
            } else {
                blocks::Blocking::want(&generator, ask("/nowhere/b.pdf", number));
            }
        }
        let known = (0..40u8)
            .filter(|number| {
                generator.state_of(What::Contents, key(*number)) != GenState::Idle || generator.state_of(What::Blocks, key(*number)) != GenState::Idle
            })
            .count();
        assert!(known <= MAX_WAITING + 1, "{known} held, for both kinds");
        *stand.gate.lock().unwrap() = 1000;
        stand.moved.notify_all();
        settle(&generator);
    }

    /// Failure: a cancel of one kind takes the other's job too (an agent's `index clear` or access going off stops the sidebar's contents, or the reader's stop takes an agent's blocks), is told of
    /// the other's running (`making` true for contents because blocks run, so a clear waits on the wrong job), or a closed tab leaves either kind's job for its file running or queued.
    #[test]
    fn each_kind_cancels_and_reports_only_itself_and_a_closed_tab_stops_both() {
        let stand = Arc::new(Stand::default());
        let generator = both(&stand);
        let (a, b) = (PathBuf::from("/nowhere/a.pdf"), PathBuf::from("/nowhere/b.pdf"));
        generator.want(&a, key(1), 10);
        running(&generator, What::Contents, 1);
        blocks::Blocking::want(&generator, ask("/nowhere/b.pdf", 2));
        generator.want(&b, key(3), 10);
        assert!(
            Generating::making(&generator, None) && !blocks::Blocking::making(&generator, None),
            "contents run, blocks don't"
        );
        blocks::Blocking::cancel_files(&generator, Some(std::slice::from_ref(&a)));
        blocks::Blocking::cancel_files(&generator, None);
        assert_eq!(
            generator.state_of(What::Contents, key(1)),
            GenState::Running,
            "a blocks cancel stops no contents job"
        );
        assert_eq!(generator.state_of(What::Contents, key(3)), GenState::Queued);
        assert_eq!(generator.state_of(What::Blocks, key(2)), GenState::Idle, "and takes blocks off the queue");
        blocks::Blocking::want(&generator, ask("/nowhere/b.pdf", 2));
        Generating::cancel_files(&generator, Some(std::slice::from_ref(&b)));
        Generating::cancel(&generator, None);
        assert_eq!(
            generator.state_of(What::Blocks, key(2)),
            GenState::Queued,
            "a contents cancel leaves blocks queued"
        );
        assert_eq!(generator.state_of(What::Contents, key(3)), GenState::Idle);
        // A closed tab stops what's running for it, of either kind, and what's queued.
        generator.cancel_path(&a);
        settle_idle(&generator, What::Contents, 1);
        assert!(stand.saw_stop.load(Ordering::SeqCst), "the running job was told to stop");
        generator.cancel_path(&b);
        // The blocks job had moved up to run when the contents one stopped: the closed tab stops it as it stops anything of its file.
        settle_idle(&generator, What::Blocks, 2);
        // A running blocks job, stopped only by a blocks cancel.
        stand.saw_stop.store(false, Ordering::SeqCst);
        blocks::Blocking::want(&generator, ask("/nowhere/e.pdf", 5));
        running(&generator, What::Blocks, 5);
        Generating::cancel(&generator, None);
        Generating::cancel_files(&generator, None);
        assert!(blocks::Blocking::making(&generator, None) && !Generating::making(&generator, None));
        assert!(!stand.saw_stop.load(Ordering::SeqCst), "a contents cancel stopped a blocks job");
        blocks::Blocking::cancel_files(&generator, None);
        settle(&generator);
        assert!(stand.saw_stop.load(Ordering::SeqCst), "a blocks cancel stops it");
    }

    fn settle_idle(generator: &Generator, what: What, number: u8) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while generator.state_of(what, key(number)) != GenState::Idle {
            assert!(Instant::now() < deadline, "{what:?} {number} stayed");
            std::thread::yield_now();
        }
    }

    /// Failure: a blocks job that a file's own nature or a limit ended is run again by every ask (a hostile file takes a gigabyte of the helper each time), or one that failed of the moment isn't paused,
    /// or what's remembered of blocks for a file is taken for what's remembered of its contents.
    #[test]
    fn a_blocks_refusal_or_limit_is_remembered_for_that_version() {
        struct Says(blocking::Outcome, Arc<AtomicU32>);
        impl blocking::Runner for Says {
            fn run(&self, _job: &BlocksAsk, _stop: &AtomicBool, _begun: &dyn Fn()) -> (blocking::Outcome, Value) {
                self.1.fetch_add(1, Ordering::SeqCst);
                let outcome = match &self.0 {
                    blocking::Outcome::Refused(code) => blocking::Outcome::Refused(code.clone()),
                    blocking::Outcome::Failed(code) => blocking::Outcome::Failed(code.clone()),
                    blocking::Outcome::Changed => blocking::Outcome::Changed,
                    _ => blocking::Outcome::AlreadyMade(Default::default()),
                };
                (outcome, Value::Null)
            }
        }
        let table = [
            (
                blocking::Outcome::Refused("needs_password".into()),
                GenState::Refused("needs_password".into()),
                1,
            ),
            (blocking::Outcome::Failed("over_memory".into()), GenState::Refused("over_memory".into()), 1),
            (blocking::Outcome::Failed("out_of_time".into()), GenState::Refused("out_of_time".into()), 1),
            (blocking::Outcome::Failed("too_large".into()), GenState::Refused("too_large".into()), 1),
            (blocking::Outcome::Failed("pages_differ".into()), GenState::Refused("pages_differ".into()), 1),
            (blocking::Outcome::Failed("busy".into()), GenState::Failed("busy".into()), 1),
            (blocking::Outcome::Changed, GenState::Idle, 2),
            (blocking::Outcome::AlreadyMade(Default::default()), GenState::Idle, 2),
        ];
        for (outcome, expected, runs) in table {
            let count = Arc::new(AtomicU32::new(0));
            let generator = Generator::with_runners(
                Path::new("/nowhere"),
                Box::new(Runs(Arc::default())),
                Box::new(Says(outcome, Arc::clone(&count))),
            );
            blocks::Blocking::want(&generator, ask("/nowhere/a.pdf", 1));
            settle(&generator);
            assert_eq!(blocks::Blocking::state(&generator, key(1)), expected);
            blocks::Blocking::want(&generator, ask("/nowhere/a.pdf", 1));
            settle(&generator);
            assert_eq!(count.load(Ordering::SeqCst), runs, "{expected:?}: asked again at once");
            assert_eq!(blocks::Blocking::state(&generator, key(2)), GenState::Idle, "another version");
            assert_eq!(Generating::state(&generator, key(1)), GenState::Idle, "its contents aren't blocks");
        }
    }

    /// Events a generator tells, kept.
    fn listening(generator: &Generator) -> Arc<Mutex<Vec<Event>>> {
        let told = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&told);
        generator.attach(Arc::new(move |events| kept.lock().unwrap().extend(events)));
        told
    }

    /// Failure: blocks that were already made are announced as made (`blocks_built` "ready" with no numbers) and as started: a watcher is told something changed when nothing did. Told only when the runner did the work: its
    /// start, then its end with the numbers; and nothing for blocks it found made.
    #[test]
    fn nothing_is_announced_for_blocks_that_were_already_made() {
        struct Works(bool);
        impl blocking::Runner for Works {
            fn run(&self, _job: &BlocksAsk, _stop: &AtomicBool, begun: &dyn Fn()) -> (blocking::Outcome, Value) {
                if self.0 {
                    begun();
                    (blocking::Outcome::Ready(Default::default()), json!({ "pages": 3 }))
                } else {
                    (blocking::Outcome::AlreadyMade(Default::default()), Value::Null)
                }
            }
        }
        for (works, names) in [(false, vec![]), (true, vec!["blocks_building", "blocks_built"])] {
            let generator = Generator::with_runners(Path::new("/nowhere"), Box::new(Runs(Arc::default())), Box::new(Works(works)));
            let told = listening(&generator);
            blocks::Blocking::want(&generator, ask("/nowhere/a.pdf", 1));
            settle(&generator);
            let told: Vec<&str> = told.lock().unwrap().iter().map(Event::name).collect::<Vec<_>>().into_iter().collect();
            assert_eq!(told, names, "work done: {works}");
        }
    }

    /// Failure: a flood of blocks refusals pushes a contents limit out of what's remembered (one list, 32 of any kind), so a hostile file's contents run again at half a gigabyte each time. Each kind keeps its own 32:
    /// one more than that of blocks refusals forgets only the oldest blocks one.
    #[test]
    fn a_flood_of_blocks_refusals_doesnt_make_contents_forget_its_limit() {
        struct Limit;
        impl Runner for Limit {
            fn run(&self, _job: &GenJob, _stop: &AtomicBool) -> (Outcome, Value) {
                (Outcome::Failed("over_memory".into()), Value::Null)
            }
        }
        struct Refuses;
        impl blocking::Runner for Refuses {
            fn run(&self, _job: &BlocksAsk, _stop: &AtomicBool, _begun: &dyn Fn()) -> (blocking::Outcome, Value) {
                (blocking::Outcome::Refused("couldnt_read".into()), Value::Null)
            }
        }
        let generator = Generator::with_runners(Path::new("/nowhere"), Box::new(Limit), Box::new(Refuses));
        generator.want(Path::new("/nowhere/a.pdf"), key(200), 10);
        settle(&generator);
        assert_eq!(generator.state_of(What::Contents, key(200)), GenState::Refused("over_memory".into()));
        for number in 0..=MAX_REMEMBERED as u8 {
            blocks::Blocking::want(&generator, ask("/nowhere/b.pdf", number));
            settle(&generator);
        }
        assert_eq!(
            generator.state_of(What::Contents, key(200)),
            GenState::Refused("over_memory".into()),
            "the contents limit is still remembered"
        );
        assert_eq!(generator.state_of(What::Blocks, key(0)), GenState::Idle, "the oldest blocks refusal gave way");
        assert_eq!(generator.state_of(What::Blocks, key(1)), GenState::Refused("couldnt_read".into()));
    }

    /// Failure: eight agents' blocks asks push a person's waiting Generate-contents ask off the queue (newest first, one cap of eight), so the button does nothing; or it waits behind the agents' asks. A person's waiting ask stays and goes first;
    /// past the cap the agents' oldest gives way, and an agent's ask when only people's wait is the one that isn't kept.
    #[test]
    fn a_persons_waiting_ask_is_never_given_way_for_an_agents() {
        let stand = Arc::new(Stand::default());
        let generator = both(&stand);
        generator.want(Path::new("/nowhere/a.pdf"), key(1), 10);
        running(&generator, What::Contents, 1);
        generator.want_as_person(Path::new("/nowhere/p.pdf"), key(2), 10);
        for number in 10..20u8 {
            blocks::Blocking::want(&generator, ask("/nowhere/b.pdf", number));
        }
        assert_eq!(generator.state_of(What::Contents, key(2)), GenState::Queued, "the person's ask stays");
        assert_eq!(generator.state_of(What::Blocks, key(10)), GenState::Idle, "the agents' oldest gave way");
        assert_eq!(generator.state_of(What::Blocks, key(12)), GenState::Idle);
        assert_eq!(generator.state_of(What::Blocks, key(19)), GenState::Queued);
        *stand.gate.lock().unwrap() = 1000;
        stand.moved.notify_all();
        settle(&generator);
        let started = stand.started.lock().unwrap().clone();
        assert_eq!(
            started[..2],
            [(What::Contents, 1), (What::Contents, 2)],
            "the person's ask runs before the agents' waiting: {started:?}"
        );

        // Only people's asks waiting: another person's ask gives way the oldest person's, an agent's isn't kept.
        let stand = Arc::new(Stand::default());
        let generator = both(&stand);
        generator.want(Path::new("/nowhere/a.pdf"), key(1), 10);
        running(&generator, What::Contents, 1);
        for number in 20..20 + MAX_WAITING as u8 {
            generator.want_as_person(Path::new("/nowhere/p.pdf"), key(number), 10);
        }
        blocks::Blocking::want(&generator, ask("/nowhere/b.pdf", 90));
        assert_eq!(
            generator.state_of(What::Blocks, key(90)),
            GenState::Idle,
            "an agent's ask doesn't push a person's off"
        );
        assert_eq!(generator.state_of(What::Contents, key(20)), GenState::Queued);
        generator.want_as_person(Path::new("/nowhere/p.pdf"), key(60), 10);
        assert_eq!(generator.state_of(What::Contents, key(60)), GenState::Queued);
        assert_eq!(
            generator.state_of(What::Contents, key(20)),
            GenState::Idle,
            "a person's own oldest gave way to their newest"
        );
        *stand.gate.lock().unwrap() = 1000;
        stand.moved.notify_all();
        settle(&generator);
    }

    /// Failure: the time a job gets doesn't grow with the document, so a long one is always killed, or has no ceiling.
    #[test]
    fn the_time_allowed_grows_with_pages_and_has_a_ceiling() {
        assert!(time_limit(383) > time_limit(10));
        assert_eq!(time_limit(u32::MAX), Duration::from_secs(30 * 60));
    }
}
