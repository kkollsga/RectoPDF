//! The one detached-process builder, with an optional retained child handle.
use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Child;

/// Starts a detached program while retaining the actual unreaped child.
/// The caller owns its cancellation and reaping; its PID is not a handle.
///
/// # Errors
/// The log could not be opened or the child could not be started.
pub fn spawn_detached_child(program: &Path, args: &[String], envs: &[(String, String)], log: &Path) -> io::Result<Child> {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};
    let out = OpenOptions::new().create(true).append(true).mode(0o600).open(log)?;
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(envs.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out);
    // SAFETY: setsid is async-signal-safe, and touches nothing of this process.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command.spawn()
}

/// Starts a detached program and gives it to a reaper for this process's lifetime.
///
/// # Errors
/// The log could not be opened or the child could not be started.
pub fn spawn_detached(program: &Path, args: &[String], envs: &[(String, String)], log: &Path) -> io::Result<u32> {
    let mut child = spawn_detached_child(program, args, envs, log)?;
    let pid = child.id();
    let _ = std::thread::Builder::new().name("recto-reap".into()).stack_size(64 * 1024).spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}
