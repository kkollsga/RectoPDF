//! What each leftover was when the surveys listed it, so a later act can prove it acts on the same thing (`docs/design/removal.md` §10, D5).
//!
//! The writer-temp and copy surveys hold an optional [`Pins`] in their caps. When it is there, each exact-shape temp is pinned (its folder's device and
//! inode, its own device, inode and kind, its name, and the process number in it) and each copy folder is pinned whole (the root, each step to it, and every
//! entry below it down to [`MAX_DEPTH`] folders, by `fstatat` on the descriptor the survey itself opened), and each copy folder is also one artifact of its
//! store, so a plan can say "2 copies". With no `Pins` the surveys do what they did before. A copy that can't be pinned (too deep, too many entries, a folder
//! swapped while it was pinned) is left out of the pins, and an act on it is refused.
//!
//! Only this module decides what is offerable: a temp whose process may still be running, or on a volume with no Trash, and a copy holding `went-back`
//! (work of an earlier version not yet brought in) are listed by [`PinSet`] and refused by the act.

use crate::record_store::WENT_BACK_NAME;
use recto_core::removal::{ArtifactKey, Store};
use recto_platform::folder::{Entry, Folder, Identity, Kind, MAX_DEPTH, Opened, Pinned, Tree};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// A crashed save's temporary, as it was listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempPin {
    pub key: ArtifactKey,
    /// Where its folder was when listed: to open it again by its path, and never to act by.
    pub folder: PathBuf,
    pub folder_id: Identity,
    pub name: OsString,
    pub id: Identity,
    /// The process number in its name. `None` when the name's number is no process number: the temp is then taken as still being written.
    pub pid: Option<u32>,
}

/// A copy of the session record, whole, as it was listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyPin {
    pub key: ArtifactKey,
    pub store: Store,
    /// The state root, and what it was.
    pub root: PathBuf,
    pub root_id: Identity,
    /// The folders from the root to the copy's own parent (none for a session copy, `update` for the update's), each with what it was.
    pub chain: Vec<(OsString, Identity)>,
    /// The copy folder's name in its parent.
    pub name: OsString,
    pub tree: Tree,
    /// It holds `went-back`: an earlier version worked in it after going back, and its work has not been brought forward.
    pub went_back: bool,
}

/// Everything the surveys pinned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PinSet {
    pub temps: Vec<TempPin>,
    pub copies: Vec<CopyPin>,
}

impl PinSet {
    pub fn temp(&self, key: &ArtifactKey) -> Option<&TempPin> {
        self.temps.iter().find(|pin| pin.key == *key)
    }

    pub fn copy(&self, key: &ArtifactKey) -> Option<&CopyPin> {
        self.copies.iter().find(|pin| pin.key == *key)
    }
}

/// The sink a survey's caps carry. Cheap to clone; the surveys fill it, and [`Pins::set`] takes what they pinned.
#[derive(Clone, Default)]
pub struct Pins(Arc<Mutex<PinSet>>);

impl Pins {
    pub fn new() -> Self {
        Self::default()
    }

    /// What has been pinned so far.
    pub fn set(&self) -> PinSet {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Pins one temporary of `folder` (open as `at`, of identity `folder_id`) that the survey listed as `entry`.
    pub(in crate::removal_store) fn temp(&self, at: &Path, folder_id: Identity, entry: &Entry, key: ArtifactKey) {
        let pin = TempPin {
            key,
            folder: at.to_path_buf(),
            folder_id,
            name: entry.name.clone(),
            id: entry.identity(),
            pid: temp_pid(entry.name.as_encoded_bytes()),
        };
        self.0.lock().unwrap_or_else(PoisonError::into_inner).temps.push(pin);
    }

    /// Pins the copy folder `name`, which the survey holds open as `copy`, with everything in it. A copy that can't be pinned is not recorded.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::removal_store) fn copy(
        &self,
        store: Store,
        key: ArtifactKey,
        root: &Path,
        root_id: Identity,
        chain: Vec<(OsString, Identity)>,
        name: &OsStr,
        copy: &Folder,
        max_entries: usize,
    ) {
        let Some(tree) = tree_of(copy, 0, max_entries) else { return };
        let went_back = store == Store::LegacySession
            && tree
                .entries
                .iter()
                .any(|pinned| pinned.id.kind == Kind::File && pinned.name.as_encoded_bytes() == WENT_BACK_NAME.as_bytes());
        let pin = CopyPin {
            key,
            store,
            root: root.to_path_buf(),
            root_id,
            chain,
            name: name.to_os_string(),
            tree,
            went_back,
        };
        self.0.lock().unwrap_or_else(PoisonError::into_inner).copies.push(pin);
    }
}

/// `folder` and everything in it, `depth` folders below the copy's own. `None` when it is deeper than [`MAX_DEPTH`], holds more than `max_entries` in
/// one folder, can't be listed, or a folder was swapped between its listing and its opening.
fn tree_of(folder: &Folder, depth: usize, max_entries: usize) -> Option<Tree> {
    if depth > MAX_DEPTH {
        return None;
    }
    let id = folder.identity().ok()?;
    let mut entries = Vec::new();
    for entry in folder.entries().ok()? {
        let entry = entry.ok()?;
        if entries.len() >= max_entries {
            return None;
        }
        let below = if entry.kind == Kind::Folder {
            let Opened::Folder(inner) = folder.open_in(&entry.name) else { return None };
            if inner.identity().ok()? != entry.identity() {
                return None;
            }
            Some(tree_of(&inner, depth + 1, max_entries)?)
        } else {
            None
        };
        entries.push(Pinned {
            id: entry.identity(),
            name: entry.name,
            below,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Some(Tree { id, entries })
}

/// The process number in a writer temporary's name, `.append-<16 hex>-<pid>-<attempt>.tmp` or `.d`; `None` for any other shape or a number past `u32`.
///
/// Restates the second number of `recto_core::save::writer_temp_shape`'s grammar, which says only whether a name has the shape.
pub(crate) fn temp_pid(name: &[u8]) -> Option<u32> {
    if !recto_core::save::writer_temp_shape(name) {
        return None;
    }
    let rest = name.strip_prefix(b".append-")?;
    let rest = rest.strip_suffix(b".tmp").or_else(|| rest.strip_suffix(b".d"))?;
    let mut parts = rest.get(17..)?.split(|b| *b == b'-');
    std::str::from_utf8(parts.next()?).ok()?.parse().ok()
}
