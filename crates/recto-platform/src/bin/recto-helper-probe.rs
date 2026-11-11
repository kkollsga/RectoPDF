//! For tests only, never in the app: `tests/helper.rs` runs it to check `recto_platform::helper::spawn` from a process set up as a test process can't
//! be, since Rust ignores `SIGPIPE` in every program it starts and a test can't close its own standard output. Each mode says what it found on standard
//! error and in its exit code: 0 when the spawn behaved.
//!
//! - `sigpipe`: `SIGPIPE` back at its default, as in a Swift host, then a write to a helper that's gone. Must be an error, not death by signal 13.
//! - `closed-stdio`: standard input and output closed, as `recto <&-` has them, then a helper that echoes what it's sent.
//! - `signals`: every signal blocked on the spawning thread and `SIGTERM` ignored, then a copy of this probe as the helper, which says its own mask
//!   and its `SIGTERM` on its standard output, relayed here on standard output.
//! - With `RECTO_HELPER_PROBE=report` in the environment and no arguments, it's that helper.

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(mac::run());
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod mac {
    use recto_platform::helper::{Priority, Spawned, spawn};
    use std::ffi::OsStr;
    use std::io::{Read, Write};
    use std::path::Path;

    pub fn run() -> i32 {
        if std::env::var_os("RECTO_HELPER_PROBE").is_some_and(|mode| mode == "report") {
            return report();
        }
        match std::env::args().nth(1).as_deref() {
            Some("sigpipe") => sigpipe(),
            Some("closed-stdio") => closed_stdio(),
            Some("signals") => signals(),
            _ => 2,
        }
    }

    fn sigpipe() -> i32 {
        // SAFETY: puts one signal back at its default, before any thread is started.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
        let Spawned { mut child, mut stdin, stdout } = spawn(Path::new("/usr/bin/true"), &[], Priority::Utility).unwrap();
        child.wait().unwrap();
        drop(stdout);
        // A pipe's buffer takes 64 KiB at most, so this many writes reach the closed end.
        match (0..64).try_for_each(|_| stdin.write_all(&[0u8; 4096])) {
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => 0,
            other => {
                eprintln!("the write said {other:?}");
                3
            }
        }
    }

    fn closed_stdio() -> i32 {
        // SAFETY: closes this process's own standard input and output, which nothing here uses after this.
        unsafe {
            libc::close(0);
            libc::close(1);
        }
        let Spawned {
            mut child,
            mut stdin,
            mut stdout,
        } = match spawn(Path::new("/bin/sh"), &[], Priority::Inherited) {
            Ok(spawned) => spawned,
            Err(error) => {
                eprintln!("spawn: {error}");
                return 4;
            }
        };
        let _ = stdin.write_all(b"echo through\n");
        drop(stdin);
        let mut said = String::new();
        let _ = stdout.read_to_string(&mut said);
        let ended = child.wait();
        if said == "through\n" && ended.as_ref().is_ok_and(|status| status.success()) {
            0
        } else {
            eprintln!("the helper said {said:?} and ended {ended:?}");
            5
        }
    }

    fn signals() -> i32 {
        // SAFETY: changes only this thread's mask and this process's disposition of one signal, for the spawn that follows.
        unsafe {
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&raw mut all);
            libc::pthread_sigmask(libc::SIG_BLOCK, &raw const all, std::ptr::null_mut());
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
        let exe = std::env::current_exe().unwrap();
        let Spawned { mut child, stdin, mut stdout } = spawn(&exe, &[("RECTO_HELPER_PROBE", OsStr::new("report"))], Priority::Inherited).unwrap();
        drop(stdin);
        let mut said = String::new();
        let _ = stdout.read_to_string(&mut said);
        let _ = child.wait();
        print!("{said}");
        0
    }

    /// As the helper: its mask and what `SIGTERM` does, before it changes either.
    fn report() -> i32 {
        // SAFETY: reads this thread's mask and one signal's disposition, changing neither.
        let (mask, term) = unsafe {
            let mut mask: libc::sigset_t = std::mem::zeroed();
            libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &raw mut mask);
            let mut action: libc::sigaction = std::mem::zeroed();
            libc::sigaction(libc::SIGTERM, std::ptr::null(), &raw mut action);
            (mask, action.sa_sigaction)
        };
        let term = if term == libc::SIG_DFL {
            "default"
        } else if term == libc::SIG_IGN {
            "ignored"
        } else {
            "handled"
        };
        println!("mask {mask} sigterm {term}");
        0
    }
}
