//! **The transaction record's store** (`docs/design/removal.md` §6 and §10, D3a): the only code that opens `<state root>/removal/pending.v1`. The format and the rules
//! for reading it are `recto_core::removal`'s (`barrier.rs`); this is what the app does with them. Nothing calls it yet (D3b wires startup and the carrier).
//!
//! **One file, one transaction.** [`begin`] makes it whole under a name of its own, gives it its name in one step that fails if a record is there already
//! (`RENAME_EXCL`), hands it to the drive (a folder `fsync`, then `F_FULLFSYNC` of the file, which also flushes the drive's cache), reads it back through a
//! fresh open to see the bytes are the ones made, and only then writes slot 0, flushed the same way. Only then is the barrier acknowledged, and a step after it
//! may purge anything. [`Held::acknowledge`] writes one slot for each later step in place and acknowledges it only once the slot is on the drive; [`Held::retire`]
//! takes the file away, which is the last step's acknowledgment. Nothing else deletes it but [`abandon`], and that only while no slot has been written. There is no
//! force-clear: a record that can't be read stays, and is said to be damaged.
//!
//! **Hostile storage.** The folder is opened by descriptor without following a link, and the file is opened from it with no link followed and no wait for a pipe.
//! Only a plain file of this user's with one name, no group or other access, and at most the cap's bytes (at most the cap plus one is read) is a record: anything
//! else at the name is `Damaged` and left alone. A folder at `removal/` that is a link, a file or someone else's is `NotOurs` and never read through. Two names are
//! checked before each slot: the name must still lead to the inode held, or the step fails with `Moved` and nothing is written to either file.
//!
//! **Cost.** At most two descriptors (the folder and the file) and no thread; the record is at most 2 MiB and read whole only by `look`, `reopen` and `abandon`.
//! Every call may wait on the disk, so none belongs on the UI thread.

use recto_core::removal::{
    Ack, BarrierFacts, BarrierRefusal, Decoded, Effect, Layout, Limits, MAX_BARRIER_BYTES, OrderError, Outcome, Plan, PlanDigest, Progress, Recorded, Surveyed,
    decode_barrier_within, encode_barrier_within,
};
use recto_platform::folder::{Folder, Opened};
use recto_platform::{folder::Kind, local};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::Path;

const DIR: &str = "removal";
const NAME: &str = "pending.v1";
const TEMP_PREFIX: &str = "pending.v1.new-";
/// The most entries one sweep looks at: a folder of ours holds a handful, so more is not ours to walk.
const MAX_SWEEP: usize = 4_096;
/// A random name that was taken is tried again this many times.
const NAME_TRIES: usize = 8;

/// What `look` found at `<root>/removal/pending.v1`.
#[derive(Debug)]
pub enum Looked {
    /// No `removal/` folder, or no `pending.v1` in it.
    Nothing,
    /// `removal/` is a link, a file or someone else's: not read through.
    NotOurs,
    /// Anything at the name but a private plain file of ours with one name, or one over the cap, or one that doesn't decode. Left as it is.
    Damaged,
    /// A record of a later version.
    TooNew,
    Pending(Box<Recorded>),
}

/// Where a call has got to, for a test to stop or change things at (a kill is a `Stop` with nothing cleaned up).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    /// The folder is prepared and its leftovers are swept; no temporary is made yet.
    Swept,
    TempMade,
    TempWritten,
    TempSynced,
    Renamed,
    FolderSynced,
    ReadBack,
    Slot0Written,
    Slot0Synced,
    /// The slot that acknowledges the step before this count was written, and then flushed.
    SlotWritten(u32),
    SlotSynced(u32),
    Unlinked,
    UnlinkSynced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Go,
    Stop,
}

/// A call's settings: the cap on a record, and what is called at each [`Point`].
pub struct Opts<'a> {
    pub cap: usize,
    pub at: &'a dyn Fn(Point) -> Flow,
}

fn go(_: Point) -> Flow {
    Flow::Go
}

impl Opts<'static> {
    pub const fn standard() -> Self {
        Self {
            cap: MAX_BARRIER_BYTES,
            at: &go,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BeginError {
    /// A record is there already: one transaction at a time.
    Pending,
    /// `removal/` is not ours, or something shaped like a temporary of ours is not a plain file of ours.
    NotOurs,
    Refused(BarrierRefusal),
    Io(io::ErrorKind),
    /// What was read back is not what was written: the record was taken away again (no slot exists, so nothing was purged).
    ReadBack,
    /// Slot 0 was not written or not flushed. A record is there, with no step done: `abandon` or resume it.
    NotDurable,
    /// A test's `Stop`.
    Stopped,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AckError {
    Order(OrderError),
    /// The name no longer leads to the record held: nothing was written. The step is acknowledged as failed.
    Moved,
    /// The slot wasn't written or flushed. The step is acknowledged as failed, and recovery is pending.
    NotDurable,
    /// The last step has no slot: its acknowledgment is the file being gone (`Held::retire`).
    UseRetire,
    /// `retire` while another step is next.
    NotAtRetire,
    Io(io::ErrorKind),
    Stopped,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AbandonError {
    /// A slot is written, so a step may have run: only going forward is safe.
    RecoveryRequired,
    NotOurs,
    /// There is no record, or it isn't the one given, or it can't be read.
    Gone,
    Io(io::ErrorKind),
}

/// The record's folder and file, held. Two descriptors.
#[derive(Debug)]
pub struct Held {
    folder: Folder,
    file: File,
    dev: u64,
    ino: u64,
    layout: Layout,
    /// The plan this record is of: nothing is written or retired for another.
    digest: PlanDigest,
}

fn dir_of(root: &Path) -> std::path::PathBuf {
    root.join(DIR)
}

/// A plain file of ours with one name and no group or other access.
fn private_plain(file: &File) -> io::Result<std::fs::Metadata> {
    let meta = file.metadata()?;
    if meta.file_type().is_file() && meta.nlink() == 1 && meta.mode() & 0o077 == 0 {
        Ok(meta)
    } else {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}

/// The whole of a file, if it is at most `cap` bytes: reads at most `cap + 1`.
fn read_capped(file: &File, cap: usize) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() <= cap).then_some(bytes))
}

/// The `removal/` folder without making it: `Ok(None)` for none, `Err` for one that is not ours.
fn open_dir(root: &Path) -> Result<Option<Folder>, ()> {
    match Folder::open(&dir_of(root)) {
        Opened::Folder(folder) => Ok(Some(folder)),
        Opened::Missing => Ok(None),
        Opened::Other => Err(()),
    }
}

/// What the record at `pending.v1` is, and the file it was read from.
fn read_record(folder: &Folder, cap: usize) -> Result<Option<(Decoded, File)>, io::ErrorKind> {
    let name = OsStr::new(NAME);
    match folder.look(name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.kind()),
        Ok(entry) if entry.kind != Kind::File || entry.links != 1 => return Err(io::ErrorKind::PermissionDenied),
        Ok(_) => {}
    }
    let file = folder.open_private_rw_in(name).map_err(|e| e.kind())?;
    private_plain(&file).map_err(|e| e.kind())?;
    let Some(bytes) = read_capped(&file, cap).map_err(|e| e.kind())? else {
        return Ok(Some((Decoded::Damaged, file)));
    };
    Ok(Some((decode_barrier_within(&bytes, cap), file)))
}

/// What is at `<root>/removal/pending.v1`. Never makes anything, and never says `Nothing` for what is there.
pub fn look(root: &Path) -> Looked {
    look_with(root, MAX_BARRIER_BYTES)
}

pub fn look_with(root: &Path, cap: usize) -> Looked {
    let folder = match open_dir(root) {
        Ok(Some(folder)) => folder,
        Ok(None) => return Looked::Nothing,
        Err(()) => return Looked::NotOurs,
    };
    match read_record(&folder, cap) {
        Ok(None) => Looked::Nothing,
        Ok(Some((Decoded::Pending(recorded), _))) => Looked::Pending(recorded),
        Ok(Some((Decoded::TooNew, _))) => Looked::TooNew,
        Ok(Some((Decoded::Damaged, _))) | Err(_) => Looked::Damaged,
    }
}

/// `removal/`, made private if it isn't there and kept out of backups, opened by descriptor.
fn prepare(root: &Path) -> Result<Folder, BeginError> {
    let dir = dir_of(root);
    match local::ensure_private_dir(&dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Err(BeginError::NotOurs),
        Err(error) => return Err(BeginError::Io(error.kind())),
    }
    // Best effort, as the app's other state folders: a volume that can't mark it is no reason to refuse.
    let _ = local::exclude_from_backups(&dir);
    match Folder::open(&dir) {
        Opened::Folder(folder) => Ok(folder),
        Opened::Missing | Opened::Other => Err(BeginError::NotOurs),
    }
}

fn is_temp_shaped(name: &OsStr) -> bool {
    let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(TEMP_PREFIX)) else {
        return false;
    };
    rest.len() == 16 && rest.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Takes away the leftover temporaries of a begin that was cut short. Anything with that exact shape that is not a plain file of ours is not ours.
fn sweep(folder: &Folder) -> Result<(), BeginError> {
    let mut stale = Vec::new();
    for entry in folder.entries().map_err(|e| BeginError::Io(e.kind()))?.take(MAX_SWEEP) {
        let entry = entry.map_err(|e| BeginError::Io(e.kind()))?;
        if !is_temp_shaped(&entry.name) {
            continue;
        }
        if entry.kind != Kind::File || entry.links != 1 {
            return Err(BeginError::NotOurs);
        }
        stale.push(entry.name);
    }
    for name in stale {
        match folder.unlink_in(&name, false) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(BeginError::Io(error.kind())),
        }
    }
    Ok(())
}

fn random_name() -> OsString {
    use std::collections::hash_map::RandomState;
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    OsString::from(format!("{TEMP_PREFIX}{:016x}", hasher.finish()))
}

fn make_temp(folder: &Folder) -> Result<(OsString, File), BeginError> {
    for _ in 0..NAME_TRIES {
        let name = random_name();
        match folder.create_private_in(&name) {
            Ok(file) => return Ok((name, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(BeginError::Io(error.kind())),
        }
    }
    Err(BeginError::Io(io::ErrorKind::AlreadyExists))
}

/// Writes the record and acknowledges the barrier. See the module's account of the order.
pub fn begin(root: &Path, plan: &Plan, surveyed: &Surveyed, limits: &Limits, progress: &mut Progress, facts: &BarrierFacts) -> Result<Held, BeginError> {
    begin_with(root, plan, surveyed, limits, progress, facts, &Opts::standard())
}

pub fn begin_with(
    root: &Path,
    plan: &Plan,
    surveyed: &Surveyed,
    limits: &Limits,
    progress: &mut Progress,
    facts: &BarrierFacts,
    opts: &Opts<'_>,
) -> Result<Held, BeginError> {
    macro_rules! at {
        ($point:expr) => {
            if (opts.at)($point) == Flow::Stop {
                return Err(BeginError::Stopped);
            }
        };
    }
    // Nothing is made before the record is known to fit.
    let encoded = encode_barrier_within(plan, surveyed, limits, progress, facts, opts.cap).map_err(BeginError::Refused)?;
    let barrier = encoded.layout.base();
    let folder = prepare(root)?;
    match folder.look(OsStr::new(NAME)) {
        Ok(_) => return Err(BeginError::Pending),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(BeginError::Io(error.kind())),
    }
    sweep(&folder)?;
    at!(Point::Swept);
    let (temp, mut file) = make_temp(&folder)?;
    let cleanup = |error: BeginError| {
        let _ = folder.unlink_in(&temp, false);
        error
    };
    at!(Point::TempMade);
    file.write_all(&encoded.bytes).map_err(|e| cleanup(BeginError::Io(e.kind())))?;
    at!(Point::TempWritten);
    local::hand_to_drive(&file).map_err(|e| cleanup(BeginError::Io(e.kind())))?;
    at!(Point::TempSynced);
    match folder.rename_excl_in(&temp, OsStr::new(NAME)) {
        Ok(()) => {}
        // Another begin got there first: this one's temporary is only taken away.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Err(cleanup(BeginError::Pending)),
        Err(error) => return Err(cleanup(BeginError::Io(error.kind()))),
    }
    at!(Point::Renamed);
    // From here the name is made; a failure takes it away again, since no slot exists and nothing was purged.
    let unmake = |error: BeginError| {
        let _ = folder.unlink_in(OsStr::new(NAME), false);
        error
    };
    folder.sync().map_err(|e| unmake(BeginError::Io(e.kind())))?;
    at!(Point::FolderSynced);
    // `sync_all` is `F_FULLFSYNC` here: it flushes the drive's cache, which covers the folder's `fsync` before it.
    file.sync_all().map_err(|e| unmake(BeginError::Io(e.kind())))?;
    let made = file.metadata().map_err(|e| unmake(BeginError::Io(e.kind())))?;
    drop(file);
    let file = folder.open_private_rw_in(OsStr::new(NAME)).map_err(|e| unmake(BeginError::Io(e.kind())))?;
    let seen = private_plain(&file).map_err(|e| unmake(BeginError::Io(e.kind())))?;
    if (seen.dev(), seen.ino()) != (made.dev(), made.ino()) {
        // Not the file that was made: another thing under our name, which is not ours to take away.
        return Err(BeginError::ReadBack);
    }
    let back = read_capped(&file, opts.cap).map_err(|e| unmake(BeginError::Io(e.kind())))?;
    if back.as_deref() != Some(&encoded.bytes[..]) {
        return Err(unmake(BeginError::ReadBack));
    }
    at!(Point::ReadBack);
    let held = Held {
        folder,
        file,
        dev: seen.dev(),
        ino: seen.ino(),
        layout: encoded.layout,
        digest: plan.digest(),
    };
    // Slot 0: the barrier itself. A failure leaves the record with no slot written or not known to be: resume or abandon it.
    let (offset, slot) = held.layout.slot(barrier + 1).ok_or(BeginError::NotDurable)?;
    held.file.write_all_at(&slot, offset).map_err(|_| BeginError::NotDurable)?;
    at!(Point::Slot0Written);
    held.file.sync_all().map_err(|_| BeginError::NotDurable)?;
    at!(Point::Slot0Synced);
    progress
        .acknowledge(plan, barrier, Ack::Done)
        .map_err(|_| BeginError::Refused(BarrierRefusal::NotAtBarrier))?;
    Ok(held)
}

/// Whether a step's slot is flushed through the drive's cache: the marks that decide what a restart does, and the install.
fn full_sync(effect: Effect) -> bool {
    matches!(
        effect,
        Effect::PersistBarrier | Effect::MarkNoReturn | Effect::InstallTarget | Effect::MarkComplete
    )
}

impl Held {
    pub const fn layout(&self) -> Layout {
        self.layout
    }

    /// Whether the name still leads to the file held.
    fn ours(&self) -> bool {
        matches!(self.folder.look(OsStr::new(NAME)), Ok(e) if e.kind == Kind::File && e.links == 1 && (e.dev, e.ino) == (self.dev, self.ino))
    }

    pub fn acknowledge(&self, plan: &Plan, progress: &mut Progress, index: u32, ack: Ack) -> Result<Outcome, AckError> {
        self.acknowledge_with(plan, progress, index, ack, &Opts::standard())
    }

    /// The only way to acknowledge a step after the barrier: the step's slot is written and flushed first, and only then is it acknowledged.
    pub fn acknowledge_with(&self, plan: &Plan, progress: &mut Progress, index: u32, ack: Ack, opts: &Opts<'_>) -> Result<Outcome, AckError> {
        let fail = |progress: &mut Progress, error: AckError| {
            let _ = progress.acknowledge(plan, index, Ack::Failed);
            error
        };
        let step = plan.steps().get(index as usize).ok_or(AckError::Order(OrderError::Finished))?;
        // Before anything is written: this record's plan, a walk of it, and the step it is at. A stopped walk is at no step.
        if plan.digest() != self.digest {
            return Err(AckError::Order(OrderError::WrongPlan));
        }
        progress.check_next(plan, index).map_err(AckError::Order)?;
        if step.effect() == Effect::RetireBarrier {
            return Err(AckError::UseRetire);
        }
        let durable = match ack {
            Ack::Done => true,
            Ack::DoneNotDurable => !step.effect().needs_durable(),
            Ack::Failed => false,
        };
        if !durable {
            return progress.acknowledge(plan, index, ack).map_err(AckError::Order);
        }
        let slot = self.layout.slot(index + 1).ok_or(AckError::Order(OrderError::OutOfOrder))?;
        if !self.ours() {
            return Err(fail(progress, AckError::Moved));
        }
        if self.file.write_all_at(&slot.1, slot.0).is_err() {
            return Err(fail(progress, AckError::NotDurable));
        }
        if (opts.at)(Point::SlotWritten(index + 1)) == Flow::Stop {
            return Err(AckError::Stopped);
        }
        let synced = if full_sync(step.effect()) {
            self.file.sync_all()
        } else {
            local::hand_to_drive(&self.file)
        };
        if synced.is_err() {
            return Err(fail(progress, AckError::NotDurable));
        }
        if (opts.at)(Point::SlotSynced(index + 1)) == Flow::Stop {
            return Err(AckError::Stopped);
        }
        if !self.ours() {
            return Err(fail(progress, AckError::Moved));
        }
        progress.acknowledge(plan, index, ack).map_err(AckError::Order)
    }

    /// The last step: takes the file away, flushes the folder, and acknowledges. Only when `RetireBarrier` is next.
    pub fn retire(self, plan: &Plan, progress: &mut Progress) -> Result<Outcome, AckError> {
        self.retire_with(plan, progress, &Opts::standard())
    }

    pub fn retire_with(self, plan: &Plan, progress: &mut Progress, opts: &Opts<'_>) -> Result<Outcome, AckError> {
        let index = progress.acked();
        if plan.digest() != self.digest {
            return Err(AckError::Order(OrderError::WrongPlan));
        }
        if plan.steps().get(index as usize).map(|s| s.effect()) != Some(Effect::RetireBarrier) || progress.next(plan).is_none() {
            return Err(AckError::NotAtRetire);
        }
        let fail = |progress: &mut Progress, error: AckError| {
            let _ = progress.acknowledge(plan, index, Ack::Failed);
            error
        };
        if !self.ours() {
            return Err(fail(progress, AckError::Moved));
        }
        if let Err(error) = self.folder.unlink_in(OsStr::new(NAME), false) {
            return Err(fail(progress, AckError::Io(error.kind())));
        }
        if (opts.at)(Point::Unlinked) == Flow::Stop {
            return Err(AckError::Stopped);
        }
        if self.folder.sync().is_err() {
            return Err(fail(progress, AckError::NotDurable));
        }
        if (opts.at)(Point::UnlinkSynced) == Flow::Stop {
            return Err(AckError::Stopped);
        }
        progress.acknowledge(plan, index, Ack::Done).map_err(AckError::Order)
    }
}

/// Holds the record again after a restart, once it was decoded by [`look`]: the file at the name must still be the one decoded.
pub fn reopen(root: &Path, recorded: &Recorded) -> Result<Held, AbandonError> {
    let folder = match open_dir(root) {
        Ok(Some(folder)) => folder,
        Ok(None) => return Err(AbandonError::Gone),
        Err(()) => return Err(AbandonError::NotOurs),
    };
    match read_record(&folder, MAX_BARRIER_BYTES) {
        Ok(Some((Decoded::Pending(now), file))) if now.body_sha() == recorded.body_sha() && now.layout() == recorded.layout() => {
            let meta = file.metadata().map_err(|e| AbandonError::Io(e.kind()))?;
            Ok(Held {
                folder,
                file,
                dev: meta.dev(),
                ino: meta.ino(),
                layout: recorded.layout(),
                digest: recorded.plan().digest(),
            })
        }
        Ok(_) => Err(AbandonError::Gone),
        Err(kind) => Err(AbandonError::Io(kind)),
    }
}

/// Takes the record away, only while no slot is written (the barrier was not acknowledged, so nothing was purged): the person cancels, or the app
/// starts again after a begin that was cut short.
pub fn abandon(root: &Path, recorded: &Recorded) -> Result<(), AbandonError> {
    let held = reopen(root, recorded)?;
    // The file now, not the decoding of a while ago: a slot may have been written since.
    let (decoded, _) = match read_record(&held.folder, MAX_BARRIER_BYTES) {
        Ok(Some(found)) => found,
        Ok(None) => return Err(AbandonError::Gone),
        Err(kind) => return Err(AbandonError::Io(kind)),
    };
    match decoded {
        Decoded::Pending(now) if now.acked() == now.layout().base() => {}
        Decoded::Pending(_) => return Err(AbandonError::RecoveryRequired),
        _ => return Err(AbandonError::Gone),
    }
    if !held.ours() {
        return Err(AbandonError::Gone);
    }
    held.folder.unlink_in(OsStr::new(NAME), false).map_err(|e| AbandonError::Io(e.kind()))?;
    held.folder.sync().map_err(|e| AbandonError::Io(e.kind()))
}

#[cfg(test)]
mod tests;
