//! Bytes from a socket client, sent to the real server, on a real socket, with a real session behind it. A line reader is only as good as what's around it, so this goes through all of it: reading, cutting lines, parsing, the level, jobs, the log.
//!
//! What must hold: every answer is a line of JSON. Nothing is an internal error, which is what a panic in a job comes back as. A `status` sent after anything is answered, or the connection is closed, as it should be for a client that broke the rules. And the next client gets in.

use crate::guard::{Budget, within};
use crate::input::Take;
use crate::targets::NamedLoader;
use recto_agent::commands::{Rights, Waits};
use recto_agent::{AgentError, Command, Condition, ConditionState, Event, Level, parse};
use recto_control::StatePaths;
use recto_control::host::{Host, SessionHost};
use recto_control::levels;
use recto_control::server::{Limits, Server, Stopper};
use recto_platform::local::Stream;
use recto_session::TabId;
use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BUDGET: Budget = Budget {
    time: Duration::from_secs(60),
    memory: 512 * 1024 * 1024,
};
const STATUS: &[u8] = b"\n{\"cmd\":\"status\"}\n";
/// More than twice the longest line a client may send, so what's past the cap gets sent.
pub const MAX_INPUT: usize = 256 * 1024;

/// The session, except that it won't start a worker: nothing here has anything to draw.
struct Guarded(SessionHost<NamedLoader>);

impl Host for Guarded {
    fn run(&self, actor: recto_agent::Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        if command.draws() {
            return Err(AgentError::new("not_here", "Nothing draws in this check.", recto_agent::exit::FAILED));
        }
        self.0.run(actor, command, events)
    }
    fn check(&self, condition: Condition, tab: &recto_agent::TabRef, page: Option<u32>) -> ConditionState {
        self.0.check(condition, tab, page)
    }
    fn tab_count(&self) -> u32 {
        self.0.tab_count()
    }
    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        self.0.unsaved_tabs()
    }
    fn acted_tab(&self, command: &Command) -> Option<TabId> {
        self.0.acted_tab(command)
    }
}

struct Running {
    paths: StatePaths,
    host: Arc<Guarded>,
    stopper: Stopper,
    thread: JoinHandle<std::io::Result<()>>,
    /// A folder of its own when `RECTO_FUZZ_STATE` doesn't name one, never one an earlier process with the same id left; removed with the server.
    _folder: Option<recto_testpdf::TempDir>,
}

static SERVER: Mutex<Option<Running>> = Mutex::new(None);

/// `RECTO_FUZZ_STATE` is how `tools/fuzz.sh` says where, so it can clear up after a run that ends by being stopped.
fn chosen_folder() -> Option<PathBuf> {
    std::env::var_os("RECTO_FUZZ_STATE").map(PathBuf::from)
}

fn start() -> Running {
    let folder = chosen_folder()
        .is_none()
        .then(|| recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()), "rf"));
    let root = chosen_folder().unwrap_or_else(|| folder.as_ref().expect("made when none is chosen").to_path_buf());
    let paths = StatePaths::overridden(&root).expect("a state folder in the temp folder");
    paths.prepare().expect("the state folder");
    levels::store(&paths, Level::Control).expect("the level file");
    let server = Server::bind(&paths).expect("the socket");
    let stopper = server.stopper().expect("a stopper");
    let host = Arc::new(Guarded(SessionHost::new(NamedLoader)));
    let hosted = Arc::clone(&host);
    let thread = std::thread::spawn(move || server.run(hosted, Limits::default()));
    Running {
        paths,
        host,
        stopper,
        thread,
        _folder: folder,
    }
}

/// Ends the server and removes its folder. A smoke run calls this. A libFuzzer run ends by being stopped, and `tools/fuzz.sh` clears up after it.
pub fn shut_down() {
    let running = SERVER.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take();
    if let Some(running) = running {
        running.stopper.stop();
        let _ = running.thread.join();
    }
    if let Some(chosen) = chosen_folder() {
        let _ = std::fs::remove_dir_all(chosen);
    }
}

/// Catches: a panic in a job, a line that stops the server answering, a client that keeps the next one out, or an answer that isn't a line of JSON.
pub fn check(data: &[u8]) {
    within("a client's bytes", BUDGET, || {
        let mut guard = SERVER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.as_ref().is_none_or(|running| running.thread.is_finished()) {
            // It was told to `stop`, which it may be, and the next one starts in its place.
            if let Some(old) = guard.take() {
                let _ = old.thread.join();
            }
            *guard = Some(start());
        }
        let running = guard.as_ref().expect("just started");
        talk(running, data);
        if !answers_status(running) {
            must_be_ending(running);
        }
        // What this input opened goes, so one input's tabs aren't the next one's.
        running.host.0.with_session(|session| {
            let ids: Vec<TabId> = session.tabs().iter().map(|tab| tab.id).collect();
            for id in ids {
                session.discard_edits(id);
                let _ = session.close(id);
            }
        });
    });
}

/// A `stop` a moment ago is the one reason the next client mightn't get in, and then the server ends, within its grace. It can have said yes to one client and be gone for the next.
fn must_be_ending(running: &Running) {
    let started = Instant::now();
    while !running.thread.is_finished() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "a new client couldn't get in, and the session hasn't ended"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn connect(running: &Running) -> Option<Stream> {
    let stream = Stream::connect(&running.paths.socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(20))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(20))).ok()?;
    Some(stream)
}

fn answers_status(running: &Running) -> bool {
    let Some(stream) = connect(running) else { return false };
    if (&stream).write_all(STATUS).is_err() {
        return false;
    }
    let mut buffer = [0u8; 4096];
    matches!((&stream).read(&mut buffer), Ok(count) if count > 0)
}

/// Whether a line asks to be left waiting. A client that does that and has more to say gets its next line read when the wait ends, which is its own affair: it holds its own slot, and nobody else's.
fn asks_to_wait(bytes: &[u8]) -> bool {
    bytes.split(|byte| *byte == b'\n').any(|line| {
        std::str::from_utf8(line.trim_ascii())
            .ok()
            .and_then(|line| parse(line).ok())
            .is_some_and(|command| {
                matches!(
                    command,
                    Command::Waits(Waits::Wait(_) | Waits::Watch(_)) | Command::Rights(Rights::PermissionRequest(_))
                )
            })
    })
}

/// Sends the bytes, in pieces the input chooses, and a `status` behind them, and reads until that's answered or the connection ends.
fn talk(running: &Running, data: &[u8]) {
    let mut take = Take(data);
    let piece = take.pick(&[usize::MAX, usize::MAX, usize::MAX, 1, 7, 4096]);
    let bytes = take.rest();
    let bytes = &bytes[..bytes.len().min(MAX_INPUT)];
    let Some(stream) = connect(running) else {
        must_be_ending(running);
        return;
    };
    let waits = asks_to_wait(bytes);
    if waits {
        // A moment is enough to see what it says. Then this client leaves, which ends a wait, and the check goes on to the next client.
        // On macOS that's `EINVAL` once the server has closed this connection, which is its right: then nothing waits anyway.
        for set in [
            stream.set_read_timeout(Some(Duration::from_millis(20))),
            stream.set_write_timeout(Some(Duration::from_secs(1))),
        ] {
            if let Err(error) = set {
                assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "a timeout: {error}");
            }
        }
    }
    let writer = stream.try_clone().expect("a descriptor for the writer");
    std::thread::scope(|scope| {
        scope.spawn(move || {
            // A client's write can fail because the server has closed on it, which is the server's right.
            for part in bytes.chunks(piece.min(bytes.len().max(1))) {
                if (&writer).write_all(part).is_err() {
                    return;
                }
            }
            let _ = (&writer).write_all(STATUS);
        });
        read_answers(&stream, running, waits);
    });
}

fn read_answers(stream: &Stream, running: &Running, may_stay_silent: bool) {
    let started = Instant::now();
    let mut seen = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let mut reader = stream;
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => seen.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                // Silent for the whole timeout, with a `status` waiting, and nothing asked it to wait.
                assert!(may_stay_silent, "the session said nothing for 20 s");
                return;
            }
            Err(_) => break,
        }
        let mut done = false;
        while let Some(end) = seen.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = seen.drain(..=end).collect();
            done |= assert_answer_holds(&line);
        }
        if done {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(45) || running.thread.is_finished(),
            "still reading answers after {:?}",
            started.elapsed()
        );
    }
    // The connection ended. Whatever came before that was still whole lines, or nothing.
    assert!(seen.is_empty(), "half an answer: {} bytes", seen.len());
}

/// Whether this was the answer to the `status` behind everything.
fn assert_answer_holds(line: &[u8]) -> bool {
    let value: Value = serde_json::from_slice(line).unwrap_or_else(|_| panic!("not a line of JSON: {:?}", String::from_utf8_lossy(line)));
    assert!(value.is_object(), "not an object: {value}");
    if value["ok"] == Value::Bool(false) {
        let code = value["error"]["code"].as_str().or_else(|| value["code"].as_str()).unwrap_or("");
        assert_ne!(code, "internal", "a command panicked: {value}");
    }
    value["cmd"] == "status"
}
