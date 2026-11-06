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
pub fn footprint_bytes() -> Option<u64> {
    isolate::process_memory(std::process::id()).map(|memory| memory.footprint_bytes)
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

/// Fills `buf` from `offset` without moving the file's position, so readers never share a cursor. Fails on a short read.
#[cfg(unix)]
pub fn read_exact_at(file: &std::fs::File, buf: &mut [u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

/// Fills `buf` from `offset`. Windows moves the file's position as it reads, which doesn't matter here because every read says where.
#[cfg(windows)]
pub fn read_exact_at(file: &std::fs::File, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let read = file.seek_read(buf, offset)?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[read..];
        offset += read as u64;
    }
    Ok(())
}

/// A process's memory as Activity Monitor counts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessMemory {
    pub footprint_bytes: u64,
    /// The highest the footprint has been in this process's life.
    pub peak_footprint_bytes: u64,
}

/// How much of this Mac's memory is free, as a percentage: `kern.memorystatus_level`, the kernel's own measure of what it could hand out. `None` where it isn't
/// implemented or can't be read, which callers must take as "don't know". Only the reading lives here; what to do about it belongs to the caller.
pub fn memory_free_percent() -> Option<u32> {
    isolate::memory_free_percent()
}

#[cfg_attr(target_os = "macos", path = "isolate_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "isolate_stub.rs")]
pub mod isolate;
