//! **Text from before it was sealed goes** (owner, 2026-11-15: "Lets store it encrypted"): the one pass that removes the plain formats' folders, at the first launch that can see them.

use super::CorpusKey;
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
    let Ok(entries) = fs::read_dir(root) else { return 0 };
    let (mut removed, mut left) = (0, 0);
    for entry in entries.flatten() {
        if entry.file_name().to_str().and_then(CorpusKey::from_hex).is_none() || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        for old in PLAIN_FORMATS {
            let folder = entry.path().join(old);
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
