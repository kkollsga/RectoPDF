//! **Highlights** (SPEC section 6, Annotations; R4.3): `recto highlight`, `recto highlight remove` and `recto highlights`, and the person's Mark Up >
//! Highlight and Remove Highlight, which send the same commands. A highlight is an edit like a turn (`recto_session::edit::marks`), so undo, redo,
//! the journal, the Save icon, discard, the close prompt and who made it all hold as for one, through the one edit path ([`crate::edit`]) and its rules
//! about the owner's edits. What's highlighted is exactly what `recto select` takes for the same arguments: the pick is read by [`selecting`], and its
//! rectangles become the highlight's. No text of the document is in any answer here.
//!
//! **A file's own highlights** are listed, read through `recto_core::highlights` (geometry and colour, never a string); taking one out isn't built yet
//! (the coordinator's scope cut, 2026-11-14), and asking says so.

use crate::Code;
use crate::selecting::{self, Pick};
use crate::{Actor, AgentError, Event, SessionAccess, TabRef, exit, locate, select};
use recto_session::edit::EditError;
use recto_session::edit::marks::{MarkChange, MarkPage, MarkRect};
use recto_session::{Author, TabId};
use serde_json::{Value, json};
use std::cell::Cell;

/// The rectangles a pick takes, by page position: what `recto select` gives, sound ones only.
fn picked<A: SessionAccess>(access: &A, pick: &Pick) -> Result<Vec<(u32, Vec<MarkRect>)>, AgentError> {
    let data = selecting::run(access, pick, true)?;
    let mut out = Vec::new();
    for page in data["pages"].as_array().map_or(&[][..], Vec::as_slice) {
        let Some(position) = page["page"].as_u64().and_then(|page| u32::try_from(page).ok()?.checked_sub(1)) else {
            continue;
        };
        let rects: Vec<MarkRect> = page["rects"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(|rect| {
                let edge = |at: usize| rect.get(at).and_then(Value::as_f64).map(|edge| edge as f32);
                Some(MarkRect {
                    x: edge(0)?,
                    y: edge(1)?,
                    width: edge(2)?,
                    height: edge(3)?,
                })
            })
            .filter(|rect| rect.is_sound())
            .collect();
        if !rects.is_empty() {
            out.push((position, rects));
        }
    }
    Ok(out)
}

fn moved_meanwhile() -> AgentError {
    AgentError::new(
        Code::CHANGED,
        "The pages were moved while what to highlight was being read, so nothing was highlighted.",
        exit::FAILED,
    )
    .then("Ask again: it reads the pages as they are now.")
}

/// **`recto highlight`, and the person's Mark Up > Highlight:** what the pick takes, highlighted, as one step whatever pages it runs over. The pages
/// are read first, off the session's lock; if they were moved meanwhile, nothing is done and that's said, as the read was of other pages.
pub(crate) fn add<A: SessionAccess>(access: &A, actor: Actor, events: &mut Vec<Event>, pick: &Pick) -> Result<Value, AgentError> {
    let (id, before) = access.with(|s| {
        let id = select(s, pick.tab)?;
        Ok::<_, AgentError>((id, s.lists().list_mark(id)))
    })?;
    let pages = picked(access, pick)?;
    if pages.is_empty() {
        return Err(AgentError::new(
            Code::NO_TEXT_THERE,
            "Nothing was highlighted: what was asked for takes no text.",
            exit::NOT_FOUND,
        ));
    }
    let lines: usize = pages.iter().map(|(_, rects)| rects.len()).sum();
    let did = match (pages.first(), pages.last()) {
        (Some((first, _)), Some((last, _))) if first != last => format!("Highlighted {lines} lines on pages {}-{}.", first + 1, last + 1),
        (Some((page, _)), _) => format!(
            "Highlighted {} on page {}.",
            if lines == 1 { "1 line".to_string() } else { format!("{lines} lines") },
            page + 1
        ),
        _ => String::new(),
    };
    let moved = Cell::new(false);
    let positions: Vec<u32> = pages.iter().map(|(position, _)| *position).collect();
    let result = crate::edit(access, actor, &TabRef::Id(id), events, did, |edits| {
        if edits.order_mark() != before {
            moved.set(true);
            return Err(EditError::OutOfRange);
        }
        let mut on = Vec::with_capacity(pages.len());
        for (position, rects) in pages {
            let (source, page) = edits.page_at(position).ok_or(EditError::OutOfRange)?;
            on.push(MarkPage { source, page, rects });
        }
        edits.mark(MarkChange::Add { pages: on })
    });
    let mut data = result.map_err(|error| if moved.get() { moved_meanwhile() } else { error })?;
    data["highlighted"] = json!({ "pages": positions.iter().map(|position| position + 1).collect::<Vec<_>>(), "lines": lines });
    Ok(data)
}

/// What `recto highlight remove` is asked to take away.
pub(crate) enum Which<'a> {
    /// One highlight's part, by its number in `recto highlights`.
    Id(u32),
    /// Whatever is under a point on a page, as a right-click finds it.
    At { page: u32, x: f32, y: f32 },
    /// Whatever a pick's text touches, as the person's selection does.
    Pick(&'a Pick<'a>),
}

/// Whether two rectangles `[x, y, width, height]` share any area.
fn overlaps(a: MarkRect, b: MarkRect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// The tab's unsaved highlights that a test says yes to, given each one's page position (none for a page that isn't in the document now).
fn edits_where<A: SessionAccess>(access: &A, id: TabId, mut wanted: impl FnMut(u32, &[MarkRect]) -> bool) -> Vec<u32> {
    access.with(|s| {
        let Some(list) = s.lists().get(id) else { return Vec::new() };
        list.marks()
            .iter()
            .filter(|mark| {
                let position = list.slots().iter().position(|slot| (slot.source, slot.page) == (mark.source, mark.page));
                position.is_some_and(|position| wanted(u32::try_from(position).unwrap_or(u32::MAX), &mark.rects))
            })
            .map(|mark| mark.id)
            .collect()
    })
}

/// Whether a file highlight is under the test, on these pages: what's said instead when only one of the file's own is there.
fn in_file<A: SessionAccess>(access: &A, tab: &TabRef, pages: &[u32], wanted: impl Fn(u32, MarkRect) -> bool) -> bool {
    pages.iter().any(|position| {
        locate(access, tab, position + 1)
            .ok()
            .and_then(|(_, _, source)| recto_core::highlights::page_highlights(&source.path, source.page).ok())
            .is_some_and(|read| {
                read.found.iter().flat_map(|found| &found.rects).any(|[x, y, width, height]| {
                    wanted(
                        *position,
                        MarkRect {
                            x: *x,
                            y: *y,
                            width: *width,
                            height: *height,
                        },
                    )
                })
            })
    })
}

/// Whether a rectangle on a page position is what a removal is about.
type Touches = Box<dyn Fn(u32, MarkRect) -> bool>;

/// **`recto highlight remove`, and the person's Mark Up > Remove Highlight:** unsaved highlights taken away, as one step. One that's already in the
/// file can't be yet, and asking for one says so rather than doing nothing.
pub(crate) fn remove<A: SessionAccess>(access: &A, actor: Actor, events: &mut Vec<Event>, tab: &TabRef, which: Which<'_>) -> Result<Value, AgentError> {
    let id = access.with(|s| select(s, tab))?;
    let (ids, pages, test): (Vec<u32>, Vec<u32>, Touches) = match which {
        Which::Id(number) => (vec![number], Vec::new(), Box::new(|_, _| false)),
        Which::At { page, x, y } => {
            let position = page.saturating_sub(1);
            let point = MarkRect {
                x,
                y,
                width: f32::EPSILON,
                height: f32::EPSILON,
            };
            let test = move |on: u32, rect: MarkRect| on == position && overlaps(rect, point);
            (
                edits_where(access, id, |on, rects| rects.iter().any(|rect| test(on, *rect))),
                vec![position],
                Box::new(test),
            )
        }
        Which::Pick(pick) => {
            let taken = picked(access, pick)?;
            let pages = taken.iter().map(|(position, _)| *position).collect();
            let test = move |on: u32, rect: MarkRect| {
                taken
                    .iter()
                    .any(|(position, rects)| *position == on && rects.iter().any(|picked| overlaps(*picked, rect)))
            };
            (
                edits_where(access, id, |on, rects| rects.iter().any(|rect| test(on, *rect))),
                pages,
                Box::new(test),
            )
        }
    };
    if ids.is_empty() {
        if in_file(access, &TabRef::Id(id), &pages, test) {
            return Err(AgentError::new(
                Code::HIGHLIGHT_IN_FILE,
                "That highlight is in the file already, and taking those out isn't built yet. Nothing was removed.",
                exit::FAILED,
            ));
        }
        return Err(AgentError::new(
            Code::NO_SUCH_HIGHLIGHT,
            "There's no unsaved highlight there, so nothing was removed.",
            exit::NOT_FOUND,
        ));
    }
    let did = if ids.len() == 1 {
        "Removed a highlight.".to_string()
    } else {
        format!("Removed {} highlights.", ids.len())
    };
    crate::edit(access, actor, &TabRef::Id(id), events, did, |edits| edits.mark(MarkChange::Remove { ids }))
}

/// **`recto highlights`:** the tab's unsaved highlights, each with its number, page, lines and who made it; and, with `--page`, the file's own on that
/// page, by their place in its list, read off the session's lock. Geometry and colour only.
pub(crate) fn list<A: SessionAccess>(access: &A, tab: &TabRef, page: Option<u32>) -> Result<Value, AgentError> {
    let (id, edits) = access.with(|s| {
        let id = select(s, tab)?;
        let edits: Vec<Value> = s.lists().get(id).map_or_else(Vec::new, |list| {
            list.marks()
                .iter()
                .map(|mark| {
                    let position = list.slots().iter().position(|slot| (slot.source, slot.page) == (mark.source, mark.page));
                    let rects: Vec<[f32; 4]> = mark.rects.iter().map(|rect| [rect.x, rect.y, rect.width, rect.height]).collect();
                    let by = if mark.by == Author::Agent { "agent" } else { "user" };
                    json!({ "id": mark.id, "page": position.map(|position| position + 1), "rects": rects, "by": by, "from": "edit" })
                })
                .collect()
        });
        Ok::<_, AgentError>((id, edits))
    })?;
    let file = match page {
        None => Value::Null,
        Some(page) => {
            let (_, _, source) = locate(access, &TabRef::Id(id), page)?;
            let read = recto_core::highlights::page_highlights(&source.path, source.page).map_err(AgentError::from)?;
            let found: Vec<Value> = read
                .found
                .iter()
                .map(|found| json!({ "index": found.index, "rects": found.rects, "colour": found.rgb.map(|rgb| format!("#{rgb:06X}")), "from": "file" }))
                .collect();
            json!({ "page": page, "highlights": found, "truncated": read.truncated })
        }
    };
    Ok(json!({
        "tab": id.0,
        "edits": edits,
        "file": file,
        "colour": format!("#{:06X}", recto_core::save::HIGHLIGHT_RGB),
        "note": "`rects` are x, y, width, height in points on the page as the file shows it, from its top left. `edits` are unsaved, and `recto highlight remove --id N` takes one away; `file` lists the file's own on `--page`, which can't be taken out yet.",
    }))
}
