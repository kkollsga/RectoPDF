//! The thin side: connects, sends one JSON line, reads the answer. It checks the other end as carefully as the server checks it.

use crate::state::StatePaths;
use recto_agent::{AgentError, VERSION, exit};
use recto_platform::local::{self, SocketFile, Stream};
use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::time::Duration;

/// The largest answer a client will hold. A page's text is far less, and the server caps its own.
const MAX_REPLY_BYTES: usize = 64 * 1024 * 1024;

/// A connection to the session.
#[derive(Debug)]
pub struct Client {
    stream: Stream,
    buffer: Vec<u8>,
}

pub fn not_running() -> AgentError {
    AgentError::new("not_running", "RectoPDF isn't running.", exit::NOT_RUNNING)
}

impl Client {
    /// Connects. "Not running" covers no socket, a stale socket, and no folder at all. A socket that's there but isn't this user's is a refusal, because something may be pretending.
    pub fn connect(paths: &StatePaths) -> Result<Self, AgentError> {
        let refused = || AgentError::new("socket_refused", "The session's socket isn't private to this user, so it won't be used.", exit::PERMISSION).then("Tell the user: something else may have made that socket or its folder.");
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
        Ok(Self { stream, buffer: Vec::new() })
    }

    /// Sends one command, as the line the socket reads.
    pub fn send(&mut self, line: &str, wait: Duration) -> Result<(), AgentError> {
        self.stream.set_write_timeout(Some(wait)).map_err(|_| ended())?;
        let mut framed = String::with_capacity(line.len() + 1);
        framed.push_str(line);
        framed.push('\n');
        (&self.stream).write_all(framed.as_bytes()).map_err(|error| if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) { no_answer(wait) } else { ended() })
    }

    /// Reads one line of answer, and checks it speaks this version.
    pub fn read(&mut self, wait: Duration) -> Result<Value, AgentError> {
        self.stream.set_read_timeout(Some(wait.max(Duration::from_millis(1)))).map_err(|_| ended())?;
        loop {
            if let Some(at) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let value: Value = serde_json::from_slice(&self.buffer[..at]).map_err(|_| bad_answer())?;
                self.buffer.drain(..=at);
                if value["v"] != VERSION {
                    return Err(AgentError::new("wrong_version", "The running session speaks a different version of this interface.", exit::FAILED));
                }
                return Ok(value);
            }
            if self.buffer.len() > MAX_REPLY_BYTES {
                return Err(bad_answer());
            }
            let mut chunk = [0u8; 16 * 1024];
            match (&self.stream).read(&mut chunk) {
                Ok(0) => return Err(ended()),
                Ok(read) => self.buffer.extend_from_slice(&chunk[..read]),
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Err(no_answer(wait)),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return Err(ended()),
            }
        }
    }

    /// Sends a command and waits for its one answer.
    pub fn call(&mut self, line: &str, wait: Duration) -> Result<Value, AgentError> {
        self.send(line, wait)?;
        self.read(wait)
    }
}

fn ended() -> AgentError {
    AgentError::new("session_ended", "The session ended before it answered.", exit::NOT_RUNNING).then("Start it again with `recto serve --background`. Its tabs and unsaved edits are gone, and `recto status` will say what's there.")
}

fn no_answer(wait: Duration) -> AgentError {
    AgentError::new("no_answer", format!("The session didn't answer within {} s.", wait.as_secs_f32().round()), exit::TIMED_OUT).then("It may be busy with a slow file. `recto status` shows whether it's alive.")
}

fn bad_answer() -> AgentError {
    AgentError::new("bad_answer", "The session's answer couldn't be read.", exit::FAILED)
}
