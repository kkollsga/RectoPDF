//! **The general filters, decoded a piece at a time,** for the page digest ([`super`]): `FlateDecode`, `LZWDecode`, `ASCIIHexDecode`, `ASCII85Decode` and
//! `RunLengthDecode`, with the PNG and TIFF predictors of `/DecodeParms`. Each wraps the reader before it, so a stream is never held whole: what's in memory
//! at once is a few buffers of [`CHUNK`] bytes and a predictor's row, which is capped. Every input is hostile: a decoder that meets what it can't read
//! returns an error, which the digest turns into "hashed as stored", and never panics.

use lopdf::{Dictionary, Object};
use std::io::{self, BufRead, BufReader, Read};

/// How much each stage reads at a time.
pub(super) const CHUNK: usize = 64 * 1024;
/// Longest predictor row taken: a real image row is a few KiB; past this the parameters are refused, so a row can't be made to cost memory.
const MAX_ROW: usize = 1 << 20;
/// Most filters in one stream's chain.
pub(super) const MAX_FILTERS: usize = 8;

/// One general filter, with what its parameters say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Filter {
    Flate(Option<Predictor>),
    Lzw { early_change: bool, predictor: Option<Predictor> },
    AsciiHex,
    Ascii85,
    RunLength,
}

/// A `/DecodeParms` predictor: PNG (10 to 15, each row saying its own) or TIFF 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Predictor {
    png: bool,
    /// Bytes in a row, without the PNG filter byte.
    row: usize,
    /// Bytes per pixel, at least 1: what PNG's filters reach back by.
    pixel: usize,
    /// For TIFF: components a pixel has (only 8-bit components are taken).
    colors: usize,
}

/// Whether a filter's name is one of the general ones, decoded for the digest rather than hashed as stored.
pub(super) fn is_general(name: &[u8]) -> bool {
    matches!(
        name,
        b"FlateDecode" | b"Fl" | b"LZWDecode" | b"LZW" | b"ASCIIHexDecode" | b"AHx" | b"ASCII85Decode" | b"A85" | b"RunLengthDecode" | b"RL"
    )
}

/// A stream's filter names, in order: `None` when `/Filter` is neither a name nor an array of names (so it isn't known which), or the chain is longer
/// than [`MAX_FILTERS`].
pub(super) fn filter_names(dict: &Dictionary, resolve: &dyn Fn(&Object) -> Option<Object>) -> Option<Vec<Vec<u8>>> {
    let Ok(filter) = dict.get(b"Filter") else { return Some(Vec::new()) };
    let names = match resolve(filter)? {
        Object::Name(name) => vec![name],
        Object::Array(items) if items.len() <= MAX_FILTERS => items
            .iter()
            .map(|item| match resolve(item)? {
                Object::Name(name) => Some(name),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    Some(names)
}

/// The chain for a stream whose filters are all general, with each one's parameters: `None` if any isn't general or its parameters can't be taken.
pub(super) fn chain_of(dict: &Dictionary, resolve: &dyn Fn(&Object) -> Option<Object>) -> Option<Vec<Filter>> {
    let names = filter_names(dict, resolve)?;
    let parms: Vec<Option<Dictionary>> = match dict.get(b"DecodeParms").ok().or_else(|| dict.get(b"DP").ok()).map(resolve) {
        None => vec![None; names.len()],
        Some(Some(Object::Dictionary(one))) => {
            let mut all = vec![None; names.len()];
            if let Some(first) = all.first_mut() {
                *first = Some(one);
            }
            if names.len() > 1 {
                // One dictionary for a chain is the first filter's, as readers take it; any other reading isn't knowable.
                return None;
            }
            all
        }
        Some(Some(Object::Array(items))) if items.len() == names.len() => items
            .iter()
            .map(|item| match resolve(item) {
                Some(Object::Dictionary(parms)) => Some(Some(parms)),
                Some(Object::Null) => Some(None),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?,
        Some(Some(Object::Null)) => vec![None; names.len()],
        _ => return None,
    };
    names
        .iter()
        .zip(parms)
        .map(|(name, parms)| {
            let number = |key: &[u8], default: i64| -> Option<i64> {
                match parms.as_ref().and_then(|parms| parms.get(key).ok()).map(resolve) {
                    None => Some(default),
                    Some(Some(Object::Integer(value))) => Some(value),
                    Some(_) => None,
                }
            };
            let predictor = || -> Option<Option<Predictor>> {
                let kind = number(b"Predictor", 1)?;
                if kind == 1 {
                    return Some(None);
                }
                let (colors, bits, columns) = (number(b"Colors", 1)?, number(b"BitsPerComponent", 8)?, number(b"Columns", 1)?);
                if !(1..=32).contains(&colors) || ![1, 2, 4, 8, 16].contains(&bits) || !(1..=(1 << 24)).contains(&columns) {
                    return None;
                }
                let bits_per_pixel = usize::try_from(colors * bits).ok()?;
                let row = (bits_per_pixel.checked_mul(usize::try_from(columns).ok()?)?).div_ceil(8);
                if row == 0 || row > MAX_ROW {
                    return None;
                }
                let colors = usize::try_from(colors).ok()?;
                match kind {
                    2 if bits == 8 => Some(Some(Predictor {
                        png: false,
                        row,
                        pixel: colors,
                        colors,
                    })),
                    10..=15 => Some(Some(Predictor {
                        png: true,
                        row,
                        pixel: bits_per_pixel.div_ceil(8).max(1),
                        colors,
                    })),
                    _ => None,
                }
            };
            Some(match name.as_slice() {
                b"FlateDecode" | b"Fl" => Filter::Flate(predictor()?),
                b"LZWDecode" | b"LZW" => Filter::Lzw {
                    early_change: number(b"EarlyChange", 1)? != 0,
                    predictor: predictor()?,
                },
                b"ASCIIHexDecode" | b"AHx" => Filter::AsciiHex,
                b"ASCII85Decode" | b"A85" => Filter::Ascii85,
                b"RunLengthDecode" | b"RL" => Filter::RunLength,
                _ => return None,
            })
        })
        .collect()
}

/// The decoded bytes of `raw` through `chain`, in order, as a reader.
pub(super) fn decoded<'a>(raw: Box<dyn Read + 'a>, chain: &[Filter]) -> Box<dyn Read + 'a> {
    let mut reader = raw;
    for filter in chain {
        reader = match *filter {
            Filter::Flate(predictor) => predicted(Box::new(flate2::read::ZlibDecoder::new_with_buf(reader, vec![0; CHUNK])), predictor),
            Filter::Lzw { early_change, predictor } => predicted(Box::new(Lzw::new(reader, early_change)), predictor),
            Filter::AsciiHex => Box::new(AsciiHex::new(reader)),
            Filter::Ascii85 => Box::new(Ascii85::new(reader)),
            Filter::RunLength => Box::new(RunLength::new(reader)),
        };
    }
    reader
}

fn predicted<'a>(reader: Box<dyn Read + 'a>, predictor: Option<Predictor>) -> Box<dyn Read + 'a> {
    match predictor {
        None => reader,
        Some(predictor) => Box::new(Predicted::new(reader, predictor)),
    }
}

fn bad(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

/// Copies out what a stage has made and is still to give.
fn give(pending: &mut Vec<u8>, at: &mut usize, buf: &mut [u8]) -> usize {
    let n = (pending.len() - *at).min(buf.len());
    buf[..n].copy_from_slice(&pending[*at..*at + n]);
    *at += n;
    if *at == pending.len() {
        pending.clear();
        *at = 0;
    }
    n
}

/// LZW, MSB first from 8-bit codes, through `weezl`, a chunk at a time.
struct Lzw<'a> {
    input: BufReader<Box<dyn Read + 'a>>,
    decoder: weezl::decode::Decoder,
    done: bool,
}

impl<'a> Lzw<'a> {
    fn new(input: Box<dyn Read + 'a>, early_change: bool) -> Self {
        let decoder = if early_change {
            weezl::decode::Decoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
        } else {
            weezl::decode::Decoder::new(weezl::BitOrder::Msb, 8)
        };
        Self {
            input: BufReader::with_capacity(CHUNK, input),
            decoder,
            done: false,
        }
    }
}

impl Read for Lzw<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while !self.done && !buf.is_empty() {
            let input = self.input.fill_buf()?;
            let result = self.decoder.decode_bytes(input, buf);
            let consumed = result.consumed_in;
            self.input.consume(consumed);
            match result.status {
                Ok(weezl::LzwStatus::Done) => self.done = true,
                // Nothing taken and nothing made: the end, or a decoder that can't go on, which is taken as the end rather than looped on.
                Ok(_) if result.consumed_out == 0 && consumed == 0 => self.done = true,
                Ok(_) => {}
                Err(_) => return Err(bad("an LZW code that can't be")),
            }
            if result.consumed_out > 0 {
                return Ok(result.consumed_out);
            }
        }
        Ok(0)
    }
}

/// Pairs of hex digits, whitespace between them, to `>`; an odd last digit is followed by a 0.
struct AsciiHex<'a> {
    input: BufReader<Box<dyn Read + 'a>>,
    /// The first digit of a pair whose second hasn't come yet.
    high: Option<u8>,
    done: bool,
}

impl<'a> AsciiHex<'a> {
    fn new(input: Box<dyn Read + 'a>) -> Self {
        Self {
            input: BufReader::with_capacity(CHUNK, input),
            high: None,
            done: false,
        }
    }
}

impl Read for AsciiHex<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut out = 0;
        while out < buf.len() && !self.done {
            let input = self.input.fill_buf()?;
            if input.is_empty() {
                self.done = true;
                break;
            }
            let mut used = 0;
            for &byte in input {
                if out == buf.len() {
                    break;
                }
                used += 1;
                let digit = match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    b'>' => {
                        self.done = true;
                        break;
                    }
                    byte if byte.is_ascii_whitespace() || byte == 0 => continue,
                    _ => return Err(bad("not a hex digit")),
                };
                match self.high.take() {
                    None => self.high = Some(digit),
                    Some(first) => {
                        buf[out] = (first << 4) | digit;
                        out += 1;
                    }
                }
            }
            self.input.consume(used);
        }
        if self.done
            && out < buf.len()
            && let Some(first) = self.high.take()
        {
            buf[out] = first << 4;
            out += 1;
        }
        Ok(out)
    }
}

/// Base-85 groups of five, `z` for four zeros, to `~>`.
struct Ascii85<'a> {
    input: BufReader<Box<dyn Read + 'a>>,
    pending: Vec<u8>,
    at: usize,
    group: [u8; 5],
    count: usize,
    done: bool,
}

impl<'a> Ascii85<'a> {
    fn new(input: Box<dyn Read + 'a>) -> Self {
        Self {
            input: BufReader::with_capacity(CHUNK, input),
            pending: Vec::with_capacity(CHUNK),
            at: 0,
            group: [0; 5],
            count: 0,
            done: false,
        }
    }

    fn flush_group(&mut self, last: bool) -> io::Result<()> {
        if self.count == 0 {
            return Ok(());
        }
        if last && self.count == 1 {
            return Err(bad("a base-85 group of one"));
        }
        let mut digits = self.group;
        for digit in &mut digits[self.count..] {
            *digit = 84;
        }
        let mut value: u64 = 0;
        for digit in digits {
            value = value * 85 + u64::from(digit);
        }
        let value = u32::try_from(value).map_err(|_| bad("a base-85 group past 2^32"))?;
        let bytes = value.to_be_bytes();
        self.pending.extend_from_slice(&bytes[..self.count - 1]);
        self.count = 0;
        Ok(())
    }
}

impl Read for Ascii85<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pending.is_empty() && !self.done {
            let input = self.input.fill_buf()?;
            if input.is_empty() {
                self.done = true;
                self.flush_group(true)?;
                break;
            }
            let input = input.to_vec();
            let mut used = 0;
            for byte in input {
                used += 1;
                match byte {
                    b'~' => {
                        self.done = true;
                        self.flush_group(true)?;
                        break;
                    }
                    b'z' if self.count == 0 => self.pending.extend_from_slice(&[0; 4]),
                    b'!'..=b'u' => {
                        self.group[self.count] = byte - b'!';
                        self.count += 1;
                        if self.count == 5 {
                            let group = self.group;
                            let mut value: u64 = 0;
                            for digit in group {
                                value = value * 85 + u64::from(digit);
                            }
                            let value = u32::try_from(value).map_err(|_| bad("a base-85 group past 2^32"))?;
                            self.pending.extend_from_slice(&value.to_be_bytes());
                            self.count = 0;
                        }
                    }
                    byte if byte.is_ascii_whitespace() || byte == 0 => {}
                    _ => return Err(bad("not a base-85 digit")),
                }
                if self.pending.len() >= CHUNK {
                    break;
                }
            }
            self.input.consume(used);
        }
        Ok(give(&mut self.pending, &mut self.at, buf))
    }
}

/// A length byte, then that many plus one bytes as they are (0 to 127), or the next byte 257 minus it times (129 to 255); 128 ends it.
struct RunLength<'a> {
    input: BufReader<Box<dyn Read + 'a>>,
    pending: Vec<u8>,
    at: usize,
    done: bool,
}

impl<'a> RunLength<'a> {
    fn new(input: Box<dyn Read + 'a>) -> Self {
        Self {
            input: BufReader::with_capacity(CHUNK, input),
            pending: Vec::with_capacity(256),
            at: 0,
            done: false,
        }
    }

    fn byte(&mut self) -> io::Result<Option<u8>> {
        let mut one = [0u8];
        Ok((self.input.read(&mut one)? == 1).then_some(one[0]))
    }
}

impl Read for RunLength<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() && !self.done {
            match self.byte()? {
                None | Some(128) => self.done = true,
                Some(length @ 0..=127) => {
                    let mut run = vec![0u8; usize::from(length) + 1];
                    self.input.read_exact(&mut run).map_err(|_| bad("a run cut short"))?;
                    self.pending = run;
                }
                Some(length) => {
                    let value = self.byte()?.ok_or_else(|| bad("a run cut short"))?;
                    self.pending = vec![value; 257 - usize::from(length)];
                }
            }
        }
        Ok(give(&mut self.pending, &mut self.at, buf))
    }
}

/// Rows undone by their predictor. A short last row is left out, as it's no whole row.
struct Predicted<'a> {
    input: Box<dyn Read + 'a>,
    predictor: Predictor,
    previous: Vec<u8>,
    row: Vec<u8>,
    pending: Vec<u8>,
    at: usize,
    done: bool,
}

impl<'a> Predicted<'a> {
    fn new(input: Box<dyn Read + 'a>, predictor: Predictor) -> Self {
        Self {
            input,
            predictor,
            previous: vec![0; predictor.row],
            row: vec![0; predictor.row + usize::from(predictor.png)],
            pending: Vec::new(),
            at: 0,
            done: false,
        }
    }

    /// One whole row in, or `false` at the end.
    fn fill_row(&mut self) -> io::Result<bool> {
        let mut filled = 0;
        while filled < self.row.len() {
            let read = self.input.read(&mut self.row[filled..])?;
            if read == 0 {
                return Ok(false);
            }
            filled += read;
        }
        Ok(true)
    }
}

impl Read for Predicted<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() && !self.done {
            if !self.fill_row()? {
                self.done = true;
                return Ok(0);
            }
            let Predictor { png, pixel, colors, .. } = self.predictor;
            if png {
                let (kind, current) = self.row.split_first_mut().expect("a PNG row has its filter byte");
                for at in 0..current.len() {
                    let left = if at >= pixel { current[at - pixel] } else { 0 };
                    let up = self.previous[at];
                    let up_left = if at >= pixel { self.previous[at - pixel] } else { 0 };
                    current[at] = current[at].wrapping_add(match *kind {
                        0 => 0,
                        1 => left,
                        2 => up,
                        3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                        4 => paeth(left, up, up_left),
                        _ => return Err(bad("a PNG row filter that isn't one")),
                    });
                }
                self.previous.copy_from_slice(current);
            } else {
                for at in colors..self.row.len() {
                    self.row[at] = self.row[at].wrapping_add(self.row[at - colors]);
                }
                self.previous.copy_from_slice(&self.row);
            }
            self.pending.extend_from_slice(&self.previous);
        }
        Ok(give(&mut self.pending, &mut self.at, buf))
    }
}

fn paeth(left: u8, up: u8, up_left: u8) -> u8 {
    let (a, b, c) = (i16::from(left), i16::from(up), i16::from(up_left));
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        left
    } else if pb <= pc {
        up
    } else {
        up_left
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chain: &[Filter], input: &[u8]) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        decoded(Box::new(input), chain).read_to_end(&mut out)?;
        Ok(out)
    }

    /// Failure caught: a general filter decoded wrongly, which would give a page the digest of other bytes than its own, so two different pages could
    /// be taken for one, or one page for two. Each filter against a known encoding of the same bytes.
    #[test]
    fn each_general_filter_decodes_its_encoding() {
        let text = b"BT /F1 12 Tf 72 712 Td (Hello) Tj ET\n".repeat(50);
        let mut flated = Vec::new();
        {
            use std::io::Write;
            let mut encoder = flate2::write::ZlibEncoder::new(&mut flated, flate2::Compression::default());
            encoder.write_all(&text).unwrap();
            encoder.finish().unwrap();
        }
        assert_eq!(run(&[Filter::Flate(None)], &flated).unwrap(), text);
        let lzw = weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8).encode(&text).unwrap();
        assert_eq!(
            run(
                &[Filter::Lzw {
                    early_change: true,
                    predictor: None
                }],
                &lzw
            )
            .unwrap(),
            text
        );
        let hex: String = text.iter().map(|byte| format!("{byte:02x} ")).collect::<String>() + ">";
        assert_eq!(run(&[Filter::AsciiHex], hex.as_bytes()).unwrap(), text);
        assert_eq!(run(&[Filter::AsciiHex], b"4142 4>").unwrap(), b"AB@");
        assert_eq!(run(&[Filter::Ascii85], b"87cURD]i,\"Ebo80~>").unwrap(), b"Hello World!");
        assert_eq!(run(&[Filter::Ascii85], b"z!!*~>").unwrap(), &[0, 0, 0, 0, 0, 1]);
        assert_eq!(run(&[Filter::RunLength], &[2, b'a', b'b', b'c', 254, b'x', 128]).unwrap(), b"abcxxx");
        // A chain: hex over flate.
        let hex_of_flate: String = flated.iter().map(|byte| format!("{byte:02X}")).collect::<String>() + ">";
        assert_eq!(run(&[Filter::AsciiHex, Filter::Flate(None)], hex_of_flate.as_bytes()).unwrap(), text);
    }

    /// Failure caught: a predicted image decoded without its predictor, so a re-encode that keeps the pixels (as qpdf's did, research §5.2) changes the
    /// digest. PNG Up and Sub rows, and TIFF 2.
    #[test]
    fn predictors_are_undone() {
        let png = Predictor {
            png: true,
            row: 3,
            pixel: 1,
            colors: 1,
        };
        // Row 1 with Sub: 1, 1, 1 → 1, 2, 3. Row 2 with Up: 1, 1, 1 → 2, 3, 4.
        let rows = [1, 1, 1, 1, 2, 1, 1, 1];
        let mut out = Vec::new();
        Predicted::new(Box::new(&rows[..]), png).read_to_end(&mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 2, 3, 4]);
        let tiff = Predictor {
            png: false,
            row: 4,
            pixel: 2,
            colors: 2,
        };
        let mut out = Vec::new();
        Predicted::new(Box::new(&[10u8, 20, 1, 2, 5][..]), tiff).read_to_end(&mut out).unwrap();
        assert_eq!(out, [10, 20, 11, 22], "the short last row is left out");
    }

    /// Failure caught: a hostile stream that panics a decoder (and with it the identity thread) or runs without end. Bad digits, a bad PNG row filter,
    /// cut runs and garbage are errors.
    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        assert!(run(&[Filter::AsciiHex], b"4G>").is_err());
        assert!(run(&[Filter::Ascii85], b"vvvvv~>").is_err(), "a group past 2^32");
        assert!(run(&[Filter::Ascii85], b"!~>").is_err(), "a group of one");
        assert!(run(&[Filter::RunLength], &[5, b'a']).is_err());
        assert!(run(&[Filter::Flate(None)], b"not zlib at all").is_err());
        let png = Predictor {
            png: true,
            row: 2,
            pixel: 1,
            colors: 1,
        };
        assert!(Predicted::new(Box::new(&[9u8, 0, 0][..]), png).read_to_end(&mut Vec::new()).is_err());
        let garbage: Vec<u8> = (0..4096u32).map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        for filter in [
            Filter::Lzw {
                early_change: true,
                predictor: None,
            },
            Filter::Lzw {
                early_change: false,
                predictor: None,
            },
            Filter::RunLength,
        ] {
            let _ = run(&[filter], &garbage);
        }
    }
}
