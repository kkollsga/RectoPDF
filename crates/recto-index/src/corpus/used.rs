//! **`app/used`, the mark eviction reads** (`docs/design/removal.md` §10, "Step 4 (quiescence) design"). It is written at most once an hour per corpus, so a search needn't write.
//!
//! **Only under the corpus's `build.lock`.** The caches' purge holds `build.lock` from its second look until the folder is gone, and a `used` renamed into `app/` meanwhile
//! stopped the walk after the barrier and left the removal pending. A reader ([`note_used`]) therefore writes only if it gets the lock itself, once, and never waits; a caller
//! that already holds the lock on its own descriptor ([`note_used_held`]: the build, and the opens made under the build's or an agent folder's lock) writes as before. A reader that
//! skips only makes eviction see the corpus as used a little earlier. No thread, timer or buffer; a reader's descriptor lives for one write.

use super::CorpusPaths;
use recto_platform::local;
use std::fs;
use std::time::Duration;

/// How stale `used` may get, so a search needn't write anything.
const USED_RESOLUTION: Duration = Duration::from_secs(60 * 60);

fn is_fresh(paths: &CorpusPaths) -> bool {
    fs::metadata(&paths.used)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < USED_RESOLUTION)
}

fn write(paths: &CorpusPaths) {
    let _ = local::write_private_file_unsynced(&paths.used, b"");
}

/// For a caller that holds `build.lock`: notes the corpus as used, if the mark is stale.
pub(super) fn note_used_held(paths: &CorpusPaths) {
    if !is_fresh(paths) {
        write(paths);
    }
}

/// For a reader: notes the corpus as used, if the mark is stale and the corpus's `build.lock` is free. The lock file is opened, never made, and no link is followed, so a purge that has
/// unlinked it can't get one back. Not there, can't be opened or held (a build, an agent folder, a purge): nothing is written.
pub(super) fn note_used(paths: &CorpusPaths) {
    if is_fresh(paths) {
        return;
    }
    let Ok(lock) = local::open_private_file(&paths.lock) else { return };
    if lock.try_lock().is_err() {
        return;
    }
    write(paths);
    // Unlocked before it is closed (`record_store/lock.rs`).
    let _ = lock.unlock();
}
