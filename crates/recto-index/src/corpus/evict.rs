//! **Eviction and clearing:** what a corpus folder may hold, by age and size, what's removed on request, and the one walk over the folders every sweep goes by.

use super::{BuildLimits, CorpusKey, CorpusPaths, ROOM_ON_A_FULL_DISK, VERSIONS_FOLDER, disk_size, tidy_versions};
use recto_platform::local;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What eviction did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Evicted {
    pub corpora: u32,
    pub bytes: u64,
    pub kept_bytes: u64,
}

/// **The corpus folders under a root:** each folder named as a key, never a link, whatever it's called (a link is someone's trap), and nothing else. The one walk every sweep, eviction and clear goes by. An error is the root's: one that isn't there has none.
pub(crate) fn key_folders(root: &Path) -> Vec<(CorpusKey, PathBuf)> {
    try_key_folders(root).unwrap_or_default()
}

fn try_key_folders(root: &Path) -> io::Result<Vec<(CorpusKey, PathBuf)>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root)?.flatten() {
        if let Some(key) = entry.file_name().to_str().and_then(CorpusKey::from_hex)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            found.push((key, entry.path()));
        }
    }
    Ok(found)
}

/// Removes corpora past their age, then the least recently used until all of them fit. Never `keep`, and never one being built. Only folders named as a key are looked at: anything else under the root isn't ours to touch.
pub fn evict(root: &Path, limits: &BuildLimits, keep: Option<CorpusKey>) -> io::Result<Evicted> {
    let mut found = Vec::new();
    let now = SystemTime::now();
    for (key, dir) in try_key_folders(root)? {
        let used = fs::metadata(CorpusPaths::new(root, key).used)
            .or_else(|_| fs::metadata(&dir))
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        found.push((used, folder_bytes(&dir), key));
    }
    found.sort_by_key(|entry| entry.0);
    let mut total: u64 = found.iter().map(|entry| entry.1).sum();
    let mut report = Evicted::default();
    for (used, bytes, key) in found {
        let too_old = now.duration_since(used).is_ok_and(|age| age > limits.max_age);
        if (total <= limits.max_total_bytes && !too_old) || keep == Some(key) || remove_unless_building(root, key).is_err() {
            continue;
        }
        total -= bytes;
        report.corpora += 1;
        report.bytes += bytes;
    }
    report.kept_bytes = total;
    tidy_versions(root);
    Ok(report)
}

/// The disk is full, so caps don't come into it: this takes the least recently used, past however much is there now, until there's a good deal more room, or nothing else left to take. A corpus can always be built again, and a user's disk filling up is worse.
pub fn make_room(root: &Path, limits: &BuildLimits, keep: CorpusKey) -> io::Result<Evicted> {
    let mut total = 0;
    for (_, dir) in try_key_folders(root)? {
        total += folder_bytes(&dir);
    }
    let tighter = BuildLimits {
        max_total_bytes: total.saturating_sub(ROOM_ON_A_FULL_DISK.max(total / 4)),
        ..*limits
    };
    evict(root, &tighter, Some(keep))
}

/// Throws away one version of a file's corpus. Refused while it's being built.
pub fn clear(root: &Path, key: CorpusKey) -> io::Result<()> {
    remove_unless_building(root, key)
}

/// What a clear did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    pub cleared: u32,
    pub being_built: u32,
    /// These couldn't be removed, so their text is still there.
    pub failed: u32,
}

impl Cleared {
    pub fn note(&mut self, result: io::Result<()>) {
        match result {
            Ok(()) => self.cleared += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.being_built += 1,
            Err(_) => self.failed += 1,
        }
    }
}

/// Throws away everything that's not being built, and says what went and what didn't.
pub fn clear_all(root: &Path) -> io::Result<Cleared> {
    let mut report = Cleared::default();
    let folders = match try_key_folders(root) {
        Ok(folders) => folders,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(report),
        Err(error) => return Err(error),
    };
    for (key, _) in folders {
        report.note(remove_unless_building(root, key));
    }
    let _ = fs::remove_dir_all(root.join(VERSIONS_FOLDER));
    Ok(report)
}

/// Removes a corpus's folder unless a build holds its lock. A build only ever locks a plain file ([`local::append_private_file`]), so anything else at the lock's name, a link or a named pipe someone put there, holds nothing, and is never opened: opening a pipe to write would wait for a reader for ever (the second review's item 2, 2026-11-15). The open itself follows no link and doesn't wait, as a build's does, in case the name is changed between the look and the open.
pub(super) fn remove_unless_building(root: &Path, key: CorpusKey) -> io::Result<()> {
    let paths = CorpusPaths::new(root, key);
    let lock = match fs::symlink_metadata(&paths.lock) {
        Ok(meta) if meta.is_file() => Some(local::append_private_file(&paths.lock)?),
        Ok(_) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(lock) = &lock
        && lock.try_lock().is_err()
    {
        return Err(io::ErrorKind::WouldBlock.into());
    }
    // `remove_dir_all` doesn't follow links.
    fs::remove_dir_all(root.join(key.hex()))
}

/// What a folder takes up, near enough. Doesn't follow links, and stops at a depth no corpus reaches.
pub fn folder_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![(dir.to_path_buf(), 0u8)];
    while let Some((folder, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() && depth < 8 {
                stack.push((entry.path(), depth + 1));
            } else if kind.is_file()
                && let Ok(meta) = entry.metadata()
            {
                total += disk_size(meta.len());
            }
        }
    }
    total
}
