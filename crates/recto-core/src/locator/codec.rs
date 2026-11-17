//! Strict canonical ASCII tokens. One bounded output allocation; parsing allocates nothing.

use super::{CharacterRange, DocumentTags, Error, Locator, MAX_TOKEN_BYTES, PageRef, Spot, Target};
use std::fmt::Write;

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
/// Largest supported form: two document tags, page 200000 and the largest two character offsets.
const ENCODED_CAPACITY: usize = 104;

/// Encode one validated value. Document tags and checks contain no path, quote or authority.
pub fn encode(locator: &Locator) -> String {
    let mut out = String::with_capacity(ENCODED_CAPACITY);
    out.push_str("r1.");
    append_check(&mut out, &locator.document.content);
    if let Some(id) = locator.document.id {
        out.push('-');
        append_check(&mut out, &id);
    }
    let _ = write!(out, ".p{}-", locator.page.page);
    append_check(&mut out, &locator.page.check);
    out.push('-');
    append_check(&mut out, &locator.page.frame.to_be_bytes());
    match locator.target {
        Target::Page => {}
        Target::Characters(range) => {
            let _ = write!(out, ".c{}-{}-", range.start, range.end);
            append_check(&mut out, &range.check);
        }
        Target::Spot(spot) => {
            let _ = write!(out, ".x{},{}", spot.x, spot.y);
            if let Some([width, height]) = spot.extent {
                let _ = write!(out, ",{width},{height}");
            }
        }
    }
    out
}

/// Decode the whole token or refuse it. No filesystem access, variable-sized collection or guessed recovery.
pub fn decode(token: &str) -> Result<Locator, Error> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(Error::TooLong);
    }
    if !token.is_ascii() {
        return Err(Error::NonAscii);
    }
    let mut parts = token.split('.');
    let version = parts.next().ok_or(Error::Malformed)?;
    if version != "r1" {
        if version
            .strip_prefix('r')
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|digit| digit.is_ascii_digit()))
        {
            return Err(Error::UnsupportedVersion);
        }
        return Err(Error::Malformed);
    }
    let document = document(parts.next().ok_or(Error::Malformed)?)?;
    let page = page(parts.next().ok_or(Error::Malformed)?)?;
    let target = parts.next().map(target).transpose()?.unwrap_or(Target::Page);
    if parts.next().is_some() {
        return Err(Error::Malformed);
    }
    Ok(Locator::new(document, page, target))
}

fn document(part: &str) -> Result<DocumentTags, Error> {
    let mut tags = part.split('-');
    let content = check(tags.next().ok_or(Error::Malformed)?)?;
    let id = tags.next().map(check).transpose()?;
    if tags.next().is_some() {
        return Err(Error::Malformed);
    }
    Ok(DocumentTags::new(content, id))
}

fn page(part: &str) -> Result<PageRef, Error> {
    let mut fields = part.strip_prefix('p').ok_or(Error::Malformed)?.split('-');
    let page = decimal(fields.next().ok_or(Error::Malformed)?)?;
    let digest = check(fields.next().ok_or(Error::Malformed)?)?;
    let frame = u32::from_be_bytes(check(fields.next().ok_or(Error::Malformed)?)?);
    if fields.next().is_some() {
        return Err(Error::Malformed);
    }
    PageRef::from_checks(page, digest, frame)
}

fn target(part: &str) -> Result<Target, Error> {
    if let Some(chars) = part.strip_prefix('c') {
        let mut fields = chars.split('-');
        let start = decimal(fields.next().ok_or(Error::Malformed)?)?;
        let end = decimal(fields.next().ok_or(Error::Malformed)?)?;
        let digest = check(fields.next().ok_or(Error::Malformed)?)?;
        if fields.next().is_some() {
            return Err(Error::Malformed);
        }
        return Ok(Target::Characters(CharacterRange::new(start, end, digest)?));
    }
    let mut fields = part.strip_prefix('x').ok_or(Error::Malformed)?.split(',');
    let x = coordinate(fields.next().ok_or(Error::Malformed)?)?;
    let y = coordinate(fields.next().ok_or(Error::Malformed)?)?;
    let spot = match fields.next() {
        None => Spot::point(x, y)?,
        Some(width) => {
            let width = coordinate(width)?;
            let height = coordinate(fields.next().ok_or(Error::Malformed)?)?;
            Spot::area(x, y, width, height)?
        }
    };
    if fields.next().is_some() {
        return Err(Error::Malformed);
    }
    Ok(Target::Spot(spot))
}

fn coordinate(input: &str) -> Result<u16, Error> {
    u16::try_from(decimal(input)?).map_err(|_| Error::OutOfRange)
}

fn decimal(input: &str) -> Result<u32, Error> {
    if input.is_empty() || (input.len() > 1 && input.starts_with('0')) {
        return Err(Error::Malformed);
    }
    input.bytes().try_fold(0_u32, |value, digit| {
        if !digit.is_ascii_digit() {
            return Err(Error::Malformed);
        }
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u32::from(digit - b'0')))
            .ok_or(Error::OutOfRange)
    })
}

fn append_check(out: &mut String, bytes: &[u8]) {
    let (mut buffer, mut bits) = (0_u16, 0_u8);
    for byte in bytes {
        buffer = (buffer << 8) | u16::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[usize::from((buffer >> bits) & 31)]));
        }
        buffer &= (1 << bits) - 1;
    }
    if bits != 0 {
        out.push(char::from(ALPHABET[usize::from(buffer << (5 - bits))]));
    }
}

fn check<const N: usize>(input: &str) -> Result<[u8; N], Error> {
    if input.len() != (N * 8).div_ceil(5) {
        return Err(Error::Malformed);
    }
    let mut bytes = [0; N];
    let (mut buffer, mut bits, mut at) = (0_u16, 0_u8, 0_usize);
    for digit in input.bytes() {
        let value = match digit {
            b'0'..=b'9' => digit - b'0',
            b'a'..=b'h' => digit - b'a' + 10,
            b'j'..=b'k' => digit - b'j' + 18,
            b'm'..=b'n' => digit - b'm' + 20,
            b'p'..=b't' => digit - b'p' + 22,
            b'v'..=b'z' => digit - b'v' + 27,
            _ => return Err(Error::Malformed),
        };
        buffer = (buffer << 5) | u16::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            *bytes.get_mut(at).ok_or(Error::Malformed)? = (buffer >> bits) as u8;
            at += 1;
        }
        buffer &= (1 << bits) - 1;
    }
    if buffer != 0 || at != N {
        return Err(Error::Malformed);
    }
    Ok(bytes)
}
