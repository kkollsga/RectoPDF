//! `ref` and `resolve`: a reference to a page, and where that page is now (`recto_core::locator`). This is the executor: the tab, its file, the core, and
//! the answer's words. The session is asked once, inside `tab_file`, for the tab's facts; everything after reads the file with the session free. A
//! token is hostile input and is never echoed, logged or kept: an answer carries only numbers and constant words.

#[cfg(test)]
mod at_tests;
mod hits;
pub(crate) use hits::HitRefs;

use crate::reading::{TabFile, tab_file};
use crate::{AgentError, Code, SessionAccess, TabRef, exit};
use recto_core::locator::{
    self, CharacterRange, DocumentMatch, Locator, Looked, MAX_CANDIDATES, Placed, Resolution, ScanCaps, Status, Target, TextMatch, Unreferenceable,
};
use recto_session::identify::Opened;
use serde_json::{Value, json};

/// The refusal for a file the tab can't be read as pages from, in constant words.
const RENDERER_ONLY: &str = "Only the renderer can read this file, so its pages have no identity to point at.";
const CHANGED_WHILE_READ: &str = "The file changed while it was being read. Try again.";
const NO_IDENTITY: &str = "That page can't be told apart from the others, so it can't be pointed at.";
const NO_DOCUMENT_TAG: &str = "This document has nothing to say which document it is, so a reference to it would mean nothing.";
const ADDED_PAGE: &str = "That page was added from another PDF and isn't in this file yet. Save first, or point at a page of the file.";

/// The refusal for characters past the end of the page's text, or no characters at all, in constant words.
const CHARS_PAST_TEXT: &str = "The page's text doesn't reach that far, so there are no such characters to point at. `recto text` gives the page's text.";
/// The refusal for a `--chars` that can't be a range of characters.
pub(crate) const CHARS_USAGE: &str = "`--chars` is `START-END` in characters of `recto text`, with the end after the start, not included, and at most 2097152.";
const UNRESOLVED_TEXT_CHANGED: &str = "The page is there, but its text at those characters isn't what the reference was made of.";

/// What a search answer says of the references on its hits.
pub(crate) const HIT_REFS_NOTE: &str = "Each hit's `ref` points at its words, as `recto ref --chars` makes one, for `recto resolve` and `--ref`, and `chars` are those characters in `recto text`'s text of the page. A ranked hit's points at the word its line shows. Another spelling gets a reference to its page. `missing` counts the hits with none, by why.";
pub(crate) const HIT_REFS_WITHDRAWN: &str = "The file changed during the search, so no hit has a reference: it would name a version that's gone. A hit's `untrusted.link` stays, but it names a page number only, and that page may now hold other words. Search again.";

const TAB_CHANGED: &str = "The tab changed while the reference was being found. Try again.";
const BAD_REFERENCE: &str = "That isn't a reference RectoPDF made.";
const REFERENCE_TOO_NEW: &str = "That reference was made by a newer RectoPDF than this one.";
const USE_REFERENCE_WHOLE: &str = "Use the `ref` that `recto ref` gave, whole.";

/// The words for a reference that can't be read, never the reference itself.
pub(crate) fn bad_reference() -> AgentError {
    AgentError::new(Code::BAD_REFERENCE, BAD_REFERENCE, exit::USAGE)
}

/// The words for a reference made by a newer RectoPDF.
pub(crate) fn reference_too_new() -> AgentError {
    AgentError::new(Code::REFERENCE_TOO_NEW, REFERENCE_TOO_NEW, exit::USAGE)
}

/// A reference as typed after `recto resolve` or in `--ref`, read, or the words for why it can't be. Constant words: the token is hostile and may be long,
/// so it is never repeated. What the table checks a value of `Kind::Reference` by.
pub(crate) fn read_reference(token: &str) -> Result<Locator, String> {
    locator::decode(token).map_err(|error| match error {
        locator::Error::UnsupportedVersion => REFERENCE_TOO_NEW.to_string(),
        _ => format!("{BAD_REFERENCE} {USE_REFERENCE_WHOLE}"),
    })
}

/// How a resolution says whether the file is the document the reference was made in.
fn document_word(document: DocumentMatch) -> &'static str {
    match document {
        DocumentMatch::Same => "same",
        DocumentMatch::Different => "different",
        DocumentMatch::Unknown => "unknown",
    }
}

fn not_referenceable(words: &'static str) -> AgentError {
    AgentError::new(Code::NOT_REFERENCEABLE, words, exit::FAILED)
}

/// `recto ref <page>`: a reference to page `page` (counted from 1 in the tab as it is now, unsaved edits included), or with `chars` to characters `[start, end)` of
/// its text, checked against that text as `recto text` reads it.
pub(crate) fn reference<A: SessionAccess>(access: &A, tab: &TabRef, page: u32, chars: Option<(u32, u32)>) -> Result<Value, AgentError> {
    reference_reading(access, tab, page, chars, crate::render::page_text)
}

/// A page's text as the reader gives it: `render::page_text` in production, a made one where a test needs the read to fail the way it can, without the
/// page that makes it fail.
type PageTextReader = fn(&std::path::Path, u32) -> Result<String, AgentError>;

fn reference_reading<A: SessionAccess>(access: &A, tab: &TabRef, page: u32, chars: Option<(u32, u32)>, read_text: PageTextReader) -> Result<Value, AgentError> {
    // A page that can't be is refused before the file is opened or its state said.
    let wanted = crate::one_based(page)?;
    let (file, opened, stamp) = readable(access, tab)?;
    // The file's page that stands at this position now. A position with none, inside the tab, is a page added from another PDF.
    let Some(file_page) = file.file_page_at(wanted) else {
        return Err(if wanted < file.pages() {
            not_referenceable(ADDED_PAGE)
        } else {
            crate::no_such_page()
        });
    };
    let made = locator::reference(opened.reader(), opened.map(), file_page, &ScanCaps::STANDARD);
    // The mapped file is let go before the renderer reads the page's text.
    drop(opened);
    unchanged(&file, stamp)?;
    let made = made.map_err(|why| match why {
        Unreferenceable::NoSuchPage => crate::no_such_page(),
        Unreferenceable::NoIdentity | Unreferenceable::Capped => not_referenceable(NO_IDENTITY),
        Unreferenceable::NoDocumentTag => not_referenceable(NO_DOCUMENT_TAG),
        Unreferenceable::Unreadable => AgentError::new(Code::FILE_UNREADABLE, "The tab's file can't be read just now.", exit::FAILED),
    })?;
    let Some((start, end)) = chars else {
        return Ok(json!({ "tab": file.id(), "page": page, "file_page": file_page + 1, "ref": locator::encode(&made) }));
    };
    let text = read_text(file.path(), file_page)?;
    let range = locator::characters(&text, start, end).map_err(|_| not_referenceable(CHARS_PAST_TEXT))?;
    drop(text);
    unchanged(&file, stamp)?;
    let made = Locator::new(made.document(), made.page(), Target::Characters(range));
    Ok(json!({ "tab": file.id(), "page": page, "file_page": file_page + 1, "chars": [start, end], "ref": locator::encode(&made) }))
}

/// What a resolution comes to once the page's text is looked at: the core's status, except that a page found with other text at the characters a reference
/// names is `TextChanged`, whichever way it was found. Decided once, here, so `resolve`, `page_of` and the note can't disagree.
enum Outcome {
    Status(Status),
    TextChanged { page: u32 },
}

/// The file's resolution of a reference, and for a character target on a page that was found, whether its text is the text it was made of. The text is read
/// with the session free, and the file's version is checked after that read as well. With no reader nothing is read: the caller checks the characters
/// against a text it has itself ([`CharsChecked::AgainstItsText`]).
fn resolved<A: SessionAccess>(
    access: &A,
    tab: &TabRef,
    reference: &Locator,
    read_text: Option<PageTextReader>,
) -> Result<(TabFile, Resolution, Outcome, Option<TextMatch>, recto_platform::FileStamp), AgentError> {
    let (file, opened, stamp) = readable(access, tab)?;
    let found = locator::resolve(reference, opened.reader(), opened.map(), &ScanCaps::STANDARD);
    drop(opened);
    unchanged(&file, stamp)?;
    let text = match (reference.target(), &found.status, read_text) {
        (Target::Characters(range), Status::Found { page } | Status::Moved { page }, Some(read_text)) => {
            let held = locator::check_characters(range, &read_text(file.path(), *page)?);
            unchanged(&file, stamp)?;
            Some(held)
        }
        _ => None,
    };
    let outcome = match (&found.status, text) {
        (Status::Found { page } | Status::Moved { page }, Some(TextMatch::Changed)) => Outcome::TextChanged { page: *page },
        (status, _) => Outcome::Status(status.clone()),
    };
    Ok((file, found, outcome, text, stamp))
}

/// `recto resolve <ref>`: where the reference's page is in this tab now. A page that moved, is gone, can't be told from others or wasn't fully looked
/// for is an answer (exit 0), never a guess.
pub(crate) fn resolve<A: SessionAccess>(access: &A, tab: &TabRef, reference: Locator) -> Result<Value, AgentError> {
    resolve_reading(access, tab, reference, crate::render::page_text)
}

fn resolve_reading<A: SessionAccess>(access: &A, tab: &TabRef, reference: Locator, read_text: PageTextReader) -> Result<Value, AgentError> {
    let (file, found, outcome, text, _) = resolved(access, tab, &reference, Some(read_text))?;
    // A page of the file, as the tab shows it now (from 1): its position among the unsaved edits, or none when they removed it.
    let place = |file_page: u32| -> Option<u32> { file.place_of(file_page).map(|place| place + 1) };
    let both = |file_page: u32| json!({ "page": place(file_page), "file_page": file_page + 1 });
    let (status, at, listed) = match &outcome {
        Outcome::TextChanged { page } => ("text_changed", Some(*page), &[][..]),
        Outcome::Status(Status::Found { page }) => ("found", Some(*page), &[][..]),
        Outcome::Status(Status::Moved { page }) => ("moved", Some(*page), &[][..]),
        Outcome::Status(Status::Ambiguous { pages, .. }) => ("ambiguous", None, pages.as_slice()),
        Outcome::Status(Status::Gone) => ("gone", None, &[][..]),
        Outcome::Status(Status::Incomplete { pages }) => ("incomplete", None, pages.as_slice()),
    };
    let was = reference.page().number();
    let target = found.target.map(|placed| target_json(&placed, text));
    let note = note_of(&found, text, at.map(place), was, file.has_edit_list());
    Ok(json!({
        "tab": file.id(),
        "status": status,
        "page": at.and_then(place),
        "file_page": at.map(|page| page + 1),
        "was_page": was,
        "candidates": listed.iter().map(|page| both(*page)).collect::<Vec<_>>(),
        "document": document_word(found.document),
        "frame": found.frame_same.map(|same| if same { "same" } else { "changed" }),
        "target": target,
        "looked_at": match found.looked {
            Looked::Hint => "hint",
            Looked::All => "all",
            Looked::Part => "part",
        },
        "pages_read": found.pages_read,
        "note": note,
    }))
}

const UNRESOLVED_GONE: &str = "That page is gone from this file: every page was read and none is it.";
const UNRESOLVED_AMBIGUOUS: &str = "More than one page has that content, so it can't say which one was meant.";
const UNRESOLVED_INCOMPLETE: &str = "It stopped before every page was read, so it can't say whether that page is here.";
const UNRESOLVED_REMOVED: &str = "Unsaved edits have removed that page from the tab.";

/// A page found by a reference, as the tab has it now: what `text` and `render` go on to read.
pub(crate) struct Located {
    /// The tab it was found in. The command goes on in this tab whatever `--tab` named, so "the active tab" moving while the read is under way changes
    /// nothing, as it changes nothing for a page given by number.
    pub tab: recto_session::TabId,
    /// Its place in the tab, counted from 1, unsaved edits included.
    pub page: u32,
    /// Its page of the file on disk, counted from 0.
    pub file_page: u32,
    /// The file's version it was found in.
    pub stamp: recto_platform::FileStamp,
    /// What the answer says of how it was found: constant words and numbers, never the token.
    pub said: Value,
    /// The characters the reference names, when they are still to be checked against the text of the command's own answer ([`attach`] does it).
    unchecked: Option<CharacterRange>,
}

#[cfg(test)]
impl Located {
    /// **For a test:** a page found in a tab, with the characters its reference names (not yet checked), as a resolved reference is.
    pub(crate) fn for_a_test(tab: recto_session::TabId, file_page: u32, stamp: recto_platform::FileStamp, chars: Option<CharacterRange>) -> Self {
        Self {
            tab,
            page: file_page + 1,
            file_page,
            stamp,
            said: serde_json::json!({ "status": "found" }),
            unchecked: chars,
        }
    }
}

/// When a reference to characters has them checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CharsChecked {
    /// Before the command goes on: `page_of` reads the page's text and refuses a reference whose characters aren't the same. For a command that draws the page
    /// and doesn't read its text.
    First,
    /// Against the text the command reads itself: the page is found without reading its text, and [`attach`] checks the characters against the answer's own
    /// `untrusted.text`, so the text is read once and the words checked are the words given. A page whose text changed is found, and drawn, before it is refused.
    AgainstItsText,
}

fn unresolved(words: &'static str) -> AgentError {
    AgentError::new(Code::REFERENCE_UNRESOLVED, words, exit::NOT_FOUND)
}

/// The page a reference points at in this tab now, or the refusal in constant words. Only `found` and `moved` give a page: a reference that is gone,
/// matches several pages, wasn't fully looked for, or points at a page unsaved edits removed is refused, because the next thing done is a read of a page
/// and a guess would read the wrong one. The token is never repeated.
pub(crate) fn page_of<A: SessionAccess>(access: &A, tab: &TabRef, reference: &Locator, when: CharsChecked) -> Result<Located, AgentError> {
    page_of_reading(access, tab, reference, crate::render::page_text, when)
}

fn page_of_reading<A: SessionAccess>(
    access: &A,
    tab: &TabRef,
    reference: &Locator,
    read_text: PageTextReader,
    when: CharsChecked,
) -> Result<Located, AgentError> {
    let (file, found, outcome, text, stamp) = resolved(access, tab, reference, (when == CharsChecked::First).then_some(read_text))?;
    let (status, file_page, page) = settle(&outcome, |file_page| file.place_of(file_page))?;
    let mut said = json!({
        "status": status,
        "was_page": reference.page().number(),
        "file_page": file_page + 1,
        "document": document_word(found.document),
    });
    if text.is_some() {
        said["text"] = json!("same");
    }
    let unchecked = match reference.target() {
        Target::Characters(range) if when == CharsChecked::AgainstItsText => Some(range),
        _ => None,
    };
    Ok(Located {
        tab: recto_session::TabId(file.id()),
        page,
        file_page,
        stamp,
        said,
        unchecked,
    })
}

/// Which page a status points at: only `found` and `moved` (with the characters the reference names, if it names any, still the same) give one, and only if the tab still has it. Each other case is refused with its own words.
fn settle(outcome: &Outcome, place_of: impl Fn(u32) -> Option<u32>) -> Result<(&'static str, u32, u32), AgentError> {
    let (word, file_page) = match outcome {
        Outcome::TextChanged { .. } => return Err(unresolved(UNRESOLVED_TEXT_CHANGED)),
        Outcome::Status(Status::Found { page }) => ("found", *page),
        Outcome::Status(Status::Moved { page }) => ("moved", *page),
        Outcome::Status(Status::Ambiguous { .. }) => return Err(unresolved(UNRESOLVED_AMBIGUOUS)),
        Outcome::Status(Status::Gone) => return Err(unresolved(UNRESOLVED_GONE)),
        Outcome::Status(Status::Incomplete { .. }) => return Err(unresolved(UNRESOLVED_INCOMPLETE)),
    };
    let place = place_of(file_page).ok_or_else(|| unresolved(UNRESOLVED_REMOVED))?;
    Ok((word, file_page, place + 1))
}

impl Located {
    /// The tab the page was found in, as a command names a tab: what the rest of the command reads and checks it by.
    pub(crate) fn tab_ref(&self) -> TabRef {
        TabRef::Id(self.tab)
    }
}

/// Refuses when the tab's file isn't the version the page was found in: what was read is about a page of another version.
pub(crate) fn still_located<A: SessionAccess>(access: &A, at: &Located) -> Result<(), AgentError> {
    unchanged(&tab_file(access, &at.tab_ref())?, at.stamp)
}

/// Refuses when the page the tab gave for the place the reference was found at isn't the page it was found as: a page added from another PDF, or another
/// page of the file standing there now.
pub(crate) fn agree(found: &Located, source: &recto_session::PageSource) -> Result<(), AgentError> {
    if source.page == found.file_page && !source.added {
        Ok(())
    } else {
        Err(tab_changed())
    }
}

impl Located {
    /// The characters the reference names, as `(start, end)` in characters of the page's text, when they are still to be checked against the text of the
    /// command's own read ([`CharsChecked::AgainstItsText`]). A command that picks them takes them from here, so it picks exactly what was checked.
    pub(crate) fn characters(&self) -> Option<(u32, u32)> {
        self.unchecked.map(|range| range.offsets())
    }

    /// Checks the reference's characters, if it names any, against the text of the page as the command read it: the characters must be there and the
    /// same. A reference to a page or a spot has nothing to check.
    pub(crate) fn held_in(&self, page_text: &str) -> Result<(), AgentError> {
        match self.unchecked {
            Some(range) if locator::held_characters(range, page_text).is_none() => Err(unresolved(UNRESOLVED_TEXT_CHANGED)),
            _ => Ok(()),
        }
    }

    /// What the answer says of how the page was found, once its characters (if any) were checked: constant words and numbers, never the token.
    pub(crate) fn told(&self) -> Value {
        let mut said = self.said.clone();
        if let Some((start, end)) = self.characters() {
            said["text"] = json!("same");
            said["chars"] = json!([start, end]);
        }
        said
    }
}

/// After the page was read: the file is still the version it was found in, and the answer says how the page was found.
pub(crate) fn attach<A: SessionAccess>(access: &A, found: &Located, data: &mut Value) -> Result<(), AgentError> {
    still_located(access, found)?;
    if let Some(range) = found.unchecked {
        // Checked against the very text the answer gives, so the words given are the words that were checked. An answer with no text can't pass: nothing is
        // attached to it.
        let text = data["untrusted"]["text"].as_str().unwrap_or("");
        found.held_in(text)?;
        let held = locator::held_characters(range, text).map(str::to_owned);
        data["untrusted"]["hit"] = json!(held);
    }
    data["ref"] = found.told();
    Ok(())
}

/// [`attach`] for a command that read the page's text itself and has checked the reference's characters against it (`checked`): the file is still the
/// version the page was found in, a reference to characters that wasn't checked is refused, and the answer says how the page was found.
pub(crate) fn attach_checked<A: SessionAccess>(access: &A, found: &Located, checked: bool, data: &mut Value) -> Result<(), AgentError> {
    still_located(access, found)?;
    if found.unchecked.is_some() && !checked {
        return Err(unresolved(UNRESOLVED_TEXT_CHANGED));
    }
    data["ref"] = found.told();
    Ok(())
}

/// [`attach`] for a command that has just written a new file at `out` for the page it read. When the answer is refused, the file is of a page the file
/// no longer has, so it is taken away: exactly `out`, which the command made (it refused an `out` that was already there before it wrote), and nothing
/// else. An agent that is told to try again then can.
pub(crate) fn attach_new_file<A: SessionAccess>(access: &A, found: &Located, data: &mut Value, out: &std::path::Path) -> Result<(), AgentError> {
    attach(access, found, data).inspect_err(|_| {
        let _ = std::fs::remove_file(out);
    })
}

/// The refusal when the page standing where the reference was found isn't the page it was found as.
fn tab_changed() -> AgentError {
    AgentError::new(Code::FILE_CHANGED, TAB_CHANGED, exit::FAILED)
}

/// The tab's file, readable as pages: not withheld, not changed under its edits, not the renderer's alone, and opened as the version it is now.
fn readable<A: SessionAccess>(access: &A, tab: &TabRef) -> Result<(TabFile, Opened, recto_platform::FileStamp), AgentError> {
    let file = tab_file(access, tab)?;
    // Withheld, or changed under the tab's edits: refused as `contents` is. `execute`'s READS_PAGES gate only refuses edits it can't read, so a file another
    // program replaced before the app has looked is caught here.
    file.require_pages_shown()?;
    if file.renderer_only() {
        return Err(not_referenceable(RENDERER_ONLY));
    }
    let unreadable = || AgentError::new(Code::FILE_UNREADABLE, "The tab's file can't be read just now.", exit::FAILED);
    let stamp = recto_platform::file_stamp(file.path()).map_err(|_| unreadable())?;
    let opened = Opened::open(file.path(), stamp).ok_or_else(unreadable)?;
    Ok((file, opened, stamp))
}

/// Refuses when the file isn't the version that was read: an answer about pages of two versions is about neither.
fn unchanged(file: &TabFile, stamp: recto_platform::FileStamp) -> Result<(), AgentError> {
    match recto_platform::file_stamp(file.path()) {
        Ok(now) if now == stamp => Ok(()),
        _ => Err(AgentError::new(Code::FILE_CHANGED, CHANGED_WHILE_READ, exit::FAILED)),
    }
}

fn target_json(placed: &Placed, text: Option<TextMatch>) -> Value {
    match placed {
        Placed::Page => json!({ "kind": "page" }),
        Placed::Spot([x, y, width, height]) => json!({ "kind": "spot", "placed": true, "rect": [x, y, width, height] }),
        Placed::SpotFrameChanged => json!({ "kind": "spot", "placed": false, "why": "frame_changed" }),
        Placed::SpotOutsidePage => json!({ "kind": "spot", "placed": false, "why": "outside_page" }),
        Placed::CharactersNotChecked { start, end } => match text {
            Some(held) => {
                json!({ "kind": "characters", "start": start, "end": end, "checked": true, "text": if held == TextMatch::Same { "same" } else { "changed" } })
            }
            None => json!({ "kind": "characters", "start": start, "end": end, "checked": false }),
        },
    }
}

/// What the answer says in words: one sentence for the status, and one for each thing worth knowing beside it. `at` is the page found (zero-based, in
/// the file) and its place in the tab now; `was` the page the reference hinted at.
fn note_of(found: &Resolution, text: Option<TextMatch>, at: Option<Option<u32>>, was: u32, edited: bool) -> String {
    // A page of another document, by content alone, isn't "the page": it says only what is known.
    let other = found.document == DocumentMatch::Different;
    let mut note = match (&found.status, at) {
        (Status::Found { page }, _) if other => format!("A page with the same content is at page {} of the file.", page + 1),
        (Status::Found { page }, _) => format!("It's still at page {} of the file.", page + 1),
        (Status::Moved { page }, _) if other => format!(
            "A page with the same content is page {} of the file now, where the reference was of page {was}. Only that page has its content.",
            page + 1
        ),
        (Status::Moved { page }, _) => format!(
            "It moved: it's page {} of the file now, and was page {was}. Only that page has its content.",
            page + 1
        ),
        (Status::Ambiguous { pages, complete: true }, _) => format!(
            "{} pages have that content, so it can't say which was meant. They're the candidates.",
            pages.len()
        ),
        (Status::Ambiguous { pages, complete: false }, _) => format!(
            "More than {MAX_CANDIDATES} pages have that content, so it can't say which was meant. The first {} are the candidates.",
            pages.len()
        ),
        (Status::Gone, _) => "Every page was read and none is that page: it's gone from this file.".to_string(),
        (Status::Incomplete { .. }, _) => {
            "It stopped before every page was read, or a page couldn't be told apart, so it can't say whether the page is here.".to_string()
        }
    };
    match at {
        Some(None) if edited => note.push_str(" Unsaved edits have removed it from the tab, so it has no page there."),
        Some(_) if edited => note.push_str(" `page` counts the tab with its unsaved edits, and `file_page` the file."),
        _ => {}
    }
    if found.document == DocumentMatch::Different {
        note.push_str(" This file doesn't look like the document the reference was made in.");
    }
    match found.target {
        Some(Placed::Spot(_)) => note.push_str(" The spot is in points on the page as the file shows it, before any unsaved turn."),
        Some(Placed::SpotFrameChanged) => note.push_str(" The page's crop or turn changed, so the spot isn't placed."),
        Some(Placed::SpotOutsidePage) => note.push_str(" The spot is outside the page now, so it isn't placed."),
        Some(Placed::CharactersNotChecked { .. }) => note.push_str(match text {
            Some(TextMatch::Same) => " The characters are the same as the ones the reference was made of.",
            Some(TextMatch::Changed) => {
                " The page is there, but its text at those characters isn't what the reference was made of, and nothing else on the page was searched for it."
            }
            None => " The characters aren't checked against the page's text.",
        }),
        Some(Placed::Page) | None => {}
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolution(status: Status, document: DocumentMatch) -> Resolution {
        Resolution {
            status,
            document,
            frame_same: None,
            target: None,
            looked: Looked::All,
            pages_read: 1,
            bytes_read: 0,
        }
    }

    /// Failure: a page matched by content in a file that doesn't look like the reference's document is called "still at" its page, as though it were the
    /// page the reference was made of, where all that is known is that a page there has the same content.
    #[test]
    fn a_page_of_another_document_is_said_to_have_the_same_content_not_to_be_still_there() {
        for status in [Status::Found { page: 2 }, Status::Moved { page: 2 }] {
            let same = note_of(&resolution(status.clone(), DocumentMatch::Same), None, Some(Some(3)), 1, false);
            let other = note_of(&resolution(status, DocumentMatch::Different), None, Some(Some(3)), 1, false);
            assert!(!same.contains("same content"), "{same}");
            assert!(
                other.contains("A page with the same content is") && other.contains("page 3 of the file"),
                "{other}"
            );
            assert!(!other.contains("still") && !other.contains("It moved"), "{other}");
        }
    }

    fn pdf(who: &str, pages: usize) -> Vec<u8> {
        let pages: Vec<recto_testpdf::Page> = (1..=pages)
            .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{who} page {page}")])))
            .collect();
        recto_testpdf::pages(&pages)
    }

    /// A real session with one file open, as `execute` would meet it, and the folder it is in.
    fn one_open(
        pages: usize,
    ) -> (
        recto_testpdf::TempDir,
        std::path::PathBuf,
        std::sync::Mutex<recto_session::Session<crate::DrawableLoader>>,
    ) {
        let dir = recto_testpdf::TempDir::new("recto-locating-");
        let path = dir.join("doc.pdf");
        std::fs::write(&path, pdf("Mine", pages)).unwrap();
        let mut session = recto_session::Session::new(crate::DrawableLoader);
        session.open(&path).unwrap();
        (dir, path, std::sync::Mutex::new(session))
    }

    /// The page `page` of the tab, found by a reference to it.
    fn found_at(access: &std::sync::Mutex<recto_session::Session<crate::DrawableLoader>>, tab: &TabRef, page: u32) -> Located {
        let made = reference(access, tab, page, None).expect("a reference");
        let token = read_reference(made["ref"].as_str().expect("its token")).expect("a reference");
        page_of(access, tab, &token, CharsChecked::First).expect("found")
    }

    /// Another program's save: new bytes of another length, renamed over the file.
    fn replace(path: &std::path::Path, bytes: &[u8]) {
        let incoming = path.with_extension("incoming");
        std::fs::write(&incoming, bytes).unwrap();
        std::fs::rename(&incoming, path).unwrap();
    }

    /// Failure: `--ref` asked for "the active tab", and the person switched tab while the agent's read was under way: the read is refused as a changed
    /// file, though it was of the tab the reference was found in and `--page` never refuses it. The tab is settled once, where the reference is found.
    #[test]
    fn a_person_switching_tab_during_a_read_by_reference_does_not_refuse_it() {
        let (dir, _path, access) = one_open(3);
        let found = found_at(&access, &TabRef::Active, 2);
        let other = dir.join("other.pdf");
        std::fs::write(&other, pdf("Other", 2)).unwrap();
        access.with(|session| session.open(&other)).unwrap();
        let mut data = json!({});
        attach(&access, &found, &mut data).expect("the tab it was found in is the one checked");
        assert_eq!(data["ref"]["file_page"], 2, "{data}");
    }

    /// Failure: `render --ref` writes its picture, and the file changed during the draw, so the agent is told `file_changed` and the picture, maybe of the
    /// wrong page, stays at `--out`, where a retry is refused as `output_exists`. The refusal takes away the file this command made, and nothing beside it;
    /// an answer that holds keeps it.
    #[test]
    fn a_refusal_after_the_file_changed_takes_away_the_picture_it_wrote_and_only_that() {
        let (dir, path, access) = one_open(3);
        let found = found_at(&access, &TabRef::Active, 2);
        let (out, beside) = (dir.join("page.png"), dir.join("beside.png"));
        std::fs::write(&out, b"picture").unwrap();
        std::fs::write(&beside, b"another").unwrap();
        attach_new_file(&access, &found, &mut json!({}), &out).expect("the file is as it was found");
        assert!(out.exists(), "an answer that holds keeps its picture");
        replace(&path, &pdf("Theirs", 4));
        let refused = attach_new_file(&access, &found, &mut json!({}), &out).expect_err("the file changed");
        assert_eq!((refused.code, refused.message.as_str()), (Code::FILE_CHANGED, CHANGED_WHILE_READ));
        assert!(!out.exists(), "the picture is left at --out");
        assert!(beside.exists(), "nothing else is touched");
    }

    /// Failure: `agree` lets a read go on with a page that isn't the one the reference found: the page of the file standing at that place changed (another
    /// program's save moved it), or a page added from another PDF now stands there. Only the page found, of the file, passes.
    #[test]
    fn agree_passes_only_the_page_that_was_found_and_refuses_an_added_page_or_another() {
        let (_dir, _path, access) = one_open(3);
        let found = found_at(&access, &TabRef::Active, 2);
        let source = |page: u32, added: bool| recto_session::PageSource {
            path: "/d/doc.pdf".into(),
            page,
            quarter_turns: 0,
            added,
            expected: None,
        };
        assert!(agree(&found, &source(found.file_page, false)).is_ok());
        for refused in [source(found.file_page + 1, false), source(found.file_page, true)] {
            let error = agree(&found, &refused).expect_err("not the page that was found");
            assert_eq!((error.code, error.message.as_str()), (Code::FILE_CHANGED, TAB_CHANGED));
        }
    }

    /// Failure: a file another program replaced after the page was found is read as though it were the version found: the read is about a page of another
    /// version. `still_located` refuses it with the file-changed words, and passes while the file is as it was.
    #[test]
    fn still_located_refuses_a_file_changed_since_the_page_was_found_in_the_file_changed_words() {
        let (_dir, path, access) = one_open(3);
        let found = found_at(&access, &TabRef::Active, 2);
        still_located(&access, &found).expect("the file is as it was found");
        replace(&path, &pdf("Theirs", 4));
        let error = still_located(&access, &found).expect_err("the file changed");
        assert_eq!((error.code, error.message.as_str()), (Code::FILE_CHANGED, CHANGED_WHILE_READ));
    }

    /// Failure: a reference that is gone, matches several pages, wasn't fully looked for, or points at a page unsaved edits removed gives a page anyway (so
    /// `text --ref` reads the wrong page), or two of those cases share their words (so an agent can't tell "look again" from "it's gone"), or a refusal
    /// carries something other than its own constant words.
    #[test]
    fn only_found_and_moved_give_a_page_every_other_status_refuses_with_its_own_words() {
        let at = |file_page: u32| (file_page != 9).then_some(file_page + 4);
        let status = Outcome::Status;
        assert_eq!(settle(&status(Status::Found { page: 2 }), at).ok(), Some(("found", 2, 7)));
        assert_eq!(settle(&status(Status::Moved { page: 3 }), at).ok(), Some(("moved", 3, 8)));
        let refused = [
            settle(&status(Status::Gone), at),
            settle(
                &status(Status::Ambiguous {
                    pages: vec![1, 2],
                    complete: true,
                }),
                at,
            ),
            settle(&status(Status::Incomplete { pages: vec![] }), at),
            settle(&status(Status::Found { page: 9 }), at),
            settle(&Outcome::TextChanged { page: 2 }, at),
        ]
        .map(|answer| answer.expect_err("refused"));
        let mut words = std::collections::BTreeSet::new();
        for error in &refused {
            assert_eq!((error.code, error.exit), (Code::REFERENCE_UNRESOLVED, exit::NOT_FOUND), "{}", error.message);
            assert!(words.insert(error.message.clone()), "two cases share their words: {}", error.message);
        }
    }
}

#[cfg(test)]
mod unreadable_text_tests {
    use super::*;
    use recto_core::locator::Locator;
    use recto_session::{DiskLoader, Session};
    use recto_testpdf::{Page, TempPdf, pages, text_lines};
    use std::sync::Mutex;

    fn unreadable(_: &std::path::Path, _: u32) -> Result<String, AgentError> {
        Err(AgentError::new(Code::RENDERER_MISBEHAVED, "made: the page's text can't be read", exit::FAILED))
    }

    /// Failure: a character target on a page whose text can't be read is answered as a status (`found`, `text_changed`) or with other words than the read's
    /// own, by any of `ref --chars`, `resolve` and a command that goes on to read the page (`page_of`), so an agent is told something about words nobody
    /// could read. The reader's refusal must come out of each unchanged, whatever made it fail.
    #[test]
    fn a_page_text_that_cannot_be_read_is_the_readers_own_refusal_in_ref_resolve_and_page_of() {
        let file = TempPdf::new(
            "two.pdf",
            &pages(&[Page::new(text_lines(&["First page"])), Page::new(text_lines(&["Second page"]))]),
        );
        let mut session = Session::new(DiskLoader);
        session.open(&file.0).unwrap();
        let access = Mutex::new(session);
        let tab = TabRef::Active;
        let whole = reference(&access, &tab, 2, None).expect("the page's reference");
        let made = read_reference(whole["ref"].as_str().unwrap()).expect("a reference");
        let range = locator::characters("Second page", 0, 6).unwrap();
        let chars = Locator::new(made.document(), made.page(), Target::Characters(range));
        let want = unreadable(std::path::Path::new(""), 0).unwrap_err();
        let same = |said: AgentError, from: &str| assert_eq!((said.code, said.exit, said.message), (want.code, want.exit, want.message.clone()), "{from}");
        same(resolve_reading(&access, &tab, chars, unreadable).expect_err("not a status"), "resolve");
        same(
            page_of_reading(&access, &tab, &chars, unreadable, CharsChecked::First)
                .err()
                .expect("not a page"),
            "page_of",
        );
        same(
            reference_reading(&access, &tab, 2, Some((0, 6)), unreadable).expect_err("not a reference"),
            "ref --chars",
        );
        // The same reference with a reader that works is answered, so the refusals above are the reader's and not the setup's.
        let answered = resolve_reading(&access, &tab, chars, |_, _| Ok("Second page".into())).expect("resolved");
        assert_eq!(answered["status"], "found", "{answered}");
    }

    /// Failure: a reference to characters read with the page's text (`text --ref`) passes without being checked against that text: the characters are
    /// handed over unchecked (so an agent is told words that aren't on the page), a missing or other text is let through, or the page's text is read
    /// before the command's own read. `page_of` reads no text; `attach` gives `text: same`, the characters and `hit` only when they are the checked words,
    /// and an answer with other text, or none, is refused with nothing attached.
    #[test]
    fn text_refs_characters_are_checked_against_the_text_it_read_and_never_pass_unchecked() {
        let file = TempPdf::new(
            "two.pdf",
            &pages(&[Page::new(text_lines(&["First page"])), Page::new(text_lines(&["Second page"]))]),
        );
        let mut session = Session::new(DiskLoader);
        session.open(&file.0).unwrap();
        let access = Mutex::new(session);
        let tab = TabRef::Active;
        let whole = reference(&access, &tab, 2, None).expect("the page's reference");
        let made = read_reference(whole["ref"].as_str().unwrap()).expect("a reference");
        let range = locator::characters("Second page", 0, 6).unwrap();
        let chars = Locator::new(made.document(), made.page(), Target::Characters(range));
        let found = page_of_reading(&access, &tab, &chars, unreadable, CharsChecked::AgainstItsText).expect("no read happens before the command's own");
        assert!(found.said.get("text").is_none(), "{}", found.said);
        let mut data = json!({ "untrusted": { "text": "Second page" } });
        attach(&access, &found, &mut data).expect("the words are those the reference was made of");
        assert_eq!(
            (&data["ref"]["text"], &data["ref"]["chars"], &data["untrusted"]["hit"]),
            (&json!("same"), &json!([0, 6]), &json!("Second"))
        );
        for other in [json!({ "untrusted": { "text": "Secone page" } }), json!({})] {
            let mut data = other.clone();
            let refused = attach(&access, &found, &mut data).expect_err("not the checked words");
            assert_eq!(
                (refused.code, refused.message.as_str()),
                (Code::REFERENCE_UNRESOLVED, UNRESOLVED_TEXT_CHANGED),
                "{other}"
            );
            assert_eq!(data, other, "nothing is attached to a refused answer");
        }
    }
}
