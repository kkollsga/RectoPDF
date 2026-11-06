//! The only crate that names an operating system. Holds no policy.

use std::path::Path;
use std::time::UNIX_EPOCH;

/// What identifies a file's version without reading it. Compared to notice a replaced file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub size: u64,
    pub mtime_ns: i64,
}

/// Reads a file's size and modification time.
pub fn file_stamp(path: &Path) -> std::io::Result<FileStamp> {
    let meta = std::fs::metadata(path)?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
    Ok(FileStamp { size: meta.len(), mtime_ns })
}

/// This process's physical footprint in bytes, the number Activity Monitor shows. `None` where it isn't implemented.
#[cfg(target_os = "macos")]
pub fn footprint_bytes() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: the buffer matches the requested flavor and lives for the whole call; the kernel fills it on success.
    #[allow(unsafe_code)]
    let info = unsafe {
        let rc = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            info.as_mut_ptr().cast::<libc::rusage_info_t>(),
        );
        if rc != 0 {
            return None;
        }
        info.assume_init()
    };
    Some(info.ri_phys_footprint)
}

#[cfg(not(target_os = "macos"))]
pub fn footprint_bytes() -> Option<u64> {
    None
}

/// How many file descriptors this process has open. `None` where it isn't implemented.
#[cfg(target_os = "macos")]
pub fn open_handle_count() -> Option<usize> {
    // Listing the folder itself opens one descriptor, which isn't counted.
    std::fs::read_dir("/dev/fd").ok().map(|dir| dir.count().saturating_sub(1))
}

#[cfg(not(target_os = "macos"))]
pub fn open_handle_count() -> Option<usize> {
    None
}
