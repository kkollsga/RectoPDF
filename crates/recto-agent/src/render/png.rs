//! A small PNG writer: streams RGB rows through zlib into IDAT chunks, so a large page never needs a whole-page buffer. Uses `flate2`,
//! already in the tree through lopdf. No filter on rows, because QA pictures favour speed and this is what `render` writes.

use flate2::{Compression, Crc, write::ZlibEncoder};
use std::fs::File;
use std::io::{self, BufWriter, Write};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// How much compressed data a single IDAT chunk holds.
const CHUNK: usize = 64 * 1024;

fn write_chunk(out: &mut impl Write, kind: &[u8; 4], data: &[u8]) -> io::Result<()> {
    out.write_all(&u32::try_from(data.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?.to_be_bytes())?;
    out.write_all(kind)?;
    out.write_all(data)?;
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.write_all(&crc.sum().to_be_bytes())
}

/// Turns each write into one IDAT chunk, stopping at the cap with `FileTooLarge`.
struct IdatChunks {
    file: BufWriter<File>,
    left: u64,
}

impl Write for IdatChunks {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if !data.is_empty() {
            self.left = self.left.checked_sub(data.len() as u64).ok_or(io::ErrorKind::FileTooLarge)?;
            write_chunk(&mut self.file, b"IDAT", data)?;
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// An 8-bit RGB PNG being written.
pub(super) struct RgbWriter {
    encoder: ZlibEncoder<BufWriter<IdatChunks>>,
    row: Vec<u8>,
}

impl RgbWriter {
    /// Starts a picture in a file the caller made, so the caller decides how it's safe to create. Compressed data past `max_bytes` is refused.
    pub(super) fn create(file: File, width: u32, height: u32, max_bytes: u64) -> io::Result<Self> {
        let mut file = BufWriter::new(file);
        file.write_all(&SIGNATURE)?;
        let mut header = [0u8; 13];
        header[..4].copy_from_slice(&width.to_be_bytes());
        header[4..8].copy_from_slice(&height.to_be_bytes());
        header[8] = 8; // Bits per channel.
        header[9] = 2; // RGB.
        write_chunk(&mut file, b"IHDR", &header)?;
        let chunks = BufWriter::with_capacity(CHUNK, IdatChunks { file, left: max_bytes });
        Ok(Self { encoder: ZlibEncoder::new(chunks, Compression::fast()), row: Vec::with_capacity(width as usize * 3 + 1) })
    }

    /// Adds one row of blue, green, red, unused.
    pub(super) fn write_bgrx_row(&mut self, pixels: &[u8]) -> io::Result<()> {
        self.row.clear();
        self.row.push(0); // No filter.
        for px in pixels.as_chunks::<4>().0.iter() {
            self.row.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        self.encoder.write_all(&self.row)
    }

    pub(super) fn finish(self) -> io::Result<()> {
        let chunks = self.encoder.finish()?;
        let IdatChunks { mut file, .. } = chunks.into_inner().map_err(io::IntoInnerError::into_error)?;
        write_chunk(&mut file, b"IEND", &[])?;
        file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    /// Failure: a written picture doesn't decode, or its pixels come out with red and blue swapped.
    #[test]
    fn a_written_png_decodes_to_the_same_pixels() {
        let path = std::env::temp_dir().join(format!("recto-png-test-{}.png", std::process::id()));
        let mut writer = RgbWriter::create(File::create(&path).unwrap(), 2, 2, u64::MAX).unwrap();
        writer.write_bgrx_row(&[1, 2, 3, 0, 4, 5, 6, 0]).unwrap();
        writer.write_bgrx_row(&[7, 8, 9, 0, 10, 11, 12, 0]).unwrap();
        writer.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bytes[..8], SIGNATURE);
        let (mut at, mut idat) = (8, Vec::new());
        let mut saw_end = false;
        while at < bytes.len() {
            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind = &bytes[at + 4..at + 8];
            let data = &bytes[at + 8..at + 8 + len];
            let mut crc = Crc::new();
            crc.update(kind);
            crc.update(data);
            assert_eq!(crc.sum().to_be_bytes(), bytes[at + 8 + len..at + 12 + len], "chunk checksum");
            if kind == b"IHDR" {
                assert_eq!(&data[..8], &[0, 0, 0, 2, 0, 0, 0, 2]);
            }
            if kind == b"IDAT" {
                idat.extend_from_slice(data);
            }
            saw_end |= kind == b"IEND";
            at += 12 + len;
        }
        assert!(saw_end);
        let mut rows = Vec::new();
        ZlibDecoder::new(&idat[..]).read_to_end(&mut rows).unwrap();
        assert_eq!(rows, [0, 3, 2, 1, 6, 5, 4, 0, 9, 8, 7, 12, 11, 10]);
    }
}
