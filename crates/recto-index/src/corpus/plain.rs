//! **Text from before it was sealed goes** (owner, 2026-11-15: "Lets store it encrypted"): the one pass that removes the plain formats' folders, at the first launch that can see them.

use recto_platform::local;
use std::fs;
use std::path::Path;

/// Formats before the text was sealed.
const PLAIN_FORMATS: &[&str] = &["v1", "v2", "v3"];

/// Marks a corpus folder whose plain formats have been looked for, so a start needs one look at a file to know there's nothing to do. Not named as a key, so nothing takes it for a corpus.
const SEALED_MARK: &str = "sealed";

/// Whether a start should look for text from before it was sealed: the folder's there and hasn't been looked through since.
pub fn plain_corpora_may_be_there(root: &Path) -> bool {
    root.is_dir() && !root.join(SEALED_MARK).exists()
}

/// **Text from before it was sealed goes.** Every corpus folder's plain formats are removed, and nothing else: only folders named as keys are looked at, and in them only the old formats' folders, never following a link. One pass, a look at each corpus folder and a removal of what it finds, on the indexer's thread at the session's first sweep. Returns how many went.
pub fn remove_plain_corpora(root: &Path) -> u32 {
    let (mut removed, mut left) = (0, 0);
    for (_, dir) in super::key_folders(root) {
        for old in PLAIN_FORMATS {
            let folder = dir.join(old);
            // `remove_dir_all` doesn't follow links, and a link here is left as it is.
            if fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir()) {
                match fs::remove_dir_all(&folder) {
                    Ok(()) => removed += 1,
                    Err(_) => left += 1,
                }
            }
        }
    }
    // Only once everything's gone. A folder that couldn't be removed is looked for again next time.
    if left == 0 {
        let _ = local::write_private_file_unsynced(&root.join(SEALED_MARK), b"");
    }
    removed
}

/// **Corpora this run can't open go,** at its first sweep, so text sealed by a key that's gone doesn't wait out its 90 days. A run on a lasting key removes every corpus it can't open; a run on a key in memory removes only those sealed by another run's key in memory, and leaves those of a lasting key it hasn't got just now, such as the Keychain's while it's locked. Never one being built. One small read and a key derivation per corpus. Returns how many went.
pub fn remove_unreadable(find_key: &recto_platform::find_key::FindKey, root: &Path) -> u32 {
    // A key bound to one corpus opens no other, so every other would look unreadable: only a session's key sweeps.
    if find_key.binding().is_some() {
        return 0;
    }
    let mut removed = 0;
    for (key, _) in super::key_folders(root) {
        let Ok(Some(header)) = local::read_private_file(&super::CorpusPaths::new(root, key).seal(), crate::seal::HEADER_BYTES as u64) else {
            continue;
        };
        if crate::seal::CorpusSeal::open(find_key, key, &header).is_ok() {
            continue;
        }
        let sweep = find_key.source().lasts() || crate::seal::CorpusSeal::lasting(&header) != Some(true);
        if sweep && super::clear(root, key).is_ok() {
            removed += 1;
        }
    }
    removed
}
