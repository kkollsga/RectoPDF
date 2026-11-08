//! Running a system tool with a limit on time and on what it can say. A tool that wedges or a server that sends for ever can't hold anything: it's stopped, and the caller's told.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How a run went.
#[derive(Debug, Default)]
pub(crate) struct Ran {
    /// Its exit code. None if it was stopped or ended by a signal.
    pub code: Option<i32>,
    pub timed_out: bool,
    /// What it was given said to stop.
    pub stopped: bool,
    /// What it said on stderr, up to a limit.
    pub stderr: Vec<u8>,
}

/// The most of a tool's stderr kept.
const STDERR_KEPT: usize = 16 * 1024;
const STEP: Duration = Duration::from_millis(10);

/// Runs it with a limit, and hands what it says on stdout to `sink`, which says whether to go on. Takes a few small threads for as long as it runs, and none after.
pub(crate) fn run(mut command: Command, limit: Duration, mut sink: impl FnMut(&[u8]) -> bool + Send) -> std::io::Result<Ran> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut out = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
    let mut err = child.stderr.take().ok_or_else(|| std::io::Error::other("no stderr"))?;
    let stop = std::sync::atomic::AtomicBool::new(false);
    let mut ran = Ran::default();
    std::thread::scope(|scope| {
        let stop = &stop;
        let reader = std::thread::Builder::new()
            .name("recto-update-out".into())
            .stack_size(128 * 1024)
            .spawn_scoped(scope, move || {
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    match out.read(&mut buffer) {
                        Ok(0) | Err(_) => return,
                        Ok(count) => {
                            if !sink(&buffer[..count]) {
                                stop.store(true, std::sync::atomic::Ordering::SeqCst);
                                return;
                            }
                        }
                    }
                }
            });
        let complaints = std::thread::Builder::new()
            .name("recto-update-err".into())
            .stack_size(64 * 1024)
            .spawn_scoped(scope, move || {
                let mut kept = Vec::new();
                let mut buffer = [0u8; 4096];
                while let Ok(count) = err.read(&mut buffer) {
                    if count == 0 {
                        break;
                    }
                    if kept.len() < STDERR_KEPT {
                        kept.extend_from_slice(&buffer[..count.min(STDERR_KEPT - kept.len())]);
                    }
                }
                kept
            });
        let deadline = Instant::now() + limit;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    ran.code = status.code();
                    break;
                }
                Ok(None) => {}
                Err(_) => break,
            }
            let over = Instant::now() >= deadline;
            if over || stop.load(std::sync::atomic::Ordering::SeqCst) {
                ran.timed_out = over;
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(STEP);
        }
        ran.stopped = stop.load(std::sync::atomic::Ordering::SeqCst);
        if let Ok(reader) = reader {
            let _ = reader.join();
        }
        if let Ok(complaints) = complaints
            && let Ok(kept) = complaints.join()
        {
            ran.stderr = kept;
        }
    });
    Ok(ran)
}

/// A system tool's whole answer, up to a limit, with whether it worked.
pub(crate) fn tool(program: &str, args: &[&str], limit: Duration, max_out: usize) -> std::io::Result<(bool, Vec<u8>, Ran)> {
    let mut command = Command::new(program);
    command.args(args).env_clear();
    let mut out = Vec::new();
    let ran = run(command, limit, |bytes| {
        if out.len() + bytes.len() > max_out {
            return false;
        }
        out.extend_from_slice(bytes);
        true
    })?;
    Ok((ran.code == Some(0) && !ran.stopped && !ran.timed_out, out, ran))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Failure: a tool that never ends holds the thread for ever, so a check or an install never finishes.
    #[test]
    fn a_tool_that_hangs_is_stopped() {
        let started = Instant::now();
        let (fine, _, ran) = tool("/bin/sleep", &["30"], Duration::from_millis(100), 1024).unwrap();
        assert!(!fine && ran.timed_out);
        assert!(started.elapsed() < Duration::from_secs(25), "{:?}", started.elapsed());
    }

    /// Failure: a tool that says too much fills memory, or one that keeps going past the limit isn't stopped.
    #[test]
    fn a_tool_that_says_too_much_is_stopped() {
        let started = Instant::now();
        let (fine, out, ran) = tool("/usr/bin/yes", &[], Duration::from_secs(20), 256 * 1024).unwrap();
        assert!(!fine && ran.stopped);
        assert!(out.len() <= 256 * 1024);
        assert!(started.elapsed() < Duration::from_secs(19), "{:?}", started.elapsed());
    }

    /// Failure: what a tool says and how it ended don't come back.
    #[test]
    fn what_a_tool_says_comes_back() {
        let (fine, out, _) = tool("/bin/sh", &["-c", "echo out; echo err >&2"], Duration::from_secs(5), 1024).unwrap();
        assert!(fine);
        assert_eq!(out, b"out\n");
        let (fine, _, ran) = tool("/bin/sh", &["-c", "echo err >&2; exit 3"], Duration::from_secs(5), 1024).unwrap();
        assert!(!fine);
        assert_eq!(ran.code, Some(3));
        assert_eq!(ran.stderr, b"err\n");
    }
}
