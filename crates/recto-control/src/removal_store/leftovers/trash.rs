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
}

impl FolderTrash {
    /// A Trash that is `folder` (made by the caller, on the same volume as what is moved into it).
    pub fn at(folder: &Path) -> Self {
        Self {
            folder: Some(folder.to_path_buf()),
            count: AtomicU64::new(0),
        }
    }

    /// A volume with no Trash.
    pub fn without_trash() -> Self {
        Self {
            folder: None,
            count: AtomicU64::new(0),
        }
    }
}

impl Trash for FolderTrash {
    fn has_trash_for(&self, _path: &Path) -> bool {
        self.folder.is_some()
    }

    fn move_to_trash(&self, path: &Path) -> Result<PathBuf, Failure> {
        let folder = self.folder.as_ref().ok_or(Failure::Refused(0))?;
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
