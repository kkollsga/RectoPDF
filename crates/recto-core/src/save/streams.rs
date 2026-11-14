//! A stream's stored bytes as a save copies them, a piece at a time: out of `save.rs` unchanged (second review of R4.3), so the save had room to
//! read a rewrite back.

use lopdf::{EncodedStreamReader, IndexedReader, ObjectId};
use std::io::Read;

/// A copied stream's stored bytes, opened at the first read.
pub(super) struct LazyStream<'c> {
    pub(super) source: &'c IndexedReader,
    pub(super) id: ObjectId,
    pub(super) open: Option<EncodedRead>,
}

impl Read for LazyStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.open.is_none() {
            let descriptor = self
                .source
                .resolve_stream_descriptor(self.id)
                .map_err(|_| std::io::Error::other("a stream's bytes couldn't be read"))?;
            self.open = Some(EncodedRead(
                descriptor
                    .open_plain_encoded()
                    .map_err(|_| std::io::Error::other("a stream's bytes couldn't be read"))?,
            ));
        }
        let read = self.open.as_mut().map_or(Ok(0), |open| open.read(buf))?;
        if read == 0 {
            self.open = None;
        }
        Ok(read)
    }
}

/// A stream's stored bytes, read as they are, a piece at a time.
pub(super) struct EncodedRead(pub(super) EncodedStreamReader);

impl Read for EncodedRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read_chunk(buf).map_err(|_| std::io::Error::other("a stream's bytes couldn't be read"))
    }
}
