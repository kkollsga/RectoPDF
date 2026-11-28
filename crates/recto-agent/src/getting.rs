//! `get <ref> --grain hit|page --as text|markdown`: the text a reference points at, and where to go on from it (`docs/design/blocks.md` §3 and §4).
//! This is the executor; `commands::getting` is the row around it. **Stateless:** no cursor, token store or session field. What a reference gives
//! is the one answer, and the neighbours it names are refs the agent asks with next.
//!
//! - **Resolved once, through `locating`:** only `found` and `moved` answer (a reference to characters also needs them to be the same); every other
//!   status is `reference_unresolved`, in `text --ref`'s words. A protected document is refused before anything is resolved, by the row's `PROTECTED_OK`.
//! - **Text of record.** When the file's `CorpusKey` still matches a finished index, the page's text is read from it and the reference's characters are
//!   checked against it; otherwise the page is read live, as `text` reads it. The key and the file's version are checked again at the end, as
//!   `HitRefs::finish` does: an answer is about one version of the file or about none.
//! - **No blocks read yet.** `paragraph`, `section` and `chapter` need blocks (S5), so they answer the page and say `widened: { to: "page", why:
//!   "blocks_building" }` while the blocks this ask started are on their way, and `"blocks_not_made"` otherwise (`crate::blocks::why_page`). A `hit` of a reference to a page answers the page, `why: "ref_is_page"`.
//! - **No path** is ever in an answer: the link is the file's name and `#page=N`, under `untrusted` with the text.

use crate::commands::getting::{Form, Grain};
use crate::commands::locating::{PageOrRef, Where};
use crate::{AgentError, Code, Event, SessionAccess, TabRef, exit};
use recto_core::locator::{self, Locator, Referrer, ScanCaps};
use recto_index::blocks::Kind;
use recto_index::blocks::markdown::{self, Piece};
use recto_index::corpus::{CorpusKey, PageKind, ReadyCorpus};
use serde_json::{Value, json};

/// An answer's default size in characters, and the most a caller may ask for (owner's answer F, 2026-11-27).
pub(crate) const DEFAULT_MAX_CHARS: u32 = 16_000;
pub(crate) const MAX_CHARS: u32 = 65_536;

const CHANGED_WHILE_READ: &str = "The file changed while it was being read. Try again.";
const NO_PAGE_TEXT: &str = "That page's text can't be read just now.";

/// What one `get` asks for.
pub(crate) struct Asked<'a> {
    pub tab: &'a TabRef,
    pub reference: &'a Locator,
    pub grain: Grain,
    pub form: Form,
    pub max_chars: u32,
}

/// A page's text as of record, and where it came from.
struct PageRead {
    text: String,
    /// Hidden stretches, in characters of `text`.
    hidden: Vec<(u32, u32)>,
    /// The key of the index it was read from, `None` for a live read.
    key: Option<CorpusKey>,
    /// Whether the page was checked for hidden text, and whether some stretches didn't fit in `hidden`.
    checked: bool,
    partial: bool,
}

/// Whether a stored page may be given as it is: it was checked for hidden text and no stretch was dropped. Otherwise its words would show unmarked what
/// `text` marks, so the page is read live.
fn index_is_enough(hidden_checked: bool, spans_dropped: bool) -> bool {
    hidden_checked && !spans_dropped
}

fn changed() -> AgentError {
    AgentError::new(Code::FILE_CHANGED, CHANGED_WHILE_READ, exit::FAILED)
}

/// The page from a finished index of this file's key, if there is one and the page has text in it.
fn from_index<A: SessionAccess>(access: &A, path: &std::path::Path, file_page: u32) -> Option<PageRead> {
    let indexing = access.indexing()?;
    let find_key = indexing.find_key()?;
    let key = CorpusKey::of_file(path).ok()?;
    let corpus = ReadyCorpus::open(find_key, indexing.root(), key).ok()??;
    let mut buffer = Vec::new();
    let stored = corpus.page(file_page, &mut buffer).ok()?;
    (stored.kind == PageKind::Text && index_is_enough(stored.hidden_checked, stored.spans_dropped)).then(|| PageRead {
        text: stored.text.to_owned(),
        hidden: stored.hidden_spans(),
        key: Some(key),
        checked: true,
        partial: false,
    })
}

/// The page as `text` reads it now.
fn read_live(path: &std::path::Path, file_page: u32) -> Result<PageRead, AgentError> {
    let said = crate::render::text(path, file_page + 1)?;
    let spans = said["hidden_spans"].as_array().map_or_else(Vec::new, |spans| {
        spans
            .iter()
            .filter_map(|span| Some((u32::try_from(span[0].as_u64()?).ok()?, u32::try_from(span[1].as_u64()?).ok()?)))
            .collect()
    });
    let text = said["untrusted"]["text"]
        .as_str()
        .ok_or_else(|| AgentError::new(Code::PAGE_UNREADABLE, NO_PAGE_TEXT, exit::FAILED))?;
    Ok(PageRead {
        text: text.to_owned(),
        hidden: spans,
        key: None,
        checked: said["hidden_text"]["checked"] == true,
        partial: said["hidden_spans_dropped"].as_u64().is_some_and(|dropped| dropped > 0) || said["hidden_spans_dropped"] == true,
    })
}

/// The page's reference, or none where it can't be made (a page added from another PDF has none, and the file's tags may be out of reach).
fn page_ref(referrer: &mut Referrer<'_>, file_page: Option<u32>) -> Value {
    file_page
        .and_then(|page| referrer.page(page).ok())
        .map_or(Value::Null, |made| json!(locator::encode(&made)))
}

/// Where the text is cut: at the last line end inside the cap, and at the cap if there is none.
fn cut_at(chars: &[char], max: usize) -> usize {
    if chars.len() <= max {
        return chars.len();
    }
    chars[..max].iter().rposition(|c| *c == '\n').filter(|at| *at > 0).unwrap_or(max)
}

/// The hidden stretches that fall inside `[from, to)`, counted from `from`.
fn spans_in(spans: &[(u32, u32)], from: u32, to: u32) -> Vec<(u32, u32)> {
    spans
        .iter()
        .filter_map(|&(start, end)| {
            let (start, end) = (start.max(from), end.min(to));
            (start < end).then_some((start - from, end - from))
        })
        .collect()
}

/// `recto get`: the unit the reference points at, as text or Markdown, with refs to go on from.
pub(crate) fn get<A: SessionAccess>(access: &A, events: &mut Vec<Event>, asked: &Asked<'_>) -> Result<Value, AgentError> {
    let Where { found, id, position, .. } = PageOrRef::Ref(*asked.reference).locate(access, asked.tab, crate::locating::CharsChecked::AgainstItsText)?;
    let Some(found) = found else {
        return Err(crate::locating::bad_reference());
    };
    let tab = found.tab_ref();
    let file = crate::reading::tab_file(access, &tab)?;
    let stamped = recto_platform::file_stamp(file.path()).map_err(|_| changed())?;
    if stamped != found.stamp {
        return Err(changed());
    }

    let read = match from_index(access, file.path(), found.file_page) {
        Some(read) => read,
        None => {
            let read = read_live(file.path(), found.file_page);
            // A draw made (the hidden-text check) or failed is told to the host, as `text` does, even if what follows refuses the answer.
            if read.as_ref().map_or(true, |read| read.checked) {
                crate::note_draw(access, id, position, read.as_ref().err().map(|error| error.code.as_str()), events);
            }
            read?
        }
    };
    // The reference's characters are checked against the text the answer is made from.
    found.held_in(&read.text)?;

    let page_chars: Vec<char> = read.text.chars().collect();
    let total = u32::try_from(page_chars.len()).unwrap_or(u32::MAX);
    let (from, to, kind, widened) = match (asked.grain, found.characters()) {
        (Grain::Hit, Some((start, end))) => (start.min(end.min(total)), end.min(total), "hit", Value::Null),
        (Grain::Hit, None) => (0, total, "page", json!({ "to": "page", "why": "ref_is_page" })),
        (Grain::Page, _) => (0, total, "page", Value::Null),
        (_, _) => (0, total, "page", json!({ "to": "page", "why": crate::blocks::why_page(access, file.path()) })),
    };
    let unit = &page_chars[from as usize..to as usize];
    let given = cut_at(unit, asked.max_chars as usize);
    let shown = &unit[..given];
    let hidden = spans_in(&read.hidden, from, from + given as u32);

    let counts = json!({ "chars_in_unit": unit.len(), "chars_given": given, "hidden_spans": hidden.len() });
    let mut untrusted = json!({ "link": locator::link_of(file.path(), found.file_page + 1).ok() });
    let mut data_hidden = Value::Null;
    match asked.form {
        Form::Text => {
            untrusted["text"] = json!(shown.iter().collect::<String>());
            data_hidden = json!(hidden.iter().map(|span| [span.0, span.1]).collect::<Vec<_>>());
        }
        Form::Markdown => {
            let piece = Piece {
                kind: Kind::Paragraph,
                level: 0,
                depth: None,
                text: shown,
                hidden: &hidden,
            };
            untrusted["markdown"] = json!(markdown::render(&[piece]));
        }
    }

    // Refs: the unit's own, the page it is on, and (for a page) its neighbours in the tab as it stands.
    let opened = recto_session::identify::Opened::open(file.path(), found.stamp).ok_or_else(changed)?;
    let caps = ScanCaps::STANDARD;
    let mut referrer = Referrer::new(opened.reader(), opened.map(), &caps);
    let here = position;
    let up = page_ref(&mut referrer, Some(found.file_page));
    let refs = if kind == "hit" {
        json!({ "self": locator::encode(asked.reference), "next": null, "prev": null, "up": up })
    } else {
        let next = page_ref(&mut referrer, file.file_page_at(here + 1));
        let prev = page_ref(&mut referrer, here.checked_sub(1).and_then(|before| file.file_page_at(before)));
        json!({ "self": up, "next": next, "prev": prev, "up": null })
    };
    drop(opened);

    // The same version of the file, and of the index it was read from, to the end.
    crate::locating::still_located(access, &found)?;
    if let Some(key) = read.key
        && CorpusKey::of_file(file.path()).ok() != Some(key)
    {
        return Err(changed());
    }

    let mut data = json!({
        "tab": file.id(),
        "grain": asked.grain.word(),
        "as": asked.form.word(),
        "ref": found.told(),
        "unit": { "kind": kind, "pages": 1, "chars": unit.len() },
        "widened": widened,
        "headings_above": [],
        "refs": refs,
        "truncated": given < unit.len(),
        "counts": counts,
        "hidden_text": { "checked": read.checked, "partial": read.partial },
        "read_from": if read.key.is_some() { "index" } else { "file" },
        "untrusted": untrusted,
    });
    if !data_hidden.is_null() {
        data["hidden_spans"] = data_hidden;
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::index_is_enough;

    /// Failure: a stored page is given though the index never checked it for hidden text, or dropped some of its stretches, so its hidden words show as
    /// plain text. Only a checked page with no dropped stretch is enough.
    #[test]
    fn only_a_checked_page_with_no_dropped_stretch_is_taken_from_the_index() {
        assert!(index_is_enough(true, false));
        assert!(!index_is_enough(false, false));
        assert!(!index_is_enough(true, true));
    }
}
