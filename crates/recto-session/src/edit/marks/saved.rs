//! **What a save asks of a list's highlights, and what a confirmed save does to them** (review of R4.3, S2 and S3). The history goes on past a save
//! (owner, 2026-11-07), so a save drops no step it can carry:
//!
//! - A highlight the save wrote is a file's highlight now: it leaves the list, and a step that took it away (an undo of it) takes the file's
//!   annotation out instead, by the place the save says it went ([`super::FileMark`]).
//! - A highlight on a page the save left out (a page deleted) stays an edit, to come back with its page: onto the copy the save kept of the file's
//!   pages for that, or its added PDF, which is still its own file. One whose page can't come back goes, and is counted.
//! - A file's highlight the save took out is gone from the file: a step that would put it back puts it back as a new highlight where it was, and one
//!   that would take it out takes that highlight. One still in the file is named by its new place: its page's position, and its place in the list
//!   counted down by what the save took out before it. One whose page the save left out can't be named in the new file, so its step rolls off,
//!   counted (coordinator, 2026-11-14).
//! - A page an edit turned is written with that turn in its `/Rotate`, so its frame turns: the rectangles the history holds for it are turned with
//!   it, within the size the save says the page was shown at before (S3).
//!
//! A save whose report isn't here (one a crash interrupted, found by the record on the next launch) can say none of that: the highlights on its pages
//! are taken as written and their steps go, a turned page's go, and every step about a file's highlight goes, all counted.

use super::{FileKey, FileMark, Hide, Mark, MarkRect, MarkStep, Unhide};
use crate::edit::{Author, PageEdits, PageSlot, Step};
use std::collections::{HashMap, HashSet};

/// **What a save is asked to do with highlights,** taken with its snapshot: each highlight made as an edit on each position its page has, as the
/// save writes it, with its number; and each of the file's taken out, on each position its page has.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveMarks {
    /// `(position, lines)`, lines `[x, y, width, height]` in the page's frame, in the order the save is asked to write them.
    pub highlights: Vec<(u32, Box<[[f32; 4]]>)>,
    /// Each one's number, in the same order.
    pub ids: Vec<u32>,
    pub gone: Vec<recto_core::save::GoneHighlight>,
}

impl SaveMarks {
    /// What a list asks of a save now.
    pub fn of(edits: &PageEdits) -> Self {
        let mut out = Self::default();
        for (position, mark) in edits.marks.on_positions(&edits.slots) {
            out.highlights
                .push((position, mark.rects.iter().map(|rect| [rect.x, rect.y, rect.width, rect.height]).collect()));
            out.ids.push(mark.id);
        }
        if edits.marks.hidden.is_empty() {
            return out;
        }
        let pages: HashSet<u32> = edits.marks.hidden.iter().map(|key| key.page).collect();
        let mut positions: HashMap<u32, Vec<u32>> = HashMap::new();
        for (position, slot) in edits.slots.iter().enumerate() {
            if slot.source == 0 && pages.contains(&slot.page) {
                positions.entry(slot.page).or_default().push(u32::try_from(position).unwrap_or(u32::MAX));
            }
        }
        for key in &edits.marks.hidden {
            for position in positions.get(&key.page).into_iter().flatten() {
                out.gone.push(recto_core::save::GoneHighlight {
                    position: *position,
                    index: key.index,
                    rect: key.rect,
                });
            }
        }
        out
    }
}

/// **What a save that happened did with the highlights,** as the history needs it: from what it was asked ([`SaveMarks`]) and what it says it did
/// ([`recto_core::save::MarksWritten`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedMarks {
    /// Each highlight written, by number: the position of its page, its place in that page's `/Annots`, and its `/Rect`.
    placed: HashMap<u32, Vec<(u32, u32, [f32; 4])>>,
    /// The size each page an edit turned was shown at before the turn, by position.
    turned: HashMap<u32, [f32; 2]>,
    /// The places taken out of each page's list, by position, ascending.
    removed: HashMap<u32, Vec<u32>>,
}

impl SavedMarks {
    /// `None` if what the save says doesn't answer what it was asked, which is then no report of this save.
    pub fn of(asked: &SaveMarks, written: &recto_core::save::MarksWritten) -> Option<Self> {
        if written.placed.len() != asked.ids.len() || asked.highlights.len() != asked.ids.len() {
            return None;
        }
        let mut out = Self {
            turned: written.turned.iter().copied().collect(),
            ..Self::default()
        };
        for ((id, (position, _)), (index, rect)) in asked.ids.iter().zip(&asked.highlights).zip(&written.placed) {
            if *index == u32::MAX {
                return None;
            }
            out.placed.entry(*id).or_default().push((*position, *index, *rect));
        }
        for (position, index) in &written.removed {
            out.removed.entry(*position).or_default().push(*index);
        }
        out.removed.values_mut().for_each(|indexes| indexes.sort_unstable());
        Some(out)
    }
}

/// The copy a save kept of the file's pages the history can put back: its source number, and which of the file's pages it holds, in order (`None`
/// for the whole file). `None` when nothing was kept.
pub(in crate::edit) type Own<'a> = Option<(u16, Option<&'a [u32]>)>;

/// Where a page of the list before a save is after it.
enum Place {
    /// In the saved file, at this position.
    Saved(u32),
    /// Not in it, but in a file the history keeps: an added PDF, or the copy kept of the file's pages.
    Kept(u16, u32),
    Gone,
}

/// What [`saved`] works from: where each page went, and what the save said.
struct Rebase<'a> {
    first: HashMap<(u16, u32), u32>,
    slots: &'a [PageSlot],
    own: Own<'a>,
    written: Option<&'a SavedMarks>,
    /// The highlights the list had, by number, before the save.
    before: HashMap<u32, Mark>,
    /// The file's highlights the save took out, by their page in the file and place in its list.
    taken_out: HashSet<(u32, u32)>,
    /// A new number for each of those, as a highlight made as an edit, given as needed.
    fresh: HashMap<(u32, u32), u32>,
    next_id: u32,
}

impl Rebase<'_> {
    fn place(&self, source: u16, page: u32) -> Place {
        if let Some(position) = self.first.get(&(source, page)) {
            return Place::Saved(*position);
        }
        match (source, self.own) {
            (0, Some((kept, None))) => Place::Kept(kept, page),
            (0, Some((kept, Some(holds)))) => holds
                .binary_search(&page)
                .ok()
                .and_then(|at| u32::try_from(at).ok())
                .map_or(Place::Gone, |at| Place::Kept(kept, at)),
            (0, None) => Place::Gone,
            (source, _) => Place::Kept(source, page),
        }
    }

    /// Lines in a page's frame before the save, in its frame after: turned with the page by the quarter turns the save wrote into it, within the
    /// size the save says it was shown at before them. `None` for a turned page whose size the save didn't say.
    fn lines_at(&self, position: u32, rects: &[MarkRect]) -> Option<Box<[MarkRect]>> {
        let turns = self.slots.get(position as usize)?.quarter_turns % 4;
        if turns == 0 {
            return Some(rects.into());
        }
        Some(turned(rects, turns, *self.written?.turned.get(&position)?))
    }

    /// A highlight made as an edit, on its page after the save; `None` if it can't be.
    fn moved(&self, mark: &Mark) -> Option<Mark> {
        match self.place(mark.source, mark.page) {
            Place::Saved(position) => Some(Mark {
                source: 0,
                page: position,
                rects: self.lines_at(position, &mark.rects)?,
                ..mark.clone()
            }),
            Place::Kept(source, page) => Some(Mark { source, page, ..mark.clone() }),
            Place::Gone => None,
        }
    }

    /// Whether a highlight in the list was written by the save.
    fn written(&self, id: u32) -> bool {
        match self.written {
            Some(written) => written.placed.contains_key(&id),
            // No report: one on a page the save had is taken as written.
            None => self.before.get(&id).is_some_and(|mark| self.first.contains_key(&(mark.source, mark.page))),
        }
    }

    /// The file's highlights a save of these highlights made, named in the new file with their lines there.
    fn written_as_files(&self, id: u32) -> Option<Vec<FileMark>> {
        let mark = self.before.get(&id)?;
        let placed = self.written?.placed.get(&id)?;
        placed
            .iter()
            .map(|(position, index, rect)| {
                Some(FileMark {
                    key: FileKey {
                        source: 0,
                        page: *position,
                        index: *index,
                        rect: *rect,
                    },
                    rects: self.lines_at(*position, &mark.rects)?,
                })
            })
            .collect()
    }

    fn fresh_id(&mut self, key: FileKey) -> u32 {
        let next = &mut self.next_id;
        *self.fresh.entry((key.page, key.index)).or_insert_with(|| {
            let id = *next;
            *next = next.saturating_add(1);
            id
        })
    }

    /// A file's highlight as the new file has it: still there at its new place, or taken out by this save, when it's a highlight made as an edit
    /// from now on (`Err`, with its new number, page and lines; its author is the step's). `None` when its page isn't in the new file, or there's no report to say.
    fn file(&mut self, file: &FileMark) -> Option<Result<FileMark, Mark>> {
        let position = *self.first.get(&(0, file.key.page))?;
        let removed = self.written?.removed.get(&position).map_or(&[][..], Vec::as_slice);
        let rects = self.lines_at(position, &file.rects)?;
        if self.taken_out.contains(&(file.key.page, file.key.index)) {
            return Some(Err(Mark {
                id: self.fresh_id(file.key),
                source: 0,
                page: position,
                rects,
                by: Author::Owner,
            }));
        }
        let below = u32::try_from(removed.iter().filter(|index| **index < file.key.index).count()).ok()?;
        Some(Ok(FileMark {
            key: FileKey {
                page: position,
                index: file.key.index - below,
                ..file.key
            },
            rects,
        }))
    }

    /// Takes away: of these numbers, a highlight the save wrote becomes the file's, named where it went.
    fn takes(&self, ids: &[u32], files: &mut Vec<FileMark>) -> Option<Vec<u32>> {
        let mut kept = Vec::new();
        for id in ids {
            if self.written(*id) {
                files.extend(self.written_as_files(*id)?);
            } else {
                kept.push(*id);
            }
        }
        Some(kept)
    }

    /// The step after the save, or `None` if it can't be carried.
    fn step(&mut self, step: &MarkStep, by: Author) -> Option<MarkStep> {
        let (files, ids, marks): (&[FileMark], &[u32], &[Mark]) = match step {
            MarkStep::Add(marks) => (&[], &[], marks),
            MarkStep::Remove(ids) => (&[], ids, &[]),
            MarkStep::RemoveFile(hide) => (&hide.files, &hide.ids, &[]),
            MarkStep::RestoreFile(back) => (&back.files, &[], &back.marks),
        };
        let mut out_files = Vec::new();
        let mut out_ids = self.takes(ids, &mut out_files)?;
        let mut out_marks: Vec<Mark> = marks.iter().filter_map(|mark| self.moved(mark)).collect();
        if !marks.is_empty() && out_marks.is_empty() {
            return None;
        }
        let puts_back = matches!(step, MarkStep::Add(_) | MarkStep::RestoreFile(_));
        for file in files {
            match self.file(file)? {
                Ok(file) => out_files.push(file),
                // Taken out by this save: a highlight made as an edit from now on, which this step puts back or takes away.
                Err(mark) if puts_back => out_marks.push(Mark { by, ..mark }),
                Err(mark) => out_ids.push(mark.id),
            }
        }
        Some(match (puts_back, out_files.is_empty()) {
            (_, _) if out_files.is_empty() && out_ids.is_empty() && out_marks.is_empty() => return None,
            (true, true) => MarkStep::Add(out_marks.into_boxed_slice()),
            (true, false) => MarkStep::RestoreFile(Box::new(Unhide {
                files: out_files.into_boxed_slice(),
                marks: out_marks.into_boxed_slice(),
            })),
            (false, true) => MarkStep::Remove(out_ids.into_boxed_slice()),
            (false, false) => MarkStep::RemoveFile(Box::new(Hide {
                files: out_files.into_boxed_slice(),
                ids: out_ids.into_boxed_slice(),
            })),
        })
    }
}

/// One clockwise quarter turn, `turns` times: a rectangle `(x, y, w, h)` of a page `W` by `H` is `(H − y − h, x, h, w)` of the page `H` by `W`.
pub(in crate::edit) fn turned(rects: &[MarkRect], turns: u8, size: [f32; 2]) -> Box<[MarkRect]> {
    let mut out = rects.to_vec();
    let [mut width, mut height] = size;
    for _ in 0..turns % 4 {
        for rect in &mut out {
            *rect = MarkRect {
                x: height - rect.y - rect.height,
                y: rect.x,
                width: rect.height,
                height: rect.width,
            };
        }
        (width, height) = (height, width);
    }
    out.into_boxed_slice()
}

/// **What a confirmed save does to a list's highlights:** see the module's notes. Called before the list's pages start again, with the copy the save
/// kept of the file's pages (if one was) and what it said it did (if that's known). Says how many steps went.
pub(in crate::edit) fn saved(edits: &mut PageEdits, own: Own<'_>, written: Option<&SavedMarks>) -> usize {
    let mut first: HashMap<(u16, u32), u32> = HashMap::new();
    for (at, slot) in edits.slots.iter().enumerate() {
        first.entry((slot.source, slot.page)).or_insert(u32::try_from(at).unwrap_or(u32::MAX));
    }
    let taken_out: HashSet<(u32, u32)> = edits
        .marks
        .hidden
        .iter()
        .filter(|key| first.contains_key(&(0, key.page)))
        .map(|key| (key.page, key.index))
        .collect();
    let list = std::mem::take(&mut edits.marks.list);
    let mut rebase = Rebase {
        first,
        slots: &edits.slots,
        own,
        written,
        before: list.iter().map(|mark| (mark.id, mark.clone())).collect(),
        taken_out,
        fresh: HashMap::new(),
        next_id: edits.marks.next_id,
    };
    let mut gone = 0;
    let mut kept_list = Vec::new();
    for mark in &list {
        if rebase.written(mark.id) {
            continue;
        }
        match rebase.moved(mark) {
            Some(moved) => kept_list.push(moved),
            None => gone += 1,
        }
    }
    for history in [&mut edits.history.undo, &mut edits.history.redo] {
        history.retain_mut(|entry| match &mut entry.step {
            Step::Mark(step) => match rebase.step(step, entry.by) {
                Some(carried) => {
                    **step = carried;
                    true
                }
                None => {
                    gone += 1;
                    false
                }
            },
            _ => true,
        });
    }
    let next_id = rebase.next_id;
    let marks = &mut edits.marks;
    marks.lines = kept_list.iter().map(|mark| mark.rects.len()).sum();
    marks.list = kept_list;
    marks.next_id = next_id;
    marks.hidden.clear();
    // What's left is on pages the saved file doesn't have.
    marks.shown = false;
    marks.changes = super::tick();
    edits.history.bytes = edits.history.undo.iter().chain(&edits.history.redo).map(|entry| entry.step.held_bytes()).sum();
    gone
}

/// **Whether a save now would drop one of the owner's steps about the file's highlights:** one naming a highlight on a page the save leaves out, which
/// the new file can't name (coordinator, 2026-11-14: a Save by an agent below Full that would is refused, by the owner's-edits rule).
pub fn drops_owner_steps(edits: &PageEdits) -> bool {
    let named_pages = || {
        edits
            .history
            .undo
            .iter()
            .chain(&edits.history.redo)
            .filter(|entry| entry.by == Author::Owner)
            .filter_map(|entry| match &entry.step {
                Step::Mark(step) => Some(step.files().iter().map(|file| file.key.page)),
                _ => None,
            })
            .flatten()
    };
    let pages: HashSet<u32> = named_pages().collect();
    if pages.is_empty() {
        return false;
    }
    let there: HashSet<u32> = edits
        .slots
        .iter()
        .filter(|slot| slot.source == 0 && pages.contains(&slot.page))
        .map(|slot| slot.page)
        .collect();
    pages.len() != there.len()
}

/// Works out whether a highlight in the list is on a page the document has: one look along the pages, only when there are any.
pub(in crate::edit) fn settle(edits: &mut PageEdits) {
    let marks = &edits.marks;
    let shown = !marks.list.is_empty() && {
        let pages: HashSet<(u16, u32)> = marks.list.iter().map(|mark| (mark.source, mark.page)).collect();
        edits.slots.iter().any(|slot| pages.contains(&(slot.source, slot.page)))
    };
    edits.marks.shown = shown;
    if !edits.marks.list.is_empty() {
        // Their pages may be at other positions now.
        edits.marks.changes = super::tick();
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FileKey, FileMark, MarkChange, MarkPage, MarkRect};
    use super::*;
    use recto_core::save::MarksWritten;

    fn rect() -> MarkRect {
        MarkRect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 12.0,
        }
    }

    fn add(edits: &mut PageEdits, page: u32) {
        let pages = vec![MarkPage {
            source: 0,
            page,
            rects: vec![rect()],
        }];
        edits.mark(MarkChange::Add { pages }).unwrap();
    }

    /// What a save of `edits` would say: each highlight it writes placed after `existing` items of its page's list, `turned` as given, and the
    /// file's taken out removed, nothing else.
    fn save(edits: &mut PageEdits, existing: u32, turned: Vec<(u32, [f32; 2])>) -> usize {
        let asked = SaveMarks::of(edits);
        let mut count: HashMap<u32, u32> = HashMap::new();
        let placed = asked
            .highlights
            .iter()
            .map(|(position, _)| {
                let next = count.entry(*position).or_insert(existing);
                *next += 1;
                (*next - 1, [20.0, 750.0, 120.0, 762.0])
            })
            .collect();
        let removed = asked.gone.iter().map(|gone| (gone.position, gone.index)).collect();
        let written = MarksWritten { placed, turned, removed };
        let saved = SavedMarks::of(&asked, &written).unwrap();
        let pages = edits.len();
        edits.rebase_on_saved(pages, None, Some(&saved)).unwrap().rolled_off
    }

    /// **Failure (review of R4.3, S2): a save throws away the steps that take a saved highlight out**, so ⌘Z after Save skips the person's
    /// highlight, and an agent's `recto save` throws away the owner's steps. After a save, undo takes the highlight out of the file (hidden now, so
    /// something to save, by a rewrite), redo puts it back, and the history's bytes are what it holds.
    #[test]
    fn undo_after_a_save_takes_the_saved_highlight_out_and_redo_puts_it_back() {
        let mut edits = PageEdits::new(3);
        add(&mut edits, 1);
        assert_eq!(save(&mut edits, 2, Vec::new()), 0, "nothing goes");
        assert!(edits.marks().is_empty() && !edits.is_edited(), "it's in the file now");
        assert!(edits.undo(), "the highlight's step is still there");
        let hidden = edits.marks().hidden().to_vec();
        assert_eq!(
            hidden,
            vec![FileKey {
                source: 0,
                page: 1,
                index: 2,
                rect: [20.0, 750.0, 120.0, 762.0]
            }],
            "the file's highlight where the save put it"
        );
        assert!(edits.is_edited() && edits.rewrites(), "something to save, by a rewrite");
        let held: usize = edits.history.undo.iter().chain(&edits.history.redo).map(|entry| entry.step.held_bytes()).sum();
        assert_eq!(edits.history.bytes, held);
        let back = PageEdits::from_record(&edits.to_record()).unwrap();
        assert_eq!(back.to_record(), edits.to_record(), "and it reads back from a record");
        assert!(edits.redo());
        assert!(edits.marks().hidden().is_empty() && !edits.is_edited(), "back to the file as saved");
    }

    /// **Failure (review of R4.3, S3): a highlight in the history of a page an edit turned comes back in the old frame after a save**, which wrote
    /// the turn into the page, so it's drawn and saved a quarter turn off its text. Turned with the page within the size it was shown at, at each
    /// quarter turn.
    #[test]
    fn a_highlight_in_the_history_of_a_turned_page_turns_with_it() {
        for turns in 1..4u8 {
            let mut edits = PageEdits::new(1);
            edits.rotate(0, 1, i32::from(turns)).unwrap();
            add(&mut edits, 0);
            assert!(edits.undo());
            save(&mut edits, 0, vec![(0, [612.0, 792.0])]);
            assert!(edits.redo());
            let mark = edits.marks().iter().next().unwrap();
            let expected = turned(&[rect()], turns, [612.0, 792.0]);
            assert_eq!(mark.rects, expected, "{turns} quarter turns");
            assert_eq!(edits.slots()[0].quarter_turns, 0, "the turn is the file's now");
        }
        let quarter = turned(&[rect()], 1, [612.0, 792.0]);
        assert_eq!(
            quarter[0],
            MarkRect {
                x: 792.0 - 20.0 - 12.0,
                y: 10.0,
                width: 12.0,
                height: 100.0
            }
        );
        assert_eq!(turned(&quarter, 3, [792.0, 612.0])[0], rect(), "four quarters are none");
        // With no size from the save, the step can't be carried, and goes, counted.
        let mut edits = PageEdits::new(1);
        edits.rotate(0, 1, 1).unwrap();
        add(&mut edits, 0);
        assert!(edits.undo());
        assert_eq!(save(&mut edits, 0, Vec::new()), 1);
    }

    /// **Failure (review of R4.3, S2): the file's highlight a save took out can't be put back by undo past that save**, or one still in the file is
    /// named by its old place after a save took out one before it on the same page, so undo hides the wrong annotation.
    #[test]
    fn a_file_highlight_taken_out_by_a_save_comes_back_as_an_edit_and_the_rest_count_down() {
        let mut edits = PageEdits::new(2);
        let removal = |index: u32| MarkChange::RemoveFile {
            files: vec![FileMark {
                key: FileKey {
                    source: 0,
                    page: 1,
                    index,
                    rect: [1.0, 2.0, 3.0, 4.0],
                },
                rects: Box::new([rect()]),
            }],
            ids: Vec::new(),
        };
        edits.mark(removal(4)).unwrap();
        edits.mark(removal(1)).unwrap();
        assert!(edits.undo(), "4 is hidden, 1 is to redo");
        assert_eq!(save(&mut edits, 0, Vec::new()), 0);
        assert!(edits.marks().hidden().is_empty());
        assert!(edits.redo(), "1 is still in the file, at its place: nothing before it went");
        assert_eq!(edits.marks().hidden()[0].index, 1);
        assert!(edits.undo() && edits.undo(), "and the one the save took out comes back");
        let back = edits.marks().iter().next().expect("as a highlight made as an edit");
        assert_eq!((back.source, back.page, back.rects.to_vec()), (0, 1, vec![rect()]));
        assert!(edits.is_edited(), "something to save");
        assert!(edits.redo(), "and redo takes it away again");
        assert!(edits.marks().is_empty());

        let mut counts_down = PageEdits::new(2);
        counts_down.mark(removal(1)).unwrap();
        counts_down.mark(removal(4)).unwrap();
        assert!(counts_down.undo(), "1 is hidden, 4 is to redo");
        save(&mut counts_down, 0, Vec::new());
        assert!(counts_down.redo());
        assert_eq!(counts_down.marks().hidden()[0].index, 3, "one before it was taken out");
    }

    /// **Failure (review of R4.3, should-fix): a save drains a highlight on a page it left out** (a deleted page), so undoing the delete brings the
    /// page back without it; or such a highlight keeps the tab reading as unsaved. It stays, on the copy the save kept, and comes back with the page;
    /// and only a page the document has makes a highlight something to save.
    #[test]
    fn a_highlight_on_a_deleted_page_comes_back_with_it() {
        let mut edits = PageEdits::new(3);
        add(&mut edits, 1);
        edits.delete(1, 1).unwrap();
        let kept = crate::edit::AddedSource {
            path: std::path::Path::new("/kept/copy.pdf").into(),
            page_count: 1,
            stamp: None,
            kept: true,
            protected: false,
            holds: Some(Box::new([1])),
        };
        let asked = SaveMarks::of(&edits);
        assert!(asked.highlights.is_empty(), "nothing to write: its page isn't in the document");
        let saved = SavedMarks::of(&asked, &MarksWritten::default()).unwrap();
        assert_eq!(edits.rebase_on_saved(2, Some(kept), Some(&saved)).unwrap().rolled_off, 0);
        assert_eq!(edits.marks().len(), 1, "kept, to come back with its page");
        assert!(!edits.is_edited(), "and not something to save while its page isn't there");
        assert!(edits.undo());
        assert!(edits.is_edited(), "the page is back, with its highlight");
        let at = edits.slots()[1];
        let mark = edits.marks().iter().next().unwrap();
        assert_eq!((mark.source, mark.page), (at.source, at.page), "on the page that came back");
    }

    /// **Failure (coordinator, 2026-11-14): a step about a file's highlight on a page a save leaves out is carried with a place the new file doesn't
    /// have**, or goes uncounted, or an agent's save below Full can make the owner's go. It rolls off, counted, and the owner's makes
    /// [`drops_owner_steps`] say so beforehand.
    #[test]
    fn a_step_about_a_file_highlight_on_a_page_left_out_rolls_off_and_is_the_owners_to_lose() {
        let mut edits = PageEdits::new(3);
        edits
            .mark(MarkChange::RemoveFile {
                files: vec![FileMark {
                    key: FileKey {
                        source: 0,
                        page: 2,
                        index: 0,
                        rect: [1.0, 2.0, 3.0, 4.0],
                    },
                    rects: Box::new([rect()]),
                }],
                ids: Vec::new(),
            })
            .unwrap();
        assert!(!drops_owner_steps(&edits));
        edits.delete(2, 1).unwrap();
        assert!(drops_owner_steps(&edits), "the owner's step names a page the save leaves out");
        let mut agents = edits.clone();
        agents.history.undo.iter_mut().for_each(|entry| entry.by = Author::Agent);
        assert!(!drops_owner_steps(&agents), "an agent's is the agent's to lose");
        // Nothing was kept of the deleted page, so the delete goes, with the older removal; with a copy kept the delete stays, and the removal
        // still can't be carried.
        let mut with_copy = edits.clone();
        assert_eq!(save(&mut edits, 0, Vec::new()), 2, "counted");
        let kept = crate::edit::AddedSource {
            path: std::path::Path::new("/kept/copy.pdf").into(),
            page_count: 1,
            stamp: None,
            kept: true,
            protected: false,
            holds: Some(Box::new([2])),
        };
        let asked = SaveMarks::of(&with_copy);
        let saved = SavedMarks::of(&asked, &MarksWritten::default()).unwrap();
        assert_eq!(with_copy.rebase_on_saved(2, Some(kept), Some(&saved)).unwrap().rolled_off, 1);
        assert_eq!(with_copy.history.undo.len(), 1, "the delete, to undo");
        assert!(edits.history.undo.iter().all(|entry| !matches!(entry.step, Step::Mark(_))));
    }

    /// Failure: a save whose report isn't known (a crash between writing and confirming) carries a step about a file's highlight by a place that may
    /// be wrong, or takes out the highlights it wrote as edits again.
    #[test]
    fn without_the_saves_report_what_can_t_be_known_goes() {
        let mut edits = PageEdits::new(2);
        add(&mut edits, 0);
        edits
            .mark(MarkChange::RemoveFile {
                files: vec![FileMark {
                    key: FileKey {
                        source: 0,
                        page: 1,
                        index: 0,
                        rect: [1.0, 2.0, 3.0, 4.0],
                    },
                    rects: Box::new([rect()]),
                }],
                ids: Vec::new(),
            })
            .unwrap();
        assert_eq!(edits.rebase_on_saved(2, None, None).unwrap().rolled_off, 2, "both steps, counted");
        assert!(edits.marks().is_empty() && !edits.is_edited() && !edits.can_undo());
    }
}
