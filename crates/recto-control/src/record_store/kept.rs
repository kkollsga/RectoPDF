//! **The copy of pages a save takes out,** kept for undoing past that save (`keep_pages_of`), in the journals' folder. Written under a temp name without the
//! progress lock (it can be 64 MiB, and a journal's write mustn't wait for it), and put in place under the lock, where a removal's hold is looked at again
//! (`hold`).

use super::*;

impl RecordStore {
    /// **Keeps a copy of the pages a history can put back, from a file about to be replaced,** for undoing past that save: a step that puts back a page the save takes out needs it from somewhere, and the history's the only place. **Only those pages** (owner, 2026-11-07: "keeps only the removed pages"), written by the save's own rewrite, so it works whatever the size of the PDF, within [`recto_session::edit::MAX_HISTORY_BYTES`], the limit a PDF's history has: past that, `None`, and the steps roll off. Synced before it's relied on, so a power cut can't leave the right stamp on the wrong contents. In the journals' folder, private and out of backups, and named so Spotlight has no importer for it: its text isn't indexed. `target` is what the save will put in place, which is what names it. On whatever thread is saving.
    pub fn keep_pages_of(&self, path: &Path, target: FileStamp, pages: &[u32]) -> Option<AddedSource> {
        if self.refuses_writes().is_some() || pages.is_empty() {
            return None;
        }
        let dir = self.dir();
        Self::ensure_dirs(&dir).ok()?;
        let folder = dir.join(JOURNALS);
        let name = (0..64)
            .map(|taken| record::kept_file_name(target, taken))
            .find(|name| std::fs::symlink_metadata(folder.join(name)).is_err())?;
        let kept = folder.join(&name);
        // Copied under a name of the shape a crash's leftover has (`.append-<key>-<pid>-0.tmp`, which a removal's survey places as `Unknown`), without the
        // progress lock: the copy can be 64 MiB, and a journal's write mustn't wait for it. It is renamed to its own name under the lock, which is where a
        // removal's hold is looked at again (`hold`).
        let key = name.strip_prefix("kept-").and_then(|rest| rest.get(..16)).unwrap_or("0000000000000000");
        let temp = folder.join(format!(".append-{key}-{}-0.tmp", std::process::id()));
        let _ = std::fs::remove_file(&temp);
        let copied = recto_core::save::keep_pages(path, pages, &temp, recto_session::edit::MAX_HISTORY_BYTES as u64).ok();
        let stamp = copied.and_then(|_| {
            // Its age is its own, which is what the 30 days go by, and it's this user's alone. Synced again, as both are changes since the copy was synced.
            let made = (|| -> std::io::Result<FileStamp> {
                local::make_private_file(&temp)?;
                age_starts_now(&temp)?.sync_all()?;
                file_stamp(&temp)
            })();
            made.ok().filter(|stamp| stamp.size <= recto_session::edit::MAX_HISTORY_BYTES as u64)
        });
        let Some(stamp) = stamp else {
            let _ = std::fs::remove_file(&temp);
            return None;
        };
        hold::parked(&self.root, true);
        let progress = self.progress();
        // A hold that came in during the copy: no kept file, and nothing of the copy left. Or the name taken meanwhile: not written over.
        if progress.withholds() || std::fs::symlink_metadata(&kept).is_ok() || std::fs::rename(&temp, &kept).is_err() {
            drop(progress);
            let _ = std::fs::remove_file(&temp);
            return None;
        }
        let _ = local::sync_folder_of(&kept);
        drop(progress);
        let count = u32::try_from(pages.len()).ok()?;
        Some(AddedSource {
            path: kept.into_boxed_path(),
            page_count: count,
            stamp: Some(stamp),
            kept: true,
            protected: false,
            holds: Some(pages.into()),
        })
    }
}
