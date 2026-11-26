//! **The names of the writer's temporary files**, so a survey can tell a crashed save's leftover from a person's own file (`docs/design/removal.md`
//! §3, §10 "D2c-2b as built").
//!
//! `lopdf::append_atomically` makes a temporary beside the target: a file `.append-<16 hex>-<pid>-<attempt>.tmp` (a Save As, or an empty start), or for a
//! Save over the same file a folder `….d` holding a copy-on-write `clone` of it. The 16 hex are the first 8 bytes of SHA-256 of the target's file name, so
//! the name says nothing of the file and fits whatever the name's length. lopdf's `temporary_prefix` and `temporary_owner` are private, so the rule is
//! restated here, at the pinned revision `e5f4971` (`src/indexed_reader/append.rs:1383` and `:1399`), and a test calls `append_atomically` and checks
//! what it makes. A fork bump that renames them fails that test and not a survey that reads "none found".

use sha2::Digest;
use std::path::Path;

/// What a temporary name says it is: the file of a Save As or empty start, or the folder of a Save over the same file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempShape {
    File,
    Folder,
}

const PREFIX: &[u8] = b".append-";

/// The start of every temporary name for `real`: `.append-<16 hex>-`, the hash being of its file name alone (`document` if the path has none), as lopdf's
/// `temporary_prefix` makes it.
pub fn writer_temp_prefix(real: &Path) -> String {
    let name = real.file_name().map_or(&b"document"[..], std::ffi::OsStr::as_encoded_bytes);
    let digest = sha2::Sha256::digest(name);
    let mut prefix = String::from(".append-");
    for byte in &digest[..8] {
        prefix.push_str(&format!("{byte:02x}"));
    }
    prefix.push('-');
    prefix
}

/// Reads `name` as exactly `.append-<16 hex>-<digits>-<digits>` followed by `.tmp` (a file) or `.d` (a folder): the 16 hex and what the name is. Anything
/// else is `None`: a person's own file is never taken for a temporary. It says nothing of whether the process number could be a process (lopdf's sweep
/// asks that; a survey doesn't care).
pub fn writer_temp_shape(name: &[u8]) -> Option<(&str, TempShape)> {
    let rest = name.strip_prefix(PREFIX)?;
    let (rest, shape) = if let Some(rest) = rest.strip_suffix(b".tmp") {
        (rest, TempShape::File)
    } else {
        (rest.strip_suffix(b".d")?, TempShape::Folder)
    };
    let (hash, numbers) = rest.split_at_checked(16)?;
    if !hash.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    let mut numbers = numbers.strip_prefix(b"-")?.split(|b| *b == b'-');
    let digits = |text: Option<&[u8]>| text.is_some_and(|text| !text.is_empty() && text.iter().all(u8::is_ascii_digit));
    if !digits(numbers.next()) || !digits(numbers.next()) || numbers.next().is_some() {
        return None;
    }
    // The hash is ASCII hex, so it is text.
    Some((std::str::from_utf8(hash).ok()?, shape))
}

#[cfg(test)]
mod tests;
