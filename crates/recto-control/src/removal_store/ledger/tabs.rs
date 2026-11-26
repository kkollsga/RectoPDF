//! **The tabs of a whole record, noted** (`note_tabs`): each tab's (path, file version) the ledger hasn't been told about yet is looked up and appended by one
//! short-lived thread, `recto-ledger`. The same thread, and the same "start it if none is running" ([`launch`]), does the one adoption of older journals
//! ([`request_adoption`]): a save never does. A record is written on the app's own cadence and the same tabs come round again and again, so this must cost almost nothing
//! when nothing is new: a hash of each pair against a small set, and no thread. The set is bounded (4,096 hashes, cleared when full) and kept for at most 8 state
//! roots; nothing is held per tab beyond it.

use super::{Config, append, canonical_version};
use recto_core::removal::{Entry, Link};
use recto_platform::FileStamp;
use recto_session::record::TabEntry;
use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Hashes a root keeps of pairs it has handed on.
const MAX_SEEN: usize = 4_096;
/// State roots whose sets are held.
const MAX_ROOTS: usize = 8;
/// New pairs one call hands to the thread.
const MAX_PER_CALL: usize = 64;

#[derive(Default)]
struct Seen {
    root: PathBuf,
    hashes: HashSet<u64>,
    /// A `recto-ledger` thread of this root is running.
    running: bool,
    /// The ledger was just begun and its thread has to adopt the older journals before it ends.
    adopt: bool,
}

/// The set of state roots held: one process-wide [`ROOTS`] in the app, and a value of their own in a test, so one test's roots never reach another's.
pub(crate) struct Roots(Mutex<Vec<Seen>>);

static ROOTS: Roots = Roots::new();

impl Roots {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Seen>> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The roots held now, and the hashes held for `root`: what this set keeps, for a test to count.
    #[cfg(test)]
    pub(crate) fn held(&self, root: &Path) -> (usize, usize) {
        let roots = self.lock();
        (roots.len(), roots.iter().find(|r| r.root == root).map_or(0, |r| r.hashes.len()))
    }

    /// Whether a `recto-ledger` thread of `root` is running.
    #[cfg(any(test, feature = "test-seams"))]
    pub(crate) fn running(&self, root: &Path) -> bool {
        self.lock().iter().any(|r| r.root == root && r.running)
    }

    /// The index of `root`'s entry, made if there is none: when eight are held, the oldest that has no thread out goes. `None` if every one has a thread.
    fn slot(held: &mut Vec<Seen>, root: &Path) -> Option<usize> {
        if let Some(at) = held.iter().position(|seen| seen.root == root) {
            return Some(at);
        }
        if held.len() >= MAX_ROOTS {
            held.remove(held.iter().position(|seen| !seen.running)?);
        }
        held.push(Seen {
            root: root.to_path_buf(),
            ..Seen::default()
        });
        Some(held.len() - 1)
    }

    /// Marks `root` as wanting its adoption; `Some(true)` if no thread of it was running, so the caller starts one. `None` if the root can't be registered.
    fn request_adoption(&self, root: &Path) -> Option<bool> {
        let mut held = self.lock();
        let at = Self::slot(&mut held, root)?;
        let seen = &mut held[at];
        seen.adopt = true;
        Some(!std::mem::replace(&mut seen.running, true))
    }

    /// Whether an adoption is wanted: takes the wish if so. If not, marks the thread over in the same step, so a request made just after can't be lost.
    fn next_job(&self, root: &Path) -> bool {
        let mut held = self.lock();
        let Some(seen) = held.iter_mut().find(|seen| seen.root == root) else {
            return false;
        };
        if seen.adopt {
            seen.adopt = false;
            return true;
        }
        seen.running = false;
        false
    }

    /// **The set's whole work, apart from the thread:** the pairs of `tabs` that `root`'s set hasn't seen (at most 64), put in the set, and the root marked as
    /// having a thread out; nothing if one is already, or nothing is new. A root is held only when it has something new, and one that has a thread out is never
    /// the one let go. The caller starts the thread and calls `finish` when it ends.
    pub(crate) fn take_new(&self, root: &Path, tabs: &[TabEntry]) -> Vec<(u64, PathBuf, FileStamp)> {
        let mut held = self.lock();
        let known = held.iter().position(|seen| seen.root == root);
        if known.is_some_and(|at| held[at].running) {
            return Vec::new();
        }
        let empty = HashSet::new();
        let new = fresh(known.map_or(&empty, |at| &held[at].hashes), tabs);
        if new.is_empty() {
            return new;
        }
        let Some(at) = Self::slot(&mut held, root) else { return Vec::new() };
        let seen = &mut held[at];
        if seen.hashes.len() + new.len() > MAX_SEEN {
            seen.hashes.clear();
        }
        seen.hashes.extend(new.iter().map(|(hash, ..)| *hash));
        seen.running = true;
        new
    }

    /// Says the thread of `take_new` is over.
    pub(crate) fn finish(&self, root: &Path) {
        if let Some(seen) = self.lock().iter_mut().find(|seen| seen.root == root) {
            seen.running = false;
            seen.adopt = false;
        }
    }
}

/// Whether a `recto-ledger` thread of `root` is running: for a test, or a measurement, to wait for it to end.
#[cfg(any(test, feature = "test-seams"))]
pub fn running(root: &Path) -> bool {
    ROOTS.running(root)
}

/// The process's set's work, public for a test that takes a set without starting a thread (see [`Roots::take_new`]).
#[cfg(any(test, feature = "test-seams"))]
pub fn take_new(root: &Path, tabs: &[TabEntry]) -> Vec<(u64, PathBuf, FileStamp)> {
    ROOTS.take_new(root, tabs)
}

/// Says the thread of [`take_new`] is over. Public for a test that takes a set without starting a thread.
#[cfg(any(test, feature = "test-seams"))]
pub fn finish(root: &Path) {
    ROOTS.finish(root);
}

fn hash_of(path: &Path, file: recto_session::record::FileRecord) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.as_os_str().as_encoded_bytes().hash(&mut hasher);
    (file.size, file.mtime_ns, file.device, file.inode).hash(&mut hasher);
    hasher.finish()
}

/// **Notes the tabs of a record that was written whole.** Returns at once; a thread notes the new ones.
pub fn note_tabs(root: &Path, tabs: &[TabEntry]) {
    note_tabs_with(root, tabs, &Config::default());
}

impl Config {
    /// The set this ledger's roots are in: the process's, or a test's own.
    fn roots(&self) -> &Roots {
        #[cfg(any(test, feature = "test-seams"))]
        if let Some(own) = &self.roots {
            return own;
        }
        &ROOTS
    }
}

pub(crate) fn note_tabs_with(root: &Path, tabs: &[TabEntry], config: &Config) {
    let new = config.roots().take_new(root, tabs);
    if !new.is_empty() {
        launch(root, config, new);
    }
}

/// **Hands the one adoption of the older journals to the ledger's thread**, starting it if none of this root is running, and returns at once. Called by a writer
/// that has just begun the ledger, with nothing held. If the root can't be registered (eight others have threads out) it isn't adopted: a backup with no record
/// reads `Unknown`, a false refusal and never a wrong answer.
pub(super) fn request_adoption(root: &Path, config: &Config) {
    let Some(start) = config.roots().request_adoption(root) else { return };
    if start {
        launch(root, config, Vec::new());
    }
}

/// Starts the `recto-ledger` thread for a root already marked running: it notes `new`, then adopts if that was asked for, then says it is over.
fn launch(root: &Path, config: &Config, new: Vec<(u64, PathBuf, FileStamp)>) {
    let spawned = std::thread::Builder::new().name("recto-ledger".into()).spawn({
        let (root, config) = (root.to_path_buf(), config.clone());
        move || {
            let mut done = Done(Some((root.clone(), config.clone())));
            work(&root, &config, new);
            // The thread ended by `next_job` saying so, under the lock that made it not running: nothing is left for the guard to say.
            done.0 = None;
        }
    });
    if spawned.is_err() {
        config.roots().finish(root);
    }
}

fn work(root: &Path, config: &Config, new: Vec<(u64, PathBuf, FileStamp)>) {
    note_all(root, config, new);
    while config.roots().next_job(root) {
        super::adopt_now(root, config);
    }
}

/// The pairs of `tabs` not in `known`, at most [`MAX_PER_CALL`].
fn fresh(known: &HashSet<u64>, tabs: &[TabEntry]) -> Vec<(u64, PathBuf, FileStamp)> {
    let mut new: Vec<(u64, PathBuf, FileStamp)> = Vec::new();
    for tab in tabs {
        let Some(file) = tab.file else { continue };
        let Some(path) = tab.path.to_path().filter(|p| p.is_absolute()) else {
            continue;
        };
        let hash = hash_of(&path, file);
        if known.contains(&hash) || new.iter().any(|(seen, ..)| *seen == hash) {
            continue;
        }
        if new.len() >= MAX_PER_CALL {
            break;
        }
        new.push((hash, path, FileStamp::from(file)));
    }
    new
}

/// Says the thread is over however it ended, a panic included, unless it already said so.
struct Done(Option<(PathBuf, Config)>);

impl Drop for Done {
    fn drop(&mut self) {
        if let Some((root, config)) = &self.0 {
            config.roots().finish(root);
        }
    }
}

fn note_all(root: &Path, config: &Config, new: Vec<(u64, PathBuf, FileStamp)>) {
    #[cfg(test)]
    if let Some(seam) = &config.in_thread {
        seam();
    }
    if new.is_empty() {
        return;
    }
    let entries: Vec<Entry> = new
        .into_iter()
        .filter_map(|(_, path, stamp)| canonical_version(&path, stamp))
        .map(|(path, version)| Entry::Link(Link::At { version, path }))
        .collect();
    if !entries.is_empty() {
        append(root, config, entries);
    }
}
