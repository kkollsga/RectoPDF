//! **The record's lock:** whether this process keeps the record, waiting a moment for it, and letting go of it as a process that ends does.
//!
//! **Letting go unlocks, then closes.** A lock on a file belongs to its open description, not to one descriptor: every copy of the descriptor holds it, and it
//! goes only when the last copy closes. A worker being started on another thread has a copy of every descriptor this process has, until its exec closes them, so
//! closing ours alone could leave the record held for as long as that start takes (2026-11-13: a test's next app found it held, twice, under load). An unlock
//! frees the description whatever copies are open.
//!
//! **Who has it, when it isn't ours.** The one that takes it writes its process and its store's number into the lock file, and a store that didn't get it keeps
//! why, from that: another process, a store of this one that still keeps it, or one that has let go while a copy of its descriptor lived on.

use super::{LOCK_PATIENCE, RecordStore};
use recto_platform::local;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Each store's number in this process, for saying which one has the record.
static STORES: AtomicU64 = AtomicU64::new(1);

/// The stores of this process that keep a record now, by the lock's path and their number.
static KEEPING: Mutex<Vec<(PathBuf, u64)>> = Mutex::new(Vec::new());

fn keeping() -> std::sync::MutexGuard<'static, Vec<(PathBuf, u64)>> {
    KEEPING.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A new store's number.
pub(super) fn next_store() -> u64 {
    STORES.fetch_add(1, Ordering::Relaxed)
}

/// Takes the lock at `path` for store `store`, and says who it's from. Or why not, in words for a failure to show (the headless kit's start says it).
pub(super) fn take_lock(path: &Path, store: u64) -> Result<File, String> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("its lock file {} couldn't be opened: {error}", path.display()))?;
    if let Err(error) = file.try_lock() {
        return Err(match error {
            std::fs::TryLockError::WouldBlock => held_by(&file, path),
            std::fs::TryLockError::Error(error) => format!("its lock file {} couldn't be locked: {error}", path.display()),
        });
    }
    // Only a way to say who: a lock that's taken is the lock, whether or not this lands.
    let _ = file
        .set_len(0)
        .and_then(|()| (&file).write_all(format!("{} {store}\n", std::process::id()).as_bytes()));
    keeping().push((path.to_path_buf(), store));
    Ok(file)
}

/// Who holds the lock, from what its taker wrote.
fn held_by(file: &File, path: &Path) -> String {
    let mut said = String::new();
    let _ = std::io::Read::take(file, 64).read_to_string(&mut said);
    let mut words = said.split_whitespace().map(str::parse::<u64>);
    let (Some(Ok(pid)), Some(Ok(store))) = (words.next(), words.next()) else {
        return "it's held by something that didn't say who".to_owned();
    };
    if pid != u64::from(std::process::id()) {
        let running = u32::try_from(pid).is_ok_and(local::process_exists);
        return format!("it's held by process {pid}, {}", if running { "which is running" } else { "which has ended" });
    }
    if keeping().iter().any(|(at, by)| at == path && *by == store) {
        format!("it's held by record store #{store} of this process, which still keeps it")
    } else {
        format!(
            "it was taken by record store #{store} of this process, which has let go of it, and a copy of its descriptor still holds it (a process being started \
             from this one has a copy until its exec)"
        )
    }
}

/// A store's lock gone from what this process keeps, with nothing unlocked: for a test making a descriptor's copy outlive its close.
#[cfg(test)]
pub(super) fn forget_keeping(path: &Path, store: u64) {
    keeping().retain(|(at, by)| !(at == path && *by == store));
}

/// Unlocks and closes: see the module's words.
fn release(path: &Path, store: u64, file: File) {
    let _ = file.unlock();
    drop(file);
    keeping().retain(|(at, by)| !(at == path && *by == store));
}

impl RecordStore {
    /// Whether this process is the one keeping this record.
    pub fn holds_the_record(&self) -> bool {
        self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// Why this store isn't keeping the record, as it was when it last asked for it: `None` while it keeps it.
    pub fn why_not_keeping(&self) -> Option<String> {
        if self.holds_the_record() {
            return None;
        }
        self.refused.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// **Lets go of the record's lock, as a process that ends does:** from here this store writes nothing, whatever's still running, and another can take the
    /// record at once, whatever copies of its descriptor are still open. For good: a straggler of the ended app asking for it again doesn't get it back. Only a
    /// test's kill calls it (the headless kit's), as the app's own process ending does this.
    pub fn let_go(&self) {
        self.gone.store(true, Ordering::SeqCst);
        let file = self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
        if let Some(file) = file {
            release(&self.lock_path, self.number, file);
        }
        *self.refused.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some("this store has let go of it".to_owned());
    }

    /// Asks for the lock again, for a moment. An app that's just gone holds it until the kernel has finished with it, and starting empty beside its own record because of that would lose the very thing an update is for. On a thread that can wait.
    pub fn wait_for_lock(&self) -> bool {
        let began = Instant::now();
        loop {
            if self.holds_the_record() {
                return true;
            }
            if began.elapsed() >= LOCK_PATIENCE || self.gone.load(Ordering::SeqCst) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10)); // ratchet: poll
            self.ask_for_lock();
        }
    }

    /// **One try at the lock, never waiting:** a `try_lock`, and whether this store keeps the record now. For a launch that didn't get it at the start and tries
    /// again on its own deadline (`recto-app`'s `lifecycle::keeping`). A store that has let go doesn't get it back.
    pub fn try_to_keep(&self) -> bool {
        if !self.holds_the_record() {
            self.ask_for_lock();
        }
        self.holds_the_record()
    }

    /// One try at the lock, keeping it or why not.
    pub(super) fn ask_for_lock(&self) {
        if self.gone.load(Ordering::SeqCst) {
            return;
        }
        match take_lock(&self.lock_path, self.number) {
            Ok(file) => *self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file),
            Err(why) => *self.refused.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(why),
        }
    }
}

impl Drop for RecordStore {
    /// A store that's gone has let go, as [`RecordStore::let_go`] does.
    fn drop(&mut self) {
        self.let_go();
    }
}
