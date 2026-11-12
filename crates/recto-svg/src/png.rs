//! A PNG written as rows arrive, and base64 written as bytes arrive, so neither holds an image. One row, one compressor, three bytes.

use crate::PngChannels;
use flate2::{Compression, Crc, write::ZlibEncoder};
use std::io::{self, Write};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// How much compressed data one IDAT chunk holds.
const CHUNK: usize = 64 * 1024;
/// PNG's `Up` filter: each byte less the one above it. Cheap, needs one row of memory, and what's above is usually the best guess in a scan or a map.
const FILTER_UP: u8 = 2;
/// The longest row this will hold. It holds three at a time, so this is what it costs at most, whatever a caller says the size is.
const MAX_ROW: u64 = 64 * 1024 * 1024;
/// Compression. Lossless either way: this trades size for time. 6 is zlib's own default; see `docs/engineering/crates/recto-svg.md` for what it costs.
const LEVEL: u32 = 3;

fn write_chunk(out: &mut impl Write, kind: &[u8; 4], data: &[u8]) -> io::Result<()> {
    out.write_all(
        &u32::try_from(data.len())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?
            .to_be_bytes(),
    )?;
    out.write_all(kind)?;
    out.write_all(data)?;
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.write_all(&crc.sum().to_be_bytes())
}

/// Turns each write into IDAT chunks of at most [`CHUNK`].
struct Idat<W: Write> {
    out: W,
    pending: Vec<u8>,
}

impl<W: Write> Write for Idat<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(data);
        while self.pending.len() >= CHUNK {
            write_chunk(&mut self.out, b"IDAT", &self.pending[..CHUNK])?;
            self.pending.drain(..CHUNK);
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A PNG in progress: bytes of rows go in, and a valid file comes out of the other side as they arrive.
pub struct PngWriter<W: Write> {
    encoder: ZlibEncoder<Idat<W>>,
    channels: usize,
    width: u32,
    rows_left: u64,
    row: Vec<u8>,
    above: Vec<u8>,
    filtered: Vec<u8>,
    filled: usize,
}

impl<W: Write> PngWriter<W> {
    /// Starts a picture, refusing a size PNG can't say.
    pub fn new(mut out: W, width: u32, height: u32, channels: PngChannels) -> io::Result<Self> {
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 || u64::from(width) * channels as u64 > MAX_ROW {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "no such picture"));
        }
        out.write_all(&SIGNATURE)?;
        let mut header = [0u8; 13];
        header[..4].copy_from_slice(&width.to_be_bytes());
        header[4..8].copy_from_slice(&height.to_be_bytes());
        header[8] = 8;
        header[9] = match channels {
            PngChannels::Grey => 0,
            PngChannels::Rgb => 2,
            PngChannels::Rgba => 6,
        };
        write_chunk(&mut out, b"IHDR", &header)?;
        let channels = channels as usize;
        let row_len = width as usize * channels;
        Ok(Self {
            encoder: ZlibEncoder::new(
                Idat {
                    out,
                    pending: Vec::with_capacity(CHUNK + 8192),
                },
                Compression::new(LEVEL),
            ),
            channels,
            width,
            rows_left: u64::from(height),
            row: vec![0; row_len],
            above: vec![0; row_len],
            filtered: vec![0; row_len + 1],
            filled: 0,
        })
    }

    fn end_row(&mut self) -> io::Result<()> {
        self.filtered[0] = FILTER_UP;
        for ((out, now), above) in self.filtered[1..].iter_mut().zip(&self.row).zip(&self.above) {
            *out = now.wrapping_sub(*above);
        }
        self.encoder.write_all(&self.filtered)?;
        std::mem::swap(&mut self.row, &mut self.above);
        self.filled = 0;
        self.rows_left -= 1;
        Ok(())
    }

    /// Ends the file. It's an error if rows are missing, because that wouldn't be a picture of the stated size.
    pub fn finish(self) -> io::Result<W> {
        if self.rows_left != 0 || self.filled != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a picture with rows missing"));
        }
        let mut idat = self.encoder.finish()?;
        if !idat.pending.is_empty() {
            let pending = std::mem::take(&mut idat.pending);
            write_chunk(&mut idat.out, b"IDAT", &pending)?;
        }
        write_chunk(&mut idat.out, b"IEND", &[])?;
        Ok(idat.out)
    }

    /// The picture's width, for a caller counting rows.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Bytes per pixel.
    pub fn channels(&self) -> usize {
        self.channels
    }
}

impl<W: Write> Write for PngWriter<W> {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let total = data.len();
        while !data.is_empty() {
            if self.rows_left == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "more rows than the picture has"));
            }
            let take = (self.row.len() - self.filled).min(data.len());
            self.row[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == self.row.len() {
                self.end_row()?;
            }
        }
        Ok(total)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 as bytes arrive, holding at most two of them between writes.
pub(crate) struct Base64<W: Write> {
    out: W,
    held: [u8; 2],
    held_len: usize,
    text: Vec<u8>,
}

impl<W: Write> Base64<W> {
    pub(crate) fn new(out: W) -> Self {
        Self {
            out,
            held: [0; 2],
            held_len: 0,
            text: Vec::with_capacity(8192),
        }
    }

    fn encode_group(text: &mut Vec<u8>, group: [u8; 3]) {
        let [a, b, c] = group;
        text.extend_from_slice(&[
            ALPHABET[usize::from(a >> 2)],
            ALPHABET[usize::from((a & 3) << 4 | b >> 4)],
            ALPHABET[usize::from((b & 15) << 2 | c >> 6)],
            ALPHABET[usize::from(c & 63)],
        ]);
    }

    /// Writes what's left, with its padding, and hands back where it was going.
    pub(crate) fn finish(mut self) -> io::Result<W> {
        match self.held_len {
            1 => {
                let a = self.held[0];
                self.text
                    .extend_from_slice(&[ALPHABET[usize::from(a >> 2)], ALPHABET[usize::from((a & 3) << 4)], b'=', b'=']);
            }
            2 => {
                let (a, b) = (self.held[0], self.held[1]);
                self.text.extend_from_slice(&[
                    ALPHABET[usize::from(a >> 2)],
                    ALPHABET[usize::from((a & 3) << 4 | b >> 4)],
                    ALPHABET[usize::from((b & 15) << 2)],
                    b'=',
                ]);
            }
            _ => {}
        }
        self.out.write_all(&self.text)?;
        Ok(self.out)
    }
}

impl<W: Write> Write for Base64<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let total = data.len();
        let mut rest = data;
        if self.held_len > 0 {
            while self.held_len < 2 && !rest.is_empty() {
                self.held[self.held_len] = rest[0];
                self.held_len += 1;
                rest = &rest[1..];
            }
            if rest.is_empty() {
                return Ok(total);
            }
            Self::encode_group(&mut self.text, [self.held[0], self.held[1], rest[0]]);
            rest = &rest[1..];
            self.held_len = 0;
        }
        let (groups, remainder) = rest.as_chunks::<3>();
        for group in groups {
            Self::encode_group(&mut self.text, *group);
            if self.text.len() >= 8192 - 8 {
                self.out.write_all(&self.text)?;
                self.text.clear();
            }
        }
        for &byte in remainder {
            self.held[self.held_len] = byte;
            self.held_len += 1;
        }
        if !self.text.is_empty() {
            self.out.write_all(&self.text)?;
            self.text.clear();
        }
        Ok(total)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    fn base64(data: &[u8], piece: usize) -> Vec<u8> {
        let mut encoder = Base64::new(Vec::new());
        for part in data.chunks(piece.max(1)) {
            encoder.write_all(part).unwrap();
        }
        encoder.finish().unwrap()
    }

    fn unbase64(text: &[u8]) -> Vec<u8> {
        let value = |b: u8| ALPHABET.iter().position(|&a| a == b).unwrap() as u32;
        let mut out = Vec::new();
        for group in text.chunks(4) {
            let pad = group.iter().filter(|&&b| b == b'=').count();
            let bits = group.iter().filter(|&&b| b != b'=').fold(0u32, |bits, &b| bits << 6 | value(b)) << (6 * pad);
            out.extend_from_slice(&bits.to_be_bytes()[1..4 - pad]);
        }
        out
    }

    /// Failure: bytes come out different depending on where the pieces happened to end, so an image embedded through frames of any size is corrupt.
    #[test]
    fn base64_is_the_same_however_the_bytes_are_cut() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        let whole = base64(&data, data.len());
        assert_eq!(unbase64(&whole), data);
        for piece in [1, 2, 3, 4, 7, 1000, 8191] {
            assert_eq!(base64(&data, piece), whole, "pieces of {piece}");
        }
        for len in 0..7 {
            assert_eq!(unbase64(&base64(&data[..len], 1)), &data[..len], "{len} bytes");
        }
    }

    /// Reads a PNG back by hand, to rows.
    fn decode(file: &[u8]) -> (u32, u32, u8, Vec<u8>) {
        assert_eq!(&file[..8], &SIGNATURE, "the signature");
        let (mut at, mut idat, mut header) = (8, Vec::new(), None);
        let mut ended = false;
        while at < file.len() {
            let len = u32::from_be_bytes(file[at..at + 4].try_into().unwrap()) as usize;
            let kind = &file[at + 4..at + 8];
            let data = &file[at + 8..at + 8 + len];
            let mut crc = Crc::new();
            crc.update(kind);
            crc.update(data);
            assert_eq!(crc.sum().to_be_bytes(), file[at + 8 + len..at + 12 + len], "a chunk's checksum");
            match kind {
                b"IHDR" => header = Some(data.to_vec()),
                b"IDAT" => idat.extend_from_slice(data),
                b"IEND" => ended = true,
                other => panic!("{other:?}"),
            }
            at += 12 + len;
        }
        assert!(ended, "the file must end with IEND");
        let header = header.unwrap();
        let (width, height) = (
            u32::from_be_bytes(header[..4].try_into().unwrap()),
            u32::from_be_bytes(header[4..8].try_into().unwrap()),
        );
        let channels: usize = match header[9] {
            0 => 1,
            2 => 3,
            6 => 4,
            other => panic!("{other}"),
        };
        let mut filtered = Vec::new();
        ZlibDecoder::new(&idat[..]).read_to_end(&mut filtered).unwrap();
        let row_len = width as usize * channels;
        let mut pixels = Vec::new();
        let mut above = vec![0u8; row_len];
        for row in filtered.chunks(row_len + 1) {
            assert_eq!(row[0], FILTER_UP);
            let now: Vec<u8> = row[1..].iter().zip(&above).map(|(value, up)| value.wrapping_add(*up)).collect();
            pixels.extend_from_slice(&now);
            above = now;
        }
        (width, height, header[9], pixels)
    }

    /// Failure: an image isn't lossless after all, or a PNG comes out that no reader takes. Pixels must come back byte for byte, however the rows were cut into pieces.
    #[test]
    fn pixels_come_back_exactly_however_they_arrive() {
        let (width, height) = (37u32, 23u32);
        for (channels, code, count) in [(PngChannels::Grey, 0u8, 1usize), (PngChannels::Rgb, 2, 3), (PngChannels::Rgba, 6, 4)] {
            let pixels: Vec<u8> = (0..width as usize * height as usize * count).map(|i| (i * 31 % 251) as u8).collect();
            for piece in [1, 5, 1000, pixels.len()] {
                let mut png = PngWriter::new(Vec::new(), width, height, channels).unwrap();
                for part in pixels.chunks(piece) {
                    png.write_all(part).unwrap();
                }
                let file = png.finish().unwrap();
                let (w, h, kind, back) = decode(&file);
                assert_eq!((w, h, kind), (width, height, code));
                assert_eq!(back, pixels, "pieces of {piece}");
            }
        }
    }

    /// Failure: a worker sends a row too few or too many, and a broken picture is written instead of a failure.
    #[test]
    fn the_wrong_number_of_rows_is_refused() {
        let mut short = PngWriter::new(Vec::new(), 2, 2, PngChannels::Grey).unwrap();
        short.write_all(&[0; 2]).unwrap();
        assert!(short.finish().is_err(), "a row short");
        let mut partial = PngWriter::new(Vec::new(), 2, 2, PngChannels::Grey).unwrap();
        partial.write_all(&[0; 3]).unwrap();
        assert!(partial.finish().is_err(), "half a row short");
        let mut long = PngWriter::new(Vec::new(), 2, 2, PngChannels::Grey).unwrap();
        assert!(long.write_all(&[0; 5]).is_err(), "a byte over");
        assert!(PngWriter::new(Vec::new(), 0, 2, PngChannels::Grey).is_err(), "no picture has no width");
        assert!(
            PngWriter::new(Vec::new(), i32::MAX as u32, 1, PngChannels::Rgba).is_err(),
            "rows it would have to hold 8 GiB of"
        );
    }
}
