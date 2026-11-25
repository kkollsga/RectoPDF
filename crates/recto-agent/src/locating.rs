//! `ref` and `resolve`: a reference to a page, and where that page is now (`recto_core::locator`). This is the executor: the tab, its file, the core, and
//! the answer's words. The session is asked once, inside `tab_file`, for the tab's facts; everything after reads the file with the session free. A
//! token is hostile input and is never echoed, logged or kept: an answer carries only numbers and constant words.

use crate::reading::{TabFile, tab_file};
use crate::{AgentError, Code, SessionAccess, TabRef, exit};
use recto_core::locator::{self, DocumentMatch, Locator, Looked, MAX_CANDIDATES, Placed, Resolution, ScanCaps, Status, Unreferenceable};
use recto_session::identify::Opened;
use serde_json::{Value, json};

/// The refusal for a file the tab can't be read as pages from, in constant words.
const RENDERER_ONLY: &str = "Only the renderer can read this file, so its pages have no identity to point at.";
const CHANGED_WHILE_READ: &str = "The file changed while it was being read. Try again.";
const NO_IDENTITY: &str = "That page can't be told apart from the others, so it can't be pointed at.";
const NO_DOCUMENT_TAG: &str = "This document has nothing to say which document it is, so a reference to it would mean nothing.";
const ADDED_PAGE: &str = "That page was added from another PDF and isn't in this file yet. Save first, or point at a page of the file.";

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

fn not_referenceable(words: &'static str) -> AgentError {
    AgentError::new(Code::NOT_REFERENCEABLE, words, exit::FAILED)
}

/// `recto ref <page>`: a reference to page `page` (counted from 1 in the tab as it is now, unsaved edits included).
pub(crate) fn reference<A: SessionAccess>(access: &A, tab: &TabRef, page: u32) -> Result<Value, AgentError> {
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
    unchanged(&file, stamp)?;
    let made = made.map_err(|why| match why {
        Unreferenceable::NoSuchPage => crate::no_such_page(),
        Unreferenceable::NoIdentity => not_referenceable(NO_IDENTITY),
        Unreferenceable::NoDocumentTag => not_referenceable(NO_DOCUMENT_TAG),
    })?;
    Ok(json!({ "tab": file.id(), "page": page, "file_page": file_page + 1, "ref": locator::encode(&made) }))
}

/// `recto resolve <ref>`: where the reference's page is in this tab now. A page that moved, is gone, can't be told from others or wasn't fully looked
/// for is an answer (exit 0), never a guess.
pub(crate) fn resolve<A: SessionAccess>(access: &A, tab: &TabRef, reference: Locator) -> Result<Value, AgentError> {
    let (file, opened, stamp) = readable(access, tab)?;
    let found = locator::resolve(&reference, opened.reader(), opened.map(), &ScanCaps::STANDARD);
    unchanged(&file, stamp)?;
    // A page of the file, as the tab shows it now (from 1): its position among the unsaved edits, or none when they removed it.
    let place = |file_page: u32| -> Option<u32> { file.place_of(file_page).map(|place| place + 1) };
    let both = |file_page: u32| json!({ "page": place(file_page), "file_page": file_page + 1 });
    let (status, at, listed) = match &found.status {
        Status::Found { page } => ("found", Some(*page), &[][..]),
        Status::Moved { page } => ("moved", Some(*page), &[][..]),
        Status::Ambiguous { pages, .. } => ("ambiguous", None, pages.as_slice()),
        Status::Gone => ("gone", None, &[][..]),
        Status::Incomplete { pages } => ("incomplete", None, pages.as_slice()),
    };
    let was = reference.page().number();
    let target = found.target.map(|placed| target_json(&placed));
    let note = note_of(&found, at.map(place), was, file.has_edit_list());
    Ok(json!({
        "tab": file.id(),
        "status": status,
        "page": at.and_then(place),
        "file_page": at.map(|page| page + 1),
        "was_page": was,
        "candidates": listed.iter().map(|page| both(*page)).collect::<Vec<_>>(),
        "document": match found.document {
            DocumentMatch::Same => "same",
            DocumentMatch::Different => "different",
            DocumentMatch::Unknown => "unknown",
        },
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
    /// The tab it was found in, so a read of "the active tab" that moved in between is refused.
    pub tab: u32,
    /// Its place in the tab, counted from 1, unsaved edits included.
    pub page: u32,
    /// Its page of the file on disk, counted from 0.
    pub file_page: u32,
    /// The file's version it was found in.
    pub stamp: recto_platform::FileStamp,
    /// What the answer says of how it was found: constant words and numbers, never the token.
    pub said: Value,
}

fn unresolved(words: &'static str) -> AgentError {
    AgentError::new(Code::REFERENCE_UNRESOLVED, words, exit::NOT_FOUND)
}

/// The page a reference points at in this tab now, or the refusal in constant words. Only `found` and `moved` give a page: a reference that is gone,
/// matches several pages, wasn't fully looked for, or points at a page unsaved edits removed is refused, because the next thing done is a read of a page
/// and a guess would read the wrong one. The token is never repeated.
pub(crate) fn page_of<A: SessionAccess>(access: &A, tab: &TabRef, reference: &Locator) -> Result<Located, AgentError> {
    let (file, opened, stamp) = readable(access, tab)?;
    let found = locator::resolve(reference, opened.reader(), opened.map(), &ScanCaps::STANDARD);
    unchanged(&file, stamp)?;
    let (status, file_page, page) = settle(&found.status, |file_page| file.place_of(file_page))?;
    let said = json!({
        "status": status,
        "was_page": reference.page().number(),
        "file_page": file_page + 1,
        "document": match found.document {
            DocumentMatch::Same => "same",
            DocumentMatch::Different => "different",
            DocumentMatch::Unknown => "unknown",
        },
    });
    Ok(Located {
        tab: file.id(),
        page,
        file_page,
        stamp,
        said,
    })
}

/// Which page a status points at: only `found` and `moved` give one, and only if the tab still has it. Each other case is refused with its own words.
fn settle(status: &Status, place_of: impl Fn(u32) -> Option<u32>) -> Result<(&'static str, u32, u32), AgentError> {
    let (word, file_page) = match status {
        Status::Found { page } => ("found", *page),
        Status::Moved { page } => ("moved", *page),
        Status::Ambiguous { .. } => return Err(unresolved(UNRESOLVED_AMBIGUOUS)),
        Status::Gone => return Err(unresolved(UNRESOLVED_GONE)),
        Status::Incomplete { .. } => return Err(unresolved(UNRESOLVED_INCOMPLETE)),
    };
    let place = place_of(file_page).ok_or_else(|| unresolved(UNRESOLVED_REMOVED))?;
    Ok((word, file_page, place + 1))
}

/// Refuses when the tab's file isn't the version the page was found in: what was read is about a page of another version.
pub(crate) fn still_located<A: SessionAccess>(access: &A, tab: &TabRef, at: &Located) -> Result<(), AgentError> {
    let file = tab_file(access, tab)?;
    if file.id() != at.tab {
        return Err(tab_changed());
    }
    unchanged(&file, at.stamp)
}

/// Refuses when the page the tab gave for the place the reference was found at isn't the page it was found as: another tab, a page added from another
/// PDF, or another page of the file standing there now.
pub(crate) fn agree(found: &Located, tab: recto_session::TabId, source: &recto_session::PageSource) -> Result<(), AgentError> {
    if tab.0 == found.tab && source.page == found.file_page && !source.added {
        Ok(())
    } else {
        Err(tab_changed())
    }
}

/// After the page was read: the file is still the version it was found in, and the answer says how the page was found.
pub(crate) fn attach<A: SessionAccess>(access: &A, tab: &TabRef, found: &Located, data: &mut Value) -> Result<(), AgentError> {
    still_located(access, tab, found)?;
    data["ref"] = found.said.clone();
    Ok(())
}

/// The refusal when the tab, or the page standing where the reference was found, isn't the one it was found in.
pub(crate) fn tab_changed() -> AgentError {
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

fn target_json(placed: &Placed) -> Value {
    match placed {
        Placed::Page => json!({ "kind": "page" }),
        Placed::Spot([x, y, width, height]) => json!({ "kind": "spot", "placed": true, "rect": [x, y, width, height] }),
        Placed::SpotFrameChanged => json!({ "kind": "spot", "placed": false, "why": "frame_changed" }),
        Placed::SpotOutsidePage => json!({ "kind": "spot", "placed": false, "why": "outside_page" }),
        Placed::CharactersNotChecked { start, end } => json!({ "kind": "characters", "start": start, "end": end, "checked": false }),
    }
}

/// What the answer says in words: one sentence for the status, and one for each thing worth knowing beside it. `at` is the page found (zero-based, in
/// the file) and its place in the tab now; `was` the page the reference hinted at.
fn note_of(found: &Resolution, at: Option<Option<u32>>, was: u32, edited: bool) -> String {
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
        Some(Placed::CharactersNotChecked { .. }) => note.push_str(" The characters aren't checked against the page's text."),
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
            let same = note_of(&resolution(status.clone(), DocumentMatch::Same), Some(Some(3)), 1, false);
            let other = note_of(&resolution(status, DocumentMatch::Different), Some(Some(3)), 1, false);
            assert!(!same.contains("same content"), "{same}");
            assert!(
                other.contains("A page with the same content is") && other.contains("page 3 of the file"),
                "{other}"
            );
            assert!(!other.contains("still") && !other.contains("It moved"), "{other}");
        }
    }

    /// Failure: a reference that is gone, matches several pages, wasn't fully looked for, or points at a page unsaved edits removed gives a page anyway (so
    /// `text --ref` reads the wrong page), or two of those cases share their words (so an agent can't tell "look again" from "it's gone"), or a refusal
    /// carries something other than its own constant words.
    #[test]
    fn only_found_and_moved_give_a_page_every_other_status_refuses_with_its_own_words() {
        let at = |file_page: u32| (file_page != 9).then_some(file_page + 4);
        assert_eq!(settle(&Status::Found { page: 2 }, at).ok(), Some(("found", 2, 7)));
        assert_eq!(settle(&Status::Moved { page: 3 }, at).ok(), Some(("moved", 3, 8)));
        let refused = [
            settle(&Status::Gone, at),
            settle(
                &Status::Ambiguous {
                    pages: vec![1, 2],
                    complete: true,
                },
                at,
            ),
            settle(&Status::Incomplete { pages: vec![] }, at),
            settle(&Status::Found { page: 9 }, at),
        ]
        .map(|answer| answer.expect_err("refused"));
        let mut words = std::collections::BTreeSet::new();
        for error in &refused {
            assert_eq!((error.code, error.exit), (Code::REFERENCE_UNRESOLVED, exit::NOT_FOUND), "{}", error.message);
            assert!(words.insert(error.message.clone()), "two cases share their words: {}", error.message);
        }
    }
}
