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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
        // Room for `MAX_HANDLES` and a header, aligned for it.
        let mut control = [0u64; 8];
        // SAFETY: a zeroed `msghdr` is valid: null pointers and zero lengths.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        if !handles.is_empty() {
            let payload = (handles.len() * size_of::<RawFd>()) as u32;
            // SAFETY: `CMSG_SPACE` is arithmetic.
            msg.msg_controllen = unsafe { libc::CMSG_SPACE(payload) };
            msg.msg_control = control.as_mut_ptr().cast();
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

    /// Reads what's available into `buf`, appending any handles that arrived to `handles`. Zero bytes means the peer has gone. A message
    /// whose handles didn't fit is `InvalidData`: nothing we send does that, so whoever sent it is misbehaving.
    pub fn recv(&self, buf: &mut [u8], handles: &mut Vec<Handle>) -> io::Result<usize> {
        let mut control = ControlBuffer::new();
        self.recv_into(buf, handles, &mut control.0)
    }

    fn recv_into(&self, buf: &mut [u8], handles: &mut Vec<Handle>, control: &mut [u8]) -> io::Result<usize> {
        let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
        // SAFETY: a zeroed `msghdr` is valid: null pointers and zero lengths.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len() as u32;
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
        // The kernel set `msg_controllen` to what it wrote. Only those bytes are looked at, whatever a header inside them claims.
        let written = (msg.msg_controllen as usize).min(control.len());
        descriptors_in(&control[..written], |fd| {
            // SAFETY: the kernel just installed this descriptor for this process, from the bytes it wrote, and nothing else owns it.
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            // Our own spawns inherit nothing by default, so a failure here costs only a second line of defence.
            let _ = set_cloexec(fd);
            handles.push(Handle(owned));
        });
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::ErrorKind::InvalidData.into());
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

/// Calls `each` with every descriptor in control bytes the kernel wrote. Bounded by the slice, whatever a header claims: a header that runs past
/// the end, as after truncation, yields only what's really there and ends the walk. Pure, so malformed input can be tested directly.
fn descriptors_in(control: &[u8], mut each: impl FnMut(RawFd)) {
    let header = size_of::<libc::cmsghdr>();
    let mut at = 0;
    while let Some(rest) = control.get(at..).filter(|rest| rest.len() >= header) {
        let field = |offset: usize| i32::from_ne_bytes([rest[offset], rest[offset + 1], rest[offset + 2], rest[offset + 3]]);
        let (claimed, level, kind) = (field(0) as u32 as usize, field(4), field(8));
        if claimed < header {
            return;
        }
        let end = claimed.min(rest.len());
        if level == libc::SOL_SOCKET && kind == libc::SCM_RIGHTS {
            for fd in rest[header..end].as_chunks::<4>().0 {
                let fd = i32::from_ne_bytes(*fd);
                if fd >= 0 {
                    each(fd);
                }
            }
        }
        if claimed > rest.len() {
            return;
        }
        // Headers sit on 4-byte boundaries on this OS.
        at += claimed.next_multiple_of(4);
    }
}

/// Room for any message a sender can make. `sendmsg` refuses control data over `MCLBYTES` (2,048), so this can't be truncated, which matters:
/// when a message is truncated the kernel has already installed the descriptors it dropped, and they can't be closed. `the_kernel_refuses_more_handles_than_fit`
/// holds that in place.
#[repr(align(8))]
struct ControlBuffer([u8; 4096]);

impl ControlBuffer {
    fn new() -> Self {
        Self([0; 4096])
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

/// What a watched child may do between jobs, with nobody else looking. Checked once, when `after` has passed since it was last active, so a
/// child that's gone costs no wake-ups and a live one costs one per `after`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdleRule {
    /// Killed if it has done nothing for this long. It should have left by itself before then.
    pub after: Duration,
    /// Killed if its footprint is past this when checked.
    pub memory_limit_bytes: u64,
}

#[derive(Debug, Default)]
struct ChildEnd {
    exit: Option<ChildExit>,
    last_memory: Option<ProcessMemory>,
}

#[derive(Debug)]
struct ChildShared {
    pid: libc::pid_t,
    /// Held across any signal or collect. A pid stays this child's until collected, and collecting happens under this lock, so a signal can't reach
    /// a stranger who was given the number later.
    end: Mutex<ChildEnd>,
    epoch: Instant,
    last_active_ms: AtomicU64,
    busy: AtomicBool,
    /// Whether the reaper is watching this child, so it will be collected whoever else lets go of it.
    watched: AtomicBool,
    /// What to run once it has ended and been collected.
    on_end: Mutex<EndAction>,
}

/// Something to run once, when a child has gone.
#[derive(Default)]
struct EndAction(Option<Box<dyn FnOnce() + Send>>);

impl std::fmt::Debug for EndAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "EndAction(pending)" } else { "EndAction(none)" })
    }
}

impl ChildShared {
    fn end(&self) -> MutexGuard<'_, ChildEnd> {
        self.end.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Waits for the end but leaves the child's record in place, so its final memory can still be read. True if it has ended.
    fn wait_without_collecting(&self, flags: c_int) -> bool {
        loop {
            // SAFETY: a zeroed `siginfo_t` is a valid buffer for the call to fill.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is live for the call, and this only reads the state of a child of this process.
            let rc = unsafe { libc::waitid(libc::P_PID, self.pid as libc::id_t, &raw mut info, libc::WEXITED | libc::WNOWAIT | flags) };
            if rc == 0 {
                return info.si_pid == self.pid;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return false;
            }
        }
    }

    /// Collects it if it has ended, then runs its end action. Never blocks, and is safe to race with itself because of the lock.
    fn try_collect(&self) -> Option<ChildExit> {
        let exit = {
            let mut end = self.end();
            if end.exit.is_none() && self.wait_without_collecting(libc::WNOHANG) {
                end.last_memory = process_memory(self.pid as u32);
                let mut status: c_int = 0;
                // SAFETY: `status` is live for the call. The child has ended, so this doesn't block.
                let collected = loop {
                    let waited = unsafe { libc::waitpid(self.pid, &raw mut status, libc::WNOHANG) };
                    if waited != -1 {
                        break waited == self.pid;
                    }
                    if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                        break false;
                    }
                };
                if collected {
                    end.exit = Some(decode_wait_status(status));
                }
            }
            end.exit
        };
        if exit.is_some() {
            self.run_end_action();
        }
        exit
    }

    fn run_end_action(&self) {
        let action = self.on_end.lock().unwrap_or_else(std::sync::PoisonError::into_inner).0.take();
        if let Some(action) = action {
            action();
        }
    }

    /// Sends the kill unless it's already been collected. Never blocks: a process stuck in a read from a dead network volume doesn't die until that
    /// read returns, so nothing here waits to see it go.
    fn kill_now(&self) -> bool {
        let end = self.end();
        if end.exit.is_some() {
            return false;
        }
        // SAFETY: not yet collected, under the lock that collecting takes, so the pid is still this child.
        unsafe { libc::kill(self.pid, libc::SIGKILL) };
        true
    }

    fn millis(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }
}

/// A child process that inherits nothing except its channel and an empty environment plus what the caller lists. Collected as soon as it ends,
/// by the one [`reaper`] thread, so a child that left leaves no zombie behind. Dropping this kills the child without waiting.
#[derive(Debug)]
pub struct ChildProcess(Arc<ChildShared>);

impl ChildProcess {
    /// Starts `exe` with `channel` as its inherited channel. Standard input, output and error are `/dev/null`.
    pub fn spawn(exe: &Path, args: &[&OsStr], env: &[(&str, &str)], channel: Channel, rule: Option<IdleRule>) -> io::Result<Self> {
        Self::spawn_linked(exe, args, env, channel, rule, None)
    }

    /// As [`Self::spawn`], with an invitation for the child to make a surface link, as `surface` describes.
    pub fn spawn_linked(exe: &Path, args: &[&OsStr], env: &[(&str, &str)], channel: Channel, rule: Option<IdleRule>, invite: Option<&crate::surface::LinkInvite>) -> io::Result<Self> {
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
                if let Some(invite) = invite {
                    check_spawn(invite.apply(&raw mut attr))?;
                }
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
        let shared = Arc::new(ChildShared {
            pid,
            end: Mutex::new(ChildEnd::default()),
            epoch: Instant::now(),
            last_active_ms: AtomicU64::new(0),
            busy: AtomicBool::new(false),
            watched: AtomicBool::new(false),
            on_end: Mutex::new(EndAction::default()),
        });
        reaper::watch(shared.clone(), rule);
        Ok(Self(shared))
    }

    pub fn pid(&self) -> u32 {
        self.0.pid as u32
    }

    /// Its memory right now, from outside, so the child can't lie. Once it has ended, its memory at the end, peak included.
    pub fn memory(&self) -> Option<ProcessMemory> {
        let end = self.0.end();
        if end.exit.is_some() { end.last_memory } else { process_memory(self.pid()) }
    }

    /// Whether it has ended, without waiting.
    pub fn try_wait(&self) -> Option<ChildExit> {
        self.0.try_collect()
    }

    /// Sends the kill and returns at once. True if the signal was sent, false if the child had already been collected, so a number given to someone
    /// else is never signalled. Safe from any thread, including one that mustn't block.
    pub fn kill_now(&self) -> bool {
        self.0.kill_now()
    }

    /// Waits for it to end, up to `limit`. For a background thread that wants its exit status. A child stuck in the kernel may outlast any limit, and
    /// the reaper collects it whenever it does end.
    pub fn wait_bounded(&self, limit: Duration) -> Option<ChildExit> {
        let started = Instant::now();
        let mut pause = Duration::from_micros(50);
        loop {
            if let Some(exit) = self.try_wait() {
                return Some(exit);
            }
            if started.elapsed() >= limit {
                return None;
            }
            std::thread::sleep(pause.min(limit.saturating_sub(started.elapsed())));
            pause = (pause * 2).min(Duration::from_millis(5));
        }
    }

    /// Runs `action` once, as soon as the child has ended and been collected, on whichever thread collects it. At once if that has already happened. This is how something can
    /// be tied to the process really being gone, however it went, with nobody needing to ask.
    pub fn on_end(&self, action: Box<dyn FnOnce() + Send>) {
        self.0.on_end.lock().unwrap_or_else(std::sync::PoisonError::into_inner).0 = Some(action);
        // It may have ended before the action was in place.
        self.0.try_collect();
    }

    /// Says a job started or ended, so the idle rule doesn't count it. One atomic store each.
    pub fn note_activity(&self, busy: bool) {
        self.0.last_active_ms.store(self.0.millis(), Ordering::Relaxed);
        self.0.busy.store(busy, Ordering::Relaxed);
    }
}

impl Drop for ChildProcess {
    fn drop(&mut self) {
        self.kill_now();
        // With no reaper thread, which is rare, nobody else would run the end action. Then it's better to wait a moment and let go than to leave it pending for ever.
        if !self.0.watched.load(Ordering::Relaxed) {
            self.wait_bounded(Duration::from_secs(1));
            self.0.run_end_action();
        }
    }
}

/// Collects children as they end and enforces their idle rules, on one thread that exists only while a child does. It sleeps in `kevent` with no
/// timeout, so it wakes only for a child ending or an idle rule coming due. When the last child has gone, the thread and its queue go too: no workers,
/// no thread, no handle.
mod reaper {
    use super::{Arc, ChildShared, IdleRule, Mutex, MutexGuard, Ordering, RawFd, check, set_cloexec};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    struct Watched {
        child: Arc<ChildShared>,
        rule: Option<IdleRule>,
    }

    struct State {
        queue: Arc<OwnedFd>,
        children: Vec<Watched>,
    }

    static STATE: Mutex<Option<State>> = Mutex::new(None);

    fn state() -> MutexGuard<'static, Option<State>> {
        STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn change(queue: RawFd, ident: libc::pid_t, filter: i16, flags: u16, fflags: u32, data: isize) -> std::io::Result<()> {
        let event = libc::kevent { ident: ident as usize, filter, flags, fflags, data, udata: std::ptr::null_mut() };
        // SAFETY: one live change, no events asked for, and a zero timeout pointer isn't read because there's nothing to wait for.
        check(unsafe { libc::kevent(queue, &raw const event, 1, std::ptr::null_mut(), 0, std::ptr::null()) })?;
        Ok(())
    }

    fn arm_timer(queue: RawFd, pid: libc::pid_t, after_ms: u64) {
        let data = isize::try_from(after_ms.max(1)).unwrap_or(isize::MAX);
        let _ = change(queue, pid, libc::EVFILT_TIMER, libc::EV_ADD | libc::EV_ONESHOT, 0, data);
    }

    /// Starts watching a new child. If the thread or queue can't be made, the child is still collected by whoever next asks after it.
    pub(super) fn watch(child: Arc<ChildShared>, rule: Option<IdleRule>) {
        let mut guard = state();
        if guard.is_none() {
            let Some(queue) = new_queue() else { return };
            let for_thread = queue.clone();
            let thread = std::thread::Builder::new().name("recto-reaper".into()).stack_size(64 * 1024).spawn(move || run(&for_thread));
            if thread.is_err() {
                return;
            }
            *guard = Some(State { queue, children: Vec::new() });
        }
        let Some(state) = guard.as_mut() else { return };
        let queue = state.queue.as_raw_fd();
        let pid = child.pid;
        child.watched.store(true, Ordering::Relaxed);
        state.children.push(Watched { child: child.clone(), rule });
        if change(queue, pid, libc::EVFILT_PROC, libc::EV_ADD | libc::EV_ONESHOT, libc::NOTE_EXIT, 0).is_err() {
            // Already gone, so no event will ever come. A timer of 1 ms brings the thread round to clean up.
            arm_timer(queue, pid, 1);
        } else if let Some(rule) = rule {
            arm_timer(queue, pid, rule.after.as_millis() as u64);
        }
    }

    fn new_queue() -> Option<Arc<OwnedFd>> {
        // SAFETY: no arguments, and the result is checked.
        let fd = check(unsafe { libc::kqueue() }).ok()?;
        // SAFETY: the call succeeded, so this descriptor is new and owned only here.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        set_cloexec(fd).ok()?;
        Some(Arc::new(owned))
    }

    fn run(queue: &OwnedFd) {
        loop {
            // SAFETY: a zeroed `kevent` is a valid buffer for the call to fill.
            let mut events: [libc::kevent; 8] = unsafe { std::mem::zeroed() };
            // SAFETY: the buffer and its length match, no changes are passed, and a null timeout waits until something happens.
            let count = unsafe { libc::kevent(queue.as_raw_fd(), std::ptr::null(), 0, events.as_mut_ptr(), events.len() as i32, std::ptr::null()) };
            if count < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                // Nothing here can recover a broken queue. Leaving means children are collected when next asked after.
                *state() = None;
                return;
            }
            for event in &events[..count as usize] {
                handle(queue.as_raw_fd(), event.ident as libc::pid_t, event.filter);
            }
            let mut guard = state();
            if guard.as_ref().is_some_and(|state| state.children.is_empty()) {
                *guard = None;
                return;
            }
        }
    }

    /// One child's turn: collected if it has ended, otherwise its idle rule is checked and its timer set again.
    fn handle(queue: RawFd, pid: libc::pid_t, filter: i16) {
        let found = {
            let guard = state();
            guard.as_ref().and_then(|state| state.children.iter().find(|w| w.child.pid == pid).map(|w| (w.child.clone(), w.rule)))
        };
        let Some((child, rule)) = found else { return };
        let exited = filter == libc::EVFILT_PROC;
        // An exit event can arrive a moment before the child can be collected. Someone else may have collected it already, which counts.
        let mut collected = child.try_collect().is_some();
        for _ in 0..50 {
            if collected || !exited {
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
            collected = child.try_collect().is_some();
        }
        if collected {
            let _ = change(queue, pid, libc::EVFILT_TIMER, libc::EV_DELETE, 0, 0);
            if let Some(state) = state().as_mut() {
                state.children.retain(|w| w.child.pid != pid);
            }
            return;
        }
        if exited {
            // Stuck on its way out. Look again later, without waiting here.
            arm_timer(queue, pid, STUCK_CHECK_MS);
            return;
        }
        let Some(rule) = rule else {
            // No rule, so this timer only came from a child that had already gone when it was added, or is stuck on its way out.
            arm_timer(queue, pid, STUCK_CHECK_MS);
            return;
        };
        let after_ms = rule.after.as_millis() as u64;
        let idle_ms = child.millis().saturating_sub(child.last_active_ms.load(Ordering::Relaxed));
        let busy = child.busy.load(Ordering::Relaxed);
        let mut killed = false;
        if !busy {
            let over = super::process_memory(pid as u32).is_some_and(|memory| memory.footprint_bytes > rule.memory_limit_bytes);
            if idle_ms >= after_ms || over {
                child.kill_now();
                killed = true;
            }
        }
        arm_timer(queue, pid, next_check_ms(after_ms, idle_ms, busy, killed));
    }

    /// A child already signalled that hasn't gone is stuck in the kernel, perhaps on a dead volume, and looking sooner won't help.
    const STUCK_CHECK_MS: u64 = 1000;

    /// When to look at a child next. Never sooner than a second for one that's already been killed, because the budget for an idle app is one wake-up a second, and a worker stuck on a dead
    /// volume can stay for as long as the volume does.
    pub(super) fn next_check_ms(after_ms: u64, idle_ms: u64, busy: bool, killed: bool) -> u64 {
        if killed {
            STUCK_CHECK_MS
        } else if busy {
            after_ms
        } else {
            after_ms.saturating_sub(idle_ms).max(1)
        }
    }
}

/// A process's memory from outside. Works on any process of this user, including a killed child not yet collected.
/// `PRIO_DARWIN_BG` on the whole process, as `taskpolicy -b` does. Children inherit it across `posix_spawn`.
pub(crate) fn run_in_background() {
    // SAFETY: plain integers, and it changes nothing but this process's scheduling.
    let _ = unsafe { libc::setpriority(libc::PRIO_DARWIN_PROCESS, 0, libc::PRIO_DARWIN_BG) };
}

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
    Some(ProcessMemory { footprint_bytes: info.ri_phys_footprint, peak_footprint_bytes: info.ri_lifetime_max_phys_footprint, cpu_ns: crate::local::ticks_to_ns(info.ri_user_time.saturating_add(info.ri_system_time)) })
}

/// The start of the kernel's `task_vm_info`, as far as `phys_footprint`: its layout's been fixed since it was extended with that, and the kernel fills as much as it's asked for.
#[repr(C)]
#[derive(Default)]
struct TaskVmInfo {
    virtual_size: u64,
    region_count: i32,
    page_size: i32,
    resident_size: u64,
    resident_size_peak: u64,
    device: u64,
    device_peak: u64,
    internal: u64,
    internal_peak: u64,
    external: u64,
    external_peak: u64,
    reusable: u64,
    reusable_peak: u64,
    purgeable_volatile_pmap: u64,
    purgeable_volatile_resident: u64,
    purgeable_volatile_virtual: u64,
    compressed: u64,
    compressed_peak: u64,
    compressed_lifetime: u64,
    phys_footprint: u64,
}

const TASK_VM_INFO: libc::task_flavor_t = 22;

/// See [`crate::OwnMemory`].
pub(crate) fn own_memory() -> Option<crate::OwnMemory> {
    let mut info = TaskVmInfo::default();
    let mut count = libc::mach_msg_type_number_t::try_from(size_of::<TaskVmInfo>() / size_of::<libc::natural_t>()).ok()?;
    // SAFETY: the buffer is `count` naturals long and lives for the call, and the kernel writes no more than `count` of them and says how many.
    #[allow(deprecated)]
    let rc = unsafe { libc::task_info(libc::mach_task_self(), TASK_VM_INFO, (&raw mut info).cast(), &raw mut count) };
    if rc != 0 || usize::try_from(count).ok()? * size_of::<libc::natural_t>() < size_of::<TaskVmInfo>() {
        return None;
    }
    let mut heap = std::mem::MaybeUninit::<libc::malloc_statistics_t>::zeroed();
    // SAFETY: a null zone asks for every zone together, and the call fills the struct it's given.
    let heap = unsafe {
        libc::malloc_zone_statistics(std::ptr::null_mut(), heap.as_mut_ptr());
        heap.assume_init()
    };
    Some(crate::OwnMemory { footprint: info.phys_footprint, compressed: info.compressed, heap: heap.size_in_use as u64 })
}

/// See [`crate::memory_free_percent`].
pub(crate) fn memory_free_percent() -> Option<u32> {
    let mut level: c_int = 0;
    let mut len = size_of::<c_int>();
    // SAFETY: the name is NUL-terminated, the buffer is a live `c_int`, and `len` says so. The call writes back no more than `len`.
    let rc = unsafe { libc::sysctlbyname(c"kern.memorystatus_level".as_ptr(), (&raw mut level).cast(), &raw mut len, std::ptr::null_mut(), 0) };
    if rc == 0 && len == size_of::<c_int>() { u32::try_from(level).ok() } else { None }
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

    /// Failure: the numbers a memory test leans on are read from the wrong place, so a test that's meant not to be fooled by the compressor is fooled by its own reading. The footprint has to agree with the one `proc_pid_rusage` gives, which is what layout the kernel's struct has, and what's allocated has to be seen.
    #[test]
    fn own_memory_reads_true() {
        let memory = own_memory().unwrap();
        let rusage = process_memory(std::process::id()).unwrap().footprint_bytes;
        assert!(memory.footprint.abs_diff(rusage) < 4 * 1024 * 1024, "{memory:?} against {rusage}");
        assert!(memory.compressed <= memory.footprint.max(memory.compressed));
        let before = own_memory().unwrap().heap;
        let held = std::hint::black_box(vec![7u8; 8 * 1024 * 1024]);
        let after = own_memory().unwrap().heap;
        assert!(after >= before + 8 * 1024 * 1024, "{before} then {after}");
        drop(held);
    }

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
        let child = ChildProcess::spawn(Path::new("/bin/sleep"), &[OsStr::new("30")], &[], theirs, None).unwrap();
        assert!(child.memory().is_some());
        assert!(child.try_wait().is_none());
        assert!(child.kill_now());
        assert_eq!(child.wait_bounded(Duration::from_secs(30)), Some(ChildExit::Signal(libc::SIGKILL)));
        let memory = child.memory().expect("the record was read before it was collected");
        assert!(memory.peak_footprint_bytes > 0);
        assert!(!child.kill_now(), "a collected child's number may be someone else's by now, so it's never signalled");
    }

    /// Failure: a child's exit code is lost or read as a crash, so a clean idle exit looks like a failure.
    #[test]
    fn a_childs_exit_code_is_read() {
        let (_ours, theirs) = Channel::pair().unwrap();
        let child = ChildProcess::spawn(Path::new("/bin/sh"), &[OsStr::new("-c"), OsStr::new("exit 7")], &[], theirs, None).unwrap();
        assert_eq!(child.wait_bounded(Duration::from_secs(30)), Some(ChildExit::Code(7)));
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
        let child = ChildProcess::spawn(Path::new("/bin/sh"), &[OsStr::new("-c"), OsStr::new(&script)], &[], theirs, None).unwrap();
        assert_eq!(child.wait_bounded(Duration::from_secs(30)), Some(ChildExit::Code(0)), "the child could see a file it wasn't given");
        drop(ours);
    }

    /// Sends handles straight through `sendmsg`, past `Channel::send`'s own cap, as a hostile worker would. The error if the kernel refuses.
    fn send_raw_handles(channel: &Channel, fd: RawFd, count: usize) -> io::Result<()> {
        let mut control = vec![0u64; (count * size_of::<RawFd>() + 64).div_ceil(8)];
        let byte = 1u8;
        let mut iov = libc::iovec { iov_base: (&raw const byte).cast_mut().cast(), iov_len: 1 };
        // SAFETY: a zeroed `msghdr` is valid: null pointers and zero lengths.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        let payload = (count * size_of::<RawFd>()) as u32;
        // SAFETY: `CMSG_SPACE` is arithmetic.
        msg.msg_controllen = unsafe { libc::CMSG_SPACE(payload) };
        // SAFETY: `control` is aligned and sized above for this header and these descriptors.
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&raw const msg);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(payload);
            let data = libc::CMSG_DATA(header).cast::<RawFd>();
            for i in 0..count {
                data.add(i).write_unaligned(fd);
            }
        }
        // SAFETY: `msg` and everything it points at outlive the call.
        if unsafe { libc::sendmsg(channel.0.as_raw_fd(), &raw const msg, 0) } < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// Failure: a worker sends ~200 handles, more than the host's control buffer held. The old walk trusted `cmsg_len`, read past the buffer and wrapped
    /// garbage as descriptors, so the host closed files it didn't own or crashed.
    #[test]
    fn a_flood_of_handles_is_received_in_bounds_and_all_closable() {
        let (a, b) = Channel::pair().unwrap();
        let file = File::open("/etc/hosts").unwrap();
        send_raw_handles(&a, file.as_raw_fd(), 200).unwrap();
        let (mut buf, mut handles) = ([0u8; 4], Vec::new());
        assert_eq!(b.recv(&mut buf, &mut handles).unwrap(), 1);
        assert_eq!(handles.len(), 200, "every one that was installed must be owned, so it can be closed");
        drop(handles);
    }

    /// Failure: a message can be larger than the receive buffer. Then the kernel installs descriptors nobody can see, and they leak for good.
    /// `ControlBuffer` is sized on the kernel's refusal of larger messages, and this holds that in place for the next macOS.
    #[test]
    fn the_kernel_refuses_more_handles_than_fit() {
        let (a, _b) = Channel::pair().unwrap();
        let file = File::open("/etc/hosts").unwrap();
        let too_many = size_of::<ControlBuffer>() / size_of::<RawFd>();
        assert!(send_raw_handles(&a, file.as_raw_fd(), too_many).is_err(), "the kernel accepted a message that can't fit the buffer");
    }

    /// Failure: a truncated message reads as a normal one, so whoever is misbehaving goes unnoticed, or the walk runs past the bytes written.
    #[test]
    fn a_truncated_message_is_reported_and_stays_in_bounds() {
        let (a, b) = Channel::pair().unwrap();
        let file = File::open("/etc/hosts").unwrap();
        send_raw_handles(&a, file.as_raw_fd(), 64).unwrap();
        let (mut buf, mut handles) = ([0u8; 4], Vec::new());
        let mut small = [0u64; 8];
        // SAFETY: reinterpreting `u64`s as bytes in place, within the array, which only this function holds.
        let small = unsafe { std::slice::from_raw_parts_mut(small.as_mut_ptr().cast::<u8>(), size_of::<[u64; 8]>()) };
        let error = b.recv_into(&mut buf, &mut handles, small).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(handles.len() <= size_of::<[u64; 8]>() / size_of::<RawFd>(), "more handles than the bytes that were written could hold");
    }

    fn cmsg(claimed: u32, level: i32, kind: i32, fds: &[i32]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&claimed.to_ne_bytes());
        out.extend_from_slice(&level.to_ne_bytes());
        out.extend_from_slice(&kind.to_ne_bytes());
        for fd in fds {
            out.extend_from_slice(&fd.to_ne_bytes());
        }
        out
    }

    fn found(control: &[u8]) -> Vec<i32> {
        let mut out = Vec::new();
        descriptors_in(control, |fd| out.push(fd));
        out
    }

    /// Failure: malformed control data makes the walk read out of bounds, hang on a zero length, or treat other messages as descriptors.
    #[test]
    fn malformed_control_data_yields_only_what_is_really_there() {
        let rights = (libc::SOL_SOCKET, libc::SCM_RIGHTS);
        assert_eq!(found(&cmsg(20, rights.0, rights.1, &[7, 8])), vec![7, 8]);
        assert_eq!(found(&cmsg(1000, rights.0, rights.1, &[7, 8])), vec![7, 8], "a claim past the end");
        assert_eq!(found(&cmsg(0, rights.0, rights.1, &[7, 8])), Vec::<i32>::new(), "a zero length mustn't loop for ever");
        assert_eq!(found(&cmsg(11, rights.0, rights.1, &[7])), Vec::<i32>::new(), "shorter than a header");
        assert_eq!(found(&cmsg(u32::MAX, rights.0, rights.1, &[7])), vec![7], "a length that would overflow");
        assert_eq!(found(&cmsg(16, rights.0, rights.1 + 1, &[7])), Vec::<i32>::new(), "another kind of message");
        assert_eq!(found(&cmsg(16, rights.0, rights.1, &[-1])), Vec::<i32>::new(), "not a descriptor");
        assert_eq!(found(&[1, 2, 3]), Vec::<i32>::new(), "shorter than a header");
        assert_eq!(found(&cmsg(14, rights.0, rights.1, &[7])), Vec::<i32>::new(), "half a descriptor");
        let mut two = cmsg(16, rights.0, rights.1, &[7]);
        two.extend(cmsg(16, rights.0, rights.1, &[8]));
        assert_eq!(found(&two), vec![7, 8], "the walk reaches the next message");
        // SAFETY: arithmetic only. The offsets used above must agree with the OS's own macros.
        unsafe {
            assert_eq!(libc::CMSG_LEN(4) as usize, 16);
            assert_eq!(libc::CMSG_SPACE(4) as usize, 16);
        }
    }

    /// Failure: a child that left stays a zombie until someone happens to ask, holding a process-table entry. It must be collected with nobody asking.
    #[test]
    fn a_child_that_ends_is_collected_with_nobody_asking() {
        let (_ours, theirs) = Channel::pair().unwrap();
        let child = ChildProcess::spawn(Path::new("/bin/sh"), &[OsStr::new("-c"), OsStr::new("exit 0")], &[], theirs, None).unwrap();
        let shared = child.0.clone();
        let started = std::time::Instant::now();
        // Only reads the record the reaper fills in, without calling anything that would collect it.
        while shared.end().exit.is_none() {
            assert!(started.elapsed() < Duration::from_secs(30), "nobody collected it");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(shared.end().exit, Some(ChildExit::Code(0)));
    }

    /// Failure: a child that ignores its idle time keeps its memory for ever, because nobody is watching between jobs.
    #[test]
    fn an_idle_rule_kills_a_child_that_outstays_it() {
        let (_ours, theirs) = Channel::pair().unwrap();
        let rule = IdleRule { after: Duration::from_millis(50), memory_limit_bytes: u64::MAX };
        let child = ChildProcess::spawn(Path::new("/bin/sleep"), &[OsStr::new("600")], &[], theirs, Some(rule)).unwrap();
        child.note_activity(false);
        assert_eq!(child.wait_bounded(Duration::from_secs(30)), Some(ChildExit::Signal(libc::SIGKILL)));
    }

    /// Failure: a long job is mistaken for idleness and killed by the idle rule, rather than judged by its own deadline.
    #[test]
    fn a_busy_child_isnt_idle() {
        let (_ours, theirs) = Channel::pair().unwrap();
        let rule = IdleRule { after: Duration::from_millis(20), memory_limit_bytes: u64::MAX };
        let child = ChildProcess::spawn(Path::new("/bin/sleep"), &[OsStr::new("600")], &[], theirs, Some(rule)).unwrap();
        child.note_activity(true);
        assert_eq!(child.wait_bounded(Duration::from_millis(200)), None, "killed during a job");
        child.note_activity(false);
        assert_eq!(child.wait_bounded(Duration::from_secs(30)), Some(ChildExit::Signal(libc::SIGKILL)), "and killed once the job is over");
    }

    /// Failure: the reading of free memory silently fails, so the heavier retry is never allowed on any Mac, or reads a number that isn't a percentage.
    #[test]
    fn free_memory_is_readable_and_a_percentage() {
        let percent = crate::memory_free_percent().expect("the reading failed");
        assert!(percent <= 100, "{percent}");
    }

    /// Failure: something tied to a child's life outlasts it, because it only ends when someone next asks. It must run with nobody asking, whether it left or was killed.
    #[test]
    fn an_end_action_runs_when_the_child_goes_with_nobody_asking() {
        let (_ours, theirs) = Channel::pair().unwrap();
        let child = ChildProcess::spawn(Path::new("/bin/sleep"), &[OsStr::new("600")], &[], theirs, None).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        child.on_end(Box::new(move || sender.send(()).unwrap()));
        assert!(receiver.try_recv().is_err(), "it ran while the child was alive");
        assert!(child.kill_now());
        receiver.recv_timeout(Duration::from_secs(30)).expect("nothing ran it");
        let (_ours, theirs) = Channel::pair().unwrap();
        let already = ChildProcess::spawn(Path::new("/bin/sh"), &[OsStr::new("-c"), OsStr::new("exit 0")], &[], theirs, None).unwrap();
        assert!(already.wait_bounded(Duration::from_secs(30)).is_some());
        let (sender, receiver) = std::sync::mpsc::channel();
        already.on_end(Box::new(move || sender.send(()).unwrap()));
        receiver.recv_timeout(Duration::from_secs(30)).expect("a child already gone should run it at once");
    }

    /// Failure: a killed worker stuck on a dead volume wakes the reaper about 1,000 times a second for as long as it's stuck, past the budget of one a second for an idle app.
    #[test]
    fn a_stuck_child_is_looked_at_once_a_second_at_most() {
        assert!(reaper::next_check_ms(100, 5000, false, true) >= 1000, "just killed, and long past its time");
        assert!(reaper::next_check_ms(100, 100, false, true) >= 1000);
        assert_eq!(reaper::next_check_ms(35_000, 5_000, false, false), 30_000, "alive and idle: when its time is up");
        assert_eq!(reaper::next_check_ms(35_000, 5_000, true, false), 35_000, "alive and busy: a whole period on");
    }
}
