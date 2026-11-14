//! **Sleep files:** each a quick way back to a tab gone dormant, written in one step, read back only for its own version of its file, and held to
//! [`MAX_SLEEP_BYTES`] in all. Losing one costs only the quick way, so nothing here is flushed or said when it fails.

use super::{CLOSED_JOURNAL_DAYS, JOURNALS, RecordStore, local, read_ours, record};
use recto_platform::FileStamp;
use recto_session::record::{CURRENT_VERSION, RecordError};
use recto_session::sleep::{MAX_SLEEP_FILE_BYTES, SleepFile, decode_sleep, encode_sleep};
use std::path::{Path, PathBuf};

/// Sleep files in all. About 256 tabs' worth of a whole picture, and more with text, which packs well.
pub(super) const MAX_SLEEP_BYTES: u64 = 256 * 1024 * 1024;

impl RecordStore {
    /// **Writes a tab's sleep file,** in one step, as it goes dormant or the app goes away. It's only a way to be quick, so failing here costs nothing but that, and isn't said. Says how many bytes went.
    pub fn write_sleep(&self, path: &Path, sleep: &SleepFile) -> u64 {
        self.write_sleep_with(path, sleep.file, || std::borrow::Cow::Owned(encode_sleep(sleep)))
    }

    /// **Writes a tab's sleep file already packed** ([`encode_sleep`]), for the file of `stamp` at `path`: an agent's, measured against agents' share
    /// before it's written, so what's written is what was measured and it isn't packed twice. Otherwise as [`Self::write_sleep`].
    pub fn write_sleep_bytes(&self, path: &Path, stamp: FileStamp, bytes: &[u8]) -> u64 {
        self.write_sleep_with(path, stamp, || std::borrow::Cow::Borrowed(bytes))
    }

    /// The one way a sleep file's written: atomically, nothing if what's there is these bytes already, and the folder pruned after. The bytes are packed
    /// only once the store would write, so a refused write packs nothing.
    fn write_sleep_with<'b>(&self, path: &Path, stamp: FileStamp, bytes: impl FnOnce() -> std::borrow::Cow<'b, [u8]>) -> u64 {
        recto_platform::slow::pause(recto_platform::slow::Seam::SleepWrite);
        if self.refuses_writes().is_some() {
            return 0;
        }
        let dir = self.dir();
        if Self::ensure_dirs(&dir).is_err() {
            return 0;
        }
        self.note_going_back(&dir);
        let bytes = bytes();
        let folder = dir.join(JOURNALS);
        let file = folder.join(record::sleep_file_name(stamp, path));
        // What's there is this already: nothing's changed, so nothing's written.
        if read_ours(&file, MAX_SLEEP_FILE_BYTES)
            .ok()
            .flatten()
            .is_some_and(|there| there[..] == bytes[..])
        {
            return 0;
        }
        // With no flush: losing one to a crash costs only the quick way, and a torn one's caught by its checksum, so it isn't worth the drive's time or wear.
        let temp = file.with_extension(format!("tmp{}", std::process::id()));
        if bytes.len() > MAX_SLEEP_FILE_BYTES || local::write_private_file_unsynced(&temp, &bytes).is_err() || std::fs::rename(&temp, &file).is_err() {
            let _ = std::fs::remove_file(&temp);
            return 0;
        }
        self.progress().wrote_bytes += bytes.len() as u64;
        prune_sleep_files(&folder);
        bytes.len() as u64
    }

    /// Removes a tab's sleep file, for this version of this file, if there's one: a write that landed after the file was retired.
    pub fn remove_sleep(&self, stamp: FileStamp, path: &Path) {
        let _ = std::fs::remove_file(self.dir().join(JOURNALS).join(record::sleep_file_name(stamp, path)));
    }

    /// A tab's sleep file, if there's one for this version of this file. The name goes by the stamp, so what's asked for is what's known of the file, and nothing here looks at the PDF, which may be on a disk that's gone away. Whoever's waking the tab looks at that on a thread of its own. **One that doesn't read is removed:** it can always be rebuilt.
    pub fn read_sleep(&self, stamp: FileStamp, path: &Path) -> Option<SleepFile> {
        recto_platform::slow::pause(recto_platform::slow::Seam::SleepRead);
        let file = self.dir().join(JOURNALS).join(record::sleep_file_name(stamp, path));
        let bytes = read_ours(&file, MAX_SLEEP_FILE_BYTES).ok().flatten()?;
        match decode_sleep(&bytes, CURRENT_VERSION) {
            Ok(sleep) if sleep.file.same_contents(stamp) && sleep.file.inode == stamp.inode => Some(sleep),
            // A newer app's, which isn't this one's to touch.
            Err(RecordError::Newer(_)) => None,
            _ => {
                let _ = std::fs::remove_file(&file);
                None
            }
        }
    }
}

/// Sleep files are held to [`MAX_SLEEP_BYTES`] in all, oldest first, and 30 days: each can be rebuilt. A look at their sizes each time one's written, which is once for a tab going dormant.
pub(super) fn prune_sleep_files(folder: &Path) {
    let now = std::time::SystemTime::now();
    let mut files: Vec<(u64, PathBuf, u64)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_str().is_some_and(record::is_sleep_name))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let age = meta
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .map_or(0, |age| age.as_secs());
            Some((age, entry.path(), meta.len()))
        })
        .collect();
    files.sort_unstable_by_key(|(age, ..)| *age);
    let mut total = 0u64;
    for (age, path, bytes) in files {
        total += bytes;
        if age > CLOSED_JOURNAL_DAYS * 86_400 || total > MAX_SLEEP_BYTES {
            let _ = std::fs::remove_file(path);
        }
    }
}
