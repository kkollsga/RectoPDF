//! **Which images and forms a content stream draws:** the names it gives the `Do` operator, in the order they're first used, read from its decoded
//! bytes as they stream by for the digest ([`super`]). A small tokenizer, part of `recto-page/1`'s definition: what it takes for a `Do` and what it skips
//! decides which XObjects a page's digest holds.
//!
//! **The rules,** over the bytes of a page's content streams in order, a stream's end taken as whitespace:
//! - Whitespace is NUL, tab, line feed, form feed, carriage return and space; the delimiters are `( ) < > [ ] { } / %`.
//! - A comment runs from `%` to the end of its line. A string runs from `(` to its matching `)`, a backslash escaping the byte after it. A hex string
//!   runs from `<` to `>`. A dictionary is from `<<` to its matching `>>`, and an array from `[` to its matching `]`.
//! - A name is `/` and the regular bytes after it, each `#` and two hex digits taken as that byte. A `#` not followed by two hex digits is kept as it
//!   is, with what was read of the digits (lopdf ends a name there, so a lookup of such a name in the resources finds nothing: kind 2). One longer
//!   than 127 bytes is no name here.
//! - A name directly before the operator `Do`, outside any dictionary or array, is a name drawn. Anything else between them (a number, a string, another
//!   operator) means it isn't.
//! - An inline image (`BI`, its dictionary, `ID`, a whitespace byte and its data) is skipped to a whitespace byte, `EI` and a whitespace byte or a
//!   delimiter.
//! - At most [`MAX_NAMES`] (8,192) different names are kept ([`super::Caps::xobjects`]); one more says the page draws more than it (a cap: the page
//!   has no identity). It was 256 until 2026-11-25; see [`super`].
//!
//! It holds at most [`MAX_NAMES`] names of 127 bytes (about 1.2 MiB, each `Vec`'s header and its allocation counted), a table of their places (0.25 MiB) and
//! a few bytes of state, whatever it's given, and never fails; each byte costs a few steps, and a name drawn a hash and a comparison. Its promise: a
//! mistake here can only change which images are hashed, or whether
//! the page passes the names cap and so has no identity, never what the scan costs.

/// Most different names kept: the digest's cap on images and forms ([`super::Caps::xobjects`]).
pub(super) const MAX_NAMES: usize = 8192;
/// Longest name taken, as PDF's own limit on names is.
const MAX_NAME: usize = 127;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::{BuildHasher, RandomState};

/// Where the tokenizer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Between tokens, or in a regular token.
    Normal,
    Comment,
    /// In a string, with its depth of parentheses and whether the last byte was a backslash.
    String {
        depth: u32,
        escaped: bool,
    },
    /// After a `<`, which is a hex string or the start of a dictionary.
    Angle,
    HexString,
    /// After a `>` outside a hex string, which may end a dictionary.
    CloseAngle,
    /// In a name, at a `#` with this many hex digits read since.
    Name {
        hash: Option<(u8, u8)>,
    },
    /// In an inline image's data, with the last three bytes.
    InlineData([u8; 3]),
}

/// Where the scan is, but for the names it has kept: small, and copied whole for a [`Mark`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Core {
    overflowed: bool,
    state: State,
    /// Depth of dictionaries and arrays together: a name inside either is never `Do`'s.
    nesting: u32,
    /// Between `BI` and `ID`.
    inline_dict: bool,
    /// The last token was a name `Do` would take, kept at `name_buf[..name_len]`.
    operand_is_name: bool,
    name_buf: [u8; MAX_NAME],
    /// Bytes of the name being read, or [`MAX_NAME`] + 1 for one too long.
    name_len: usize,
    /// The first bytes of the regular token being read, enough to tell `Do`, `BI` and `ID`; `token_len` counts on past 2 (saturating) so a longer
    /// token is none of them.
    token: [u8; 2],
    token_len: u8,
}

/// The scan as it was, which a stream that didn't decode is put back to ([`DoScan::back_to`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mark {
    names: usize,
    core: Core,
}

/// The names a page's content gives `Do`, in order of first use.
#[derive(Clone, Debug)]
pub(super) struct DoScan {
    names: Vec<Vec<u8>>,
    /// The place in `names` of each name kept, by a keyed hash of it (the key is random for each scan, so no input can make names collide on
    /// purpose): looking a name up, and keeping one, costs a hash and one comparison however many are kept, where a sorted `Vec`'s insert moved up to
    /// 16 KiB each time (64 MiB for 8,192 names, once for every failing form and every content stream put back). Holds no second copy of a name: at
    /// most 8,192 entries of 16 bytes, about 0.25 MiB with the table's slack.
    places: HashMap<u64, u16>,
    /// The places of names whose hash an earlier name has (in `places`): empty but for a 64-bit coincidence, and looked through whole when not.
    collided: Vec<u16>,
    hasher: RandomState,
    /// Which bits of the hash count: all, but for a test that makes every name collide.
    hash_mask: u64,
    /// Most different names kept ([`MAX_NAMES`] unless a test holds it small), at most `u16::MAX`, which a place fits.
    cap: usize,
    core: Core,
}

impl Default for DoScan {
    fn default() -> Self {
        Self::with_cap(MAX_NAMES)
    }
}

impl DoScan {
    /// A scan that keeps at most `cap` different names ([`super::Caps::xobjects`]), clamped to `u16::MAX`.
    pub(super) fn with_cap(cap: usize) -> Self {
        Self {
            names: Vec::new(),
            places: HashMap::new(),
            collided: Vec::new(),
            hasher: RandomState::new(),
            hash_mask: u64::MAX,
            cap: cap.min(usize::from(u16::MAX)),
            core: Core {
                overflowed: false,
                state: State::Normal,
                nesting: 0,
                inline_dict: false,
                operand_is_name: false,
                name_buf: [0; MAX_NAME],
                name_len: 0,
                token: [0; 2],
                token_len: 0,
            },
        }
    }
}

fn is_white(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

fn is_delimiter(byte: u8) -> bool {
    matches!(byte, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl DoScan {
    /// The names drawn so far, in order of first use.
    pub(super) fn names(&self) -> &[Vec<u8>] {
        &self.names
    }

    /// Whether more different names were drawn than [`MAX_NAMES`].
    pub(super) fn overflowed(&self) -> bool {
        self.core.overflowed
    }

    /// Where it is now, to come back to if what follows is taken back.
    pub(super) fn mark(&self) -> Mark {
        Mark {
            names: self.names.len(),
            core: self.core,
        }
    }

    /// Back to `mark`: what was fed since is as if it never was.
    pub(super) fn back_to(&mut self, mark: Mark) {
        for at in mark.names..self.names.len() {
            let hash = self.hash_of(&self.names[at]);
            if self.places.get(&hash).is_some_and(|place| usize::from(*place) == at) {
                self.places.remove(&hash);
            }
        }
        // A name that came after the one holding its hash came after the mark when that one did.
        self.collided.retain(|place| usize::from(*place) < mark.names);
        self.names.truncate(mark.names);
        self.core = mark.core;
    }

    /// A stream's end: whitespace, between two streams of one page.
    pub(super) fn end_of_stream(&mut self) {
        self.feed(b" ");
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.byte(byte);
        }
    }

    fn byte(&mut self, byte: u8) {
        match self.core.state {
            State::Comment => {
                if byte == b'\n' || byte == b'\r' {
                    self.core.state = State::Normal;
                }
            }
            State::String { depth, escaped } => {
                self.core.state = match (escaped, byte) {
                    (true, _) => State::String { depth, escaped: false },
                    (false, b'\\') => State::String { depth, escaped: true },
                    (false, b'(') => State::String {
                        depth: depth.saturating_add(1),
                        escaped: false,
                    },
                    (false, b')') if depth == 0 => State::Normal,
                    (false, b')') => State::String {
                        depth: depth - 1,
                        escaped: false,
                    },
                    _ => State::String { depth, escaped: false },
                };
            }
            State::Angle => {
                if byte == b'<' {
                    self.core.nesting = self.core.nesting.saturating_add(1);
                    self.core.state = State::Normal;
                } else {
                    self.core.state = State::HexString;
                    self.byte(byte);
                }
            }
            State::HexString => {
                if byte == b'>' {
                    self.core.state = State::Normal;
                }
            }
            State::CloseAngle => {
                self.core.state = State::Normal;
                if byte == b'>' {
                    self.core.nesting = self.core.nesting.saturating_sub(1);
                } else {
                    self.byte(byte);
                }
            }
            State::Name { hash } => self.name_byte(byte, hash),
            State::InlineData(last) => {
                let ended = is_white(last[0]) && last[1] == b'E' && last[2] == b'I' && (is_white(byte) || is_delimiter(byte));
                if ended {
                    self.core.state = State::Normal;
                    self.core.inline_dict = false;
                    self.byte(byte);
                } else {
                    self.core.state = State::InlineData([last[1], last[2], byte]);
                }
            }
            State::Normal => self.normal_byte(byte),
        }
    }

    fn normal_byte(&mut self, byte: u8) {
        if is_white(byte) || is_delimiter(byte) {
            self.end_token();
            if self.core.state != State::Normal {
                // `ID` began an inline image's data, which this byte was the first of.
                return;
            }
        }
        if is_white(byte) {
            return;
        }
        match byte {
            b'%' => self.core.state = State::Comment,
            b'(' => {
                self.core.operand_is_name = false;
                self.core.state = State::String { depth: 0, escaped: false };
            }
            b'<' => {
                self.core.operand_is_name = false;
                self.core.state = State::Angle;
            }
            b'>' => {
                self.core.operand_is_name = false;
                self.core.state = State::CloseAngle;
            }
            b'[' => {
                self.core.operand_is_name = false;
                self.core.nesting = self.core.nesting.saturating_add(1);
            }
            b']' => {
                self.core.operand_is_name = false;
                self.core.nesting = self.core.nesting.saturating_sub(1);
            }
            b'{' | b'}' | b')' => self.core.operand_is_name = false,
            b'/' => {
                self.core.operand_is_name = false;
                self.core.name_len = 0;
                self.core.state = State::Name { hash: None };
            }
            _ => {
                if usize::from(self.core.token_len) < self.core.token.len() {
                    self.core.token[usize::from(self.core.token_len)] = byte;
                }
                self.core.token_len = self.core.token_len.saturating_add(1);
            }
        }
    }

    /// A regular token has ended: an operator, a number or a keyword.
    fn end_token(&mut self) {
        if self.core.token_len == 0 {
            return;
        }
        let token = (self.core.token_len == 2).then_some(self.core.token);
        self.core.token_len = 0;
        match token.as_ref().map(<[u8; 2]>::as_slice) {
            Some(b"Do") if self.core.operand_is_name && self.core.nesting == 0 && !self.core.inline_dict => self.keep_name(),
            Some(b"BI") => self.core.inline_dict = true,
            Some(b"ID") if self.core.inline_dict => {
                // The whitespace byte after `ID` is the one that ends this token; the data starts after it.
                self.core.state = State::InlineData([b'x'; 3]);
            }
            _ => {}
        }
        self.core.operand_is_name = false;
    }

    fn name_byte(&mut self, byte: u8, hash: Option<(u8, u8)>) {
        if let Some((seen, high)) = hash {
            match (seen, hex_value(byte)) {
                (0, Some(value)) => {
                    self.core.state = State::Name { hash: Some((1, value)) };
                    return;
                }
                (1, Some(value)) => {
                    self.push_name_byte((high << 4) | value);
                    self.core.state = State::Name { hash: None };
                    return;
                }
                // A `#` not followed by two hex digits is itself, with what was read of the digits.
                (seen, _) => {
                    self.push_name_byte(b'#');
                    if seen == 1 {
                        self.push_name_byte(if high < 10 { b'0' + high } else { b'a' + high - 10 });
                    }
                    self.core.state = State::Name { hash: None };
                }
            }
        }
        if is_white(byte) || is_delimiter(byte) {
            self.core.state = State::Normal;
            self.core.operand_is_name = self.core.name_len <= MAX_NAME && !self.core.inline_dict && self.core.nesting == 0;
            self.byte(byte);
            return;
        }
        if byte == b'#' {
            self.core.state = State::Name { hash: Some((0, 0)) };
        } else {
            self.push_name_byte(byte);
        }
    }

    fn push_name_byte(&mut self, byte: u8) {
        if self.core.name_len < MAX_NAME {
            self.core.name_buf[self.core.name_len] = byte;
        }
        self.core.name_len = self.core.name_len.saturating_add(1).min(MAX_NAME + 1);
    }

    fn hash_of(&self, name: &[u8]) -> u64 {
        self.hasher.hash_one(name) & self.hash_mask
    }

    fn keep_name(&mut self) {
        let name = &self.core.name_buf[..self.core.name_len.min(MAX_NAME)];
        if self.core.overflowed {
            return;
        }
        let hash = self.hash_of(name);
        let same = |place: &u16| self.names[usize::from(*place)].as_slice() == name;
        if self.places.get(&hash).is_some_and(same) || self.collided.iter().any(same) {
            return;
        }
        if self.names.len() >= self.cap {
            self.core.overflowed = true;
            return;
        }
        // At most `cap` (clamped to u16::MAX) names, so a place always fits.
        let place = u16::try_from(self.names.len()).unwrap_or(u16::MAX);
        match self.places.entry(hash) {
            Entry::Vacant(slot) => {
                slot.insert(place);
            }
            Entry::Occupied(_) => self.collided.push(place),
        }
        self.names.push(name.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_of(parts: &[&[u8]]) -> (Vec<String>, bool) {
        names_of_capped(MAX_NAMES, parts)
    }

    fn names_of_capped(cap: usize, parts: &[&[u8]]) -> (Vec<String>, bool) {
        let mut scan = DoScan::with_cap(cap);
        for part in parts {
            scan.feed(part);
            scan.end_of_stream();
        }
        (
            scan.names().iter().map(|name| String::from_utf8_lossy(name).into_owned()).collect(),
            scan.overflowed(),
        )
    }

    /// Failure caught: an image the content draws isn't taken (so a page differing only in it shares a digest), or something that isn't a `Do` is
    /// taken (so a page's digest holds an image it never draws, and changes when that image does). Each rule of the module doc.
    #[test]
    fn what_is_drawn_and_only_that() {
        assert_eq!(names_of(&[b"q /Im1 Do Q /Fm2 Do /Im1 Do"]).0, ["Im1", "Fm2"], "first use, once");
        assert_eq!(names_of(&[b"/Im#31 Do /A#2 Do"]).0, ["Im1", "A#2"], "#hh is its byte, a lone # itself");
        assert_eq!(
            names_of(&[b"/Im1 5 Do /Im2 (x) Do /Im3 gs Do"]).0,
            Vec::<String>::new(),
            "only a name right before Do"
        );
        assert_eq!(names_of(&[b"(/Im1 Do) <2F496D31> /Im2 Do"]).0, ["Im2"], "strings and hex strings skipped");
        assert_eq!(names_of(&[b"(a \\) /Im1 Do (b)) /Im2 Do"]).0, ["Im2"], "escapes and nested parentheses");
        assert_eq!(names_of(&[b"% /Im1 Do\n/Im2 Do"]).0, ["Im2"], "comments to their line's end");
        assert_eq!(
            names_of(&[b"/P << /N /Im1 Do >> BDC [/Im2 Do] /Im3 Do"]).0,
            ["Im3"],
            "dictionaries and arrays skipped"
        );
        assert_eq!(
            names_of(&[b"BI /W 1 /H 1 ID \x00/Im1 Do EI\xff EI /Im2 Do"]).0,
            ["Im2"],
            "an inline image's data skipped to EI"
        );
        assert_eq!(
            names_of(&[b"/Im1", b"Do /Im2\nDo"]).0,
            ["Im1", "Im2"],
            "a stream's end is whitespace between tokens"
        );
        assert_eq!(names_of(&[b"/Im1Do /ImX DoX /Im2 Do"]).0, ["Im2"], "tokens are whole");
        let long = format!("/{} Do /Ok Do", "x".repeat(128));
        assert_eq!(names_of(&[long.as_bytes()]).0, ["Ok"], "a name past 127 bytes is no name");
        let most = format!("/{} Do", "y".repeat(127));
        assert_eq!(names_of(&[most.as_bytes()]).0.len(), 1, "127 bytes is one");
    }

    /// Failure caught: a page drawing more different names than the cap is hashed by some of them, so its digest depends on where the cap fell, or the
    /// scan keeps names without bound. With the cap held at 4 (what's proved is the exactness at the limit, not the value): 4 names all kept; at 5 it says
    /// it overflowed, and keeps no more. A cap past `u16::MAX` is clamped, so a name's place never wraps.
    #[test]
    fn the_names_are_capped_at_their_limit_plus_one() {
        let many = |count: usize| (0..count).map(|at| format!("/N{at} Do ")).collect::<String>();
        assert_eq!(names_of_capped(4, &[many(4).as_bytes()]), ((0..4).map(|at| format!("N{at}")).collect(), false));
        let (names, overflowed) = names_of_capped(4, &[many(5).as_bytes(), b"/N0 Do"]);
        assert_eq!((names.len(), overflowed), (4, true));
        assert_eq!(
            DoScan::with_cap(usize::MAX).cap,
            usize::from(u16::MAX),
            "a cap past what a place holds is clamped"
        );
    }

    /// Failure caught: a stream that didn't decode (hashed as stored instead) leaves the names of its first part, so the digest depends on how far a
    /// decoder got before its error; or names taken back stay in the lookup of names seen, so one drawn again after is dropped. Put back to its mark,
    /// the scan is as before the stream.
    #[test]
    fn a_stream_taken_back_leaves_nothing() {
        let mut scan = DoScan::default();
        scan.feed(b"/A Do (open");
        let mark = scan.mark();
        scan.feed(b") /B Do /C");
        scan.back_to(mark);
        scan.feed(b") /D Do");
        scan.end_of_stream();
        assert_eq!(scan.names(), [b"A".to_vec(), b"D".to_vec()]);
        // A name taken back is a name not seen: drawn again, it's kept again, and a name kept stays kept once.
        scan.feed(b"/B Do /A Do /B Do");
        scan.end_of_stream();
        assert_eq!(scan.names(), [b"A".to_vec(), b"D".to_vec(), b"B".to_vec()]);
    }

    /// Failure caught: names whose hashes are equal (a coincidence of 64 bits, made here by keeping none of them) are taken for one another, so a page
    /// drawing both images hashes one, or a name put back with its stream is still taken as seen. With every name colliding, the names, their order,
    /// the overflow at the cap and the put-back are those of the scan with no collisions.
    #[test]
    fn names_that_collide_are_still_told_apart() {
        let run = |mask: u64| {
            let mut scan = DoScan::with_cap(6);
            scan.hash_mask = mask;
            scan.feed(b"/A Do /B Do /A Do ");
            let mark = scan.mark();
            scan.feed(b"/C Do /D Do /B Do ");
            scan.back_to(mark);
            scan.feed(b"/D Do /C Do /B Do /D Do /E Do /A Do /F Do /G Do /H Do ");
            scan.end_of_stream();
            (scan.names().to_vec(), scan.overflowed())
        };
        let (names, overflowed) = run(u64::MAX);
        assert_eq!(names.len(), 6, "kept to the cap");
        assert!(overflowed);
        assert_eq!(run(0), (names.clone(), overflowed));
        assert_eq!(
            names[..4],
            [b"A".to_vec(), b"B".to_vec(), b"D".to_vec(), b"C".to_vec()],
            "taken back, and kept again in the order drawn"
        );
    }

    /// Failure caught: hostile bytes panic the scan or make it hold more than its bound. Every byte value, deep nesting, an unclosed string and an
    /// inline image without its end.
    #[test]
    fn hostile_bytes_cost_nothing() {
        let all: Vec<u8> = (0..=255u8).cycle().take(1 << 16).collect();
        let mut scan = DoScan::default();
        scan.feed(&all);
        scan.feed(&[b'['; 1 << 12]);
        scan.feed(&[b'('; 1 << 12]);
        scan.feed(b"BI ID ");
        scan.feed(&all);
        assert!(scan.names().len() <= MAX_NAMES);
    }
}
