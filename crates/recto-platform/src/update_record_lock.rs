//! The app record's existing writer lock, held by rollback only while mutating.
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Tries the same exclusive lock the window's RecordStore holds.
/// A FIFO cannot stall opening; symlinks, foreign owners and non-files fail.
///
/// # Errors
/// The private folder/file is unsafe, another writer holds the lock, or an OS
/// operation fails. The caller must preserve recovery files on any error.
pub fn try_update_record_lock(root: &Path) -> io::Result<File> {
    let folder = root.join("run");
    super::ensure_private_dir(&folder)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(folder.join("session.lock"))?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != super::current_uid() {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    file.try_lock()?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: rollback uses another lock inode or ignores the app's writer.
    #[test]
    fn shares_the_record_stores_writer_lock() {
        let root = recto_testpdf::TempDir::new("update-record-lock-");
        let held = try_update_record_lock(root.path()).unwrap();
        let other = OpenOptions::new().read(true).write(true).open(root.join("run/session.lock")).unwrap();
        assert!(other.try_lock().is_err());
        assert!(try_update_record_lock(root.path()).is_err());
        drop(held);
        assert!(other.try_lock().is_ok());
    }

    /// Failure: a planted link or FIFO is accepted as the record's writer lock.
    #[test]
    fn refuses_links_and_special_files() {
        let root = recto_testpdf::TempDir::new("update-record-special-");
        let folder = root.join("run");
        super::super::ensure_private_dir(&folder).unwrap();
        let lock = folder.join("session.lock");
        let target = root.join("other");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, &lock).unwrap();
        assert!(try_update_record_lock(root.path()).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        std::fs::remove_file(&lock).unwrap();
        let name = std::ffi::CString::new(lock.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: live NUL-terminated path inside this test's private directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let at = root.path().to_path_buf();
        assert!(recto_testpdf::returns_in_time("the record lock on a pipe", move || try_update_record_lock(&at).is_err()));
    }
}
