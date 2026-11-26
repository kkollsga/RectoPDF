//! What the surveys' tests share: a file version, limits, a pipe (the bounded run is `recto_testpdf::returns_in_time`). Only compiled for tests.

use recto_core::removal::{FileVersion, Limits};
use std::num::NonZeroU16;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// What a test runs with a path just before it is used: its way to change the path under a survey.
pub(crate) type Hook = Arc<dyn Fn(&Path) + Send + Sync>;

pub(super) fn version(inode: u64) -> FileVersion {
    FileVersion {
        volume: Some([3; 16]),
        inode,
        size: 10,
        mtime_ns: 1,
    }
}

pub(super) fn limits(max: u32) -> Limits {
    Limits {
        max_artifacts: max,
        max_batch: NonZeroU16::new(4).unwrap(),
    }
}

pub(super) fn put(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

pub(super) fn mkfifo(path: &Path) {
    assert!(std::process::Command::new("mkfifo").arg(path).status().unwrap().success());
}

pub(super) fn mode(path: &Path, bits: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits)).unwrap();
}

/// Puts a mode back on drop so a failed run can't leave a folder a `TempDir` can't remove.
pub(super) struct Restore(pub PathBuf);

impl Drop for Restore {
    fn drop(&mut self) {
        mode(&self.0, 0o700);
    }
}
