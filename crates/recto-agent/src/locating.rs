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

/// The words for a reference that can't be read, never the reference itself.
pub(crate) fn bad_reference() -> AgentError {
    AgentError::new(Code::BAD_REFERENCE, "That isn't a reference RectoPDF made.", exit::USAGE)
}

/// The words for a reference made by a newer RectoPDF.
pub(crate) fn reference_too_new() -> AgentError {
    AgentError::new(
        Code::REFERENCE_TOO_NEW,
        "That reference was made by a newer RectoPDF than this one.",
        exit::USAGE,
    )
}

fn not_referenceable(words: &'static str) -> AgentError {
    AgentError::new(Code::NOT_REFERENCEABLE, words, exit::FAILED)
}

/// `recto ref <page>`: a reference to page `page` (counted from 1 in the tab as it is now, unsaved edits included).
pub(crate) fn reference<A: SessionAccess>(access: &A, tab: &TabRef, page: u32) -> Result<Value, AgentError> {
    let (file, opened, stamp) = readable(access, tab)?;
    let wanted = crate::one_based(page)?;
    let file_page = match &file.positions {
        None => wanted,
        // Edited: the file's page that stands at this position now. A position with none is a page added from another PDF.
        Some(positions) => match positions.iter().position(|place| *place == wanted) {
            Some(file_page) => file_page as u32,
            None if wanted < file.pages => return Err(not_referenceable(ADDED_PAGE)),
            None => return Err(crate::no_such_page()),
        },
    };
    let made = locator::reference(opened.reader(), opened.map(), file_page, &ScanCaps::STANDARD);
    unchanged(&file, stamp)?;
    let made = made.map_err(|why| match why {
        Unreferenceable::NoSuchPage => crate::no_such_page(),
        Unreferenceable::NoIdentity => not_referenceable(NO_IDENTITY),
        Unreferenceable::NoDocumentTag => not_referenceable(NO_DOCUMENT_TAG),
    })?;
    Ok(json!({ "tab": file.id, "page": page, "file_page": file_page + 1, "ref": locator::encode(&made) }))
}

/// `recto resolve <ref>`: where the reference's page is in this tab now. A page that moved, is gone, can't be told from others or wasn't fully looked
/// for is an answer (exit 0), never a guess.
pub(crate) fn resolve<A: SessionAccess>(access: &A, tab: &TabRef, reference: Locator) -> Result<Value, AgentError> {
    let (file, opened, stamp) = readable(access, tab)?;
    let found = locator::resolve(&reference, opened.reader(), opened.map(), &ScanCaps::STANDARD);
    unchanged(&file, stamp)?;
    // A page of the file, as the tab shows it now: its position among the unsaved edits, or none when they removed it.
    let place = |file_page: u32| -> Option<u32> {
        match &file.positions {
            None => Some(file_page + 1),
            Some(positions) => positions
                .get(file_page as usize)
                .copied()
                .filter(|place| *place != u32::MAX)
                .map(|place| place + 1),
        }
    };
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
    let note = note_of(&found, at.map(|page| (page, place(page))), was, file.positions.is_some());
    Ok(json!({
        "tab": file.id,
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

/// The tab's file, readable as pages: not withheld, not changed under its edits, not the renderer's alone, and opened as the version it is now.
fn readable<A: SessionAccess>(access: &A, tab: &TabRef) -> Result<(TabFile, Opened, recto_platform::FileStamp), AgentError> {
    let file = tab_file(access, tab)?;
    if file.withheld {
        return Err(crate::source_changed(&file.path));
    }
    // A changed file is refused before `run` by `execute`'s READS_PAGES gate (`source_changed`), the one implementation of that rule.
    if file.renderer_only {
        return Err(not_referenceable(RENDERER_ONLY));
    }
    let unreadable = || AgentError::new(Code::FILE_UNREADABLE, "The tab's file can't be read just now.", exit::FAILED);
    let stamp = recto_platform::file_stamp(&file.path).map_err(|_| unreadable())?;
    let opened = Opened::open(&file.path, stamp).ok_or_else(unreadable)?;
    Ok((file, opened, stamp))
}

/// Refuses when the file isn't the version that was read: an answer about pages of two versions is about neither.
fn unchanged(file: &TabFile, stamp: recto_platform::FileStamp) -> Result<(), AgentError> {
    match recto_platform::file_stamp(&file.path) {
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
fn note_of(found: &Resolution, at: Option<(u32, Option<u32>)>, was: u32, edited: bool) -> String {
    let mut note = match (&found.status, at) {
        (Status::Found { page }, _) => format!("It's still at page {} of the file.", page + 1),
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
        Some((_, None)) if edited => note.push_str(" Unsaved edits have removed it from the tab, so it has no page there."),
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
