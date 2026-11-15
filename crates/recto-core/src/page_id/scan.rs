//! **Which images and forms a content stream draws:** the names it gives the `Do` operator, in the order they're first used, read from its decoded
//! bytes as they stream by for the digest ([`super`]). A small tokenizer, part of `recto-page/1`'s definition: what it takes for a `Do` and what it skips
//! decides which XObjects a page's digest holds.
//!
//! **The rules,** over the bytes of a page's content streams in order, a stream's end taken as whitespace:
//! - Whitespace is NUL, tab, line feed, form feed, carriage return and space; the delimiters are `( ) < > [ ] { } / %`.
//! - A comment runs from `%` to the end of its line. A string runs from `(` to its matching `)`, a backslash escaping the byte after it. A hex string
//!   runs from `<` to `>`. A dictionary is from `<<` to its matching `>>`, and an array from `[` to its matching `]`.
//! - A name is `/` and the regular bytes after it, each `#` and two hex digits taken as that byte. One longer than 127 bytes is no name here.
//! - A name directly before the operator `Do`, outside any dictionary or array, is a name drawn. Anything else between them (a number, a string, another
//!   operator) means it isn't.
//! - An inline image (`BI`, its dictionary, `ID`, a whitespace byte and its data) is skipped to a whitespace byte, `EI` and a whitespace byte or a
//!   delimiter.
//! - At most [`MAX_NAMES`] different names are kept; one more says the page draws more than it (a cap: the page has no identity).
//!
//! It holds at most [`MAX_NAMES`] names of 127 bytes and a few bytes of state, whatever it's given, and never fails: a mistake here can only change
//! which images a digest holds, never what the scan costs.

/// Most different names kept: the digest's cap on images and forms ([`super::Caps::xobjects`]).
pub(super) const MAX_NAMES: usize = 64;
/// Longest name taken, as PDF's own limit on names is.
const MAX_NAME: usize = 127;

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
    core: Core,
}

impl Default for DoScan {
    fn default() -> Self {
        Self {
            names: Vec::new(),
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

    fn keep_name(&mut self) {
        let name = &self.core.name_buf[..self.core.name_len.min(MAX_NAME)];
        if self.core.overflowed || self.names.iter().any(|kept| kept.as_slice() == name) {
            return;
        }
        if self.names.len() == MAX_NAMES {
            self.core.overflowed = true;
            return;
        }
        self.names.push(name.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_of(parts: &[&[u8]]) -> (Vec<String>, bool) {
        let mut scan = DoScan::default();
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
    /// scan keeps names without bound. At 64 names all kept; at 65 it says it overflowed, and keeps no more.
    #[test]
    fn the_names_are_capped_at_their_limit_plus_one() {
        let many = |count: usize| (0..count).map(|at| format!("/N{at} Do ")).collect::<String>();
        assert_eq!(names_of(&[many(64).as_bytes()]), ((0..64).map(|at| format!("N{at}")).collect(), false));
        let (names, overflowed) = names_of(&[many(65).as_bytes(), b"/N0 Do"]);
        assert_eq!((names.len(), overflowed), (64, true));
    }

    /// Failure caught: a stream that didn't decode (hashed as stored instead) leaves the names of its first part, so the digest depends on how far a
    /// decoder got before its error. Put back to its mark, the scan is as before the stream.
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
