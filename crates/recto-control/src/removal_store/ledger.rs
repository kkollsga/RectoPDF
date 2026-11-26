//! **The provenance ledger's writer and reader** (`docs/design/removal.md` §10, D2b): the only code that opens `<state root>/removal/ledger.v1`. The format and the
//! rules for reading it are `recto_core::removal`'s; this is what the app does with them: notes a save makes, the tabs a record names, the one adoption of older
//! journals, and the lock that keeps two writers from appending the same thing twice.
//!
//! It records paths and numbers, and opens a document never: only a stat and `canonicalize` ([`canonical_version`]). A save never fails, or waits longer than the lock's
//! bound ([`Config::lock_wait`], 250 ms), because of it: whatever can't be written leaves `removal/ledger.gap`, and the ledger then reads as having a gap, so a
//! removal refuses rather than reads "nothing found".
//!
//! **Why a note is not flushed to the drive.** A note lost to a crash or a power cut leaves a backup with no record, which reads `Unknown` and refuses a removal:
//! a false refusal, never a wrong answer. A flush on the save's own path would be an unbounded wait on the disk, which a save must not do for this.

use recto_agent::saving::SaveNote;
use recto_core::removal::{Entry, FileVersion, Gap, LEDGER_LIMITS, Ledger, LedgerLimits, Link, MAX_LEDGER_BYTES, encode, encode_header, read_ledger};
use recto_core::save::backup_name;
use recto_platform::{FileStamp, local};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod adopt;
mod tabs;

pub use tabs::{finish, note_tabs, running, take_new};
#[cfg(test)]
pub(crate) use tabs::{held, note_tabs_with};

const DIR: &str = "removal";
const FILE: &str = "ledger.v1";
/// Created, never written into, when an append failed or timed out.
const GAP_MARK: &str = "ledger.gap";

/// What bounds the writer. The defaults are the design's; tests make them small, so a limit is tested at the limit and one over it.
#[derive(Clone)]
pub struct Config {
    pub limits: LedgerLimits,
    /// Most journal headers adoption reads; one more is `Gap::AdoptionCut`.
    pub adoption_headers: usize,
    /// How long a writer retries for the lock, never blocking.
    pub lock_wait: Duration,
    /// How long adoption may take before it stops with `Gap::AdoptionCut`.
    pub adoption_time: Duration,
    /// Test seams: called with the lock held and everything read, just before the append, and by the `recto-ledger` thread before it looks anything up.
    #[cfg(test)]
    pub(crate) before_write: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    pub(crate) in_thread: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            limits: LEDGER_LIMITS,
            adoption_headers: 1_024,
            lock_wait: Duration::from_millis(250),
            adoption_time: Duration::from_millis(100),
            #[cfg(test)]
            before_write: None,
            #[cfg(test)]
            in_thread: None,
        }
    }
}

/// Where the ledger's folder is, for a state root.
fn dir_of(root: &Path) -> PathBuf {
    root.join(DIR)
}

/// The canonical path of `path` and its version, if a stat of that canonical path says it is `expect` in all four fields. The same `canonicalize` call that
/// `keep_backup` and `backup_of` hash (`recto_core::save`), so what is recorded is exactly what names the backup: never made to see through more. A path
/// reached through a link that has since moved, or a file that has since changed, records nothing.
pub fn canonical_version(path: &Path, expect: FileStamp) -> Option<(PathBuf, FileVersion)> {
    let real = path.canonicalize().ok()?;
    let version = verified(&real, expect)?;
    Some((real, version))
}

fn verified(real: &Path, expect: FileStamp) -> Option<FileVersion> {
    let now = recto_platform::file_stamp(real).ok()?;
    (now == expect).then(|| FileVersion::new(now, recto_platform::volume::volume_identity(real)))
}

/// What a save's step adds to the ledger. Pure over the files' stats.
fn entries_for(note: &SaveNote<'_>) -> Vec<Entry> {
    match note {
        SaveNote::Starting { path, from } => {
            let Ok(real) = path.canonicalize() else { return Vec::new() };
            // A version that can't be proven is still a file that was here: its path reads on both sides.
            let found = from.and_then(|from| verified(&real, from));
            vec![Entry::Link(match found {
                Some(version) => Link::At { version, path: real },
                None => Link::Unplaced { path: real },
            })]
        }
        // A Save As starts a history of its own: the new file is recorded when it lands, and nothing joins it to the one it was made from.
        SaveNote::Prepared { over_itself: false, .. } => Vec::new(),
        SaveNote::Prepared {
            path,
            from,
            to,
            over_itself: true,
        } => {
            let Some((real, from)) = from.and_then(|from| canonical_version(path, from)) else {
                return Vec::new();
            };
            let to = FileVersion::new(*to, recto_platform::volume::volume_identity(&real));
            vec![Entry::Link(Link::Saved { from, to })]
        }
        SaveNote::Landed { path, to, original, backup } => {
            let mut out: Vec<Entry> = to
                .and_then(|to| canonical_version(path, to))
                .map(|(path, version)| Entry::Link(Link::At { version, path }))
                .into_iter()
                .collect();
            if let Some(backup) = backup {
                let expected = original.canonicalize().ok().map(|real| backup_name(&real));
                if backup.file_name().and_then(|n| n.to_str()) != expected.as_deref() {
                    out.push(Entry::Gap(Gap::BackupMismatch));
                }
            }
            out
        }
    }
}

/// **Notes a step of a save.** `backups_folder` is the state root's `backups.noindex`; the ledger is `../removal/` beside it. Called on the save's own thread,
/// with no lock of the session held. Never fails, and waits at most the lock's bound.
pub fn note_save(backups_folder: &Path, note: &SaveNote<'_>) {
    note_save_with(backups_folder, note, &Config::default());
}

pub(crate) fn note_save_with(backups_folder: &Path, note: &SaveNote<'_>, config: &Config) {
    let Some(root) = backups_folder.parent() else { return };
    let entries = entries_for(note);
    if !entries.is_empty() {
        append(root, config, entries);
    }
}

/// Reads the ledger of this state root: what is in it and why it may not be all of the story. Nothing is written, however it reads.
pub fn read(root: &Path) -> Ledger {
    read_with(root, &LEDGER_LIMITS)
}

pub(crate) fn read_with(root: &Path, limits: &LedgerLimits) -> Ledger {
    let dir = dir_of(root);
    let mut ledger = match bytes_in(&dir) {
        Ok(None) => Ledger::missing(),
        Ok(Some(bytes)) => read_ledger(&bytes, limits),
        Err(()) => Ledger::damaged(),
    };
    if std::fs::symlink_metadata(dir.join(GAP_MARK)).is_ok() {
        ledger.gap = ledger.gap.or(Some(Gap::WriteFailed));
    }
    ledger
}

/// The ledger's bytes, at most one more than a ledger may hold. `Ok(None)` if there is no folder or no file, and `Err` for anything but a plain private file in a
/// folder that is this user's alone: a folder that is a link holds somebody else's ledger as far as can be told.
fn bytes_in(dir: &Path) -> Result<Option<Vec<u8>>, ()> {
    match std::fs::symlink_metadata(dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        _ => local::check_private_dir(dir).map_err(|_| ())?,
    }
    match local::open_private_file(&dir.join(FILE)) {
        Ok(file) => read_capped(&file).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

fn read_capped(file: &File) -> Result<Vec<u8>, ()> {
    let mut bytes = Vec::new();
    file.take(MAX_LEDGER_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|_| ())?;
    Ok(bytes)
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_nanos()).ok())
        .unwrap_or(i64::MAX)
}

/// Takes the lock without blocking, retrying for at most `wait`.
fn lock(file: &File, wait: Duration) -> bool {
    let began = Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return true,
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(_)) => return false,
        }
        if began.elapsed() >= wait {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
}

/// What a write has room for: the frames and path bytes the file holds, against the caps.
struct Room {
    frames: usize,
    path_total: usize,
    overflowed: bool,
    max_links: usize,
    max_paths: usize,
}

impl Room {
    fn of(ledger: &Ledger, limits: &LedgerLimits) -> Self {
        let path_total = ledger
            .links
            .iter()
            .map(|link| match link {
                Link::At { path, .. } | Link::Unplaced { path } => path.as_os_str().as_encoded_bytes().len(),
                Link::Saved { .. } => 0,
            })
            .sum();
        Self {
            frames: ledger.frames,
            path_total,
            overflowed: ledger.overflowed,
            max_links: limits.max_links as usize,
            max_paths: limits.max_total_path_bytes as usize,
        }
    }

    /// Adds `entry`'s frame to `out` if there is room for it and still one frame for `Gap { Overflowed }`; if not, writes that gap once, and nothing after it.
    fn admit(&mut self, entry: &Entry, out: &mut Vec<u8>) {
        if self.overflowed {
            return;
        }
        let paths = match entry {
            Entry::Link(Link::At { path, .. } | Link::Unplaced { path }) => path.as_os_str().as_encoded_bytes().len(),
            _ => 0,
        };
        if self.frames + 2 > self.max_links || self.path_total + paths > self.max_paths {
            self.overflowed = true;
            if self.frames < self.max_links
                && let Some(frame) = encode(&Entry::Gap(Gap::Overflowed))
            {
                out.extend_from_slice(&frame);
                self.frames += 1;
            }
            return;
        }
        if let Some(frame) = encode(entry) {
            out.extend_from_slice(&frame);
            self.frames += 1;
            self.path_total += paths;
        }
    }
}

/// Appends `entries` under the lock, once each. A failure, or the lock not coming in time, leaves `removal/ledger.gap`. Never panics, never waits past the lock's bound
/// (and adoption's).
pub(crate) fn append(root: &Path, config: &Config, entries: Vec<Entry>) {
    match try_append(root, config, entries) {
        Ok(()) | Err(Failed::Unwritable) => {}
        Err(Failed::Lost) => mark_gap(root),
    }
}

/// Leaves the gap mark, only in a folder that is really this user's own (a linked `removal/` would put it in whatever the link points at), and opened as the
/// ledger is: a link isn't followed and a pipe isn't waited on. If it can't be made, nothing is: a save is never held up for it.
fn mark_gap(root: &Path) {
    let dir = dir_of(root);
    if local::check_private_dir(&dir).is_ok() {
        let _ = local::append_private_file(&dir.join(GAP_MARK));
    }
}

/// What an open that failed means: a link, a pipe or a folder at the name is a ledger this code doesn't write to; anything else (no descriptors, no access, an
/// interrupted call) is a write that should have happened and didn't.
fn failure_of(path: &Path) -> Failed {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => Failed::Unwritable,
        _ => Failed::Lost,
    }
}

enum Failed {
    /// A ledger this code doesn't write to (damaged, newer, not a plain file): left as it is, and it reads as a gap by itself.
    Unwritable,
    /// A write that should have happened didn't.
    Lost,
}

fn try_append(root: &Path, config: &Config, entries: Vec<Entry>) -> Result<(), Failed> {
    let dir = dir_of(root);
    let made = !dir.exists();
    local::ensure_private_dir(&dir).map_err(|_| Failed::Lost)?;
    if made || !local::is_excluded_from_backups(&dir) {
        let _ = local::exclude_from_backups(&dir);
    }
    // Not followed, not blocking on a pipe: a link or a non-file at the name is refused below.
    let file = local::append_private_file(&dir.join(FILE)).map_err(|_| failure_of(&dir.join(FILE)))?;
    let meta = file.metadata().map_err(|_| Failed::Lost)?;
    if !meta.is_file() {
        return Err(Failed::Unwritable);
    }
    if !lock(&file, config.lock_wait) {
        return Err(Failed::Lost);
    }
    // Read through a second descriptor of the same file: the one that holds the lock only appends.
    let reading = local::open_private_file(&dir.join(FILE)).map_err(|_| failure_of(&dir.join(FILE)))?;
    let (here, there) = (recto_platform::open_file_stamp(&file), recto_platform::open_file_stamp(&reading));
    let (Ok(here), Ok(there)) = (here, there) else { return Err(Failed::Lost) };
    if (here.device, here.inode) != (there.device, there.inode) {
        return Err(Failed::Unwritable);
    }
    let bytes = read_capped(&reading).map_err(|()| Failed::Lost)?;
    let ledger = read_ledger(&bytes, &config.limits);
    let mut out = Vec::new();
    let mut room = Room::of(&ledger, &config.limits);
    let first = match ledger.stopped {
        Some(Gap::Damaged | Gap::TooNew | Gap::Missing) => return Err(Failed::Unwritable),
        // A cut-short tail: cut back to the last whole frame, and say so. A header cut short is a ledger that never began.
        Some(Gap::Torn) => {
            file.set_len(ledger.good_len as u64).map_err(|_| Failed::Lost)?;
            if ledger.good_len == 0 {
                true
            } else {
                room.admit(&Entry::Gap(Gap::Torn), &mut out);
                false
            }
        }
        _ => false,
    };
    // A ledger just begun has no gap yet: the one a cut-short header read as was its own.
    let mut known_gap = !first && (ledger.gap.is_some() || !out.is_empty());
    let mut adopted = Vec::new();
    if first {
        out.extend_from_slice(&encode_header(now_ns()));
        let (found, cut) = adopt::adopt(root, config);
        adopted = found;
        adopted.extend(cut.then_some(Entry::Gap(Gap::AdoptionCut)));
    }
    let mut seen: Vec<&Link> = ledger.links.iter().collect();
    for entry in adopted.iter().chain(entries.iter()) {
        match entry {
            Entry::Link(link) => {
                if seen.contains(&link) {
                    continue;
                }
                seen.push(link);
            }
            Entry::Gap(_) if known_gap => continue,
            Entry::Gap(_) => known_gap = true,
        }
        room.admit(entry, &mut out);
    }
    if out.is_empty() {
        return Ok(());
    }
    #[cfg(test)]
    if let Some(seam) = &config.before_write {
        seam();
    }
    (&file).write_all(&out).map_err(|_| Failed::Lost)
}

#[cfg(test)]
mod tests;
