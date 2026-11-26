//! **The tabs of a whole record, noted** (`note_tabs`): each tab's (path, file version) the ledger hasn't been told about yet is looked up and appended by one
//! short-lived thread, `recto-ledger`. A record is written on the app's own cadence and the same tabs come round again and again, so this must cost almost nothing
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
}

static ROOTS: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

fn roots() -> std::sync::MutexGuard<'static, Vec<Seen>> {
    ROOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The roots held now, and the hashes held for `root`: what this module keeps, for a test to count.
#[cfg(test)]
pub(crate) fn held(root: &Path) -> (usize, usize) {
    let roots = roots();
    (roots.len(), roots.iter().find(|r| r.root == root).map_or(0, |r| r.hashes.len()))
}

/// Whether a `recto-ledger` thread of `root` is running: for a test, or a measurement, to wait for it to end.
pub fn running(root: &Path) -> bool {
    roots().iter().any(|r| r.root == root && r.running)
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

pub(crate) fn note_tabs_with(root: &Path, tabs: &[TabEntry], config: &Config) {
    let mut held = roots();
    let at = match held.iter().position(|seen| seen.root == root) {
        Some(at) => at,
        None => {
            if held.len() >= MAX_ROOTS {
                held.remove(0);
            }
            held.push(Seen {
                root: root.to_path_buf(),
                ..Seen::default()
            });
            held.len() - 1
        }
    };
    let seen = &mut held[at];
    if seen.running {
        return;
    }
    let mut new = Vec::new();
    for tab in tabs {
        let Some(file) = tab.file else { continue };
        let Some(path) = tab.path.to_path().filter(|p| p.is_absolute()) else {
            continue;
        };
        let hash = hash_of(&path, file);
        if seen.hashes.contains(&hash) {
            continue;
        }
        if new.len() >= MAX_PER_CALL {
            break;
        }
        new.push((hash, path, FileStamp::from(file)));
    }
    if new.is_empty() {
        return;
    }
    if seen.hashes.len() + new.len() > MAX_SEEN {
        seen.hashes.clear();
    }
    seen.hashes.extend(new.iter().map(|(hash, ..)| *hash));
    seen.running = true;
    drop(held);
    let (root_owned, config) = (root.to_path_buf(), config.clone());
    let spawned = std::thread::Builder::new().name("recto-ledger".into()).spawn({
        let root = root_owned.clone();
        move || {
            let _done = Done(root.clone());
            note_all(&root, &config, new);
        }
    });
    if spawned.is_err() {
        finish(&root_owned);
    }
}

/// Says the thread is over however it ended, a panic included.
struct Done(PathBuf);

impl Drop for Done {
    fn drop(&mut self) {
        finish(&self.0);
    }
}

fn finish(root: &Path) {
    if let Some(seen) = roots().iter_mut().find(|seen| seen.root == root) {
        seen.running = false;
    }
}

fn note_all(root: &Path, config: &Config, new: Vec<(u64, PathBuf, FileStamp)>) {
    #[cfg(test)]
    if let Some(seam) = &config.in_thread {
        seam();
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
