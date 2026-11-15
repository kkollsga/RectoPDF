//! **The general filters, decoded a piece at a time,** for the page digest ([`super`]): `FlateDecode`, `LZWDecode`, `ASCIIHexDecode`, `ASCII85Decode` and
//! `RunLengthDecode`, with the PNG and TIFF predictors of `/DecodeParms`. Each wraps the reader before it, so a stream is never held whole: what's in memory
//! at once is a few buffers of [`CHUNK`] bytes and a predictor's row, which is capped. Every input is hostile: a decoder that meets what it can't read
//! returns an error, and never panics.
//!
//! **This is part of `recto-page/1`'s definition** (normative, with the golden vectors in `super`'s tests): what decodes, and to what, decides a
//! stream's kind (0 decoded; otherwise 1 or 3, hashed as stored) and its bytes.
//! - **Which filters:** `/Filter` absent (no filter: the stored bytes are the decoded ones), a name, or an array of at most [`MAX_FILTERS`] (8) names,
//!   each resolved through references; anything else is a filter not known. The general names are the five above and their abbreviations `Fl`, `LZW`,
//!   `AHx`, `A85` and `RL`; a chain with any other is not general.
//! - **Which parameters:** `/DecodeParms` (or `/DP`) absent or null (none for any filter); one dictionary for a chain of one filter (for a longer chain,
//!   refused, as which filter it's for isn't knowable); or an array of exactly as many entries as filters, each a dictionary or null. Any other form, or
//!   a value in one that isn't an integer, is refused: the stream doesn't decode.
//! - **Predictors:** `/Predictor` 1 or absent, none; 2, TIFF, only with `/BitsPerComponent` 8; 10 to 15, PNG, each row saying its own filter (0 to 4;
//!   any other is an error). `/Colors` 1 to 32, `/BitsPerComponent` 1, 2, 4, 8 or 16, `/Columns` 1 to 2^24, defaults 1, 8 and 1; a row of more than
//!   [`MAX_ROW`] bytes (`Colors × BitsPerComponent × Columns`, rounded up to bytes) is refused. A short last row is left out, as it's no whole row.
//!   PNG's filters reach back by `max(1, ceil(Colors × BitsPerComponent / 8))` bytes.
//! - **Flate:** zlib (RFC 1950), through `flate2`: a stream cut short (no final block), a wrong Adler-32 or bad data is an error, so the stream is
//!   hashed as stored. `/EarlyChange` for LZW: an integer, 0 for none, anything else (and the default) for early change; MSB first, codes from 9 bits, clear
//!   256 and end 257; a code that can't be is an error, and input ending without the end code ends there.
//! - **ASCIIHex:** pairs of hex digits of either case to `>`; whitespace and NUL between are skipped; an odd last digit is followed by a 0; the input's
//!   end without `>` ends it too; any other byte is an error.
//! - **ASCII85:** groups of five digits `!` to `u` to `~` (what follows it isn't read); `z` for four zeros, only at a group's start; whitespace and NUL
//!   are skipped; a last group of 2 to 4 digits is padded with `u` and gives one byte fewer than its digits, one of 1 is an error, as is a group past
//!   2^32 or any other byte; the input's end ends it as `~` would.
//! - **RunLength:** a length byte, then that many plus one bytes as they are (0 to 127), or the next byte 257 minus it times (129 to 255); 128 or the
//!   input's end ends it; a run cut short is an error.
//! - **How far each reads, which the caps depend on** (review F3): a decoder gives its bytes a unit at a time, and reads nothing for a unit past the
//!   one holding the last byte asked. The units: an ASCII85 group (up to 4 bytes; a `z` is one), an ASCIIHex pair, a predictor's **whole row** (read
//!   whole before any of it is given, so a row with a bad PNG filter byte, or a short last row, is met before any of its bytes count), a RunLength
//!   **whole run** (a literal run cut short is an error before any of its bytes count), and for Flate and LZW what their libraries give for the bytes
//!   asked. So a stream past a cap reaches it ([`super`] asks for one byte past the cap) unless the unit holding the cap's byte is an error, in which
//!   case the stream didn't decode; what lies after that unit is never read for it.

use lopdf::{Dictionary, Object};
use std::io::{self, BufRead, BufReader, Read};

/// How much each stage reads at a time.
pub(super) const CHUNK: usize = 64 * 1024;
/// Longest predictor row taken: a real image row is a few KiB (none of the 5,872 corpus pages changes digest with a limit of 16 KiB, measured
/// 2026-11-15); past this the parameters are refused, so a row can't be made to cost memory: eight chained predictors hold about 6 MiB at most, where
/// 1 MiB rows let them hold 24.
const MAX_ROW: usize = 256 << 10;
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

/// The filter names a fuzz input picks from: the general ones, their abbreviations, and one that isn't general.
pub(super) const FUZZ_NAMES: [&[u8]; 11] = [
    b"FlateDecode",
    b"LZWDecode",
    b"ASCIIHexDecode",
    b"ASCII85Decode",
    b"RunLengthDecode",
    b"Fl",
    b"LZW",
    b"AHx",
    b"A85",
    b"RL",
    b"DCTDecode",
];

/// A fuzz input's stream dictionary, and the stream's bytes: [`super::fuzz_stream`] says the layout.
pub(super) fn fuzz_dictionary(input: &[u8]) -> (Dictionary, &[u8]) {
    let mut at = 0;
    let mut byte = || {
        let value = input.get(at).copied().unwrap_or(0);
        at = (at + 1).min(input.len());
        value
    };
    let count = usize::from(byte() % 10);
    let names: Vec<Object> = (0..count)
        .map(|_| Object::Name(FUZZ_NAMES[usize::from(byte()) % FUZZ_NAMES.len()].to_vec()))
        .collect();
    let form = byte() % 5;
    let parms = |byte: &mut dyn FnMut() -> u8| {
        let mut dict = Dictionary::new();
        let predictor = [1, 2, 10, 12, 15, 5, 99][usize::from(byte()) % 7];
        dict.set("Predictor", Object::Integer(predictor));
        dict.set("Colors", Object::Integer(i64::from(byte()) - 2));
        dict.set("BitsPerComponent", Object::Integer([1, 2, 4, 8, 16, 3][usize::from(byte()) % 6]));
        let columns = u16::from_le_bytes([byte(), byte()]);
        dict.set("Columns", Object::Integer(if columns == u16::MAX { 1 << 30 } else { i64::from(columns) }));
        dict.set("EarlyChange", Object::Integer(i64::from(byte() % 3)));
        Object::Dictionary(dict)
    };
    let mut dict = Dictionary::new();
    match names.len() {
        0 => {}
        1 if form != 4 => dict.set("Filter", names[0].clone()),
        _ => dict.set("Filter", Object::Array(names.clone())),
    }
    match form {
        1 => {
            let one = parms(&mut byte);
            dict.set("DecodeParms", one);
        }
        2 => {
            let all: Vec<Object> = (0..names.len()).map(|_| parms(&mut byte)).collect();
            dict.set("DecodeParms", Object::Array(all));
        }
        3 => dict.set("DecodeParms", Object::Null),
        4 => dict.set("DecodeParms", Object::Integer(7)),
        _ => {}
    }
    (dict, &input[at..])
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

/// Base-85 groups of five, `z` for four zeros, to `~>`. Gives no more than it's asked for: it decodes a group at a time into the caller's buffer and
/// stops once that's full, so what it meets past the bytes asked (a bad byte after a cap's byte) is never read for them (review F3: a read-ahead of
/// 64 KiB made the digest depend on it).
struct Ascii85<'a> {
    input: BufReader<Box<dyn Read + 'a>>,
    /// The bytes of the last group that didn't fit in what was asked, given first at the next read: at most 3.
    carry: [u8; 4],
    carry_at: usize,
    carry_len: usize,
    group: [u8; 5],
    count: usize,
    done: bool,
    /// An error met after some bytes of a read were given: said at the next read, so the bytes before it are given as a lazy decode gives them.
    failed: Option<&'static str>,
}

impl<'a> Ascii85<'a> {
    fn new(input: Box<dyn Read + 'a>) -> Self {
        Self {
            input: BufReader::with_capacity(CHUNK, input),
            carry: [0; 4],
            carry_at: 0,
            carry_len: 0,
            group: [0; 5],
            count: 0,
            done: false,
            failed: None,
        }
    }
}

/// A group's value: its digits, a short group padded with `u` (84).
fn base85(group: &[u8; 5], count: usize) -> Result<[u8; 4], &'static str> {
    let value = group
        .iter()
        .enumerate()
        .fold(0u64, |value, (at, digit)| value * 85 + u64::from(if at < count { *digit } else { 84 }));
    u32::try_from(value).map(u32::to_be_bytes).map_err(|_| "a base-85 group past 2^32")
}

/// The last group, at the input's end or `~`: none, or its digits less one of bytes; a group of one is an error.
fn last_group(group: &[u8; 5], count: usize) -> Result<Option<([u8; 4], usize)>, &'static str> {
    match count {
        0 => Ok(None),
        1 => Err("a base-85 group of one"),
        count => Ok(Some((base85(group, count)?, count - 1))),
    }
}

impl Read for Ascii85<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut out = 0;
        while self.carry_at < self.carry_len && out < buf.len() {
            buf[out] = self.carry[self.carry_at];
            self.carry_at += 1;
            out += 1;
        }
        if out == 0
            && let Some(what) = self.failed.take()
        {
            return Err(bad(what));
        }
        while out < buf.len() && !self.done && self.failed.is_none() {
            let input = match self.input.fill_buf() {
                Ok(input) => input,
                Err(error) if out == 0 => return Err(error),
                Err(_) => {
                    self.failed = Some("the stream couldn't be read");
                    break;
                }
            };
            let at_end = input.is_empty();
            let mut used = 0;
            for &byte in input.iter().chain(at_end.then_some(&b'~')) {
                used += 1;
                let made = match byte {
                    b'~' => {
                        self.done = true;
                        last_group(&self.group, self.count)
                    }
                    b'z' if self.count == 0 => Ok(Some(([0; 4], 4))),
                    b'!'..=b'u' => {
                        self.group[self.count] = byte - b'!';
                        self.count += 1;
                        if self.count == 5 {
                            self.count = 0;
                            base85(&self.group, 5).map(|bytes| Some((bytes, 4)))
                        } else {
                            Ok(None)
                        }
                    }
                    byte if byte.is_ascii_whitespace() || byte == 0 => Ok(None),
                    _ => Err("not a base-85 digit"),
                };
                match made {
                    Ok(Some((bytes, n))) => {
                        let fits = n.min(buf.len() - out);
                        buf[out..out + fits].copy_from_slice(&bytes[..fits]);
                        out += fits;
                        self.carry[..n - fits].copy_from_slice(&bytes[fits..n]);
                        (self.carry_at, self.carry_len) = (0, n - fits);
                    }
                    Ok(None) => {}
                    Err(what) => self.failed = Some(what),
                }
                if self.done || self.failed.is_some() || out == buf.len() {
                    break;
                }
            }
            self.input.consume(used.min(self.input.buffer().len()));
        }
        if out == 0
            && let Some(what) = self.failed.take()
        {
            return Err(bad(what));
        }
        Ok(out)
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

    /// Failure caught: the decoders do other than the module doc says where it's said in words only (and so a page's kind and bytes differ from
    /// the definition): a Flate stream cut short is an error, and so is a wrong Adler-32; ASCIIHex and ASCII85 end at the input's end as at their
    /// end marks; ASCII85's last group of 2 to 4 digits gives a byte fewer; RunLength ends at the input's end.
    #[test]
    fn the_decoders_do_what_their_doc_says_at_the_edges() {
        let text = b"BT /F1 12 Tf 72 712 Td (Hello) Tj ET\n".repeat(200);
        let mut flated = Vec::new();
        {
            use std::io::Write;
            let mut encoder = flate2::write::ZlibEncoder::new(&mut flated, flate2::Compression::none());
            encoder.write_all(&text).unwrap();
            encoder.finish().unwrap();
        }
        assert!(run(&[Filter::Flate(None)], &flated[..flated.len() / 2]).is_err(), "a stream cut short");
        assert_eq!(run(&[Filter::Flate(None)], &flated).unwrap(), text, "and whole, it decodes");
        let mut wrong = flated.clone();
        *wrong.last_mut().unwrap() ^= 1;
        assert!(run(&[Filter::Flate(None)], &wrong).is_err(), "a wrong Adler-32");
        assert_eq!(run(&[Filter::AsciiHex], b"414").unwrap(), b"A@");
        assert_eq!(run(&[Filter::Ascii85], b"87cURD]i,\"Ebo80").unwrap(), b"Hello World!");
        assert_eq!(run(&[Filter::Ascii85], b"87cUR~>").unwrap(), b"Hell");
        assert_eq!(run(&[Filter::Ascii85], b"87cU~>").unwrap(), b"Hel");
        assert_eq!(run(&[Filter::RunLength], &[1, b'a', b'b']).unwrap(), b"ab");
    }

    /// Failure caught (review F3): ASCII85 reads ahead of what it's asked, so a bad byte after the bytes asked is met in the same read and fails them;
    /// or a group split across two reads loses or repeats a byte. Asked a byte at a time, it gives each group's bytes before the bad byte after them
    /// and the error only then, and the bytes of a stream read a byte at a time are the bytes read whole.
    #[test]
    fn ascii85_gives_no_more_than_it_is_asked_for() {
        let mut decoder = Ascii85::new(Box::new(&b"87cURx"[..]));
        let mut got = Vec::new();
        let mut one = [0u8];
        let error = loop {
            match decoder.read(&mut one) {
                Ok(0) => panic!("the bad byte is an error"),
                Ok(_) => got.push(one[0]),
                Err(error) => break error,
            }
        };
        assert_eq!((got.as_slice(), error.kind()), (&b"Hell"[..], io::ErrorKind::InvalidData));
        let text = b"87cURD]i,\"Ebo80 z 87cU~>";
        let mut decoder = Ascii85::new(Box::new(&text[..]));
        let mut by_byte = Vec::new();
        while decoder.read(&mut one).unwrap() == 1 {
            by_byte.push(one[0]);
        }
        assert_eq!(by_byte, run(&[Filter::Ascii85], text).unwrap());
        assert_eq!(by_byte, b"Hello World!\0\0\0\0Hel");
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
