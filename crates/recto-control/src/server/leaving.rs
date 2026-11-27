//! **The socket's name goes before the session ends, whichever way it ends.** A client whose connection closes connects again to ask whether the session is still there, so
//! while the name is on disk and the listener open it hears "still running" from a session on its way out. Every way the process ends takes the name first.

use recto_platform::local::{self, SocketFile};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The sockets this process has bound, for the ways out that have no server in hand (an update's exit, a failed first frame).
static BOUND: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub(super) fn bound(socket: &Path) {
    let mut names = BOUND.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !names.iter().any(|name| name == socket) {
        names.push(socket.to_path_buf());
    }
}

/// Takes a socket's name away, if it is still a socket of this user's own. Idempotent. Never touches anything else that's there.
pub(super) fn release(socket: &Path) {
    if matches!(local::socket_file(socket), Ok(SocketFile::OwnSocket)) {
        let _ = std::fs::remove_file(socket);
    }
}

/// Takes the name of every socket this process serves, before it ends by a path that doesn't go through a server's stop.
pub fn release_socket_names() {
    let names = BOUND.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    for name in names {
        release(&name);
    }
}

/// **Ends the process with `code`,** the socket's name first. `exit` is the process's own end; a test gives it something that returns.
pub fn exit_process_with<T>(code: i32, exit: impl FnOnce(i32) -> T) -> T {
    release_socket_names();
    exit(code)
}

/// Ends the process with `code`, the socket's name first.
pub fn exit_process(code: i32) -> ! {
    exit_process_with(code, |code| std::process::exit(code))
}
