//! A small baseline JPEG writer, for a snapshot saved as JPG. Rows go in as they're drawn and a block row of eight at a time goes out, so a large snapshot never needs the whole picture in memory: eight rows of three channels.
//!
//! It's lossy by nature, so whatever writes one says it's re-encoded (SPEC, "JPG honesty"): only an image's own bytes are a lossless JPEG, and those never come through here. Full-resolution colour (4:4:4), so text in a snapshot keeps its edges, at quality 90, with the standard tables of ITU T.81 Annex K and the floating-point AAN transform IJG uses.

use std::io::{self, Write};

/// What a snapshot as JPG is written at. High enough that text reads as it did, as the point of a snapshot is what's on the page.
pub(super) const QUALITY: u8 = 90;

/// The longest side a baseline JPEG can say.
const MAX_SIDE: u32 = u16::MAX as u32;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29,
    22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const LUMA_QUANT: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109,
    103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const CHROMA_QUANT: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

const AAN: [f32; 8] = [1.0, 1.387_039_8, 1.306_563, 1.175_875_6, 1.0, 0.785_694_96, 0.541_196_1, 0.275_899_38];

/// Annex K's tables: sixteen counts of codes by length, then the values.
const DC_LUMA: ([u8; 16], &[u8]) = ([0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
const DC_CHROMA: ([u8; 16], &[u8]) = ([0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0], &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
const AC_LUMA: ([u8; 16], &[u8]) = (
    [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d],
    &[
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23,
        0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a,
        0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64,
        0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94,
        0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2,
        0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8,
        0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ],
);
const AC_CHROMA: ([u8; 16], &[u8]) = (
    [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77],
    &[
        0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1,
        0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28,
        0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63,
        0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92,
        0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9,
        0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7,
        0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ],
);

/// A table's codes, by value: (code, length).
struct Huffman {
    codes: [(u16, u8); 256],
}

impl Huffman {
    fn new((counts, values): ([u8; 16], &[u8])) -> Self {
        let mut codes = [(0u16, 0u8); 256];
        let (mut code, mut at) = (0u16, 0usize);
        for (length, count) in counts.iter().enumerate() {
            for _ in 0..*count {
                codes[usize::from(values[at])] = (code, length as u8 + 1);
                code += 1;
                at += 1;
            }
            code <<= 1;
        }
        Self { codes }
    }
}

/// Bits on their way out, with a 0 after every 0xFF as the format needs.
struct Bits<W: Write> {
    out: W,
    buffer: u32,
    count: u32,
    pending: Vec<u8>,
}

impl<W: Write> Bits<W> {
    fn put(&mut self, code: u16, length: u8) {
        self.buffer = (self.buffer << length) | u32::from(code);
        self.count += u32::from(length);
        while self.count >= 8 {
            self.count -= 8;
            let byte = (self.buffer >> self.count) as u8;
            self.pending.push(byte);
            if byte == 0xFF {
                self.pending.push(0);
            }
        }
        self.buffer &= (1 << self.count) - 1;
    }

    fn flush_pending(&mut self) -> io::Result<()> {
        self.out.write_all(&self.pending)?;
        self.pending.clear();
        Ok(())
    }

    /// Pads the last byte with ones, as the format says.
    fn finish(mut self) -> io::Result<W> {
        if self.count > 0 {
            let fill = 8 - self.count;
            self.put((1 << fill) - 1, fill as u8);
        }
        self.flush_pending()?;
        Ok(self.out)
    }
}

/// How many bits a value needs, and its bits as the format codes a negative one.
fn magnitude(value: i32) -> (u8, u16) {
    let size = (32 - value.unsigned_abs().leading_zeros()) as u8;
    let bits = if value < 0 { (value - 1) as u16 & ((1u16 << size) - 1) } else { value as u16 };
    (size, bits)
}

/// The quantiser for one table at a quality, folded with the AAN transform's own scale.
fn divisors(base: &[u16; 64], quality: u8) -> ([u8; 64], [f32; 64]) {
    let quality = u32::from(quality.clamp(1, 100));
    let scale = if quality < 50 { 5000 / quality } else { 200 - quality * 2 };
    let mut table = [0u8; 64];
    let mut divisor = [0f32; 64];
    for at in 0..64 {
        let value = ((u32::from(base[at]) * scale + 50) / 100).clamp(1, 255);
        table[at] = value as u8;
        divisor[at] = value as f32 * AAN[at / 8] * AAN[at % 8] * 8.0;
    }
    (table, divisor)
}

fn fdct(block: &mut [f32; 64]) {
    for pass in 0..2 {
        for line in 0..8 {
            let at = |k: usize| if pass == 0 { line * 8 + k } else { k * 8 + line };
            let d: [f32; 8] = std::array::from_fn(|k| block[at(k)]);
            let (tmp0, tmp7) = (d[0] + d[7], d[0] - d[7]);
            let (tmp1, tmp6) = (d[1] + d[6], d[1] - d[6]);
            let (tmp2, tmp5) = (d[2] + d[5], d[2] - d[5]);
            let (tmp3, tmp4) = (d[3] + d[4], d[3] - d[4]);
            let (tmp10, tmp13, tmp11, tmp12) = (tmp0 + tmp3, tmp0 - tmp3, tmp1 + tmp2, tmp1 - tmp2);
            block[at(0)] = tmp10 + tmp11;
            block[at(4)] = tmp10 - tmp11;
            let z1 = (tmp12 + tmp13) * 0.707_106_77;
            block[at(2)] = tmp13 + z1;
            block[at(6)] = tmp13 - z1;
            let (tmp10, tmp11, tmp12) = (tmp4 + tmp5, tmp5 + tmp6, tmp6 + tmp7);
            let z5 = (tmp10 - tmp12) * 0.382_683_43;
            let z2 = 0.541_196_1 * tmp10 + z5;
            let z4 = 1.306_563 * tmp12 + z5;
            let z3 = tmp11 * 0.707_106_77;
            let (z11, z13) = (tmp7 + z3, tmp7 - z3);
            block[at(5)] = z13 + z2;
            block[at(3)] = z13 - z2;
            block[at(1)] = z11 + z4;
            block[at(7)] = z11 - z4;
        }
    }
}

/// A JPEG in progress: RGB rows in, top to bottom, and the file out a block row at a time.
pub(super) struct JpegWriter<W: Write> {
    bits: Bits<W>,
    width: u32,
    rows_left: u32,
    /// Up to eight rows of Y, Cb and Cr, each as wide as the picture rounded up to a block.
    planes: [Vec<f32>; 3],
    rows_held: u32,
    previous_dc: [i32; 3],
    luma: [f32; 64],
    chroma: [f32; 64],
    dc: [Huffman; 2],
    ac: [Huffman; 2],
}

impl<W: Write> JpegWriter<W> {
    /// Starts a picture, refusing a size a baseline JPEG can't say.
    pub(super) fn new(mut out: W, width: u32, height: u32, quality: u8) -> io::Result<Self> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a JPEG can't be that size"));
        }
        let (luma_table, luma) = divisors(&LUMA_QUANT, quality);
        let (chroma_table, chroma) = divisors(&CHROMA_QUANT, quality);
        let mut head = vec![0xFF, 0xD8];
        // JFIF, so every reader takes the channels as YCbCr.
        head.extend_from_slice(&[0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0]);
        for (id, table) in [(0u8, luma_table), (1, chroma_table)] {
            head.extend_from_slice(&[0xFF, 0xDB, 0, 67, id]);
            head.extend(ZIGZAG.iter().map(|&at| table[at]));
        }
        head.extend_from_slice(&[0xFF, 0xC0, 0, 17, 8]);
        head.extend_from_slice(&(height as u16).to_be_bytes());
        head.extend_from_slice(&(width as u16).to_be_bytes());
        head.extend_from_slice(&[3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
        for (class_id, (counts, values)) in [(0x00u8, DC_LUMA), (0x10, AC_LUMA), (0x01, DC_CHROMA), (0x11, AC_CHROMA)] {
            let length = 2 + 1 + 16 + values.len();
            head.extend_from_slice(&[0xFF, 0xC4]);
            head.extend_from_slice(&(length as u16).to_be_bytes());
            head.push(class_id);
            head.extend_from_slice(&counts);
            head.extend_from_slice(values);
        }
        head.extend_from_slice(&[0xFF, 0xDA, 0, 12, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
        out.write_all(&head)?;
        let padded = width.div_ceil(8) as usize * 8;
        Ok(Self {
            bits: Bits {
                out,
                buffer: 0,
                count: 0,
                pending: Vec::with_capacity(4096),
            },
            width,
            rows_left: height,
            planes: std::array::from_fn(|_| vec![0.0; padded * 8]),
            rows_held: 0,
            previous_dc: [0; 3],
            luma,
            chroma,
            dc: [Huffman::new(DC_LUMA), Huffman::new(DC_CHROMA)],
            ac: [Huffman::new(AC_LUMA), Huffman::new(AC_CHROMA)],
        })
    }

    /// One row of RGB, as wide as the picture.
    pub(super) fn write_rgb_row(&mut self, row: &[u8]) -> io::Result<()> {
        if self.rows_left == 0 || row.len() != self.width as usize * 3 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "a row that isn't part of this picture"));
        }
        let padded = self.planes[0].len() / 8;
        let base = self.rows_held as usize * padded;
        for (x, rgb) in row.as_chunks::<3>().0.iter().enumerate() {
            let (r, g, b) = (f32::from(rgb[0]), f32::from(rgb[1]), f32::from(rgb[2]));
            self.planes[0][base + x] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
            self.planes[1][base + x] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
            self.planes[2][base + x] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
        }
        // Past the right edge, the last pixel again, so the edge doesn't ring.
        for plane in &mut self.planes {
            let last = plane[base + row.len() / 3 - 1];
            plane[base + row.len() / 3..base + padded].fill(last);
        }
        self.rows_held += 1;
        self.rows_left -= 1;
        if self.rows_held == 8 || self.rows_left == 0 {
            self.encode_block_row()?;
        }
        Ok(())
    }

    fn encode_block_row(&mut self) -> io::Result<()> {
        let padded = self.planes[0].len() / 8;
        // Below the bottom edge, the last row again.
        for row in self.rows_held as usize..8 {
            for plane in &mut self.planes {
                let (above, below) = plane.split_at_mut(row * padded);
                below[..padded].copy_from_slice(&above[(self.rows_held as usize - 1) * padded..self.rows_held as usize * padded]);
            }
        }
        for column in 0..padded / 8 {
            for component in 0..3 {
                let mut block = [0f32; 64];
                for y in 0..8 {
                    block[y * 8..y * 8 + 8].copy_from_slice(&self.planes[component][y * padded + column * 8..y * padded + column * 8 + 8]);
                }
                self.encode_block(component, &mut block);
            }
            if self.bits.pending.len() >= 4096 {
                self.bits.flush_pending()?;
            }
        }
        self.rows_held = 0;
        Ok(())
    }

    fn encode_block(&mut self, component: usize, block: &mut [f32; 64]) {
        fdct(block);
        let table = usize::from(component > 0);
        let divisor = if table == 0 { &self.luma } else { &self.chroma };
        let quantised: [i32; 64] = std::array::from_fn(|k| (block[ZIGZAG[k]] / divisor[ZIGZAG[k]]).round() as i32);
        let difference = quantised[0] - self.previous_dc[component];
        self.previous_dc[component] = quantised[0];
        let (size, bits) = magnitude(difference);
        let (code, length) = self.dc[table].codes[usize::from(size)];
        self.bits.put(code, length);
        if size > 0 {
            self.bits.put(bits, size);
        }
        let mut zeros = 0u8;
        for &value in &quantised[1..] {
            if value == 0 {
                zeros += 1;
                continue;
            }
            while zeros >= 16 {
                let (code, length) = self.ac[table].codes[0xF0];
                self.bits.put(code, length);
                zeros -= 16;
            }
            let (size, bits) = magnitude(value.clamp(-1023, 1023));
            let (code, length) = self.ac[table].codes[usize::from(zeros << 4 | size)];
            self.bits.put(code, length);
            self.bits.put(bits, size);
            zeros = 0;
        }
        if zeros > 0 {
            let (code, length) = self.ac[table].codes[0x00];
            self.bits.put(code, length);
        }
    }

    /// Ends the file. It's an error if rows are missing, as for a PNG.
    pub(super) fn finish(self) -> io::Result<W> {
        if self.rows_left != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "a picture with rows missing"));
        }
        let mut out = self.bits.finish()?;
        out.write_all(&[0xFF, 0xD9])?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_proto::jpeg::JpegScan;

    fn encode(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut writer = JpegWriter::new(Vec::new(), width, height, QUALITY).unwrap();
        for y in 0..height {
            let row: Vec<u8> = (0..width).flat_map(|x| pixel(x, y)).collect();
            writer.write_rgb_row(&row).unwrap();
        }
        writer.finish().unwrap()
    }

    /// Failure: what's written isn't a sound JPEG: a marker out of place, a 0xFF not followed by a 0 in the picture, or the wrong size in its header, so a reader refuses it or shows something else. Checked by the same parser that checks a worker's JPEGs, over sizes that aren't whole blocks.
    #[test]
    fn what_comes_out_is_a_sound_jpeg_of_the_right_size() {
        for (width, height) in [(1, 1), (8, 8), (13, 7), (64, 33), (300, 9)] {
            let bytes = encode(width, height, |x, y| [(x * 7) as u8, (y * 13) as u8, ((x ^ y) * 29) as u8]);
            let mut scan = JpegScan::new(false);
            scan.feed(&bytes, &mut Vec::new()).unwrap();
            scan.finish().unwrap();
            let sof = bytes.windows(2).position(|pair| pair == [0xFF, 0xC0]).unwrap();
            assert_eq!(u16::from_be_bytes([bytes[sof + 5], bytes[sof + 6]]), height as u16);
            assert_eq!(u16::from_be_bytes([bytes[sof + 7], bytes[sof + 8]]), width as u16);
            assert!(bytes.ends_with(&[0xFF, 0xD9]));
        }
    }

    /// Failure: rows short or too many make a file that says one size and holds another.
    #[test]
    fn a_wrong_number_of_rows_is_refused() {
        let mut writer = JpegWriter::new(Vec::new(), 4, 2, QUALITY).unwrap();
        writer.write_rgb_row(&[0; 12]).unwrap();
        assert!(writer.write_rgb_row(&[0; 9]).is_err(), "a row of the wrong width");
        let short = JpegWriter::new(Vec::new(), 4, 2, QUALITY).unwrap();
        assert!(short.finish().is_err());
        assert!(JpegWriter::new(Vec::new(), 70_000, 1, QUALITY).is_err());
        assert!(JpegWriter::new(Vec::new(), 0, 1, QUALITY).is_err());
    }

    /// Failure: a negative coefficient is coded wrong, which shows as stripes in every block with an edge in it. The format codes it as its ones' complement in the bits its size needs.
    #[test]
    fn values_are_coded_as_the_format_says() {
        assert_eq!(magnitude(0), (0, 0));
        assert_eq!(magnitude(1), (1, 1));
        assert_eq!(magnitude(-1), (1, 0));
        assert_eq!(magnitude(5), (3, 5));
        assert_eq!(magnitude(-5), (3, 2));
        assert_eq!(magnitude(-1023), (10, 0));
    }
}
