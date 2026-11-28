//! **Running heads left out of `get`, the glue** (`recto_index::blocks::unit::heads` decides what is kept). A stored page's chrome is decided from its own stored edge pieces and its neighbours', by
//! `chrome::chrome_spans` through `StoredChrome`; a page read live takes the spans `text` marked it with. A page whose edges aren't stored is unknown, and is given whole and counted.

use crate::commands::getting::Form;
use recto_index::blocks::Kind;
use recto_index::blocks::markdown::{self, Piece};
use recto_index::blocks::unit::heads::{Heads, MAX_CHROME_CHARS_LEFT_OUT, State};
use recto_index::chrome::EdgeLine;
use recto_index::chrome::stored::StoredChrome;
use recto_index::corpus::ReadyCorpus;
use std::sync::Arc;

/// A corpus's pages' edges, asked for in the order pages are walked.
pub(super) type Ring = StoredChrome<Box<dyn FnMut(u32) -> Option<Vec<EdgeLine>>>>;

/// A page's chrome spans, `None` when they aren't known.
type Spans = Option<Vec<(u32, u32)>>;

/// What `get` of a page or a hit gives of the characters `[from, to)`: the ones that aren't chrome, up to the cap.
pub(super) struct Slice {
    pub shown: Vec<char>,
    /// The characters the unit has to give, left-out ones not counted.
    pub in_unit: usize,
    pub given: usize,
    /// Where it goes on, in the page's characters: after any stretch left out.
    pub cut: u32,
    pub hidden: Vec<(u32, u32)>,
    pub left_out: u32,
    /// The page's counts: unknown, over the cap.
    pub unknown: bool,
    pub over_cap: bool,
}

impl Slice {
    /// Puts the text in `untrusted` as asked, and gives where hidden text is in it (for text; null for Markdown, which marks it).
    pub(super) fn write(&self, form: Form, untrusted: &mut serde_json::Value) -> serde_json::Value {
        use serde_json::json;
        match form {
            Form::Text => {
                untrusted["text"] = json!(self.shown.iter().collect::<String>());
                json!(self.hidden.iter().map(|span| [span.0, span.1]).collect::<Vec<_>>())
            }
            Form::Markdown => {
                let piece = Piece {
                    kind: Kind::Paragraph,
                    level: 0,
                    depth: None,
                    text: &self.shown,
                    hidden: &self.hidden,
                };
                untrusted["markdown"] = json!(markdown::render(&[piece]));
                serde_json::Value::Null
            }
        }
    }

    /// The answer's `counts` for a page or a hit.
    pub(super) fn counts(&self) -> serde_json::Value {
        serde_json::json!({
            "chars_in_unit": self.in_unit, "chars_given": self.given, "hidden_spans": self.hidden.len(),
            "chrome_chars_left_out": self.left_out, "chrome_pages": u32::from(self.left_out > 0),
            "chrome_unknown_pages": u32::from(self.unknown), "chrome_over_cap_pages": u32::from(self.over_cap),
        })
    }
}

/// The slice of a page. A `hit` with characters is those characters, none of them left out.
pub(super) fn slice(read: &super::PageRead, chars: &[char], named: Option<(u32, u32)>, hit: bool, (from, to): (u32, u32), max: usize) -> Slice {
    let total = u32::try_from(chars.len()).unwrap_or(u32::MAX);
    let whole = hit && named.is_some();
    let heads = if whole {
        Heads::default()
    } else {
        Heads::of(read.chrome.as_deref(), total, named, MAX_CHROME_CHARS_LEFT_OUT)
    };
    let in_unit = heads.kept_len(from, to) as usize;
    let cut = heads.take(chars, &read.hidden, from, to, max);
    Slice {
        in_unit,
        given: cut.text.len(),
        cut: cut.cut,
        hidden: cut.hidden,
        left_out: cut.left_out,
        unknown: !whole && heads.state == State::Unknown,
        over_cap: heads.state == State::OverCap,
        shown: cut.text,
    }
}

/// What a blocks answer holds of chrome while it reads: the window over the stored edges, and the spans of the page read last, kept for the builder's next question about it.
pub(super) struct Chrome {
    ring: Ring,
    read: Option<(u32, Spans)>,
}

impl Chrome {
    pub(super) fn new(corpus: &Arc<ReadyCorpus>) -> Self {
        Self {
            ring: ring(corpus),
            read: None,
        }
    }

    /// What the stored edges say page `page` leaves out, `None` if not known.
    pub(super) fn spans(&mut self, page: u32) -> Option<Vec<(u32, u32)>> {
        self.ring.spans(page)
    }

    pub(super) fn keep(&mut self, page: u32, spans: Option<Vec<(u32, u32)>>) {
        self.read = Some((page, spans));
    }

    pub(super) fn take(&mut self, page: u32) -> Option<Vec<(u32, u32)>> {
        self.read.take().filter(|(read, _)| *read == page).and_then(|(_, spans)| spans)
    }
}

/// A page's edge buffer is given back past this many bytes.
const BIG_BUFFER: usize = 1 << 20;

/// The chrome window over a corpus: holds at most five pages' edges, and one page's bytes at a time in a buffer of its own, overwritten after each.
pub(super) fn ring(corpus: &Arc<ReadyCorpus>) -> Ring {
    let held = Arc::clone(corpus);
    let mut buffer: Vec<u8> = Vec::new();
    StoredChrome::new(
        corpus.pages(),
        Box::new(move |page| {
            let edges = held.page(page, &mut buffer).ok().and_then(|stored| stored.edges());
            zeroize::Zeroize::zeroize(&mut buffer);
            if buffer.capacity() > BIG_BUFFER {
                buffer = Vec::new();
            }
            edges
        }),
    )
}

#[cfg(test)]
mod tests;
