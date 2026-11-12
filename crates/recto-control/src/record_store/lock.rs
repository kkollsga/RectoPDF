//! **The record's lock:** whether this process keeps the record, waiting a moment for it, and letting go of it as a process that ends does.

use super::{LOCK_PATIENCE, RecordStore, take_lock};
use std::time::{Duration, Instant};

impl RecordStore {
    /// Whether this process is the one keeping this record.
    pub fn holds_the_record(&self) -> bool {
        self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    /// **Lets go of the record's lock, as a process that ends does:** from here this store writes nothing, whatever's still running, and another can take the
    /// record. Only a test's kill calls it (the headless kit's), as the app's own process ending does this.
    pub fn let_go(&self) {
        drop(self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take());
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
            std::thread::sleep(Duration::from_millis(10)); // ratchet: poll
            if let Some(file) = take_lock(&self.lock_path) {
                *self.lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(file);
            }
        }
    }
}
