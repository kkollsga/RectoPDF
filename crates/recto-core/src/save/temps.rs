//! **The names of the writer's temporary files**, so a survey can tell a crashed save's leftover from a person's own file by shape alone (`docs/design/removal.md`
//! §3, §10 "D2c-2b as built": any lopdf temp in a listed folder refuses, whatever its hash).
//!
//! `lopdf::append_atomically` makes a temporary beside the target: a file `.append-<16 hex>-<pid>-<attempt>.tmp` (a Save As, or an empty start), or for a
//! Save over the same file a folder `….d` holding a copy-on-write `clone` of it. The 16 hex are the first 8 bytes of SHA-256 of the target's file name, so
//! the name says nothing of the file and fits whatever the name's length. lopdf's `temporary_prefix` and `temporary_owner` are private, so the rule is
//! restated here, at the pinned revision `e5f4971` (`src/indexed_reader/append.rs:1383` and `:1399`), and a test calls `append_atomically` and checks
//! what it makes. A fork bump that renames them fails that test and not a survey that reads "none found".

use sha2::Digest;
use std::path::Path;

const PREFIX: &[u8] = b".append-";

/// **Used by the person's confirmation** (`leftovers::saves_of` finds a temp's document by it) **and by tests** (a survey reads shape alone; the tests of the survey and of the pinned writer name a temp by it). The start of every temporary name for `real`: `.append-<16 hex>-`, the hash being of its file name alone (`document` if the path has none), as lopdf's
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

/// Whether `name` is exactly `.append-<16 hex>-<digits>-<digits>` followed by `.tmp` (the file of a Save As or empty start) or `.d` (the folder of a Save over
/// the same file). Anything else is a person's own file, never taken for a temporary. It says nothing of whose temporary it is (any lopdf temp in a listed
/// folder refuses a removal, whatever its hash) nor of whether the process number could be a process (lopdf's sweep asks that).
pub fn writer_temp_shape(name: &[u8]) -> bool {
    let Some(rest) = name.strip_prefix(PREFIX) else { return false };
    let Some(rest) = rest.strip_suffix(b".tmp").or_else(|| rest.strip_suffix(b".d")) else {
        return false;
    };
    let Some((hash, numbers)) = rest.split_at_checked(16) else { return false };
    if !hash.iter().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return false;
    }
    let Some(numbers) = numbers.strip_prefix(b"-") else { return false };
    let mut numbers = numbers.split(|b| *b == b'-');
    let digits = |text: Option<&[u8]>| text.is_some_and(|text| !text.is_empty() && text.iter().all(u8::is_ascii_digit));
    digits(numbers.next()) && digits(numbers.next()) && numbers.next().is_none()
}

#[cfg(test)]
mod tests;
