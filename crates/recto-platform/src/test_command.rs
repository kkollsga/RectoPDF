//! Bounded pipe capture for tests' owned commands; compiled into the library only with `test-seams`. The leader stays waitable until its group has been retired.

use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

const CAP: usize = 64 * 1024;

pub fn output(command: &mut Command, timeout: Duration) -> io::Result<(bool, Output)> {
    output_after_spawn(command, timeout, CAP, |_| Ok(()))
}

/// The tests handshake an actual peer before injecting a zero deadline or an error. Normal callers add no work here and use the public cap.
pub(super) fn output_after_spawn(
    command: &mut Command,
    timeout: Duration,
    cap: usize,
    ready: impl FnOnce(u32) -> io::Result<()>,
) -> io::Result<(bool, Output)> {
    let mut owned = Owned {
        child: command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?,
        anchor: true,
        retiring: false,
    };
    let deadline = Instant::now() + timeout;
    let mut stdout = Pipe::new(owned.child.stdout.take().ok_or_else(|| io::Error::other("missing stdout pipe"))?)?;
    let mut stderr = Pipe::new(owned.child.stderr.take().ok_or_else(|| io::Error::other("missing stderr pipe"))?)?;
    ready(owned.child.id())?;
    let mut status = None;
    let exceeded = loop {
        // One bounded read per stream per turn: neither a busy writer nor EINTR can starve the other pipe or the deadline.
        let out_full = stdout.read(cap)?;
        let err_full = stderr.read(cap)?;
        if out_full || err_full || Instant::now() >= deadline {
            break true;
        }
        if status.is_none() && owned.exited()? {
            status = Some(owned.finish()?);
        }
        if status.is_some() && stdout.eof && stderr.eof {
            break false;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let mut fds = [stdout.pollfd(), stderr.pollfd()];
        let millis = remaining.as_millis().clamp(1, 10) as libc::c_int;
        // SAFETY: both descriptors belong to the live pipe objects, and fds has two entries.
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    };
    let status = match status {
        Some(status) => status,
        None => owned.finish()?,
    };
    Ok((
        exceeded,
        Output {
            status,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
        },
    ))
}

struct Pipe<R> {
    reader: R,
    bytes: Vec<u8>,
    eof: bool,
}

impl<R: Read + AsRawFd> Pipe<R> {
    fn new(reader: R) -> io::Result<Self> {
        let fd = reader.as_raw_fd();
        // SAFETY: fd belongs to reader; only its nonblocking flag is changed.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            reader,
            bytes: Vec::new(),
            eof: false,
        })
    }

    fn read(&mut self, cap: usize) -> io::Result<bool> {
        if self.eof {
            return Ok(false);
        }
        let mut chunk = [0; 4096];
        let room = cap.saturating_sub(self.bytes.len());
        let count = chunk.len().min(room.saturating_add(1));
        match self.reader.read(&mut chunk[..count]) {
            Ok(0) => {
                self.eof = true;
                Ok(false)
            }
            Ok(count) => {
                self.bytes.extend_from_slice(&chunk[..count.min(room)]);
                Ok(count > room)
            }
            Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn pollfd(&self) -> libc::pollfd {
        libc::pollfd {
            fd: if self.eof { -1 as RawFd } else { self.reader.as_raw_fd() },
            events: libc::POLLIN,
            revents: 0,
        }
    }
}

struct Owned {
    child: Child,
    anchor: bool,
    retiring: bool,
}

impl Owned {
    fn exited(&mut self) -> io::Result<bool> {
        // SAFETY: zero initialization is valid for siginfo_t, which waitid fills on success. WNOWAIT deliberately leaves the ownership anchor unreaped.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::waitid(libc::P_PID, self.child.id(), &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
        if result == 0 {
            return Ok(info.si_pid != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        if error.raw_os_error() == Some(libc::ECHILD) {
            self.anchor = false;
        }
        Err(error)
    }

    fn finish(&mut self) -> io::Result<ExitStatus> {
        if !self.anchor {
            return Err(io::Error::from_raw_os_error(libc::ECHILD));
        }
        self.retiring = true;
        // Verify ownership before any signal, including cleanup after an I/O or callback error.
        self.exited()?;
        let group = libc::pid_t::try_from(self.child.id()).map_err(|_| io::Error::other("invalid owned process group"))?;
        if group <= 1 {
            return Err(io::Error::other("invalid owned process group"));
        }
        // SAFETY: this command created group=leader PID, and that child remains owned/unreaped. Retire descendants BEFORE releasing that anchor.
        if unsafe { libc::kill(-group, libc::SIGKILL) } != 0 {
            let error = io::Error::last_os_error();
            // The leader can exit between the ownership check and signal. Verify again without reaping before accepting Darwin's zombie-only result.
            if error.raw_os_error() == Some(libc::EPERM) {
                self.await_exit()?;
                if !no_live_members(group) {
                    return Err(io::Error::new(error.kind(), format!("owned group {group} cleanup refused: {error}")));
                }
            } else if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(io::Error::new(error.kind(), format!("owned group {group} cleanup failed: {error}")));
            }
        }
        self.await_exit()?;
        let status = self.child.wait();
        // No future signal can target this identifier after wait, even if wait reports lost ownership.
        self.anchor = false;
        status
    }

    fn await_exit(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if self.exited()? {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("owned leader/group {} did not exit during cleanup", self.child.id()),
                ));
            }
            // SAFETY: a zero-descriptor poll waits only for this bounded OS-state retry; the child remains owned/unreaped.
            if unsafe { libc::poll(std::ptr::null_mut(), 0, left.as_millis().clamp(1, 10) as i32) } < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }
}

/// Darwin's group signal excludes zombies and can return EPERM for a zombie-only group (XNU kern_sig.c, killpg1). Accept that case only while the leader remains anchored and a bounded group-only scan proves no live member.
fn no_live_members(group: libc::pid_t) -> bool {
    const PROC_PGRP_ONLY: u32 = 2; // Apple SDK sys/proc_info.h.
    let mut pids = [0 as libc::pid_t; 64];
    let capacity = std::mem::size_of_val(&pids) as libc::c_int;
    // SAFETY: the fixed buffer and byte capacity match; selector2 restricts this scan to the still-owned process group.
    let (count, errno) = unsafe {
        *libc::__error() = 0;
        let count = libc::proc_listpids(PROC_PGRP_ONLY, group as u32, pids.as_mut_ptr().cast(), capacity);
        (count, *libc::__error())
    };
    if count < 0 || (count == 0 && errno != 0) || count >= capacity || !(count as usize).is_multiple_of(size_of::<libc::pid_t>()) {
        return false;
    }
    for pid in pids.into_iter().take(count as usize / size_of::<libc::pid_t>()) {
        if pid <= 0 {
            return false;
        }
        if pid == group {
            continue;
        }
        // SAFETY: zeroed proc_bsdinfo is filled by proc_pidinfo, with its exact byte size.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let expected = size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let found = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), expected) };
        if found != expected || info.pbi_pgid != group as u32 || info.pbi_status != libc::SZOMB {
            return false;
        }
    }
    true
}

impl Drop for Owned {
    fn drop(&mut self) {
        if self.anchor && !self.retiring {
            let _ = self.finish();
        }
    }
}
