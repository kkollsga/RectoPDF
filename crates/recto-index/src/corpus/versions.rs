//! Which versions of a file there is a corpus for (`by-file/`): noted before a page is written, and settled when a build finishes, so a new version takes the old one's text away.

use super::{CorpusKey, remove_unless_building};
use recto_platform::local;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::Path;

/// Where a file's last key is noted. Not named as a key, so nothing takes it for a corpus.
pub(super) const VERSIONS_FOLDER: &str = "by-file";
/// Most versions noted for one file. Only builds that stop partway leave more than one or two.
pub(super) const MAX_KEYS_PER_ALIAS: usize = 32;
/// What a file's versions are found by. None of them says what the file is.
fn file_aliases(path: &Path) -> Vec<String> {
    let hash_of = |kind: &str, bytes: &[u8]| {
        let mut hash = Sha256::new();
        hash.update(kind.as_bytes());
        hash.update([0]);
        hash.update(bytes);
        CorpusKey(hash.finalize().into()).hex()
    };
    let mut aliases = vec![hash_of("path", path.as_os_str().as_encoded_bytes())];
    if let Ok(real) = fs::canonicalize(path) {
        let real = hash_of("path", real.as_os_str().as_encoded_bytes());
        if !aliases.contains(&real) {
            aliases.push(real);
        }
    }
    if let Some((device, inode)) = recto_platform::file_device_and_inode(path) {
        aliases.push(hash_of("inode", &[device.to_le_bytes(), inode.to_le_bytes()].concat()));
    }
    aliases
}

/// The keys a file has had that may still have text on disk, oldest first. Anything that isn't a key is left out.
fn read_alias(path: &Path) -> Vec<CorpusKey> {
    let Ok(Some(bytes)) = local::read_private_file(path, MAX_KEYS_PER_ALIAS as u64 * 65) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&bytes).lines().filter_map(CorpusKey::from_hex).collect()
}

fn write_alias(path: &Path, keys: &[CorpusKey]) -> io::Result<()> {
    if keys.is_empty() {
        return match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let text: Vec<String> = keys.iter().map(CorpusKey::hex).collect();
    local::write_private_file_unsynced(path, text.join("\n").as_bytes())
}

/// Notes a key that's about to have text on disk. If it can't be noted, nothing should be written.
pub(super) fn note_starting(root: &Path, file: &Path, key: CorpusKey) -> io::Result<()> {
    let folder = root.join(VERSIONS_FOLDER);
    local::ensure_private_dir(&folder)?;
    for alias in file_aliases(file) {
        let path = folder.join(alias);
        let mut keys = read_alias(&path);
        if keys.contains(&key) {
            continue;
        }
        keys.push(key);
        // A file that keeps changing under builds never stopped: the oldest of its partial text goes, so this can't grow for ever.
        while keys.len() > MAX_KEYS_PER_ALIAS {
            let _ = remove_unless_building(root, keys.remove(0));
        }
        write_alias(&path, &keys)?;
    }
    Ok(())
}

/// This key is what the file has now, so what it had goes. A key stays noted if its text couldn't be removed, so it's still found next time, and by a clear.
pub(super) fn note_finished(root: &Path, file: &Path, key: CorpusKey) -> io::Result<()> {
    let folder = root.join(VERSIONS_FOLDER);
    for alias in file_aliases(file) {
        let path = folder.join(alias);
        let mut keys = read_alias(&path);
        keys.retain(|old| *old == key || matches!(remove_unless_building(root, *old), Err(error) if error.kind() != io::ErrorKind::NotFound));
        if !keys.contains(&key) {
            keys.push(key);
        }
        write_alias(&path, &keys)?;
    }
    Ok(())
}

/// Every version of a file there's a corpus for, as far as can be told, even if the file itself has gone. In no order, and each once.
pub fn keys_of_file(root: &Path, path: &Path) -> Vec<CorpusKey> {
    let folder = root.join(VERSIONS_FOLDER);
    let mut keys: Vec<CorpusKey> = CorpusKey::of_file(path).into_iter().collect();
    for alias in file_aliases(path) {
        for key in read_alias(&folder.join(alias)) {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

/// Forgets which versions a file had. They've been thrown away.
pub fn forget_file(root: &Path, path: &Path) {
    for alias in file_aliases(path) {
        let _ = fs::remove_file(root.join(VERSIONS_FOLDER).join(alias));
    }
}

/// Clears out notes about corpora that are gone, so these can't pile up.
pub(super) fn tidy_versions(root: &Path) {
    let Ok(entries) = fs::read_dir(root.join(VERSIONS_FOLDER)) else { return };
    for entry in entries.flatten() {
        let keys = read_alias(&entry.path());
        let left: Vec<CorpusKey> = keys.iter().copied().filter(|key| root.join(key.hex()).exists()).collect();
        if left.len() != keys.len() || keys.is_empty() {
            let _ = write_alias(&entry.path(), &left);
        }
    }
}
