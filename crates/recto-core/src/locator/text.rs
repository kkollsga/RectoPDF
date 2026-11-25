//! The character check of `recto-text/1`, frozen: a reference to characters of a page carries the first 8 bytes of
//! SHA-256 of `"recto-text/1\0"` followed by the UTF-8 of characters `[start, end)` of the page's text, where a character is a Unicode scalar
//! value and the page's text is what `recto text` gives in `untrusted.text` (`recto_index::read_page`). Any change to this is `recto-text/2`, with
//! token version `r2`, never a quiet change here.
//!
//! The core hashes the text it is given and never normalises it, reads no file and keeps nothing. The offsets are checked at the same places only:
//! words that moved within the page are `Changed`, never searched for. Every walk stops at the range's end, which is at most
//! [`MAX_CHARACTER`](super::MAX_CHARACTER), whatever the length of the text.

use super::{CharacterRange, Error, MAX_CHARACTER};
use sha2::{Digest, Sha256};

/// What the check is of: the name of the check and its version, then a NUL, before the characters.
const DOMAIN: &[u8] = b"recto-text/1\0";

/// The check of a run of characters, as `recto-text/1` defines it.
pub fn text_check(characters: &str) -> [u8; 8] {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update(characters.as_bytes());
    let digest = hasher.finalize();
    let mut check = [0; 8];
    check.copy_from_slice(&digest[..8]);
    check
}

/// The bytes of `text` that hold characters `[start, end)`, or `None` when the text has fewer than `end` characters. Walks no further than `end`
/// characters.
fn span(text: &str, start: u32, end: u32) -> Option<&str> {
    let mut from = (start == 0).then_some(0);
    let mut count = 0u64;
    for (index, (at, _)) in text.char_indices().enumerate() {
        if index as u64 == u64::from(start) {
            from = Some(at);
        }
        if index as u64 == u64::from(end) {
            return from.map(|from| &text[from..at]);
        }
        count = index as u64 + 1;
    }
    // The text ended: it holds the range only if it ends exactly where the range does.
    (count == u64::from(end)).then_some(()).and_then(|()| from.map(|from| &text[from..]))
}

/// The reference's range over the characters `[start, end)` of `page_text`, with their check. Refused when it is empty, past the cap, or past the end of the
/// text.
pub fn characters(page_text: &str, start: u32, end: u32) -> Result<CharacterRange, Error> {
    if start >= end || end > MAX_CHARACTER {
        return Err(Error::OutOfRange);
    }
    let held = span(page_text, start, end).ok_or(Error::OutOfRange)?;
    CharacterRange::new(start, end, text_check(held))
}

/// Whether a range's check still holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextMatch {
    /// The characters at those offsets are the ones the reference was made of.
    Same,
    /// They are not, or the text no longer reaches that far.
    Changed,
}

/// The characters of `page_text` at the range's own offsets, when their check is the one the range was made with, and `None` when the text doesn't reach
/// that far or holds other characters there. No other place in the text is looked at. One walk to the range's end and one hash.
pub fn held_characters(range: CharacterRange, page_text: &str) -> Option<&str> {
    let (start, end) = range.offsets();
    span(page_text, start, end).filter(|held| text_check(held) == range.text_check())
}

/// Whether `page_text` holds, at the range's own offsets, the characters its check was made of. No other place in the text is looked at.
pub fn check_characters(range: CharacterRange, page_text: &str) -> TextMatch {
    held_characters(range, page_text).map_or(TextMatch::Changed, |_| TextMatch::Same)
}
