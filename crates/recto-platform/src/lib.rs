//! The only crate that names an operating system. Holds no policy.

use std::path::Path;
use std::time::UNIX_EPOCH;

/// What identifies a file's version without reading it. Compared to notice a replaced file. Device and inode say which file it is, wherever it's been moved to on that volume, and they're zero where there's no such thing.
// size: 32 B. Held per tab, including cold ones, in `TabRecord`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub size: u64,
    pub mtime_ns: i64,
    pub device: u64,
    pub inode: u64,
}

impl FileStamp {
    /// Whether two looks at a file saw the same contents, going by size and time. The file's own number may differ: a volume can come back under another device number, and a backup comes back under another inode.
    pub fn same_contents(self, other: Self) -> bool {
        self.size == other.size && self.mtime_ns == other.mtime_ns
    }
}

/// Reads a file's size, modification time and number.
pub fn file_stamp(path: &Path) -> std::io::Result<FileStamp> {
    let meta = std::fs::metadata(path)?;
    let mtime_ns = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    };
    #[cfg(not(unix))]
    let (device, inode) = (0, 0);
    Ok(FileStamp { size: meta.len(), mtime_ns, device, inode })
}

/// What tells one version of a file from another, stronger than [`FileStamp`]: a file put in its place has another inode, and writing to it or setting its modification time moves its change time, which can't be set back without moving the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_ns: i64,
    pub ctime_ns: i64,
}

/// An open file's identity.
#[cfg(unix)]
pub fn file_identity(file: &std::fs::File) -> std::io::Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata()?;
    let nanos = |seconds: i64, nanos: i64| seconds.saturating_mul(1_000_000_000).saturating_add(nanos);
    Ok(FileIdentity { device: meta.dev(), inode: meta.ino(), size: meta.size(), mtime_ns: nanos(meta.mtime(), meta.mtime_nsec()), ctime_ns: nanos(meta.ctime(), meta.ctime_nsec()) })
}

/// An open file's identity. Windows has no inode or change time through the standard library, so its contents' samples carry more of the weight there. Not used at run time yet.
#[cfg(not(unix))]
pub fn file_identity(file: &std::fs::File) -> std::io::Result<FileIdentity> {
    let meta = file.metadata()?;
    let nanos = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
    Ok(FileIdentity { device: 0, inode: 0, size: meta.len(), mtime_ns: nanos, ctime_ns: nanos })
}

/// A file's device and inode, with no opening of it: opening a pipe would wait for a writer for ever. `None` where there's no such thing, or it can't be looked at.
pub fn file_device_and_inode(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|meta| (meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Puts this process at background priority: slower CPUs and slower disk, so it gets out of the way of whatever someone's waiting for. A failure leaves it as it was, which only costs politeness. The children it starts after this inherit it.
pub fn run_in_background() {
    isolate::run_in_background();
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
    /// How much processor time it has had.
    pub cpu_ns: u64,
}

/// How much of this Mac's memory is free, as a percentage: `kern.memorystatus_level`, the kernel's own measure of what it could hand out. `None` where it isn't
/// implemented or can't be read, which callers must take as "don't know". Only the reading lives here; what to do about it belongs to the caller.
pub fn memory_free_percent() -> Option<u32> {
    isolate::memory_free_percent()
}

#[cfg_attr(target_os = "macos", path = "isolate_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "isolate_stub.rs")]
pub mod isolate;

/// The control socket's OS calls: one blocking wait over many local sockets, the peer's user, and files and folders private to this user.
#[cfg_attr(target_os = "macos", path = "local_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "local_stub.rs")]
pub mod local;

/// Pixel memory the compositor can show directly, and the way it reaches a worker with no copy.
#[cfg_attr(target_os = "macos", path = "surface_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "surface_stub.rs")]
pub mod surface;
