//! The purge of one plan step of the four cache stores (`docs/design/removal.md` §4 and §10 "D2e caches as built"): `CorpusText`, `CorpusAgentPages`,
//! `CorpusAliases` and `GeneratedContents`, by the pins [`survey_caches_to_clear`](super::survey_caches_to_clear) made. **Not wired to any command, and takes no
//! lease:** its quiescing (`CachesQuiet`), the walk, the barrier and the words belong to the command that calls it.
//!
//! By pin and by descriptor, never following a link and never opening a file for its content (a lock file is opened to take its lock and for nothing else):
//! - **Notes and contents** are files, taken by the history purge's two passes ([`purge_files`]): the one function, over the one folder.
//! - **A corpus** goes whole, by a tree pinned just before acting, one at a time. Pass 1 for the whole step: each is the folder that was pinned, and no build or
//!   agent session holds its lock; nothing is unlinked until every one has passed. Pass 2, folder by folder: look again; open it from the root's descriptor
//!   and compare; **hold `build.lock`** until it is gone; probe the `.session` locks again; build the tree under the survey's own rule
//!   ([`is_plain_cache_entry`]) and walk it, `ready.sealed`, `manifest.json` and `ready.json` first and made durable, so a stop part way leaves a corpus no
//!   search reads. A tree that isn't all plain stops as `NotThePinned`.
//! - **Descriptors:** never more than 8 at once (root, the four folder levels of a corpus, one listing, the held lock, one transient probe): a design limit
//!   that replaces §8's "four artifact descriptors" for this adapter.
//! - **An agent session's lock is probed and released** (a `.session` is never held through the act): a claim made in the gap before its folder goes finds the
//!   folder gone, and no data is kept. Step 4's quiescence (a [`CachesQuiet`]) closes that gap in this process; the probes are the safety net against another
//!   process's session and against a missing quiescence. **Our own process's live claim is `Busy` too**: flock locks are per open, so step 4 releases it first.
//! - **The older build's `corpus`** is a level deeper than a pin holds, so it goes child by child: each child by its own tree (a file by a look and an unlink),
//!   then `corpus` itself, looked at again, as a folder.

use super::{TREE_ENTRIES, is_plain_cache_entry, names, plain_tree_in};
use crate::removal_store::journals::{self, ListFailed, MAX_ENTRIES};
use crate::removal_store::purge::{Place, PurgeStop, Purged, Seams, VolumeNow, check_pins, purge_files, synced};
use recto_core::removal::{ArtifactKey, Pin, PinKind, Refusal, Store};
use recto_platform::folder::remove::{Failure, Walk, remove_tree_in};
use recto_platform::folder::{Entry, Folder, Kind, Opened};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Path, PathBuf};

#[cfg(test)]
use crate::removal_store::testing::Hook;

/// What the caches' writers being stopped looks like to the purge: it can't be called without one. The index, heading and blocks jobs, the agent exports and this
/// process's own agent claim are stopped and awaited by step 4, which makes the real one from its holds; there is no constructor in production yet.
#[derive(Debug)]
pub struct CachesQuiet {
    _stopped: (),
}

impl CachesQuiet {
    /// Stands for "no writer is running": a test runs no writer, or holds the real locks to stand for "not quiet".
    #[cfg(any(test, feature = "test-seams"))]
    pub fn assumed_for_tests() -> Self {
        Self { _stopped: () }
    }
}

/// Where the cache stores are.
pub struct CacheRoots<'a> {
    pub corpus: &'a Path,
    pub contents: &'a Path,
}

/// A test's gate on a walk's entries.
#[cfg(test)]
pub(in crate::removal_store) type Gate = std::sync::Arc<dyn Fn(&OsStr) -> bool + Send + Sync>;

/// A test's way to change a folder under the purge, to stop a walk, to say who owns a name, and to make a sync fail.
#[derive(Default)]
pub(in crate::removal_store) struct CacheSeams {
    /// The file purge's (notes and contents).
    pub files: Seams,
    /// Called with the name of each entry or folder of a tree just before it is unlinked; `false` stops the walk there.
    #[cfg(test)]
    pub gate: Option<Gate>,
    /// Called after each entry or folder of a tree was really unlinked.
    #[cfg(test)]
    pub unlinked: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    /// Called once, with the corpus root, between pass 1 and pass 2.
    #[cfg(test)]
    pub between_passes: Option<Hook>,
    /// Called with a folder's path in pass 2, before it is looked at again.
    #[cfg(test)]
    pub before_folder: Option<Hook>,
    /// The most entries one tree holds, in place of [`TREE_ENTRIES`].
    #[cfg(test)]
    pub tree_entries: Option<usize>,
    #[cfg(test)]
    pub uid_of: Option<super::UidOf>,
}

impl CacheSeams {
    fn tree_cap(&self) -> usize {
        #[cfg(test)]
        if let Some(cap) = self.tree_entries {
            return cap;
        }
        TREE_ENTRIES
    }

    fn uid(&self, entry: &Entry) -> u32 {
        #[cfg(test)]
        if let Some(uid) = self.uid_of.as_ref().and_then(|of| of(&entry.name)) {
            return uid;
        }
        entry.uid
    }

    fn gate(&self, name: &OsStr) -> bool {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            return gate(name);
        }
        let _ = name;
        true
    }

    fn unlinked(&self) {
        #[cfg(test)]
        if let Some(unlinked) = &self.unlinked {
            unlinked();
        }
    }

    fn folder_looked_at(&self, at: impl FnOnce() -> PathBuf) {
        #[cfg(test)]
        if let Some(hook) = &self.before_folder {
            hook(&at());
        }
        #[cfg(not(test))]
        let _ = at;
    }

    fn between_passes(&self, root: &Path) {
        #[cfg(test)]
        if let Some(hook) = &self.between_passes {
            hook(root);
        }
        #[cfg(not(test))]
        let _ = root;
    }
}

/// Takes the things of one plan step out of one cache store: `wanted` are the keys the clear-all survey gave and the pins the barrier recorded for them.
///
/// In this order, nothing touched until pass 1 has passed for the whole step: the store (`NotCache`), the batch (`DESIGN_LIMITS.max_batch`) and the pins (a
/// folder's for the two folder stores, a file's for the other two, on this volume, no key twice); the cache's root, opened by descriptor, and a listing of it
/// capped as the survey caps it; a wanted key not listed is already gone; pass 1, each match is what was pinned and no lock is held on it; pass 2, each looked
/// at again just before its own unlink; the root synced on every way out; a last listing that no wanted key is in.
pub fn purge_caches(store: Store, wanted: &[(ArtifactKey, Pin)], roots: &CacheRoots<'_>, volume: VolumeNow, quiet: &CachesQuiet) -> Result<Purged, PurgeStop> {
    purge_caches_with(store, wanted, roots, volume, quiet, &CacheSeams::default())
}

pub(in crate::removal_store) fn purge_caches_with(
    store: Store,
    wanted: &[(ArtifactKey, Pin)],
    roots: &CacheRoots<'_>,
    volume: VolumeNow,
    _quiet: &CachesQuiet,
    seams: &CacheSeams,
) -> Result<Purged, PurgeStop> {
    let kind = match store {
        Store::CorpusText | Store::CorpusAgentPages => PinKind::Folder,
        Store::CorpusAliases | Store::GeneratedContents => PinKind::File,
        _ => return Err(PurgeStop::NotCache),
    };
    let want = check_pins(wanted, volume, kind)?;
    match store {
        Store::CorpusAliases => purge_notes(&want, roots, volume, seams),
        Store::GeneratedContents => purge_contents(&want, roots, volume, seams),
        Store::CorpusText => purge_corpora(&want, roots, volume, seams),
        _ => purge_agent_folders(&want, roots, volume, seams),
    }
}

/// The folder at `path` of this user's, `None` if it isn't there, `FolderNotOurs` if it is a link, a file or another user's.
fn open_ours(path: &Path) -> Result<Option<Folder>, PurgeStop> {
    match Folder::open(path) {
        Opened::Missing => Ok(None),
        Opened::Other => Err(PurgeStop::FolderNotOurs),
        Opened::Folder(folder) => Ok(Some(folder)),
    }
}

fn open_in_ours(parent: &Folder, name: &OsStr) -> Result<Option<Folder>, PurgeStop> {
    match parent.open_in(name) {
        Opened::Missing => Ok(None),
        Opened::Other => Err(PurgeStop::FolderNotOurs),
        Opened::Folder(folder) => Ok(Some(folder)),
    }
}

fn list(folder: &Folder, cap: usize) -> Result<Vec<Entry>, PurgeStop> {
    journals::list_capped(folder, cap).map_err(|failed| match failed {
        ListFailed::Unreadable => PurgeStop::Listing,
        ListFailed::TooLarge => PurgeStop::TooLarge,
    })
}

fn key_in(store: Store, parts: &[&OsStr]) -> ArtifactKey {
    let mut key = Vec::new();
    for (at, part) in parts.iter().enumerate() {
        if at > 0 {
            key.push(b'/');
        }
        key.extend_from_slice(part.as_encoded_bytes());
    }
    ArtifactKey::of(store, &key)
}

/// A version note: the survey's key `by-file/<name>`.
fn purge_notes(want: &BTreeMap<ArtifactKey, Pin>, roots: &CacheRoots<'_>, volume: VolumeNow, seams: &CacheSeams) -> Result<Purged, PurgeStop> {
    let mut places = Vec::new();
    if let Some(root) = open_ours(roots.corpus)?
        && let Some(folder) = open_in_ours(&root, OsStr::new(names::VERSIONS))?
    {
        places.push(Place::new(folder, Vec::new(), roots.corpus.join(names::VERSIONS)));
    }
    let key_of = |_: &Place, entry: &Entry| names::alias_stem(&entry.name).map(|_| key_in(Store::CorpusAliases, &[OsStr::new(names::VERSIONS), &entry.name]));
    purge_files(&places, want, &key_of, MAX_ENTRIES, volume, &seams.files)
}

/// A generated contents file, by its name.
fn purge_contents(want: &BTreeMap<ArtifactKey, Pin>, roots: &CacheRoots<'_>, volume: VolumeNow, seams: &CacheSeams) -> Result<Purged, PurgeStop> {
    let mut places = Vec::new();
    if let Some(folder) = open_ours(roots.contents)? {
        places.push(Place::new(folder, Vec::new(), roots.contents.to_path_buf()));
    }
    let key_of = |_: &Place, entry: &Entry| names::contents_name(&entry.name).map(|_| key_in(Store::GeneratedContents, &[&entry.name]));
    purge_files(&places, want, &key_of, MAX_ENTRIES, volume, &seams.files)
}

/// Why a listed or looked-at entry isn't the folder that was pinned, if it isn't: a folder pin to a file, a link or a pipe is `NotAFile` (the kind that was
/// pinned isn't there), another device or inode `NotThePinned`.
fn differs_folder(entry: &Entry, pin: &Pin, volume: VolumeNow) -> Option<PurgeStop> {
    if entry.kind != Kind::Folder {
        return Some(PurgeStop::NotAFile);
    }
    (entry.dev != volume.dev || entry.ino != pin.inode).then_some(PurgeStop::NotThePinned)
}

/// What `look` found at `name`, `None` if nothing is there.
fn look(folder: &Folder, name: &OsStr) -> Result<Option<Entry>, PurgeStop> {
    match folder.look(name) {
        Ok(entry) => Ok(Some(entry)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(PurgeStop::Listing),
    }
}

/// Takes the lock of the plain file `name` in `folder` if nothing holds it: `Ok(None)` when there is no such file (a lock that isn't there is fine: nothing can
/// hold it, and a link or a pipe at the name holds nothing either, the tree refuses it), `Busy` when it is held. The file is returned held, for the caller to
/// keep or to let go.
fn lock_of(folder: &Folder, name: &str) -> Result<Option<File>, PurgeStop> {
    let name = OsStr::new(name);
    match look(folder, name)? {
        Some(entry) if entry.kind == Kind::File => {}
        _ => return Ok(None),
    }
    let file = match folder.open_private_rw_in(name) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(PurgeStop::Listing),
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(PurgeStop::Busy),
        Err(std::fs::TryLockError::Error(_)) => Err(PurgeStop::Listing),
    }
}

/// A corpus's `v4/app/build.lock`, taken and held for the caller to keep.
fn build_lock(key_folder: &Folder) -> Result<Option<File>, PurgeStop> {
    let Some(format) = open_in_ours(key_folder, OsStr::new(names::FORMAT_FOLDER)).or_else(not_there)? else {
        return Ok(None);
    };
    let Some(app) = open_in_ours(&format, OsStr::new(names::APP_FOLDER)).or_else(not_there)? else {
        return Ok(None);
    };
    lock_of(&app, names::BUILD_LOCK)
}

/// A folder that is there but not this user's folder holds nothing the purge can have pinned: the tree check refuses what it can't walk.
fn not_there(stop: PurgeStop) -> Result<Option<Folder>, PurgeStop> {
    if stop == PurgeStop::FolderNotOurs { Ok(None) } else { Err(stop) }
}

/// Every agent session lock under a corpus is free: each `.session` of an agent folder in any format folder is taken and let go at once, one at a time.
fn sessions_free(key_folder: &Folder) -> Result<(), PurgeStop> {
    for format in list(key_folder, 64)?.iter().filter(|entry| entry.kind == Kind::Folder) {
        let Some(format) = open_in_ours(key_folder, &format.name).or_else(not_there)? else {
            continue;
        };
        for agent in list(&format, 256)?
            .iter()
            .filter(|entry| entry.kind == Kind::Folder && names::is_agent_folder(&entry.name))
        {
            let Some(agent) = open_in_ours(&format, &agent.name).or_else(not_there)? else {
                continue;
            };
            lock_of(&agent, names::SESSION_FILE)?;
        }
    }
    Ok(())
}

/// Which locks a folder's act takes: a corpus's, with every agent's session under it, or an agent folder's own session with its corpus's build lock.
enum Locks<'a> {
    Corpus,
    Agent { corpus: &'a Folder },
}

impl Locks<'_> {
    /// Whether nothing holds what the folder `folder` (open) is held by, and the build lock if there is one, held until the caller drops it.
    fn take(&self, folder: &Folder) -> Result<Option<File>, PurgeStop> {
        match self {
            Locks::Corpus => {
                let held = build_lock(folder)?;
                sessions_free(folder)?;
                Ok(held)
            }
            Locks::Agent { corpus } => {
                let held = build_lock(corpus)?;
                lock_of(folder, names::SESSION_FILE)?;
                Ok(held)
            }
        }
    }
}

/// Whether a folder act took the name away.
enum Taken {
    Gone,
    Removed,
}

/// Pass 2 for one folder `name` of `parent`: look again, open it from `parent`'s descriptor and compare, hold its locks, build its tree under the survey's own
/// rule and walk it. Returns whether the folder went, and says in `went` that it did even when the walk stopped after taking the folder's name away.
fn take_folder(
    parent: &Folder,
    name: &OsStr,
    pin: &Pin,
    volume: VolumeNow,
    locks: &Locks<'_>,
    seams: &CacheSeams,
    touched: &Cell<bool>,
) -> Result<Taken, PurgeStop> {
    let Some(entry) = look(parent, name)? else { return Ok(Taken::Gone) };
    if let Some(stop) = differs_folder(&entry, pin, volume) {
        return Err(stop);
    }
    // The folder's descriptor, and the held lock, live in this block only: the walk below opens each level itself. The lock is held until the folder is gone,
    // so it is kept past the block in `_held`.
    let (_held, tree) = {
        let folder = match parent.open_in(name) {
            Opened::Missing => return Ok(Taken::Gone),
            Opened::Other => return Err(PurgeStop::NotThePinned),
            Opened::Folder(folder) => folder,
        };
        if folder.identity().map_err(|_| PurgeStop::Listing)? != entry.identity() {
            return Err(PurgeStop::NotThePinned);
        }
        let held = locks.take(&folder)?;
        (held, plain_tree(&folder, seams)?)
    };
    walk_away(parent, name, &tree, seams, touched).map(|_| Taken::Removed)
}

/// The tree of an open folder under the survey's rule, as a purge's refusal.
/// Closes a folder now (a named step, so the descriptor bound is read in the code).
fn drop_folder(folder: Folder) {
    let _ = folder;
}

fn plain_tree(folder: &Folder, seams: &CacheSeams) -> Result<recto_platform::folder::Tree, PurgeStop> {
    match plain_tree_in(folder, seams.tree_cap(), &|entry| seams.uid(entry)) {
        Ok(Some(tree)) => Ok(tree),
        Ok(None) => Err(PurgeStop::NotThePinned),
        Err(Refusal::TooLarge) => Err(PurgeStop::TooLarge),
        Err(_) => Err(PurgeStop::Listing),
    }
}

/// `remove_tree_in` with the caches' order (what makes a corpus usable first, made durable) and the seams, its failures as a purge's.
fn walk_away(parent: &Folder, name: &OsStr, tree: &recto_platform::folder::Tree, seams: &CacheSeams, touched: &Cell<bool>) -> Result<(), PurgeStop> {
    // 0: what makes a corpus unusable. 1: `app` among a format folder's children (depth 1: the entries of `<key>/<format>`, whose own folder is depth 1 under
    // the corpus at 0), the text and the index, before the agent folders. 2: the rest.
    let rank = |depth: usize, name: &OsStr, _: Kind| match () {
        () if names::is_first_to_go(name) => 0,
        () if depth == 1 && name == names::APP_FOLDER => 1,
        () => 2,
    };
    let mut gate = |name: &OsStr| seams.gate(name);
    let mut unlinked = || {
        touched.set(true);
        seams.unlinked();
    };
    let mut walk = Walk {
        rank: &rank,
        gate: &mut gate,
        unlinked: &mut unlinked,
    };
    match remove_tree_in(parent, name, tree, &mut walk) {
        Ok(_) => Ok(()),
        Err(Failure::Changed(_) | Failure::Unpinned(_)) => Err(PurgeStop::NotThePinned),
        Err(Failure::TooDeep) => Err(PurgeStop::TooLarge),
        Err(Failure::Stopped(_) | Failure::NotDurable(_) | Failure::Io(..)) => Err(PurgeStop::Listing),
    }
}

/// A corpus (and the older build's `corpus`) of `CorpusText`, taken whole.
fn purge_corpora(want: &BTreeMap<ArtifactKey, Pin>, roots: &CacheRoots<'_>, volume: VolumeNow, seams: &CacheSeams) -> Result<Purged, PurgeStop> {
    let old_key = key_in(Store::CorpusText, &[OsStr::new("old"), OsStr::new(names::OLD_FOLDER)]);
    let root = open_ours(roots.corpus)?;
    let mut matches = Vec::new();
    if let Some(root) = &root {
        for entry in list(root, MAX_ENTRIES)? {
            if matches!(names::root_name(&entry.name), names::RootName::Key(_))
                && let Some(pin) = want.get(&key_in(Store::CorpusText, &[&entry.name]))
            {
                matches.push((entry, *pin));
            }
        }
    }
    // A fixed order, so a stop part way leaves the same corpora and a resume takes the same rest.
    matches.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    let beside = beside_the_root(roots.corpus)?;
    let old = match (&beside, want.get(&old_key)) {
        (Some(parent), Some(pin)) => look(parent, OsStr::new(names::OLD_FOLDER))?.map(|entry| (entry, *pin)),
        _ => None,
    };
    let mut already_gone = (want.len() - matches.len() - usize::from(old.is_some())) as u32;
    // Pass 1 and pass 2 share their way out below: a stop in pass 1 syncs too, when a name was already gone (a run killed before its sync took it).
    let (mut removed, touched_root, touched_beside) = (0u32, Cell::new(false), Cell::new(false));
    let mut stop = corpora_pass_one(&root, &matches, &old, &beside, volume, seams).err();
    if stop.is_none() {
        seams.between_passes(roots.corpus);
        // Pass 2. `touched`: something was unlinked in the root's folders, so the root is synced on every way out, a stop inside a tree included.
        for (entry, pin) in &matches {
            let Some(root) = &root else { break };
            seams.folder_looked_at(|| roots.corpus.join(&entry.name));
            match take_folder(root, &entry.name, pin, volume, &Locks::Corpus, seams, &touched_root) {
                Ok(Taken::Removed) => removed += 1,
                Ok(Taken::Gone) => already_gone += 1,
                Err(failed) => {
                    stop = Some(failed);
                    break;
                }
            }
        }
    }
    if stop.is_none()
        && let (Some((_, pin)), Some(parent)) = (&old, &beside)
    {
        seams.folder_looked_at(|| roots.corpus.with_file_name(names::OLD_FOLDER));
        match take_old(parent, pin, volume, seams, Some(&touched_beside)) {
            Ok(Taken::Removed) => removed += 1,
            Ok(Taken::Gone) => already_gone += 1,
            Err(failed) => stop = Some(failed),
        }
    }
    // Every way out syncs what lost a name; one already gone may have been taken by a run that was killed before its sync.
    let mut durable = true;
    for (folder, lost) in [(&root, touched_root.get()), (&beside, touched_beside.get())] {
        if let Some(folder) = folder
            && (lost || already_gone > 0)
            && !synced(folder, &seams.files)
        {
            durable = false;
        }
    }
    if let Some(stop) = stop {
        return Err(stop);
    }
    // A last listing in which no wanted key is found.
    if let Some(root) = &root {
        for entry in list(root, MAX_ENTRIES)? {
            if want.contains_key(&key_in(Store::CorpusText, &[&entry.name])) {
                return Err(PurgeStop::StillThere);
            }
        }
    }
    if let Some(parent) = &beside
        && want.contains_key(&old_key)
        && look(parent, OsStr::new(names::OLD_FOLDER))?.is_some()
    {
        return Err(PurgeStop::StillThere);
    }
    Ok(Purged {
        removed,
        already_gone,
        durable,
    })
}

/// Pass 1 of a corpus step: nothing is unlinked unless every folder to go is what was pinned and nothing holds it, the older build's `corpus` included.
fn corpora_pass_one(
    root: &Option<Folder>,
    matches: &[(Entry, Pin)],
    old: &Option<(Entry, Pin)>,
    beside: &Option<Folder>,
    volume: VolumeNow,
    seams: &CacheSeams,
) -> Result<(), PurgeStop> {
    for (entry, pin) in matches {
        if let Some(stop) = differs_folder(entry, pin, volume) {
            return Err(stop);
        }
        let Some(root) = root else { break };
        let Some(folder) = open_in_ours(root, &entry.name)? else { continue };
        if folder.identity().map_err(|_| PurgeStop::Listing)? != entry.identity() {
            return Err(PurgeStop::NotThePinned);
        }
        Locks::Corpus.take(&folder)?;
    }
    if let (Some((entry, pin)), Some(parent)) = (old, beside) {
        if let Some(stop) = differs_folder(entry, pin, volume) {
            return Err(stop);
        }
        take_old(parent, pin, volume, seams, None)?;
    }
    Ok(())
}

/// The folder beside the corpus root, if the root is named as the app names it (where the older build's `corpus` is).
fn beside_the_root(root: &Path) -> Result<Option<Folder>, PurgeStop> {
    if root.file_name() != Some(OsStr::new(names::CORPUS_ROOT_NAME)) {
        return Ok(None);
    }
    match root.parent() {
        Some(parent) => open_ours(parent),
        None => Ok(None),
    }
}

/// The older build's `corpus` in `parent`: a check (`check_only`: each child is plain, nothing taken) or the act, child by child and then itself.
fn take_old(parent: &Folder, pin: &Pin, volume: VolumeNow, seams: &CacheSeams, act: Option<&Cell<bool>>) -> Result<Taken, PurgeStop> {
    let name = OsStr::new(names::OLD_FOLDER);
    let Some(entry) = look(parent, name)? else { return Ok(Taken::Gone) };
    if let Some(stop) = differs_folder(&entry, pin, volume) {
        return Err(stop);
    }
    let Some(old) = open_in_ours(parent, name)? else { return Ok(Taken::Gone) };
    if old.identity().map_err(|_| PurgeStop::Listing)? != entry.identity() {
        return Err(PurgeStop::NotThePinned);
    }
    for child in list(&old, MAX_ENTRIES)? {
        match child.kind {
            Kind::Folder => {
                let Some(inner) = open_in_ours(&old, &child.name)? else { continue };
                if inner.identity().map_err(|_| PurgeStop::Listing)? != child.identity() || !is_plain_cache_entry(&child, seams.uid(&child)) {
                    return Err(PurgeStop::NotThePinned);
                }
                let tree = plain_tree(&inner, seams)?;
                drop_folder(inner);
                if let Some(touched) = act {
                    walk_away(&old, &child.name, &tree, seams, touched)?;
                }
            }
            Kind::File if is_plain_cache_entry(&child, seams.uid(&child)) => {
                if let Some(touched) = act {
                    unlink_file(&old, &child, seams, touched)?;
                }
            }
            _ => return Err(PurgeStop::NotThePinned),
        }
    }
    let Some(touched) = act else { return Ok(Taken::Removed) };
    // Looked at again, and unlinked as a folder.
    let Some(now) = look(parent, name)? else { return Ok(Taken::Gone) };
    if let Some(stop) = differs_folder(&now, pin, volume) {
        return Err(stop);
    }
    match parent.unlink_in(name, true) {
        Ok(()) => {
            touched.set(true);
            Ok(Taken::Removed)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Taken::Gone),
        Err(error) if error.raw_os_error() == Some(libc::ENOTEMPTY) => Err(PurgeStop::NotThePinned),
        Err(_) => Err(PurgeStop::Listing),
    }
}

/// A plain child of the older folder, looked at once more and unlinked.
fn unlink_file(folder: &Folder, listed: &Entry, seams: &CacheSeams, touched: &Cell<bool>) -> Result<(), PurgeStop> {
    if !seams.gate(&listed.name) {
        return Err(PurgeStop::Listing);
    }
    let Some(now) = look(folder, &listed.name)? else { return Ok(()) };
    if now.identity() != listed.identity() || !is_plain_cache_entry(&now, seams.uid(&now)) {
        return Err(PurgeStop::NotThePinned);
    }
    match folder.unlink_in(&listed.name, false) {
        Ok(()) => {
            touched.set(true);
            seams.unlinked();
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PurgeStop::Listing),
    }
}

/// One agent folder, found through its corpus and format folder.
struct Agent {
    corpus: OsString,
    format: OsString,
    entry: Entry,
    pin: Pin,
}

/// Every agent folder of every corpus that is one of `want`'s, keyed as the survey keys them (`<corpus>/<format>/<agent>`). Listed as the survey lists: the
/// root's entries, a corpus's, and each format folder's, all capped. A corpus or format folder that isn't there holds none.
fn find_agents(root: &Folder, want: &BTreeMap<ArtifactKey, Pin>) -> Result<Vec<Agent>, PurgeStop> {
    let mut found = Vec::new();
    for corpus in list(root, MAX_ENTRIES)?
        .iter()
        .filter(|entry| entry.kind == Kind::Folder && matches!(names::root_name(&entry.name), names::RootName::Key(_)))
    {
        let Some(key_folder) = open_in_ours(root, &corpus.name).or_else(not_there)? else {
            continue;
        };
        let formats = list(&key_folder, 64)?;
        let mut opened = 0;
        for format in formats.iter().filter(|entry| entry.kind == Kind::Folder) {
            opened += 1;
            if opened > 8 {
                return Err(PurgeStop::TooLarge);
            }
            let Some(folder) = open_in_ours(&key_folder, &format.name).or_else(not_there)? else {
                continue;
            };
            for agent in list(&folder, 256)? {
                if !names::is_agent_folder(&agent.name) {
                    continue;
                }
                if let Some(pin) = want.get(&key_in(Store::CorpusAgentPages, &[&corpus.name, &format.name, &agent.name])) {
                    found.push(Agent {
                        corpus: corpus.name.clone(),
                        format: format.name.clone(),
                        entry: agent,
                        pin: *pin,
                    });
                }
            }
        }
    }
    Ok(found)
}

/// How far an agent's corpus and format folder open, by the root's descriptor.
enum Chain {
    Whole {
        corpus: Folder,
        format: Folder,
    },
    /// The corpus is there and the format folder isn't.
    FormatGone {
        corpus: Folder,
    },
    CorpusGone,
}

fn open_chain(root: &Folder, agent: &Agent) -> Result<Chain, PurgeStop> {
    let Some(corpus) = open_in_ours(root, &agent.corpus).or_else(not_there)? else {
        return Ok(Chain::CorpusGone);
    };
    let Some(format) = open_in_ours(&corpus, &agent.format).or_else(not_there)? else {
        return Ok(Chain::FormatGone { corpus });
    };
    Ok(Chain::Whole { corpus, format })
}

/// Pass 1 of an agent step: each is the folder that was pinned and no session or build holds it.
fn agents_pass_one(root: &Folder, agents: &[Agent], volume: VolumeNow) -> Result<(), PurgeStop> {
    for agent in agents {
        if let Some(stop) = differs_folder(&agent.entry, &agent.pin, volume) {
            return Err(stop);
        }
        let Chain::Whole { corpus, format } = open_chain(root, agent)? else {
            continue;
        };
        let Some(folder) = open_in_ours(&format, &agent.entry.name)? else { continue };
        if folder.identity().map_err(|_| PurgeStop::Listing)? != agent.entry.identity() {
            return Err(PurgeStop::NotThePinned);
        }
        Locks::Agent { corpus: &corpus }.take(&folder)?;
    }
    Ok(())
}

/// The agent folders of `CorpusAgentPages`: normally found already gone, because their corpus went with the `CorpusText` step before this one.
fn purge_agent_folders(want: &BTreeMap<ArtifactKey, Pin>, roots: &CacheRoots<'_>, volume: VolumeNow, seams: &CacheSeams) -> Result<Purged, PurgeStop> {
    let Some(root) = open_ours(roots.corpus)? else {
        return Ok(Purged {
            removed: 0,
            already_gone: want.len() as u32,
            durable: true,
        });
    };
    let agents = find_agents(&root, want)?;
    let mut already_gone = (want.len() - agents.len()) as u32;
    // Pass 1 and pass 2 share their way out below: a stop in pass 1 syncs the root too, when a name was already gone.
    let (mut removed, touched_root) = (0u32, Cell::new(false));
    let mut stop = agents_pass_one(&root, &agents, volume).err();
    if stop.is_none() {
        seams.between_passes(roots.corpus);
    }
    // Pass 2.
    let mut durable = true;
    for agent in agents.iter().filter(|_| stop.is_none()) {
        seams.folder_looked_at(|| roots.corpus.join(&agent.corpus).join(&agent.format).join(&agent.entry.name));
        let (corpus, format) = match open_chain(&root, agent) {
            Ok(Chain::Whole { corpus, format }) => (corpus, format),
            Ok(gone) => {
                already_gone += 1;
                // The format folder (or its corpus) that lost the name may have been taken by a run killed before its sync.
                if let Chain::FormatGone { corpus } = &gone {
                    durable &= synced(corpus, &seams.files);
                }
                continue;
            }
            Err(failed) => {
                stop = Some(failed);
                break;
            }
        };
        let touched = Cell::new(false);
        let taken = take_folder(
            &format,
            &agent.entry.name,
            &agent.pin,
            volume,
            &Locks::Agent { corpus: &corpus },
            seams,
            &touched,
        );
        // What lost a name is synced on every way out, a stop inside the tree included; so is the format folder of one found already gone (a killed run's).
        // A sync that fails is `durable: false` and the step goes on.
        if touched.get() {
            touched_root.set(true);
        }
        if touched.get() || matches!(taken, Ok(Taken::Gone)) {
            durable &= synced(&format, &seams.files);
        }
        match taken {
            Ok(Taken::Removed) => removed += 1,
            Ok(Taken::Gone) => already_gone += 1,
            Err(failed) => {
                stop = Some(failed);
                break;
            }
        }
    }
    if (touched_root.get() || already_gone > 0) && !synced(&root, &seams.files) {
        durable = false;
    }
    if let Some(stop) = stop {
        return Err(stop);
    }
    if !find_agents(&root, want)?.is_empty() {
        return Err(PurgeStop::StillThere);
    }
    Ok(Purged {
        removed,
        already_gone,
        durable,
    })
}

#[cfg(test)]
mod tests;
