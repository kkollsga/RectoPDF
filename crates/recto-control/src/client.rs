//! The thin side: connects, sends one JSON line, reads the answer. It checks the other end as carefully as the server checks it.

use crate::state::StatePaths;
use recto_agent::Code;
use recto_agent::{AgentError, VERSION, exit};
use recto_platform::local::{self, SocketFile, Stream};
use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

/// The largest answer a client will hold. A page's text is far less, and the server caps its own.
const MAX_REPLY_BYTES: usize = 64 * 1024 * 1024;

/// A connection to the session.
#[derive(Debug)]
pub struct Client {
    stream: Stream,
    buffer: Vec<u8>,
    /// How much of `buffer` is known to hold no line end.
    searched: usize,
    /// The session's socket, asked whether the session is still there when a connection ends with no answer.
    socket: Option<PathBuf>,
}

pub fn not_running() -> AgentError {
    AgentError::new(Code::NOT_RUNNING, "RectoPDF isn't running.", exit::NOT_RUNNING)
}

impl Client {
    /// Connects. "Not running" covers no socket, a stale socket, and no folder at all. A socket that's there but isn't this user's is a refusal, because something may be pretending.
    pub fn connect(paths: &StatePaths) -> Result<Self, AgentError> {
        let refused = || {
            AgentError::new(
                Code::SOCKET_REFUSED,
                "The session's socket isn't private to this user, so it won't be used.",
                exit::PERMISSION,
            )
            .then("Tell the user: something else may have made that socket or its folder.")
        };
        match local::socket_file(&paths.socket) {
            Ok(SocketFile::OwnSocket) => {}
            Ok(SocketFile::Nothing) => return Err(not_running()),
            Err(error) if error.kind() == ErrorKind::PermissionDenied => return Err(refused()),
            Err(_) => return Err(not_running()),
            Ok(SocketFile::Other) => return Err(refused()),
        }
        if local::check_private_dir(&paths.run_dir).is_err() {
            return Err(refused());
        }
        let stream = Stream::connect(&paths.socket).map_err(|_| not_running())?;
        if stream.peer_uid().map_or(true, |uid| uid != local::current_uid()) {
            return Err(refused());
        }
        Ok(Self {
            stream,
            buffer: Vec::new(),
            searched: 0,
            socket: Some(paths.socket.clone()),
        })
    }

    /// Sends one command, as the line the socket reads.
    pub fn send(&mut self, line: &str, wait: Duration) -> Result<(), AgentError> {
        // Never decides anything, for the reason in `read`: a session that's gone shows at the write, which says so.
        let _ = self.stream.set_write_timeout(Some(wait));
        let mut framed = String::with_capacity(line.len() + 1);
        framed.push_str(line);
        framed.push('\n');
        (&self.stream).write_all(framed.as_bytes()).map_err(|error| {
            if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) {
                no_answer(wait)
            } else {
                self.ended()
            }
        })
    }

    /// Reads one line of answer, and checks it speaks this version.
    pub fn read(&mut self, wait: Duration) -> Result<Value, AgentError> {
        // macOS refuses this once the other end has closed, and an answer can be waiting by then: `stop` answers and closes. So the read below decides, and a closed socket can't leave it waiting.
        let _ = self.stream.set_read_timeout(Some(wait.max(Duration::from_millis(1))));
        loop {
            if let Some(at) = line_end(&self.buffer, &mut self.searched) {
                let value: Value = serde_json::from_slice(&self.buffer[..at]).map_err(|_| bad_answer())?;
                self.buffer.drain(..=at);
                if value["v"] != VERSION {
                    return Err(AgentError::new(
                        Code::WRONG_VERSION,
                        "The running session speaks a different version of this interface.",
                        exit::FAILED,
                    ));
                }
                return Ok(value);
            }
            if self.buffer.len() > MAX_REPLY_BYTES {
                return Err(bad_answer());
            }
            let mut chunk = [0u8; 16 * 1024];
            match (&self.stream).read(&mut chunk) {
                Ok(0) => return Err(self.ended()),
                Ok(read) => self.buffer.extend_from_slice(&chunk[..read]),
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Err(no_answer(wait)),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(self.ended()),
            }
        }
    }

    /// The connection ended with no answer. Whether the session ended too is asked of its socket: one that's gone takes no connection, and one that closed only this connection still does. Out of descriptors it drops a client as it arrives, and it lets go of the oldest of too many, and neither has lost anything.
    fn ended(&self) -> AgentError {
        if self.socket.as_deref().is_some_and(|socket| Stream::connect(socket).is_ok()) {
            dropped()
        } else {
            ended()
        }
    }

    /// Sends a command and waits for its one answer.
    pub fn call(&mut self, line: &str, wait: Duration) -> Result<Value, AgentError> {
        self.send(line, wait)?;
        self.read(wait)
    }
}

/// Where the next line ends, looking only at what hasn't been looked at. A large answer comes a few kilobytes at a time, and looking from the start after each would make its cost grow with its square. The caller takes the line out of `buffer` when one's found.
fn line_end(buffer: &[u8], searched: &mut usize) -> Option<usize> {
    let found = buffer[*searched..].iter().position(|byte| *byte == b'\n').map(|at| at + *searched);
    *searched = if found.is_some() { 0 } else { buffer.len() };
    found
}

fn ended() -> AgentError {
    AgentError::new(Code::SESSION_ENDED, "The session ended before it answered.", exit::NOT_RUNNING)
        .then("Start it again with `recto serve --background`. Its tabs and unsaved edits are gone, and `recto status` will say what's there.")
}

fn dropped() -> AgentError {
    AgentError::new(Code::CONNECTION_DROPPED, "The session closed this connection without an answer. It's still running.", exit::BUSY)
        .then("It does that when it's out of descriptors or has too many clients, and takes connections again as others close. Check whether the command took effect before trying it again.")
}

fn no_answer(wait: Duration) -> AgentError {
    AgentError::new(
        Code::NO_ANSWER,
        format!("The session didn't answer within {} s.", wait.as_secs_f32().round()),
        exit::TIMED_OUT,
    )
    .then("It may be busy with a slow file. `recto status` shows whether it's alive.")
}

fn bad_answer() -> AgentError {
    AgentError::new(Code::BAD_ANSWER, "The session's answer couldn't be read.", exit::FAILED)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the session answers and closes, as `stop` does, before the client gets to read. macOS then refuses to set the socket's timeout, and the client says the session ended before it answered, with the answer sitting there unread.
    #[test]
    fn an_answer_is_read_even_after_the_session_has_closed() {
        let (ours, theirs) = Stream::pair().unwrap();
        let answer = format!("{}\n", serde_json::json!({ "v": VERSION, "ok": true }));
        (&theirs).write_all(answer.as_bytes()).unwrap();
        drop(theirs);
        let mut client = Client {
            stream: ours,
            buffer: Vec::new(),
            searched: 0,
            socket: None,
        };
        let read = client.read(Duration::from_secs(5)).unwrap();
        assert_eq!(read["ok"], true);
        assert_eq!(
            client.read(Duration::from_secs(5)).unwrap_err().code,
            "session_ended",
            "and after it, the truth"
        );
        assert_eq!(
            client.send("{}", Duration::from_secs(5)).unwrap_err().code,
            "session_ended",
            "a closed socket doesn't take a command"
        );
    }

    /// Failure: a session that only closed this connection, out of descriptors or letting go of the oldest of too many clients, is reported as ended, with "its tabs and
    /// unsaved edits are gone" and an instruction to start another. It's still running and has lost nothing. Once its socket takes no connection, it has ended.
    #[test]
    fn a_dropped_connection_isnt_a_session_that_ended() {
        let socket = std::env::temp_dir().join(format!("recto-drop-{}", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = local::Listener::bind(&socket).unwrap();
        let connect = || Client {
            stream: Stream::connect(&socket).unwrap(),
            buffer: Vec::new(),
            searched: 0,
            socket: Some(socket.clone()),
        };
        // The client's own look at the socket leaves a connection behind it, already closed, which macOS can refuse to finish accepting. None is waiting before each client connects.
        let clear = || while !matches!(listener.accept(), Ok(None)) {};
        let drop_next = || {
            let started = std::time::Instant::now();
            loop {
                if let Ok(Some(accepted)) = listener.accept() {
                    break drop(accepted);
                }
                assert!(started.elapsed() < Duration::from_secs(5), "the client's connection never arrived");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let mut reading = connect();
        drop_next();
        let error = reading.read(Duration::from_secs(5)).unwrap_err();
        assert_eq!((error.code.as_str(), error.exit), ("connection_dropped", exit::BUSY), "{}", error.message);
        clear();
        let mut writing = connect();
        drop_next();
        assert_eq!(writing.send("{}", Duration::from_secs(5)).unwrap_err().code, "connection_dropped");
        clear();
        let mut last = connect();
        drop_next();
        drop(listener);
        assert_eq!(
            last.read(Duration::from_secs(5)).unwrap_err().code,
            "session_ended",
            "nothing takes a connection there now"
        );
        let _ = std::fs::remove_file(&socket);
    }

    /// Failure: every read of a large answer looks through all of it again for the line's end, so its cost grows with its square: a 5 MB answer took seconds, and on a slower machine that's enough to look like a client that's stopped. And looking only at what's new mustn't miss a line, or find one twice.
    #[test]
    fn finding_a_lines_end_looks_at_each_byte_once() {
        let (mut buffer, mut searched) = (Vec::new(), 0);
        for _ in 0..100 {
            buffer.extend_from_slice(&[b'x'; 8192]);
            assert_eq!(line_end(&buffer, &mut searched), None);
            assert_eq!(searched, buffer.len(), "the next look would start over");
        }
        let end = buffer.len();
        buffer.extend_from_slice(b"\nnext\nlast");
        assert_eq!(line_end(&buffer, &mut searched), Some(end));
        buffer.drain(..=end);
        assert_eq!(line_end(&buffer, &mut searched), Some(4), "a line already here, after the one taken");
        buffer.drain(..=4);
        assert_eq!(line_end(&buffer, &mut searched), None);
        buffer.push(b'\n');
        assert_eq!(line_end(&buffer, &mut searched), Some(4), "the rest of a line that came in two reads");
    }
}
