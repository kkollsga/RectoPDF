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
use recto_session::edit::{AddedSource, PageEdits};
use recto_session::record::{
    self, AppRecord, CURRENT_VERSION, DecodedRecord, FileNotice, FindRecord, JOURNAL_COMPACT_ABOVE, JournalHeader, JournalOp, MAX_JOURNAL_BYTES,
    MAX_PLACES_LOG_BYTES, MAX_RECORD_BYTES, ParsedJournal, PlaceDelta, RecordError, SessionRecord, TabEntry, ViewRecord, closed_journal_name,
    is_closed_journal_name, is_journal_name, journal_is_for, journal_named_is_for,
};
use recto_session::sleep::{MAX_SLEEP_FILE_BYTES, SleepFile, decode_sleep, encode_sleep};
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
/// Sleep files in all. About 256 tabs' worth of a whole picture, and more with text, which packs well.
const MAX_SLEEP_BYTES: u64 = 256 * 1024 * 1024;
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
    let Ok(bytes) = read_ours(path, MAX_JOURNAL_BYTES) else {
        return Err(RecordError::Damaged("unreadable"));
    };
    let Some(bytes) = bytes else { return Ok(None) };
    let length = bytes.len() as u64;
    record::parse_journal(&bytes, CURRENT_VERSION).map(|parsed| Some((parsed, length)))
}

/// Whether a journal was saved away: it says a save was about to put a file of this identity in place, and that's what's there. So its edits are in the PDF, and it's stale, whether or not the crash let it be removed.
fn saved_into_its_file(parsed: &ParsedJournal, now: Option<FileStamp>) -> bool {
    let Some(now) = now else { return false };
    parsed
        .ops
        .iter()
        .rev()
        .find_map(|op| {
            if let JournalOp::Saving(target) = op {
                Some(FileStamp::from(*target))
            } else {
                None
            }
        })
        .or_else(|| parsed.header.saving.map(FileStamp::from))
        .is_some_and(|target| target.same_contents(now))
}

/// A list brought back from what a journal's file says.
fn restored_from(parsed: &ParsedJournal) -> Option<RestoredEdits> {
    let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).ok()?;
    let lost = parsed.lost_lines || applied < parsed.ops.len();
    Some(RestoredEdits {
        edits,
        seq: parsed.header.base_seq + applied as u64,
        lost_lines: lost,
        rolled_off: false,
    })
}

/// Whether a file kept for a history is the one it was. A sleeping history can be gone back to months on, by which time this file's a stranger.
fn kept_file_is_there(added: &AddedSource) -> bool {
    added
        .stamp
        .is_some_and(|want| file_stamp(&added.path).is_ok_and(|now| now.same_contents(want) && now.inode == want.inode))
}

/// Takes off the steps that need a kept file that isn't what it was, so undo stops short of them rather than undoing wrongly. A look at each, which are few. Says whether any went.
fn roll_off_what_isnt_kept(edits: &mut PageEdits) -> bool {
    let gone: Vec<u16> = edits
        .kept_sources()
        .filter(|(_, added)| !kept_file_is_there(added))
        .map(|(source, _)| source)
        .collect();
    let mut rolled = 0;
    for source in gone {
        rolled += edits.roll_off_kept(source);
    }
    rolled > 0
}

/// A journal whose save happened and was never confirmed, as a crash came in between. Its page order is in the file, so what a confirmed save would have done is done here, and the history goes on from that file under its new name, with the old one gone. `None` if there's no history to go on with, or a journal that isn't whole, which can't be trusted to be what went in. Either way what the file has is what the tab shows.
fn carried_over(folder: &Path, parsed: &ParsedJournal, now: FileStamp, path: &Path, progress: &mut Progress) -> Option<(String, RestoredEdits, usize)> {
    let mut restored = restored_from(parsed)?;
    if restored.lost_lines {
        return None;
    }
    let keeping = restored
        .edits
        .keeping()
        .cloned()
        .or_else(|| parsed.header.keeping.as_ref().and_then(AddedSource::from_record))
        .filter(kept_file_is_there);
    let pages = restored.edits.len();
    let rebased = restored.edits.rebase_on_saved(pages, keeping)?;
    for gone in &rebased.let_go {
        if let Some(name) = gone.file_name().and_then(|name| name.to_str()).filter(|name| record::is_kept_name(name)) {
            let _ = std::fs::remove_file(folder.join(name));
        }
    }
    if !restored.edits.has_history() {
        return None;
    }
    let name = record::journal_file_name(Some(now), path, None);
    let seq = restored.edits.journal_seq().max(restored.seq);
    let header = JournalHeader::new(parsed.header.tab, path, Some(now), pages, seq);
    let start = record::journal_start(&header, Some(&restored.edits.to_record()));
    replace(&folder.join(&name), &start, false).ok()?;
    progress.wrote_bytes += start.len() as u64;
    progress.journal_bytes.insert(name.clone(), start.len() as u64);
    restored.seq = seq;
    Some((name, restored, rebased.rolled_off))
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
        Self {
            root: paths.root.clone(),
            dir: Mutex::new(dir),
            lock: Mutex::new(lock),
            lock_path,
            progress: Mutex::new(Progress::default()),
            version,
        }
    }

    /// Whether this process is the one keeping this record.
    pub fn holds_the_record(&self) -> bool {
        self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// Asks for the lock again, for a moment. An app that's just gone holds it until the kernel has finished with it, and starting empty beside its own record because of that would lose the very thing an update is for. On a thread that can wait.
    pub fn wait_for_lock(&self) -> bool {
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
        // Owner, 2026-11-07: history stays out of backups, however the folder came to be here. A look on each visit to the disk. A failure here only costs that.
        if !local::is_excluded_from_backups(&journals) {
            let _ = local::exclude_from_backups(&journals);
        }
        if made {
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
                let notice = newest_passed_over(&main, slot).then_some(record::notices::OLDER_RECORD);
                self.finish_load(&main, Some((decoded, slot)), notice)
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

    /// The folder the record's in, as this version keeps it.
    #[must_use]
    pub fn main_folder(&self) -> PathBuf {
        self.fold_root()
    }

    /// This version's own folder beside a newer app's record, which it reads instead of that one.
    #[must_use]
    pub fn own_folder(&self) -> PathBuf {
        self.version_folder(&self.fold_root(), self.version)
    }

    fn fold_root(&self) -> PathBuf {
        let locked = self.lock_path.file_stem().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        self.root.join(locked)
    }

    fn version_folder(&self, main: &Path, version: u32) -> PathBuf {
        self.root.join(format!(
            "{}.v{version}",
            main.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned())
        ))
    }

    /// Makes room for one more damaged state, the oldest going, and says where it goes.
    fn make_room_for_damaged(&self, main: &Path) -> PathBuf {
        let name = main.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        let slot = |at: usize| {
            if at == 0 {
                self.root.join(format!("{name}.damaged"))
            } else {
                self.root.join(format!("{name}.damaged.{at}"))
            }
        };
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
            None => SessionRecord {
                app: AppRecord::default(),
                view: ViewRecord::default(),
                find: FindRecord::default(),
                edit_mode: false,
                active: None,
                next_id: 1,
                tabs: Vec::new(),
                comparison: None,
                recent: Vec::new(),
            },
        };
        // What the disk has, so what loading the journals changes in it does get written.
        let as_read = had_record.then(|| crc_of(&record));
        // A PDF is open once, whatever a record says. The journals of tabs that lost out are left as they are, and said.
        let not_merged = record::one_tab_per_file(&mut record);
        let journals = load_journals(dir, &mut record, &mut progress, &not_merged);
        // Nothing's changed by coming back, unless the journals said otherwise: writing what was read would be a write for nothing.
        progress.whole_crc = as_read;
        drop(progress);
        prune_sleep_files(&dir.join(JOURNALS));
        let in_use: Vec<String> = journals
            .values()
            .flat_map(|restored| {
                restored
                    .edits
                    .kept_sources()
                    .filter_map(|(_, added)| added.path.file_name()?.to_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .collect();
        prune_history(&dir.join(JOURNALS), Some(&in_use));
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
        let mut closed_one = false;
        let mut progress = self.progress();
        let folder = dir.join(JOURNALS);
        for name in &work.retired {
            if !is_journal_name(name) && !record::is_kept_name(name) && !record::is_sleep_name(name) {
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
            // A journal starting again is the whole list, in one step, and whatever it takes over from goes once it's there.
            if let Some(base) = &append.base {
                let header = JournalHeader {
                    base_seq: append.through,
                    ..append.header.clone()
                };
                let start = record::journal_start(&header, Some(base));
                let path = folder.join(&name);
                if replace(&path, &start, force).is_err() {
                    failed = Some(io_failed("written to"));
                    continue;
                }
                progress.wrote_bytes += start.len() as u64;
                progress.appends += 1;
                if let Some(old) = append.replaces.as_ref().filter(|old| is_journal_name(old) && **old != name) {
                    if std::fs::remove_file(folder.join(old)).is_err() {
                        outcome.not_retired.push(old.clone());
                    }
                    progress.journal_bytes.remove(old);
                }
                if append.closing {
                    progress.journal_bytes.remove(&name);
                    if let Some(closed) = closed_journal_name(&name) {
                        let _ = std::fs::rename(&path, folder.join(closed));
                    }
                    closed_one = true;
                } else {
                    progress.journal_bytes.insert(name.clone(), start.len() as u64);
                }
                let _ = local::hand_folder_to_drive(&folder);
                outcome.written.push((append.tab, append.through, name));
                continue;
            }
            let mut lines = Vec::new();
            if known.is_none() {
                // New: its header goes first, for as many operations as came before these.
                let header = JournalHeader {
                    base_seq: append.through.saturating_sub(append.ops.len() as u64),
                    ..append.header.clone()
                };
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
                closed_one = true;
            } else {
                progress.journal_bytes.insert(name.clone(), start + lines.len() as u64);
            }
            outcome.written.push((append.tab, append.through, name));
        }
        drop(progress);
        // Something's being done here anyway, and a closed history is what the limits are about. No timer.
        if closed_one {
            prune_history(&folder, None);
        }
        (outcome, failed)
    }

    /// Why this won't write, if it won't.
    fn refuses_writes(&self) -> Option<StoreError> {
        if !self.holds_the_record() {
            return Some(self.other_instance());
        }
        self.progress()
            .read_only
            .then(|| StoreError::new("record_unreadable", record::notices::words(record::notices::UNREADABLE).unwrap_or_default()))
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
            let delta = PlaceDelta {
                seq: progress.seq,
                contents_scroll: record.app.contents_scroll,
                focus: Some(record.app.focus),
                ..delta.clone()
            };
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
        self.verify_with_asleep(want, lists, &HashMap::new())
    }

    /// [`Self::verify`], and lists that have gone to disk have to read as what's known of them.
    pub fn verify_with_asleep(
        &self,
        want: &SessionRecord,
        lists: &HashMap<String, record::EditsRecord>,
        asleep: &HashMap<String, record::EditsSummary>,
    ) -> Result<(), StoreError> {
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
            let parsed = read_journal(&path)
                .map_err(|_| mismatch("a journal"))?
                .ok_or_else(|| mismatch("a journal that isn't there"))?
                .0;
            let restored = restored_from(&parsed).ok_or_else(|| mismatch("a journal"))?;
            if restored.lost_lines || restored.edits.to_record() != *list {
                return Err(mismatch("unsaved edits"));
            }
        }
        for (name, summary) in asleep {
            let parsed = read_journal(&dir.join(JOURNALS).join(name))
                .map_err(|_| mismatch("a journal"))?
                .ok_or_else(|| mismatch("a journal that isn't there"))?
                .0;
            let restored = restored_from(&parsed).ok_or_else(|| mismatch("a journal"))?;
            if restored.lost_lines || restored.edits.summary() != *summary {
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
        let main = progress
            .newest_slot
            .and_then(|slot| std::fs::metadata(dir.join(SLOTS[slot])).ok())
            .map_or(0, |meta| meta.len());
        (main + progress.places_bytes, progress.journal_bytes.values().sum())
    }

    /// Whether this is writing beside a newer app's record, which it won't touch.
    pub fn beside_newer(&self) -> bool {
        self.progress().beside_newer
    }

    fn other_instance(&self) -> StoreError {
        StoreError::new("other_instance", record::notices::OTHER_INSTANCE)
    }

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
        recto_core::save::keep_pages(path, pages, &kept, recto_session::edit::MAX_HISTORY_BYTES as u64).ok()?;
        // Its age is its own, which is what the 30 days go by, and it's this user's alone. Synced again, as both are changes since the copy was synced.
        let made = (|| -> std::io::Result<FileStamp> {
            local::make_private_file(&kept)?;
            let file = OpenOptions::new().write(true).open(&kept)?;
            file.set_modified(std::time::SystemTime::now())?;
            file.sync_all()?;
            drop(file);
            local::sync_folder_of(&kept)?;
            file_stamp(&kept)
        })();
        let stamp = match made {
            Ok(stamp) if stamp.size <= recto_session::edit::MAX_HISTORY_BYTES as u64 => stamp,
            _ => {
                let _ = std::fs::remove_file(&kept);
                return None;
            }
        };
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

    /// **What going dormant does to a list on disk:** the journal's checkpoint moves up, if that's worth it, and it's read back and compared, as what's in memory is about to go. Says whether it's all there.
    pub fn settle_list(&self, name: &str, header: &JournalHeader, base: &record::EditsRecord) -> bool {
        let _ = self.compact(name, header, base);
        self.read_list(name).is_some_and(|restored| restored.edits.to_record() == *base)
    }

    /// **Writes a tab's sleep file,** in one step, as it goes dormant or the app goes away. It's only a way to be quick, so failing here costs nothing but that, and isn't said. Says how many bytes went.
    pub fn write_sleep(&self, path: &Path, sleep: &SleepFile) -> u64 {
        recto_platform::slow::pause(recto_platform::slow::Seam::SleepWrite);
        if self.refuses_writes().is_some() {
            return 0;
        }
        let dir = self.dir();
        if Self::ensure_dirs(&dir).is_err() {
            return 0;
        }
        self.note_going_back(&dir);
        let bytes = encode_sleep(sleep);
        let folder = dir.join(JOURNALS);
        let file = folder.join(record::sleep_file_name(sleep.file, path));
        // What's there is this already: nothing's changed, so nothing's written.
        if read_ours(&file, MAX_SLEEP_FILE_BYTES).ok().flatten().is_some_and(|there| there == bytes) {
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

    /// **Whether a journal on disk says a save is about to put a file of this identity in place,** read back from the file itself. A save checks this before its file's moved, so a crash after the move always finds the note it needs (review: "check the save marker again once it's on disk").
    pub fn journal_says_saving(&self, name: &str, target: FileStamp) -> bool {
        if !is_journal_name(name) {
            return false;
        }
        let path = self.dir().join(JOURNALS).join(name);
        matches!(read_journal(&path), Ok(Some((parsed, _))) if saved_into_its_file(&parsed, Some(target)))
    }

    /// Removes a journal found to be stale.
    pub fn remove_journal(&self, name: &str) {
        if is_journal_name(name) || record::is_kept_name(name) || record::is_sleep_name(name) {
            let _ = std::fs::remove_file(self.dir().join(JOURNALS).join(name));
            self.progress().journal_bytes.remove(name);
        }
    }
}

impl HistorySource for RecordStore {
    fn read_sleep(&self, stamp: FileStamp, path: &Path) -> Option<SleepFile> {
        RecordStore::read_sleep(self, stamp, path)
    }

    fn read_list(&self, name: &str) -> Option<RestoredEdits> {
        if !is_journal_name(name) {
            return None;
        }
        let (parsed, _) = read_journal(&self.dir().join(JOURNALS).join(name)).ok().flatten()?;
        let mut restored = restored_from(&parsed)?;
        if restored.lost_lines {
            return None;
        }
        restored.rolled_off = roll_off_what_isnt_kept(&mut restored.edits);
        Some(restored)
    }

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
        let mut restored = restored_from(&parsed)?;
        if restored.lost_lines || std::fs::symlink_metadata(folder.join(&name)).is_ok() {
            return None;
        }
        std::fs::rename(folder.join(&closed), folder.join(&name)).ok()?;
        let mut bytes = parsed.good_bytes;
        if roll_off_what_isnt_kept(&mut restored.edits) {
            restored.rolled_off = true;
            let header = JournalHeader {
                base_seq: restored.seq,
                ..parsed.header.clone()
            };
            let start = record::journal_start(&header, Some(&restored.edits.to_record()));
            if replace(&folder.join(&name), &start, false).is_ok() {
                bytes = start.len() as u64;
            }
        }
        self.progress().journal_bytes.insert(name.clone(), bytes);
        Some(TakenHistory { name, restored })
    }
}

/// Whether the record read is the older of two, as the other's been written since and doesn't read. A slot older than what's good is no news, as a crash mid-write of it leaves just that.
fn newest_passed_over(dir: &Path, slot: usize) -> bool {
    let modified = |path: PathBuf| std::fs::metadata(path).and_then(|meta| meta.modified()).ok();
    let (ours, other) = (dir.join(SLOTS[slot]), dir.join(SLOTS[1 - slot]));
    modified(other).zip(modified(ours)).is_some_and(|(theirs, ours)| theirs > ours)
}

/// A journal's other name, as something's at this one that isn't to be touched.
fn free_name(folder: &Path, append: &recto_session::JournalAppend) -> Option<String> {
    let path = append.header.path.to_path().unwrap_or_default();
    (0..64u32)
        .map(|at| record::journal_file_name(append.header.file.map(Into::into), &path, Some(append.tab.0.saturating_add(at))))
        .find(|name| std::fs::symlink_metadata(folder.join(name)).is_err())
}

fn truncate_ours(path: &Path, length: u64) -> std::io::Result<()> {
    local::open_private_for_writing(path, false)?.set_len(length)
}

/// Temp files a crash left, of ours by their name, in a folder of ours. Nothing else is touched.
fn remove_temp_files(dir: &Path) {
    for folder in [dir.to_path_buf(), dir.join(JOURNALS)] {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let name = entry.file_name();
            let is_temp = name.to_str().is_some_and(|name| {
                (name.starts_with("record.")
                    || name.starts_with("journal-")
                    || name.starts_with("closed-")
                    || name.starts_with("kept-")
                    || name.starts_with("sleep-")
                    || name.starts_with(PLACES))
                    && name
                        .rsplit('.')
                        .next()
                        .is_some_and(|tail| tail.starts_with("tmp") && tail[3..].bytes().all(|byte| byte.is_ascii_digit()))
            });
            if is_temp && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Closed PDFs' histories and the files kept for histories, for 30 days within 1 GiB, oldest out first. A look at their sizes and times, with nothing read, when something's running anyway: no timer. Only a launch knows which kept files open tabs need, so only it says, and only it lets any go. Kept files in use count towards the limit. One that goes while a closed history needs it costs that history those steps, and it says so when it's taken up.
fn prune_history(folder: &Path, kept_in_use: Option<&[String]>) {
    let now = std::time::SystemTime::now();
    let mut files: Vec<(u64, PathBuf, u64, bool)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let is_kept = record::is_kept_name(name);
            if !is_kept && !is_closed_journal_name(name) {
                return None;
            }
            let meta = entry.metadata().ok()?;
            let age = meta
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .map_or(0, |age| age.as_secs());
            Some((age, entry.path(), meta.len(), is_kept))
        })
        .collect();
    files.sort_unstable_by_key(|(age, ..)| *age);
    let mut total = 0u64;
    for (age, path, bytes, is_kept) in files {
        total += bytes;
        if age <= CLOSED_JOURNAL_DAYS * 86_400 && total <= MAX_CLOSED_JOURNAL_BYTES {
            continue;
        }
        let may_go = !is_kept
            || kept_in_use.is_some_and(|in_use| {
                !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| in_use.iter().any(|used| used == name))
            });
        if may_go {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Sleep files are held to [`MAX_SLEEP_BYTES`] in all, oldest first, and 30 days: each can be rebuilt. A look at their sizes each time one's written, which is once for a tab going dormant.
fn prune_sleep_files(folder: &Path) {
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

/// What a record comes to, to know it's changed. Encoded with sequence zero, so only what's in it counts.
fn crc_of(record: &SessionRecord) -> u32 {
    let bytes = record::encode_record(record, 0);
    bytes
        .iter()
        .fold(0x811c_9dc5u32, |value, byte| (value ^ u32::from(*byte)).wrapping_mul(0x0100_0193))
}

/// Copies a folder of ours one level deep, and its journals. For keeping what an older app wrote.
fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    local::ensure_private_dir(to)?;
    local::ensure_private_dir(&to.join(JOURNALS))?;
    let _ = local::exclude_from_backups(&to.join(JOURNALS));
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

/// Whether a journal isn't the one a save it was for carries on under, so it can go.
fn carried_name_differs(path: &Path, about: &Path, now: FileStamp) -> bool {
    path.file_name().and_then(|name| name.to_str()) != Some(record::journal_file_name(Some(now), about, None).as_str())
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
/// 2. Otherwise a tab on the very same file, by its path or number, with no journal: a first edit whose journal went out before the record next did.
/// 3. Otherwise, with real unsaved edits, it's what a crash leaves before the record had heard of the tab, and it comes back as one.
/// 4. Otherwise it's a closed PDF's history, and goes to `closed-`, where no launch reads it.
///
/// A journal from a newer app is left exactly as it is, and so is what's past the total a launch will read. One that can't be read is kept to one side. One saved away is removed. What couldn't be read of one is copied beside it before anything's cut, and what's left is written as a clean checkpoint, so nothing added after it is lost behind a line replay can't get past.
fn load_journals(dir: &Path, record: &mut SessionRecord, progress: &mut Progress, not_merged: &[String]) -> HashMap<String, RestoredEdits> {
    let folder = dir.join(JOURNALS);
    let mut journals = HashMap::new();
    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| is_journal_name(name))
        .collect();
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
        let named = record.tabs.iter().position(|tab| {
            tab.journal.as_deref() == Some(read.name.as_str())
                && journal_named_is_for(&read.parsed.header, tab.file.map(Into::into), &tab.path.to_path().unwrap_or_default())
        });
        match named {
            Some(at) if !taken_tabs[at] => {
                taken_tabs[at] = true;
                claim(&folder, read, record, at, progress, &mut journals);
            }
            _ => unclaimed.push(read),
        }
    }
    for read in unclaimed {
        // Already taken over by what it would be: a save's crash left both.
        if journals.contains_key(&read.name) {
            continue;
        }
        // A save was confirmed and this is its new journal, with the old one gone, and a crash kept the record from hearing. The tab names what's gone, on a path that's now this file.
        let moved_on = record.tabs.iter().enumerate().position(|(at, tab)| {
            !taken_tabs[at]
                && tab.path == read.parsed.header.path
                && tab
                    .journal
                    .as_deref()
                    .is_some_and(|named| std::fs::symlink_metadata(folder.join(named)).is_err())
                && read.parsed.header.file.is_some_and(|said| {
                    tab.path
                        .to_path()
                        .and_then(|path| file_stamp(&path).ok())
                        .is_some_and(|now| FileStamp::from(said).same_contents(now) && FileStamp::from(said).inode == now.inode)
                })
        });
        if let Some(at) = moved_on {
            taken_tabs[at] = true;
            record.tabs[at].file = read.parsed.header.file;
            record.tabs[at].page_count = read.parsed.header.file_pages;
            claim(&folder, read, record, at, progress, &mut journals);
            continue;
        }
        let same_file = record.tabs.iter().enumerate().position(|(at, tab)| {
            !taken_tabs[at] && tab.journal.is_none() && journal_is_for(&read.parsed.header, tab.file.map(Into::into), &tab.path.to_path().unwrap_or_default())
        });
        if let Some(at) = same_file {
            taken_tabs[at] = true;
            claim(&folder, read, record, at, progress, &mut journals);
            continue;
        }
        let JournalRead { name, path, parsed } = read;
        let Some(about) = parsed.header.path.to_path() else { continue };
        let now = file_stamp(&about).ok();
        if let Some(now) = now.filter(|_| saved_into_its_file(&parsed, now)) {
            // A tab's on what this would carry on as, so it's only that a remove failed: the live one's not to be touched, and this one's done with.
            let carries_on_as = record::journal_file_name(Some(now), &about, None);
            if record.tabs.iter().any(|tab| tab.journal.as_deref() == Some(carries_on_as.as_str())) && carried_name_differs(&path, &about, now) {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            // Saved, with no tab to hold it: its history waits for the PDF to be opened again.
            if let Some((carried, _, _)) = carried_over(&folder, &parsed, now, &about, progress) {
                progress.journal_bytes.remove(&carried);
                if let Some(closed) = closed_journal_name(&carried) {
                    let _ = std::fs::rename(folder.join(&carried), folder.join(closed));
                }
            }
            if carried_name_differs(&path, &about, now) {
                let _ = std::fs::remove_file(&path);
            }
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
        record.tabs.push(TabEntry {
            id,
            path: parsed.header.path.clone(),
            file: parsed.header.file,
            page: 0,
            page_count: parsed.header.file_pages,
            place: None,
            view_rotation: 0,
            toc_open: true,
            layout: Default::default(),
            renderer_only: false,
            selection: None,
            notice: None,
            journal: Some(name.clone()),
            edits: Some(restored.edits.summary()),
            generated_contents: false,
            contents_folds: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
            password: false,
        });
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
    if let Some(saved) = now.filter(|_| saved_into_its_file(&parsed, now)) {
        // Its page order went into the PDF, and only the confirming didn't happen.
        let about = tab.path.to_path().unwrap_or_default();
        let carried = carried_over(folder, &parsed, saved, &about, progress);
        if carried_name_differs(&path, &about, saved) {
            let _ = std::fs::remove_file(&path);
        }
        progress.journal_bytes.remove(&name);
        tab.file = Some(saved.into());
        tab.notice = None;
        match carried {
            Some((carried, restored, rolled_off)) => {
                tab.journal = Some(carried.clone());
                tab.edits = Some(restored.edits.summary());
                tab.page_count = restored.edits.len();
                tab.page = tab.page.min(restored.edits.len().saturating_sub(1));
                if rolled_off > 0 {
                    tab.notice = Some(FileNotice::HistoryRolledOff);
                }
                progress
                    .journal_bytes
                    .insert(carried.clone(), std::fs::metadata(folder.join(&carried)).map_or(0, |meta| meta.len()));
                journals.insert(carried, restored);
            }
            None => {
                tab.journal = None;
                tab.edits = None;
            }
        }
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
        let header = JournalHeader {
            base_seq: restored.seq,
            ..parsed.header.clone()
        };
        let start = record::journal_start(&header, Some(&restored.edits.to_record()));
        if replace(&path, &start, false).is_ok() {
            good_bytes = start.len() as u64;
            progress.wrote_bytes += start.len() as u64;
        }
        tab.notice = Some(FileNotice::EditsUnreadable);
        restored.lost_lines = false;
    }
    if roll_off_what_isnt_kept(&mut restored.edits) {
        restored.rolled_off = true;
        tab.notice = Some(FileNotice::HistoryRolledOff);
        // The journal has to say what the list now does, or it would differ on reading back.
        let header = JournalHeader {
            base_seq: restored.seq,
            ..parsed.header.clone()
        };
        let start = record::journal_start(&header, Some(&restored.edits.to_record()));
        if replace(&path, &start, false).is_ok() {
            good_bytes = start.len() as u64;
            progress.wrote_bytes += start.len() as u64;
        }
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
        let root =
            std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("rs{}-{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
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
            tabs: (1..=tabs)
                .map(|id| TabEntry {
                    id,
                    path: PathField::Text(format!("/d/{id}.pdf")),
                    file: None,
                    page: 0,
                    page_count: 5,
                    place: None,
                    view_rotation: 0,
                    toc_open: true,
                    layout: Default::default(),
                    renderer_only: false,
                    selection: None,
                    notice: None,
                    journal: None,
                    edits: None,
                    generated_contents: false,
                    contents_folds: Vec::new(),
                    back: Vec::new(),
                    forward: Vec::new(),
                    password: false,
                })
                .collect(),
            comparison: None,
            recent: Vec::new(),
        }
    }

    fn loaded_record(store: &RecordStore) -> SessionRecord {
        match store.load() {
            Loaded::Restored(restored) => restored.record,
            other => panic!("{other:?}"),
        }
    }

    fn place() -> record::PlaceRecord {
        record::PlaceRecord {
            page: 3,
            anchor_page: 3,
            within: 0.25,
            x: 1.0,
            y: 2.0,
            document_height: 900.0,
            zoom: 1.5,
            page_width: 612.0,
            page_height: 792.0,
        }
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
        let delta = PlaceDelta {
            places: vec![(1, 3, Some(place()))],
            active: Some(1),
            ..PlaceDelta::default()
        };
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
        assert_eq!(
            std::fs::read(root.join(format!("session.v{CURRENT_VERSION}")).join(SLOTS[0])).unwrap(),
            v1,
            "kept"
        );
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
        assert_eq!(
            second.write_record(&sample(1), &PlaceDelta::default(), false).unwrap_err().code,
            "other_instance"
        );
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

    /// Failure: a save moves its file into place though its note never reached the disk, so a crash just after finds a journal that doesn't say a save happened, and replays the edits over the saved file. The note is read back from the file itself, and only this target's counts.
    #[test]
    fn a_saves_note_is_read_back_from_the_disk() {
        let (_root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let name = record::journal_file_name(None, Path::new("/d/1.pdf"), None);
        let target = FileStamp {
            size: 77,
            mtime_ns: 88,
            device: 1,
            inode: 99,
        };
        assert!(!store.journal_says_saving(&name, target), "no journal, no note");
        assert!(store.write_journals(&journal_work_for(&name, vec![rotation()], 1), false).1.is_none());
        assert!(!store.journal_says_saving(&name, target), "edits, but no save");
        assert!(
            store
                .write_journals(&journal_work_for(&name, vec![JournalOp::Saving(target.into())], 2), false)
                .1
                .is_none()
        );
        assert!(store.journal_says_saving(&name, target));
        assert!(
            !store.journal_says_saving(&name, FileStamp { size: 78, ..target }),
            "another file isn't this save's"
        );
        assert!(!store.journal_says_saving("../../etc/passwd", target), "only a journal's own name is read");
    }

    fn journal_work_for(name: &str, ops: Vec<JournalOp>, through: u64) -> JournalWork {
        let header = JournalHeader::new(1, Path::new("/d/1.pdf"), None, 5, 0);
        JournalWork {
            retired: Vec::new(),
            appends: vec![recto_session::JournalAppend {
                tab: TabId(1),
                name: name.to_string(),
                header,
                through,
                ops,
                closing: false,
                base: None,
                replaces: None,
            }],
        }
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
            let work = JournalWork {
                retired: Vec::new(),
                appends: vec![recto_session::JournalAppend {
                    tab: TabId(1),
                    name: name.clone(),
                    header,
                    through: 2,
                    ops: vec![rotation(), JournalOp::Saving(target.into())],
                    closing: false,
                    base: None,
                    replaces: None,
                }],
            };
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
        assert_eq!(restored.record.tabs[0].file, Some(FileRecord::from(before)));
        drop(store);
        // Crash with it in place, before the confirming call: the history goes on from the saved file, as if it had been confirmed.
        std::fs::rename(&temp, &file).unwrap();
        let now = file_stamp(&file).unwrap();
        let carried = record::journal_file_name(Some(now), &file, None);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        let tab = &restored.record.tabs[0];
        assert_eq!(tab.journal.as_deref(), Some(carried.as_str()), "under the new file's name");
        assert_eq!(tab.file, Some(now.into()));
        assert!(!restored.journals[&carried].edits.is_edited(), "its edits are in the PDF, so it reads as saved");
        assert!(restored.journals[&carried].edits.can_undo(), "and undo still goes past them");
        assert!(!root.join("session").join(JOURNALS).join(&name).exists(), "the old one's gone");
        assert!(root.join("session").join(JOURNALS).join(&carried).exists());
        // And that's what's read back, which is what a hand-off checks.
        let lists = HashMap::from([(carried.clone(), restored.journals[&carried].edits.to_record())]);
        store.write_record(&restored.record, &PlaceDelta::default(), false).unwrap();
        store.verify(&restored.record, &lists).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a crash between the steps of a confirmed save, with the new journal written and the old one there or gone, and the record still naming the old, loses the history or says the edits can't be read. Each comes back to one journal, about the saved file.
    #[test]
    fn a_crash_after_a_save_was_confirmed_keeps_the_history() {
        for old_still_there in [true, false] {
            let (root, paths) = state();
            let file = root.join("doc.pdf");
            std::fs::write(&file, b"before").unwrap();
            let before = file_stamp(&file).unwrap();
            let old = record::journal_file_name(Some(before), &file, None);
            let temp = root.join("doc.saving");
            std::fs::write(&temp, b"after, and longer").unwrap();
            let target = file_stamp(&temp).unwrap();
            let store = RecordStore::open(&paths, Flavor::App);
            let header = JournalHeader::new(1, &file, Some(before), 5, 0);
            let work = JournalWork {
                retired: Vec::new(),
                appends: vec![recto_session::JournalAppend {
                    tab: TabId(1),
                    name: old.clone(),
                    header,
                    through: 2,
                    ops: vec![rotation(), JournalOp::Saving(target.into())],
                    closing: false,
                    base: None,
                    replaces: None,
                }],
            };
            assert!(store.write_journals(&work, false).1.is_none());
            let mut record = sample(1);
            record.tabs[0].path = PathField::from_path(&file);
            record.tabs[0].file = Some(FileRecord::from(before));
            record.tabs[0].journal = Some(old.clone());
            store.write_record(&record, &PlaceDelta::default(), false).unwrap();
            // The save lands, and the new journal goes out. The record doesn't.
            std::fs::rename(&temp, &file).unwrap();
            let (mut list, _) = PageEdits::replay(5, None, &[rotation()]).unwrap();
            list.rebase_on_saved(5, None).unwrap();
            let name = record::journal_file_name(Some(target), &file, None);
            let header = JournalHeader::new(1, &file, Some(target), 5, 0);
            let work = JournalWork {
                retired: Vec::new(),
                appends: vec![recto_session::JournalAppend {
                    tab: TabId(1),
                    name: name.clone(),
                    header,
                    through: 3,
                    ops: Vec::new(),
                    closing: false,
                    base: Some(list.to_record()),
                    replaces: (!old_still_there).then(|| old.clone()),
                }],
            };
            assert!(store.write_journals(&work, false).1.is_none());
            assert_eq!(root.join("session").join(JOURNALS).join(&old).exists(), old_still_there);
            drop(store);
            let store = RecordStore::open(&paths, Flavor::App);
            let Loaded::Restored(restored) = store.load() else { panic!() };
            let tab = &restored.record.tabs[0];
            assert_eq!(tab.journal.as_deref(), Some(name.as_str()), "old one there: {old_still_there}");
            assert_eq!(tab.notice, None);
            assert_eq!(tab.file, Some(target.into()));
            assert_eq!(restored.journals.len(), 1);
            assert!(restored.journals[&name].edits.can_undo());
            assert!(!restored.journals[&name].edits.is_edited());
            assert!(!root.join("session").join(JOURNALS).join(&old).exists(), "the old one's gone");
            assert!(!root.join("session").join(JOURNALS).join(closed_journal_name(&name).unwrap()).exists());
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    /// Five pages, each saying `PAGEMARK` and its number.
    fn five_pages() -> Vec<u8> {
        let pages: Vec<recto_testpdf::Page> = (0..5)
            .map(|at| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("PAGEMARK{at}")])))
            .collect();
        recto_testpdf::pages(&pages)
    }

    /// Failure (decision 25): the history's in Time Machine, for good, after it's been cleared here, however its folder came to be: new, already there unmarked, or brought over from another version's. Or a kept copy is anyone's to read, or has the file's own age, and is pruned as soon as it's made.
    #[test]
    fn history_stays_out_of_backups_and_is_private() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let journals = root.join("session").join(JOURNALS);
        local::ensure_private_dir(&root.join("session")).unwrap();
        local::ensure_private_dir(&journals).unwrap();
        assert!(!local::is_excluded_from_backups(&journals));
        assert!(
            store
                .write_journals(&journal_work_for("journal-00000000000000aa.log", vec![rotation()], 1), false)
                .1
                .is_none()
        );
        assert!(local::is_excluded_from_backups(&journals), "marked, though it was already there");
        let copy = root.join("copied");
        copy_folder(&root.join("session"), &copy).unwrap();
        assert!(local::is_excluded_from_backups(&copy.join(JOURNALS)));
        let file = root.join("doc.pdf");
        std::fs::write(&file, five_pages()).unwrap();
        let target = FileStamp {
            size: 9,
            mtime_ns: 1,
            device: 1,
            inode: 2,
        };
        let kept = store.keep_pages_of(&file, target, &[1, 3]).unwrap();
        assert!(kept.kept && kept.path.starts_with(&journals));
        assert_eq!((kept.page_count, kept.holds.as_deref()), (2, Some(&[1, 3][..])), "only the pages asked for");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&kept.path).unwrap().permissions().mode() & 0o077, 0, "it's the user's alone");
        let bytes = std::fs::read(&kept.path).unwrap();
        assert!(bytes.windows(9).any(|window| window == b"PAGEMARK1") && bytes.windows(9).any(|window| window == b"PAGEMARK3"));
        assert!(
            !bytes.windows(9).any(|window| window == b"PAGEMARK0" || window == b"PAGEMARK2"),
            "and nothing of the others"
        );
        assert!(
            std::fs::metadata(&kept.path).unwrap().modified().unwrap().elapsed().unwrap().as_secs() < 60,
            "its age is its own"
        );
        assert!(kept_file_is_there(&kept));
        assert!(
            !kept.path.extension().is_some_and(|extension| extension == "pdf"),
            "no PDF importer for Spotlight to run on it"
        );
        // A second for the same save is beside it, not over it.
        let second = store.keep_pages_of(&file, target, &[1]).unwrap();
        assert_ne!(second.path, kept.path);
        // What isn't a PDF it can read isn't kept, and the steps roll off.
        std::fs::write(root.join("junk.pdf"), b"not a pdf").unwrap();
        assert!(store.keep_pages_of(&root.join("junk.pdf"), target, &[0]).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a kept file's been removed or changed, by hand or by its 30 days, and undo draws the wrong thing or nothing. At a restore and when a closed PDF's opened again, the steps needing it roll off and the tab says so. And the journal's rewritten to match, or a hand-off's read-back would never pass again.
    #[test]
    fn a_kept_file_gone_rolls_its_steps_off_and_says_so() {
        let (root, paths) = state();
        let file = root.join("doc.pdf");
        std::fs::write(&file, b"saved").unwrap();
        let now = file_stamp(&file).unwrap();
        let name = record::journal_file_name(Some(now), &file, None);
        let store = RecordStore::open(&paths, Flavor::App);
        let original = root.join("original.pdf");
        std::fs::write(&original, five_pages()).unwrap();
        let target = FileStamp {
            size: 9,
            mtime_ns: 1,
            device: 1,
            inode: 2,
        };
        let kept = store.keep_pages_of(&original, target, &[0]).unwrap();
        let (mut list, _) = PageEdits::replay(5, None, &[JournalOp::Delete(0, 1, record::AuthorName::Owner)]).unwrap();
        list.rebase_on_saved(4, Some(kept.clone())).unwrap();
        assert!(list.can_undo());
        let write = |store: &RecordStore, name: &str, record_it: bool| {
            let header = JournalHeader::new(1, &file, Some(now), 4, 0);
            let work = JournalWork {
                retired: Vec::new(),
                appends: vec![recto_session::JournalAppend {
                    tab: TabId(1),
                    name: name.to_string(),
                    header,
                    through: 1,
                    ops: Vec::new(),
                    closing: false,
                    base: Some(list.to_record()),
                    replaces: None,
                }],
            };
            assert!(store.write_journals(&work, false).1.is_none());
            if record_it {
                let mut record = sample(1);
                record.tabs[0].path = PathField::from_path(&file);
                record.tabs[0].file = Some(FileRecord::from(now));
                record.tabs[0].page_count = 4;
                record.tabs[0].journal = Some(name.to_string());
                store.write_record(&record, &PlaceDelta::default(), false).unwrap();
            }
        };
        write(&store, &name, true);
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert!(restored.journals[&name].edits.can_undo(), "with the file there, all's well");
        assert_eq!(restored.record.tabs[0].notice, None);
        drop(store);
        std::fs::remove_file(&kept.path).unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert!(!restored.journals[&name].edits.can_undo(), "the step needing it has gone");
        assert!(restored.journals[&name].rolled_off);
        assert_eq!(restored.record.tabs[0].notice, Some(FileNotice::HistoryRolledOff));
        let lists = HashMap::from([(name.clone(), restored.journals[&name].edits.to_record())]);
        store.write_record(&restored.record, &PlaceDelta::default(), false).unwrap();
        store.verify(&restored.record, &lists).expect("the journal says what the list does");
        drop(store);
        // A second launch has nothing to say or rewrite.
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert!(!restored.journals[&name].rolled_off);
        // A closed PDF's, opened again.
        let closed = closed_journal_name(&name).unwrap();
        std::fs::remove_file(root.join("session").join(JOURNALS).join(&name)).unwrap();
        let kept_again = store.keep_pages_of(&original, target, &[0]).unwrap();
        let (mut list, _) = PageEdits::replay(5, None, &[JournalOp::Delete(0, 1, record::AuthorName::Owner)]).unwrap();
        list.rebase_on_saved(4, Some(kept_again.clone())).unwrap();
        let header = JournalHeader::new(1, &file, Some(now), 4, 1);
        std::fs::write(
            root.join("session").join(JOURNALS).join(&closed),
            record::journal_start(&header, Some(&list.to_record())),
        )
        .unwrap();
        std::fs::remove_file(&kept_again.path).unwrap();
        let taken = store.history_of(Some(now), &file).unwrap();
        assert!(taken.restored.rolled_off && !taken.restored.edits.can_undo());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review R7): an old journal a remove failed on is still there, with the record already naming the new one. Going over it again overwrites the live journal, and moves it to `closed-`, so the next `session save` says it doesn't read back and a kill loses what's kept.
    #[test]
    fn a_leftover_old_journal_doesnt_touch_the_live_one() {
        let (root, paths) = state();
        let file = root.join("doc.pdf");
        std::fs::write(&file, b"before").unwrap();
        let before = file_stamp(&file).unwrap();
        let old = record::journal_file_name(Some(before), &file, None);
        let temp = root.join("doc.saving");
        std::fs::write(&temp, b"after, and longer").unwrap();
        let target = file_stamp(&temp).unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let header = JournalHeader::new(1, &file, Some(before), 5, 0);
        let work = JournalWork {
            retired: Vec::new(),
            appends: vec![recto_session::JournalAppend {
                tab: TabId(1),
                name: old.clone(),
                header,
                through: 2,
                ops: vec![rotation(), JournalOp::Saving(target.into())],
                closing: false,
                base: None,
                replaces: None,
            }],
        };
        assert!(store.write_journals(&work, false).1.is_none());
        std::fs::rename(&temp, &file).unwrap();
        let (mut list, _) = PageEdits::replay(5, None, &[rotation(), rotation()]).unwrap();
        list.rebase_on_saved(5, None).unwrap();
        let name = record::journal_file_name(Some(target), &file, None);
        let header = JournalHeader::new(1, &file, Some(target), 5, 3);
        let work = JournalWork {
            retired: Vec::new(),
            appends: vec![recto_session::JournalAppend {
                tab: TabId(1),
                name: name.clone(),
                header,
                through: 3,
                ops: Vec::new(),
                closing: false,
                base: Some(list.to_record()),
                replaces: None,
            }],
        };
        assert!(store.write_journals(&work, false).1.is_none());
        let live = std::fs::read(root.join("session").join(JOURNALS).join(&name)).unwrap();
        let mut record = sample(1);
        record.tabs[0].path = PathField::from_path(&file);
        record.tabs[0].file = Some(FileRecord::from(target));
        record.tabs[0].journal = Some(name.clone());
        store.write_record(&record, &PlaceDelta::default(), false).unwrap();
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        let folder = root.join("session").join(JOURNALS);
        assert_eq!(std::fs::read(folder.join(&name)).unwrap(), live, "the live one's as it was");
        assert!(!folder.join(closed_journal_name(&name).unwrap()).exists());
        assert!(!folder.join(&old).exists(), "and the leftover's gone");
        assert_eq!(restored.journals[&name].edits.to_record(), list.to_record());
        let lists = HashMap::from([(name.clone(), list.to_record())]);
        store.write_record(&restored.record, &PlaceDelta::default(), false).unwrap();
        store.verify(&restored.record, &lists).expect("it reads back");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a sleep file for another version of the file is used, or a damaged one stays to be read at every wake, or a bad name reaches outside the folder, or a newer app's is thrown away. It's only ever a way to be quick, so all of these just cost that.
    #[test]
    fn a_sleep_file_is_only_for_its_own_file() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let file = root.join("doc.pdf");
        std::fs::write(&file, b"contents").unwrap();
        let stamp = file_stamp(&file).unwrap();
        let sleep = SleepFile {
            file: stamp,
            file_pages: 3,
            pages: 3,
            list_seq: 0,
            renderer_only: false,
            version: None,
            toc: recto_core::Toc::default(),
            usual: Some((612.0, 792.0)),
            sizes: Vec::new(),
            pictures: Vec::new(),
        };
        assert!(store.write_sleep(&file, &sleep) > 0);
        assert_eq!(store.read_sleep(stamp, &file), Some(sleep.clone()));
        let on_disk = root.join("session").join(JOURNALS).join(record::sleep_file_name(stamp, &file));
        assert!(local::is_excluded_from_backups(on_disk.parent().unwrap()));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&on_disk).unwrap().permissions().mode() & 0o077, 0);
        // Another version of the file has another name.
        let later = FileStamp { size: stamp.size + 1, ..stamp };
        assert_eq!(store.read_sleep(later, &file), None);
        assert!(on_disk.exists());
        // Swapped under another's name, or damaged: gone.
        let other = root.join("other.pdf");
        let other_stamp = FileStamp {
            inode: stamp.inode + 1,
            ..stamp
        };
        std::fs::copy(&on_disk, root.join("session").join(JOURNALS).join(record::sleep_file_name(other_stamp, &other))).unwrap();
        assert_eq!(store.read_sleep(other_stamp, &other), None);
        assert!(!root.join("session").join(JOURNALS).join(record::sleep_file_name(other_stamp, &other)).exists());
        let mut bytes = std::fs::read(&on_disk).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        std::fs::write(&on_disk, &bytes).unwrap();
        assert_eq!(store.read_sleep(stamp, &file), None);
        assert!(!on_disk.exists());
        // A newer app's stays.
        let newer = record::seal_as("recto-sleep", CURRENT_VERSION + 1, 0, b"{}\n");
        std::fs::write(&on_disk, &newer).unwrap();
        assert_eq!(store.read_sleep(stamp, &file), None);
        assert!(on_disk.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: sleep files grow without end over thousands of tabs. They're held to their limit, oldest first, and that doesn't touch anything else.
    #[test]
    fn sleep_files_stay_within_their_limit() {
        let (root, _paths) = state();
        let folder = root.join("journals");
        std::fs::create_dir_all(&folder).unwrap();
        let make = |name: &str, age: u64, bytes: u64| {
            let path = folder.join(name);
            let file = File::create(&path).unwrap();
            file.set_len(bytes).unwrap();
            file.set_modified(std::time::SystemTime::now() - Duration::from_secs(age)).unwrap();
            path
        };
        let new = make("sleep-00000000000000aa.bin", 10, MAX_SLEEP_BYTES / 2);
        let middle = make("sleep-00000000000000bb.bin", 20, MAX_SLEEP_BYTES / 2);
        let oldest = make("sleep-00000000000000cc.bin", 30, 10);
        let expired = make("sleep-00000000000000dd.bin", (CLOSED_JOURNAL_DAYS + 1) * 86_400, 0);
        let journal = make("journal-00000000000000ee.log", (CLOSED_JOURNAL_DAYS + 1) * 86_400, 10);
        prune_sleep_files(&folder);
        assert!(new.exists() && middle.exists());
        assert!(!oldest.exists() && !expired.exists());
        assert!(journal.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure: a kept file in use by an open tab is pruned for being old, or a journal's naming one makes this remove something outside the folder, or an old unneeded one stays for good.
    #[test]
    fn kept_files_are_pruned_by_age_unless_in_use() {
        let (root, _paths) = state();
        let folder = root.join("journals");
        std::fs::create_dir_all(&folder).unwrap();
        let old = |name: &str| {
            let path = folder.join(name);
            std::fs::write(&path, b"x").unwrap();
            let file = OpenOptions::new().write(true).open(&path).unwrap();
            file.set_modified(std::time::SystemTime::now() - Duration::from_secs((CLOSED_JOURNAL_DAYS + 1) * 86_400))
                .unwrap();
            path
        };
        let in_use = old("kept-00000000000000aa.pdf");
        let not_used = old("kept-00000000000000bb.pdf");
        let other = old("something-else.pdf");
        prune_history(&folder, None);
        assert!(not_used.exists(), "only a launch knows what's in use");
        prune_history(&folder, Some(&["kept-00000000000000aa.pdf".to_string()]));
        assert!(in_use.exists());
        assert!(!not_used.exists());
        assert!(other.exists(), "not ours");
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
        let work = JournalWork {
            retired: Vec::new(),
            appends: vec![recto_session::JournalAppend {
                tab: TabId(9),
                name: name.clone(),
                header,
                through: 1,
                ops: vec![rotation()],
                closing: false,
                base: None,
                replaces: None,
            }],
        };
        assert!(store.write_journals(&work, false).1.is_none());
        let closed = record::journal_file_name(None, Path::new("/d/closed.pdf"), None);
        let header = JournalHeader::new(8, Path::new("/d/closed.pdf"), None, 5, 0);
        let work = JournalWork {
            retired: Vec::new(),
            appends: vec![recto_session::JournalAppend {
                tab: TabId(8),
                name: closed.clone(),
                header,
                through: 2,
                ops: vec![JournalOp::Rotate(0, 1, 1, record::AuthorName::Agent), JournalOp::Undo],
                closing: false,
                base: None,
                replaces: None,
            }],
        };
        assert!(store.write_journals(&work, false).1.is_none());
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert_eq!(restored.record.tabs.len(), 2, "{:?}", restored.record.tabs);
        assert_eq!(restored.record.tabs[1].journal.as_deref(), Some(name.as_str()));
        assert!(restored.journals.contains_key(&name));
        assert!(!restored.journals.contains_key(&closed), "a closed PDF's history");
        assert!(
            root.join("session").join(JOURNALS).join(record::closed_journal_name(&closed).unwrap()).exists(),
            "kept for when it's opened, under a name no launch reads"
        );
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
        assert_eq!(
            store.verify(&record, &lists).unwrap_err().code,
            "read_back_differs",
            "a journal that isn't there"
        );
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
        let append = |tab: u32, name: &str, path: &str| recto_session::JournalAppend {
            tab: TabId(tab),
            name: name.to_string(),
            header: JournalHeader::new(tab, Path::new(path), None, 5, 0),
            through: 1,
            ops: vec![rotation()],
            closing: false,
            base: None,
            replaces: None,
        };
        // Known as a journal, and something that can't be opened as a file is in its place: a folder does it, as a real ENOSPC or EIO would.
        write_ok(&store, &journal_work_for(&bad, vec![rotation()], 1));
        let folder = root.join("session").join(JOURNALS);
        std::fs::remove_file(folder.join(&bad)).unwrap();
        std::fs::create_dir(folder.join(&bad)).unwrap();
        let work = JournalWork {
            retired: Vec::new(),
            appends: vec![append(9, &bad, "/d/bad.pdf"), append(1, &good, "/d/good.pdf")],
        };
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
        assert_eq!(
            record::parse_journal(&std::fs::read(folder.join(went_to)).unwrap(), CURRENT_VERSION)
                .unwrap()
                .ops
                .len(),
            1
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (review T8): a record's tab applies a journal about another file. Proved with one for `/etc/hosts`: its list went on a 6-page tab. The tab says it couldn't, and what's in the journal is left alone.
    #[test]
    fn a_tab_never_applies_a_journal_about_another_file() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let mut saved = sample(1);
        let was = FileStamp {
            size: 10,
            mtime_ns: 20,
            device: 1,
            inode: 30,
        };
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
        bytes.extend_from_slice(&record::journal_lines(&[
            rotation(),
            JournalOp::Rotate(99, 1, 1, record::AuthorName::Owner),
            rotation(),
        ]));
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
        assert!(
            store
                .write_journals(&journal_work_for(&name, vec![rotation()], list.seq + 1), false)
                .1
                .is_none()
        );
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
        let delta = PlaceDelta {
            places: vec![(1, 3, Some(place()))],
            active: Some(1),
            ..PlaceDelta::default()
        };
        store.write_record(&saved, &delta, false).unwrap();
        drop(store);
        let store = RecordStore::open(&paths, Flavor::App);
        assert_eq!(loaded_record(&store).tabs[0].page, 3, "the line after the tear");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Failure (final review): the newest slot can't be read and the older one can, and the older record comes back as though it were the latest. It's right to use it, and it says so. A slot older than what's good is no news, as a crash mid-write of it leaves just that.
    #[test]
    fn falling_back_to_the_older_slot_says_so() {
        let (root, paths) = state();
        let store = RecordStore::open(&paths, Flavor::App);
        let older = sample(1);
        store.write_record(&older, &PlaceDelta::default(), false).unwrap();
        let mut newer = sample(1);
        newer.edit_mode = true;
        store.write_record(&newer, &PlaceDelta::default(), false).unwrap();
        drop(store);
        let slots = [root.join("session").join(SLOTS[0]), root.join("session").join(SLOTS[1])];
        let whole = std::fs::read(&slots[1]).unwrap();
        std::fs::write(&slots[1], b"damaged").unwrap();
        // Written after the good one, by its time.
        File::options()
            .write(true)
            .open(&slots[0])
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(60))
            .unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert_eq!(restored.record, older);
        assert_eq!(restored.notice, Some(record::notices::OLDER_RECORD));
        drop(store);
        // The older one damaged instead: nothing's been lost by that.
        std::fs::write(&slots[1], &whole).unwrap();
        std::fs::write(&slots[0], b"damaged").unwrap();
        File::options()
            .write(true)
            .open(&slots[0])
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(60))
            .unwrap();
        let store = RecordStore::open(&paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!() };
        assert_eq!(restored.record, newer);
        assert_eq!(restored.notice, None);
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
        assert_eq!(
            store.write_record(&sample(2), &PlaceDelta::default(), false).unwrap_err().code,
            "record_unreadable"
        );
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
        assert!(
            loaded.tabs.iter().any(|tab| tab.journal.as_deref() == Some(name.as_str())),
            "with what it was editing"
        );
        assert!(
            !root.join(format!("session.v{CURRENT_VERSION}")).join(WENT_BACK).exists(),
            "and that's done with"
        );
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
        prune_history(&folder, Some(&[]));
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
