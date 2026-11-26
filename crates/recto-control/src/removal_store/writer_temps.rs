//! The writer's temporary files (`Store::WriterTemp`): a crashed save's whole copy of the document, left beside it in the person's own folder
//! (`docs/design/removal.md` §3, §10 "D2c-2b as built").
//!
//! **Read-only, and nothing in the person's folder is opened.** The folder of each path the lineage puts on the target's side (an `Unplaced` path
//! included) is opened once by its descriptor (no link followed at its last part, this user's) and only listed; each entry is looked at by `fstatat`
//! without following a link. The naming rule is the writer's own (`recto_core::save::writer_temp_shape`).
//!
//! **What an entry is.** Not lopdf's exact shape: the person's own file, ignored (naming it `Unknown` would refuse every removal from a busy
//! folder). Exact shape and a hash that is a lineage path's file name in this folder: placed by that path's sides, as a backup is. Exact shape and
//! a hash that is no lineage path's: ignored if it changed after the ledger began, `Unknown` if it changed before or has no time. The
//! time rule holds because Save, Save As and Replace all record the path the writer names its temp for at `Starting`, before the writer makes anything (a
//! destination as `Unplaced`, by its canonical folder), or leave a gap that refuses; so a temp that appeared after the ledger began and matches no path is
//! no save's of this app. **Known limits (the first a false "none found", the second a false refusal):** a Save As started by a build from before this
//! recording isn't in the ledger, so its temp reads "none found" until it's swept; and a Save As that crashes just after its rename leaves its `.tmp` as a
//! hard link to the target, which reads `Shared` (a false refusal) until lopdf's `clear_leftover_links` runs. A placed `.tmp` must be a plain file (`Shared` with other hard links); a `.d` must
//! be a real folder and is not looked into. Any other kind, or one changed before the ledger began, is `Unknown`.

#[cfg(test)]
use super::testing::Hook;
use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Side, Sides, Store, Survey, TargetMode};
use recto_core::save::{TempShape, writer_temp_prefix, writer_temp_shape};
use recto_platform::folder::{Entry, Folder, Opened};
use std::collections::HashMap;
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
    /// Called with a folder's path once it is opened and before it is listed: a test's way to change the path under the survey.
    #[cfg(test)]
    pub before_list: Option<Hook>,
}

impl Default for TempCaps {
    fn default() -> Self {
        Self {
            entries: MAX_ENTRIES,
            folders: MAX_FOLDERS,
            #[cfg(test)]
            before_list: None,
        }
    }
}

/// Lists the folder of each target-side path of `lineage` and adds every writer temporary in it to `survey`, marking `Store::WriterTemp` complete
/// only after every folder was listed to the end.
///
/// A folder that isn't there is complete and empty; one that isn't a real folder of this user's (a link, a file, another user's), one that can't be
/// listed or an entry that can't be looked at leaves the store unmarked, so the plan says `Incomplete` rather than "none found". The adapter does
/// not check the lineage's gap: the plan's `HistoryIncomplete` gate (`Survey::history`) does. More than 64 folders, or 65,536 entries in one, is
/// `TooLarge`. `add`'s refusal passes through.
pub fn survey_writer_temps(lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey) -> Result<(), Refusal> {
    survey_writer_temps_with(lineage, mode, survey, &TempCaps::default())
}

pub(crate) fn survey_writer_temps_with(lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey, caps: &TempCaps) -> Result<(), Refusal> {
    let folders = folders_of(lineage);
    if folders.iter().filter(|(_, hashes)| hashes.listed).count() > caps.folders {
        return Err(Refusal::TooLarge);
    }
    let began = lineage.began_ns();
    for (path, hashes) in folders.iter().filter(|(_, hashes)| hashes.listed) {
        let folder = match Folder::open(path) {
            Opened::Missing => continue,
            Opened::Other => return Ok(()),
            Opened::Folder(folder) => folder,
        };
        #[cfg(test)]
        if let Some(hook) = &caps.before_list {
            hook(path);
        }
        let Ok(entries) = folder.entries() else { return Ok(()) };
        let mut seen = 0usize;
        for entry in entries {
            let Ok(entry) = entry else { return Ok(()) };
            seen += 1;
            if seen > caps.entries {
                return Err(Refusal::TooLarge);
            }
            let Some(owner) = owner_of(&entry, &hashes.sides, began, mode) else {
                continue;
            };
            let mut key = path.as_os_str().as_encoded_bytes().to_vec();
            key.push(b'/');
            key.extend_from_slice(entry.name.as_encoded_bytes());
            survey.add(Found {
                store: Store::WriterTemp,
                key: ArtifactKey::of(Store::WriterTemp, &key),
                owner,
                format: Format::Current,
            })?;
        }
    }
    survey.mark_complete(Store::WriterTemp);
    Ok(())
}

/// What the lineage says about one folder.
#[derive(Default)]
struct Hashes {
    /// Whether any path of this folder is on the target's side: only then is the folder listed.
    listed: bool,
    /// By the 16 hex of a path's file name: the sides that path was seen on.
    sides: HashMap<String, Sides>,
}

/// Each folder that holds a lineage path, with the hashes of the file names in it. Each folder once, whatever the number of paths in it.
fn folders_of(lineage: &Lineage<'_>) -> Vec<(PathBuf, Hashes)> {
    let mut folders: Vec<(PathBuf, Hashes)> = Vec::new();
    let mut at: HashMap<PathBuf, usize> = HashMap::new();
    for (path, side) in lineage.paths() {
        let Some(hashes) = hashes_of(&mut folders, &mut at, path) else { continue };
        hashes.listed |= side == Side::Target;
        let sides = hashes.sides.entry(hash_of(path)).or_default();
        *sides = sides.with(side);
    }
    // A save that was going to a path and never landed may have left its temp there, named for it: the folder is listed, and that temp is no one's to place.
    for path in lineage.destinations() {
        let Some(hashes) = hashes_of(&mut folders, &mut at, path) else { continue };
        hashes.listed = true;
        let sides = hashes.sides.entry(hash_of(path)).or_default();
        *sides = sides.with(Side::Target).with(Side::Other);
    }
    folders
}

/// The record of the folder `path` is in, made if it is the first of that folder.
fn hashes_of<'a>(folders: &'a mut Vec<(PathBuf, Hashes)>, at: &mut HashMap<PathBuf, usize>, path: &Path) -> Option<&'a mut Hashes> {
    let parent = path.parent()?;
    // The writer's own rule for a path with no folder part.
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    let index = *at.entry(parent.to_path_buf()).or_insert_with(|| {
        folders.push((parent.to_path_buf(), Hashes::default()));
        folders.len() - 1
    });
    Some(&mut folders[index].1)
}

/// The 16 hex of a path's file name that its temporaries carry (`.append-` and the `-` after them frame it).
fn hash_of(path: &Path) -> String {
    let prefix = writer_temp_prefix(path);
    prefix[".append-".len()..prefix.len() - 1].to_string()
}

/// What an entry of the person's folder is, or `None` if it isn't the survey's business.
fn owner_of(entry: &Entry, sides: &HashMap<String, Sides>, began: i64, mode: TargetMode) -> Option<Owner> {
    let (hash, shape) = writer_temp_shape(entry.name.as_encoded_bytes())?;
    let before_ledger = entry.modified_ns.is_none_or(|time| time < began);
    let Some(sides) = sides.get(hash) else {
        // A temp of some save the ledger saw nothing of: from before it began, nobody can say whose; from after, every save recorded its path first.
        return before_ledger.then_some(Owner::Unknown);
    };
    let right_kind = match shape {
        TempShape::File => entry.is_file,
        TempShape::Folder => entry.is_dir,
    };
    Some(match () {
        _ if !right_kind || before_ledger => Owner::Unknown,
        _ if shape == TempShape::File && entry.links != 1 => Owner::Shared,
        _ => sides.owner(mode),
    })
}

#[cfg(test)]
mod tests;
