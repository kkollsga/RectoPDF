//! **The one backup of an original** a save keeps, and the pruning of backups at a session's start.

use super::{BACKUP_DAYS, MAX_BACKUP_BYTES};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// **The one backup of an original, made before its first save over itself** (owner, 2026-11-08). An APFS clone where it can be, which costs no space until the original's blocks are gone, or else a copy, synced. Named for the file's real path, so a later save of the same file finds it and makes none: it's the original, not the last version. Says where it went, or `None` if there already was one.
pub fn keep_backup(original: &Path, folder: &Path) -> std::io::Result<Option<PathBuf>> {
    keep_backup_by(original, folder, true)
}

/// [`keep_backup`], with `clone` saying whether an APFS clone is tried first: `false` is the copy that follows a failed clone, for a test to reach.
fn keep_backup_by(original: &Path, folder: &Path, clone: bool) -> std::io::Result<Option<PathBuf>> {
    recto_platform::local::ensure_private_dir(folder)?;
    let _ = recto_platform::local::exclude_from_backups(folder);
    let real = original.canonicalize()?;
    let target = folder.join(backup_name(&real));
    if target.exists() {
        return Ok(None);
    }
    let partial = folder.join(format!(".{}.partial", backup_name(&real)));
    let _ = std::fs::remove_file(&partial);
    if !clone || recto_platform::local::clone_file(&real, &partial).is_err() {
        std::fs::copy(&real, &partial)?;
    }
    // **A backup's age is counted from when it was made.** A clone keeps the original's modification time, and the pruning at a session's start ages
    // backups by theirs, so a file last changed 40 days ago would be backed up and the backup removed at the next launch: the only copy of the file as it was
    // before this app wrote to it. Set before the rename, so no backup is ever seen under its name with the old time. If it can't be set the backup is kept
    // as it is (its age then counts from the original's, as it did): losing the save or the backup for a time stamp would be worse. The original's own time
    // isn't kept anywhere: nothing reads it from a backup.
    // Private first: a clone of a read-only original is read-only too, and opening it to write needs this. If it still won't open for writing, the backup is
    // kept with the time it has: it is only flushed.
    let _ = recto_platform::local::make_private_file(&partial);
    match std::fs::File::options().write(true).open(&partial) {
        Ok(backup) => {
            let _ = backup.set_modified(SystemTime::now());
            backup.sync_all()?;
        }
        Err(_) => std::fs::File::open(&partial)?.sync_all()?,
    }
    std::fs::rename(&partial, &target)?;
    let _ = recto_platform::local::sync_folder_of(&target);
    Ok(Some(target))
}

/// A backup's name: a hash of the path it's of, so the name says nothing of the file, with `.pdf` so Quick Look shows it.
pub fn backup_name(real: &Path) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(real.as_os_str().as_encoded_bytes());
    let mut name = String::from("original-");
    for byte in &digest[..12] {
        name.push_str(&format!("{byte:02x}"));
    }
    name.push_str(".pdf");
    name
}

/// The backup of this file, if there is one.
pub fn backup_of(original: &Path, folder: &Path) -> Option<PathBuf> {
    let found = folder.join(backup_name(&original.canonicalize().ok()?));
    found.is_file().then_some(found)
}

/// Backups past [`BACKUP_DAYS`] go, then the oldest until what's left is within [`MAX_BACKUP_BYTES`]. Says how many went. Run when a session starts, never on a timer.
pub fn prune_backups(folder: &Path, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else { return 0 };
    let mut found: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .take(65_536)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("original-") && name.ends_with(".pdf"))
        })
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file()
                .then(|| (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), entry.path()))
        })
        .collect();
    found.sort();
    let mut gone = 0;
    let mut total: u64 = found.iter().map(|(_, len, _)| *len).sum();
    let limit = Duration::from_secs(BACKUP_DAYS * 24 * 60 * 60);
    for (made, len, path) in found {
        let old = now.duration_since(made).is_ok_and(|age| age > limit);
        if (old || total > MAX_BACKUP_BYTES) && std::fs::remove_file(&path).is_ok() {
            total -= len;
            gone += 1;
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a backup of a file last changed long ago keeps that old time (a clone and a copy both do), and the pruning at the next launch ages backups by
    /// their time, so the only copy of the file as it was before RectoPDF wrote to it is removed at once. A backup's age counts from when it was made, by the
    /// clone and by the copy that follows a failed clone, and it is still there when the session after it starts.
    #[test]
    fn a_backup_of_an_old_file_is_aged_from_when_it_was_made() {
        for clone in [true, false] {
            let dir = std::env::temp_dir().join(format!("recto-backup-age-{}-{clone}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let original = dir.join("old.pdf");
            std::fs::write(&original, b"%PDF-1.4 as it was").unwrap();
            let long_ago = SystemTime::now() - Duration::from_secs((BACKUP_DAYS * 3) * 24 * 60 * 60);
            std::fs::File::options().write(true).open(&original).unwrap().set_modified(long_ago).unwrap();
            let folder = dir.join("backups");
            let kept = keep_backup_by(&original, &folder, clone).unwrap().expect("a backup");
            assert_eq!(std::fs::read(&kept).unwrap(), b"%PDF-1.4 as it was", "clone {clone}");
            assert_eq!(
                prune_backups(&folder, SystemTime::now()),
                0,
                "clone {clone}: a backup made now is not past its age"
            );
            assert!(kept.exists(), "clone {clone}");
            let later = SystemTime::now() + Duration::from_secs((BACKUP_DAYS + 1) * 24 * 60 * 60);
            assert_eq!(prune_backups(&folder, later), 1, "clone {clone}: and goes 30 days after it was made");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
