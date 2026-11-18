//! Test-only command ownership/capture guards; OS helper processes, never an app launch.
#![cfg(target_os = "macos")]

#[path = "probe/command.rs"]
mod command;

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LIMIT: Duration = Duration::from_secs(5);

struct Peer(UnixStream);

impl Peer {
    fn closed(&mut self) -> bool {
        self.0.read(&mut [0]).is_ok_and(|n| n == 0)
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        // A failing seeded collector must not leak its writer. This is a private live connection, so releasing it needs no PID-based signal.
        let _ = self.0.write_all(b"q");
        let _ = self.closed();
    }
}

struct Fixture {
    _scratch: recto_testpdf::TempDir,
    listener: UnixListener,
    path: std::path::PathBuf,
    peer: Option<Peer>,
}

impl Fixture {
    fn new() -> Self {
        let scratch = recto_testpdf::TempDir::new_in(std::path::Path::new("/tmp"), "recto-cmd-");
        let path = scratch.join("ready");
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            _scratch: scratch,
            listener,
            path,
            peer: None,
        }
    }

    fn command(&self, mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "collector_peer", "--ignored", "--nocapture"])
            .env("RECTO_COMMAND_PEER", mode)
            .env("RECTO_COMMAND_SOCKET", &self.path);
        command
    }

    fn ready(&mut self) -> io::Result<()> {
        let began = Instant::now();
        let stream = loop {
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            let left = LIMIT.saturating_sub(began.elapsed());
            if left.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "owned peer never became ready"));
            }
            let mut fd = libc::pollfd {
                fd: self.listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: the listener owns this descriptor throughout the poll.
            if unsafe { libc::poll(&mut fd, 1, left.as_millis().min(i32::MAX as u128) as i32) } < 0 {
                return Err(io::Error::last_os_error());
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(LIMIT))?;
        stream.set_write_timeout(Some(LIMIT))?;
        let mut peer = Peer(stream);
        let mut pid = [0; 4];
        peer.0.read_exact(&mut pid)?;
        assert!(u32::from_be_bytes(pid) > 1);
        self.peer = Some(peer);
        Ok(())
    }

    fn closed(&mut self) -> bool {
        self.peer.as_mut().expect("peer handshook").closed()
    }
}

/// Invoked only as a subprocess by these tests. The writer holds inherited stdout/stderr until its private owner releases it or the collector retires its group.
#[test]
#[ignore]
fn collector_peer() {
    let mode = std::env::var("RECTO_COMMAND_PEER").unwrap();
    if mode == "leader" {
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "collector_peer", "--ignored", "--nocapture"])
            .env("RECTO_COMMAND_PEER", "writer")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        std::process::exit(0);
    }
    let mut stream = UnixStream::connect(std::env::var_os("RECTO_COMMAND_SOCKET").unwrap()).unwrap();
    stream.write_all(&std::process::id().to_be_bytes()).unwrap();
    loop {
        let mut byte = [0];
        if stream.read_exact(&mut byte).is_err() || byte == *b"q" {
            break;
        }
        if stream.write_all(&byte).is_err() {
            break;
        }
    }
}

/// Failure: regular-file capture under the probe's real FSIZE=0 kills a command that could print to pipes; signal exits must remain ordinary captured exits rather than fabricated timeouts.
#[test]
fn pipes_preserve_zero_file_limit_output_and_signal_status() {
    let mut print = Command::new("/bin/sh");
    print.args(["-c", "printf pipe-output; printf pipe-error >&2"]);
    // SAFETY: the child-only hook performs one async-signal-safe limit syscall and no allocation.
    unsafe {
        print.pre_exec(|| {
            let zero = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            if libc::setrlimit(libc::RLIMIT_FSIZE, &zero) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
    let (bounded, output) = command::output(&mut print, LIMIT).unwrap();
    assert!(!bounded && output.status.success(), "FSIZE=0 must allow pipe output: {output:?}");
    assert_eq!(output.stdout, b"pipe-output");
    assert_eq!(output.stderr, b"pipe-error");
    let (bounded, output) = command::output(Command::new("/bin/sh").args(["-c", "kill -TERM $$"]), LIMIT).unwrap();
    assert!(!bounded);
    assert_eq!(output.status.signal(), Some(libc::SIGTERM));
}

/// Failure: capturing into unbounded files and truncating afterward never reports cap+1 and permits unbounded output while a command runs. Both streams are checked at the small injected cap and cap+1.
#[test]
fn each_stream_is_capped_while_the_command_produces_output() {
    const CAP: usize = 32;
    for stream in ["", " >&2"] {
        for bytes in [CAP, CAP + 1] {
            let mut print = Command::new("/bin/sh");
            print.args(["-c", &format!("printf '%0{bytes}d' 0{stream}")]);
            let (bounded, output) =
                command::output_after_spawn(&mut print, LIMIT, CAP, |_| Ok(())).unwrap_or_else(|error| panic!("stream={stream:?}, bytes={bytes}: {error}"));
            assert_eq!(bounded, bytes > CAP, "stream={stream:?}, bytes={bytes}, {output:?}");
            assert!(output.stdout.len() <= CAP && output.stderr.len() <= CAP);
            assert_eq!(output.stdout.len() + output.stderr.len(), CAP);
        }
    }
}

/// Failure: the command leader is reaped without retiring the ready descendant that inherited its output descriptors, so a writer survives or EOF collection waits forever.
#[test]
fn an_exited_leader_retires_its_inherited_writer_before_reaping() {
    let mut fixture = Fixture::new();
    let mut leader = fixture.command("leader");
    let (bounded, output) = command::output_after_spawn(&mut leader, LIMIT, 4096, |_| fixture.ready()).unwrap();
    assert!(!bounded && output.status.success(), "{output:?}");
    assert!(fixture.closed(), "the inherited writer survived normal leader exit");
}

/// Failure: a deadline or an early capture error forgets the owned process group. The zero deadline is injected only after the real peer is ready, and private socket EOF proves cleanup without waiting a default timeout.
#[test]
fn deadlines_and_errors_retire_the_ready_owned_peer() {
    for error in [false, true] {
        let mut fixture = Fixture::new();
        let mut peer = fixture.command("writer");
        let result = command::output_after_spawn(&mut peer, Duration::ZERO, 4096, |_| {
            fixture.ready()?;
            if error {
                Err(io::Error::other("injected capture setup failure"))
            } else {
                Ok(())
            }
        });
        if error {
            assert!(result.is_err());
        } else {
            assert!(result.unwrap().0);
        }
        assert!(fixture.closed(), "owned peer survived deadline/error cleanup");
    }
}

/// Failure: cleanup signals a process-group number after another waiter consumed its leader, so the identifier no longer proves ownership. The inherited writer must remain alive until this fixture releases its private control connection.
#[test]
fn losing_the_leader_anchor_never_signals_its_former_group() {
    let mut fixture = Fixture::new();
    let mut leader = fixture.command("leader");
    let result = command::output_after_spawn(&mut leader, LIMIT, 4096, |pid| {
        fixture.ready()?;
        let mut status = 0;
        // SAFETY: this test intentionally consumes precisely its spawned leader; the collector must observe ECHILD and refuse further group signals.
        assert_eq!(unsafe { libc::waitpid(pid as i32, &mut status, 0) }, pid as i32);
        Ok(())
    });
    assert_eq!(result.unwrap_err().raw_os_error(), Some(libc::ECHILD));
    let peer = fixture.peer.as_mut().unwrap();
    peer.0.write_all(b"p").unwrap();
    let mut reply = [0];
    peer.0.read_exact(&mut reply).expect("the live peer must answer after ownership was lost");
    assert_eq!(reply, *b"p", "the former group was signalled after ECHILD");
    peer.0.write_all(b"q").unwrap();
    assert!(peer.closed(), "released private peer did not exit");
}
