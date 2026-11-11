//! **A folder of a run's own, never one already there,** for shipping code that works in a temp folder for a while: `recto budget` and `recto qa
//! budget`. A process id comes round again, and a run killed partway leaves its folder behind, so a folder named by the id, or the id and a count,
//! was another run's, with its files still in it (2026-11-11: 707 such folders in the temp folder, from tests named the same way). Only the last part
//! of the path is made, with `create_dir`, and a name already there is passed over for the next. Private to this user, so it can be a state folder,
//! which `StatePaths::prepare` holds to that. Removed with everything in it when dropped, a panic's unwinding too.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// How many names it tries before saying so: each taken name is a folder someone left, and past this many something else is wrong.
const MOST_TRIES: u32 = 1_000;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A fresh folder, removed when dropped. See the module's notes.
#[derive(Debug)]
pub struct FreshDir(PathBuf);

impl FreshDir {
    /// `<base>/<prefix><pid>-<n><suffix>`, for the first `n` not already there. `base` must exist. The suffix is for a name's end, such as `.noindex`
    /// to keep Spotlight out.
    pub fn new_in(base: &Path, prefix: &str, suffix: &str) -> io::Result<Self> {
        for _ in 0..MOST_TRIES {
            let path = base.join(format!("{prefix}{}-{}{suffix}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match make_private(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{MOST_TRIES} names in {} were all taken", base.display()),
        ))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for FreshDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn make_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn make_private(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a run's folder is one an earlier run with the same process id left behind, and it finds that run's files beside its own, or its
    /// folder can't be a state folder, or stays behind after the run, a panic's included.
    #[test]
    fn a_fresh_folder_is_never_one_left_behind_and_goes_with_the_run() {
        let base = recto_testpdf::TempDir::new("rfr");
        let next = NEXT.load(Ordering::Relaxed);
        let left = base.join(format!("run-{}-{next}", std::process::id()));
        std::fs::create_dir(&left).unwrap();
        std::fs::write(left.join("theirs.pdf"), b"x").unwrap();

        let fresh = FreshDir::new_in(&base, "run-", ".noindex").unwrap();
        assert_ne!(fresh.path(), left, "took the folder left behind");
        assert!(fresh.path().file_name().unwrap().to_string_lossy().ends_with(".noindex"));
        assert_eq!(std::fs::read_dir(fresh.path()).unwrap().count(), 0, "a fresh folder had something in it");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(fresh.path()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let path = fresh.path().to_path_buf();
        std::fs::write(path.join("mine.pdf"), b"x").unwrap();
        drop(fresh);
        assert!(!path.exists(), "the folder stayed after its run");
        assert!(left.join("theirs.pdf").exists(), "someone else's folder was touched");

        let made = std::sync::Mutex::new(None);
        let unwound = std::panic::catch_unwind(|| {
            let fresh = FreshDir::new_in(&base, "run-", "").unwrap();
            std::fs::write(fresh.path().join("mine.pdf"), b"x").unwrap();
            *made.lock().unwrap() = Some(fresh.path().to_path_buf());
            panic!("a run that fails partway");
        });
        assert!(unwound.is_err());
        let path = made.lock().unwrap().take().unwrap();
        assert!(!path.exists(), "a panic left the folder behind");
    }

    /// Failure: a base that can't be written in loops for ever, or panics, instead of saying so.
    #[test]
    fn a_base_that_isnt_there_is_an_error() {
        let base = recto_testpdf::TempDir::new("rfr");
        let error = FreshDir::new_in(&base.join("not-there"), "run-", "").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
