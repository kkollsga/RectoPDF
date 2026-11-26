//! The acts of Clear leftovers (`docs/design/removal.md` §10, D5): an unfinished save to the Trash, a copy of the session record deleted whole. Each is done by
//! descriptor, to what was pinned when the survey listed it, and stops at the first thing that is not that. **Not wired to any command, and takes no lease:**
//! the check, the leases, the words, the confirmation and the progress record belong to the command that calls it.
//!
//! - **A temp** goes only if the process number in its name is dead, its volume has a Trash, its folder and its own identity are the pinned ones, and the path
//!   the Trash will be given (`F_GETPATH` of the held folder, and its name) still leads to the pinned thing. Afterwards the item in the Trash must be that
//!   thing and its name gone from the folder. No Trash never means delete: the temp stays.
//! - **A copy** goes by [`remove_tree_in`]: every folder opened from its parent's descriptor, no link followed, every entry pinned, depth at most 3. A copy
//!   holding `went-back` is refused. An update's copy loses `.whole` first, so a copy cut short is one `restore_record` leaves alone; record slots and `places.log`
//!   go before `journals/`, then the rest. The parent is flushed once the copy is gone.
//! - **Order:** temps (the reversible), then the update's copy, then the session copies.
//!
//! **The live record is guarded** by [`is_live`], before the first act of a run and again just before each copy: a copy is refused (`Why::LiveRecord`) if its
//! folder is, by device and inode looked at then, either of the two live folders `Env::live` names, or if its name is `<flavor>.v<CURRENT_VERSION>` (beside a newer
//! record that name *is* the live store). Only pinned copies are ever opened: the surveys take them from `<root>/<flavor>.v<N>`, `<root>/<flavor>.damaged[.k]` and
//! `<root>/update/before`, and the live `<root>/<flavor>` is no name they list.

use super::pin::{CopyPin, PinSet, TempPin};
use super::trash::Trash;
use recto_core::removal::{ArtifactKey, Store};
use recto_platform::folder::remove::{Failure, Walk, remove_tree_in};
use recto_platform::folder::{Folder, Identity, Kind, Opened, identity_at};
use recto_platform::trash::Failure as TrashFailure;
use recto_session::record::CURRENT_VERSION;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// What the acts need from outside.
pub struct Env<'a> {
    pub trash: &'a dyn Trash,
    /// Whether a process number may be a running process. `recto_platform::local::process_exists` on a real run.
    pub alive: &'a dyn Fn(u32) -> bool,
    /// Whether a path is on a local volume: a process number in a name proves nothing anywhere else. `recto_platform::folder::is_local` on a real run.
    pub local: &'a dyn Fn(&Path) -> bool,
    /// The live record's folders of both flavors (`session` and `session-windowless`, as `RecordStore::dir()` says). Each is looked at again just before
    /// each copy, and a copy that is one of them is never deleted: beside a newer record the live store is `session.v<CURRENT>`, which a survey also finds
    /// as a copy. Always required: a clear that isn't told where the live record is does not run.
    pub live: [&'a Path; 2],
}

impl<'a> Env<'a> {
    pub fn new(trash: &'a dyn Trash, live: [&'a Path; 2]) -> Self {
        Self {
            trash,
            alive: &recto_platform::local::process_exists,
            local: &recto_platform::folder::is_local,
            live,
        }
    }
}

/// What a caller sees and may stop. Every method has a default that lets the act go on.
pub trait Hooks {
    /// Before an artifact's act. `false` stops the run before it.
    fn before(&mut self, _key: &ArtifactKey) -> bool {
        true
    }
    /// After an artifact's act was done and made durable.
    fn after(&mut self, _key: &ArtifactKey, _done: &Done) {}
    /// Before each entry of a copy is unlinked. `false` stops the walk before it.
    fn entry(&mut self, _name: &OsStr) -> bool {
        true
    }
}

/// Hooks that watch nothing.
pub struct Unwatched;

impl Hooks for Unwatched {}

/// What one artifact's act did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    /// The unfinished save is in the Trash, here.
    Trashed(PathBuf),
    /// The copy is gone: this many entries and folders unlinked, and this many pinned names that were already gone.
    Deleted { unlinked: usize, already_gone: usize },
    /// The temp was already gone from its folder.
    AlreadyGone,
}

/// Why a run stopped at an artifact. Whatever was done before it stays done.
#[derive(Debug)]
pub enum Why {
    /// The survey never pinned it (too deep, too large, or it changed while it was looked at).
    NotPinned,
    /// The process number in its name may still be running: it is being written.
    BeingWritten,
    /// Its volume has no Trash, so it is left.
    NoTrash,
    /// It holds `went-back`: work of an earlier version not yet brought in.
    WentBack,
    /// This is not what was pinned.
    Changed(OsString),
    /// This was not there when it was pinned.
    Unpinned(OsString),
    /// The pin goes deeper than the walk allows.
    TooDeep,
    /// The Trash refused it, or what it did left something else than what was pinned.
    Trash(TrashFailure),
    /// The item moved, but isn't what was pinned or its name is still in the folder. This is where it went, for the words to name.
    NotMoved(PathBuf),
    /// It is the live record's folder, which is never deleted.
    LiveRecord,
    /// It is done but the folder could not be made durable: the walk stops, and a power loss may bring it back (it is cleared again then).
    NotDurable,
    /// A system call failed.
    Io(OsString, io::ErrorKind),
    /// The caller's hook said stop.
    Asked,
}

impl Why {
    /// Whether it is unknown if the item is where it was: it moved or was taken out and could not be made durable, or what is in the Trash is not what
    /// was pinned. A carrier calls the step failed only when this is `false` for a Trash step (the file is where it was), and otherwise says the step is
    /// done but not durable.
    pub fn in_doubt(&self) -> bool {
        matches!(self, Self::NotMoved(_) | Self::NotDurable | Self::Trash(TrashFailure::MovedUnknown))
    }
}

/// What a run did: each artifact done, in order, and where and why it stopped, if it did.
#[derive(Debug, Default)]
pub struct Report {
    pub done: Vec<(ArtifactKey, Done)>,
    pub stopped: Option<(ArtifactKey, Why)>,
}

/// Whether an unfinished save may be moved now: what the check offers and what the act requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempState {
    Offerable,
    /// The process number in its name may be running (or isn't one).
    BeingWritten,
    NoTrash,
}

pub fn temp_state(pin: &TempPin, env: &Env<'_>) -> TempState {
    if pin.pid.is_none_or(|pid| (env.alive)(pid)) || !(env.local)(&pin.folder) {
        return TempState::BeingWritten;
    }
    if !env.trash.has_trash_for(&pin.folder) {
        return TempState::NoTrash;
    }
    TempState::Offerable
}

/// Whether a copy may be deleted now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyState {
    Offerable,
    /// It holds `went-back`.
    WentBack,
}

/// Whether a copy is, or may be, the live record: its folder is the identity one of the two live folders has right now, or its name is
/// `<flavor>.v<CURRENT_VERSION>`, which beside a newer record is the live store and which nothing may delete until the live folders are told for real.
pub fn is_live(pin: &CopyPin, env: &Env<'_>) -> bool {
    let named = ["session", "session-windowless"]
        .into_iter()
        .any(|flavor| pin.name.as_encoded_bytes() == format!("{flavor}.v{CURRENT_VERSION}").as_bytes());
    named || env.live.iter().any(|live| identity_at(live) == Some(pin.tree.id))
}

pub fn copy_state(pin: &CopyPin) -> CopyState {
    if pin.went_back { CopyState::WentBack } else { CopyState::Offerable }
}

/// Clears the artifacts named by `keys` that `pins` holds: first all checked without touching anything (one with no pin, a save being written, no Trash, a
/// copy holding `went-back` stops the run before the first act), then done one by one, each announced to `hooks`.
pub fn clear(pins: &PinSet, keys: &[ArtifactKey], env: &Env<'_>, hooks: &mut dyn Hooks) -> Report {
    let mut report = Report::default();
    let mut temps: Vec<&TempPin> = Vec::new();
    let mut copies: Vec<&CopyPin> = Vec::new();
    for key in keys {
        if temps.iter().any(|t| t.key == *key) || copies.iter().any(|c| c.key == *key) {
            continue;
        }
        if let Some(pin) = pins.temp(key) {
            match temp_state(pin, env) {
                TempState::Offerable => temps.push(pin),
                TempState::BeingWritten => return stopped(report, *key, Why::BeingWritten),
                TempState::NoTrash => return stopped(report, *key, Why::NoTrash),
            }
        } else if let Some(pin) = pins.copy(key) {
            if is_live(pin, env) {
                return stopped(report, *key, Why::LiveRecord);
            }
            match copy_state(pin) {
                CopyState::Offerable => copies.push(pin),
                CopyState::WentBack => return stopped(report, *key, Why::WentBack),
            }
        } else {
            return stopped(report, *key, Why::NotPinned);
        }
    }
    // The update's copy before the session copies; the sort is stable, so the caller's order holds within each.
    copies.sort_by_key(|pin| pin.store != Store::UpdateCopy);
    for pin in temps {
        if !hooks.before(&pin.key) {
            return stopped(report, pin.key, Why::Asked);
        }
        match trash_temp(pin, env) {
            Ok(done) => {
                hooks.after(&pin.key, &done);
                report.done.push((pin.key, done));
            }
            Err(why) => return stopped(report, pin.key, why),
        }
    }
    for pin in copies {
        if !hooks.before(&pin.key) {
            return stopped(report, pin.key, Why::Asked);
        }
        match delete_copy(pin, env, hooks) {
            Ok(done) => {
                hooks.after(&pin.key, &done);
                report.done.push((pin.key, done));
            }
            Err(why) => return stopped(report, pin.key, why),
        }
    }
    report
}

fn stopped(mut report: Report, key: ArtifactKey, why: Why) -> Report {
    report.stopped = Some((key, why));
    report
}

fn trash_temp(pin: &TempPin, env: &Env<'_>) -> Result<Done, Why> {
    // Again, now: the process may have started since it was looked at.
    match temp_state(pin, env) {
        TempState::Offerable => {}
        TempState::BeingWritten => return Err(Why::BeingWritten),
        TempState::NoTrash => return Err(Why::NoTrash),
    }
    let folder = match Folder::open(&pin.folder) {
        Opened::Folder(folder) => folder,
        Opened::Missing | Opened::Other => return Err(Why::Changed(pin.folder.clone().into_os_string())),
    };
    // Asked of the folder just opened, not of a path that may lead elsewhere now.
    if !folder.is_local() {
        return Err(Why::BeingWritten);
    }
    if folder.identity().map_err(|e| Why::Io(pin.name.clone(), e.kind()))? != pin.folder_id {
        return Err(Why::Changed(pin.folder.clone().into_os_string()));
    }
    match folder.look(&pin.name) {
        Ok(now) if now.identity() == pin.id => {}
        Ok(_) => return Err(Why::Changed(pin.name.clone())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Done::AlreadyGone),
        Err(error) => return Err(Why::Io(pin.name.clone(), error.kind())),
    }
    // The path the Trash is given leads, as the held folder is now placed, to the pinned thing.
    let at = folder.path().map_err(|e| Why::Io(pin.name.clone(), e.kind()))?.join(&pin.name);
    if identity_at(&at) != Some(pin.id) {
        return Err(Why::Changed(pin.name.clone()));
    }
    let to = env.trash.move_to_trash(&at).map_err(Why::Trash)?;
    if identity_at(&to) != Some(pin.id) || folder.look(&pin.name).is_ok() {
        return Err(Why::NotMoved(to));
    }
    folder.sync().map_err(|_| Why::NotDurable)?;
    Ok(Done::Trashed(to))
}

/// The order within a copy: what restoring reads first goes first. `.whole` before everything (a copy cut short without it is one the installer's
/// `restore_record` leaves alone), the record's slots and `places.log`, the rest, then `journals/` last. Only the order: a name not here is only later.
fn rank(depth: usize, name: &OsStr, kind: Kind) -> u8 {
    if depth > 0 {
        return 2;
    }
    match name.as_encoded_bytes() {
        b".whole" => 0,
        b"record.a.json" | b"record.b.json" | b"places.log" => 1,
        _ if kind == Kind::Folder => 3,
        _ => 2,
    }
}

fn delete_copy(pin: &CopyPin, env: &Env<'_>, hooks: &mut dyn Hooks) -> Result<Done, Why> {
    if copy_state(pin) != CopyState::Offerable {
        return Err(Why::WentBack);
    }
    if is_live(pin, env) {
        return Err(Why::LiveRecord);
    }
    let changed = |name: &OsStr| Why::Changed(name.to_os_string());
    let mut parent = match Folder::open(&pin.root) {
        Opened::Folder(folder) => folder,
        Opened::Missing | Opened::Other => return Err(changed(pin.root.as_os_str())),
    };
    if parent.identity().map_err(|e| Why::Io(pin.root.clone().into_os_string(), e.kind()))? != pin.root_id {
        return Err(changed(pin.root.as_os_str()));
    }
    for (name, id) in &pin.chain {
        parent = match parent.open_in(name) {
            Opened::Folder(next) if next.identity().ok() == Some(*id) => next,
            Opened::Missing => return Ok(Done::Deleted { unlinked: 0, already_gone: 1 }),
            _ => return Err(changed(name)),
        };
    }
    let mut walk = Walk {
        rank: &rank,
        gate: &mut |name| hooks.entry(name),
    };
    match remove_tree_in(&parent, &pin.name, &pin.tree, &mut walk) {
        Ok(removed) => {
            parent.sync().map_err(|_| Why::NotDurable)?;
            Ok(Done::Deleted {
                unlinked: removed.unlinked,
                already_gone: removed.already_gone,
            })
        }
        Err(Failure::Changed(name)) => Err(Why::Changed(name)),
        Err(Failure::Unpinned(name)) => Err(Why::Unpinned(name)),
        Err(Failure::TooDeep) => Err(Why::TooDeep),
        Err(Failure::Stopped(_)) => Err(Why::Asked),
        Err(Failure::NotDurable(_)) => Err(Why::NotDurable),
        Err(Failure::Io(name, error)) => Err(Why::Io(name, error.kind())),
    }
}
