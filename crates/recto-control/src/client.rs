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
    /// How much of `buffer` is known to hold no line end.
    searched: usize,
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
        Ok(Self { stream, buffer: Vec::new(), searched: 0 })
    }

    /// Sends one command, as the line the socket reads.
    pub fn send(&mut self, line: &str, wait: Duration) -> Result<(), AgentError> {
        // Never decides anything, for the reason in `read`: a session that's gone shows at the write, which says so.
        let _ = self.stream.set_write_timeout(Some(wait));
        let mut framed = String::with_capacity(line.len() + 1);
        framed.push_str(line);
        framed.push('\n');
        (&self.stream).write_all(framed.as_bytes()).map_err(|error| if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) { no_answer(wait) } else { ended() })
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

/// Where the next line ends, looking only at what hasn't been looked at. A large answer comes a few kilobytes at a time, and looking from the start after each would make its cost grow with its square. The caller takes the line out of `buffer` when one's found.
fn line_end(buffer: &[u8], searched: &mut usize) -> Option<usize> {
    let found = buffer[*searched..].iter().position(|byte| *byte == b'\n').map(|at| at + *searched);
    *searched = if found.is_some() { 0 } else { buffer.len() };
    found
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
        let mut client = Client { stream: ours, buffer: Vec::new(), searched: 0 };
        let read = client.read(Duration::from_secs(5)).unwrap();
        assert_eq!(read["ok"], true);
        assert_eq!(client.read(Duration::from_secs(5)).unwrap_err().code, "session_ended", "and after it, the truth");
        assert_eq!(client.send("{}", Duration::from_secs(5)).unwrap_err().code, "session_ended", "a closed socket doesn't take a command");
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
