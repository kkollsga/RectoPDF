//! **The state record on disk.** What's in it is `recto_session::record`'s business. This is where it lives and how it gets there, in the order that means a crash can't break it.
//!
//! - `session/record.a.json`, `record.b.json`: the main record, in two slots. A write goes over the older, and the newer of the two that read whole is what's taken. So a crash, a torn write or a power cut mid-write leaves the other slot, with nothing depending on what order a drive puts things down in.
//! - `session/places.log`: one line per move of a place, for the main record of its sequence. A long list of tabs isn't rewritten to say the reader went on a page.
//! - `session/journals/journal-*.log`: each open PDF's unsaved edits and rolling history. Appended, never rewritten to add a step, and every line has its own checksum, so a torn tail costs that line.
//! - `session/journals/closed-*.log`: the same, for a PDF that's been closed with history and nothing unsaved. No launch reads these. They wait for the PDF to be opened, for 30 days and within 1 GiB, oldest out first, and they're kept out of backups (owner, 2026-11-07).
//!
//! **A journal has one owner.** Its header says whose it is, and at a launch each is matched against the tabs by what it's about: a tab that names it, or failing that a tab on the same file with none. A tab never applies a journal about another file. One with real unsaved edits and no tab is what a crash leaves, and comes back as a tab. One with nothing unsaved goes to `closed-`. A file that's there and can't be read as ours is never truncated or overwritten: writes go under another name beside it.
//!
//! **Flushing.** Ordinary writes get a plain `fsync`, which hands them to the drive and asks nothing more of it. Only [`RecordStore::flush`], for a hand-off, uses `F_FULLFSYNC`, which forces the drive itself to flush. A crash of the app loses nothing that was written. A power cut can lose what the drive hadn't got to: at worst the older slot or a shorter journal, never a broken one.
//!
//! **One writer.** A lock held for the process's life says this process keeps this record. A second app on the same folder neither reads nor writes it, and says so. A session with no window has a folder of its own, so `recto session save` from one can't overwrite the app's.
//!
//! **Versions.** A record from a newer app is never touched. This app works in a folder of its own version beside it, which is also what a migration keeps: going forward copies the old folder first, so going back finds it as it was.

use crate::state::StatePaths;
use recto_platform::{FileStamp, file_stamp, local};
use recto_session::edit::PageEdits;
use recto_session::record::{self, AppRecord, CURRENT_VERSION, DecodedRecord, FileNotice, FindRecord, JOURNAL_COMPACT_ABOVE, JournalHeader, JournalOp, MAX_JOURNAL_BYTES, MAX_PLACES_LOG_BYTES, MAX_RECORD_BYTES, ParsedJournal, PlaceDelta, RecordError, SessionRecord, TabEntry, ViewRecord, closed_journal_name, is_closed_journal_name, is_journal_name, journal_is_for};
use recto_session::{HistorySource, JournalOutcome, JournalWork, RestoredEdits, TakenHistory};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The two slots.
const SLOTS: [&str; 2] = ["record.a.json", "record.b.json"];
const PLACES: &str = "places.log";
const JOURNALS: &str = "journals";
/// All closed PDFs' histories together, past which the oldest go first. Owner, 2026-11-07.
const MAX_CLOSED_JOURNAL_BYTES: u64 = 1024 * 1024 * 1024;
/// How long a closed PDF's history waits to be picked up. Owner, 2026-11-07.
const CLOSED_JOURNAL_DAYS: u64 = 30;
/// Most bytes of journals a launch reads, whatever's there. Real ones are a few kilobytes each. What's past it stays where it is, and those tabs say they couldn't be read.
const MAX_JOURNAL_BYTES_AT_LAUNCH: u64 = 1024 * 1024 * 1024;
/// How many damaged states are kept.
const DAMAGED_KEPT: usize = 3;
/// What says a folder of an older version has work done in it after going back from a newer one, to bring forward.
const WENT_BACK: &str = "went-back";

/// How long a launch waits for the lock, in case what holds it is the app it's taking over from, on its way out.
const LOCK_PATIENCE: Duration = Duration::from_millis(600);

fn take_lock(path: &Path) -> Option<File> {
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()?;
    file.try_lock().ok()?;
    Some(file)
}

/// Why a record couldn't be written or confirmed. The updater won't go ahead on any of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    pub code: &'static str,
    pub message: String,
}

impl StoreError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

/// How long a confirmed write took, and what it came to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlushReport {
    pub tabs: usize,
    pub edited_tabs: usize,
    pub record_bytes: u64,
    pub journal_bytes: u64,
    /// Bytes it wrote.
    pub wrote_bytes: u64,
    pub gather_us: u64,
    pub write_us: u64,
    /// How long making the drive flush took, within `write_us`. Zero if it wasn't asked for.
    pub force_us: u64,
    pub verify_us: u64,
    pub total_us: u64,
}

/// What a launch found.
#[derive(Debug)]
pub enum Loaded {
    /// There's nothing here. A first launch, or one with everything closed long ago.
    Nothing,
    Restored(Box<Restored>),
    /// A main record was damaged in both slots, and nothing could be brought back. It's been kept to one side.
    Damaged,
    /// A record's there and couldn't be read just now. It's left as it is, and nothing's written over it.
    Unreadable,
    /// A newer app's record, left alone, and no record of this version beside it.
    Newer,
    /// Another process keeps this folder's record.
    OtherInstance,
}

#[derive(Debug)]
pub struct Restored {
    pub record: SessionRecord,
    pub journals: HashMap<String, RestoredEdits>,
    /// What to tell the reader about the record as a whole, by its code.
    pub notice: Option<&'static str>,
}

/// What's kept of how far things have got. Under one lock, held across writes and never across anything of the session's.
#[derive(Debug, Default)]
struct Progress {
    seq: u64,
    /// Which slot has the newest record. `None` before there's one.
    newest_slot: Option<usize>,
    /// The checksum of the main record last written, whole and with what a line can say taken out.
    whole_crc: Option<u32>,
    bare_crc: Option<u32>,
    places_bytes: u64,
    /// The bytes each journal has, by name.
    journal_bytes: HashMap<String, u64>,
    /// Bytes this has written, in all.
    wrote_bytes: u64,
    /// The newest change the record has been written with, for a look from `session show`.
    writes: u64,
    appends: u64,
    /// A newer app's record is here, so this one is in a folder of its own.
    beside_newer: bool,
    /// What's here couldn't be read, so nothing's to be written over it.
    read_only: bool,
}

/// Why no record came of two slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotsError {
    Newer,
    Damaged,
    Unreadable,
}

/// One state folder's record.
#[derive(Debug)]
pub struct RecordStore {
    root: PathBuf,
    dir: Mutex<PathBuf>,
    /// Held for the process's life, however it ends. `None` if another has it.
    lock: Mutex<Option<File>>,
    lock_path: PathBuf,
    progress: Mutex<Progress>,
    /// Which version this writes, which a test can make it pretend isn't this build's.
    version: u32,
}

/// Which session this is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// The app: the record that restores on launch.
    App,
    /// A session with no window, whose `session save` must not touch the app's.
    Windowless,
}

fn fold_name(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::App => "session",
        Flavor::Windowless => "session-windowless",
    }
}

fn io_failed(what: &str) -> StoreError {
    StoreError::new("write_failed", format!("The state folder couldn't be {what}."))
}

/// Reads a file of ours with a cap. A link, or a file someone else can write, isn't ours, and reads as an error.
fn read_ours(path: &Path, cap: usize) -> Result<Option<Vec<u8>>, ()> {
    local::read_private_file(path, cap as u64).map_err(|_| ())
}

/// The bytes of a file in the folder are written and handed to the drive, with the drive forced to flush them only if asked.
fn sync(file: &File, force: bool) -> std::io::Result<()> {
    if force { file.sync_all() } else { local::hand_to_drive(file) }
}

/// Writes a file through a temp file beside it, so there's never half of one under its name.
fn replace(path: &Path, bytes: &[u8], force: bool) -> std::io::Result<()> {
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let mut file = local::create_private_file(&temp)?;
    let written = file.write_all(bytes).and_then(|()| sync(&file, force));
    match written.and_then(|()| std::fs::rename(&temp, path)) {
        Ok(()) if force => local::sync_folder_of(path),
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// What a journal's file reads as, if it's there.
fn read_journal(path: &Path) -> Result<Option<(ParsedJournal, u64)>, RecordError> {
    let Ok(bytes) = read_ours(path, MAX_JOURNAL_BYTES) else { return Err(RecordError::Damaged("unreadable")) };
    let Some(bytes) = bytes else { return Ok(None) };
    let length = bytes.len() as u64;
    record::parse_journal(&bytes, CURRENT_VERSION).map(|parsed| Some((parsed, length)))
}

/// Whether a journal was saved away: it says a save was about to put a file of this identity in place, and that's what's there. So its edits are in the PDF, and it's stale, whether or not the crash let it be removed.
fn saved_into_its_file(parsed: &ParsedJournal, now: Option<FileStamp>) -> bool {
    let Some(now) = now else { return false };
    parsed.ops.iter().rev().find_map(|op| if let JournalOp::Saving(target) = op { Some(FileStamp::from(*target)) } else { None }).or_else(|| parsed.header.saving.map(FileStamp::from)).is_some_and(|target| target.same_contents(now))
}

/// A list brought back from what a journal's file says.
fn restored_from(parsed: &ParsedJournal) -> Option<RestoredEdits> {
    let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).ok()?;
    let lost = parsed.lost_lines || applied < parsed.ops.len();
    Some(RestoredEdits { edits, seq: parsed.header.base_seq + applied as u64, lost_lines: lost })
}

impl RecordStore {
    /// This state folder's record, for one kind of session. Takes the lock. Reads nothing else.
    pub fn open(paths: &StatePaths, flavor: Flavor) -> Self {
        Self::open_as(paths, flavor, CURRENT_VERSION)
    }

    /// The same, as if this build were of another version. For tests of going forward and back.
    pub fn open_as(paths: &StatePaths, flavor: Flavor, version: u32) -> Self {
        let dir = paths.root.join(fold_name(flavor));
        let lock_path = paths.root.join("run").join(format!("{}.lock", fold_name(flavor)));
        let lock = paths.prepare().ok().and_then(|()| take_lock(&lock_path));
        Self { root: paths.root.clone(), dir: Mutex::new(dir), lock: Mutex::new(lock), lock_path, progress: Mutex::new(Progress::default()), version }
    }

    /// Whether this process is the one keeping this record.
    pub fn holds_the_record(&self) -> bool {
        self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// Asks for the lock again, for a moment. An app that's just gone holds it until the kernel has finished with it, and starting empty beside its own record because of that would lose the very thing an update is for. On a thread that can wait.
    fn wait_for_lock(&self) -> bool {
        let began = Instant::now();
        loop {
            if self.holds_the_record() {
                return true;
            }
            if began.elapsed() >= LOCK_PATIENCE {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
            if let Some(file) = take_lock(&self.lock_path) {
                *self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file);
            }
        }
    }

    fn dir(&self) -> PathBuf {
        self.dir.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn progress(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ensure_dirs(dir: &Path) -> std::io::Result<()> {
        local::ensure_private_dir(dir)?;
        let journals = dir.join(JOURNALS);
        let made = !journals.exists();
        local::ensure_private_dir(&journals)?;
        // Owner, 2026-11-07: history stays out of backups. A failure here only costs that.
        if made {
            let _ = local::exclude_from_backups(&journals);
            let _ = local::hand_folder_to_drive(dir);
        }
        Ok(())
    }

    /// The newer of two slots that reads whole. A slot that doesn't check out is passed over, for the other. Damage is a file that's been read and isn't what it should be; a file that can't be read at all isn't known to be anything, and never gets moved aside as if it were.
    fn read_slots(dir: &Path, newest: u32) -> Result<Option<(DecodedRecord, usize)>, SlotsError> {
        let mut best: Option<(DecodedRecord, usize)> = None;
        let (mut damaged, mut unreadable) = (false, false);
        for (slot, name) in SLOTS.iter().enumerate() {
            let Ok(bytes) = read_ours(&dir.join(name), MAX_RECORD_BYTES) else {
                // A file past the cap reads as this too, and that's damage.
                let too_large = std::fs::metadata(dir.join(name)).is_ok_and(|meta| meta.len() > MAX_RECORD_BYTES as u64);
                if too_large {
                    damaged = true;
                } else {
                    unreadable = true;
                }
                continue;
            };
            let Some(bytes) = bytes else { continue };
            match record::decode_record(&bytes, newest) {
                Ok(decoded) => {
                    if best.as_ref().is_none_or(|(known, _)| decoded.seq > known.seq) {
                        best = Some((decoded, slot));
                    }
                }
                Err(RecordError::Newer(_)) => return Err(SlotsError::Newer),
                Err(_) => damaged = true,
            }
        }
        match best {
            Some(found) => Ok(Some(found)),
            None if unreadable => Err(SlotsError::Unreadable),
            None if damaged => Err(SlotsError::Damaged),
            None => Ok(None),
        }
    }

    /// Looks at what's here, on a thread that can wait. See the crate's rules for what becomes of what.
    pub fn load(&self) -> Loaded {
        if !self.wait_for_lock() {
            return Loaded::OtherInstance;
        }
        *self.progress() = Progress::default();
        let main = self.fold_root();
        *self.dir.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = main.clone();
        let own = self.version_folder(&main, self.version);
        remove_temp_files(&main);
        // Work done on the older app, after going back, comes forward: it's newer than what's here.
        let main_read = Self::read_slots(&main, self.version);
        let came_back = self.bring_forward_from_going_back(&main);
        let main_read = if came_back { Self::read_slots(&main, self.version) } else { main_read };
        match main_read {
            Ok(Some((decoded, slot))) => {
                if decoded.version < self.version {
                    // Going forward: what's here stays as it was, for going back.
                    let kept = self.version_folder(&main, decoded.version);
                    let _ = copy_folder(&main, &kept);
                }
                self.finish_load(&main, Some((decoded, slot)), None)
            }
            Ok(None) => self.finish_load(&main, None, None),
            Err(SlotsError::Newer) => {
                // The newer app's record is never touched. This app has its own place beside it, which also holds what it left when it went forward.
                *self.dir.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = own.clone();
                self.progress().beside_newer = true;
                match Self::read_slots(&own, self.version) {
                    Ok(Some(found)) => self.finish_load(&own, Some(found), Some(record::notices::NEWER_KEPT)),
                    _ => match self.finish_load(&own, None, None) {
                        Loaded::Nothing => Loaded::Newer,
                        other => other,
                    },
                }
            }
            Err(SlotsError::Unreadable) => {
                // Not damage: it's there and this couldn't read it, as a wrong mode or a failing read. It's left exactly as it is.
                self.progress().read_only = true;
                Loaded::Unreadable
            }
            Err(SlotsError::Damaged) => {
                // Kept to one side, a few at most, and only the record: the journals stay, as they're what can still be brought back.
                let aside = self.make_room_for_damaged(&main);
                if local::ensure_private_dir(&aside).is_ok() {
                    for name in SLOTS.iter().copied().chain([PLACES]) {
                        let _ = std::fs::rename(main.join(name), aside.join(name));
                    }
                }
                match self.finish_load(&main, None, Some(record::notices::DAMAGED)) {
                    Loaded::Nothing => Loaded::Damaged,
                    other => other,
                }
            }
        }
    }

    fn fold_root(&self) -> PathBuf {
        let locked = self.lock_path.file_stem().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        self.root.join(locked)
    }

    fn version_folder(&self, main: &Path, version: u32) -> PathBuf {
        self.root.join(format!("{}.v{version}", main.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned())))
    }

    /// Makes room for one more damaged state, the oldest going, and says where it goes.
    fn make_room_for_damaged(&self, main: &Path) -> PathBuf {
        let name = main.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        let slot = |at: usize| if at == 0 { self.root.join(format!("{name}.damaged")) } else { self.root.join(format!("{name}.damaged.{at}")) };
        let _ = std::fs::remove_dir_all(slot(DAMAGED_KEPT - 1));
        for at in (0..DAMAGED_KEPT - 1).rev() {
            let _ = std::fs::rename(slot(at), slot(at + 1));
        }
        slot(0)
    }

    /// If an older version of the app worked here after going back, its folder is newer than what's in the main one. What it left comes across, so going forward again loses none of it.
    fn bring_forward_from_going_back(&self, main: &Path) -> bool {
        for version in 1..self.version {
            let folder = self.version_folder(main, version);
            if !folder.join(WENT_BACK).exists() {
                continue;
            }
            if copy_folder(&folder, main).is_ok() {
                for name in SLOTS.iter().copied().chain([PLACES]) {
                    if !folder.join(name).exists() {
                        let _ = std::fs::remove_file(main.join(name));
                    }
                }
                let _ = std::fs::remove_file(folder.join(WENT_BACK));
                let _ = std::fs::remove_file(main.join(WENT_BACK));
                return true;
            }
        }
        false
    }

    fn finish_load(&self, dir: &Path, found: Option<(DecodedRecord, usize)>, notice: Option<&'static str>) -> Loaded {
        let mut progress = self.progress();
        let had_record = found.is_some();
        let mut record = match found {
            Some((decoded, slot)) => {
                let DecodedRecord { seq, mut record, .. } = decoded;
                progress.seq = seq;
                progress.newest_slot = Some(slot);
                progress.bare_crc = Some(crc_of(&record::without_what_deltas_carry(&record)));
                if let Ok(Some(log)) = read_ours(&dir.join(PLACES), MAX_PLACES_LOG_BYTES as usize * 2) {
                    let whole = record::apply_places_log(&mut record, seq, &log);
                    // A torn tail is cut, or everything added after it would never be read.
                    if whole < log.len() as u64 {
                        let _ = truncate_ours(&dir.join(PLACES), whole);
                    }
                    progress.places_bytes = whole;
                }
                record
            }
            None => SessionRecord { app: AppRecord::default(), view: ViewRecord::default(), find: FindRecord::default(), edit_mode: false, active: None, next_id: 1, tabs: Vec::new() },
        };
        // A PDF is open once, whatever a record says. The journals of tabs that lost out are left as they are, and said.
        let not_merged = record::one_tab_per_file(&mut record);
        let journals = load_journals(dir, &mut record, &mut progress, &not_merged);
        // Nothing's changed by coming back: writing what was read would be a write for nothing.
        progress.whole_crc = had_record.then(|| crc_of(&record));
        drop(progress);
        prune_closed_journals(&dir.join(JOURNALS));
        if !had_record && record.tabs.is_empty() {
            return Loaded::Nothing;
        }
        Loaded::Restored(Box::new(Restored { record, journals, notice }))
    }

    /// Writes journals' waiting operations: removes what's retired first, as one being taken up again starts afresh, then appends. One failing stops only itself: the outcome says what's in the file, for the session to let go of, and the error says what wasn't, for trying again. Nothing that's there and isn't known is truncated or overwritten: it goes under another name beside it. A journal that's past its compaction point is left to [`Self::compact`].
    pub fn write_journals(&self, work: &JournalWork, force: bool) -> (JournalOutcome, Option<StoreError>) {
        let mut outcome = JournalOutcome::default();
        if let Some(error) = self.refuses_writes() {
            outcome.not_retired.clone_from(&work.retired);
            return (outcome, Some(error));
        }
        let dir = self.dir();
        if Self::ensure_dirs(&dir).is_err() {
            outcome.not_retired.clone_from(&work.retired);
            return (outcome, Some(io_failed("written to")));
        }
        self.note_going_back(&dir);
        let mut failed = None;
        let mut progress = self.progress();
        let folder = dir.join(JOURNALS);
        for name in &work.retired {
            if !is_journal_name(name) {
                continue;
            }
            match std::fs::remove_file(folder.join(name)) {
                Ok(()) => {
                    progress.journal_bytes.remove(name);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    progress.journal_bytes.remove(name);
                }
                Err(_) => {
                    outcome.not_retired.push(name.clone());
                    failed = Some(io_failed("written to"));
                }
            }
        }
        if !work.retired.is_empty() {
            let _ = local::hand_folder_to_drive(&folder);
        }
        for append in &work.appends {
            if !is_journal_name(&append.name) {
                failed = Some(StoreError::new("write_failed", "A journal's name isn't one of ours."));
                continue;
            }
            let mut name = append.name.clone();
            let known = progress.journal_bytes.get(&name).copied();
            // Something's at that name that this process hasn't read. It's left exactly as it is.
            if known.is_none() && std::fs::symlink_metadata(folder.join(&name)).is_ok() {
                let Some(free) = free_name(&folder, append) else {
                    failed = Some(io_failed("written to"));
                    continue;
                };
                name = free;
            }
            let mut lines = Vec::new();
            if known.is_none() {
                // New: its header goes first, for as many operations as came before these.
                let header = JournalHeader { base_seq: append.through.saturating_sub(append.ops.len() as u64), ..append.header.clone() };
                lines.extend_from_slice(&record::journal_start(&header, None));
            }
            lines.extend_from_slice(&record::journal_lines(&append.ops));
            let path = folder.join(&name);
            let start = known.unwrap_or(0);
            let result = (|| -> std::io::Result<()> {
                let mut file = local::open_private_for_writing(&path, false)?;
                // A torn tail from a crash, or a write that failed, is cut off, and what's good is built on.
                file.set_len(start)?;
                file.seek(SeekFrom::Start(start))?;
                file.write_all(&lines)?;
                sync(&file, force)
            })();
            if result.is_err() {
                failed = Some(io_failed("written to"));
                continue;
            }
            progress.wrote_bytes += lines.len() as u64;
            progress.appends += 1;
            if append.closing {
                // Closed: no launch reads it now. If this doesn't go, the next launch sees to it.
                progress.journal_bytes.remove(&name);
                if let Some(closed) = closed_journal_name(&name) {
                    let _ = std::fs::rename(&path, folder.join(closed));
                }
            } else {
                progress.journal_bytes.insert(name.clone(), start + lines.len() as u64);
            }
            outcome.written.push((append.tab, append.through, name));
        }
        (outcome, failed)
    }

    /// Why this won't write, if it won't.
    fn refuses_writes(&self) -> Option<StoreError> {
        if !self.holds_the_record() {
            return Some(self.other_instance());
        }
        self.progress().read_only.then(|| StoreError::new("record_unreadable", record::notices::words(record::notices::UNREADABLE).unwrap_or_default()))
    }

    /// Working beside a newer app's record, this folder is what going forward has to bring with it.
    fn note_going_back(&self, dir: &Path) {
        if self.progress().beside_newer && !dir.join(WENT_BACK).exists() {
            let _ = local::write_private_file_unsynced(&dir.join(WENT_BACK), b"1");
        }
    }

    /// Whether a journal's grown past what's worth keeping as it is.
    pub fn should_compact(&self, name: &str) -> bool {
        self.progress().journal_bytes.get(name).is_some_and(|bytes| *bytes > JOURNAL_COMPACT_ABOVE)
    }

    /// Writes a whole list as its journal, for a session that keeps none as it goes.
    pub fn write_checkpoint(&self, name: &str, header: &JournalHeader, base: &record::EditsRecord) -> Result<(), StoreError> {
        if !is_journal_name(name) {
            return Ok(());
        }
        if let Some(error) = self.refuses_writes() {
            return Err(error);
        }
        let dir = self.dir();
        Self::ensure_dirs(&dir).map_err(|_| io_failed("made"))?;
        self.note_going_back(&dir);
        let start = record::journal_start(header, Some(base));
        replace(&dir.join(JOURNALS).join(name), &start, false).map_err(|_| io_failed("written to"))?;
        let mut progress = self.progress();
        progress.journal_bytes.insert(name.to_string(), start.len() as u64);
        progress.wrote_bytes += start.len() as u64;
        Ok(())
    }

    /// Moves a journal's checkpoint up: one line for all of the list as it stands at `seq`. Only if it comes out well under half of what's there, so a rewrite isn't made for nothing. The history within the limit is in it, so nothing's lost by this. In one step, through a temp file.
    pub fn compact(&self, name: &str, header: &JournalHeader, base: &record::EditsRecord) -> Result<bool, StoreError> {
        if !is_journal_name(name) || self.refuses_writes().is_some() {
            return Ok(false);
        }
        let dir = self.dir();
        let start = record::journal_start(header, Some(base));
        let mut progress = self.progress();
        if progress.journal_bytes.get(name).is_none_or(|bytes| start.len() as u64 * 2 > *bytes) {
            return Ok(false);
        }
        replace(&dir.join(JOURNALS).join(name), &start, false).map_err(|_| io_failed("written to"))?;
        progress.journal_bytes.insert(name.to_string(), start.len() as u64);
        progress.wrote_bytes += start.len() as u64;
        Ok(true)
    }

    /// Writes the main record, if it's changed. If only what a line can say has, a line says it. `force` makes the drive flush, for a hand-off. Says how many bytes went.
    pub fn write_record(&self, record: &SessionRecord, delta: &PlaceDelta, force: bool) -> Result<u64, StoreError> {
        if let Some(error) = self.refuses_writes() {
            return Err(error);
        }
        let dir = self.dir();
        Self::ensure_dirs(&dir).map_err(|_| io_failed("made"))?;
        self.note_going_back(&dir);
        let mut progress = self.progress();
        let whole = crc_of(record);
        if progress.whole_crc == Some(whole) {
            return Ok(0);
        }
        let bare = crc_of(&record::without_what_deltas_carry(record));
        // A line, if that's all that's needed and the log has room.
        if progress.bare_crc == Some(bare) && progress.newest_slot.is_some() && progress.places_bytes < MAX_PLACES_LOG_BYTES {
            let delta = PlaceDelta { seq: progress.seq, contents_scroll: record.app.contents_scroll, focus: Some(record.app.focus), ..delta.clone() };
            let line = record::places_line(&delta);
            let path = dir.join(PLACES);
            let appended = (|| -> std::io::Result<()> {
                let mut file = local::open_private_for_writing(&path, true)?;
                file.write_all(&line)?;
                sync(&file, force)
            })();
            if appended.is_ok() {
                progress.places_bytes += line.len() as u64;
                progress.wrote_bytes += line.len() as u64;
                progress.whole_crc = Some(whole);
                progress.writes += 1;
                return Ok(line.len() as u64);
            }
        }
        let seq = progress.seq.saturating_add(1);
        let bytes = record::encode_record(record, seq);
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(StoreError::new("record_too_large", "This session's state is too large to be kept."));
        }
        let slot = progress.newest_slot.map_or(0, |newest| 1 - newest);
        replace(&dir.join(SLOTS[slot]), &bytes, force).map_err(|_| io_failed("written to"))?;
        // That record has everything, so the lines about the one before it are done with.
        let _ = std::fs::remove_file(dir.join(PLACES));
        // A new name in a folder only stays with the folder's own sync.
        let _ = local::hand_folder_to_drive(&dir);
        progress.seq = seq;
        progress.newest_slot = Some(slot);
        progress.whole_crc = Some(whole);
        progress.bare_crc = Some(bare);
        progress.places_bytes = 0;
        progress.wrote_bytes += bytes.len() as u64;
        progress.writes += 1;
        Ok(bytes.len() as u64)
    }

    /// **Makes the drive itself flush everything a launch would read:** the newest record, the log of places, every journal, and the folders their names are in. Earlier writes had only a plain `fsync`, which is what ordinary ones get, so a hand-off has to cover those too, and not just what it wrote itself. The only `F_FULLFSYNC` there is, as it costs time and wear.
    pub fn force_to_drive(&self) -> Result<(), StoreError> {
        let dir = self.dir();
        let progress = self.progress();
        let failed = || io_failed("flushed");
        let force = |path: &Path| -> Result<(), StoreError> {
            match File::open(path) {
                Ok(file) => file.sync_all().map_err(|_| failed()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(_) => Err(failed()),
            }
        };
        if let Some(slot) = progress.newest_slot {
            force(&dir.join(SLOTS[slot]))?;
        }
        force(&dir.join(PLACES))?;
        for name in progress.journal_bytes.keys() {
            force(&dir.join(JOURNALS).join(name))?;
        }
        force(&dir.join(JOURNALS))?;
        force(&dir)?;
        Ok(())
    }

    /// Reads back what's on disk, as a launch would, and compares it with what's in memory: the main record whole, and every tab's list in full. Part of a hand-off.
    pub fn verify(&self, want: &SessionRecord, lists: &HashMap<String, record::EditsRecord>) -> Result<(), StoreError> {
        let dir = self.dir();
        let mismatch = |what: &str| StoreError::new("read_back_differs", format!("What was written doesn't read back the same: {what}."));
        let (decoded, _) = Self::read_slots(&dir, self.version).ok().flatten().ok_or_else(|| mismatch("the main record"))?;
        let DecodedRecord { seq, mut record, .. } = decoded;
        if let Ok(Some(log)) = read_ours(&dir.join(PLACES), MAX_PLACES_LOG_BYTES as usize * 2) {
            record::apply_places_log(&mut record, seq, &log);
        }
        if record::without_what_deltas_carry(&record) != record::without_what_deltas_carry(want) {
            return Err(mismatch("the tabs and settings"));
        }
        if record != *want {
            return Err(mismatch("a place"));
        }
        for (name, list) in lists {
            let path = dir.join(JOURNALS).join(name);
            let parsed = read_journal(&path).map_err(|_| mismatch("a journal"))?.ok_or_else(|| mismatch("a journal that isn't there"))?.0;
            let restored = restored_from(&parsed).ok_or_else(|| mismatch("a journal"))?;
            if restored.lost_lines || restored.edits.to_record() != *list {
                return Err(mismatch("unsaved edits"));
            }
        }
        Ok(())
    }

    /// Makes the next record go out whole, whatever it looks like: a line couldn't be trusted to say it.
    pub fn next_write_whole(&self) {
        let mut progress = self.progress();
        progress.whole_crc = None;
        progress.bare_crc = None;
    }

    /// How much this has written, and how often, for a look.
    pub fn wrote_bytes(&self) -> u64 {
        self.progress().wrote_bytes
    }

    /// Main record writes and journal appends so far.
    pub fn writes(&self) -> (u64, u64) {
        let progress = self.progress();
        (progress.writes, progress.appends)
    }

    /// The main record's bytes and the journals', on disk, as far as this has seen.
    pub fn sizes(&self) -> (u64, u64) {
        let progress = self.progress();
        let dir = self.dir();
        let main = progress.newest_slot.and_then(|slot| std::fs::metadata(dir.join(SLOTS[slot])).ok()).map_or(0, |meta| meta.len());
        (main + progress.places_bytes, progress.journal_bytes.values().sum())
    }

    /// Whether this is writing beside a newer app's record, which it won't touch.
    pub fn beside_newer(&self) -> bool {
        self.progress().beside_newer
    }

    fn other_instance(&self) -> StoreError {
        StoreError::new("other_instance", record::notices::OTHER_INSTANCE)
    }

    /// Removes a journal found to be stale.
    pub fn remove_journal(&self, name: &str) {
        if is_journal_name(name) {
            let _ = std::fs::remove_file(self.dir().join(JOURNALS).join(name));
            self.progress().journal_bytes.remove(name);
        }
    }
}

impl HistorySource for RecordStore {
    /// A PDF being opened: what's left of its history, if any, from when it was closed. On whatever thread is reading the file. It must be about this file, as the name only says it's likely to be: a forged or stale one is left where it is. If it's all there, it's open again.
    fn history_of(&self, stamp: Option<FileStamp>, path: &Path) -> Option<TakenHistory> {
        if self.refuses_writes().is_some() {
            return None;
        }
        let name = record::journal_file_name(stamp, path, None);
        let closed = closed_journal_name(&name)?;
        let folder = self.dir().join(JOURNALS);
        let (parsed, _) = read_journal(&folder.join(&closed)).ok().flatten()?;
        if !journal_is_for(&parsed.header, stamp, path) {
            return None;
        }
        let restored = restored_from(&parsed)?;
        if restored.lost_lines || std::fs::symlink_metadata(folder.join(&name)).is_ok() {
            return None;
        }
        std::fs::rename(folder.join(&closed), folder.join(&name)).ok()?;
        self.progress().journal_bytes.insert(name.clone(), parsed.good_bytes);
        Some(TakenHistory { name, restored })
    }
}

/// A journal's other name, as something's at this one that isn't to be touched.
fn free_name(folder: &Path, append: &recto_session::JournalAppend) -> Option<String> {
    let path = append.header.path.to_path().unwrap_or_default();
    (0..64u32).map(|at| record::journal_file_name(append.header.file.map(Into::into), &path, Some(append.tab.0.saturating_add(at)))).find(|name| std::fs::symlink_metadata(folder.join(name)).is_err())
}

fn truncate_ours(path: &Path, length: u64) -> std::io::Result<()> {
    local::open_private_for_writing(path, false)?.set_len(length)
}

/// Temp files a crash left, of ours by their name, in a folder of ours. Nothing else is touched.
fn remove_temp_files(dir: &Path) {
    for folder in [dir.to_path_buf(), dir.join(JOURNALS)] {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let is_temp = name.to_str().is_some_and(|name| (name.starts_with("record.") || name.starts_with("journal-") || name.starts_with("closed-") || name.starts_with(PLACES)) && name.rsplit('.').next().is_some_and(|tail| tail.starts_with("tmp") && tail[3..].bytes().all(|byte| byte.is_ascii_digit())));
            if is_temp && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Closed PDFs' histories, kept for 30 days within 1 GiB, oldest out first. A look at their sizes and times, with nothing read, when a launch is running anyway: no timer.
fn prune_closed_journals(folder: &Path) {
    let now = std::time::SystemTime::now();
    let mut closed: Vec<(u64, PathBuf, u64)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_str().is_some_and(is_closed_journal_name))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let age = meta.modified().ok().and_then(|modified| now.duration_since(modified).ok()).map_or(0, |age| age.as_secs());
            Some((age, entry.path(), meta.len()))
        })
        .collect();
    closed.sort_unstable_by_key(|(age, _, _)| *age);
    let mut total = 0u64;
    for (age, path, bytes) in closed {
        total += bytes;
        if age > CLOSED_JOURNAL_DAYS * 86_400 || total > MAX_CLOSED_JOURNAL_BYTES {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// What a record comes to, to know it's changed. Encoded with sequence zero, so only what's in it counts.
fn crc_of(record: &SessionRecord) -> u32 {
    let bytes = record::encode_record(record, 0);
    bytes.iter().fold(0x811c_9dc5u32, |value, byte| (value ^ u32::from(*byte)).wrapping_mul(0x0100_0193))
}

/// Copies a folder of ours one level deep, and its journals. For keeping what an older app wrote.
fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    local::ensure_private_dir(to)?;
    local::ensure_private_dir(&to.join(JOURNALS))?;
    for (from, to) in [(from.to_path_buf(), to.to_path_buf()), (from.join(JOURNALS), to.join(JOURNALS))] {
        for entry in std::fs::read_dir(&from)?.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_file())
                && let Some(Ok(Some(bytes))) = entry.file_name().to_str().map(|name| read_ours(&from.join(name), MAX_JOURNAL_BYTES))
            {
                replace(&to.join(entry.file_name()), &bytes, false)?;
            }
        }
    }
    Ok(())
}

/// What's been decided about one journal file.
struct JournalRead {
    name: String,
    path: PathBuf,
    parsed: ParsedJournal,
}

/// **Every `journal-` file in a folder gets one owner, or none.**
///
/// 1. A tab that names it, if it's about that tab's file. A journal for another file never applies, whatever a record says: the tab says so, and the journal's left for its own.
/// 2. Otherwise a tab on the same file with no journal: a first edit whose journal went out before the record next did.
/// 3. Otherwise, with real unsaved edits, it's what a crash leaves before the record had heard of the tab, and it comes back as one.
/// 4. Otherwise it's a closed PDF's history, and goes to `closed-`, where no launch reads it.
///
/// A journal from a newer app is left exactly as it is, and so is what's past the total a launch will read. One that can't be read is kept to one side. One saved away is removed. What couldn't be read of one is copied beside it before anything's cut, and what's left is written as a clean checkpoint, so nothing added after it is lost behind a line replay can't get past.
fn load_journals(dir: &Path, record: &mut SessionRecord, progress: &mut Progress, not_merged: &[String]) -> HashMap<String, RestoredEdits> {
    let folder = dir.join(JOURNALS);
    let mut journals = HashMap::new();
    let mut names: Vec<String> = std::fs::read_dir(&folder).into_iter().flatten().flatten().filter_map(|entry| entry.file_name().to_str().map(str::to_string)).filter(|name| is_journal_name(name)).collect();
    names.sort_unstable();
    let mut read_total = 0u64;
    let mut reads = Vec::new();
    for name in names {
        if not_merged.contains(&name) {
            // A record's other tab on this file: its edits can't be merged, and they aren't lost.
            let _ = std::fs::rename(folder.join(&name), folder.join(&name).with_extension("not-merged"));
            continue;
        }
        let path = folder.join(&name);
        read_total += std::fs::metadata(&path).map_or(0, |meta| meta.len());
        if read_total > MAX_JOURNAL_BYTES_AT_LAUNCH {
            continue;
        }
        match read_journal(&path) {
            Ok(Some((parsed, _))) => reads.push(JournalRead { name, path, parsed }),
            Ok(None) => {}
            // A newer app's: not ours to touch. A tab that names it says it couldn't be read, and writes elsewhere.
            Err(RecordError::Newer(_)) => {}
            Err(RecordError::Damaged(_)) => {
                let _ = std::fs::rename(&path, path.with_extension("unreadable"));
            }
        }
    }
    let mut taken_tabs: Vec<bool> = record.tabs.iter().map(|_| false).collect();
    // First everything a tab names, so a tab on the same file can't take what another has claimed.
    let mut unclaimed = Vec::new();
    for read in reads {
        let named = record.tabs.iter().position(|tab| tab.journal.as_deref() == Some(read.name.as_str()) && journal_is_for(&read.parsed.header, tab.file.map(Into::into), &tab.path.to_path().unwrap_or_default()));
        match named {
            Some(at) if !taken_tabs[at] => {
                taken_tabs[at] = true;
                claim(&folder, read, record, at, progress, &mut journals);
            }
            _ => unclaimed.push(read),
        }
    }
    for read in unclaimed {
        let same_file = record.tabs.iter().enumerate().position(|(at, tab)| !taken_tabs[at] && tab.journal.is_none() && journal_is_for(&read.parsed.header, tab.file.map(Into::into), &tab.path.to_path().unwrap_or_default()));
        if let Some(at) = same_file {
            taken_tabs[at] = true;
            claim(&folder, read, record, at, progress, &mut journals);
            continue;
        }
        let JournalRead { name, path, parsed } = read;
        let Some(about) = parsed.header.path.to_path() else { continue };
        let now = file_stamp(&about).ok();
        if saved_into_its_file(&parsed, now) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let Some(restored) = restored_from(&parsed) else { continue };
        if !restored.edits.is_edited() {
            // Nothing unsaved and no tab: a closed PDF's, which a crash caught before it could say so.
            if let Some(closed) = closed_journal_name(&name) {
                let _ = std::fs::rename(&path, folder.join(closed));
            }
            continue;
        }
        if record.tabs.len() >= recto_session::MAX_TABS || record.next_id >= record::MAX_TAB_ID {
            continue;
        }
        let id = record.next_id;
        record.next_id += 1;
        record.tabs.push(TabEntry { id, path: parsed.header.path.clone(), file: parsed.header.file, page: 0, page_count: parsed.header.file_pages, place: None, view_rotation: 0, toc_open: true, renderer_only: false, selection: None, notice: None, journal: Some(name.clone()), edits: Some(restored.edits.summary()) });
        taken_tabs.push(true);
        if record.active.is_none() {
            record.active = Some(id);
        }
        let at = record.tabs.len() - 1;
        claim(&folder, JournalRead { name, path, parsed }, record, at, progress, &mut journals);
    }
    // A tab naming a journal that's gone to no one keeps saying so: it's restored with that noted.
    for (at, tab) in record.tabs.iter_mut().enumerate() {
        if !taken_tabs[at] && tab.journal.take().is_some() {
            tab.edits = None;
            tab.notice = Some(FileNotice::EditsUnreadable);
        }
    }
    journals
}

/// A journal has its tab. If it was saved away, it's stale and goes. If any of it couldn't be used, a copy is kept and what's left is written clean.
fn claim(folder: &Path, read: JournalRead, record: &mut SessionRecord, at: usize, progress: &mut Progress, journals: &mut HashMap<String, RestoredEdits>) {
    let JournalRead { name, path, parsed } = read;
    let tab = &mut record.tabs[at];
    let now = tab.path.to_path().and_then(|path| file_stamp(&path).ok());
    if saved_into_its_file(&parsed, now) {
        // Its edits went into the PDF, and only the confirming didn't happen.
        let _ = std::fs::remove_file(&path);
        tab.journal = None;
        tab.edits = None;
        tab.file = now.map(Into::into);
        tab.notice = None;
        return;
    }
    let Some(mut restored) = restored_from(&parsed) else {
        let _ = std::fs::rename(&path, path.with_extension("unreadable"));
        tab.journal = None;
        tab.edits = None;
        tab.notice = Some(FileNotice::EditsUnreadable);
        return;
    };
    let mut good_bytes = parsed.good_bytes;
    if restored.lost_lines {
        // What couldn't be used is kept, as the notice says it is. Then there's a clean start: a line replay can't pass would otherwise hide everything added after it, and every hand-off would fail against it.
        let _ = std::fs::copy(&path, path.with_extension("kept"));
        let header = JournalHeader { base_seq: restored.seq, ..parsed.header.clone() };
        let start = record::journal_start(&header, Some(&restored.edits.to_record()));
        if replace(&path, &start, false).is_ok() {
            good_bytes = start.len() as u64;
            progress.wrote_bytes += start.len() as u64;
        }
        tab.notice = Some(FileNotice::EditsUnreadable);
        restored.lost_lines = false;
    }
    tab.journal = Some(name.clone());
    tab.edits = Some(restored.edits.summary());
    let _ = folder;
    progress.journal_bytes.insert(name.clone(), good_bytes);
    journals.insert(name, restored);
}

/// The code of what to tell the reader about the record itself.
pub fn notice_for(loaded: &Loaded) -> Option<&'static str> {
    match loaded {
        Loaded::Nothing => None,
        Loaded::Restored(restored) => restored.notice,
        Loaded::Damaged => Some(record::notices::DAMAGED),
        Loaded::Unreadable => Some(record::notices::UNREADABLE),
        Loaded::Newer => Some(record::notices::NEWER),
        Loaded::OtherInstance => Some(record::notices::OTHER_INSTANCE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_session::TabId;
    use recto_session::record::{FileRecord, MAIN_KIND, PathField};

    fn state() -> (PathBuf, StatePaths) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!("rs{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&root);
        let paths = StatePaths::overridden(&root).unwrap();
        paths.prepare().unwrap();
        (root, paths)
    }

    fn sample(tabs: u32) -> SessionRecord {
        SessionRecord {
            app: AppRecord::default(),
            view: ViewRecord::default(),
            find: FindRecord::default(),
            edit_mode: false,
            active: (tabs > 0).then_some(1),
            next_id: tabs + 1,
            tabs: (1..=tabs).map(|id| TabEntry { id, path: PathField::Text(format!("/d/{id}.pdf")), file: None, page: 0, page_count: 5, place: None, view_rotation: 0, toc_open: true, renderer_only: false, selection: None, notice: None, journal: None, edits: None }).collect(),
        }
    }

    fn loaded_record(store: &RecordStore) -> SessionRecord {
        match store.load() {
            Loaded::Restored(restored) => restored.record,
            other => panic!("{other:?}"),
        }
    }

    fn place() -> record::PlaceRecord {
        record::PlaceRecord { page: 3, anchor_page: 3, within: 0.25, x: 1.0, y: 2.0, document_height: 900.0, zoom: 1.5, page_width: 612.0, page_height: 792.0 }
    }

    /// Failure: a place turning over rewrites a thousand tabs' record, or nothing having changed writes something. A line is all it takes, nothing at all when nothing moved, and what's read back has the place in it.
    #[test]
    fn a_place_is_one_line_and_no_change_is_no_write() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let mut record = sample(1000);
        let first = store.write_record(&record, &PlaceDelta::default(), false).unwrap();
        assert!(first > 10_000, "{first}");
        assert_eq!(store.write_record(&record, &PlaceDelta::default(), false).unwrap(), 0, "nothing changed");
        record.tabs[0].page = 3;
        record.tabs[0].place = Some(place());
        let delta = PlaceDelta { places: vec![(1, 3, Some(place()))], active: Some(1), ..PlaceDelta::default() };
        let line = store.write_record(&record, &delta, false).unwrap();
        assert!(line < 512, "{line} bytes for a place");
        drop(store);
        let again = RecordStore::open(&paths, Flavor::App);
        assert_eq!(loaded_record(&again), record);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a crash mid-write of the main record leaves nothing to read. Whatever happens to the newest slot, the one before it is whole, and it's what's read: the old or the new, never a broken one, at every byte of a torn write.
    #[test]
    fn a_torn_write_leaves_the_record_before_it() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let mut record = sample(2);
        store.write_record(&record, &PlaceDelta::default(), false).unwrap();
        let older = record.clone();
        record.edit_mode = true;
        store.write_record(&record, &PlaceDelta::default(), false).unwrap();
        drop(store);
        let newest = root.join("session").join(SLOTS[1]);
        let whole = std::fs::read(&newest).unwrap();
        for cut in (0..whole.len()).step_by(7) {
            std::fs::write(&newest, &whole[..cut]).unwrap();
            let store = RecordStore::open(&paths, Flavor::App);
            assert_eq!(loaded_record(&store), older, "cut at {cut}");
        }
        std::fs::write(&newest, &whole).unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        assert_eq!(loaded_record(&store), record);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: damage to both slots is started over on top of, or panics. What's there is kept to one side, the app starts empty and says so.
    #[test]
    fn a_damaged_record_is_kept_and_said() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        store.write_record(&sample(2), &PlaceDelta::default(), false).unwrap();
        drop(store);
        std::fs::write(root.join("session").join(SLOTS[0]), b"garbage").unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let loaded = store.load();
        assert!(matches!(loaded, Loaded::Damaged), "{loaded:?}");
        assert_eq!(notice_for(&loaded), Some(record::notices::DAMAGED));
        assert_eq!(std::fs::read(root.join("session.damaged").join(SLOTS[0])).unwrap(), b"garbage");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: an older app overwrites a newer app's record, so going forward again finds it gone. Or it has nowhere to keep its own work, so what's done on the older app is lost. Left exactly as it was, byte for byte, and this one works beside it.
    #[test]
    fn a_newer_record_is_never_touched() {
        let (root, paths) = state();
        let main = root.join("session");
        local::ensure_private_dir(&main).unwrap();
        let newer = record::seal_as(MAIN_KIND, CURRENT_VERSION + 1, 9, b"from the future");
        local::write_private_file(&main.join(SLOTS[0]), &newer).unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let loaded = store.load();
        assert!(matches!(loaded, Loaded::Newer));
        assert_eq!(notice_for(&loaded), Some(record::notices::NEWER));
        store.write_record(&sample(1), &PlaceDelta::default(), false).unwrap();
        assert_eq!(std::fs::read(main.join(SLOTS[0])).unwrap(), newer, "not touched");
        assert!(!main.join(SLOTS[1]).exists(), "nor beside it");
        assert!(store.beside_newer());
        // And what was written is what it reads next time.
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert_eq!(restored.notice, Some(record::notices::NEWER_KEPT));
        assert_eq!(restored.record.tabs.len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: going forward overwrites the older app's record with one it can't read, so an update that goes back has nothing. Pretending to be the next version: the record is read and carried forward, and a copy is left as it was, which is what the older one reads after going back.
    #[test]
    fn going_forward_keeps_what_going_back_needs() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        store.write_record(&sample(3), &PlaceDelta::default(), false).unwrap();
        let v1 = std::fs::read(root.join("session").join(SLOTS[0])).unwrap();
        drop(store);
        let next = RecordStore::open_as(&paths, Flavor::App, CURRENT_VERSION + 1);
        assert_eq!(loaded_record(&next).tabs.len(), 3, "migrated");
        assert_eq!(std::fs::read(root.join(format!("session.v{CURRENT_VERSION}")).join(SLOTS[0])).unwrap(), v1, "kept");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a second app on the same folder reads or writes the record that the first is keeping, so one overwrites what the other's user just did.
    #[test]
    fn one_process_keeps_a_record() {
        let (root, paths) = state();
        let first = RecordStore::open(&paths, Flavor::App);
        assert!(first.holds_the_record());
        let second = RecordStore::open(&paths, Flavor::App);
        assert!(!second.holds_the_record());
        assert!(matches!(second.load(), Loaded::OtherInstance));
        assert_eq!(second.write_record(&sample(1), &PlaceDelta::default(), false).unwrap_err().code, "other_instance");
        assert!(second.write_journals(&JournalWork::default(), false).1.is_some());
        let windowless = RecordStore::open(&paths, Flavor::Windowless);
        assert!(windowless.holds_the_record(), "a session with no window has its own");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: the new app comes up while the old one's still on its way out, holding the lock for a moment more, and starts empty beside its own record, which is exactly what an update mustn't do. It waits for a moment. Someone else really holding it isn't waited on for long.
    #[test]
    fn a_launch_waits_a_moment_for_the_app_going_out() {
        let (root, paths) = state();
        let first = RecordStore::open(&paths, Flavor::App);
        first.write_record(&sample(2), &PlaceDelta::default(), false).unwrap();
        let second = RecordStore::open(&paths, Flavor::App);
        assert!(!second.holds_the_record());
        let going = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            drop(first);
        });
        assert_eq!(loaded_record(&second).tabs.len(), 2);
        assert!(second.holds_the_record());
        going.join().unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn journal_work_for(name: &str, ops: Vec<JournalOp>, through: u64) -> JournalWork {
        let header = JournalHeader::new(1, Path::new("/d/1.pdf"), None, 5, 0);
        JournalWork { retired: Vec::new(), appends: vec![recto_session::JournalAppend { tab: TabId(1), name: name.to_string(), header, through, ops, closing: false }] }
    }

    fn rotation() -> JournalOp {
        JournalOp::Rotate(0, 1, 1, record::AuthorName::Owner)
    }

    /// Failure: an edit rewrites its journal, so a burst costs the square of its size. A hundred steps come to about a hundred lines' worth, whether written together or apart. And what's cut off a torn tail is cut before the next line goes on, so the file reads through.
    #[test]
    fn a_journal_is_only_ever_appended_to() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let name = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
        let mut through = 0;
        for _ in 0..100 {
            through += 1;
            assert!(store.write_journals(&journal_work_for(&name, vec![rotation()], through), false).1.is_none());
        }
        let path = root.join("session").join(JOURNALS).join(&name);
        let size = std::fs::metadata(&path).unwrap().len();
        assert_eq!(store.wrote_bytes(), size, "not a byte more than the file holds");
        assert!(size < 100 * 40 + 400, "{size}");
        // A torn tail, and then more work: the tear goes, and the rest is whole.
        drop(store);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"0123abcd {\"r\":[");
        std::fs::write(&path, &bytes).unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let mut record = sample(1);
        record.tabs[0].journal = Some(name.clone());
        store.write_record(&record, &PlaceDelta::default(), false).unwrap();
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        let list = &restored.journals[&name];
        assert!(!list.lost_lines, "a torn tail is what a crash does");
        assert_eq!(list.seq, 100);
        assert!(store.write_journals(&journal_work_for(&name, vec![rotation()], 101), false).1.is_none());
        let (parsed, _) = read_journal(&path).unwrap().unwrap();
        assert_eq!(parsed.ops.len(), 101);
        assert!(!parsed.lost_lines);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a crash at any step of a save brings back the wrong thing. Before the file's in place: everything's as it was, and the journal stands. After it's in place, before it's confirmed: the journal is known to be stale, by the identity it noted ahead of it, and goes, so old history isn't laid on a file that already has it. A journal that's noted a save that didn't happen stands.
    #[test]
    fn a_crash_at_any_step_of_a_save_keeps_or_ends_the_history_rightly() {
        let (root, paths) = state();
        let file = root.join("doc.pdf");
        std::fs::write(&file, b"before").unwrap();
        let before = file_stamp(&file).unwrap();
        let name = record::journal_file_name(Some(before), &file, None);
        // What the save will put in place: made first, so its identity is known.
        let temp = root.join("doc.saving");
        std::fs::write(&temp, b"after, and longer").unwrap();
        let target = file_stamp(&temp).unwrap();
        let tab = |record: &mut SessionRecord| {
            record.tabs[0].path = PathField::from_path(&file);
            record.tabs[0].file = Some(FileRecord::from(before));
            record.tabs[0].journal = Some(name.clone());
        };
        let write = |store: &RecordStore| {
            let header = JournalHeader::new(1, &file, Some(before), 5, 0);
            let work = JournalWork { retired: Vec::new(), appends: vec![recto_session::JournalAppend { tab: TabId(1), name: name.clone(), header, through: 2, ops: vec![rotation(), JournalOp::Saving(target.into())], closing: false }] };
            assert!(store.write_journals(&work, false).1.is_none());
            let mut record = sample(1);
            tab(&mut record);
            store.write_record(&record, &PlaceDelta::default(), false).unwrap();
        };
        let store = RecordStore::open(&paths, Flavor::App);
        write(&store);
        drop(store);
        // Crash after noting it, with the file not yet in place.
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert!(restored.journals[&name].edits.is_edited(), "the save didn't happen, so the edits stand");
        drop(store);
        // Crash with it in place, before the confirming call.
        std::fs::rename(&temp, &file).unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert!(restored.journals.is_empty(), "it's in the PDF, so the journal is stale");
        assert_eq!(restored.record.tabs[0].journal, None);
        assert_eq!(restored.record.tabs[0].file, Some(file_stamp(&file).unwrap().into()));
        assert!(!root.join("session").join(JOURNALS).join(&name).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a crash after a tab's first edit, before the record next went out, loses the tab and with it the edits, though its journal is there. It comes back as a tab. And a closed PDF's history doesn't come back as one: it waits to be asked for.
    #[test]
    fn a_journal_the_record_hasnt_heard_of_comes_back() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        store.write_record(&sample(1), &PlaceDelta::default(), false).unwrap();
        let name = record::journal_file_name(None, Path::new("/d/new.pdf"), None);
        let header = JournalHeader::new(9, Path::new("/d/new.pdf"), None, 5, 0);
        let work = JournalWork { retired: Vec::new(), appends: vec![recto_session::JournalAppend { tab: TabId(9), name: name.clone(), header, through: 1, ops: vec![rotation()], closing: false }] };
        assert!(store.write_journals(&work, false).1.is_none());
        let closed = record::journal_file_name(None, Path::new("/d/closed.pdf"), None);
        let header = JournalHeader::new(8, Path::new("/d/closed.pdf"), None, 5, 0);
        let work = JournalWork { retired: Vec::new(), appends: vec![recto_session::JournalAppend { tab: TabId(8), name: closed.clone(), header, through: 2, ops: vec![JournalOp::Rotate(0, 1, 1, record::AuthorName::Agent), JournalOp::Undo], closing: false }] };
        assert!(store.write_journals(&work, false).1.is_none());
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert_eq!(restored.record.tabs.len(), 2, "{:?}", restored.record.tabs);
        assert_eq!(restored.record.tabs[1].journal.as_deref(), Some(name.as_str()));
        assert!(restored.journals.contains_key(&name));
        assert!(!restored.journals.contains_key(&closed), "a closed PDF's history");
        assert!(root.join("session").join(JOURNALS).join(record::closed_journal_name(&closed).unwrap()).exists(), "kept for when it's opened, under a name no launch reads");
        assert!(record::validate_record(&restored.record).is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: reading back doesn't catch what was written differing from what's in memory, so an updater goes ahead on state that won't come back: a changed place, an edit that didn't reach its journal, a record that's gone.
    #[test]
    fn reading_back_notices_a_difference() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let record = sample(2);
        store.write_record(&record, &PlaceDelta::default(), true).unwrap();
        assert!(store.verify(&record, &HashMap::new()).is_ok());
        let mut other = record.clone();
        other.tabs[0].page = 1;
        assert_eq!(store.verify(&other, &HashMap::new()).unwrap_err().code, "read_back_differs");
        let list = PageEdits::new(5).to_record();
        let lists = HashMap::from([(record::journal_file_name(None, Path::new("/d/1.pdf"), None), list)]);
        assert_eq!(store.verify(&record, &lists).unwrap_err().code, "read_back_differs", "a journal that isn't there");
        std::fs::remove_dir_all(root.join("session")).unwrap();
        assert_eq!(store.verify(&record, &HashMap::new()).unwrap_err().code, "read_back_differs");
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn write_ok(store: &RecordStore, work: &JournalWork) -> JournalOutcome {
        let (outcome, failed) = store.write_journals(work, false);
        assert_eq!(failed, None);
        outcome
    }

    /// Failure (review B6): one journal's failing throws away what the others got, so they go out again and again, and a page deleted once is deleted three times on the next launch. Or the failing one comes first and starves the rest. Each is on its own: the good one is written once, and only the other's asked for again.
    #[test]
    fn a_journal_that_fails_holds_up_only_itself() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let good = record::journal_file_name(None, Path::new("/d/good.pdf"), None);
        let bad = record::journal_file_name(None, Path::new("/d/bad.pdf"), None);
        let append = |tab: u32, name: &str, path: &str| recto_session::JournalAppend { tab: TabId(tab), name: name.to_string(), header: JournalHeader::new(tab, Path::new(path), None, 5, 0), through: 1, ops: vec![rotation()], closing: false };
        // Known as a journal, and something that can't be opened as a file is in its place: a folder does it, as a real ENOSPC or EIO would.
        write_ok(&store, &journal_work_for(&bad, vec![rotation()], 1));
        let folder = root.join("session").join(JOURNALS);
        std::fs::remove_file(folder.join(&bad)).unwrap();
        std::fs::create_dir(folder.join(&bad)).unwrap();
        let work = JournalWork { retired: Vec::new(), appends: vec![append(9, &bad, "/d/bad.pdf"), append(1, &good, "/d/good.pdf")] };
        let (outcome, failed) = store.write_journals(&work, false);
        assert!(failed.is_some());
        assert_eq!(outcome.written.len(), 1, "the good one went");
        assert_eq!(outcome.written[0].0, TabId(1));
        let lines = std::fs::read(folder.join(&good)).unwrap();
        assert_eq!(record::parse_journal(&lines, CURRENT_VERSION).unwrap().ops.len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review B2, B5, T15): an append to a name this process hasn't read truncates what's there: a journal the launch couldn't parse, or a newer app's, and whatever was in it is gone. It goes under another name beside it, and what was there is byte for byte as it was.
    #[test]
    fn what_isnt_known_is_never_written_over() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let name = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
        let folder = root.join("session").join(JOURNALS);
        write_ok(&store, &journal_work_for("journal-0000000000000000.log", vec![rotation()], 1));
        local::write_private_file(&folder.join(&name), b"from a newer app, or anything else").unwrap();
        let outcome = write_ok(&store, &journal_work_for(&name, vec![rotation()], 1));
        assert_eq!(std::fs::read(folder.join(&name)).unwrap(), b"from a newer app, or anything else");
        let went_to = &outcome.written[0].2;
        assert_ne!(went_to, &name);
        assert!(record::is_journal_name(went_to));
        assert_eq!(record::parse_journal(&std::fs::read(folder.join(went_to)).unwrap(), CURRENT_VERSION).unwrap().ops.len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T8): a record's tab applies a journal about another file. Proved with one for `/etc/hosts`: its list went on a 6-page tab. The tab says it couldn't, and what's in the journal is left alone.
    #[test]
    fn a_tab_never_applies_a_journal_about_another_file() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let mut saved = sample(1);
        let was = FileStamp { size: 10, mtime_ns: 20, device: 1, inode: 30 };
        saved.tabs[0].file = Some(was.into());
        let name = record::journal_file_name(Some(was), Path::new("/d/1.pdf"), None);
        saved.tabs[0].journal = Some(name.clone());
        store.write_record(&saved, &PlaceDelta::default(), false).unwrap();
        let folder = root.join("session").join(JOURNALS);
        let other = JournalHeader::new(4, Path::new("/etc/hosts"), Some(FileStamp { size: 99, ..was }), 100, 0);
        let mut bytes = record::journal_start(&other, None);
        bytes.extend_from_slice(&record::journal_lines(&[JournalOp::Undo]));
        local::write_private_file(&folder.join(&name), &bytes).unwrap();
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert!(!restored.journals.contains_key(&name));
        assert_eq!(restored.record.tabs[0].journal, None);
        assert_eq!(restored.record.tabs[0].notice, Some(FileNotice::EditsUnreadable));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T15): a line that can't be used in the middle of a journal: what's past it is cut by the next append though the notice says it was kept, or new edits go after it, where replay never reaches them, and every hand-off fails against it. A copy is kept, what's left is written clean, it reads back the same as what's in memory, and the next edit is reachable.
    #[test]
    fn a_journal_with_a_bad_line_is_kept_and_goes_on() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let name = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
        let mut saved = sample(1);
        saved.tabs[0].journal = Some(name.clone());
        store.write_record(&saved, &PlaceDelta::default(), false).unwrap();
        let folder = root.join("session").join(JOURNALS);
        let path = folder.join(&name);
        // A good step, an operation that won't apply, and another good one behind it.
        let mut bytes = record::journal_start(&JournalHeader::new(1, Path::new("/d/1.pdf"), None, 5, 0), None);
        bytes.extend_from_slice(&record::journal_lines(&[rotation(), JournalOp::Rotate(99, 1, 1, record::AuthorName::Owner), rotation()]));
        local::write_private_file(&path, &bytes).unwrap();
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert_eq!(restored.record.tabs[0].notice, Some(FileNotice::EditsUnreadable));
        assert_eq!(std::fs::read(path.with_extension("kept")).unwrap(), bytes, "as the notice says");
        let list = &restored.journals[&name];
        assert_eq!(list.edits.slots()[0].quarter_turns, 1, "what could be used");
        let lists = HashMap::from([(name.clone(), list.edits.to_record())]);
        let parsed = read_journal(&path).unwrap().unwrap().0;
        assert_eq!(restored_from(&parsed).unwrap().edits.to_record(), lists[&name], "it reads back the same");
        assert!(!parsed.lost_lines);
        assert!(store.write_journals(&journal_work_for(&name, vec![rotation()], list.seq + 1), false).1.is_none());
        let parsed = read_journal(&path).unwrap().unwrap().0;
        let replayed = restored_from(&parsed).unwrap();
        assert_eq!(replayed.edits.slots()[0].quarter_turns, 2, "and the next one is reachable");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T16): a torn tail on the log of places hides every line added after it until the next whole record, so page 17 came back as page 5. It's cut at the launch that finds it.
    #[test]
    fn a_torn_log_of_places_is_cut() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let mut saved = sample(1);
        store.write_record(&saved, &PlaceDelta::default(), false).unwrap();
        drop(store);
        let log = root.join("session").join(PLACES);
        local::write_private_file(&log, b"0123abcd {\"seq\":1,\"pla").unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        assert_eq!(loaded_record(&store), saved);
        saved.tabs[0].page = 3;
        saved.tabs[0].place = Some(place());
        let delta = PlaceDelta { places: vec![(1, 3, Some(place()))], active: Some(1), ..PlaceDelta::default() };
        store.write_record(&saved, &delta, false).unwrap();
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        assert_eq!(loaded_record(&store).tabs[0].page, 3, "the line after the tear");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T14): a record that can't be read just now, as a wrong mode, counts as damage: it was moved aside, journals and all, and a new one started over it. It isn't touched, nothing's written, and it says so.
    #[test]
    fn a_record_that_cant_be_read_isnt_taken_for_damage() {
        use std::os::unix::fs::PermissionsExt;
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        store.write_record(&sample(1), &PlaceDelta::default(), false).unwrap();
        drop(store);
        let slot = root.join("session").join(SLOTS[0]);
        std::fs::set_permissions(&slot, std::fs::Permissions::from_mode(0o666)).unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let loaded = store.load();
        assert!(matches!(loaded, Loaded::Unreadable), "{loaded:?}");
        assert_eq!(notice_for(&loaded), Some(record::notices::UNREADABLE));
        assert!(slot.exists() && !root.join("session.damaged").exists());
        assert_eq!(store.write_record(&sample(2), &PlaceDelta::default(), false).unwrap_err().code, "record_unreadable");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T14): a damaged record takes unsaved edits with it, as the whole folder moved aside. And an earlier damaged state goes. The journals stay and come back, and a few damaged states are kept.
    #[test]
    fn damage_to_the_record_doesnt_lose_the_journals() {
        let (root, paths) = state();
        for round in 0..2 {
            let store = RecordStore::open(&paths, Flavor::App);
            let _ = store.load();
            store.next_write_whole();
            store.write_record(&sample(1), &PlaceDelta::default(), false).unwrap();
            let name = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
            assert!(store.write_journals(&journal_work_for(&name, vec![rotation()], 1 + round), false).1.is_none());
            drop(store);
            std::fs::write(root.join("session").join(SLOTS[0]), format!("garbage {round}")).unwrap();
            let store = RecordStore::open(&paths, Flavor::App);
            let Loaded::Restored(restored) = store.load() else { panic!() };
            assert_eq!(restored.notice, Some(record::notices::DAMAGED));
            assert_eq!(restored.record.tabs.len(), 1, "the edits' tab");
            assert!(restored.journals.contains_key(&name));
        }
        assert_eq!(std::fs::read(root.join("session.damaged").join(SLOTS[0])).unwrap(), b"garbage 1");
        assert_eq!(std::fs::read(root.join("session.damaged.1").join(SLOTS[0])).unwrap(), b"garbage 0");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T5): work done on the older app after going back never comes forward, as the next version reads its own folder, and an update loses it. Pretending to be the next version again: it's what's restored, with its journals.
    #[test]
    fn work_done_after_going_back_comes_forward() {
        let (root, paths) = state();
        let main = root.join("session");
        local::ensure_private_dir(&main).unwrap();
        // The newer app's record is here, so this one works beside it.
        local::write_private_file(&main.join(SLOTS[0]), &record::seal_as(record::MAIN_KIND, CURRENT_VERSION + 1, 9, b"{}")).unwrap();
        let older = RecordStore::open(&paths, Flavor::App);
        assert!(matches!(older.load(), Loaded::Newer));
        older.write_record(&sample(4), &PlaceDelta::default(), false).unwrap();
        let name = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
        assert!(older.write_journals(&journal_work_for(&name, vec![rotation()], 1), false).1.is_none());
        drop(older);
        let next = RecordStore::open_as(&paths, Flavor::App, CURRENT_VERSION + 1);
        // Its own record is replaced by what was done since, which reads through this version's decoder.
        let loaded = loaded_record(&next);
        assert_eq!(loaded.tabs.len(), 4, "the journal goes with the tab on its file");
        assert!(loaded.tabs.iter().any(|tab| tab.journal.as_deref() == Some(name.as_str())), "with what it was editing");
        assert!(!root.join(format!("session.v{CURRENT_VERSION}")).join(WENT_BACK).exists(), "and that's done with");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T3): `F_FULLFSYNC` only covers what a pass wrote, so an edit written a moment earlier never gets it. There's no way to see a drive's cache from here, so this holds what can be seen: it opens each and says if any refuses.
    #[test]
    fn a_forced_flush_covers_everything_a_launch_reads() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        store.write_record(&sample(1), &PlaceDelta::default(), false).unwrap();
        let name = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
        assert!(store.write_journals(&journal_work_for(&name, vec![rotation()], 1), false).1.is_none());
        store.force_to_drive().unwrap();
        // A journal that's gone shows the walk really goes through them: nothing to flush isn't an error. One it can't open is.
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("session").join(JOURNALS).join(&name);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(store.force_to_drive().unwrap_err().code, "write_failed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T21): a place line goes out on every launch with nothing changed, and temp files a crash left are there for ever. Coming back writes nothing, and they go.
    #[test]
    fn coming_back_writes_nothing_and_clears_up() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        store.write_record(&sample(2), &PlaceDelta::default(), false).unwrap();
        drop(store);
        let temp = root.join("session").join("record.a.tmp4242");
        let not_ours = root.join("session").join("notes.tmp1");
        std::fs::write(&temp, b"x").unwrap();
        std::fs::write(&not_ours, b"x").unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let back = loaded_record(&store);
        assert!(!temp.exists());
        assert!(not_ours.exists());
        assert_eq!(store.write_record(&back, &PlaceDelta::default(), false).unwrap(), 0, "nothing's changed");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a closed PDF's history is kept for ever, or a launch reads it. 30 days and 1 GiB, by the owner, by a look at each one's size and time.
    #[test]
    fn closed_histories_are_pruned_by_age() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        store.write_record(&sample(1), &PlaceDelta::default(), false).unwrap();
        let name = record::journal_file_name(None, Path::new("/d/old.pdf"), None);
        let closed = record::closed_journal_name(&name).unwrap();
        let folder = root.join("session").join(JOURNALS);
        local::write_private_file(&folder.join(&closed), b"not even a journal: no launch reads it").unwrap();
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        assert!(matches!(store.load(), Loaded::Restored(_)));
        assert!(folder.join(&closed).exists(), "young, and unread");
        let old = std::time::SystemTime::now() - Duration::from_secs(31 * 86_400);
        File::options().write(true).open(folder.join(&closed)).unwrap().set_modified(old).unwrap();
        prune_closed_journals(&folder);
        assert!(!folder.join(&closed).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: the record isn't private: anyone on this Mac can read which files a person has open and what they typed in Find. Every file and folder is theirs alone.
    #[test]
    fn everything_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        store.write_record(&sample(1), &PlaceDelta::default(), false).unwrap();
        let name = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
        assert!(store.write_journals(&journal_work_for(&name, vec![rotation()], 1), false).1.is_none());
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(root.join("session")), 0o700);
        assert_eq!(mode(root.join("session").join(JOURNALS)), 0o700);
        assert_eq!(mode(root.join("session").join(SLOTS[0])), 0o600);
        assert_eq!(mode(root.join("session").join(JOURNALS).join(&name)), 0o600);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
