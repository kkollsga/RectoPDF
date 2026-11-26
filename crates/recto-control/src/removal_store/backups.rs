//! The original-backup folder (`Store::Backup`): which of its files are the target's, by name only. A backup is never opened or read.

use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Sides, Store, Survey, TargetMode};
use recto_core::save::backup_name;
use recto_platform::folder::{Folder, Opened};
use std::collections::HashMap;
use std::path::Path;

/// Lists `folder` and adds every file in it to `survey`, marking `Store::Backup` complete only after the whole listing.
///
/// A name made from a path in `lineage` (or its partial copy) is owned by the sides that path was seen on. Any other name, a directory or a
/// link is `Unknown` and refuses the plan; a file with other hard links is `Shared`; a file last changed before the ledger began is `Unknown`
/// (it was made before anything was recorded and can't be placed). A missing folder is complete (nothing was kept); a folder that is a
/// link, one that isn't this user's, or one that can't be read, is not marked, nor is any folder while the lineage is incomplete (a gap or a fork), so the plan
/// refuses rather than reads "none found". The folder is opened once without following a link and listed through that descriptor, so a path
/// swapped for a link after the check changes nothing. `add`'s refusal passes through.
pub fn survey_backups(folder: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey) -> Result<(), Refusal> {
    survey_backups_with(folder, lineage, mode, survey, &|_| {})
}

/// `survey_backups`, calling `before_list` with the folder's path once the folder is checked and before it is listed: a test's way to change the
/// path under the survey.
pub(crate) fn survey_backups_with(
    folder: &Path,
    lineage: &Lineage<'_>,
    mode: TargetMode,
    survey: &mut Survey,
    before_list: &dyn Fn(&Path),
) -> Result<(), Refusal> {
    let folder_path = folder;
    let folder = match Folder::open(folder_path) {
        Opened::Folder(folder) => folder,
        Opened::Missing => {
            if lineage.complete() {
                survey.mark_complete(Store::Backup);
            }
            return Ok(());
        }
        Opened::Other => return Ok(()),
    };
    let mut names: HashMap<Vec<u8>, Sides> = HashMap::new();
    for (path, side) in lineage.paths() {
        let name = backup_name(path);
        for name in [format!(".{name}.partial"), name] {
            let sides = names.entry(name.into_bytes()).or_default();
            *sides = sides.with(side);
        }
    }
    let began = lineage.began_ns();
    before_list(folder_path);
    let Ok(entries) = folder.entries() else { return Ok(()) };
    for entry in entries {
        let Ok(entry) = entry else { return Ok(()) };
        let name = entry.name.as_encoded_bytes();
        let finder = finder_file(name, entry.is_file, entry.links);
        let owner = match names.get(name) {
            _ if finder == Some(Finder::Skip) => continue,
            _ if let Some(Finder::Owner(owner)) = finder => owner,
            _ if !entry.is_file => Owner::Unknown,
            _ if entry.modified_ns.is_none_or(|t| t < began) => Owner::Unknown,
            None => Owner::Unknown,
            Some(_) if entry.links != 1 => Owner::Shared,
            Some(sides) => sides.owner(mode),
        };
        survey.add(Found {
            store: Store::Backup,
            key: ArtifactKey::of(Store::Backup, name),
            owner,
            format: Format::Current,
        })?;
    }
    if lineage.complete() {
        survey.mark_complete(Store::Backup);
    }
    Ok(())
}

/// What Finder's `.DS_Store` is in a folder a survey lists, for every folder it surveys: a plain file with one name is Finder's own and left out; a hard
/// link is a second way to reach another file, so `Shared`; a link, a folder or a pipe so named is `Unknown`. `None` for any other name.
pub(super) fn finder_file(name: &[u8], is_file: bool, links: u64) -> Option<Finder> {
    if name != b".DS_Store" {
        return None;
    }
    Some(match (is_file, links) {
        (true, 1) => Finder::Skip,
        (true, _) => Finder::Owner(Owner::Shared),
        (false, _) => Finder::Owner(Owner::Unknown),
    })
}

/// What a survey does with a `.DS_Store`.
#[derive(PartialEq)]
pub(super) enum Finder {
    Skip,
    Owner(Owner),
}

#[cfg(test)]
mod tests;
