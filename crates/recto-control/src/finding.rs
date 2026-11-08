//! Find, off every thread that mustn't wait. **One query at a time:** a new one stops the last between two pages and takes its place, and the one thread here exists only while a query runs, so a session at rest has none. It reads pages as `search` does, from a corpus or through one worker, so at most one worker is held, and only while it runs.
//!
//! **Bounded from outside:** [`recto_agent::find::MAX_HITS`] hits kept, [`recto_agent::find::MAX_TIME`] a query, and whoever's listening hears no more often than every [`SAY_EVERY`], with the last word always said.

use crate::indexing::{Indexer, Notifier};
use recto_agent::Event;
use recto_agent::find::{self, FindPlan, Finding, Results};
use recto_agent::index::Indexing;
use serde_json::json;
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
        Self(Arc::new(Shared { indexer, inner: Mutex::new(Inner::default()), changed: Condvar::new(), notify: OnceLock::new() }))
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
            let indexing = self.indexer.as_ref().map(|indexer| indexer as &dyn Indexing);
            let mut said = Instant::now();
            let generation = job.generation;
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                find::run(&job.plan, &job.needle, indexing, &stop, &mut |hits, progress| {
                    let had_hits = !hits.is_empty();
                    self.lock().results.add(generation, hits, progress);
                    // The first hits are said at once, so the list starts as soon as there's something in it.
                    if said.elapsed() >= SAY_EVERY || (had_hits && progress.pages_with_hits == 1) {
                        said = Instant::now();
                        self.say(vec![Event::new("find_changed", json!({ "running": true }))]);
                    }
                })
            }));
            let (end, progress) = outcome.unwrap_or_else(|_| (find::End::Failed("Find stopped on something it couldn't read. Try again.".to_string()), find::Progress::default()));
            let current = {
                let mut inner = self.lock();
                inner.results.finish(generation, end, progress);
                self.changed.notify_all();
                inner.results.generation == generation
            };
            if current {
                self.say(vec![Event::new("find_finished", json!({ "hits": progress.total }))]);
            }
        }
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
            let spawned = std::thread::Builder::new().name("recto-find".into()).stack_size(8 * 1024 * 1024).spawn(move || shared.work());
            match spawned {
                Ok(_) => inner.working = true,
                Err(_) => {
                    inner.job = None;
                    inner.results.finish(generation, find::End::Failed("Find couldn't start just now. Try again.".to_string()), find::Progress::default());
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
