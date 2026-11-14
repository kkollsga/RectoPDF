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

/// **The file's own highlights a tab has taken out, for drawing a page of its own file,** as the worker that draws that page leaves them out: so a picture,
/// print or SVG made from the tab never shows one the person has removed (coordinator, 2026-11-14). None for a page added from another PDF, or with
/// none taken out.
pub fn hidden_in<L: recto_session::DocLoader>(
    s: &recto_session::Session<L>,
    id: TabId,
    source: &recto_session::PageSource,
) -> Option<std::sync::Arc<recto_proto::hidden::HiddenAnnots>> {
    if source.added {
        return None;
    }
    // The whole file's set, whatever the page: a worker leaves out only what's on the page it draws, and one set for every page of the file is
    // what lets a print's pages share one worker (a set goes to it only when it's another).
    s.lists().get(id)?.marks().hidden_annots()
}

/// [`hidden_in`], through the session's access.
pub(crate) fn hidden_on<A: SessionAccess>(
    access: &A,
    id: TabId,
    source: &recto_session::PageSource,
) -> Option<std::sync::Arc<recto_proto::hidden::HiddenAnnots>> {
    access.with(|s| hidden_in(s, id, source))
}

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
        drawn_at(doc, 0)
    }

    fn drawn_at(doc: &mut recto_render_service::DocRenderer, page: u32) -> (u32, u32, Vec<u8>) {
        let (mut size, mut pixels, mut row) = ((0, 0), Vec::new(), Vec::new());
        draw_whole_page(doc, page, 1.0, 0, &OutputCaps::default(), &mut None, |event| {
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
        yellow_at(doc, 0)
    }

    fn yellow_at(doc: &mut recto_render_service::DocRenderer, page: u32) -> usize {
        yellow_within(doc, page, [0.0, 0.0, f32::MAX, f32::MAX])
    }

    /// [`yellow`]'s pixels inside `[left, top, right, bottom]` of a page, in points from its top left: one highlight's, and not a note's icon.
    fn yellow_within(doc: &mut recto_render_service::DocRenderer, page: u32, [left, top, right, bottom]: [f32; 4]) -> usize {
        let (width, _, pixels) = drawn_at(doc, page);
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(at, pixel)| {
                let (x, y) = ((at % width as usize) as f32 + 0.5, (at / width as usize) as f32 + 0.5);
                x > left && x < right && y > top && y < bottom && pixel[0] < 160 && pixel[2] > 200 && pixel[1] > 150
            })
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
        drop((doc, fresh));
        // Taken out by the command, and saved: the file has it no more.
        let [x, y, width, height] = found.rects[0];
        let remove = crate::commands::highlight::HighlightRemove {
            tab: TabRef::default(),
            what: crate::commands::highlight::RemoveWhat::At {
                page: 1,
                x: x + width / 2.0,
                y: y + height / 2.0,
            },
        };
        let dir = file.0.parent().unwrap().to_path_buf();
        let pictures = |session: &mut Session<crate::DrawableLoader>, name: &str| {
            let out = dir.join(format!("{name}.png"));
            let render = crate::commands::export::Render {
                tab: TabRef::default(),
                page: 1,
                scale: 1.0,
                out: out.clone(),
                anyway: false,
            };
            run(session, Command::from(render)).expect("rendered");
            let shot = dir.join(format!("{name}-shot.png"));
            let snapshot = crate::commands::export::Snapshot {
                tab: TabRef::default(),
                page: 1,
                rect: [0.0, 0.0, 300.0, 200.0],
                format: "png".into(),
                out: Some(shot.clone()),
                scale: 1.0,
                anyway: false,
            };
            run(session, Command::from(snapshot)).expect("snapshot");
            (std::fs::read(out).unwrap(), std::fs::read(shot).unwrap())
        };
        let with = pictures(&mut session, "with");
        run(&mut session, Command::from(remove)).expect("taken out");
        // A picture an agent makes of the tab follows the person's unsaved removal (coordinator, 2026-11-14): it isn't in it, and undone, it is.
        let without = pictures(&mut session, "without");
        assert!(
            without.0 != with.0 && without.1 != with.1,
            "render and snapshot leave out a highlight taken out"
        );
        run(&mut session, Command::from(crate::commands::editing::Undo { tab: TabRef::default() })).expect("undo");
        assert_eq!(pictures(&mut session, "undone"), with, "and put it back once that's undone");
        run(&mut session, Command::from(crate::commands::editing::Redo { tab: TabRef::default() })).expect("redo");
        run(
            &mut session,
            Command::from(crate::commands::saving::Save {
                tab: TabRef::default(),
                out: None,
                replace: false,
            }),
        )
        .expect("saved");
        assert_eq!(
            yellow(&mut crate::render::open_for_text(&file.0).expect("a worker")),
            0,
            "saved, it's gone from the file"
        );
    }

    /// **Failure (second review of R4.3, S1): a file's highlight taken out and saved is still drawn by another reader**, because the file reaches it
    /// besides the place it was taken from: listed twice on its page and once on another, named by a reply's `/IRT` and the structure tree's `/OBJR`.
    /// PDFium draws both pages before (yellow on each) and after the save (none on either).
    #[test]
    fn a_file_highlight_reached_many_ways_is_drawn_nowhere_once_saved_out() {
        let highlight = "<< /Type /Annot /Subtype /Highlight /Rect [50 700 150 712] /QuadPoints [50 712 150 712 50 700 150 700] /AP << /N 7 0 R >> /Popup 8 0 R /StructParent 1 >>";
        let bytes = recto_testpdf::build(&[
            recto_testpdf::Object::Dict("<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 11 0 R >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Pages /Kids [3 0 R 10 0 R] /Count 2 >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Annots [5 0 R 6 0 R 9 0 R 6 0 R] >>".into()),
            recto_testpdf::Object::Stream {
                dict: String::new(),
                data: text_lines(&["Words"]),
            },
            recto_testpdf::Object::Dict("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".into()),
            recto_testpdf::Object::Dict(highlight.into()),
            recto_testpdf::Object::Stream {
                dict: "/Type /XObject /Subtype /Form /BBox [50 700 150 712]".into(),
                data: b"1 0.88 0.3 rg 50 700 100 12 re f".to_vec(),
            },
            recto_testpdf::Object::Dict("<< /Type /Annot /Subtype /Popup /Rect [200 700 300 760] /Parent 6 0 R >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /IRT 6 0 R /Contents (A reply) >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Annots [6 0 R] >>".into()),
            recto_testpdf::Object::Dict("<< /Type /StructTreeRoot /K 12 0 R >>".into()),
            recto_testpdf::Object::Dict("<< /Type /StructElem /S /Annot /P 11 0 R /Pg 3 0 R /K << /Type /OBJR /Obj 6 0 R >> >>".into()),
        ]);
        let file = TempPdf::new("reached.pdf", &bytes);
        // The highlight's own rectangle, from the top left: the reply's note draws a yellow icon of its own elsewhere.
        let both = |path: &std::path::Path| {
            let mut doc = crate::render::open_for_text(path).expect("a worker");
            [0, 1].map(|page| yellow_within(&mut doc, page, [50.0, 80.0, 150.0, 92.0]))
        };
        let before = both(&file.0);
        assert!(before.iter().all(|shown| *shown > 500), "drawn on both pages before: {before:?}");
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
        let remove = crate::commands::highlight::HighlightRemove {
            tab: TabRef::default(),
            what: crate::commands::highlight::RemoveWhat::At { page: 1, x: 100.0, y: 86.0 },
        };
        run(&mut session, Command::from(remove)).expect("taken out");
        let save = crate::commands::saving::Save {
            tab: TabRef::default(),
            out: None,
            replace: false,
        };
        run(&mut session, Command::from(save)).expect("saved");
        assert_eq!(both(&file.0), [0, 0], "drawn on neither page once it's saved out");
    }

    /// **Failure (second review of R4.3, S2): the history names the wrong place for a highlight a rewrite wrote**: the save reported the place it worked
    /// out before the rewrite took a link to a deleted page out of the same list, so an undo after it hides another place (here past the list's end)
    /// and the next save refuses it. A contents page whose first link goes to page 5: page 5 deleted, a line highlighted on page 1, Save, undo: the
    /// place taken out is the highlight's, and the next Save takes out exactly it, the note beside it staying.
    #[test]
    fn an_undo_after_a_rewrite_names_the_highlights_place() {
        let mut objects = vec![
            recto_testpdf::Object::Dict("<< /Type /Catalog /Pages 2 0 R >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Pages /Kids [3 0 R 7 0 R 8 0 R 9 0 R 10 0 R] /Count 5 >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Annots [5 0 R 6 0 R] >>".into()),
            recto_testpdf::Object::Stream {
                dict: String::new(),
                data: text_lines(&["Contents of this document"]),
            },
            recto_testpdf::Object::Dict("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /Dest [10 0 R /Fit] >>".into()),
            recto_testpdf::Object::Dict("<< /Type /Annot /Subtype /Text /Rect [500 20 520 40] /Contents (A note) >>".into()),
        ];
        objects.extend((0..4).map(|_| recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".into())));
        let file = TempPdf::new("contents.pdf", &recto_testpdf::build(&objects));
        let mut session = Session::new(crate::DrawableLoader);
        let run = |session: &mut Session<crate::DrawableLoader>, command: Command| run_command(session, Level::Full, &command);
        let open = crate::commands::tabs::Open {
            path: file.0.clone(),
            page: None,
        };
        run(&mut session, Command::from(open)).unwrap();
        let highlight = crate::commands::highlight::Highlight {
            tab: TabRef::default(),
            page: Some(1),
            to_page: None,
            chars: Some("0-7".into()),
            at: None,
            to: None,
            unit: None,
        };
        let delete = crate::commands::editing::PageDelete {
            tab: TabRef::default(),
            pages: "5".into(),
        };
        // The delete first: with no copy kept of the file's pages here, its step rolls off at the save, and the highlight's stays above it.
        run(&mut session, Command::from(delete)).expect("deleted");
        run(&mut session, Command::from(highlight)).expect("highlighted");
        let save = || {
            Command::from(crate::commands::saving::Save {
                tab: TabRef::default(),
                out: None,
                replace: false,
            })
        };
        run(&mut session, save()).expect("saved");
        let found = recto_core::highlights::page_highlights(&file.0, 0).unwrap().found;
        assert_eq!(found.len(), 1, "the highlight is the file's");
        run(&mut session, Command::from(crate::commands::editing::Undo { tab: TabRef::default() })).expect("undo");
        let listed = list(&Direct(RefCell::new(&mut session), None), &TabRef::default(), None).unwrap();
        assert_eq!(
            listed["removed"][0]["index"].as_u64(),
            Some(u64::from(found[0].index)),
            "the place taken out is the highlight's: {listed}"
        );
        run(&mut session, save()).expect("saved again");
        assert!(recto_core::highlights::page_highlights(&file.0, 0).unwrap().found.is_empty());
        let reader = lopdf::IndexedReader::open(lopdf::FileSource::open(&file.0).unwrap()).unwrap();
        let map = reader.page_map().unwrap();
        let lopdf::Object::Dictionary(page) = reader.resolve_object(map.get(0).unwrap().id()).unwrap() else {
            panic!("page 1")
        };
        assert_eq!(page.get(b"Annots").unwrap().as_array().unwrap().len(), 1, "the note stays");
    }

    /// Every place in the app's crates that opens a renderer, outside tests: the file under `crates/`, how many it opens, and for one that draws a
    /// tab's pages, how many times it hands that renderer the tab's set (`hidden.draw_for(`); 0 for one that reads text or structure, copies an
    /// embedded image as it is, or draws no tab.
    const RENDERERS: &[(&str, usize, usize, &str)] = &[
        ("recto-agent/src/agreement.rs", 1, 0, "the agreement gate's look at the file's text"),
        (
            "recto-agent/src/extract.rs",
            6,
            3,
            "a snapshot's picture and SVG (the agent's), the Snapshot tool's PNG and JPG, and its SVG; the rest copy an embedded image or read text",
        ),
        ("recto-agent/src/loader.rs", 1, 0, "a file's facts when it's opened"),
        ("recto-agent/src/print.rs", 1, 1, "each page printed"),
        ("recto-agent/src/render.rs", 3, 1, "`recto render`; the others are `open_for_text` itself, named and opening"),
        ("recto-agent/src/render/image.rs", 1, 0, "the embedded images a page has"),
        ("recto-agent/src/render/snapshot.rs", 1, 0, "whether the file allows copying"),
        ("recto-agent/src/render/svg.rs", 1, 1, "`recto export svg`"),
        ("recto-agent/src/search.rs", 1, 0, "the text searched"),
        ("recto-agent/src/selecting.rs", 1, 0, "the characters a pick takes"),
        ("recto-app/src/core/updates.rs", 1, 0, "a new version's own test page, no tab's"),
        (
            "recto-reader/src/engine/run.rs",
            1,
            1,
            "the page view's tiles: `renderer_for` hands over the tab's set",
        ),
    ];

    /// **Failure (second review of R4.3, S3): a new way of drawing a tab's pages skips the tab's set**, as the Snapshot tool's did, and copies or shows
    /// a highlight the person took out. Every renderer opened in the app's crates outside tests is in [`RENDERERS`] with what it's for, and one that
    /// draws a tab hands it the set as often as it opens one: a new one fails here until it's said which it is.
    #[test]
    fn every_renderer_that_draws_a_tab_is_given_its_set() {
        fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files(&path, out);
                } else if path.extension().is_some_and(|ending| ending == "rs") {
                    out.push(path);
                }
            }
        }
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let mut found = Vec::new();
        for krate in ["recto-agent", "recto-app", "recto-reader", "recto-ffi", "recto-session", "recto-control"] {
            let mut paths = Vec::new();
            files(&crates.join(krate).join("src"), &mut paths);
            for path in paths {
                let text = std::fs::read_to_string(&path).unwrap();
                let code = text.split("#[cfg(test)]").next().unwrap_or_default();
                let lines = || code.lines().filter(|line| !line.trim_start().starts_with("//"));
                let opens = lines()
                    .map(|line| {
                        ["open_for_text(", "DocRenderer::new(", "DocRenderer::with_file("]
                            .iter()
                            .map(|call| line.matches(call).count())
                            .sum::<usize>()
                    })
                    .sum::<usize>();
                let given = lines().map(|line| line.matches("hidden.draw_for(").count()).sum::<usize>();
                if opens > 0 {
                    let name = path.strip_prefix(crates).unwrap().to_string_lossy().replace('\\', "/");
                    found.push((name, opens, given));
                }
            }
        }
        found.sort();
        let listed: Vec<(String, usize, usize)> = RENDERERS.iter().map(|(name, opens, given, _)| (name.to_string(), *opens, *given)).collect();
        assert_eq!(
            found, listed,
            "a renderer opened or given the tab's set where RENDERERS doesn't say: one that draws a tab's pages hands it `hidden_in`'s set"
        );
    }
}
