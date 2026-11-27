//! A helper process with a pipe each way and a scheduling level for the whole of it, set as it starts. `std::process::Command` can't set a spawn's
//! attributes, so this is `posix_spawn` itself.
//!
//! **Why utility, not Darwin background** (2026-11-11). `PRIO_DARWIN_BG` makes a process's disk I/O wait behind anyone else's, up to 25 ms an I/O on
//! an SSD and 200 ms on a disk the kernel doesn't take for one, and puts it below everything for the processor. On GitHub's virtual Macs that starved
//! the indexer and the headings helper for minutes: `recto wait indexing-done` timed out at 60 s on a four-page file in three runs in one day. Here,
//! with another process reading the same SSD, 64 MiB of uncached reads took 12 s at Darwin background, 3.5 s at utility and 1.2 s at the default
//! level. QoS utility is still below whatever the person is looking at, and its I/O tier is the lightest of the throttled ones (5 ms an I/O on an
//! SSD, 40 ms on another disk), so it's never starved. `docs/engineering/search-index.md` has the numbers.
//!
//! `posix_spawnattr_set_qos_class_np` clamps the whole process at that level, every thread, including any thread pool a library starts, and
//! whatever it starts in turn inherits the clamp: the helper's threads' and its render worker's base priority (`pth_priority`) is 20, against
//! 31 for an ordinary process and 4 at Darwin background, and the CPU time they use is charged to the utility bucket (`ri_cpu_time_qos_utility`)
//! and no other. A thread's current priority is not ours to promise: the kernel raises it for a moment past the clamp while it holds a contended lock.
//! The base priority isn't moved by that, but a user-space push from a higher-QoS thread of the same process can raise it (a thread at user-interactive
//! waiting on a lock a utility thread holds lifted it from 20 to 31, measured 2026-11-25), which the task clamp doesn't apply to.

use std::ffi::{CString, OsStr, c_int};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;

/// `qos_class_t`'s `QOS_CLASS_UTILITY`, from `<sys/qos.h>`.
const QOS_CLASS_UTILITY: u32 = 0x11;

// From `<pthread/spawn.h>`, not yet in the `libc` crate. For utility, background and maintenance it sets the process's QoS clamp, which its threads and
// its children's inherit.
unsafe extern "C" {
    fn posix_spawnattr_set_qos_class_np(attr: *mut libc::posix_spawnattr_t, qos_class: u32) -> c_int;
}

/// How a helper is scheduled, the whole process and what it starts. Which one is the caller's decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// QoS utility: below what the person is looking at, never starved however busy the Mac is.
    Utility,
    /// As this process is, for measuring the work apart from the waiting.
    Inherited,
}

/// A started helper, with its pipes. Dropping the child without waiting for it kills it.
#[derive(Debug)]
pub struct Spawned {
    pub child: Helper,
    /// Its standard input. Writing after it's gone is an error, never a `SIGPIPE`.
    pub stdin: File,
    /// Its standard output.
    pub stdout: File,
}

/// A helper process, collected by whoever holds this: [`Helper::wait`], or [`Helper::try_wait`] until it says it's ended.
#[derive(Debug)]
pub struct Helper {
    pid: libc::pid_t,
    ended: Option<ExitStatus>,
    /// `waitpid` said this isn't a child of this process (`ECHILD`): something else collected it. The number may already be another process's, so
    /// it's never signalled.
    lost: bool,
}

fn check(rc: c_int) -> io::Result<c_int> {
    if rc == -1 { Err(io::Error::last_os_error()) } else { Ok(rc) }
}

fn check_spawn(rc: c_int) -> io::Result<()> {
    if rc == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(rc)) }
}

/// A pipe as two files, read end first, both close-on-exec and numbered above 2. Between `pipe` and the flags, a fork elsewhere in this process could
/// take a copy, as with the standard library's own pipes on macOS; every spawn here closes everything it isn't given.
///
/// Above 2, because a process started with its standard input or output closed (`recto <&-`) gets 0 or 1 from `pipe`, and then putting one end on
/// the child's 0 would overwrite the other before it's put on 1, or the child would be given an end it should never have.
fn pipe() -> io::Result<(File, File)> {
    let mut fds: [RawFd; 2] = [-1; 2];
    // SAFETY: `fds` has room for the two descriptors the call writes.
    check(unsafe { libc::pipe(fds.as_mut_ptr()) })?;
    // SAFETY: both are fresh descriptors that nothing else owns, and each `File` closes its own.
    let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
    Ok((above_stdio(read)?, above_stdio(write)?))
}

/// The same file, close-on-exec, at a number above 2: moved there if it isn't, and the low number closed.
fn above_stdio(file: File) -> io::Result<File> {
    let fd = file.as_raw_fd();
    if fd > 2 {
        // SAFETY: a flag change on a descriptor this owns.
        check(unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) })?;
        return Ok(file);
    }
    // SAFETY: duplicates a descriptor this owns onto the lowest free number from 3, close-on-exec. `file` closes the old one as it goes.
    let moved = check(unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) })?;
    // SAFETY: a fresh descriptor nothing else owns.
    Ok(unsafe { File::from_raw_fd(moved) })
}

/// Starts `exe` with no arguments, only `env` for its environment, its standard input and output on pipes to this process, its standard error on
/// `/dev/null`, and no other descriptor.
pub fn spawn(exe: &Path, env: &[(&str, &OsStr)], priority: Priority) -> io::Result<Spawned> {
    let cstring = |bytes: &[u8]| CString::new(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput));
    let path = cstring(exe.as_os_str().as_bytes())?;
    let argv = [path.as_ptr().cast_mut(), std::ptr::null_mut()];
    let owned_env = env
        .iter()
        .map(|(name, value)| cstring(&[name.as_bytes(), b"=", value.as_bytes()].concat()))
        .collect::<io::Result<Vec<_>>>()?;
    let mut envp: Vec<*mut libc::c_char> = owned_env.iter().map(|entry| entry.as_ptr().cast_mut()).collect();
    envp.push(std::ptr::null_mut());
    let (child_in, stdin) = pipe()?;
    let (stdout, child_out) = pipe()?;
    // SAFETY: a flag change on a descriptor this owns.
    check(unsafe { libc::fcntl(stdin.as_raw_fd(), libc::F_SETNOSIGPIPE, 1) })?;
    let dev_null = c"/dev/null";
    let mut pid: libc::pid_t = 0;
    // SAFETY: `actions` and `attr` are initialised before use and destroyed on every path below. Every pointer passed to the spawn family is
    // NUL-terminated and alive until the call returns, and `argv` and `envp` end in null.
    unsafe {
        let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        check_spawn(libc::posix_spawn_file_actions_init(&raw mut actions))?;
        if let Err(error) = check_spawn(libc::posix_spawnattr_init(&raw mut attr)) {
            libc::posix_spawn_file_actions_destroy(&raw mut actions);
            return Err(error);
        }
        let prepared = (|| {
            // Nothing is inherited unless named below, whatever else in this process forgot to mark close-on-exec. And no signal state either: the
            // spawning thread's blocked signals and anything this process ignores would otherwise carry over, so a helper could start unable to
            // be told to stop. Every signal starts at its default and none is blocked, as the standard library's own spawn does.
            let mut none: libc::sigset_t = std::mem::zeroed();
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&raw mut none);
            libc::sigfillset(&raw mut all);
            check_spawn(libc::posix_spawnattr_setsigmask(&raw mut attr, &raw const none))?;
            check_spawn(libc::posix_spawnattr_setsigdefault(&raw mut attr, &raw const all))?;
            check_spawn(libc::posix_spawnattr_setflags(
                &raw mut attr,
                (libc::POSIX_SPAWN_CLOEXEC_DEFAULT | libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF) as libc::c_short,
            ))?;
            if priority == Priority::Utility {
                check_spawn(posix_spawnattr_set_qos_class_np(&raw mut attr, QOS_CLASS_UTILITY))?;
            }
            check_spawn(libc::posix_spawn_file_actions_adddup2(&raw mut actions, child_in.as_raw_fd(), 0))?;
            check_spawn(libc::posix_spawn_file_actions_adddup2(&raw mut actions, child_out.as_raw_fd(), 1))?;
            check_spawn(libc::posix_spawn_file_actions_addopen(
                &raw mut actions,
                2,
                dev_null.as_ptr(),
                libc::O_WRONLY,
                0,
            ))?;
            check_spawn(libc::posix_spawn(
                &raw mut pid,
                path.as_ptr(),
                &raw const actions,
                &raw const attr,
                argv.as_ptr(),
                envp.as_ptr(),
            ))
        })();
        libc::posix_spawn_file_actions_destroy(&raw mut actions);
        libc::posix_spawnattr_destroy(&raw mut attr);
        prepared?;
    }
    // The child's ends close here, so the helper's end of input is this process's `stdin` and nobody else's.
    drop((child_in, child_out));
    Ok(Spawned {
        child: Helper { pid, ended: None, lost: false },
        stdin,
        stdout,
    })
}

impl Helper {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    fn collect(&mut self, flags: c_int) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.ended {
            return Ok(Some(status));
        }
        if self.lost {
            return Err(io::Error::from_raw_os_error(libc::ECHILD));
        }
        let mut status: c_int = 0;
        loop {
            // SAFETY: `status` is live for the call, and the pid is this child's until it's collected, which only this does.
            let waited = unsafe { libc::waitpid(self.pid, &raw mut status, flags) };
            if waited == self.pid {
                self.ended = Some(ExitStatus::from_raw(status));
                return Ok(self.ended);
            }
            if waited == 0 {
                return Ok(None);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                self.lost = true;
            }
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    /// Whether it's ended, without waiting.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.collect(libc::WNOHANG)
    }

    /// Waits for it to end.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.collect(0)?.ok_or_else(|| io::Error::other("waitpid returned without a status"))
    }

    /// Sends the kill, unless it's already been collected or lost, so a number given to another process is never signalled.
    pub fn kill(&mut self) -> io::Result<()> {
        if self.ended.is_none() && !self.lost {
            // SAFETY: not yet collected, so the pid is still this child.
            check(unsafe { libc::kill(self.pid, libc::SIGKILL) })?;
        }
        Ok(())
    }
}

impl Drop for Helper {
    /// Whoever starts a helper waits for it, on every path. One dropped without that is a bug: it's killed, and collected only if it's already gone,
    /// as this never blocks, so **otherwise it stays a zombie** until this process ends. A debug build says so by panicking, unless it's already
    /// unwinding from a panic of its own.
    fn drop(&mut self) {
        if self.ended.is_none() && !self.lost {
            let _ = self.kill();
            let _ = self.collect(libc::WNOHANG);
            debug_assert!(std::thread::panicking(), "a helper (pid {}) was dropped without a wait", self.pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// One thread's scheduling numbers, from `proc_threadinfo`: where it is now (`pth_curpri`, which the kernel raises for a moment when the thread
    /// holds a contended lock), its base (`pth_priority`, which the task's QoS clamp holds down) and `pth_maxpriority` (63 even under the clamp).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Level {
        current: i32,
        base: i32,
        ceiling: i32,
    }

    /// Every thread of `pid`, read one after another by `proc_pidinfo` (a thread that ends meanwhile is left out), as `thread_names` does. An error
    /// names the pid and the system's reason, for a process that is gone or isn't ours.
    fn thread_levels(pid: u32) -> io::Result<Vec<Level>> {
        // <sys/proc_info.h>: not in the libc crate.
        const PROC_PIDLISTTHREADS: c_int = 6;
        let raw_pid = c_int::try_from(pid).unwrap();
        let mut handles = vec![0u64; 512];
        let room = c_int::try_from(handles.len() * size_of::<u64>()).unwrap();
        // SAFETY: the buffer and its size in bytes describe `handles`; what's written is a count of bytes within it.
        let written = unsafe { libc::proc_pidinfo(raw_pid, PROC_PIDLISTTHREADS, 0, handles.as_mut_ptr().cast(), room) };
        let written = usize::try_from(written).ok().filter(|&bytes| bytes > 0);
        let Some(written) = written else {
            return Err(io::Error::other(format!("listing the threads of {pid}: {}", io::Error::last_os_error())));
        };
        handles.truncate(written / size_of::<u64>());
        let size = c_int::try_from(size_of::<libc::proc_threadinfo>()).unwrap();
        Ok(handles
            .iter()
            .filter_map(|&handle| {
                let mut info = std::mem::MaybeUninit::<libc::proc_threadinfo>::uninit();
                // SAFETY: the buffer and its size describe `info`.
                let got = unsafe { libc::proc_pidinfo(raw_pid, libc::PROC_PIDTHREADINFO, handle, info.as_mut_ptr().cast(), size) };
                if got != size {
                    return None;
                }
                // SAFETY: a return of `size` means it was filled.
                let info = unsafe { info.assume_init() };
                Some(Level {
                    current: info.pth_curpri,
                    base: info.pth_priority,
                    ceiling: info.pth_maxpriority,
                })
            })
            .collect())
    }

    /// The CPU time `pid` has used, by the QoS bucket it ran in, as `proc_pid_rusage` keeps it.
    fn qos_time(pid: u32) -> io::Result<libc::rusage_info_v4> {
        let raw_pid = c_int::try_from(pid).unwrap();
        let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::uninit();
        // SAFETY: the buffer is a `rusage_info_v4`, which flavor 4 fills; a return of 0 means it was.
        let rc = unsafe { libc::proc_pid_rusage(raw_pid, libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast()) };
        if rc != 0 {
            return Err(io::Error::other(format!("proc_pid_rusage of {pid}: {}", io::Error::last_os_error())));
        }
        // SAFETY: filled, as checked.
        Ok(unsafe { info.assume_init() })
    }

    /// A process id read from a line a child printed: positive and within `pid_t`, or `None`. `0` and `-1` would make `kill` signal this test's own
    /// process group or every process the user owns.
    fn parse_pid(line: &str) -> Option<libc::pid_t> {
        line.trim().parse::<libc::pid_t>().ok().filter(|&pid| pid > 0)
    }

    /// Failure: a line the stand-in prints that isn't a process id becomes a `kill` of this test's own group or of everything the user owns.
    #[test]
    fn only_a_positive_pid_is_taken_from_a_helpers_line() {
        for bad in ["0", "-1", "4294967295", "x", "", "12 13"] {
            assert_eq!(parse_pid(bad), None, "{bad:?}");
        }
        assert_eq!(parse_pid(" 4242\n"), Some(4242));
    }

    /// What one reading of the stand-in shell and of what it started gave.
    struct Seen {
        shell: Vec<Level>,
        sleeper: Vec<Level>,
        shell_time: libc::rusage_info_v4,
        sleeper_time: libc::rusage_info_v4,
    }

    /// Reads the sleeper's pid, which its shell prints once it has done its work, then both processes. Every failure is a value, so the caller can
    /// kill and wait before it asserts.
    fn observe(shell: u32, stdout: &mut File) -> (Option<libc::pid_t>, io::Result<Seen>) {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while matches!(io::Read::read(stdout, &mut byte), Ok(1)) && byte[0] != b'\n' {
            line.push(byte[0]);
        }
        let Some(sleeper) = parse_pid(&String::from_utf8_lossy(&line)) else {
            return (
                None,
                Err(io::Error::other(format!(
                    "no pid line from the stand-in shell, got {:?}",
                    String::from_utf8_lossy(&line)
                ))),
            );
        };
        let sleeper_id = u32::try_from(sleeper).expect("a positive pid");
        let seen = (|| {
            Ok(Seen {
                shell: thread_levels(shell)?,
                sleeper: thread_levels(sleeper_id)?,
                shell_time: qos_time(shell)?,
                sleeper_time: qos_time(sleeper_id)?,
            })
        })();
        (Some(sleeper), seen)
    }

    /// The buckets that mean "not utility", as `(name, ns)`, for a message and a check.
    fn outside_utility(info: &libc::rusage_info_v4) -> Vec<(&'static str, u64)> {
        [
            ("default", info.ri_cpu_time_qos_default),
            ("maintenance", info.ri_cpu_time_qos_maintenance),
            ("background", info.ri_cpu_time_qos_background),
            ("legacy", info.ri_cpu_time_qos_legacy),
            ("user_initiated", info.ri_cpu_time_qos_user_initiated),
            ("user_interactive", info.ri_cpu_time_qos_user_interactive),
        ]
        .into_iter()
        .filter(|&(_, ns)| ns > 0)
        .collect()
    }

    /// Failure: the helper runs at Darwin background (also with the clamp kept and background set on top) or at the default priority instead of utility, or a child it starts escapes the level. A shell
    /// that starts a `sleep` stands in for the helper and its render worker. On CI's virtual Macs, Darwin background starved both helpers for minutes.
    /// It doesn't assert a thread's current priority: the kernel lifts it above the clamp for a moment while the thread holds a contended lock, by
    /// design (`ps` read 46 once at load 12, a stand-in shell with a clamp of 20). The QoS bucket the CPU time was charged to doesn't move with that,
    /// so the time is what's asserted, and the threads' numbers are shown on failure.
    #[test]
    fn a_utility_helper_and_what_it_starts_run_at_utility() {
        let started = spawn(Path::new("/bin/sh"), &[], Priority::Utility).unwrap();
        let Spawned {
            mut child,
            mut stdin,
            mut stdout,
        } = started;
        // The stand-in shell works a little, then starts a child that works a little too, and only that child's report (its pid, once it has run and
        // before it becomes a `sleep`) lets this test read it: a reading of a child that hasn't run yet would pass for nothing. The outer shell closes its
        // own stdout, so only that child holds the pipe and its death ends the read instead of hanging it, and the outer shell kills it when its input
        // closes, so a failed run leaves no `sleep`.
        stdin
            .write_all(
                b"i=0; while [ $i -lt 2000 ]; do i=$((i+1)); done; \
                  /bin/sh -c 'i=0; while [ $i -lt 2000 ]; do i=$((i+1)); done; echo $$; exec /bin/sleep 600' & job=$!; \
                  exec >&-; read line; kill $job\n",
            )
            .unwrap();
        let (sleeper_pid, seen) = observe(child.id(), &mut stdout);
        if let Some(pid) = sleeper_pid {
            // SAFETY: a signal to the `sleep` this test started.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        drop(stdin);
        // Its `read` fails as its input closes, so it doesn't end with success. That it ends is what matters.
        child.wait().unwrap();
        let Seen {
            shell,
            sleeper,
            shell_time,
            sleeper_time,
        } = seen.unwrap();
        // The promise: the work ran at utility, and nowhere else. Both have done real work by now, so each has utility time. The threads' scheduling
        // numbers are in the message only: with the clamp kept and Darwin background set on top (the old regression), the buckets fail as well, so a
        // base-priority assert would only repeat them. A kernel lock promotion moves `current`, and a push from a higher-QoS thread of the same
        // process moves `base`, so neither is a promise.
        for (who, time, levels) in [("the helper", &shell_time, &shell), ("what it started", &sleeper_time, &sleeper)] {
            let threads: Vec<String> = levels
                .iter()
                .map(|l| format!("current {} base {} ceiling {}", l.current, l.base, l.ceiling))
                .collect();
            assert!(
                time.ri_cpu_time_qos_utility > 0,
                "{who} used no CPU time at utility: outside it {:?}; threads {threads:?}",
                outside_utility(time)
            );
            // Not a zero tolerance: the kernel can charge a few nanoseconds (53 ns, seen once under load) to the default bucket, most likely around the
            // exec before the clamp is seen; the work itself took milliseconds. The regressions this guards charge the work itself elsewhere: all of it at default
            // priority, or at Darwin background. So the bulk must be at utility, with at most 1 in 20 of it anywhere else.
            let outside: u64 = outside_utility(time).iter().map(|&(_, ns)| ns).sum();
            assert!(
                outside <= time.ri_cpu_time_qos_utility / 20,
                "{who} ran mostly outside utility ({} ns at utility), by bucket in ns: {:?}; threads {threads:?}",
                time.ri_cpu_time_qos_utility,
                outside_utility(time)
            );
        }
    }

    /// Failure: the helper sees more than it's given: an environment variable of this process, or a descriptor beyond its three. A file's name or a
    /// socket reaching a helper that reads hostile PDFs is what this rules out.
    #[test]
    fn a_helper_gets_only_its_environment_and_its_pipes() {
        // A descriptor this process left open across exec, as a library might, at a number nothing else here uses.
        let leaked = std::fs::File::open("/dev/null").unwrap();
        // SAFETY: `dup2` onto a number this test process doesn't use; the copy isn't close-on-exec, which is the point.
        assert_eq!(unsafe { libc::dup2(std::os::fd::AsRawFd::as_raw_fd(&leaked), 77) }, 77);
        let Spawned {
            mut child,
            mut stdin,
            mut stdout,
        } = spawn(Path::new("/bin/sh"), &[("ONLY", OsStr::new("this"))], Priority::Inherited).unwrap();
        stdin.write_all(b"env | sort | tr '\\n' ' '; ls /dev/fd | tr '\\n' ' '\n").unwrap();
        drop(stdin);
        let mut said = String::new();
        stdout.read_to_string(&mut said).unwrap();
        assert!(child.wait().unwrap().success());
        let words: Vec<&str> = said.split_whitespace().collect();
        assert!(words.contains(&"ONLY=this"), "{said}");
        assert!(!words.iter().any(|word| word.starts_with("HOME=") || word.starts_with("CARGO")), "{said}");
        let fds: Vec<&str> = words.iter().copied().filter(|word| word.parse::<u32>().is_ok()).collect();
        assert!(["0", "1", "2"].iter().all(|fd| fds.contains(fd)) && !fds.contains(&"77"), "{said}");
        // SAFETY: closes the copy this test made.
        unsafe { libc::close(77) };
    }

    /// Failure: a helper dropped without a wait is left running, or a debug build doesn't say that a caller forgot to wait. Callers wait: this checks
    /// the path that catches one that doesn't, without collecting the helper itself, so what the drop did is what's seen.
    #[test]
    fn a_helper_dropped_without_a_wait_is_killed_and_said_to_be_a_bug() {
        // A shell waiting on its input stands in for a stuck helper. Its input stays open, so only the kill can end it.
        let Spawned { child, stdin, .. } = spawn(Path::new("/bin/sh"), &[], Priority::Inherited).unwrap();
        let stuck = child.id();
        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(child)));
        assert_eq!(
            dropped.is_err(),
            cfg!(debug_assertions),
            "a debug build says a helper was dropped without a wait"
        );
        // Killed: gone, or a zombie nobody's collected, which `ps` shows as `Z`.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let state = loop {
            let out = std::process::Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &stuck.to_string()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if state.is_empty() || state.contains('Z') || std::time::Instant::now() > deadline {
                break state;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(state.is_empty() || state.contains('Z'), "a dropped helper is killed, and it's {state:?}");
        drop(stdin);
    }
}
