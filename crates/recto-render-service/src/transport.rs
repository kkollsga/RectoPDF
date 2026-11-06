//! Frames over a channel, for both sides. Knows bytes and handles, nothing about jobs.

use recto_platform::isolate::{Channel, Handle, MAX_HANDLES};
use recto_proto::{HEADER_LEN, Header, Kind, ProtoError};
use std::io;

/// A whole frame.
#[derive(Debug)]
pub struct Frame {
    pub kind: Kind,
    pub payload: Vec<u8>,
    pub handles: Vec<Handle>,
}

/// Why no frame is coming.
#[derive(Debug)]
pub enum ReadError {
    /// The other end has gone.
    Closed,
    /// The bytes aren't a frame. From a worker, that's a violation.
    Bad(ProtoError),
    Io(io::Error),
}

/// How much a single read grows a payload. The buffer follows the data, so a large claimed length costs nothing until bytes arrive.
const READ_STEP: usize = 64 * 1024;

/// Assembles frames from a stream that delivers them in any pieces. Reads no further than the current frame's end, so handles stay with
/// their frame.
#[derive(Debug, Default)]
pub struct FrameReader {
    header: [u8; HEADER_LEN],
    header_filled: usize,
    expecting: Option<Header>,
    payload: Vec<u8>,
    filled: usize,
    handles: Vec<Handle>,
}

impl FrameReader {
    /// Reads once. Returns a frame if that completed one, `None` if more is needed.
    /// On a non-blocking channel a read with nothing ready is `None` too.
    pub fn read(&mut self, channel: &Channel) -> Result<Option<Frame>, ReadError> {
        let Some(header) = self.expecting else {
            let got = recv(channel, &mut self.header[self.header_filled..], &mut self.handles)?;
            let Some(got) = got else { return Ok(None) };
            self.header_filled += got;
            if self.header_filled < HEADER_LEN {
                return Ok(None);
            }
            let header = Header::decode(&self.header).map_err(ReadError::Bad)?;
            self.header_filled = 0;
            self.expecting = Some(header);
            self.payload = Vec::new();
            self.filled = 0;
            return if header.len == 0 { Ok(Some(self.finish(header))) } else { Ok(None) };
        };
        let len = header.len as usize;
        let target = len.min(self.filled + READ_STEP);
        if self.payload.len() < target {
            self.payload.resize(target, 0);
        }
        let Some(got) = recv(channel, &mut self.payload[self.filled..target], &mut self.handles)? else { return Ok(None) };
        self.filled += got;
        if self.filled == len { Ok(Some(self.finish(header))) } else { Ok(None) }
    }

    fn finish(&mut self, header: Header) -> Frame {
        self.expecting = None;
        Frame { kind: header.kind, payload: std::mem::take(&mut self.payload), handles: std::mem::take(&mut self.handles) }
    }

    /// Blocks until a whole frame arrives, on a blocking channel.
    pub fn read_blocking(&mut self, channel: &Channel) -> Result<Frame, ReadError> {
        loop {
            if let Some(frame) = self.read(channel)? {
                return Ok(frame);
            }
        }
    }
}

fn recv(channel: &Channel, buf: &mut [u8], handles: &mut Vec<Handle>) -> Result<Option<usize>, ReadError> {
    let received = channel.recv(buf, handles);
    // No frame needs more. Past this they're closed at once, so a worker dripping a long frame with handles on it can't fill the host's table.
    handles.truncate(MAX_HANDLES);
    match received {
        Ok(0) => Err(ReadError::Closed),
        Ok(got) => Ok(Some(got)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(ReadError::Io(error)),
    }
}

/// Writes all of `bytes` on a blocking channel, with `handles` on the first piece.
pub fn send_all(channel: &Channel, mut bytes: &[u8], mut handles: &[&Handle]) -> io::Result<()> {
    while !bytes.is_empty() {
        let sent = channel.send(bytes, handles)?;
        bytes = &bytes[sent..];
        handles = &[];
    }
    Ok(())
}
