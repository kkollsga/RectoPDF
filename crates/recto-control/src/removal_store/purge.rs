//! The pins of the history surveys, and the purge of one plan step by them (`docs/design/removal.md` §4 and §10 "D2e history as built"), for the six history
//! stores: `Backup`, `Journal`, `JournalVariant`, `KeptPages`, `PageIds` and `SleepFile`. **Not wired to any command, and takes no lease:** the walk, its
//! quiescing (`record_store::hold`), the barrier, the words and the progress record belong to the command that calls it.
//!
//! - **Pins.** [`HistoryPins`] is filled by `survey_journals_pinned` and `survey_backups_pinned` with the identity (device, inode, kind) of every artifact
//!   added as the target's, and of nothing else (a Save As source is never acted on, and any other owner refuses the plan). A file placed by its name alone
//!   is pinned from the listed entry; one placed by its bytes (a journal, a variant, a sidecar) from the entry too, which the survey proved is its opened
//!   descriptor's own device, inode and single link. Turning an identity into the barrier's `Pin` (a volume's position) is the carrier's.
//! - **The purge** ([`purge_history`]) acts on what was pinned and stops at the first thing that isn't it, by descriptor, as the leftovers' acts do.
//!   Nothing is opened, no link is followed, nothing is allocated past the capped listing and the capped wanted set, and no thread is started.
//!   Before the first unlink every file to go is checked (pass 1); each is looked at again just before its own unlink (pass 2). **The window between
//!   that look and the unlink is the name-level limit** PLAN tracks (D5 act follow-up (b)): a file swapped for another in it is unlinked by name. A file
//!   that is gone at pass 2 counts as already gone.

use super::backups;
use super::journals::{self, FLAVORS, MAX_ENTRIES};
use recto_core::removal::{Ack, ArtifactKey, DESIGN_LIMITS, Pin, PinKind, Refusal, Store};
use recto_platform::folder::{Entry, Folder, Identity, Kind, Opened};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[cfg(test)]
use super::testing::Hook;

/// The most pins held: the survey's own cap on artifacts (design limit).
const MAX_PINS: usize = 8_192;

/// What the history surveys saw of each artifact that is the target's: its device, inode and kind when it was listed. Never of another owner's.
#[derive(Debug, Default)]
pub struct HistoryPins {
    pins: BTreeMap<ArtifactKey, Identity>,
}

impl HistoryPins {
    pub fn get(&self, key: &ArtifactKey) -> Option<Identity> {
        self.pins.get(key).copied()
    }

    /// Pins a target's artifact. The 8,193rd is `TooLarge`, as the survey's own cap is.
    pub(super) fn note(&mut self, key: ArtifactKey, identity: Identity) -> Result<(), Refusal> {
        if self.pins.len() >= MAX_PINS && !self.pins.contains_key(&key) {
            return Err(Refusal::TooLarge);
        }
        self.pins.insert(key, identity);
        Ok(())
    }
}

/// Where the history stores are.
pub struct HistoryRoots<'a> {
    pub state_root: &'a Path,
    pub backups: &'a Path,
}

/// The state root's volume now: `root` is its position in the record's volume list, `dev` its `st_dev` at this launch (the carrier matched the UUID).
#[derive(Clone, Copy, Debug)]
pub struct VolumeNow {
    pub root: u8,
    pub dev: u64,
}

/// What a purge did. `already_gone` counts wanted files that weren't there: a step repeated after a kill between its act and its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Purged {
    pub removed: u32,
    pub already_gone: u32,
    /// Every folder that lost a name was synced.
    pub durable: bool,
}

/// Why a step stopped. Nothing was unlinked for any of the first four (and `Listing`, `FolderNotOurs`, `TooLarge`, `NotAFile`, `Linked` and `NotThePinned`
/// from pass 1); what an unlink before a stop in pass 2 removed stays gone, and a resume counts it as already gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PurgeStop {
    /// The store isn't one of the six.
    NotHistory,
    /// More files wanted than a step holds, or a folder with more entries than a survey reads.
    TooLarge,
    /// A pin that isn't a file's.
    BadPin,
    /// A pin for another volume than the state root's.
    OtherVolume,
    /// A folder, or something on the way to it, is a link, a file or another user's.
    FolderNotOurs,
    /// A folder couldn't be listed, or looked at, or an unlink failed.
    Listing,
    /// A wanted name is a folder, a link or something else than a file.
    NotAFile,
    /// A wanted file has another name.
    Linked,
    /// A wanted name leads to another file than the one pinned.
    NotThePinned,
    /// After the unlinks a wanted name was listed again.
    StillThere,
}

/// What a result is, as the walk acknowledges it.
pub fn ack(result: &Result<Purged, PurgeStop>) -> Ack {
    match result {
        Ok(purged) if purged.durable => Ack::Done,
        Ok(_) => Ack::DoneNotDurable,
        Err(_) => Ack::Failed,
    }
}

/// A test's way to change a folder under the purge, and to make a sync fail.
#[derive(Default)]
pub(super) struct Seams {
    /// Called with a folder's path once it is opened and before it is listed.
    #[cfg(test)]
    pub before_list: Option<Hook>,
    #[cfg(test)]
    pub fail_sync: bool,
}

/// One folder of the store: open by descriptor, with the place its keys start with and where it was opened (for a test's seams; never opened again).
struct Place {
    folder: Folder,
    prefix: Vec<u8>,
    #[cfg(test)]
    at: PathBuf,
}

/// One wanted file that was listed.
struct Match {
    place: usize,
    entry: Entry,
    pin: Pin,
}

const fn is_history(store: Store) -> bool {
    matches!(
        store,
        Store::Backup | Store::Journal | Store::JournalVariant | Store::KeptPages | Store::PageIds | Store::SleepFile
    )
}

/// Takes the files of one plan step out of the history stores: `wanted` are the keys the survey gave and the pins the barrier recorded for them.
///
/// In this order, nothing touched until pass 1 has passed: the store and the batch (`DESIGN_LIMITS.max_batch`), the pins (files, on this volume); the
/// folders the store lives in, opened as their survey opens them; a listing of each, capped, keyed as the survey keys; a wanted key not listed is already
/// gone; pass 1, every listed match is a plain file with one link and the pinned device and inode; pass 2, each looked at again just before its own
/// unlink; every folder that lost a name synced; a last listing that no wanted key is in.
pub fn purge_history(store: Store, wanted: &[(ArtifactKey, Pin)], roots: &HistoryRoots<'_>, volume: VolumeNow) -> Result<Purged, PurgeStop> {
    purge_with(store, wanted, roots, volume, &Seams::default())
}

pub(super) fn purge_with(store: Store, wanted: &[(ArtifactKey, Pin)], roots: &HistoryRoots<'_>, volume: VolumeNow, seams: &Seams) -> Result<Purged, PurgeStop> {
    #[cfg(not(test))]
    let _ = seams;
    if !is_history(store) {
        return Err(PurgeStop::NotHistory);
    }
    if wanted.len() > usize::from(DESIGN_LIMITS.max_batch.get()) {
        return Err(PurgeStop::TooLarge);
    }
    for (_, pin) in wanted {
        if pin.kind != PinKind::File {
            return Err(PurgeStop::BadPin);
        }
        if pin.root != volume.root {
            return Err(PurgeStop::OtherVolume);
        }
    }
    let want: BTreeMap<ArtifactKey, Pin> = wanted.iter().copied().collect();
    let places = open_places(store, roots)?;
    let mut matches = Vec::new();
    for (at, place) in places.iter().enumerate() {
        #[cfg(test)]
        if let Some(hook) = &seams.before_list {
            hook(&place.at);
        }
        list(store, place, |key, entry| {
            if let Some(pin) = want.get(&key) {
                matches.push(Match { place: at, entry, pin: *pin });
            }
        })?;
    }
    let mut already_gone = (want.len() - matches.len()) as u32;
    // Pass 1: nothing is unlinked unless every file to go is what was pinned.
    for found in &matches {
        if let Some(stop) = differs(&found.entry, &found.pin, volume) {
            return Err(stop);
        }
    }
    // Pass 2: each looked at again just before its own unlink.
    let mut removed = 0u32;
    let mut lost = vec![false; places.len()];
    for found in &matches {
        let folder = &places[found.place].folder;
        match folder.look(&found.entry.name) {
            Ok(now) => {
                if let Some(stop) = differs(&now, &found.pin, volume) {
                    return Err(stop);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                already_gone += 1;
                continue;
            }
            Err(_) => return Err(PurgeStop::Listing),
        }
        match folder.unlink_in(&found.entry.name, false) {
            Ok(()) => {
                removed += 1;
                lost[found.place] = true;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => already_gone += 1,
            Err(_) => return Err(PurgeStop::Listing),
        }
    }
    let mut durable = true;
    for (place, lost) in places.iter().zip(&lost) {
        if *lost && !synced(&place.folder, seams) {
            durable = false;
        }
    }
    let mut still_there = false;
    for place in &places {
        list(store, place, |key, _| still_there |= want.contains_key(&key))?;
    }
    if still_there {
        return Err(PurgeStop::StillThere);
    }
    Ok(Purged {
        removed,
        already_gone,
        durable,
    })
}

/// Why a listed or looked-at entry isn't the pinned file, if it isn't.
fn differs(entry: &Entry, pin: &Pin, volume: VolumeNow) -> Option<PurgeStop> {
    if entry.kind != Kind::File {
        return Some(PurgeStop::NotAFile);
    }
    if entry.links != 1 {
        return Some(PurgeStop::Linked);
    }
    (entry.dev != volume.dev || entry.ino != pin.inode).then_some(PurgeStop::NotThePinned)
}

fn synced(folder: &Folder, seams: &Seams) -> bool {
    #[cfg(test)]
    if seams.fail_sync {
        return false;
    }
    #[cfg(not(test))]
    let _ = seams;
    folder.sync().is_ok()
}

/// The folders `store` lives in, opened by the survey's own opening: a folder that isn't there holds nothing, and one that is a link, a file or another
/// user's is `FolderNotOurs`.
fn open_places(store: Store, roots: &HistoryRoots<'_>) -> Result<Vec<Place>, PurgeStop> {
    let mut places = Vec::new();
    let mut take = |opened: Opened, prefix: Vec<u8>, at: PathBuf| -> Result<(), PurgeStop> {
        #[cfg(not(test))]
        let _ = &at;
        match opened {
            Opened::Missing => Ok(()),
            Opened::Other => Err(PurgeStop::FolderNotOurs),
            Opened::Folder(folder) => {
                places.push(Place {
                    folder,
                    prefix,
                    #[cfg(test)]
                    at,
                });
                Ok(())
            }
        }
    };
    if store == Store::Backup {
        take(backups::open_backups(roots.backups), Vec::new(), roots.backups.to_path_buf())?;
    } else {
        for flavor in FLAVORS {
            take(
                journals::open_journals(roots.state_root, flavor),
                journals::flavor_prefix(flavor),
                roots.state_root.join(flavor).join("journals"),
            )?;
        }
    }
    Ok(places)
}

/// Every entry of the folder that is one of `store`'s, with its key: capped at the survey's 8,192 entries (one more is `TooLarge`).
fn list(store: Store, place: &Place, mut each: impl FnMut(ArtifactKey, Entry)) -> Result<(), PurgeStop> {
    let entries = place.folder.entries().map_err(|_| PurgeStop::Listing)?;
    let mut seen = 0usize;
    for entry in entries {
        let entry = entry.map_err(|_| PurgeStop::Listing)?;
        if seen >= MAX_ENTRIES {
            return Err(PurgeStop::TooLarge);
        }
        seen += 1;
        let key = if store == Store::Backup {
            backups::backup_key(entry.name.as_encoded_bytes())
        } else if journals::entry_store(&entry.name) == store {
            journals::journal_key(store, &place.prefix, &entry.name)
        } else {
            continue;
        };
        each(key, entry);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
