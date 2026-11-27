//! Reading a link back: `rectopdf:///folder/name.pdf#page=N` (what Copy Link makes) or `file:///folder/name.pdf#page=N`, as a path and a page of the file.
//!
//! The inverse of [`address_of`](super::address_of): pure bytes in, a value out, with no I/O and no state. The text is hostile (a web page, a chat and
//! any program can hand one over), so it is bounded before anything else is done to it, a path can't climb (`..`, `%2E%2E`, `%2F`) or hold a NUL, and the
//! only verb a link has is "open this file at this page": a name it doesn't know is ignored, because nothing else could be done with it. An error never
//! holds or repeats any part of the text.

use super::{MAX_NAME_BYTES, MAX_PAGE, MAX_PATH_BYTES, MAX_READ_BYTES};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

/// Longest fragment (the part after `#`) read.
const MAX_FRAGMENT_BYTES: usize = 1024;

/// What a link names: a file and, when it says one, the page of the file as saved (counted from 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub path: PathBuf,
    pub file_page: Option<u32>,
}

/// Why a link can't be read. The words are for the person and the agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    /// Not a `rectopdf:` or `file:` link to a path.
    NotALink,
    /// The file location is malformed, climbs, or is too long.
    Path,
    /// The page is malformed or out of range.
    Page,
}

const NOT_A_LINK: &str = "This isn't a link to a PDF on this Mac. A link here starts with rectopdf:/// or file:/// and names a PDF.";
const PATH: &str = "This link's file location can't be read.";
const PAGE: &str = "This link's page can't be read. A page is a whole number from 1, as #page=12.";

impl AddressError {
    /// A short stable name, for a log or an answer's code.
    pub fn code(self) -> &'static str {
        match self {
            Self::NotALink => "not_a_link",
            Self::Path => "bad_path",
            Self::Page => "bad_page",
        }
    }

    /// The words, the same whoever sent the link. A constant: never any part of the link.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotALink => NOT_A_LINK,
            Self::Path => PATH,
            Self::Page => PAGE,
        }
    }
}

/// `text` as the file and page it names, in either form. See the module for what is refused.
pub fn read_address(text: &str) -> Result<Address, AddressError> {
    // First, so that a huge string costs one length check.
    if text.len() > MAX_READ_BYTES {
        return Err(AddressError::Path);
    }
    let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
    let after_scheme = strip_scheme(text).ok_or(AddressError::NotALink)?;
    let (before, fragment) = match after_scheme.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (after_scheme, None),
    };
    if before.contains('?') {
        return Err(AddressError::Path);
    }
    let path = read_path(before)?;
    let file_page = match fragment {
        Some(fragment) => read_page_fragment(fragment)?,
        None => None,
    };
    Ok(Address { path, file_page })
}

/// What follows `scheme://authority`, starting at the path's `/`, or `None` when the text isn't one of the two forms.
fn strip_scheme(text: &str) -> Option<&str> {
    let (scheme, rest) = text.split_once(':')?;
    let rest = rest.strip_prefix("//")?;
    let allowed_authority: &[&str] = if scheme.eq_ignore_ascii_case(super::SCHEME) {
        &[""]
    } else if scheme.eq_ignore_ascii_case("file") {
        &["", "localhost"]
    } else {
        return None;
    };
    let slash = rest.find('/')?;
    let authority = &rest[..slash];
    allowed_authority
        .iter()
        .any(|allowed| authority.eq_ignore_ascii_case(allowed))
        .then_some(&rest[slash..])
}

/// `/a/b%20c` as a path: each component decoded and checked.
fn read_path(path: &str) -> Result<PathBuf, AddressError> {
    let mut bytes: Vec<u8> = Vec::with_capacity(path.len());
    for component in path[1..].split('/') {
        let decoded = decode_component(component)?;
        if decoded.is_empty() || decoded == b"." || decoded == b".." || decoded.len() > MAX_NAME_BYTES || decoded.iter().any(|b| matches!(b, b'/' | 0)) {
            return Err(AddressError::Path);
        }
        bytes.push(b'/');
        bytes.extend_from_slice(&decoded);
        if bytes.len() > MAX_PATH_BYTES {
            return Err(AddressError::Path);
        }
    }
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
}

fn decode_component(component: &str) -> Result<Vec<u8>, AddressError> {
    let raw = component.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut at = 0;
    while at < raw.len() {
        match raw[at] {
            b'%' => {
                let pair = raw.get(at + 1..at + 3).ok_or(AddressError::Path)?;
                let hex = |b: u8| char::from(b).to_digit(16).ok_or(AddressError::Path);
                out.push((hex(pair[0])? * 16 + hex(pair[1])?) as u8);
                at += 3;
            }
            byte if byte < 0x20 || byte == 0x7F => return Err(AddressError::Path),
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    Ok(out)
}

/// The page a fragment (the part after `#`, without it) names: `page=N`, among any other `name` or `name=value` parts, which are ignored. `None` for none.
pub fn read_page_fragment(fragment: &str) -> Result<Option<u32>, AddressError> {
    if fragment.len() > MAX_FRAGMENT_BYTES {
        return Err(AddressError::Page);
    }
    let mut page = None;
    for part in fragment.split('&') {
        let (name, value) = part.split_once('=').map_or((part, None), |(name, value)| (name, Some(value)));
        if !name.eq_ignore_ascii_case("page") {
            continue;
        }
        let value = value.ok_or(AddressError::Page)?;
        if page.is_some() || value.is_empty() || value.len() > 10 || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AddressError::Page);
        }
        let number: u64 = value.parse().map_err(|_| AddressError::Page)?;
        if !(1..=u64::from(MAX_PAGE)).contains(&number) {
            return Err(AddressError::Page);
        }
        page = Some(number as u32);
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locator::address_of;
    use std::ffi::OsStr;
    use std::path::Path;

    fn read(text: &str) -> Result<Address, AddressError> {
        read_address(text)
    }

    /// Failure: Copy Link makes a link the app can't read back (an encoder and a decoder that drift apart), so a copied link opens another file or none:
    /// `%`, `#`, `?`, spaces, non-UTF-8 and decomposed names, the longest path, the first and last page, in both forms.
    #[test]
    fn reads_back_every_link_address_of_makes() {
        let long = {
            let mut path = String::new();
            while path.len() + 201 <= MAX_PATH_BYTES {
                path.push('/');
                path.push_str(&"\u{ff}".repeat(100));
            }
            let rest = MAX_PATH_BYTES - path.len();
            path.push('/');
            path.push_str(&"y".repeat(rest - 1));
            PathBuf::from(path)
        };
        let paths = [
            PathBuf::from("/Users/a b/100%/é#?.pdf"),
            Path::new("/d").join(OsStr::from_bytes(b"a\xFFb.pdf")),
            PathBuf::from("/d/cafe\u{301}.pdf"),
            PathBuf::from("/a.pdf"),
            long,
        ];
        for path in &paths {
            for page in [1, 2, MAX_PAGE] {
                let made = address_of(path, page).expect("an address");
                assert_eq!(
                    read(&made),
                    Ok(Address {
                        path: path.clone(),
                        file_page: Some(page)
                    }),
                    "{made}"
                );
                let as_file = made.replacen("rectopdf:", "file:", 1);
                assert_eq!(
                    read(&as_file),
                    Ok(Address {
                        path: path.clone(),
                        file_page: Some(page)
                    }),
                    "{as_file}"
                );
            }
        }
    }

    /// Failure: a link to something else (a web address, a mail address, a host, a name without a path) is taken for a file link and opens a path, or the
    /// scheme's case matters so a link a program upper-cased is refused.
    #[test]
    fn refuses_what_isnt_a_link() {
        for text in [
            "",
            "http://x/a.pdf",
            "mailto:a@b.c",
            "rectopdf://localhost/a.pdf",
            "rectopdf://Users/a.pdf",
            "rectopdf:/a.pdf",
            "rectopdf:a.pdf",
            "file://server/a.pdf",
            "file:x.pdf",
            "name.pdf#page=3",
            "rectopdf://",
            "rectopdf:///",
        ] {
            let got = read(text);
            if text == "rectopdf:///" {
                assert_eq!(got, Err(AddressError::Path), "{text:?}");
            } else {
                assert_eq!(got, Err(AddressError::NotALink), "{text:?}");
            }
        }
        for text in [
            "RECTOPDF:///A.pdf",
            "FILE:///A.pdf",
            "file://localhost/a.pdf",
            "File://LocalHost/a.pdf",
            "  rectopdf:///a.pdf\n",
        ] {
            assert!(read(text).is_ok(), "{text:?}");
        }
    }

    /// Failure: a link climbs out of its folder or smuggles a byte a path can't hold (`..`, an encoded `..` or `/`, NUL, an empty component, a query, a
    /// raw control byte, a bad escape), or a limit is off by one, so a hostile page names a file it should not.
    #[test]
    fn a_path_cannot_climb_or_smuggle() {
        let component_256 = "x".repeat(256);
        let path_1025 = format!("rectopdf://{}", "/xxxxxxxxx".repeat(103));
        let past_read = format!("rectopdf:///{}", "a".repeat(MAX_READ_BYTES));
        for text in [
            "rectopdf:///a/../b.pdf",
            "rectopdf:///a/%2E%2E/b.pdf",
            "rectopdf:///a/%2e%2e/b.pdf",
            "rectopdf:///a/./b.pdf",
            "rectopdf:///a%2Fb.pdf",
            "rectopdf:///a%00b.pdf",
            "rectopdf:///a//b.pdf",
            "rectopdf:///a/",
            "rectopdf:///a.pdf?q=1",
            "rectopdf:///a\nb.pdf",
            "rectopdf:///a%.pdf",
            "rectopdf:///a%G1.pdf",
            "rectopdf:///a%1",
            &format!("rectopdf:///{component_256}"),
            &path_1025,
            &past_read,
        ] {
            assert_eq!(read(text), Err(AddressError::Path), "{:?}", &text[..text.len().min(40)]);
        }
        assert!(path_1025.len() > MAX_PATH_BYTES + 11);
        assert!(read(&format!("rectopdf:///{}", "x".repeat(255))).is_ok());
    }

    /// Failure: a page part is read loosely (a sign, a dot, a space, zero, an overflow, a second `page`), so a link opens a page it didn't name, or an
    /// unknown name changes what the link does.
    #[test]
    fn the_page_part_is_strict_and_bounded() {
        for fragment in [
            "page=0",
            "page=",
            "page",
            "page=abc",
            "page=-1",
            "page=+3",
            "page=3.0",
            "page= 3",
            "page=200001",
            "page=99999999999999999999",
            "page=3&page=4",
        ] {
            assert_eq!(read_page_fragment(fragment), Err(AddressError::Page), "{fragment}");
        }
        assert_eq!(read_page_fragment(&"a".repeat(1025)), Err(AddressError::Page));
        for (fragment, page) in [
            ("page=3&zoom=100", Some(3)),
            ("zoom=100&page=7", Some(7)),
            ("rl=x&page=2", Some(2)),
            ("nameddest=x", None),
            ("", None),
            ("PAGE=5", Some(5)),
        ] {
            assert_eq!(read_page_fragment(fragment), Ok(page), "{fragment}");
        }
        assert_eq!(read_page_fragment("page=200000"), Ok(Some(200_000)));
        assert_eq!(read("rectopdf:///a.pdf#page=0"), Err(AddressError::Page));
        assert_eq!(read("rectopdf:///a.pdf").map(|a| a.file_page), Ok(None));
    }

    /// Failure: hex in lower case, or a pasted raw space or raw UTF-8, is refused, so a link a program rewrote can't be opened.
    #[test]
    fn lower_case_hex_and_raw_spaces_are_read() {
        assert_eq!(read("rectopdf:///a%2fb").map(|a| a.path), Err(AddressError::Path));
        assert_eq!(
            read("rectopdf:///a%c3%a9 b/é.pdf#page=2"),
            Ok(Address {
                path: PathBuf::from("/aé b/é.pdf"),
                file_page: Some(2)
            })
        );
    }

    /// Failure: an error's words hold part of the link, so a page can put its own text in front of the person. Every message is a constant.
    #[test]
    fn no_error_repeats_the_input() {
        for text in ["http://MARKER/x", "rectopdf:///MARKER/../x", "rectopdf:///MARKER.pdf#page=MARKER", "MARKER"] {
            if let Err(why) = read(text) {
                assert!(!why.message().contains("MARKER") && !why.code().contains("MARKER"));
            } else {
                panic!("{text} should be refused");
            }
        }
    }
}
