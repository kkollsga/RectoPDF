//! **The provenance ledger's writer and reader** (`docs/design/removal.md` §10, D2b): the only code that opens `<state root>/removal/ledger.v1`. The format and the
//! rules for reading it are `recto_core::removal`'s; this is what the app does with them: notes a save makes, the tabs a record names, the one adoption of older
//! journals, and the lock that keeps two writers from appending the same thing twice.
//!
//! It records paths and numbers, and opens a document never: only a stat and `canonicalize` ([`canonical_version`]). A save never fails, or waits longer than the lock's
//! bound ([`Config::lock_wait`], 250 ms), because of it: the one adoption of the older journals, which can take as long as a slow disk takes, is done by the
//! `recto-ledger` thread, never on a save's own. Whatever can't be written leaves `removal/ledger.gap` (a link or a pipe at its name isn't followed or waited
//! on); the ledger then reads as having a gap, so a removal refuses rather than reads "nothing found".
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
mod support;
mod tabs;

pub use support::ProvenanceOnly;

pub use tabs::note_tabs;
#[cfg(test)]
pub(crate) use tabs::{Roots, note_tabs_with};
#[cfg(any(test, feature = "test-seams"))]
pub use tabs::{finish, running, take_new};

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
    /// Test seam: called, once, when a writer finds the lock held and is about to retry.
    #[cfg(test)]
    pub(crate) at_lock: Option<Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    pub(crate) in_thread: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Test seam: the set of roots a test's ledger threads use, so neighbours' roots in the process's set can't evict or crowd it.
    #[cfg(test)]
    pub(crate) roots: Option<Arc<tabs::Roots>>,
    /// Test seam: called by the `recto-ledger` thread when it has been handed the adoption, before it reads a journal.
    #[cfg(test)]
    pub(crate) before_adoption: Option<Arc<dyn Fn() + Send + Sync>>,
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
            at_lock: None,
            #[cfg(test)]
            in_thread: None,
            #[cfg(test)]
            roots: None,
            #[cfg(test)]
            before_adoption: None,
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

/// The destination's frames: where it is by name ([`destination_entry`]) and, if something is there already, where the writer resolves it to. lopdf's
/// `real_path` follows the whole path of an existing destination (a link too) and makes its temp in that file's folder, named for that file: a Replace
/// onto a link into another folder works there. Both are recorded, so both folders are listed. At most two frames.
fn destination_entries(destination: &Path) -> Vec<Entry> {
    let by_name = destination_entry(destination);
    let mut out: Vec<Entry> = by_name.iter().cloned().collect();
    if !matches!(by_name, Some(Entry::Destination(_))) || std::fs::symlink_metadata(destination).is_err() {
        return out;
    }
    match destination.canonicalize() {
        Ok(real) => {
            let entry = Entry::Destination(real);
            if !out.contains(&entry) {
                // Too long to hold is a note lost without a word.
                let too_long = matches!(&entry, Entry::Destination(p) if p.as_os_str().as_encoded_bytes().len() > recto_core::removal::MAX_PATH_BYTES);
                out.push(if too_long { Entry::Gap(Gap::WriteFailed) } else { entry });
            }
        }
        // The writer fails the same way at once (a link that names nothing, a loop of links): it can make no temp.
        Err(error) if writer_fails_here(&error) || error.kind() == std::io::ErrorKind::NotFound => {}
        // Any other error: a temp may be where nobody can list.
        Err(_) => out.push(Entry::Gap(Gap::WriteFailed)),
    }
    out
}

/// **Where a Save As or a Replace is going, before it is there.** The writer makes its temporary, named for the destination's file name, in the
/// destination's folder before anything lands, so a crash leaves it there with the whole document in it, and the survey can only list that folder if the
/// ledger names a path in it. The file may not be there yet, nor its folder, so the nearest folder that exists (the destination's parent, `.` for a bare name)
/// is canonicalized and the missing parts and the file name appended: a folder made between this note and the write is still the one recorded.
///
/// It is a [`Entry::Destination`], not a link: it joins no version, so the copy's own journals and backups place as they would for any file, and the survey of
/// the writer's temporaries alone reads it (for good: no landing or later sighting of the path drops it). A path the writer fails on at once (a part of the parent that is
/// a file, a name too long, a loop of links) records nothing and is no gap: it can make no temp. Any other failure to resolve it (no permission, an I/O
/// error) records nothing and leaves a gap, the safe direction: a temp may be there that nobody can list, so every removal refuses.
fn destination_entry(destination: &Path) -> Option<Entry> {
    let parent = destination.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let Some(name) = destination.file_name() else {
        return Some(Entry::Gap(Gap::WriteFailed));
    };
    // From the whole parent up, one part at a time: at most the path's own components.
    let mut missing: Vec<&std::ffi::OsStr> = Vec::new();
    let mut at = parent;
    let real = loop {
        match if at.as_os_str().is_empty() { Path::new(".") } else { at }.canonicalize() {
            Ok(real) => break real,
            Err(error) if writer_fails_here(&error) => return None,
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Some(Entry::Gap(Gap::WriteFailed)),
            Err(_) => {}
        }
        // Only a plain name is a part that may yet be made; `..` and the root can't be missing.
        match (at.file_name(), at.parent()) {
            (Some(part), Some(up)) => {
                missing.push(part);
                at = up;
            }
            _ => return None,
        }
    };
    let mut path = real;
    path.extend(missing.iter().rev());
    path.push(name);
    // A path the ledger can't hold would be a note that was lost without a word.
    if path.as_os_str().as_encoded_bytes().len() > recto_core::removal::MAX_PATH_BYTES {
        return Some(Entry::Gap(Gap::WriteFailed));
    }
    Some(Entry::Destination(path))
}

/// Whether the writer, given this error for a part of the path, fails there and can make no temporary.
fn writer_fails_here(error: &std::io::Error) -> bool {
    use std::io::ErrorKind::{InvalidFilename, NotADirectory};
    // `ErrorKind` has no stable name for a loop of links.
    matches!(error.kind(), NotADirectory | InvalidFilename) || error.raw_os_error() == Some(libc::ELOOP)
}

/// What a save's step adds to the ledger. Pure over the files' stats.
fn entries_for(note: &SaveNote<'_>) -> Vec<Entry> {
    match note {
        SaveNote::Starting { path, from, destination } => {
            let mut out = Vec::new();
            if let Ok(real) = path.canonicalize() {
                // A version that can't be proven is still a file that was here: its path reads on both sides.
                let found = from.and_then(|from| verified(&real, from));
                out.push(Entry::Link(match found {
                    Some(version) => Link::At { version, path: real },
                    None => Link::Unplaced { path: real },
                }));
            }
            out.extend(destination.map(destination_entries).unwrap_or_default());
            out
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
fn lock(file: &File, config: &Config) -> bool {
    let (began, wait) = (Instant::now(), config.lock_wait);
    #[cfg(test)]
    let mut told = false;
    loop {
        match file.try_lock() {
            Ok(()) => return true,
            Err(std::fs::TryLockError::WouldBlock) =>
            {
                #[cfg(test)]
                if let (Some(seam), false) = (&config.at_lock, std::mem::replace(&mut told, true)) {
                    seam();
                }
            }
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
        Self {
            frames: ledger.frames,
            path_total: ledger.path_bytes,
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
            Entry::Link(Link::At { path, .. } | Link::Unplaced { path }) | Entry::Destination(path) => path.as_os_str().as_encoded_bytes().len(),
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
        Ok(began) => {
            // A ledger just begun is adopted into by the ledger's thread, never by this one: the older journals are many, and this may be a save.
            if began {
                tabs::request_adoption(root, config);
            }
        }
        Err(Failed::Unwritable) => {}
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

/// `Ok(true)` if this call began the ledger (wrote its header).
fn try_append(root: &Path, config: &Config, entries: Vec<Entry>) -> Result<bool, Failed> {
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
    if !lock(&file, config) {
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
    if first {
        out.extend_from_slice(&encode_header(now_ns()));
    }
    let mut seen: Vec<&Link> = ledger.links.iter().collect();
    let mut seen_destinations: Vec<&PathBuf> = ledger.destinations.iter().collect();
    for entry in &entries {
        match entry {
            Entry::Link(link) => {
                if seen.contains(&link) {
                    continue;
                }
                seen.push(link);
            }
            Entry::Destination(path) => {
                // Once per path, ever: a landing doesn't make the next save to the name a new destination, since the reader keeps each one.
                if seen_destinations.contains(&path) {
                    continue;
                }
                seen_destinations.push(path);
            }
            Entry::Gap(_) if known_gap => continue,
            Entry::Gap(_) => known_gap = true,
        }
        room.admit(entry, &mut out);
    }
    if out.is_empty() {
        return Ok(false);
    }
    #[cfg(test)]
    if let Some(seam) = &config.before_write {
        seam();
    }
    (&file).write_all(&out).map_err(|_| Failed::Lost)?;
    Ok(first)
}

/// **Adoption, on the `recto-ledger` thread:** what the older journals prove, read with no lock held, then appended once like any other note (a link already
/// there is not added again).
pub(super) fn adopt_now(root: &Path, config: &Config) {
    #[cfg(test)]
    if let Some(seam) = &config.before_adoption {
        seam();
    }
    let (mut found, cut) = adopt::adopt(root, config);
    found.extend(cut.then_some(Entry::Gap(Gap::AdoptionCut)));
    if !found.is_empty() {
        append(root, config, found);
    }
}

#[cfg(test)]
mod tests;
