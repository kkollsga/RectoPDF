//! The leases a clear holds from before its recheck to after its last act (`docs/design/removal.md` §10, D5). Each is **tried once and never waited for**: one
//! that isn't had is `Busy`, with nothing done.
//!
//! - **The installer's lock** (`update/installer.lock`): blocks the installer's `copy_record`, `restore_record` and the swap, which touch `update/before` and
//!   the session copies. An installer started meanwhile waits its quit limit and stands down. Taken only if `update/` is a real folder: it is never made just
//!   to be locked, since an installer needs its plan there. [`Leases::look_again`] looks again for it after the recheck and before the confirmation is taken,
//!   and takes the lock then, or refuses, so a refusal there uses no plan up and the clear never acts with an update folder present and the lock not held.
//!   **The updater's week-old cleanup of `update/before` does not take this lock** (it runs in this app, on the launch's own thread). The race is harmless: it
//!   and the clear only delete, the clear's walk counts a pinned name already gone as done (`already_gone`), and each deletes the update's copy whole, so
//!   whichever is second finds nothing or the rest of the same copy.
//! - **The windowless session's lock** (`run/session-windowless.lock`): blocks a `recto session` load, where the `.damaged` rotation and the `.v<N>` copy happen.
//! - **What only the app knows** ([`Outside`]): it keeps its own record (`session.lock`), the record isn't beside a newer app's, and the sleeper's pass over older
//!   sleep files, which touches `update/before/journals`, has finished. The app's own rotation runs only in `load`, before any command is taken.
//!
//! The locks are `flock`s on a descriptor kept here: dropped, or the process ended, they go.

use recto_platform::local;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Where the installer's lock is under the state folder: `update/installer.lock`, as `recto_update::plan::Files::installer_lock` has it (a test below holds the two
/// together, so this crate needs no dependency on the updater).
pub const INSTALLER_LOCK: [&str; 2] = ["update", "installer.lock"];

/// The windowless session's lock under the state folder, as the record store names it (`run/<flavor folder>.lock`).
const WINDOWLESS_LOCK: [&str; 2] = ["run", "session-windowless.lock"];

/// What the app knows that no folder says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outside {
    /// This app keeps its record: it holds `run/session.lock`.
    pub keeps_record: bool,
    /// The record store is beside a newer app's record, whose folder is then a copy's name.
    pub beside_newer: bool,
    /// The sleeper's pass over older sleep files has ended.
    pub sleep_pass_done: bool,
}

/// Why a lease was not had. Nothing was done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    /// An installer, or the week's cleanup, holds its lock.
    Installer,
    /// A `recto session` (the windowless kind) is running.
    Windowless,
    /// The app is still tidying up after starting: it doesn't keep its record yet, or the sleeper's pass hasn't ended.
    Starting,
    /// A newer app's record is beside this one: the clear is that version's.
    BesideNewer,
    /// An unfinished removal's record is in the state folder (or something is where it would be that can't be read): a clear's own steps would move and delete
    /// files the removal's plan has pinned, and the one thing that may do that is the removal finishing (`docs/design/removal.md` §6).
    RemovalPending,
}

/// The leases, held until this is dropped.
#[derive(Debug)]
pub struct Leases {
    root: PathBuf,
    installer: Option<File>,
    _windowless: File,
}

fn real_folder(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_dir())
}

/// Tries to lock the file at `path` once. `Ok(None)` when it can't be had because it is held.
fn try_lock(path: &Path) -> Result<Option<File>, std::io::Error> {
    let file = local::open_private_for_writing(path, true)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

fn installer_path(root: &Path) -> PathBuf {
    INSTALLER_LOCK.iter().fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Takes the lease on the installer's lock if the folder is there. `Err` when it is there and the lock is held, or can't be tried (a lock that can't be tried
/// is not a lock had).
fn installer_lease(root: &Path) -> Result<Option<File>, Busy> {
    if !real_folder(&root.join(INSTALLER_LOCK[0])) {
        return Ok(None);
    }
    match try_lock(&installer_path(root)) {
        Ok(Some(file)) => Ok(Some(file)),
        Ok(None) | Err(_) => Err(Busy::Installer),
    }
}

impl Leases {
    /// Takes every lease once, or says which was not had. The record's own lease and the sleeper's are the app's to say ([`Outside`]).
    pub fn take(root: &Path, outside: Outside) -> Result<Self, Busy> {
        if outside.beside_newer {
            return Err(Busy::BesideNewer);
        }
        if !outside.keeps_record || !outside.sleep_pass_done {
            return Err(Busy::Starting);
        }
        let windowless = match try_lock(&WINDOWLESS_LOCK.iter().fold(root.to_path_buf(), |path, part| path.join(part))) {
            Ok(Some(file)) => file,
            Ok(None) | Err(_) => return Err(Busy::Windowless),
        };
        let installer = installer_lease(root)?;
        // After the locks, so what is found is found with nothing else moving: a record that appears now is the removal's own, and still stops this.
        if crate::removal_store::gate::anything_there(root) {
            return Err(Busy::RemovalPending);
        }
        Ok(Self {
            root: root.to_path_buf(),
            installer,
            _windowless: windowless,
        })
    }

    /// Whether the installer's lock is held by this.
    pub fn holds_installer(&self) -> bool {
        self.installer.is_some()
    }

    /// Looks for `update/` again, right before the first act: if it has appeared since the leases were taken, takes its lock now, or is `Busy`. Never acts with an
    /// update folder there and the lock not held.
    pub fn look_again(&mut self) -> Result<(), Busy> {
        if self.installer.is_none() {
            self.installer = installer_lease(&self.root)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_testpdf::TempDir;

    fn root() -> TempDir {
        let root = TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-leases-");
        std::fs::create_dir(root.join("run")).unwrap();
        root
    }

    const READY: Outside = Outside {
        keeps_record: true,
        beside_newer: false,
        sleep_pass_done: true,
    };

    /// Failure: the constant names another file than the installer's, so the clear leases a lock the installer never takes and runs beside an installer.
    #[test]
    fn the_installer_lock_is_the_updaters_own() {
        let files = recto_update::plan::Files::in_state(Path::new("/state"));
        assert_eq!(installer_path(Path::new("/state")), files.installer_lock());
    }

    /// Failure: an installer holding its lock doesn't stop the clear, so a copy it is reading or restoring is deleted under it.
    #[test]
    fn an_installer_holding_its_lock_stops_the_clear() {
        let root = root();
        std::fs::create_dir(root.join("update")).unwrap();
        let installer = try_lock(&root.join("update").join("installer.lock")).unwrap().expect("the installer's lock");
        assert_eq!(Leases::take(&root, READY).unwrap_err(), Busy::Installer);
        drop(installer);
        assert!(Leases::take(&root, READY).unwrap().holds_installer());
    }

    /// Failure: a `recto session` holding its lock doesn't stop the clear, so a copy it is about to rotate or make is deleted under it.
    #[test]
    fn a_windowless_session_holding_its_lock_stops_the_clear() {
        let root = root();
        let session = try_lock(&root.join("run").join("session-windowless.lock")).unwrap().expect("its lock");
        assert_eq!(Leases::take(&root, READY).unwrap_err(), Busy::Windowless);
        drop(session);
        assert!(Leases::take(&root, READY).is_ok());
    }

    /// Failure: the lease creates `update/` to lock it, leaving a folder an installer never made, or takes no lease and acts with a folder present.
    #[test]
    fn update_is_never_made_to_be_locked_and_is_looked_for_again() {
        let root = root();
        let mut leases = Leases::take(&root, READY).unwrap();
        assert!(!root.join("update").exists(), "never made to be locked");
        assert!(!leases.holds_installer());
        // It appears before the first act, free: taken then.
        std::fs::create_dir(root.join("update")).unwrap();
        leases.look_again().unwrap();
        assert!(leases.holds_installer());
    }

    /// Failure: `update/` appears after the leases and its lock is held by an installer, and the clear goes on without it.
    #[test]
    fn an_update_folder_that_appears_with_its_lock_held_stops_the_clear_before_the_first_act() {
        let root = root();
        let mut leases = Leases::take(&root, READY).unwrap();
        std::fs::create_dir(root.join("update")).unwrap();
        let _installer = try_lock(&root.join("update").join("installer.lock")).unwrap().expect("the installer's lock");
        assert_eq!(leases.look_again().unwrap_err(), Busy::Installer);
    }

    /// Failure (D3b): a clear's own moves and deletes run while an unfinished removal's record is in the state folder, in whatever shape (one that can't be
    /// read as much as a good one), against files the removal's plan has pinned. No lease is given, and a state folder with none gives it as before.
    #[test]
    fn an_unfinished_removal_stops_the_clear() {
        use crate::removal_store::barrier::fixture::{Kind, seed};
        for kind in [Kind::Pending(1), Kind::Garbage, Kind::TooNew, Kind::FolderIsFile] {
            let root = root();
            seed(root.path(), kind);
            assert_eq!(Leases::take(&root, READY).unwrap_err(), Busy::RemovalPending, "{kind:?}");
        }
        let root = root();
        seed(root.path(), Kind::LedgerOnly);
        assert!(Leases::take(&root, READY).is_ok(), "the ledger alone is no record");
    }

    /// Failure: a clear runs while the app is still starting, beside a newer record, or without keeping its own.
    #[test]
    fn what_only_the_app_knows_is_asked() {
        let root = root();
        let busy = |outside| Leases::take(&root, outside).unwrap_err();
        assert_eq!(busy(Outside { beside_newer: true, ..READY }), Busy::BesideNewer);
        assert_eq!(busy(Outside { keeps_record: false, ..READY }), Busy::Starting);
        assert_eq!(
            busy(Outside {
                sleep_pass_done: false,
                ..READY
            }),
            Busy::Starting
        );
    }
}
