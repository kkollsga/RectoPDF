//! **The record's lock, taken by a test,** standing in for another copy of the app that keeps the record (`run/session.lock`, `recto-control`'s
//! `record_store::lock`).
//!
//! **A run that's ended can still hold it for a moment.** A run is gone to the harness when the system can't be asked about its process any more and nothing
//! answers on its socket; the kernel closes the process's descriptors after that, and the lock goes only with the last of them. So a test that takes the lock
//! right after a quit can find the run it just ended still holding it: CI run 43584994981 did, under a load of 51 on 3 cores (failed 1 of 5 runs there). The app itself waits
//! for it at a launch (its store's patience, then its tries again); a test that takes it waits too, bounded, and says who held it if it never came free.

use super::State;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::time::{Duration, Instant};

/// Past any process's end: the kernel's teardown takes milliseconds, and the app's own tries again come every 10 s.
const PATIENCE: Duration = Duration::from_secs(10);

/// Takes the record's lock of `state` for the test, once whatever held it has let go, and keeps it until the file is dropped. Fails, saying who held it by
/// what its taker wrote in the file, if it isn't free within [`PATIENCE`].
pub fn take_once_free(state: &State) -> File {
    let path = state.root.join("run/session.lock");
    let lock = OpenOptions::new().create(true).read(true).write(true).truncate(false).open(&path).unwrap();
    let began = Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => return lock,
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(error)) => panic!("the record's lock {} couldn't be taken: {error}", path.display()),
        }
        assert!(
            began.elapsed() < PATIENCE,
            "the record's lock {} was still held after {:?}: {}",
            path.display(),
            began.elapsed(),
            held_by(&lock)
        );
        std::thread::sleep(Duration::from_millis(10)); // ratchet: poll
    }
}

/// Who holds it, from the `<process> <store>` its taker writes, and whether that process is still there.
fn held_by(mut lock: &File) -> String {
    let mut said = String::new();
    let _ = Read::take(&mut lock, 64).read_to_string(&mut said);
    let pid = said.split_whitespace().next().and_then(|pid| pid.parse::<u32>().ok());
    match pid {
        Some(pid) if recto_platform::local::process_exists(pid) => format!("its file says process {pid}, which is running ({})", said.trim()),
        Some(pid) => format!("its file says process {pid}, which has ended ({})", said.trim()),
        None => format!("its file doesn't say who: {said:?}"),
    }
}
