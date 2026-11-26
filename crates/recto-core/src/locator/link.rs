//! The link form: a file's own name and a page, as `name.pdf#page=N`, the standard open parameter every PDF reader takes.
//!
//! Pure bytes in, a string out: no I/O, no state, no parsing. The name is untrusted (a file's name is anyone's), so every byte outside the unreserved set
//! `A-Za-z0-9-._~` becomes `%XX`: a name can't add a scheme (`:`), a fragment (`#`), a query (`?`) or a path (`/`), and can't change the page. Our own
//! part of a link comes later as `rl=`, with the token's own version; none is here.

use super::MAX_PAGE;

/// Longest file name, in bytes, the link form takes: the most a file system holds.
pub const MAX_NAME_BYTES: usize = 255;
/// Longest link made: every name byte as `%XX`, then `#page=` and the page's digits.
pub const MAX_LINK_BYTES: usize = 3 * MAX_NAME_BYTES + 12;

/// Why a link can't be made. Never holds or repeats the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// The page is 0 or past [`MAX_PAGE`].
    Page,
    /// The name is empty, `.` or `..`, holds `/` or NUL, or is past [`MAX_NAME_BYTES`].
    Name,
}

/// `name` (the file's own name, as bytes) and `file_page` (counted from 1) as `name.pdf#page=N`.
pub fn page_link(name: &[u8], file_page: u32) -> Result<String, LinkError> {
    if !(1..=MAX_PAGE).contains(&file_page) {
        return Err(LinkError::Page);
    }
    if name.is_empty() || name == b"." || name == b".." || name.len() > MAX_NAME_BYTES || name.iter().any(|byte| matches!(byte, b'/' | 0)) {
        return Err(LinkError::Name);
    }
    use std::fmt::Write;
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut link = String::with_capacity(3 * name.len() + 12);
    for &byte in name {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            link.push(char::from(byte));
        } else {
            link.push('%');
            link.push(char::from(HEX[usize::from(byte >> 4)]));
            link.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    link.push_str("#page=");
    write!(link, "{file_page}").expect("a String takes a write");
    debug_assert_eq!(link.capacity(), 3 * name.len() + 12, "the link outgrew its capacity");
    Ok(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a reader of the link does with the part before `#`: undo `%XX`. Strict, so a stray `%` or lower-case hex is a failure of the maker.
    fn decoded(encoded: &str) -> Vec<u8> {
        let bytes = encoded.as_bytes();
        let mut out = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            if bytes[at] == b'%' {
                let pair = &encoded[at + 1..at + 3];
                assert!(
                    pair.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b)),
                    "upper-case hex only: {encoded}"
                );
                out.push(u8::from_str_radix(pair, 16).expect("hex"));
                at += 3;
            } else {
                out.push(bytes[at]);
                at += 1;
            }
        }
        out
    }

    /// Failure: a name that starts `http:` makes a link a program reads as another scheme, so clicking it goes somewhere else. `:` is encoded.
    #[test]
    fn a_colon_in_a_name_never_becomes_a_scheme() {
        assert_eq!(page_link(b"http:evil.pdf", 1).as_deref(), Ok("http%3Aevil.pdf#page=1"));
    }

    /// Failure: a `#` or `%` in a name ends the name early or adds an open parameter, so the link opens another page than the one asked for.
    #[test]
    fn a_hash_or_percent_in_a_name_cant_change_the_page() {
        let link = page_link(b"a#page=9.pdf", 2).expect("a link");
        assert_eq!(link, "a%23page%3D9.pdf#page=2");
        assert_eq!(link.matches('#').count(), 1, "one fragment, ours");
        let (name, page) = link.split_once('#').expect("a fragment");
        assert_eq!((decoded(name), page), (b"a#page=9.pdf".to_vec(), "page=2"));
        assert_eq!(page_link(b"100%.pdf", 1).as_deref(), Ok("100%25.pdf#page=1"));
    }

    /// Failure: a byte comes out raw or in lower-case hex, so a name with `é`, a right-to-left mark or a newline makes a link that shows or reads as another
    /// name. Every byte but `/` and NUL is either unreserved or `%XX`, and the part before `#` decodes back to the bytes.
    #[test]
    fn every_byte_is_encoded_or_unreserved_and_decodes_back() {
        let mut cases: Vec<Vec<u8>> = (1u8..=255).filter(|byte| *byte != b'/').map(|byte| vec![b'a', byte]).collect();
        cases.push("é".as_bytes().to_vec());
        cases.push("a\u{202E}b.pdf".as_bytes().to_vec());
        cases.push(b"two\nlines.pdf".to_vec());
        for name in cases {
            let link = page_link(&name, 7).expect("a link");
            let (before, page) = link.split_once('#').expect("a fragment");
            assert_eq!(page, "page=7");
            assert!(
                before
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'%' | b'A'..=b'F')),
                "{link}"
            );
            assert_eq!(decoded(before), name, "{link}");
        }
    }

    /// Failure: a limit off by one: page 0 or one past the cap gets a link, or a name at the cap is refused, or one past it passes, or the longest link is
    /// longer than `MAX_LINK_BYTES`.
    #[test]
    fn page_and_name_limits_at_the_cap_and_one_past() {
        assert!(page_link(b"a.pdf", 1).is_ok() && page_link(b"a.pdf", MAX_PAGE).is_ok());
        assert_eq!(page_link(b"a.pdf", 0), Err(LinkError::Page));
        assert_eq!(page_link(b"a.pdf", MAX_PAGE + 1), Err(LinkError::Page));
        let longest = page_link(&[0xFF; MAX_NAME_BYTES], MAX_PAGE).expect("the longest name");
        assert!(longest.len() <= MAX_LINK_BYTES, "{}", longest.len());
        // Never grown past what it was made with: a String that reallocated has a capacity other than the one asked for.
        assert_eq!(longest.capacity(), MAX_LINK_BYTES, "the link was reallocated");
        assert_eq!(page_link(&[0xFF; MAX_NAME_BYTES + 1], 1), Err(LinkError::Name));
        for name in [&b""[..], b".", b"..", b"a/b.pdf", b"/", b"a\0b.pdf"] {
            assert_eq!(page_link(name, 1), Err(LinkError::Name), "{name:?}");
        }
        assert!(page_link(b"...pdf", 1).is_ok(), "only `.` and `..` alone are refused");
    }
}
