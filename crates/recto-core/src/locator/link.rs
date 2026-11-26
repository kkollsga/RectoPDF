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

/// Longest path, in bytes, [`address_of`] takes.
pub const MAX_PATH_BYTES: usize = 1024;
/// Longest address made: `file://`, every path byte as `%XX`, then `#page=` and the page's digits.
pub const MAX_ADDRESS_BYTES: usize = 7 + 3 * MAX_PATH_BYTES + 12;

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
    let mut link = String::with_capacity(3 * name.len() + 12);
    encode(&mut link, name);
    push_page(&mut link, file_page);
    debug_assert_eq!(link.capacity(), 3 * name.len() + 12, "the link outgrew its capacity");
    Ok(link)
}

/// `bytes` onto `out`: unreserved bytes as they are, every other as `%XX` in upper-case hex. The one encoder of a name or a path component.
fn encode(out: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
}

/// `#page=N`, with `file_page` already checked.
fn push_page(out: &mut String, file_page: u32) {
    use std::fmt::Write;
    out.push_str("#page=");
    write!(out, "{file_page}").expect("a String takes a write");
}

/// The whole file address, `file:///folder/name.pdf#page=N` (RFC 8089 with RFC 8118's open parameter), for `path` (absolute) and `file_page` (from 1).
/// Every component is encoded as [`page_link`] encodes a name, so a folder or file can't add a fragment, a query or a scheme. This is the person's
/// clipboard's alone: no agent is given it. A relative path, a `..` or empty component, a NUL, a component past [`MAX_NAME_BYTES`], the root alone or a path
/// past [`MAX_PATH_BYTES`] is [`LinkError::Name`].
pub fn address_of(path: &std::path::Path, file_page: u32) -> Result<String, LinkError> {
    use std::path::Component;
    if !(1..=MAX_PAGE).contains(&file_page) {
        return Err(LinkError::Page);
    }
    if !path.is_absolute() || path.as_os_str().len() > MAX_PATH_BYTES {
        return Err(LinkError::Name);
    }
    let mut address = String::with_capacity(MAX_ADDRESS_BYTES);
    address.push_str("file://");
    let mut parts = 0;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let bytes = name.as_encoded_bytes();
                if bytes.is_empty() || bytes.len() > MAX_NAME_BYTES || bytes.contains(&0) {
                    return Err(LinkError::Name);
                }
                address.push('/');
                encode(&mut address, bytes);
                parts += 1;
            }
            _ => return Err(LinkError::Name),
        }
    }
    if parts == 0 {
        return Err(LinkError::Name);
    }
    push_page(&mut address, file_page);
    debug_assert!(address.len() <= MAX_ADDRESS_BYTES);
    Ok(address)
}

/// The link to `file_page` (counted from 1) of the file at `path`, by the file's own last component and no folder: [`page_link`] over that name. A path
/// with no last component is [`LinkError::Name`].
pub fn link_of(path: &std::path::Path, file_page: u32) -> Result<String, LinkError> {
    page_link(path.file_name().map(|name| name.as_encoded_bytes()).unwrap_or_default(), file_page)
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

    /// Failure: a `#`, `?`, `%` or space in a folder or the file's name ends the path early or adds a parameter, so the address opens another file or page; or
    /// a byte is raw or in lower-case hex. The address holds exactly one `#`, ours, and each component decodes back.
    #[test]
    fn the_address_encodes_every_component_and_keeps_one_hash() {
        let address = address_of(std::path::Path::new("/Users/a b/100%/é#?.pdf"), 3).expect("an address");
        assert_eq!(address, "file:///Users/a%20b/100%25/%C3%A9%23%3F.pdf#page=3");
        assert_eq!(address.matches('#').count(), 1);
        let (path, page) = address.split_once('#').expect("a fragment");
        assert_eq!(page, "page=3");
        let parts: Vec<Vec<u8>> = path.strip_prefix("file://").expect("scheme").split('/').skip(1).map(decoded).collect();
        assert_eq!(parts, vec![b"Users".to_vec(), b"a b".to_vec(), b"100%".to_vec(), "é#?.pdf".as_bytes().to_vec()]);
    }

    /// Failure: the address's last component is encoded differently from the link's name, so the two forms of one file name disagree.
    #[test]
    fn the_last_component_is_encoded_as_the_links_name_is() {
        let name = "a:b#c é.pdf";
        let link = page_link(name.as_bytes(), 2).expect("a link");
        let address = address_of(&std::path::Path::new("/x").join(name), 2).expect("an address");
        assert_eq!(address, format!("file:///x/{link}"));
    }

    /// Failure: a name that isn't UTF-8 is refused or lossy-converted (U+FFFD), or a decomposed name (`e` and U+0301, as the file system holds it) is
    /// composed, so the address names another file than the one that's open. Each component's bytes are encoded as they are.
    #[test]
    fn a_non_utf8_and_a_decomposed_name_are_encoded_as_their_bytes_are() {
        use std::os::unix::ffi::OsStrExt;
        let raw = std::ffi::OsStr::from_bytes(b"a\xFFb.pdf");
        let address = address_of(&std::path::Path::new("/d").join(raw), 1).expect("an address");
        assert_eq!(address, "file:///d/a%FFb.pdf#page=1");
        let address = address_of(std::path::Path::new("/d/cafe\u{301}.pdf"), 1).expect("an address");
        assert_eq!(address, "file:///d/cafe%CC%81.pdf#page=1", "not composed to %C3%A9");
    }

    /// Failure: a path that can't be an address gets one anyway (a relative path opens another file, `..` leaves the folder, a NUL ends a name early), or a
    /// limit is off by one.
    #[test]
    fn the_address_refuses_what_cant_be_one_at_the_limits() {
        use std::path::Path;
        for path in ["a/b.pdf", "/a/../b.pdf", "/", "", "/a\u{0}b.pdf"] {
            assert_eq!(address_of(Path::new(path), 1), Err(LinkError::Name), "{path:?}");
        }
        let long = "x".repeat(MAX_NAME_BYTES);
        assert!(address_of(&Path::new("/").join(&long), 1).is_ok());
        assert_eq!(address_of(&Path::new("/").join(long + "x"), 1), Err(LinkError::Name));
        // A path of exactly MAX_PATH_BYTES is taken, one more is not; the longest address is within MAX_ADDRESS_BYTES.
        let mut path = String::new();
        while path.len() + 1 + 200 <= MAX_PATH_BYTES {
            path.push('/');
            path.push_str(&"\u{ff}".repeat(100));
        }
        let rest = MAX_PATH_BYTES - path.len();
        path.push('/');
        path.push_str(&"y".repeat(rest - 1));
        assert_eq!(path.len(), MAX_PATH_BYTES);
        let longest = address_of(Path::new(&path), MAX_PAGE).expect("the longest path");
        assert!(longest.len() <= MAX_ADDRESS_BYTES, "{}", longest.len());
        path.push('y');
        assert_eq!(address_of(Path::new(&path), 1), Err(LinkError::Name));
        assert_eq!(address_of(Path::new("/a.pdf"), 0), Err(LinkError::Page));
        assert_eq!(address_of(Path::new("/a.pdf"), MAX_PAGE + 1), Err(LinkError::Page));
    }
}
