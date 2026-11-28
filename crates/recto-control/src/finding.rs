//! Find, off every thread that mustn't wait. **One query at a time:** a new one stops the last between two pages and takes its place, and the one thread here exists only while a query runs, so a session at rest has none. It reads pages as `search` does, from a corpus or through one worker, so at most one worker is held, and only while it runs.
//!
//! **Never waits for the Find key:** a query reads from corpora only with the key in hand ([`Indexer::find_key_in_hand`]). The first one of a run, before the key's been taken, reads its pages live while the key's thread goes on, and the next has the index. Find is the person's and `recto find`'s alike, so an agent's `find` doesn't wait either; an agent's `search` may, as it takes the key itself.
//!
//! **Bounded from outside:** [`recto_agent::find::MAX_HITS`] hits kept, [`recto_agent::find::MAX_TIME`] a query, and whoever's listening hears no more often than every [`SAY_EVERY`], with the last word always said.

use crate::indexing::{Indexer, Notifier};
use recto_agent::Event;
use recto_agent::find::{self, FindPlan, Finding, Results};
use recto_agent::index::Indexing;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// How often listeners hear how a query's going, at most. Often enough for a list that grows as you watch, and no more.
pub const SAY_EVERY: Duration = Duration::from_millis(80);

struct Job {
    generation: u64,
    plan: FindPlan,
    needle: String,
}

#[derive(Default)]
struct Inner {
    results: Results,
    job: Option<Job>,
    /// The run going on now's, so a new query can stop it.
    stop: Arc<AtomicBool>,
    working: bool,
    stopped: bool,
}

struct Shared {
    indexer: Option<Indexer>,
    inner: Mutex<Inner>,
    changed: Condvar,
    notify: OnceLock<Notifier>,
}

/// Whoever holds a session holds one of these. A handle: a clone is the same Find.
#[derive(Clone)]
pub struct Finder(Arc<Shared>);

impl std::fmt::Debug for Finder {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Finder").finish_non_exhaustive()
    }
}

impl Finder {
    /// Reading pages from corpora this indexer keeps, and asking it for them, as a search does.
    pub fn new(indexer: Option<Indexer>) -> Self {
        Self(Arc::new(Shared {
            indexer,
            inner: Mutex::new(Inner::default()),
            changed: Condvar::new(),
            notify: OnceLock::new(),
        }))
    }

    pub fn attach(&self, notify: Notifier) {
        let _ = self.0.notify.set(notify);
    }

    /// Stops what's running, for good.
    pub fn shut_down(&self) {
        let mut inner = self.0.lock();
        inner.stopped = true;
        inner.job = None;
        inner.stop.store(true, Ordering::SeqCst);
    }

    /// Whether a query is running now.
    pub fn is_running(&self) -> bool {
        self.0.lock().results.running()
    }

    /// How many hits are held now.
    pub fn hits_held(&self) -> usize {
        self.0.lock().results.hits.len()
    }

    /// The thread that runs queries, while there's one.
    pub fn has_thread(&self) -> bool {
        self.0.lock().working
    }

    /// **While the thread's there, how far it's got:** which run this is (every query started, found again or cleared counts one) and the pages read of it,
    /// for a wait that runs out on Find to say whether one query was slow or queries kept starting again. `None` with no thread.
    pub fn looking(&self) -> Option<(u64, find::Progress)> {
        let inner = self.0.lock();
        inner.working.then_some((inner.results.generation, inner.results.progress))
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn say(&self, events: Vec<Event>) {
        if let Some(notify) = self.notify.get() {
            notify(events);
        }
    }

    /// The thread: takes the latest query, runs it, and goes when there's none.
    fn work(self: Arc<Self>) {
        loop {
            let (job, stop) = {
                let mut inner = self.lock();
                match inner.job.take() {
                    Some(job) if !inner.stopped => (job, Arc::clone(&inner.stop)),
                    _ => {
                        inner.working = false;
                        self.changed.notify_all();
                        return;
                    }
                }
            };
            let in_hand = self.indexer.as_ref().map(KeyInHand);
            let indexing = in_hand.as_ref().map(|indexer| indexer as &dyn Indexing);
            let mut said = Instant::now();
            let generation = job.generation;
            // The document it searches: what decides whether its news may go to an agent (`recto_agent::withheld`).
            let tab = Some(recto_session::TabId(job.plan.tab()));
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                find::run(&job.plan, &job.needle, indexing, &stop, &mut |hits, progress| {
                    let had_hits = !hits.is_empty();
                    self.lock().results.add(generation, hits, progress);
                    // A step waiting for its hit looks again.
                    self.changed.notify_all();
                    // The first hits are said at once, so the list starts as soon as there's something in it.
                    if said.elapsed() >= SAY_EVERY || (had_hits && progress.pages_with_hits == 1) {
                        said = Instant::now();
                        self.say(vec![Event::FindRunning { tab }]);
                    }
                })
            }));
            let (end, progress) = outcome.unwrap_or_else(|_| {
                (
                    find::End::Failed("Find stopped on something it couldn't read. Try again.".to_string()),
                    find::Progress::default(),
                )
            });
            let current = {
                let mut inner = self.lock();
                inner.results.finish(generation, end, progress);
                self.changed.notify_all();
                inner.results.generation == generation
            };
            if current {
                self.say(vec![Event::FindFinished { tab, hits: progress.total }]);
            }
        }
    }
}

/// The indexer as Find reads it: everything as it is, but the key only if it's in hand.
struct KeyInHand<'a>(&'a Indexer);

impl Indexing for KeyInHand<'_> {
    fn root(&self) -> &std::path::Path {
        Indexing::root(self.0)
    }
    fn find_key(&self) -> Option<recto_agent::index::KeyUse<'_>> {
        self.0.find_key_in_hand()
    }
    fn cache_use(&self) -> Option<recto_agent::index::CacheUse<'_>> {
        Indexing::cache_use(self.0)
    }
    fn made_agent_folder(&self, key: recto_index::corpus::CorpusKey) {
        self.0.made_agent_folder(key);
    }
    fn find_key_report(&self) -> serde_json::Value {
        self.0.find_key_report()
    }
    fn want(&self, path: &std::path::Path, key: recto_index::corpus::CorpusKey) {
        self.0.want(path, key);
    }
    fn job(&self, key: recto_index::corpus::CorpusKey) -> recto_agent::index::JobState {
        self.0.job(key)
    }
    fn load(&self) -> (u32, u32) {
        Indexing::load(self.0)
    }
    fn note_asked(&self) {
        self.0.note_asked();
    }
    fn cancel(&self, key: Option<recto_index::corpus::CorpusKey>) {
        self.0.cancel(key);
    }
    fn starts_builds(&self) -> bool {
        self.0.starts_builds()
    }
}

impl Finding for Finder {
    fn start(&self, plan: FindPlan, query: &str, needle: String) -> u64 {
        let mut inner = self.0.lock();
        // The last run stops between two pages, and a new flag is this one's.
        inner.stop.store(true, Ordering::SeqCst);
        inner.stop = Arc::new(AtomicBool::new(false));
        let generation = inner.results.begin_on(&plan, query);
        if inner.stopped {
            inner.results.finish(generation, find::End::Stopped, find::Progress::default());
            return generation;
        }
        inner.job = Some(Job { generation, plan, needle });
        if !inner.working {
            let shared = Arc::clone(&self.0);
            // The same parser and normaliser path the server's job threads run, on the same 8 MiB they have, which is only address space until it's touched.
            let spawned = std::thread::Builder::new()
                .name("recto-find".into())
                .stack_size(8 * 1024 * 1024)
                .spawn(move || shared.work());
            match spawned {
                Ok(_) => inner.working = true,
                Err(_) => {
                    inner.job = None;
                    inner.results.finish(
                        generation,
                        find::End::Failed("Find couldn't start just now. Try again.".to_string()),
                        find::Progress::default(),
                    );
                }
            }
        }
        generation
    }

    fn clear(&self) {
        let mut inner = self.0.lock();
        inner.stop.store(true, Ordering::SeqCst);
        inner.job = None;
        inner.results.clear();
        self.0.changed.notify_all();
    }

    fn with_results(&self, step: &mut dyn FnMut(&mut Results)) {
        step(&mut self.0.lock().results);
    }

    fn wait_past(&self, version: u64, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut inner = self.0.lock();
        loop {
            if inner.results.version() != version {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            inner = self.0.changed.wait_timeout(inner, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    fn wait(&self, generation: u64, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut inner = self.0.lock();
        loop {
            if inner.results.generation != generation || inner.results.end.is_some() {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            inner = self.0.changed.wait_timeout(inner, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find_keys::{AppKey, Plan};
    use recto_platform::find_key::FindKey;
    use std::sync::Condvar;

    /// The Keychain's stand-in, held shut until the test opens it, as the release check and the Keychain hold a real run.
    static GATE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
    static ENTERED: AtomicBool = AtomicBool::new(false);

    fn held_shut(_app: recto_platform::find_key::App) -> Option<(FindKey, Option<&'static str>)> {
        ENTERED.store(true, Ordering::SeqCst);
        let (open, moved) = &GATE;
        let mut open = open.lock().unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while !*open && Instant::now() < deadline {
            open = moved.wait_timeout(open, Duration::from_millis(200)).unwrap().0;
        }
        FindKey::in_memory().ok().map(|key| (key, None))
    }

    fn open_the_gate() {
        *GATE.0.lock().unwrap() = true;
        GATE.1.notify_all();
    }

    /// Failure (the second review's item 5, 2026-11-15): the person's Find, the first of a run, waits while the key is taken, which checks the whole bundle's signature (about 0.6 s on a 143 MB build, [`Indexer::find_key_in_hand`]) and asks the Keychain, so typing in the field shows nothing for that long or, with the Keychain slow, longer. With the loader held shut, Find answers from the pages read live, the key's taking has started and goes on, and once it's in, a search has it.
    #[test]
    fn find_answers_while_the_key_is_still_being_taken() {
        let file = recto_testpdf::TempPdf::new(
            "key-in-hand.pdf",
            &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["alpha words"]))]),
        );
        let mut session = recto_session::Session::new(recto_agent::DrawableLoader);
        recto_agent::run(
            &mut session,
            recto_agent::Level::Full,
            &recto_agent::Command::from(recto_agent::commands::tabs::Open {
                path: file.0.clone(),
                page: None,
            }),
        )
        .unwrap();
        let plan = find::plan(&session, session.tabs().active().unwrap()).unwrap();
        let temp = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-key-in-hand-");
        let root = temp.join("corpus.noindex");
        recto_platform::local::ensure_private_dir(&root).unwrap();
        let indexer = Indexer::with_limits(
            &root,
            Box::new(crate::indexing::HelperProcess {
                priority: crate::helpers::HELPER_PRIORITY,
            }),
            recto_index::corpus::BuildLimits::default(),
            Plan::AppKeychain(AppKey::standing_in(held_shut)),
        );
        let finder = Finder::new(Some(indexer.clone()));
        let generation = finder.start(plan, "alpha", find::needle("alpha").unwrap());
        let answered = finder.wait(generation, Duration::from_secs(20));
        let still_taking = indexer.find_key_report()["source"].is_null();
        if !answered {
            open_the_gate();
        }
        assert!(answered, "Find waited for the key");
        assert!(still_taking, "the key was in hand before Find ended, so this tested nothing");
        let mut total = 0;
        finder.with_results(&mut |results| total = results.progress.total);
        assert_eq!(total, 1, "Find found nothing in the pages it read live");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ENTERED.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(ENTERED.load(Ordering::SeqCst), "Find didn't start the key's taking");
        open_the_gate();
        while indexer.find_key_in_hand().is_none() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(indexer.find_key_in_hand().is_some(), "the key's taking didn't go on to the end");
    }

    /// Failure: Find's own view of the indexer (`KeyInHand`) forwards by hand, and a `cache_use` that answers plain there lets Find's throw-away of a damaged corpus run uncounted and unrefused during a removal. Over a held indexer it answers `None`, and over a free one a use the hold waits for.
    #[test]
    fn find_gives_no_cache_use_while_the_indexer_is_held() {
        let indexer = Indexer::with_runner(
            std::path::Path::new("/nowhere"),
            Box::new(crate::indexing::HelperProcess {
                priority: crate::helpers::HELPER_PRIORITY,
            }),
        );
        let find_view = KeyInHand(&indexer);
        let using = find_view.cache_use().expect("free: a use");
        assert!(
            indexer.hold_for_removal(Instant::now()).is_err(),
            "the use Find's view took is counted by the indexer"
        );
        drop(using);
        let hold = indexer.hold_for_removal(Instant::now()).expect("held");
        assert!(find_view.cache_use().is_none());
        drop(hold);
    }
}
