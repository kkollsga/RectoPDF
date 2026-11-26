//! A folder held by its descriptor, so a path swapped for a link after the check is not followed (`docs/design/removal.md` §10, D2c-1 review).
//!
//! A path check followed by a listing by path is two looks, and `O_NOFOLLOW` guards only the last part of a path: the folder can be swapped for a
//! link in between and the listing follows it. Here the folder is opened once, with `O_DIRECTORY | O_NOFOLLOW`, checked to be this user's by
//! `fstat` on that descriptor, and then listed (`fdopendir`), looked at (`fstatat` with `AT_SYMLINK_NOFOLLOW`) and opened from (`openat` with
//! `O_NOFOLLOW | O_NONBLOCK`) through the descriptor alone. A child folder is opened the same way from its parent's descriptor, so a chain below a
//! root the app made is never resolved by path again.
//!
//! **Measured on this Mac (macOS 27, APFS, 2026-11-26; the tests below hold each line).** `open(link, O_DIRECTORY|O_NOFOLLOW)` fails (`ELOOP` or
//! `ENOTDIR`, never a descriptor); a missing name is `ENOENT`; `fdopendir` on a fresh `openat(fd, ".")` of the descriptor lists the folder that was opened even
//! after its path was renamed away and a link put in its place; `fstatat(AT_SYMLINK_NOFOLLOW)` on a link entry says "link", not "file"; `openat`
//! with `O_NOFOLLOW` on a link entry fails and on a pipe with no writer returns at once (`O_NONBLOCK`), as a pipe, which is then refused.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

pub use crate::folder_tree::{Identity, Kind, MAX_DEPTH, Pinned, Tree};

#[path = "folder_mac/remove.rs"]
pub mod remove;

/// An open folder of this user's. Closed on drop.
#[derive(Debug)]
pub struct Folder {
    fd: OwnedFd,
}

/// What opening a folder found. A link, a file, another user's folder, or one that can't be entered is `Other`.
#[derive(Debug)]
pub enum Opened {
    Missing,
    Other,
    Folder(Folder),
}

/// One entry of a folder, as `fstatat` without following a link says: a link, a pipe or a folder is not `is_file`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: OsString,
    pub is_file: bool,
    pub links: u64,
    /// Nanoseconds since 1970, if the system gives one.
    pub modified_ns: Option<i64>,
    /// The device and inode, and what the entry is, as `fstatat` without following a link says.
    pub dev: u64,
    pub ino: u64,
    pub kind: Kind,
}

impl Entry {
    /// What this entry was when it was looked at.
    pub fn identity(&self) -> Identity {
        Identity {
            dev: self.dev,
            ino: self.ino,
            kind: self.kind,
        }
    }
}

fn kind_of(mode: libc::mode_t) -> Kind {
    match mode & libc::S_IFMT {
        libc::S_IFREG => Kind::File,
        libc::S_IFDIR => Kind::Folder,
        libc::S_IFLNK => Kind::Link,
        _ => Kind::Other,
    }
}

fn entry_of(name: OsString, st: &libc::stat) -> Entry {
    let modified_ns = st.st_mtime.checked_mul(1_000_000_000).and_then(|s| s.checked_add(st.st_mtime_nsec));
    Entry {
        name,
        is_file: st.st_mode & libc::S_IFMT == libc::S_IFREG,
        links: u64::from(st.st_nlink),
        modified_ns,
        dev: st.st_dev as u64,
        ino: st.st_ino,
        kind: kind_of(st.st_mode),
    }
}

fn flags() -> libc::c_int {
    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

fn ours(fd: OwnedFd) -> Opened {
    let file = File::from(fd);
    match file.metadata() {
        Ok(meta) if meta.is_dir() && meta.uid() == crate::local::current_uid() => Opened::Folder(Folder { fd: file.into() }),
        _ => Opened::Other,
    }
}

fn name_of(name: &OsStr) -> io::Result<CString> {
    // One plain name of this folder: `openat` ignores the folder for an absolute name (`/dev/fd/N` reopens descriptor N) and leaves it for `..`.
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    CString::new(bytes).map_err(|_| io::ErrorKind::InvalidInput.into())
}

impl Folder {
    /// Opens `path` as a folder without following a link at its last part. Everything below it is then opened from it by descriptor.
    pub fn open(path: &Path) -> Opened {
        match OpenOptions::new().read(true).custom_flags(flags()).open(path) {
            Ok(file) => ours(file.into()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Opened::Missing,
            Err(_) => Opened::Other,
        }
    }

    /// Opens the folder `name` of this one, without following a link.
    pub fn open_in(&self, name: &OsStr) -> Opened {
        let Ok(name) = name_of(name) else { return Opened::Other };
        // SAFETY: `fd` is open for as long as `self`, and `name` is a NUL-ended string.
        let fd = unsafe { libc::openat(self.fd.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | flags()) };
        if fd >= 0 {
            // SAFETY: a new descriptor `openat` just returned, owned by nobody else.
            return ours(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        match io::Error::last_os_error().kind() {
            io::ErrorKind::NotFound => Opened::Missing,
            _ => Opened::Other,
        }
    }

    /// The entries of this folder, as it was when opened whatever its path now leads to. `.` and `..` are left out.
    pub fn entries(&self) -> io::Result<Entries<'_>> {
        // A duplicate would share the offset with every other listing, so each listing opens the folder again through the descriptor.
        // SAFETY: `fd` is open for as long as `self`, and "." is a NUL-ended string.
        let raw = unsafe { libc::openat(self.fd.as_raw_fd(), c".".as_ptr(), libc::O_RDONLY | flags()) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        let copy = raw;
        // SAFETY: `copy` is a descriptor we own; on success the stream owns it.
        let dir = unsafe { libc::fdopendir(copy) };
        if dir.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: `fdopendir` failed, so the descriptor is still ours.
            drop(unsafe { OwnedFd::from_raw_fd(copy) });
            return Err(error);
        }
        Ok(Entries {
            dir,
            folder: self.fd.as_raw_fd(),
            _folder: PhantomData,
        })
    }

    /// What this folder is, by `fstat` on its descriptor: the same thing whatever its path now leads to.
    pub fn identity(&self) -> io::Result<Identity> {
        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `fd` is open for as long as `self`, and `st` is the size `fstat` writes.
        let st = unsafe {
            if libc::fstat(self.fd.as_raw_fd(), st.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
            st.assume_init()
        };
        Ok(entry_of(OsString::new(), &st).identity())
    }

    /// What the entry `name` of this folder is now, without following a link (`fstatat` on the descriptor).
    pub fn look(&self, name: &OsStr) -> io::Result<Entry> {
        let plain = name_of(name)?;
        stat_at(self.fd.as_raw_fd(), &plain).map(|st| entry_of(name.to_owned(), &st))
    }

    /// Takes the entry `name` out of this folder: a folder with `folder` set (it must be empty), anything else, a link included, without following
    /// it (`unlinkat`). `NotFound` if it is already gone.
    pub fn unlink_in(&self, name: &OsStr, folder: bool) -> io::Result<()> {
        let plain = name_of(name)?;
        let flag = if folder { libc::AT_REMOVEDIR } else { 0 };
        // SAFETY: `fd` is open for as long as `self`, and `plain` is a NUL-ended string naming one entry of it.
        if unsafe { libc::unlinkat(self.fd.as_raw_fd(), plain.as_ptr(), flag) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Where the system says this folder is now (`F_GETPATH` on the descriptor), which is a path to look at and never one to act by.
    pub fn path(&self) -> io::Result<PathBuf> {
        let mut buffer = [0u8; libc::PATH_MAX as usize];
        // SAFETY: `fd` is open for as long as `self`, and `F_GETPATH` writes at most `MAXPATHLEN` bytes into the buffer given.
        if unsafe { libc::fcntl(self.fd.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } == -1 {
            return Err(io::Error::last_os_error());
        }
        let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
        Ok(PathBuf::from(OsStr::from_bytes(&buffer[..end])))
    }

    /// Makes what was taken out of, or put into, this folder last through a power loss (`fsync` on the descriptor).
    pub fn sync(&self) -> io::Result<()> {
        // SAFETY: `fd` is open for as long as `self`.
        if unsafe { libc::fsync(self.fd.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Opens a file of this folder for reading, only if it's a plain file of this user's that nobody else can write (the checks of
    /// `local::open_private_file`, on a descriptor). A link isn't followed and a pipe doesn't wait for a writer.
    pub fn open_private_in(&self, name: &OsStr) -> io::Result<File> {
        let name = name_of(name)?;
        // SAFETY: `fd` is open for as long as `self`, and `name` is a NUL-ended string.
        let fd = unsafe {
            libc::openat(
                self.fd.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new descriptor `openat` just returned, owned by nobody else.
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata()?;
        if !meta.file_type().is_file() || meta.uid() != crate::local::current_uid() || meta.mode() & 0o022 != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "this file isn't a private file of this user's"));
        }
        Ok(file)
    }
}

/// A folder's entries, one at a time. Holds the folder borrowed, so it can't outlive it.
pub struct Entries<'a> {
    dir: *mut libc::DIR,
    folder: libc::c_int,
    _folder: PhantomData<&'a Folder>,
}

impl Iterator for Entries<'_> {
    type Item = io::Result<Entry>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // SAFETY: `__error` is this thread's errno, and `dir` a live stream; the entry is read before the next `readdir`.
            let entry = unsafe {
                *libc::__error() = 0;
                libc::readdir(self.dir)
            };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                return (error.raw_os_error() != Some(0)).then_some(Err(error));
            }
            // SAFETY: `readdir` gave a live entry whose name is NUL-ended.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            return Some(self.look(name));
        }
    }
}

impl Entries<'_> {
    fn look(&self, name: &CStr) -> io::Result<Entry> {
        stat_at(self.folder, name).map(|st| entry_of(OsString::from_vec(name.to_bytes().to_vec()), &st))
    }
}

/// `fstatat` of `name` in the folder open as `folder`, without following a link.
fn stat_at(folder: libc::c_int, name: &CStr) -> io::Result<libc::stat> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `folder` is open for the caller's borrow, `name` is NUL-ended, and `st` is the size `fstatat` writes.
    unsafe {
        if libc::fstatat(folder, name.as_ptr(), st.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(st.assume_init())
    }
}

impl Drop for Entries<'_> {
    fn drop(&mut self) {
        // SAFETY: `dir` is a live stream, closed once; this closes the descriptor it owns (the fresh `openat(fd, ".")` of its listing).
        unsafe { libc::closedir(self.dir) };
    }
}

#[cfg(test)]
#[path = "folder_mac/tests.rs"]
mod tests;
