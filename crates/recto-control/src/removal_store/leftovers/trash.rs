//! The Trash an act moves an unfinished save to, behind a trait so the act is tested, and a quiet run goes, with no real Trash touched.

use recto_platform::trash::Failure;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A place to move a file to that the person can empty and Put Back from: never a delete.
pub trait Trash: Send + Sync {
    /// Whether the volume `path` is on has a Trash for this user. `false` means the item stays where it is and is never deleted instead.
    fn has_trash_for(&self, path: &Path) -> bool;
    /// Moves `path` there and says where it went. On a failure it is where it was.
    fn move_to_trash(&self, path: &Path) -> Result<PathBuf, Failure>;
}

/// The Mac's own Trash of the volume, through Foundation (`recto_platform::trash`). Never made by a test or a quiet run.
pub struct MacTrash;

impl Trash for MacTrash {
    fn has_trash_for(&self, path: &Path) -> bool {
        recto_platform::trash::has_trash_for(path)
    }

    fn move_to_trash(&self, path: &Path) -> Result<PathBuf, Failure> {
        recto_platform::trash::move_to_trash(path)
    }
}

/// A folder standing in for the Trash: an item is renamed into it, as the real one does on its own volume. For tests and quiet runs, which never touch
/// the person's Trash. `without_trash` is a volume that has none.
pub struct FolderTrash {
    folder: Option<PathBuf>,
    count: AtomicU64,
    /// Only what is a quiet run's file ([`crate::quiet::is_a_quiet_runs_file`]) may be moved into it: a quiet run opened on a person's real document must never
    /// move its unfinished saves into a hidden folder.
    confined: bool,
}

impl FolderTrash {
    /// A Trash that is `folder` (made by the caller, on the same volume as what is moved into it).
    pub fn at(folder: &Path) -> Self {
        Self {
            folder: Some(folder.to_path_buf()),
            count: AtomicU64::new(0),
            confined: false,
        }
    }

    /// A stand-in for a quiet run or a test: [`Self::at`], that takes only a quiet run's files (in the system's temp folders), and says it has no Trash for any
    /// other. Made at the first move, never by a look.
    pub fn in_quiet_run(folder: &Path) -> Self {
        Self {
            confined: true,
            ..Self::at(folder)
        }
    }

    /// A volume with no Trash.
    pub fn without_trash() -> Self {
        Self {
            folder: None,
            count: AtomicU64::new(0),
            confined: false,
        }
    }
}

impl Trash for FolderTrash {
    fn has_trash_for(&self, path: &Path) -> bool {
        self.folder.is_some() && (!self.confined || crate::quiet::is_a_quiet_runs_file(path))
    }

    fn move_to_trash(&self, path: &Path) -> Result<PathBuf, Failure> {
        let folder = self.folder.as_ref().ok_or(Failure::Refused(0))?;
        if self.confined && !crate::quiet::is_a_quiet_runs_file(path) {
            return Err(Failure::Refused(2));
        }
        // Made at the first move: a check, which moves nothing, makes nothing.
        std::fs::create_dir_all(folder).map_err(|_| Failure::Refused(3))?;
        let name = path.file_name().ok_or(Failure::BadPath)?;
        let mut to = folder.join(name);
        // Finder's own rule when the name is taken: another name, never over what is there.
        if std::fs::symlink_metadata(&to).is_ok() {
            let mut numbered = name.to_os_string();
            numbered.push(format!(" {}", self.count.fetch_add(1, Ordering::Relaxed) + 1));
            to = folder.join(numbered);
        }
        std::fs::rename(path, &to).map_err(|_| Failure::Refused(1))?;
        Ok(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_testpdf::TempDir;

    /// Failure (review round 1, 2 and 9): a stand-in Trash takes a file outside the system's temp folders (a quiet run opened on a person's real document moves
    /// its unfinished save into a hidden folder), or is made by a look. Inside the temp folders it moves; outside, it has no Trash for the path and refuses;
    /// and it is made by the first move and not before.
    #[test]
    fn a_quiet_runs_trash_takes_only_a_quiet_runs_files_and_is_made_by_the_first_move() {
        let temp = TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-trash-");
        let beside = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .canonicalize()
            .expect("the checkout's target folder");
        let outside = TempDir::new_in(&beside, "recto-not-temp-");
        let (inside_file, outside_file) = (temp.join("a.tmp"), outside.join("b.tmp"));
        std::fs::write(&inside_file, b"x").unwrap();
        std::fs::write(&outside_file, b"y").unwrap();
        let folder = temp.join("stand-in");
        let trash = FolderTrash::in_quiet_run(&folder);
        assert!(trash.has_trash_for(&inside_file) && !trash.has_trash_for(&outside_file));
        assert!(!folder.exists(), "a look makes nothing");
        assert!(
            trash.move_to_trash(&outside_file).is_err() && outside_file.exists(),
            "outside the temp folders nothing is moved"
        );
        assert!(!folder.exists(), "a refusal makes nothing either");
        let moved = trash.move_to_trash(&inside_file).expect("a quiet run's file");
        assert!(moved.starts_with(&folder) && moved.exists() && !inside_file.exists());
    }
}
