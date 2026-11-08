//! A mutator that knows what a PDF looks like, on stable. A random byte in a real PDF mostly lands in a stream, or breaks the file's offsets so nothing but the repair path ever runs. This swaps references, plants hostile numbers and structure, and can write a fresh cross-reference table so the normal route is what gets tested.

use crate::input::Rng;

/// Numbers that find overflows and bad assumptions.
pub const HOSTILE_NUMBERS: &[&str] = &[
    "0",
    "-1",
    "1",
    "2",
    "255",
    "256",
    "65535",
    "65536",
    "2147483647",
    "2147483648",
    "-2147483648",
    "4294967295",
    "4294967296",
    "9223372036854775807",
    "9223372036854775808",
    "99999999999999999999999",
    "1e308",
    "0.0000001",
    "-0.0",
];

/// What the walks in `recto_core` look for, and the tokens the reader is built around. `fuzz/dict/pdf.dict` is the same list for libFuzzer.
pub const TOKENS: &[&str] = &[
    "/Kids",
    "/Parent",
    "/Count",
    "/Pages",
    "/Page",
    "/Type",
    "/Catalog",
    "/Outlines",
    "/First",
    "/Last",
    "/Next",
    "/Prev",
    "/Title",
    "/Dest",
    "/Dests",
    "/A",
    "/S",
    "/D",
    "/GoTo",
    "/Names",
    "/Limits",
    "/AcroForm",
    "/Fields",
    "/FT",
    "/Sig",
    "/T",
    "/V",
    "/P",
    "/ByteRange",
    "/Filter",
    "/SubFilter",
    "/Perms",
    "/DocMDP",
    "/Reason",
    "/Location",
    "/M",
    "/Name",
    "/Info",
    "/Root",
    "/Encrypt",
    "/Size",
    "/Length",
    "/ObjStm",
    "/XRef",
    "/N",
    "/W",
    "/Index",
    "/FlateDecode",
    "/DecodeParms",
    "/Predictor",
    "/Columns",
    "/MediaBox",
    "/Rotate",
    "/UserUnit",
    "null",
    "true",
    "<<",
    ">>",
    "[",
    "]",
    "(",
    ")",
    "<",
    ">",
    "R",
    "obj",
    "endobj",
    "stream",
    "endstream",
    "xref",
    "trailer",
    "startxref",
    "%%EOF",
    "%PDF-1.7",
    "<FEFF",
    "\u{feff}",
];

/// Where a piece of ASCII digits starts and ends, at or after a place.
fn number_at(bytes: &[u8], from: usize) -> Option<(usize, usize)> {
    let start = from + bytes[from..].iter().position(u8::is_ascii_digit)?;
    let len = bytes[start..].iter().position(|b| !b.is_ascii_digit()).unwrap_or(bytes.len() - start);
    Some((start, start + len))
}

/// Every `N G R` in the bytes, as the place of its `N`. At most a few thousand, so a large file costs little.
fn references(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = bytes[at..]
        .windows(3)
        .position(|window| window == b" R ".as_slice() || window == b" R/".as_slice() || window == b" R]".as_slice() || window == b" R>".as_slice())
    {
        let end = at + offset;
        // Back over the generation, then the number.
        let before = &bytes[..end];
        let generation = before.iter().rposition(|b| !b.is_ascii_digit()).unwrap_or(0);
        let number_end = before[..generation].iter().rposition(|b| !b.is_ascii_whitespace()).map_or(0, |last| last + 1);
        let number_start = before[..number_end].iter().rposition(|b| !b.is_ascii_digit()).map_or(0, |last| last + 1);
        if number_start < number_end {
            found.push((number_start, number_end));
        }
        at = end + 3;
        if found.len() >= 4096 {
            break;
        }
    }
    found
}

/// Replaces a stretch with other bytes.
fn splice(bytes: &mut Vec<u8>, range: (usize, usize), with: &[u8]) {
    bytes.splice(range.0..range.1, with.iter().copied());
}

/// Changes a PDF a little, keeping it under `max_len`.
pub fn pdf(bytes: &mut Vec<u8>, rng: &mut Rng, max_len: usize) {
    for _ in 0..1 + rng.below(4) {
        match rng.below(10) {
            // Point a reference somewhere else: at itself, at another object, or at nothing.
            0..=2 => {
                let found = references(bytes);
                if !found.is_empty() {
                    let target = rng.pick(&found);
                    let source = rng.pick(&found);
                    let with = if rng.below(4) == 0 {
                        rng.pick(HOSTILE_NUMBERS).as_bytes().to_vec()
                    } else {
                        bytes[source.0..source.1].to_vec()
                    };
                    splice(bytes, target, &with);
                }
            }
            // A number becomes a hostile one.
            3..=4 => {
                if let Some(range) = number_at(bytes, rng.below(bytes.len())) {
                    splice(bytes, range, rng.pick(HOSTILE_NUMBERS).as_bytes());
                }
            }
            // A token goes in.
            5 => {
                let at = rng.below(bytes.len() + 1);
                let token = format!(" {} ", rng.pick(TOKENS));
                splice(bytes, (at, at), token.as_bytes());
            }
            // A piece is copied elsewhere, which is how deep and repeated structure gets made.
            6 => {
                let (from, len) = (rng.below(bytes.len()), 1 + rng.below(256));
                let piece = bytes[from..(from + len).min(bytes.len())].to_vec();
                let at = rng.below(bytes.len() + 1);
                splice(bytes, (at, at), &piece);
            }
            // A piece goes.
            7 => {
                let (from, len) = (rng.below(bytes.len()), 1 + rng.below(256));
                let end = (from + len).min(bytes.len());
                bytes.drain(from..end);
            }
            // Cut short, as a download might be.
            8 => bytes.truncate(rng.below(bytes.len() + 1)),
            // A byte or a bit, for everything nobody thought of.
            _ => {
                if !bytes.is_empty() {
                    let at = rng.below(bytes.len());
                    bytes[at] = if rng.below(2) == 0 {
                        rng.next_u64() as u8
                    } else {
                        bytes[at] ^ (1 << rng.below(8))
                    };
                }
            }
        }
        if bytes.is_empty() {
            bytes.extend_from_slice(b"%PDF-1.7\n");
        }
    }
    // Half the time the offsets get mended, so the walks past the repair path are reached.
    if rng.below(2) == 0 {
        fresh_xref(bytes);
    }
    bytes.truncate(max_len);
}

/// Bytes in general: flips, splices and hostile numbers, for inputs with no PDF in them.
pub fn bytes(data: &mut Vec<u8>, rng: &mut Rng, max_len: usize) {
    for _ in 0..1 + rng.below(4) {
        if data.is_empty() {
            data.push(0);
        }
        match rng.below(5) {
            0 => {
                let at = rng.below(data.len());
                data[at] = rng.next_u64() as u8;
            }
            1 => {
                let at = rng.below(data.len());
                data[at] ^= 1 << rng.below(8);
            }
            2 => {
                let (from, len) = (rng.below(data.len()), 1 + rng.below(64));
                let piece = data[from..(from + len).min(data.len())].to_vec();
                let at = rng.below(data.len() + 1);
                splice(data, (at, at), &piece);
            }
            3 => {
                let (from, len) = (rng.below(data.len()), 1 + rng.below(64));
                let end = (from + len).min(data.len());
                data.drain(from..end);
            }
            _ => {
                let width = rng.pick(&[1usize, 2, 4, 8]);
                let at = rng.below(data.len());
                let value = rng.pick(&[0u64, 1, 0x7F, 0x80, 0xFF, 0xFFFF, 0x7FFF_FFFF, 0xFFFF_FFFF, u64::MAX]).to_le_bytes();
                let end = (at + width).min(data.len());
                let room = end - at;
                data[at..end].copy_from_slice(&value[..room]);
            }
        }
    }
    data.truncate(max_len);
}

/// Appends a cross-reference table for the objects it can see, with a trailer that names the catalog it finds. Damaged input stays damaged, and now the reader meets it by the front door.
pub fn fresh_xref(bytes: &mut Vec<u8>) {
    let mut objects: Vec<(u32, usize)> = Vec::new();
    let mut root = None;
    let mut at = 0;
    while let Some(offset) = bytes[at..].windows(4).position(|window| window == b" obj") {
        let end = at + offset;
        at = end + 4;
        // `N G obj`, reading back from the `obj`.
        let before = &bytes[..end];
        let generation_start = before.iter().rposition(|b| !b.is_ascii_digit()).map_or(0, |last| last + 1);
        if generation_start == 0 || generation_start == end {
            continue;
        }
        let number_end = generation_start - 1;
        let number_start = before[..number_end].iter().rposition(|b| !b.is_ascii_digit()).map_or(0, |last| last + 1);
        let Some(number) = std::str::from_utf8(&before[number_start..number_end])
            .ok()
            .and_then(|text| text.parse::<u32>().ok())
        else {
            continue;
        };
        if number == 0 || number > 1_000_000 {
            continue;
        }
        objects.push((number, number_start));
        let body = &bytes[at..(at + 256).min(bytes.len())];
        if root.is_none() && body.windows(8).any(|window| window == b"/Catalog") {
            root = Some(number);
        }
        if objects.len() >= 8192 {
            break;
        }
    }
    let (Some(root), false) = (root, objects.is_empty()) else { return };
    objects.sort_unstable();
    objects.dedup_by_key(|object| object.0);
    let start = bytes.len();
    let mut table = String::from("\nxref\n0 1\n0000000000 65535 f \n");
    for (number, offset) in &objects {
        table.push_str(&format!("{number} 1\n{offset:010} 00000 n \n"));
    }
    let size = objects.last().map_or(1, |last| last.0 + 1);
    table.push_str(&format!("trailer\n<< /Size {size} /Root {root} 0 R >>\nstartxref\n{}\n%%EOF\n", start + 1));
    bytes.extend_from_slice(table.as_bytes());
}
