//! The original-backup folder (`Store::Backup`): which of its files are the target's, by name only. A backup is never opened or read.

use recto_core::removal::{ArtifactKey, Format, Found, Lineage, Owner, Refusal, Sides, Store, Survey, TargetMode};
use recto_core::save::backup_name;
use std::collections::HashMap;
use std::path::Path;

/// Lists `folder` and adds every file in it to `survey`, marking `Store::Backup` complete only after the whole listing.
///
/// A name made from a path in `lineage` (or its partial copy) is owned by the sides that path was seen on. Any other name, a directory or a
/// link is `Unknown` and refuses the plan; a file with other hard links is `Shared`; a file last changed before the ledger began is `Unknown`
/// (it was made before anything was recorded and can't be placed). A missing folder is complete (nothing was kept); a folder that is a
/// link, or one that can't be read, is not marked, nor is any folder while the lineage is incomplete (a gap or a fork), so the plan
/// refuses rather than reads "none found". `add`'s refusal passes through.
pub fn survey_backups(folder: &Path, lineage: &Lineage<'_>, mode: TargetMode, survey: &mut Survey) -> Result<(), Refusal> {
    match std::fs::symlink_metadata(folder) {
        Ok(meta) if meta.is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if lineage.complete() {
                survey.mark_complete(Store::Backup);
            }
            return Ok(());
        }
        _ => return Ok(()),
    }
    let mut names: HashMap<Vec<u8>, Sides> = HashMap::new();
    for (path, side) in lineage.paths() {
        let name = backup_name(path);
        for name in [format!(".{name}.partial"), name] {
            let sides = names.entry(name.into_bytes()).or_default();
            *sides = sides.with(side);
        }
    }
    let began = lineage.began_ns();
    let Ok(entries) = std::fs::read_dir(folder) else { return Ok(()) };
    for entry in entries {
        let Ok(entry) = entry else { return Ok(()) };
        let name = entry.file_name();
        let name = name.as_encoded_bytes();
        let Ok(meta) = entry.metadata() else { return Ok(()) };
        if name == b".DS_Store" && meta.is_file() && links(&meta) == 1 {
            continue;
        }
        let owner = match names.get(name) {
            _ if !meta.is_file() => Owner::Unknown,
            // Only a hard-linked one gets here: a plain `.DS_Store` was skipped above.
            _ if name == b".DS_Store" => Owner::Shared,
            _ if modified_ns(&meta).is_none_or(|t| t < began) => Owner::Unknown,
            None => Owner::Unknown,
            Some(_) if links(&meta) != 1 => Owner::Shared,
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

/// When the file was last changed, in nanoseconds since 1970; `None` if the system doesn't say.
fn modified_ns(meta: &std::fs::Metadata) -> Option<i64> {
    let since = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_nanos()).ok()
}

/// Hard links to the file. Off Unix it is not known, so a file reads as shared.
pub(super) fn links(meta: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        std::os::unix::fs::MetadataExt::nlink(meta)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        2
    }
}

#[cfg(test)]
mod tests;
