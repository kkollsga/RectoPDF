//! The writer's temporary files (`Store::WriterTemp`): a crashed save's whole copy of the document, left beside it in the person's own folder
//! (`docs/design/removal.md` §3, §10 "The writer's temporaries: the plainly safe rule").
//!
//! **Read-only, and nothing in the person's folder is opened.** The folder of each path the lineage puts on the target's side (an `Unplaced` path
//! included), and of **every** Save As or Replace destination ever recorded (`Lineage::destinations`, which no other survey reads; a later landing doesn't
//! drop it), is opened once by its descriptor (no link followed at its last part, this user's) and only listed; each entry is looked at by `fstatat`
//! without following a link. The naming rule is the writer's own (`recto_core::save::writer_temp_shape`).
//!
//! **What an entry is.** Not lopdf's exact shape: the person's own file, ignored (naming it `Unknown` would refuse every removal from a busy folder).
//! Exact shape: `Unknown`, whatever its hash (target's, another lineage's or none), its time, kind or links. lopdf sweeps only a temp at least an hour old
//! whose owner is dead, so a later save landing proves nothing about an earlier one's leftover, and no rule about hashes or times can say whose a temp is
//! safely. The cost is a false refusal: any leftover lopdf temp in a listed folder refuses removal until it is gone. **Known limit (a false "none found"):**
//! a Save As started by a build from before the destination was recorded isn't in the ledger, so its temp in a folder nothing else lists reads "none found".
//! A folder that is missing when listed is complete and empty: a purge must list again before it deletes (a remounted volume).

use super::leftovers::pin::Pins;
#[cfg(test)]
use super::testing::Hook;
use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Side, Store, Survey};
use recto_core::save::writer_temp_shape;
use recto_platform::folder::{Folder, Opened};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The most entries looked at in one folder: lopdf's own sweep bound, so a temp past it can't be swept either. Design limit, not a measurement.
const MAX_ENTRIES: usize = 65_536;
/// The most folders listed. Design limit, not a measurement.
const MAX_FOLDERS: usize = 64;

/// The caps, so a test can make them small.
#[derive(Clone)]
pub(crate) struct TempCaps {
    pub entries: usize,
    pub folders: usize,
    /// The Leftovers mode: each temp is pinned (its folder's and its own identity), and is an artifact only if its process has ended and its volume has a Trash.
    /// `None` is the Removal survey: every exact-shape temp is `Unknown`.
    pub pins: Option<Pins>,
    /// Called with a folder's path once it is opened and before it is listed: a test's way to change the path under the survey.
    #[cfg(test)]
    pub before_list: Option<Hook>,
}

impl Default for TempCaps {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            folders: MAX_FOLDERS,
            pins: None,
            #[cfg(test)]
            before_list: None,
        }
    }
}

/// Lists the folder of each target-side path of `lineage` and adds every writer temporary in it to `survey`, marking `Store::WriterTemp` complete
/// only after every folder was listed to the end.
///
/// A folder that isn't there is complete and empty and counts toward no cap; one that isn't a real folder of this user's (a link, a file, another user's), one that can't be
/// listed or an entry that can't be looked at leaves the store unmarked, so the plan says `Incomplete` rather than "none found". The adapter does
/// not check the lineage's gap: the plan's `HistoryIncomplete` gate (`Survey::history`) does. More than 64 folders that open, or 65,536 entries in one, is
/// `TooLarge`. `add`'s refusal passes through.
pub fn survey_writer_temps(lineage: &Lineage<'_>, survey: &mut Survey) -> Result<(), Refusal> {
    survey_writer_temps_with(lineage, survey, &TempCaps::default())
}

pub(crate) fn survey_writer_temps_with(lineage: &Lineage<'_>, survey: &mut Survey, caps: &TempCaps) -> Result<(), Refusal> {
    let folders = folders_of(lineage);
    // Only a folder that opens counts toward the cap: one that isn't there holds no file, so no temp can be in it (a Save As to a folder made later, or
    // removed since, costs nothing), and the cap is on what is listed, which is the work.
    let mut opened = 0usize;
    for path in &folders {
        let folder = match Folder::open(path) {
            Opened::Missing => continue,
            Opened::Other => return Ok(()),
            Opened::Folder(folder) => folder,
        };
        opened += 1;
        if opened > caps.folders {
            return Err(Refusal::TooLarge);
        }
        #[cfg(test)]
        if let Some(hook) = &caps.before_list {
            hook(path);
        }
        let folder_id = match &caps.pins {
            Some(_) => match folder.identity() {
                Ok(id) => Some(id),
                Err(_) => return Ok(()),
            },
            None => None,
        };
        let Ok(entries) = folder.entries() else { return Ok(()) };
        let mut seen = 0usize;
        for entry in entries {
            let Ok(entry) = entry else { return Ok(()) };
            seen += 1;
            if seen > caps.entries {
                return Err(Refusal::TooLarge);
            }
            // Exact shape and nothing else: whose it is can't be told safely.
            if !writer_temp_shape(entry.name.as_encoded_bytes()) {
                continue;
            }
            let mut key = path.as_os_str().as_encoded_bytes().to_vec();
            key.push(b'/');
            key.extend_from_slice(entry.name.as_encoded_bytes());
            let key = ArtifactKey::of(Store::WriterTemp, &key);
            // In the Leftovers survey a temp whose process has ended and whose volume has a Trash is clearable; any other is left out, and the pins say why.
            let owner = match (&caps.pins, folder_id) {
                (Some(pins), Some(folder_id)) => match pins.temp(path, folder_id, &entry, key) {
                    Some(owner) => owner,
                    None => continue,
                },
                _ => Owner::Unknown,
            };
            survey.add(Found {
                store: Store::WriterTemp,
                key,
                owner,
                format: Format::Current,
            })?;
        }
    }
    survey.mark_complete(Store::WriterTemp);
    Ok(())
}

/// The folders to list, each once: those of the target-side paths and of every destination ever recorded.
fn folders_of(lineage: &Lineage<'_>) -> Vec<PathBuf> {
    let mut seen: HashSet<&Path> = HashSet::new();
    let mut folders = Vec::new();
    let targets = lineage.paths().filter(|(_, side)| *side == Side::Target).map(|(path, _)| path);
    for path in targets.chain(lineage.destinations()) {
        let Some(parent) = path.parent() else { continue };
        // The writer's own rule for a path with no folder part.
        let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        if seen.insert(parent) {
            folders.push(parent.to_path_buf());
        }
    }
    folders
}

#[cfg(test)]
mod tests;
