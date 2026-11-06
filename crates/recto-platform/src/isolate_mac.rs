//! A killable child process, the channel to it, shared memory and a loaded library. Everything the renderer's isolation needs from
//! the OS, so nothing above this file names one. A plain child process over a socket pair for now; XPC can replace this module's
//! insides because the channel already carries handles the way XPC does.

use crate::ProcessMemory;
use std::ffi::{CStr, CString, OsStr, c_int, c_void};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

// Apple's extension, not yet in the `libc` crate. Marks a descriptor to survive `POSIX_SPAWN_CLOEXEC_DEFAULT` under its own number.
unsafe extern "C" {
    fn posix_spawn_file_actions_addinherit_np(actions: *mut libc::posix_spawn_file_actions_t, fd: c_int) -> c_int;
}

/// The descriptor number a worker finds its channel on.
const CHILD_CHANNEL_FD: RawFd = 3;
/// The most handles one message can carry.
pub const MAX_HANDLES: usize = 2;

/// Reports the last OS error unless `rc` says the call worked.
fn check(rc: c_int) -> io::Result<c_int> {
    if rc == -1 { Err(io::Error::last_os_error()) } else { Ok(rc) }
}

/// Reports an error code a `posix_spawn` family call returned.
fn check_spawn(rc: c_int) -> io::Result<()> {
    if rc == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(rc)) }
}

/// An OS handle that can travel to another process: a file, or shared memory.
#[derive(Debug)]
pub struct Handle(OwnedFd);

impl Handle {
    /// A second handle on an open file. The file's own handle stays where it is.
    pub fn from_file(file: &File) -> io::Result<Self> {
        Ok(Self(OwnedFd::from(file.try_clone()?)))
    }

    pub fn into_file(self) -> File {
        File::from(self.0)
    }
}

/// Whether a wait ended because something is ready or because time ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    Ready,
    TimedOut,
}

/// One end of a two-way byte stream between two processes, able to carry handles.
#[derive(Debug)]
pub struct Channel(OwnedFd);

impl Channel {
    pub fn pair() -> io::Result<(Self, Self)> {
        let mut fds = [0 as RawFd; 2];
        // SAFETY: `fds` has room for the two descriptors the call writes.
        check(unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) })?;
        // SAFETY: the call succeeded, so both descriptors are new and owned only here.
        let (a, b) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        for fd in [&a, &b] {
            // macOS has no flag to set these at creation. A spawn on another thread in this tiny window is covered by
            // `POSIX_SPAWN_CLOEXEC_DEFAULT` for our own spawns.
            set_cloexec(fd.as_raw_fd())?;
            let on: c_int = 1;
            // SAFETY: the option value is a live `c_int` and the length says so.
            check(unsafe {
                libc::setsockopt(fd.as_raw_fd(), libc::SOL_SOCKET, libc::SO_NOSIGPIPE, (&raw const on).cast(), size_of::<c_int>() as u32)
            })?;
        }
        Ok((Self(a), Self(b)))
    }

    /// The channel a worker's parent left for it. Call once, at the start of `main`.
    pub fn inherited() -> io::Result<Self> {
        // SAFETY: `F_GETFD` reads flags and touches no memory.
        check(unsafe { libc::fcntl(CHILD_CHANNEL_FD, libc::F_GETFD) })?;
        set_cloexec(CHILD_CHANNEL_FD)?;
        // SAFETY: the parent put this descriptor here for this process alone, and this is its only owner.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(CHILD_CHANNEL_FD) }))
    }

    pub fn set_nonblocking(&self, on: bool) -> io::Result<()> {
        let fd = self.0.as_raw_fd();
        // SAFETY: `fcntl` with these commands takes an integer and touches no memory.
        let flags = check(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
        let flags = if on { flags | libc::O_NONBLOCK } else { flags & !libc::O_NONBLOCK };
        // SAFETY: as above.
        check(unsafe { libc::fcntl(fd, libc::F_SETFL, flags) })?;
        Ok(())
    }

    /// Sends some of `bytes`, with `handles` attached to the first of them. Returns how many bytes went.
    pub fn send(&self, bytes: &[u8], handles: &[&Handle]) -> io::Result<usize> {
        if handles.len() > MAX_HANDLES {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut iov = libc::iovec { iov_base: bytes.as_ptr().cast_mut().cast(), iov_len: bytes.len() };
        let mut control = ControlBuffer::new();
        // SAFETY: a zeroed `msghdr` is valid: null pointers and zero lengths.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        if !handles.is_empty() {
            let payload = (handles.len() * size_of::<RawFd>()) as u32;
            // SAFETY: `CMSG_SPACE` is arithmetic.
            msg.msg_controllen = unsafe { libc::CMSG_SPACE(payload) };
            msg.msg_control = control.0.as_mut_ptr().cast();
            // SAFETY: `msg` points at `control`, which is aligned and large enough for `MAX_HANDLES`, so the first header exists
            // and its data has room for the descriptors.
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&raw const msg);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(payload);
                let data = libc::CMSG_DATA(header).cast::<RawFd>();
                for (i, handle) in handles.iter().enumerate() {
                    data.add(i).write_unaligned(handle.0.as_raw_fd());
                }
            }
        }
        loop {
            // SAFETY: `msg` and everything it points at outlive the call. The kernel only reads them.
            let sent = unsafe { libc::sendmsg(self.0.as_raw_fd(), &raw const msg, 0) };
            if sent >= 0 {
                return Ok(sent as usize);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    /// Reads what's available into `buf`, appending any handles that arrived to `handles`. Zero bytes means the peer has gone.
    pub fn recv(&self, buf: &mut [u8], handles: &mut Vec<Handle>) -> io::Result<usize> {
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
        let mut control = ControlBuffer::new();
        // SAFETY: a zeroed `msghdr` is valid: null pointers and zero lengths.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.0.as_mut_ptr().cast();
        msg.msg_controllen = size_of::<ControlBuffer>() as u32;
        let got = loop {
            // SAFETY: `msg` and everything it points at outlive the call, and the lengths describe real buffers.
            let got = unsafe { libc::recvmsg(self.0.as_raw_fd(), &raw mut msg, 0) };
            if got >= 0 {
                break got as usize;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        };
        // SAFETY: the kernel set `msg_controllen` to what it wrote into `control`, and the macros stay inside that length.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&raw const msg);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                    let data = libc::CMSG_DATA(header);
                    let len = ((*header).cmsg_len as usize).saturating_sub(data.offset_from(header.cast()) as usize);
                    for i in 0..len / size_of::<RawFd>() {
                        let fd = data.cast::<RawFd>().add(i).read_unaligned();
                        // Owned first, so it's closed if nothing keeps it. Our own spawns inherit nothing by default, so a failure
                        // to mark it costs only the second line of defence.
                        let owned = OwnedFd::from_raw_fd(fd);
                        let _ = set_cloexec(fd);
                        handles.push(Handle(owned));
                    }
                }
                header = libc::CMSG_NXTHDR(&raw const msg, header);
            }
        }
        Ok(got)
    }

    /// Waits until a read won't block, or the peer has gone. `None` waits for ever.
    pub fn wait_readable(&self, timeout: Option<Duration>) -> io::Result<Wait> {
        self.wait(libc::POLLIN, timeout)
    }

    /// Waits until a write won't block, or the peer has gone.
    pub fn wait_writable(&self, timeout: Option<Duration>) -> io::Result<Wait> {
        self.wait(libc::POLLOUT, timeout)
    }

    fn wait(&self, events: i16, timeout: Option<Duration>) -> io::Result<Wait> {
        let ms = timeout.map_or(-1, |t| c_int::try_from(t.as_micros().div_ceil(1000)).unwrap_or(c_int::MAX));
        let mut poll = libc::pollfd { fd: self.0.as_raw_fd(), events, revents: 0 };
        // SAFETY: one live `pollfd`, and the count says one.
        match unsafe { libc::poll(&raw mut poll, 1, ms) } {
            0 => Ok(Wait::TimedOut),
            -1 => {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted { Ok(Wait::TimedOut) } else { Err(error) }
            }
            _ => Ok(Wait::Ready),
        }
    }

    /// Whether the other end has gone, with nothing left to read. Doesn't consume anything. A peek rather than `POLLHUP`, which macOS
    /// doesn't report on an idle socket pair.
    pub fn peer_gone(&self) -> bool {
        let mut byte = 0u8;
        // SAFETY: one live byte, and the length says one. The flags make this a look that can't block.
        let got = unsafe { libc::recv(self.0.as_raw_fd(), (&raw mut byte).cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT) };
        got == 0
    }
}

/// Room for one `SCM_RIGHTS` message of [`MAX_HANDLES`], aligned for its header.
#[repr(align(8))]
struct ControlBuffer([u8; 64]);

impl ControlBuffer {
    fn new() -> Self {
        Self([0; 64])
    }
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fcntl` with these commands takes an integer and touches no memory.
    check(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) })?;
    Ok(())
}

/// Memory shared with another process. Created by the host, passed as a [`Handle`], unlinked from any name at birth.
///
/// A shared-memory object can't be resized once it has a size on macOS, so a hostile worker can't shrink it to make the host fault
/// on its next read. `a_surface_cannot_be_shrunk` holds that in place.
#[derive(Debug)]
pub struct SharedMemory {
    handle: Handle,
    ptr: NonNull<u8>,
    len: usize,
}

// SAFETY: the mapping belongs to this value, and it's only reached through `copy_out`, atomics, or a `&mut self` slice.
unsafe impl Send for SharedMemory {}
// SAFETY: as above. `&self` access is copies and atomics, neither of which makes a Rust reference to the bytes.
unsafe impl Sync for SharedMemory {}

static NEXT_SHM: AtomicU32 = AtomicU32::new(0);

impl SharedMemory {
    /// New zeroed memory. Pages cost nothing until someone touches them.
    pub fn create(len: usize) -> io::Result<Self> {
        if len == 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        // The name exists only between these two calls. macOS limits it to 31 bytes.
        let name = format!("/recto.{}.{}", std::process::id(), NEXT_SHM.fetch_add(1, Ordering::Relaxed));
        let name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let mode: libc::c_uint = 0o600;
        // SAFETY: `name` is NUL-terminated and outlives the call. The variadic mode is the `c_uint` the C API promotes it to.
        let fd = check(unsafe { libc::shm_open(name.as_ptr(), libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, mode) })?;
        // SAFETY: the call succeeded, so this descriptor is new and owned only here.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: `name` is NUL-terminated and outlives the call.
        unsafe { libc::shm_unlink(name.as_ptr()) };
        set_cloexec(fd)?;
        let size = libc::off_t::try_from(len).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: the descriptor is valid and this is its first and only size.
        check(unsafe { libc::ftruncate(fd, size) })?;
        Self::map(Handle(owned), len)
    }

    /// Maps memory another process created. Refuses if it's smaller than `len`, because touching past its end is a fault.
    pub fn map(handle: Handle, len: usize) -> io::Result<Self> {
        if len == 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let fd = handle.0.as_raw_fd();
        // SAFETY: a zeroed `stat` is a valid buffer for the call to fill.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: `stat` is live and the right type.
        check(unsafe { libc::fstat(fd, &raw mut stat) })?;
        if u64::try_from(stat.st_size).unwrap_or(0) < len as u64 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        // SAFETY: a new mapping at an address the kernel picks, on a descriptor checked to be at least this long.
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0) };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let ptr = NonNull::new(ptr.cast::<u8>()).ok_or_else(|| io::Error::from(io::ErrorKind::Other))?;
        Ok(Self { handle, ptr, len })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// The bytes, for the process that's drawing into them. Don't use this on memory a hostile process also holds: the worker
    /// trusts the host, the host never trusts the worker and reads through [`Self::copy_out`].
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: the mapping is `len` bytes, valid while `self` lives, and `&mut self` makes this the only reference in this process.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    /// Copies out bytes that a hostile process may be writing at this moment. The result may be any bytes, and is never read twice.
    /// Returns false if the range isn't inside the memory.
    pub fn copy_out(&self, offset: usize, out: &mut [u8]) -> bool {
        let Some(end) = offset.checked_add(out.len()) else { return false };
        if end > self.len {
            return false;
        }
        // SAFETY: the range was just checked to be inside the mapping, and `out` is ordinary memory that can't overlap it. No reference
        // to the shared bytes is ever made, so another process writing them can only change what's copied.
        unsafe { std::ptr::copy_nonoverlapping(self.ptr.as_ptr().add(offset), out.as_mut_ptr(), out.len()) };
        true
    }

    /// An atomic in the memory, for one word both sides can see without a system call. `None` if out of range or misaligned.
    pub fn atomic_u64(&self, offset: usize) -> Option<&AtomicU64> {
        let end = offset.checked_add(size_of::<u64>())?;
        if end > self.len || !offset.is_multiple_of(align_of::<AtomicU64>()) {
            return None;
        }
        // SAFETY: in range and aligned (the mapping is page-aligned), valid while `self` lives, and only ever accessed atomically
        // at this offset by both sides.
        Some(unsafe { &*self.ptr.as_ptr().add(offset).cast::<AtomicU64>() })
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        // SAFETY: this is the mapping `map` made, and nothing refers to it once `self` is gone.
        unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}

/// How a child ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildExit {
    Code(i32),
    Signal(i32),
}

fn decode_wait_status(status: c_int) -> ChildExit {
    if libc::WIFEXITED(status) { ChildExit::Code(libc::WEXITSTATUS(status)) } else { ChildExit::Signal(libc::WTERMSIG(status)) }
}

/// A child process that inherits nothing except its channel and an empty environment plus what the caller lists.
#[derive(Debug)]
pub struct ChildProcess {
    pid: libc::pid_t,
    exit: Option<ChildExit>,
    /// Its memory at the moment it ended, read before it was collected.
    last_memory: Option<ProcessMemory>,
}

impl ChildProcess {
    /// Starts `exe` with `channel` as its inherited channel. Standard input, output and error are `/dev/null`.
    pub fn spawn(exe: &Path, args: &[&OsStr], env: &[(&str, &str)], channel: Channel) -> io::Result<Self> {
        let cstring = |bytes: &[u8]| CString::new(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput));
        let path = cstring(exe.as_os_str().as_bytes())?;
        let mut owned_args = vec![path.clone()];
        for arg in args {
            owned_args.push(cstring(arg.as_bytes())?);
        }
        let mut argv: Vec<*mut libc::c_char> = owned_args.iter().map(|a| a.as_ptr().cast_mut()).collect();
        argv.push(std::ptr::null_mut());
        let owned_env = env.iter().map(|(k, v)| cstring(format!("{k}={v}").as_bytes())).collect::<io::Result<Vec<_>>>()?;
        let mut envp: Vec<*mut libc::c_char> = owned_env.iter().map(|e| e.as_ptr().cast_mut()).collect();
        envp.push(std::ptr::null_mut());
        let dev_null = c"/dev/null";

        let child_fd = channel.0.as_raw_fd();
        let mut pid: libc::pid_t = 0;
        // SAFETY: `actions` and `attr` are initialised before use and destroyed on every path below. Every pointer passed to the spawn
        // family is NUL-terminated and alive until the call returns, and `argv` and `envp` end in null.
        unsafe {
            let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
            let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
            check_spawn(libc::posix_spawn_file_actions_init(&raw mut actions))?;
            if let Err(error) = check_spawn(libc::posix_spawnattr_init(&raw mut attr)) {
                libc::posix_spawn_file_actions_destroy(&raw mut actions);
                return Err(error);
            }
            let prepared = (|| {
                // Nothing is inherited unless named below, whatever else in this process forgot to mark close-on-exec.
                check_spawn(libc::posix_spawnattr_setflags(&raw mut attr, libc::POSIX_SPAWN_CLOEXEC_DEFAULT as libc::c_short))?;
                if child_fd == CHILD_CHANNEL_FD {
                    check_spawn(posix_spawn_file_actions_addinherit_np(&raw mut actions, child_fd))?;
                } else {
                    check_spawn(libc::posix_spawn_file_actions_adddup2(&raw mut actions, child_fd, CHILD_CHANNEL_FD))?;
                }
                for (fd, flags) in [(0, libc::O_RDONLY), (1, libc::O_WRONLY), (2, libc::O_WRONLY)] {
                    check_spawn(libc::posix_spawn_file_actions_addopen(&raw mut actions, fd, dev_null.as_ptr(), flags, 0))?;
                }
                check_spawn(libc::posix_spawn(&raw mut pid, path.as_ptr(), &raw const actions, &raw const attr, argv.as_ptr(), envp.as_ptr()))
            })();
            libc::posix_spawn_file_actions_destroy(&raw mut actions);
            libc::posix_spawnattr_destroy(&raw mut attr);
            prepared?;
        }
        Ok(Self { pid, exit: None, last_memory: None })
    }

    pub fn pid(&self) -> u32 {
        self.pid as u32
    }

    /// Its memory right now, from outside, so the child can't lie. Once it has ended, its memory at the end, peak included.
    pub fn memory(&self) -> Option<ProcessMemory> {
        if self.exit.is_some() { self.last_memory } else { process_memory(self.pid as u32) }
    }

    /// Whether it has ended, without waiting.
    pub fn try_wait(&mut self) -> Option<ChildExit> {
        if self.exit.is_none() && self.wait_without_collecting(libc::WNOHANG) {
            self.collect();
        }
        self.exit
    }

    /// Kills it at once and collects it. Safe to call more than once.
    pub fn kill(&mut self) -> Option<ChildExit> {
        if self.exit.is_none() {
            // SAFETY: the pid is this process's own uncollected child, so the number can't have been reused.
            unsafe { libc::kill(self.pid, libc::SIGKILL) };
            self.wait_without_collecting(0);
            self.collect();
        }
        self.exit
    }

    /// Waits for the end but leaves the child's record in place, so its final memory can still be read. True if it has ended.
    fn wait_without_collecting(&self, flags: c_int) -> bool {
        loop {
            // SAFETY: a zeroed `siginfo_t` is a valid buffer for the call to fill.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is live for the call. The pid is this process's own uncollected child.
            let rc = unsafe { libc::waitid(libc::P_PID, self.pid as libc::id_t, &raw mut info, libc::WEXITED | libc::WNOWAIT | flags) };
            if rc == 0 {
                return info.si_pid == self.pid;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return false;
            }
        }
    }

    fn collect(&mut self) {
        self.last_memory = process_memory(self.pid as u32);
        loop {
            let mut status: c_int = 0;
            // SAFETY: `status` is live for the call. The pid is this process's own uncollected child.
            let waited = unsafe { libc::waitpid(self.pid, &raw mut status, 0) };
            if waited == self.pid {
                self.exit = Some(decode_wait_status(status));
                return;
            }
            if waited == -1 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return;
            }
        }
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

/// A process's memory from outside. Works on any process of this user, including a killed child not yet collected.
pub fn process_memory(pid: u32) -> Option<ProcessMemory> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
    // SAFETY: the buffer matches the requested flavor and lives for the whole call; the kernel fills it on success.
    let info = unsafe {
        let rc = libc::proc_pid_rusage(pid as c_int, libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast::<libc::rusage_info_t>());
        if rc != 0 {
            return None;
        }
        info.assume_init()
    };
    Some(ProcessMemory { footprint_bytes: info.ri_phys_footprint, peak_footprint_bytes: info.ri_lifetime_max_phys_footprint })
}

/// Takes away what a renderer process never needs, before it touches a hostile file. Lowered for good: hard and soft together.
/// Memory isn't here because macOS has no working limit for it: `setrlimit` refuses `RLIMIT_AS`, `RLIMIT_DATA` and `RLIMIT_RSS` with
/// `EINVAL` (measured on macOS 27), so the host polls from outside instead.
pub fn harden_this_process() {
    for (resource, value) in [(libc::RLIMIT_CORE, 0), (libc::RLIMIT_FSIZE, 0), (libc::RLIMIT_NPROC, 0)] {
        let limit = libc::rlimit { rlim_cur: value, rlim_max: value };
        // SAFETY: `limit` is live for the call. A refusal changes nothing, and each of these is a second line only.
        unsafe { libc::setrlimit(resource, &raw const limit) };
    }
}

/// Ends this process at once with no destructors or exit handlers, which matters when another thread is inside C++ that may never return.
pub fn exit_now(code: i32) -> ! {
    // SAFETY: `_exit` has no preconditions and doesn't return.
    unsafe { libc::_exit(code) }
}

/// What a library file is called here.
pub fn library_file_name(stem: &str) -> String {
    format!("lib{stem}.dylib")
}

/// A loaded library. Never unloaded: PDFium keeps global state and code that may still be on a stack.
#[derive(Debug)]
pub struct Library(NonNull<c_void>);

// SAFETY: `dlsym` is safe from any thread, and this type offers nothing else.
unsafe impl Send for Library {}
// SAFETY: as above.
unsafe impl Sync for Library {}

impl Library {
    pub fn open(path: &Path) -> io::Result<Self> {
        let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: `path` is NUL-terminated and outlives the call. Loading runs the library's initialisers, which is why this is called
        // only on the digest-checked file the caller names.
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        NonNull::new(handle).map(Self).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the library couldn't be loaded"))
    }

    /// A symbol's address, for the one seam that casts it to its known signature.
    pub fn symbol(&self, name: &CStr) -> Option<NonNull<c_void>> {
        // SAFETY: the handle is live for ever and `name` is NUL-terminated.
        NonNull::new(unsafe { libc::dlsym(self.0.as_ptr(), name.as_ptr()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a hostile worker shrinks a surface while the host reads it, and the host dies of a bus error.
    #[test]
    fn a_surface_cannot_be_shrunk() {
        let memory = SharedMemory::create(1 << 20).unwrap();
        // SAFETY: the descriptor is valid. This is the call a hostile process would make on its copy.
        let rc = unsafe { libc::ftruncate(memory.handle().0.as_raw_fd(), 4096) };
        assert_eq!(rc, -1, "macOS let a shared-memory object be resized, so the host's reads can fault");
        let mut last = [0u8; 8];
        assert!(memory.copy_out((1 << 20) - 8, &mut last), "the end of the surface is still readable");
    }

    /// Failure: reads off the end of a surface return neighbouring memory instead of being refused.
    #[test]
    fn copy_out_refuses_ranges_outside_the_memory() {
        let memory = SharedMemory::create(4096).unwrap();
        let mut one = [0u8; 1];
        assert!(!memory.copy_out(4096, &mut one));
        assert!(!memory.copy_out(usize::MAX, &mut one));
        assert!(memory.copy_out(4095, &mut one));
        assert!(memory.atomic_u64(3).is_none(), "misaligned");
        assert!(memory.atomic_u64(4096).is_none(), "out of range");
        assert!(memory.atomic_u64(8).is_some());
    }

    /// Failure: mapping memory that's smaller than the job says lets the worker fault on the first row past its end.
    #[test]
    fn mapping_refuses_memory_smaller_than_asked() {
        let memory = SharedMemory::create(4096).unwrap();
        let handle = Handle(memory.handle().0.try_clone().unwrap());
        assert!(SharedMemory::map(handle, 1 << 20).is_err());
    }

    /// Failure: a handle doesn't arrive with its message, so a worker has no file to read or draws into nothing.
    #[test]
    fn handles_travel_with_their_bytes() {
        let (a, b) = Channel::pair().unwrap();
        let mut memory = SharedMemory::create(4096).unwrap();
        memory.as_mut_slice()[7] = 42;
        a.send(b"hello", &[memory.handle()]).unwrap();
        let mut buf = [0u8; 16];
        let mut handles = Vec::new();
        assert_eq!(b.recv(&mut buf, &mut handles).unwrap(), 5);
        assert_eq!(&buf[..5], b"hello");
        let mapped = SharedMemory::map(handles.pop().unwrap(), 4096).unwrap();
        let mut byte = [0u8; 1];
        assert!(mapped.copy_out(7, &mut byte));
        assert_eq!(byte[0], 42, "both sides see the same memory");
    }

    /// Failure: a worker never notices its host has died, and a hostile page keeps growing with nobody to kill it.
    #[test]
    fn peer_gone_notices_a_closed_channel() {
        let (a, b) = Channel::pair().unwrap();
        assert!(!a.peer_gone());
        b.send(b"x", &[]).unwrap();
        assert!(!a.peer_gone(), "unread bytes aren't a departure");
        let mut buf = [0u8; 1];
        a.recv(&mut buf, &mut Vec::new()).unwrap();
        assert!(!a.peer_gone(), "an idle channel isn't a closed one");
        drop(b);
        assert!(a.peer_gone());
        assert_eq!(a.wait_readable(Some(Duration::ZERO)).unwrap(), Wait::Ready, "a read would return at once");
    }

    /// Failure: another process's memory can't be read, so the host's limit silently does nothing.
    #[test]
    fn memory_of_this_process_is_readable_from_outside() {
        let memory = process_memory(std::process::id()).unwrap();
        assert!(memory.footprint_bytes > 0);
        assert!(memory.peak_footprint_bytes >= memory.footprint_bytes / 2);
        assert_eq!(process_memory(u32::MAX >> 1), None);
    }

    /// Failure: a killed child's numbers are lost, so the peak a bomb reached can't be reported and the limit can't be proved.
    #[test]
    fn a_killed_childs_final_memory_can_be_read() {
        let (_ours, theirs) = Channel::pair().unwrap();
        let mut child = ChildProcess::spawn(Path::new("/bin/sleep"), &[OsStr::new("30")], &[], theirs).unwrap();
        assert!(child.memory().is_some());
        assert!(child.try_wait().is_none());
        assert_eq!(child.kill(), Some(ChildExit::Signal(libc::SIGKILL)));
        let memory = child.memory().expect("the record was read before it was collected");
        assert!(memory.peak_footprint_bytes > 0);
        assert_eq!(child.kill(), Some(ChildExit::Signal(libc::SIGKILL)), "killing twice is harmless");
    }

    /// Failure: a child's exit code is lost or read as a crash, so a clean idle exit looks like a failure.
    #[test]
    fn a_childs_exit_code_is_read() {
        let (_ours, theirs) = Channel::pair().unwrap();
        let mut child = ChildProcess::spawn(Path::new("/bin/sh"), &[OsStr::new("-c"), OsStr::new("exit 7")], &[], theirs).unwrap();
        let started = std::time::Instant::now();
        while child.try_wait().is_none() {
            assert!(started.elapsed() < Duration::from_secs(10), "it never ended");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(child.try_wait(), Some(ChildExit::Code(7)));
    }

    /// Failure: a worker inherits the host's other open files, so a compromised one can read what the user has open.
    #[test]
    fn a_child_inherits_only_its_channel() {
        let held_open = File::open("/etc/hosts").unwrap();
        // Not close-on-exec, as a careless library in the host might leave one, and well above the numbers a child starts with.
        // SAFETY: `F_DUPFD` makes a new descriptor at or above 40, with the close-on-exec flag clear, and touches no memory.
        let leaky = check(unsafe { libc::fcntl(held_open.as_raw_fd(), libc::F_DUPFD, 40) }).unwrap();
        // SAFETY: the call succeeded, so this descriptor is new and owned only here.
        let _leaky = unsafe { OwnedFd::from_raw_fd(leaky) };
        let (ours, theirs) = Channel::pair().unwrap();
        let script = format!("[ -e /dev/fd/{leaky} ] && exit 9; exit 0");
        let mut child = ChildProcess::spawn(Path::new("/bin/sh"), &[OsStr::new("-c"), OsStr::new(&script)], &[], theirs).unwrap();
        let started = std::time::Instant::now();
        while child.try_wait().is_none() {
            assert!(started.elapsed() < Duration::from_secs(10), "it never ended");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(child.try_wait(), Some(ChildExit::Code(0)), "the child could see a file it wasn't given");
        drop(ours);
    }
}
