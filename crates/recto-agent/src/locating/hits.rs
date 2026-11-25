//! References on search hits: each phrase or pattern hit's `chars` and `ref`, made as `recto ref --chars` makes them, and the answer's `references`
//! count of what could not be. Its own compartment: `search` calls [`HitRefs::mark`] once per hit and [`HitRefs::finish`] once, and knows nothing else.
//!
//! The cost of one search is bounded: the document's tags are read once (at most [`locator::TAG_PAGES`] pages), a page is read once for all its hits,
//! and the whole is under [`HIT_REF_CAPS`] (a page for each hit the search may show, and the tag pages). Characters are counted on from where the last
//! hit ended, so a page's work is proportional to the page's length, never to its hits times its length. Nothing is kept past the call: no cache, no
//! thread. The file is opened only when a hit first needs it, and its version is checked again at the end: a reference must name the version the pages
//! were read from. No token and no text is ever logged.

use super::{HIT_REFS_NOTE, HIT_REFS_WITHDRAWN};
use recto_core::locator::{self, CharacterRange, Locator, Referrer, ScanCaps, Target, Unreferenceable};
use recto_core::page_id;
use recto_platform::FileStamp;
use recto_session::identify::Opened;
use serde_json::{Value, json};
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

/// What one search may spend on references: the tag pages and a page for each hit it can show, and 256 MiB of page reads. A full answer of 200 hits
/// on 200 pages is referred to whole.
pub(crate) const HIT_REF_CAPS: ScanCaps = ScanCaps {
    bytes: 256 << 20,
    pages: locator::TAG_PAGES as u32 + crate::search::MAX_LIMIT,
    page: page_id::Caps::STANDARD,
};

/// Why a hit has no reference. The code is what `references.missing` counts it under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Why {
    /// The hit is on a page of an added PDF, which is not in the tab's file.
    AddedFile,
    /// A pattern's range does not begin and end on characters.
    NotOnCharacters,
    /// Nothing is left of the words once trailing whitespace is trimmed.
    Empty,
    /// The characters end past what a reference can hold.
    PastCharacterCap,
    File,
    Page(Unreferenceable),
}

impl Why {
    fn code(self) -> &'static str {
        match self {
            Self::AddedFile => "added_file",
            Self::NotOnCharacters => "not_on_characters",
            Self::Empty => "empty",
            Self::PastCharacterCap => "past_character_cap",
            Self::File => "file_unreadable",
            Self::Page(Unreferenceable::NoSuchPage) => "no_such_page",
            Self::Page(Unreferenceable::NoIdentity) => "no_identity",
            Self::Page(Unreferenceable::NoDocumentTag) => "no_document_tag",
            Self::Page(Unreferenceable::Capped) => "caps",
        }
    }
}

/// Where the character count stands in the text of one page.
#[derive(Default)]
struct Cursor {
    page: u32,
    /// The text it was counted in: its address and length, so a text that isn't the one counted never continues the count.
    text: (usize, usize),
    /// A byte offset on a character, and the characters before it.
    byte: usize,
    chars: u64,
}

pub(crate) struct HitRefs<'f> {
    path: Option<&'f Path>,
    /// The file's version when the search began. `None`: it could not be read.
    stamp: Option<FileStamp>,
    opened: &'f OnceCell<Option<Opened>>,
    caps: &'f ScanCaps,
    referrer: Option<Referrer<'f>>,
    cursor: Cursor,
    /// Characters stepped over, for the test that holds the count linear.
    walked: u64,
    made: u32,
    missing: BTreeMap<&'static str, u32>,
}

impl<'f> HitRefs<'f> {
    pub(crate) fn new(path: Option<&'f Path>, opened: &'f OnceCell<Option<Opened>>) -> Self {
        Self::with_caps(path, opened, &HIT_REF_CAPS)
    }

    pub(crate) fn with_caps(path: Option<&'f Path>, opened: &'f OnceCell<Option<Opened>>, caps: &'f ScanCaps) -> Self {
        Self {
            path,
            stamp: path.and_then(|path| recto_platform::file_stamp(path).ok()),
            opened,
            caps,
            referrer: None,
            cursor: Cursor::default(),
            walked: 0,
            made: 0,
            missing: BTreeMap::new(),
        }
    }

    /// Gives `hit` its `chars` and `ref`, or nulls and a count of why. `page` is the zero-based page of the file, `text` the page's text as the search
    /// read it and `range` the hit's bytes in it; `None` for another spelling, which gets a reference to its page and no characters.
    pub(crate) fn mark(&mut self, hit: &mut Value, added: bool, page: u32, text: &str, range: Option<&Range<usize>>) {
        hit["chars"] = Value::Null;
        hit["ref"] = Value::Null;
        match self.made_for(added, page, text, range) {
            Ok((chars, token)) => {
                if let Some((start, end)) = chars {
                    hit["chars"] = json!([start, end]);
                }
                hit["ref"] = json!(token);
                self.made += 1;
            }
            Err(why) => *self.missing.entry(why.code()).or_default() += 1,
        }
    }

    /// Checks free things first: the page is of the file, the range is on characters; then the file, then the page.
    fn made_for(&mut self, added: bool, page: u32, text: &str, range: Option<&Range<usize>>) -> Result<(Option<(u32, u32)>, String), Why> {
        if added {
            return Err(Why::AddedFile);
        }
        let counted = range.map(|range| self.count(page, text, range)).transpose()?;
        let whole = self.page(page)?;
        let Some((start, end, check)) = counted else {
            return Ok((None, locator::encode(&whole)));
        };
        let range = CharacterRange::new(start, end, check).map_err(|_| Why::PastCharacterCap)?;
        let made = Locator::new(whole.document(), whole.page(), Target::Characters(range));
        Ok((Some((start, end)), locator::encode(&made)))
    }

    /// A reference to the whole page, the file opened and its tags read when this is the first.
    fn page(&mut self, page: u32) -> Result<Locator, Why> {
        if self.referrer.is_none() {
            let (path, stamp) = self.path.zip(self.stamp).ok_or(Why::File)?;
            let opened = self.opened.get_or_init(|| Opened::open(path, stamp)).as_ref().ok_or(Why::File)?;
            self.referrer = Some(Referrer::new(opened.reader(), opened.map(), self.caps));
        }
        self.referrer.as_mut().ok_or(Why::File)?.page(page).map_err(Why::Page)
    }

    /// The range as characters `[start, end)` of the page's text and their check, trailing whitespace left out. Counted on from where the last hit
    /// on this text ended; from the start of the text only when this one begins earlier, or is on another page.
    fn count(&mut self, page: u32, text: &str, range: &Range<usize>) -> Result<(u32, u32, [u8; 8]), Why> {
        let held = text.get(range.clone()).ok_or(Why::NotOnCharacters)?;
        let kept = held.trim_end();
        if kept.is_empty() {
            return Err(Why::Empty);
        }
        let key = (text.as_ptr() as usize, text.len());
        if self.cursor.page != page || self.cursor.text != key || range.start < self.cursor.byte {
            self.cursor = Cursor {
                page,
                text: key,
                byte: 0,
                chars: 0,
            };
        }
        let before = self.cursor.chars + text[self.cursor.byte..range.start].chars().count() as u64;
        let inside = kept.chars().count() as u64;
        self.walked += before - self.cursor.chars + inside;
        self.cursor.byte = range.start + kept.len();
        self.cursor.chars = before + inside;
        let end = before + inside;
        if end > u64::from(locator::MAX_CHARACTER) {
            return Err(Why::PastCharacterCap);
        }
        Ok((before as u32, end as u32, locator::text_check(kept)))
    }

    /// The answer's `references`. If the file is not the version the search began with, no hit keeps a reference: they would name a version that is gone.
    pub(crate) fn finish(mut self, hits: &mut Value) -> Value {
        let changed = self
            .stamp
            .is_some_and(|stamp| self.path.is_none_or(|path| recto_platform::file_stamp(path).ok() != Some(stamp)));
        let mut note = HIT_REFS_NOTE;
        if changed {
            let all = hits.as_array_mut().map_or(0, |hits| {
                for hit in hits.iter_mut() {
                    hit["chars"] = Value::Null;
                    hit["ref"] = Value::Null;
                }
                hits.len()
            });
            self.made = 0;
            self.missing = BTreeMap::from([("file_changed", all as u32)]);
            note = HIT_REFS_WITHDRAWN;
        }
        json!({ "made": self.made, "missing": self.missing, "note": note })
    }
}

#[cfg(test)]
mod tests;
