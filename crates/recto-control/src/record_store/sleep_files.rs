//! **Sleep files:** each a quick way back to a tab gone dormant, its facts only (no picture of any page since 2026-11-15), written in one step, read back
//! only for its own version of its file, and held to [`MAX_SLEEP_BYTES`] in all. Losing one costs only the quick way, so nothing here is flushed or said
//! when it fails. Every file of an older format, which could hold pictures of pages, is removed by one pass after a launch
//! ([`RecordStore::remove_older_sleep_files`]) and at its first read.

use super::{CLOSED_JOURNAL_DAYS, JOURNALS, RecordStore, local, read_ours, record};
use recto_platform::FileStamp;
use recto_session::record::{CURRENT_VERSION, RecordError};
use recto_session::sleep::{FORMAT_PREFIX_BYTES, MAX_SLEEP_FILE_BYTES, SLEEP_FORMAT, SleepFile, decode_sleep, encode_sleep, sleep_format_of};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Sleep files in all: what the folder could hold of files with pictures in them, and far past what facts come to.
pub const MAX_SLEEP_BYTES: u64 = 256 * 1024 * 1024;

/// **Sleep files the pass after a launch looks at, at most** ([`RecordStore::remove_older_sleep_files`]): more than the folder held of 0.1.0's files at its
/// 256 MiB (65 KB on average, measured 2026-11-13, so about 4,000). Past it the pass stops; the next launch's goes on, and a file left meanwhile is still
/// refused and removed at its first read.
pub const MAX_SLEEP_FILES_LOOKED_AT: usize = 8192;

/// What the pass after a launch came to, for `qa state`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OlderSleepFiles {
    /// Sleep files looked at: never past the cap.
    pub looked_at: u32,
    /// Of those, removed: of an older format, or not a sleep file's start.
    pub removed: u32,
    /// What was read of them, at most [`FORMAT_PREFIX_BYTES`] each.
    pub bytes_read: u64,
    /// It stopped at its cap, with sleep files it didn't look at.
    pub stopped_at_cap: bool,
}

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
        // Under the progress lock from here to the end, where a removal's hold is looked at again (`hold`).
        let mut progress = self.progress();
        if progress.withholds() {
            return 0;
        }
        // With no flush: losing one to a crash costs only the quick way, and a torn one's caught by its checksum, so it isn't worth the drive's time or wear.
        let temp = file.with_extension(format!("tmp{}", std::process::id()));
        if bytes.len() > MAX_SLEEP_FILE_BYTES || local::write_private_file_unsynced(&temp, &bytes).is_err() || std::fs::rename(&temp, &file).is_err() {
            let _ = std::fs::remove_file(&temp);
            return 0;
        }
        progress.wrote_bytes += bytes.len() as u64;
        prune_sleep_files(&folder);
        drop(progress);
        bytes.len() as u64
    }

    /// Removes a tab's sleep file, for this version of this file, if there's one: a write that landed after the file was retired.
    pub fn remove_sleep(&self, stamp: FileStamp, path: &Path) {
        if self.gate().is_some() {
            return;
        }
        self.remove_unless_held(&self.dir().join(JOURNALS).join(record::sleep_file_name(stamp, path)));
    }

    /// A tab's sleep file, if there's one for this version of this file. The name goes by the stamp, so what's asked for is what's known of the file, and nothing here looks at the PDF, which may be on a disk that's gone away. Whoever's waking the tab looks at that on a thread of its own. **One that doesn't read is removed:** it can always be rebuilt.
    pub fn read_sleep(&self, stamp: FileStamp, path: &Path) -> Option<SleepFile> {
        recto_platform::slow::pause(recto_platform::slow::Seam::SleepRead);
        // This reads, and removes what it can't read: neither while a removal is unfinished.
        if self.gate().is_some() {
            return None;
        }
        let file = self.dir().join(JOURNALS).join(record::sleep_file_name(stamp, path));
        let bytes = match read_ours(&file, MAX_SLEEP_FILE_BYTES) {
            Ok(bytes) => bytes?,
            // Past what a file of facts can be (an older format's, with pictures), or not a plain private file: it goes, as one that doesn't read does.
            Err(()) => {
                self.remove_unless_held(&file);
                return None;
            }
        };
        match decode_sleep(&bytes, CURRENT_VERSION) {
            Ok(sleep) if sleep.file.same_contents(stamp) && sleep.file.inode == stamp.inode => Some(sleep),
            // A newer app's, which isn't this one's to touch.
            Err(RecordError::Newer(_)) => None,
            _ => {
                self.remove_unless_held(&file);
                None
            }
        }
    }
}

/// **Where the copy of the record an update keeps goes, under the state folder:** `update/before/`, as `recto_update::plan::Files::before` has it (a test in
/// `recto-app`, which sees both, holds the two together). 0.1.0's installer clones everything of the record into it but closed journals, its sleep files
/// with their pictures among them, and keeps it a week.
pub const UPDATE_COPY: [&str; 2] = ["update", "before"];

impl RecordStore {
    /// **Removes every sleep file of an older format** (owner, 2026-11-15: no picture of a page is kept; every format before
    /// [`SLEEP_FORMAT`], 0.1.0's included, could hold some), once after a launch, off the UI thread and the first frame's path: in the state's `journals/`
    /// folder, and in the copy of the record an update keeps ([`UPDATE_COPY`]`/journals/`, where 0.1.0's installer put a clone of every sleep file for a
    /// week). Only files named as sleep files, plain files of this user's (a link is never followed or touched, nor a folder on the way that's a link), and
    /// at most `cap` of them in all; of each, only its first [`FORMAT_PREFIX_BYTES`], which say its format ([`sleep_format_of`]). A file that can't say it's
    /// this format or newer goes: nothing of it is what a wake can use, and its tab wakes from its PDF once (in the update's copy, if a going back puts it
    /// back). Nothing else in either folder is touched, but for the copy's folder itself, which is marked to be kept out of backups if it isn't (0.1.0's
    /// installer left it unmarked). Nothing's removed by a copy that doesn't keep the record. Nothing's said if it fails.
    pub fn remove_older_sleep_files(&self, cap: usize) -> OlderSleepFiles {
        let mut done = OlderSleepFiles::default();
        if self.refuses_writes().is_some() {
            return done;
        }
        // Under the progress lock, where a removal's hold is looked at again (`hold`); once after a launch, off the UI thread.
        let progress = self.progress();
        if progress.withholds() {
            return done;
        }
        sweep_older(&self.dir().join(JOURNALS), cap, &mut done);
        // The update's copy, only by real folders all the way down from the state folder: a link there could lead anywhere.
        let real_folder = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir());
        let mut copy = self.root.clone();
        for part in UPDATE_COPY {
            copy.push(part);
            if !real_folder(&copy) {
                return done;
            }
        }
        // It holds the edit history, which stays out of Time Machine (PRIVACY.md); 0.1.0's installer made it without the mark the installer gives it now
        // (`recto_update::install::copy_record`), so it's given here, by the same function. A failure costs only that.
        if !local::is_excluded_from_backups(&copy) {
            let _ = local::exclude_from_backups(&copy);
        }
        copy.push(JOURNALS);
        if real_folder(&copy) {
            sweep_older(&copy, cap, &mut done);
        }
        done
    }
}

/// [`RecordStore::remove_older_sleep_files`] in one folder, counting into `done` and stopping at `cap` in all.
fn sweep_older(folder: &Path, cap: usize, done: &mut OlderSleepFiles) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let mut prefix = Vec::with_capacity(FORMAT_PREFIX_BYTES);
    for entry in entries.flatten() {
        let is_sleep = entry.file_name().to_str().is_some_and(record::is_sleep_name);
        // The entry's own type, not what a link points at.
        if !is_sleep || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if done.looked_at as usize >= cap {
            done.stopped_at_cap = true;
            return;
        }
        done.looked_at += 1;
        let path = entry.path();
        let Ok(file) = local::open_private_file(&path) else { continue };
        prefix.clear();
        if file.take(FORMAT_PREFIX_BYTES as u64).read_to_end(&mut prefix).is_err() {
            continue;
        }
        done.bytes_read += prefix.len() as u64;
        if sleep_format_of(&prefix).is_none_or(|format| format < SLEEP_FORMAT) && std::fs::remove_file(&path).is_ok() {
            done.removed += 1;
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
