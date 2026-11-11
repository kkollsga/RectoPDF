//! `recto_platform::helper::spawn` from processes a test process can't be: one with `SIGPIPE` at its default, one with its standard input and output
//! closed, and one spawning from a thread with every signal blocked. `recto-helper-probe` is each of them, and says what it found.

#![cfg(target_os = "macos")]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};

const PROBE: &str = env!("CARGO_BIN_EXE_recto-helper-probe");

fn probe(mode: &str) -> (std::process::ExitStatus, String, String) {
    let out = Command::new(PROBE).arg(mode).stdin(Stdio::null()).output().unwrap();
    (
        out.status,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Failure: a write to a helper that's gone raises `SIGPIPE`, which ends a host that doesn't ignore it, as a Swift app doesn't: the whole app gone
/// because a helper crashed. Rust ignores `SIGPIPE` in every program it starts, this test's included, so only a process that puts it back can see this.
#[test]
fn a_write_to_a_helper_thats_gone_is_an_error_even_with_sigpipe_at_its_default() {
    let (status, _, said) = probe("sigpipe");
    assert_ne!(status.signal(), Some(libc::SIGPIPE), "the host died of SIGPIPE");
    assert!(status.success(), "{status}: {said}");
}

/// Failure: with this process's standard input or output closed (`recto <&-`), a pipe takes descriptor 0 or 1, and the helper gets the wrong end, or
/// none: it can't read its job, or what it says never comes back.
#[test]
fn a_helper_gets_its_pipes_when_this_process_has_no_standard_input_or_output() {
    let (status, _, said) = probe("closed-stdio");
    assert!(status.success(), "{status}: {said}");
}

/// Failure: a helper inherits the spawning thread's blocked signals or this process's ignored ones, so it starts unable to be told to stop.
#[test]
fn a_helper_starts_with_no_signal_blocked_and_sigterm_at_its_default() {
    let (status, out, said) = probe("signals");
    assert!(status.success(), "{status}: {said}");
    assert_eq!(out.trim(), "mask 0 sigterm default", "{said}");
}
