//! **Highlights** (SPEC section 6, Annotations; R4.3): `recto highlight`, `recto highlight remove` and `recto highlights`, and the person's Mark Up >
//! Highlight and Remove Highlight, which send the same commands. A highlight is an edit like a turn (`recto_session::edit::marks`), so undo, redo,
//! the journal, the Save icon, discard, the close prompt and who made it all hold as for one, through the one edit path ([`crate::edit`]) and its rules
//! about the owner's edits. What's highlighted is exactly what `recto select` takes for the same arguments: the pick is read by [`selecting`], and its
//! rectangles become the highlight's. No text of the document is in any answer here.
//!
//! **A file's own highlights** are listed, read through `recto_core::highlights` (geometry and colour, never a string), and taken out by the same
//! `recto highlight remove` (review of R4.3, S2): an edit like any other, which hides the highlight from the page as drawn until a save takes it out of
//! the file by a rewrite (`recto_session::edit::marks::FileMark`). Only the tab's own file's: one on a page added from another PDF is refused until
//! a save makes that page the file's (coordinator, 2026-11-14), for the person and an agent alike.

use crate::Code;
use crate::selecting::{self, Pick};
use crate::{Actor, AgentError, Event, SessionAccess, TabRef, exit, locate, select};
use recto_session::edit::EditError;
use recto_session::edit::marks::{FileKey, FileMark, MarkChange, MarkPage, MarkRect};
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

/// The tab's unsaved highlights that a test says yes to, given each one's page position (none for a page that isn't in the document now). Each
/// highlight is on every position its page has; one look along the pages (review of R4.3: it was one for every highlight).
fn edits_where<A: SessionAccess>(access: &A, id: TabId, mut wanted: impl FnMut(u32, &[MarkRect]) -> bool) -> Vec<u32> {
    access.with(|s| {
        let Some(list) = s.lists().get(id) else { return Vec::new() };
        let mut ids: Vec<u32> = list
            .marks()
            .on_positions(list.slots())
            .into_iter()
            .filter(|(position, mark)| wanted(*position, &mark.rects))
            .map(|(_, mark)| mark.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    })
}

fn mark_rect([x, y, width, height]: [f32; 4]) -> MarkRect {
    MarkRect { x, y, width, height }
}

/// **The file's own highlights a test says yes to, on these pages,** read off the session's lock, as removals name them; those already taken out
/// are left out. One on a page added from another PDF refuses: see the module's notes.
fn files_where<A: SessionAccess>(access: &A, id: TabId, pages: &[u32], wanted: &dyn Fn(u32, MarkRect) -> bool) -> Result<Vec<FileMark>, AgentError> {
    let hidden: Vec<FileKey> = access.with(|s| s.lists().get(id).map_or_else(Vec::new, |list| list.marks().hidden().to_vec()));
    let mut out = Vec::new();
    for position in pages {
        let (_, _, source) = locate(access, &TabRef::Id(id), position + 1)?;
        let read = recto_core::highlights::page_highlights(&source.path, source.page).map_err(AgentError::from)?;
        for found in &read.found {
            if !found.rects.iter().any(|rect| wanted(*position, mark_rect(*rect))) {
                continue;
            }
            if source.added {
                return Err(AgentError::new(
                    Code::HIGHLIGHT_IN_ADDED_FILE,
                    "That highlight came with a PDF added to this one, so it can't be removed until the document is saved. Nothing was removed.",
                    exit::FAILED,
                ));
            }
            let Some(rect) = found.rect else { continue };
            let key = FileKey {
                source: 0,
                page: source.page,
                index: found.index,
                rect,
            };
            if hidden.contains(&key) || out.iter().any(|file: &FileMark| file.key == key) {
                continue;
            }
            out.push(FileMark {
                key,
                rects: found.rects.iter().copied().map(mark_rect).collect(),
            });
        }
    }
    Ok(out)
}

/// Whether a rectangle on a page position is what a removal is about.
type Touches = Box<dyn Fn(u32, MarkRect) -> bool>;

/// **`recto highlight remove`, and the person's Mark Up > Remove Highlight:** highlights taken away, as one step: those made as edits, and the file's
/// own under the point or touched by the pick (taken out of the file by the next save). By number, only one made as an edit.
pub(crate) fn remove<A: SessionAccess>(access: &A, actor: Actor, events: &mut Vec<Event>, tab: &TabRef, which: Which<'_>) -> Result<Value, AgentError> {
    let id = access.with(|s| select(s, tab))?;
    let (ids, pages, test): (Vec<u32>, Vec<u32>, Touches) = match which {
        Which::Id(number) => (vec![number], Vec::new(), Box::new(|_, _| false)),
        Which::At { page, x, y } => {
            let position = page
                .checked_sub(1)
                .ok_or_else(|| AgentError::new(Code::NO_SUCH_PAGE, "Pages count from 1.", exit::NOT_FOUND))?;
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
    let files = files_where(access, id, &pages, &*test)?;
    if ids.is_empty() && files.is_empty() {
        return Err(AgentError::new(
            Code::NO_SUCH_HIGHLIGHT,
            "There's no highlight there, so nothing was removed.",
            exit::NOT_FOUND,
        ));
    }
    let did = match ids.len() + files.len() {
        1 => "Removed a highlight.".to_string(),
        many => format!("Removed {many} highlights."),
    };
    let change = if files.is_empty() {
        MarkChange::Remove { ids }
    } else {
        MarkChange::RemoveFile { files, ids }
    };
    crate::edit(access, actor, &TabRef::Id(id), events, did, |edits| edits.mark(change))
}

/// **`recto highlights`:** the tab's unsaved highlights, each with its number, page, lines and who made it; the file's own taken out and unsaved, by
/// page and place; and, with `--page`, the file's own on that page, by their place in its list, read off the session's lock. Geometry and colour
/// only.
pub(crate) fn list<A: SessionAccess>(access: &A, tab: &TabRef, page: Option<u32>) -> Result<Value, AgentError> {
    let (id, edits, removed) = access.with(|s| {
        let id = select(s, tab)?;
        let Some(list) = s.lists().get(id) else {
            return Ok::<_, AgentError>((id, Vec::new(), Vec::new()));
        };
        let first = list.marks().first_positions(list.slots());
        let edits: Vec<Value> = list
            .marks()
            .iter()
            .map(|mark| {
                let rects: Vec<[f32; 4]> = mark.rects.iter().map(|rect| [rect.x, rect.y, rect.width, rect.height]).collect();
                let by = if mark.by == Author::Agent { "agent" } else { "user" };
                json!({ "id": mark.id, "page": first.get(&mark.id).map(|position| position + 1), "rects": rects, "by": by, "from": "edit" })
            })
            .collect();
        let removed: Vec<(Option<u32>, FileKey)> = list.marks().hidden().iter().map(|key| (list.position_of(key.page), *key)).collect();
        Ok((id, edits, removed))
    })?;
    let listed_removed: Vec<Value> = removed
        .iter()
        .map(|(position, key)| json!({ "page": position.map(|position| position + 1), "index": key.index, "from": "file", "removed": true }))
        .collect();
    let file = match page {
        None => Value::Null,
        Some(page) => {
            let (_, _, source) = locate(access, &TabRef::Id(id), page)?;
            let read = recto_core::highlights::page_highlights(&source.path, source.page).map_err(AgentError::from)?;
            let taken = |index: u32| !source.added && removed.iter().any(|(_, key)| key.page == source.page && key.index == index);
            let found: Vec<Value> = read
                .found
                .iter()
                .map(|found| json!({ "index": found.index, "rects": found.rects, "colour": found.rgb.map(|rgb| format!("#{rgb:06X}")), "from": "file", "removed": taken(found.index) }))
                .collect();
            json!({ "page": page, "highlights": found, "truncated": read.truncated })
        }
    };
    Ok(json!({
        "tab": id.0,
        "edits": edits,
        "removed": listed_removed,
        "file": file,
        "colour": format!("#{:06X}", recto_core::save::HIGHLIGHT_RGB),
        "note": "`rects` are x, y, width, height in points on the page as the file shows it, from its top left. `edits` are unsaved, and `recto highlight remove --id N` takes one away. `removed` are the file's own taken out, which the next save takes out of the file; `recto undo` puts one back. `file` lists the file's own on `--page`, and `recto highlight remove --page N --at X,Y` takes one out.",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Direct, Level, run as run_command};
    use recto_render_service::{OutputCaps, PageEvent, draw_whole_page};
    use recto_session::Session;
    use recto_testpdf::{Page, TempPdf, pages, text_lines};
    use std::cell::RefCell;

    /// A page drawn by PDFium in the worker, at one pixel a point: its width, height, and BGRA pixels, top row first.
    fn drawn(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
        drawn_by(&mut crate::render::open_for_text(path).expect("a worker"))
    }

    fn drawn_by(doc: &mut recto_render_service::DocRenderer) -> (u32, u32, Vec<u8>) {
        let (mut size, mut pixels, mut row) = ((0, 0), Vec::new(), Vec::new());
        draw_whole_page(doc, 0, 1.0, 0, &OutputCaps::default(), &mut None, |event| {
            match event {
                PageEvent::Begin { width, height } => {
                    size = (width, height);
                    row = vec![0u8; width as usize * 4];
                }
                PageEvent::Rows { memory, rect, stride, .. } => {
                    for y in 0..rect.height as usize {
                        assert!(memory.copy_out(y * stride as usize, &mut row));
                        pixels.extend_from_slice(&row);
                    }
                }
            }
            Ok(())
        })
        .expect("drawn");
        (size.0, size.1, pixels)
    }

    /// **Failure: a saved highlight isn't drawn where its text is, by a reader that isn't the one that wrote it** (R4.3, coordinator 2026-11-14): the
    /// write and the read-back agree with each other and both are wrong, on a plain page or on one turned a quarter and cropped. The page is drawn by
    /// PDFium before and after the save: inside the highlight's rectangle (where the page was white) the blue of the yellow comes down, and away from
    /// it nothing changed. The property, not the pixels.
    #[test]
    fn a_saved_highlight_is_drawn_by_pdfium_where_its_text_is() {
        for extra in ["", "/Rotate 90 /CropBox [40 60 560 740]"] {
            let mut page = Page::new(text_lines(&["Highlighted words on a page"]));
            page.page_extra = extra.into();
            let file = TempPdf::new("highlighted.pdf", &pages(&[page]));
            let before = file.0.with_file_name("before.pdf");
            std::fs::copy(&file.0, &before).unwrap();
            let mut session = Session::new(crate::DrawableLoader);
            let run = |session: &mut Session<crate::DrawableLoader>, command: Command| run_command(session, Level::Full, &command);
            run(
                &mut session,
                Command::from(crate::commands::tabs::Open {
                    path: file.0.clone(),
                    page: None,
                }),
            )
            .unwrap();
            let highlight = crate::commands::highlight::Highlight {
                tab: TabRef::default(),
                page: Some(1),
                to_page: None,
                chars: Some("0-11".into()),
                at: None,
                to: None,
                unit: None,
            };
            run(&mut session, Command::from(highlight)).expect("highlighted");
            let listed = list(&Direct(RefCell::new(&mut session), None), &TabRef::default(), None).unwrap();
            let rect: Vec<f64> = listed["edits"][0]["rects"][0]
                .as_array()
                .unwrap()
                .iter()
                .map(|edge| edge.as_f64().unwrap())
                .collect();
            run(
                &mut session,
                Command::from(crate::commands::saving::Save {
                    tab: TabRef::default(),
                    out: None,
                    replace: false,
                }),
            )
            .expect("saved");
            let ((width, height, was), (_, _, now)) = (drawn(&before), drawn(&file.0));
            let mut inside_changed = 0;
            let mut inside_white = 0;
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let at = (y * width as usize + x) * 4;
                    let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
                    let inside = fx > rect[0] + 1.0 && fx < rect[0] + rect[2] - 1.0 && fy > rect[1] + 1.0 && fy < rect[1] + rect[3] - 1.0;
                    let near = fx > rect[0] - 3.0 && fx < rect[0] + rect[2] + 3.0 && fy > rect[1] - 3.0 && fy < rect[1] + rect[3] + 3.0;
                    if inside && was[at..at + 3].iter().all(|channel| *channel > 240) {
                        inside_white += 1;
                        // BGRA: the yellow takes the blue down, and leaves the red.
                        inside_changed += usize::from(now[at] < 160 && now[at + 2] > 200);
                    } else if !near {
                        assert_eq!(
                            &now[at..at + 4],
                            &was[at..at + 4],
                            "{extra:?}: ({x}, {y}) changed away from the highlight at {rect:?}"
                        );
                    }
                }
            }
            assert!(inside_white > 50, "{extra:?}: a highlight of some size: {rect:?}");
            assert!(
                inside_changed * 10 > inside_white * 9,
                "{extra:?}: {inside_changed} of {inside_white} of the highlight's white went yellow"
            );
        }
    }

    /// Pixels a highlight's yellow is on: the blue taken down, the red left.
    fn yellow(doc: &mut recto_render_service::DocRenderer) -> usize {
        let (_, _, pixels) = drawn_by(doc);
        pixels
            .chunks_exact(4)
            .filter(|pixel| pixel[0] < 160 && pixel[2] > 200 && pixel[1] > 150)
            .count()
    }

    /// **Failure (review of R4.3, S2): a file's highlight taken out and unsaved is still drawn by the worker**, or stays hidden once undone, or a
    /// set names another annotation than the file has there (the file changed on disk since) and hides it anyway, or a renderer that draws one file
    /// for two tabs (one's own file, the other's added pages) shows one tab's removal in the other's view. PDFium draws the page each time.
    #[test]
    fn a_hidden_file_highlight_isnt_drawn_and_only_in_its_own_tabs_view() {
        use recto_proto::hidden::HiddenAnnots;
        use std::sync::Arc;
        let file = TempPdf::new("marked.pdf", &pages(&[Page::new(text_lines(&["Highlighted words on a page"]))]));
        let mut session = Session::new(crate::DrawableLoader);
        let run = |session: &mut Session<crate::DrawableLoader>, command: Command| run_command(session, Level::Full, &command);
        run(
            &mut session,
            Command::from(crate::commands::tabs::Open {
                path: file.0.clone(),
                page: None,
            }),
        )
        .unwrap();
        let highlight = crate::commands::highlight::Highlight {
            tab: TabRef::default(),
            page: Some(1),
            to_page: None,
            chars: Some("0-11".into()),
            at: None,
            to: None,
            unit: None,
        };
        run(&mut session, Command::from(highlight)).expect("highlighted");
        run(
            &mut session,
            Command::from(crate::commands::saving::Save {
                tab: TabRef::default(),
                out: None,
                replace: false,
            }),
        )
        .expect("saved");
        let found = recto_core::highlights::page_highlights(&file.0, 0).unwrap().found.remove(0);
        let (index, rect) = (found.index, found.rect.unwrap());
        let set = |generation: u64, rect: [f32; 4]| {
            Some(Arc::new(HiddenAnnots {
                generation,
                pages: vec![(0, vec![(index, rect)])],
            }))
        };
        let mut doc = crate::render::open_for_text(&file.0).expect("a worker");
        let shown = yellow(&mut doc);
        assert!(shown > 50, "the file's highlight is drawn: {shown}");
        doc.hidden.draw_for(2, set(1, rect));
        assert_eq!(yellow(&mut doc), 0, "taken out, it isn't");
        doc.hidden.draw_for(1, None);
        assert_eq!(yellow(&mut doc), shown, "another tab's view of the same file has it");
        doc.hidden.draw_for(2, set(1, rect));
        assert_eq!(yellow(&mut doc), 0, "and back in the first view, it's out again");
        doc.hidden.draw_for(2, None);
        assert_eq!(yellow(&mut doc), shown, "undone, it's drawn again");
        let moved = [rect[0] + 2.0, rect[1], rect[2] + 2.0, rect[3]];
        doc.hidden.draw_for(2, set(2, moved));
        assert_eq!(yellow(&mut doc), shown, "a set that isn't what the file has there hides nothing");
        let mut fresh = crate::render::open_for_text(&file.0).expect("a worker");
        fresh.hidden.draw_for(2, set(3, rect));
        assert_eq!(yellow(&mut fresh), 0, "a worker started with a set hides from its first draw");
    }
}
