//! The control socket's OS calls on the Mac: a Unix socket, the peer's user, a blocking wait on many sockets, and files only this user can touch. Nothing here decides anything.

use std::ffi::{CStr, OsStr};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The longest socket path `bind` takes here: `sun_path` is 104 bytes, one of them the end marker.
pub const MAX_SOCKET_PATH: usize = 103;

/// The user this process runs as.
pub fn current_uid() -> u32 {
    // SAFETY: getuid has no arguments, no failure and no side effects.
    unsafe { libc::getuid() }
}

/// This user's real home folder, from the user database. Deliberately not `$HOME`, which a run can point anywhere: this is what a state folder is checked against.
pub fn real_home() -> Option<PathBuf> {
    let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut text = vec![0u8; 4096];
    let mut found = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the length given, and getpwuid_r writes only inside them. `found` is non-null only if `record` was filled.
    let rc = unsafe { libc::getpwuid_r(libc::getuid(), record.as_mut_ptr(), text.as_mut_ptr().cast(), text.len(), &raw mut found) };
    if rc != 0 || found.is_null() {
        return None;
    }
    // SAFETY: filled, as checked above.
    let record = unsafe { record.assume_init() };
    if record.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir is a NUL-terminated string inside `text`, which is still alive.
    let home = unsafe { CStr::from_ptr(record.pw_dir) };
    Some(PathBuf::from(OsStr::from_bytes(home.to_bytes())))
}

/// This user's own temp folder, from the OS rather than `$TMPDIR`, which a run can point anywhere.
pub fn system_temp_dir() -> Option<PathBuf> {
    // _CS_DARWIN_USER_TEMP_DIR, from <unistd.h>.
    const USER_TEMP_DIR: libc::c_int = 65537;
    let mut text = vec![0u8; 1024];
    // SAFETY: the pointer and length describe `text`, and confstr writes at most that many bytes including the end marker.
    let needed = unsafe { libc::confstr(USER_TEMP_DIR, text.as_mut_ptr().cast(), text.len()) };
    if needed == 0 || needed > text.len() {
        return None;
    }
    text.truncate(needed - 1);
    Some(PathBuf::from(OsStr::from_bytes(&text)))
}

/// A listening socket. Non-blocking: `accept` returns `None` when nobody is waiting, and [`poll`] is what blocks.
#[derive(Debug)]
pub struct Listener(UnixListener);

impl Listener {
    /// Binds at `path`, then narrows it to this user alone. The folder is already private, so the moment between the two exposes nothing.
    pub fn bind(path: &Path) -> io::Result<Self> {
        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        Ok(Self(listener))
    }

    pub fn accept(&self) -> io::Result<Option<Stream>> {
        match self.0.accept() {
            Ok((stream, _)) => {
                set_no_sigpipe(stream.as_raw_fd())?;
                Ok(Some(Stream(stream)))
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn token(&self) -> Token {
        Token(self.0.as_raw_fd())
    }
}

/// One end of a local connection.
#[derive(Debug)]
pub struct Stream(UnixStream);

impl Stream {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        set_no_sigpipe(stream.as_raw_fd())?;
        Ok(Self(stream))
    }

    /// A connected pair, for waking a thread that's blocked in [`poll`].
    pub fn pair() -> io::Result<(Self, Self)> {
        let (a, b) = UnixStream::pair()?;
        set_no_sigpipe(a.as_raw_fd())?;
        set_no_sigpipe(b.as_raw_fd())?;
        Ok((Self(a), Self(b)))
    }

    /// The user on the other end, as the kernel knows it.
    pub fn peer_uid(&self) -> io::Result<u32> {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: the descriptor is a live socket owned by self, and both pointers are valid for writes.
        let rc = unsafe { libc::getpeereid(self.0.as_raw_fd(), &raw mut uid, &raw mut gid) };
        if rc == 0 { Ok(uid) } else { Err(io::Error::last_os_error()) }
    }

    pub fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        self.0.set_nonblocking(on)
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_write_timeout(timeout)
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self(self.0.try_clone()?))
    }

    pub fn token(&self) -> Token {
        Token(self.0.as_raw_fd())
    }
}

impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&self.0).read(buf)
    }
}

impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&self.0).write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer that has gone away gives an error here, never a signal that ends this process. Rust programs ignore that signal, but a host app might not.
fn set_no_sigpipe(fd: RawFd) -> io::Result<()> {
    let on: libc::c_int = 1;
    // SAFETY: `fd` is a live socket, and the option's pointer and length describe `on`.
    let rc = unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, (&raw const on).cast(), size_of::<libc::c_int>() as libc::socklen_t) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Names a socket to [`poll`]. Only valid while the socket it came from is alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token(RawFd);

impl Token {
    /// A place in the list that `poll` skips, because a negative descriptor is ignored.
    pub const NONE: Self = Self(-1);
}

/// One socket to wait on, and what came back.
#[derive(Clone, Copy, Debug)]
pub struct PollEntry {
    pub token: Token,
    pub want_read: bool,
    pub want_write: bool,
    pub readable: bool,
    pub writable: bool,
    /// The other end has gone, or the socket failed.
    pub gone: bool,
}

impl PollEntry {
    pub fn new(token: Token, want_read: bool, want_write: bool) -> Self {
        Self { token, want_read, want_write, readable: false, writable: false, gone: false }
    }
}

/// Blocks until one of the sockets is ready or the timeout passes. With no timeout it uses no CPU and wakes for nothing else. An interrupted wait returns with nothing ready.
pub fn poll(entries: &mut [PollEntry], timeout: Option<Duration>) -> io::Result<()> {
    let mut fds: Vec<libc::pollfd> = entries
        .iter()
        .map(|entry| {
            let mut events = 0;
            if entry.want_read {
                events |= libc::POLLIN;
            }
            if entry.want_write {
                events |= libc::POLLOUT;
            }
            libc::pollfd { fd: entry.token.0, events, revents: 0 }
        })
        .collect();
    let millis = timeout.map_or(-1, |t| libc::c_int::try_from(t.as_millis().saturating_add(1)).unwrap_or(libc::c_int::MAX));
    // SAFETY: the pointer and count describe `fds`, which outlives the call.
    let rc = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
    if rc < 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::Interrupted { Ok(()) } else { Err(error) };
    }
    for (entry, fd) in entries.iter_mut().zip(&fds) {
        entry.readable = fd.revents & libc::POLLIN != 0;
        entry.writable = fd.revents & libc::POLLOUT != 0;
        entry.gone = fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0;
    }
    Ok(())
}

fn refused(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, what)
}

/// Makes a folder only this user can enter, or checks that one already is. A link, someone else's folder or a folder others can reach is refused, never repaired: it may be a trap.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            match std::fs::create_dir(path) {
                Ok(()) => std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
        Ok(_) => {}
    }
    check_private_dir(path)
}

/// Whether a folder is really this user's alone.
pub fn check_private_dir(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_dir() || meta.uid() != current_uid() || meta.mode() & 0o077 != 0 {
        return Err(refused("this folder isn't private to this user"));
    }
    Ok(())
}

/// The name of the kind of file system a file is on, such as `apfs`.
pub fn file_system_name(file: &File) -> io::Result<String> {
    let mut info = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: the descriptor is open for as long as `file`, and the buffer is the size `fstatfs` writes.
    let info = unsafe {
        if libc::fstatfs(std::os::fd::AsRawFd::as_raw_fd(file), info.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        info.assume_init()
    };
    let name: Vec<u8> = info.f_fstypename.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
    Ok(String::from_utf8_lossy(&name).into_owned())
}

/// Opens a file for reading, only if it's a plain file of this user's that nobody else can write. A link isn't followed, and a pipe left in its place doesn't wait for a writer.
pub fn open_private_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    let meta = file.metadata()?;
    if !meta.file_type().is_file() || meta.uid() != current_uid() || meta.mode() & 0o022 != 0 {
        return Err(refused("this file isn't a private file of this user's"));
    }
    Ok(file)
}

/// Reads a file only if it's a plain file of this user's that nobody else can write, in at most `max_bytes`. `None` if it isn't there. Anything else is an error, which callers take as "don't trust it".
pub fn read_private_file(path: &Path, max_bytes: u64) -> io::Result<Option<Vec<u8>>> {
    let file = match open_private_file(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let meta = file.metadata()?;
    if meta.len() > max_bytes {
        return Err(refused("this file is larger than it can be"));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(max_bytes).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Replaces a file in one step, mode 0600, through a new temp file beside it. A planted link at the temp name or the target is never followed.
pub fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let written = OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&temp).and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()));
    match written.and_then(|()| std::fs::rename(&temp, path)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// [`write_private_file`], and the folder synced after the rename, so that a power cut straight afterwards can't bring the old name back. For what a person's work depends on.
pub fn write_private_file_durably(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_private_file(path, bytes)?;
    sync_folder_of(path)
}

/// Opens a file of ours for writing, making it mode 0600 if it's new, with a planted link at its name not followed. Nothing's cut short: where to write is the caller's to say.
pub fn open_private_for_writing(path: &Path, append: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW);
    if append {
        options.append(true);
    } else {
        options.write(true).truncate(false);
    }
    options.open(path)
}

/// Hands a file's bytes to the drive, with a plain `fsync`, and doesn't make the drive flush its own cache, which `File::sync_all` does on this OS and which is what costs time and wear.
pub fn hand_to_drive(file: &File) -> io::Result<()> {
    // SAFETY: the descriptor is valid for as long as the file, and fsync takes nothing else.
    if unsafe { libc::fsync(file.as_raw_fd()) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Syncs the folder a file is in, so what was just renamed or removed there stays that way.
pub fn sync_folder_of(path: &Path) -> io::Result<()> {
    let Some(folder) = path.parent() else { return Ok(()) };
    File::open(folder)?.sync_all()
}

/// Where a file of this number on this volume is now. Asks the file system, so nothing is searched, and a volume that's gone or a number nothing has says `None`. The answer may be a different file that has since taken the number, so a caller compares what it finds.
pub fn path_of_file_number(device: u64, inode: u64) -> Option<PathBuf> {
    #[repr(C)]
    struct Fsid {
        val: [i32; 2],
    }
    unsafe extern "C" {
        // sys/fsgetpath.h, public since macOS 10.13. `libc` keeps `fsid_t`'s field private.
        fn fsgetpath(buf: *mut libc::c_char, buflen: libc::size_t, fsid: *mut Fsid, obj_id: u64) -> libc::ssize_t;
    }
    let mut fsid = Fsid { val: [i32::try_from(device).ok()?, 0] };
    let mut buffer = vec![0u8; 4096];
    // SAFETY: the buffer is its stated length and lives for the call, and `fsid` is a valid struct for it. The kernel writes at most that length.
    let written = unsafe { fsgetpath(buffer.as_mut_ptr().cast(), buffer.len(), &raw mut fsid, inode) };
    let written = usize::try_from(written).ok().filter(|written| *written > 0)?;
    buffer.truncate(written);
    if buffer.last() == Some(&0) {
        buffer.pop();
    }
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buffer)))
}

/// The same with nothing synced, for what a kill or a power cut may lose: a cache that can be made again. A sync per small file costs milliseconds on a real disk.
pub fn write_private_file_unsynced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let written = create_private_file(&temp).and_then(|mut file| file.write_all(bytes));
    match written.and_then(|()| std::fs::rename(&temp, path)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Makes a new file, mode 0600, for writing. Refuses if anything's already at that name, and a planted link isn't followed.
pub fn create_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(path)
}

/// The attribute `tmutil addexclusion` sets, which goes with the item wherever it's moved. Its value is the binary property list for the string `com.apple.backupd`.
const BACKUP_EXCLUDE_NAME: &std::ffi::CStr = c"com.apple.metadata:com_apple_backup_excludeItem";
const BACKUP_EXCLUDE_VALUE: &[u8] = b"bplist00_\x10\x11com.apple.backupd\x08\0\0\0\0\0\0\x01\x01\0\0\0\0\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\x1c";

fn c_path(path: &Path) -> io::Result<std::ffi::CString> {
    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| io::ErrorKind::InvalidInput.into())
}

/// Keeps a folder out of Time Machine.
pub fn exclude_from_backups(path: &Path) -> io::Result<()> {
    let path = c_path(path)?;
    // SAFETY: both strings are NUL-terminated and live for the call, and the value is its given length. XATTR_NOFOLLOW so a link isn't what gets marked.
    let rc = unsafe { libc::setxattr(path.as_ptr(), BACKUP_EXCLUDE_NAME.as_ptr(), BACKUP_EXCLUDE_VALUE.as_ptr().cast(), BACKUP_EXCLUDE_VALUE.len(), 0, libc::XATTR_NOFOLLOW) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Whether [`exclude_from_backups`] has marked this.
pub fn is_excluded_from_backups(path: &Path) -> bool {
    let Ok(path) = c_path(path) else { return false };
    // SAFETY: a null buffer asks only for the value's length, and the strings live for the call.
    let rc = unsafe { libc::getxattr(path.as_ptr(), BACKUP_EXCLUDE_NAME.as_ptr(), std::ptr::null_mut(), 0, 0, libc::XATTR_NOFOLLOW) };
    rc > 0
}

/// Opens a file for appending, made with mode 0600 if new. A link isn't followed.
pub fn append_private_file(path: &Path) -> io::Result<File> {
    // Not blocking: opening a pipe to write would otherwise wait for a reader for ever.
    OpenOptions::new().append(true).create(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)
}

/// What's at a socket's path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketFile {
    Nothing,
    /// A socket of this user's.
    OwnSocket,
    /// Anything else: a file, a link, or someone else's socket.
    Other,
}

pub fn socket_file(path: &Path) -> io::Result<SocketFile> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(SocketFile::Nothing),
        Err(error) => Err(error),
        Ok(meta) if meta.file_type().is_socket() && meta.uid() == current_uid() => Ok(SocketFile::OwnSocket),
        Ok(_) => Ok(SocketFile::Other),
    }
}

/// `mach_timebase_info_data_t`: what turns the kernel's time units into nanoseconds. On Apple silicon they aren't the same.
#[repr(C)]
struct Timebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_timebase_info(info: *mut Timebase) -> libc::c_int;
}

/// Turns the kernel's time units, as `proc_pid_rusage` gives them, into nanoseconds.
pub(crate) fn ticks_to_ns(ticks: u64) -> u64 {
    let mut timebase = Timebase { numer: 0, denom: 0 };
    // SAFETY: the pointer is valid for writes, and the struct matches the C one.
    let rc = unsafe { mach_timebase_info(&raw mut timebase) };
    let ns = if rc == 0 && timebase.denom != 0 { u128::from(ticks) * u128::from(timebase.numer) / u128::from(timebase.denom) } else { u128::from(ticks) };
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// What a process has used so far, for a test or a budget to compare at two moments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessCounters {
    pub cpu_ns: u64,
    /// Package and interrupt wake-ups, the numbers Activity Monitor's Energy tab is built from.
    pub wakeups: u64,
    pub context_switches: u64,
    pub threads: u32,
    /// Bytes it's had written to disk in its life, the number SSD wear goes by. Counted when the write reaches the file system, whatever happens to the file after.
    pub disk_bytes_written: u64,
}

/// A process's counters. `None` if it's gone or belongs to someone else.
pub fn process_counters(pid: u32) -> Option<ProcessCounters> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::uninit();
    // SAFETY: the buffer is the size the V4 flavour writes.
    let rc = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast::<libc::rusage_info_t>()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: filled, as rc says.
    let info = unsafe { info.assume_init() };
    let mut task = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = libc::c_int::try_from(size_of::<libc::proc_taskinfo>()).ok()?;
    // SAFETY: the buffer and its size describe `task`, and a return of `size` means it was filled.
    let written = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTASKINFO, 0, task.as_mut_ptr().cast(), size) };
    if written != size {
        return None;
    }
    // SAFETY: filled, as checked.
    let task = unsafe { task.assume_init() };
    Some(ProcessCounters {
        cpu_ns: ticks_to_ns(info.ri_user_time.saturating_add(info.ri_system_time)),
        wakeups: info.ri_pkg_idle_wkups.saturating_add(info.ri_interrupt_wkups),
        context_switches: u64::try_from(task.pti_csw).unwrap_or(0),
        threads: u32::try_from(task.pti_threadnum).unwrap_or(0),
        disk_bytes_written: info.ri_diskio_byteswritten,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("recto-local-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Failure: the wait spins or misses a closed peer, so a dead client holds its slot until a deadline, or an idle server burns CPU.
    #[test]
    fn poll_blocks_until_ready_and_sees_a_closed_peer() {
        let (a, b) = Stream::pair().unwrap();
        let mut entries = [PollEntry::new(a.token(), true, false)];
        poll(&mut entries, Some(Duration::from_millis(10))).unwrap();
        assert!(!entries[0].readable && !entries[0].gone);
        (&b).write_all(b"x").unwrap();
        poll(&mut entries, None).unwrap();
        assert!(entries[0].readable);
        let mut byte = [0];
        (&a).read_exact(&mut byte).unwrap();
        drop(b);
        // A closed peer shows as readable, with nothing to read. It needs read interest here: macOS reports no hang-up for a socket with no interest at all.
        poll(&mut entries, None).unwrap();
        assert!(entries[0].readable || entries[0].gone);
        assert_eq!((&a).read(&mut byte).unwrap(), 0);
        // A place in the list that's skipped doesn't wake anything and doesn't fail.
        let mut skipped = [PollEntry::new(Token::NONE, true, true)];
        poll(&mut skipped, Some(Duration::from_millis(1))).unwrap();
        assert!(!skipped[0].readable && !skipped[0].writable && !skipped[0].gone);
    }

    /// Failure: the socket is reachable by other users, or the peer's user can't be read, so the check it rests on means nothing.
    #[test]
    fn a_bound_socket_is_private_and_knows_its_peer() {
        let dir = temp("bind");
        ensure_private_dir(&dir).unwrap();
        let path = dir.join("s");
        let listener = Listener::bind(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        assert_eq!(socket_file(&path).unwrap(), SocketFile::OwnSocket);
        let client = Stream::connect(&path).unwrap();
        let mut entries = [PollEntry::new(listener.token(), true, false)];
        poll(&mut entries, None).unwrap();
        let server = listener.accept().unwrap().unwrap();
        assert_eq!(server.peer_uid().unwrap(), current_uid());
        assert_eq!(client.peer_uid().unwrap(), current_uid());
        assert!(listener.accept().unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a folder others can reach, or a link planted where the private folder should be, is used anyway.
    #[test]
    fn folders_that_arent_private_are_refused() {
        let dir = temp("private");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ensure_private_dir(&dir).is_err(), "a group-readable folder was accepted");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = dir.with_extension("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(ensure_private_dir(&link).is_err(), "a link was accepted");
        std::fs::remove_file(&link).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a file moved to another folder can't be found, so a tab that could have followed it says it's gone. Or a number nothing has, or a volume that isn't there, answers with something other than nothing.
    #[test]
    fn a_moved_file_is_found_by_its_number() {
        let dir = temp("moved");
        ensure_private_dir(&dir).unwrap();
        ensure_private_dir(&dir.join("elsewhere")).unwrap();
        let (from, to) = (dir.join("a.pdf"), dir.join("elsewhere/b.pdf"));
        std::fs::write(&from, b"x").unwrap();
        let meta = std::fs::metadata(&from).unwrap();
        let (device, inode) = (meta.dev(), meta.ino());
        std::fs::rename(&from, &to).unwrap();
        let found = path_of_file_number(device, inode).expect("found").canonicalize().unwrap();
        assert_eq!(found, to.canonicalize().unwrap());
        std::fs::remove_file(&to).unwrap();
        assert!(path_of_file_number(device, inode).is_none_or(|path| path != found), "it's gone");
        assert_eq!(path_of_file_number(0x7fff_fff0, 2), None, "no such volume");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a file others can write, a planted link or a huge file is read as if it were ours.
    #[test]
    fn only_private_files_are_read() {
        let dir = temp("files");
        ensure_private_dir(&dir).unwrap();
        let file = dir.join("f");
        assert_eq!(read_private_file(&file, 16).unwrap(), None);
        write_private_file(&file, b"hello").unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
        assert_eq!(read_private_file(&file, 16).unwrap().unwrap(), b"hello");
        assert!(read_private_file(&file, 2).is_err(), "too long");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(read_private_file(&file, 16).is_err(), "writable by others");
        let link = dir.join("l");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(read_private_file(&link, 16).is_err(), "through a link");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: the numbers budgets depend on read as zero, so an "idle" test passes whatever the server does.
    #[test]
    fn counters_report_this_process() {
        let before = process_counters(std::process::id()).unwrap();
        let mut spin = 0u64;
        for i in 0..5_000_000u64 {
            spin = spin.wrapping_add(i).rotate_left(3);
        }
        std::hint::black_box(spin);
        let after = process_counters(std::process::id()).unwrap();
        assert!(after.cpu_ns > before.cpu_ns);
        assert!(after.threads >= 1);
        assert!(real_home().is_some());
        assert!(system_temp_dir().is_some_and(|temp| temp.is_absolute()), "{:?}", system_temp_dir());
    }
}
