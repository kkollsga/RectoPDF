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
    Ok(stamp_of(&std::fs::metadata(path)?))
}

/// [`file_stamp`] of a file already open: the version that was opened, whatever's at its path now.
pub fn open_file_stamp(file: &std::fs::File) -> std::io::Result<FileStamp> {
    Ok(stamp_of(&file.metadata()?))
}

fn stamp_of(meta: &std::fs::Metadata) -> FileStamp {
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
    FileStamp {
        size: meta.len(),
        mtime_ns,
        device,
        inode,
    }
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
    Ok(FileIdentity {
        device: meta.dev(),
        inode: meta.ino(),
        size: meta.size(),
        mtime_ns: nanos(meta.mtime(), meta.mtime_nsec()),
        ctime_ns: nanos(meta.ctime(), meta.ctime_nsec()),
    })
}

/// An open file's identity. Windows has no inode or change time through the standard library, so its contents' samples carry more of the weight there. Not used at run time yet.
#[cfg(not(unix))]
pub fn file_identity(file: &std::fs::File) -> std::io::Result<FileIdentity> {
    let meta = file.metadata()?;
    let nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
    Ok(FileIdentity {
        device: 0,
        inode: 0,
        size: meta.len(),
        mtime_ns: nanos,
        ctime_ns: nanos,
    })
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

/// This process's own memory, three ways. The footprint is what Activity Monitor shows. `compressed` is the part of it in the compressor, which moves with the whole Mac's memory pressure: a page of a system library's data the app never touched again isn't charged while it's resident and shared, and is once the system compresses it, so the footprint can rise by megabytes with nothing done. `heap` is what's allocated and not freed, in every malloc zone, which is where a leak of this app's or AppKit's shows, and which pressure doesn't move.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnMemory {
    pub footprint: u64,
    pub compressed: u64,
    pub heap: u64,
}

/// See [`OwnMemory`]. `None` where it isn't implemented.
pub fn own_memory() -> Option<OwnMemory> {
    isolate::own_memory()
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

/// A helper process on pipes, at a scheduling level set for the whole of it as it starts.
#[cfg_attr(target_os = "macos", path = "helper_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "helper_stub.rs")]
pub mod helper;

/// The control socket's OS calls: one blocking wait over many local sockets, the peer's user, and files and folders private to this user.
#[cfg_attr(target_os = "macos", path = "local_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "local_stub.rs")]
pub mod local;

/// What Launch Services has registered of the app's copies, and forgetting a copy that isn't the one install: a test's, a build's, a quiet run's.
#[cfg_attr(target_os = "macos", path = "registry_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "registry_stub.rs")]
pub mod registry;

/// Another thread's CPU time and run state, for telling a stalled thread that waits from one that isn't given a core.
#[cfg_attr(target_os = "macos", path = "thread_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "thread_stub.rs")]
pub mod thread;

/// A folder of a run's own in a temp folder, never one already there, removed when it's dropped.
pub mod fresh;

/// A quiet run's own folders, the only places a session on a test state folder opens or writes a file.
pub mod run_folders;

/// A password the person typed, held so its bytes are overwritten when it goes, and the file identity it's kept by.
pub mod secret;
pub use secret::{MAX_PASSWORD_BYTES, Password};

/// **Whether this was built as a release:** `RECTO_RELEASE` set when it's compiled, by `app/release-zip.sh`, and it can't be turned on later. The one place it's read: the updater, the renderer's host and the slow mode all go by this (review 1: three crates each read it, and nothing held them to agree).
pub const RELEASE_BUILD: bool = option_env!("RECTO_RELEASE").is_some();

/// A test's slow mode: every waiting point made late on purpose, in a quiet run of a build that isn't a release.
pub mod slow;

/// What the window server says is on screen, for tests that prove a quiet run shows nothing.
#[cfg_attr(target_os = "macos", path = "screen_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "screen_stub.rs")]
pub mod screen;

/// Pixel memory the compositor can show directly, and the way it reaches a worker with no copy.
#[cfg_attr(target_os = "macos", path = "surface_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "surface_stub.rs")]
pub mod surface;

/// The renderer's walls from the inside: whether the App Sandbox is on, and the drop of every Mach right that reaches another process.
#[cfg_attr(target_os = "macos", path = "wall_mac.rs")]
#[cfg_attr(not(target_os = "macos"), path = "wall_stub.rs")]
pub mod wall;
