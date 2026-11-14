//! **Highlights made as edits** (SPEC section 6, Annotations; R4.3): what's held of each, the steps that add and take them away, and their records.
//!
//! A highlight belongs to a page by what the page is, `(source, page in that source)` as a [`super::PageSlot`] says, never by its position, so it follows
//! its page through moves and turns, and one on a deleted page is still there to come back with an undo. Its rectangles are in the page's frame as
//! `recto select` gives them: points from the page's top left, y down, past its crop and turned by the file's own `/Rotate`, before any turn an edit adds.
//! What a save writes is worked out from these by `recto_core::annotate`, which has the page in hand.
//!
//! **What it costs.** A tab with none has no list at all (no `PageEdits`), and a list with none holds one empty `Vec`. A highlight is a [`Mark`] of 32
//! bytes plus 16 a line rectangle, boxed in a step only while its step is in the history.
//!
//! **What may be in it,** for a list made from a hostile journal as much as from a person's selection: at most [`MAX_MARKS`] a document and
//! [`MAX_MARK_RECTS`] rectangles each, every edge finite and within [`MAX_EDGE`] points. A step names a highlight by its number, and one that names
//! nothing does nothing, so no record can make a step that panics.

use super::Author;
use crate::record::{AddedRecord, AuthorName};
use serde::{Deserialize, Serialize};

/// Most highlights one document may hold as unsaved edits. Far past a person's, and what keeps a hostile journal's list small.
pub const MAX_MARKS: usize = 10_000;
/// Most rectangles one highlight may have: a line each, and `recto select` gives at most 32 pages of them.
pub const MAX_MARK_RECTS: usize = 4096;
/// Largest edge, in points. PDF's own limit on a page is 14,400 points; past this a rectangle is nonsense.
pub const MAX_EDGE: f32 = 200_000.0;

/// One line of a highlight, in the page's frame (see the module's notes).
// size: 16 bytes, held per line of each highlight.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarkRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Every rectangle held is [`MarkRect::is_sound`], finite, and JSON can't say NaN, so equality is an equivalence for the records that hold them.
impl Eq for MarkRect {}

impl MarkRect {
    /// Finite, of some area, and inside the bounds any page could have.
    pub fn is_sound(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|edge| edge.is_finite() && edge.abs() <= MAX_EDGE)
            && self.width > 0.0
            && self.height > 0.0
    }
}

/// A highlight held as an edit.
// size: asserted <= 32 by `a_mark_stays_small`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Its number in this list, which the commands name it by. Never reused while the list lives.
    pub id: u32,
    /// 0 is the tab's own PDF; others are added PDFs, as a page slot says.
    pub source: u16,
    /// Zero-based, in that PDF.
    pub page: u32,
    pub rects: Box<[MarkRect]>,
    pub by: Author,
}

impl Mark {
    /// What it holds beyond its own size.
    fn held_bytes(&self) -> usize {
        size_of::<Self>() + size_of_val::<[MarkRect]>(&self.rects)
    }

    fn is_sound(&self) -> bool {
        !self.rects.is_empty() && self.rects.len() <= MAX_MARK_RECTS && self.rects.iter().all(|rect| rect.is_sound())
    }

    pub fn to_record(&self) -> MarkRecord {
        MarkRecord {
            id: self.id,
            source: self.source,
            page: self.page,
            rects: self.rects.to_vec(),
            by: self.by.into(),
        }
    }

    /// A highlight from a record, or nothing if what it says can't be one.
    pub fn from_record(record: &MarkRecord) -> Option<Self> {
        let mark = Self {
            id: record.id,
            source: record.source,
            page: record.page,
            rects: record.rects.clone().into_boxed_slice(),
            by: record.by.into(),
        };
        mark.is_sound().then_some(mark)
    }
}

/// What's asked of a document's highlights: one added on a page, or one taken away by its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkChange {
    Add { source: u16, page: u32, rects: Vec<MarkRect> },
    Remove { id: u32 },
}

/// A step of the history about highlights, and how to reverse it. Boxed, so a step of the history stays small.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MarkStep {
    Add(Box<Mark>),
    Remove(u32),
}

impl MarkStep {
    /// What it holds beyond its own size.
    pub(super) fn held_bytes(&self) -> usize {
        match self {
            Self::Add(mark) => mark.held_bytes(),
            Self::Remove(_) => 0,
        }
    }

    pub(super) fn to_record(&self) -> MarkStepRecord {
        match self {
            Self::Add(mark) => MarkStepRecord::Add(mark.to_record()),
            Self::Remove(id) => MarkStepRecord::Remove(*id),
        }
    }

    /// From a record already checked by [`MarkStepRecord::is_sound`].
    pub(super) fn from_record(record: &MarkStepRecord) -> Self {
        match record {
            MarkStepRecord::Add(mark) => Mark::from_record(mark).map_or(Self::Remove(mark.id), |mark| Self::Add(Box::new(mark))),
            MarkStepRecord::Remove(id) => Self::Remove(*id),
        }
    }
}

/// A document's highlights made as edits, in the order they were made.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marks {
    list: Vec<Mark>,
    /// The next number given.
    next_id: u32,
}

impl Marks {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Mark> {
        self.list.iter()
    }

    pub fn get(&self, id: u32) -> Option<&Mark> {
        self.list.iter().find(|mark| mark.id == id)
    }

    /// The step a change makes, checked, with the number a new one takes. Nothing changes until it's applied.
    pub(super) fn step_for(&mut self, change: MarkChange, by: Author) -> Result<MarkStep, MarkRefusal> {
        self.step_within(change, by, MAX_MARKS)
    }

    /// [`Self::step_for`] under a cap the tests set small.
    fn step_within(&mut self, change: MarkChange, by: Author, cap: usize) -> Result<MarkStep, MarkRefusal> {
        match change {
            MarkChange::Add { source, page, rects } => {
                if self.list.len() >= cap {
                    return Err(MarkRefusal::TooMany);
                }
                let mark = Mark {
                    id: self.next_id,
                    source,
                    page,
                    rects: rects.into_boxed_slice(),
                    by,
                };
                if !mark.is_sound() {
                    return Err(MarkRefusal::Unsound);
                }
                self.next_id = self.next_id.checked_add(1).ok_or(MarkRefusal::TooMany)?;
                Ok(MarkStep::Add(Box::new(mark)))
            }
            MarkChange::Remove { id } => self.get(id).map(|_| MarkStep::Remove(id)).ok_or(MarkRefusal::NoSuch),
        }
    }

    /// Applies a step and gives back its inverse. A step naming a highlight that isn't here, or one already here, does nothing either way, so a hostile
    /// record can't make it panic or two highlights share a number.
    pub(super) fn apply(&mut self, step: MarkStep) -> MarkStep {
        match step {
            MarkStep::Add(mark) => {
                if self.get(mark.id).is_some() || self.list.len() >= MAX_MARKS {
                    return MarkStep::Remove(u32::MAX);
                }
                self.next_id = self.next_id.max(mark.id.saturating_add(1));
                let id = mark.id;
                self.list.push(*mark);
                MarkStep::Remove(id)
            }
            MarkStep::Remove(id) => match self.list.iter().position(|mark| mark.id == id) {
                Some(at) => MarkStep::Add(Box::new(self.list.remove(at))),
                None => MarkStep::Remove(u32::MAX),
            },
        }
    }

    /// **What a confirmed save does to them:** every one is in the file now, so none is an edit any more; their numbers are given back, for the history
    /// to drop the steps that would take them out of the file, which can't be done yet.
    pub(super) fn saved(&mut self) -> Vec<u32> {
        self.list.drain(..).map(|mark| mark.id).collect()
    }

    pub(super) fn to_record(&self) -> Vec<MarkRecord> {
        self.list.iter().map(Mark::to_record).collect()
    }

    /// Each highlight on the positions its page has in `slots` (a page that's there twice has it twice, and one that isn't there has none), its
    /// rectangles as a save writes them. For a save's snapshot: what it holds is the highlights' own rectangles, and nothing more.
    pub fn on_positions(&self, slots: &[super::PageSlot]) -> Vec<(u32, Box<[[f32; 4]]>)> {
        if self.list.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (position, slot) in slots.iter().enumerate() {
            for mark in self.list.iter().filter(|mark| (mark.source, mark.page) == (slot.source, slot.page)) {
                let rects = mark.rects.iter().map(|rect| [rect.x, rect.y, rect.width, rect.height]).collect();
                out.push((u32::try_from(position).unwrap_or(u32::MAX), rects));
            }
        }
        out
    }

    /// Which PDF each is on, for a list that renumbers its added PDFs.
    pub(super) fn sources_mut(&mut self) -> impl Iterator<Item = &mut u16> {
        self.list.iter_mut().map(|mark| &mut mark.source)
    }

    /// From records: each sound, numbers not repeated, at most [`MAX_MARKS`]. `None` if any isn't.
    pub(super) fn from_record(records: &[MarkRecord]) -> Option<Self> {
        if records.len() > MAX_MARKS {
            return None;
        }
        let mut marks = Self::default();
        for record in records {
            let mark = Mark::from_record(record)?;
            if marks.get(mark.id).is_some() {
                return None;
            }
            marks.next_id = marks.next_id.max(mark.id.checked_add(1)?);
            marks.list.push(mark);
        }
        Some(marks)
    }

    /// Numbers past any a step names, so one brought back by an undo never meets a new one.
    pub(super) fn count_past(&mut self, id: u32) {
        self.next_id = self.next_id.max(id.saturating_add(1));
    }
}

/// **What a confirmed save does to a list's highlights** (coordinator, 2026-11-14, E1): every highlight is in the file now, so none is an edit, and the
/// steps that would take one out of the file leave undo and redo, as taking a file's highlight out isn't built yet. Every other step stays: an undone
/// highlight's way back moves onto the page's number in the new file, `slots` being the order that was saved, and goes only if its page isn't in it.
/// The bytes the history holds are counted again. Says how many steps had to go for want of their page. Called before the list's pages start again.
pub(super) fn saved(edits: &mut super::PageEdits) -> usize {
    let saved: std::collections::HashSet<u32> = edits.marks.saved().into_iter().collect();
    let mut position: std::collections::HashMap<(u16, u32), u32> = std::collections::HashMap::new();
    for (at, slot) in edits.slots.iter().enumerate() {
        position.entry((slot.source, slot.page)).or_insert(u32::try_from(at).unwrap_or(u32::MAX));
    }
    let mut gone = 0;
    for list in [&mut edits.history.undo, &mut edits.history.redo] {
        list.retain_mut(|entry| match &mut entry.step {
            super::Step::Mark(MarkStep::Remove(id)) => !saved.contains(id),
            super::Step::Mark(MarkStep::Add(mark)) => match position.get(&(mark.source, mark.page)) {
                Some(at) => {
                    (mark.source, mark.page) = (0, *at);
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
    edits.history.bytes = edits.history.undo.iter().chain(&edits.history.redo).map(|entry| entry.step.held_bytes()).sum();
    gone
}

/// Why a change to highlights wasn't made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkRefusal {
    NoSuch,
    TooMany,
    Unsound,
}

/// A highlight as a journal and a record hold it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkRecord {
    pub id: u32,
    pub source: u16,
    pub page: u32,
    #[serde(deserialize_with = "crate::record::capped::<_, _, MAX_MARK_RECTS>")]
    pub rects: Vec<MarkRect>,
    pub by: AuthorName,
}

impl MarkRecord {
    /// Sound, and on a page the document has: one of the file's own, or of a PDF added to it.
    pub fn fits(&self, file_pages: u32, added: &[AddedRecord]) -> bool {
        let pages = match self.source {
            0 => file_pages,
            source => added.get(usize::from(source) - 1).map_or(0, |added| added.page_count),
        };
        self.page < pages && self.id != u32::MAX && Mark::from_record(self).is_some()
    }

    /// A record's whole list: each fits, at most [`MAX_MARKS`], and no number twice.
    pub fn all_fit(records: &[Self], file_pages: u32, added: &[AddedRecord]) -> bool {
        let mut seen = std::collections::HashSet::new();
        records.len() <= MAX_MARKS && records.iter().all(|record| record.fits(file_pages, added) && seen.insert(record.id))
    }
}

/// A list's record as it always printed, with its highlights only when it has some, so what's logged and pinned of lists without any is unchanged.
impl std::fmt::Debug for crate::record::EditsRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = formatter.debug_struct("EditsRecord");
        out.field("file_pages", &self.file_pages)
            .field("pages", &self.pages)
            .field("added", &self.added)
            .field("undo", &self.undo)
            .field("redo", &self.redo)
            .field("history_dropped", &self.history_dropped)
            .field("owner_step_dropped", &self.owner_step_dropped);
        if !self.marks.is_empty() {
            out.field("marks", &self.marks);
        }
        out.finish()
    }
}

/// A step about highlights, as a record holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkStepRecord {
    #[serde(rename = "a")]
    Add(MarkRecord),
    #[serde(rename = "x")]
    Remove(u32),
}

impl MarkStepRecord {
    pub fn fits(&self, file_pages: u32, added: &[AddedRecord]) -> bool {
        match self {
            Self::Add(mark) => mark.fits(file_pages, added),
            Self::Remove(_) => true,
        }
    }
}

/// What a journal notes of a change to highlights: what was asked, forwards, and by whom, to be asked again through the same checked call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkOp {
    #[serde(rename = "a")]
    Add {
        source: u16,
        page: u32,
        #[serde(deserialize_with = "crate::record::capped::<_, _, MAX_MARK_RECTS>")]
        rects: Vec<MarkRect>,
        by: AuthorName,
    },
    #[serde(rename = "x")]
    Remove { id: u32, by: AuthorName },
}

impl MarkOp {
    pub(super) fn change(&self) -> (MarkChange, AuthorName) {
        match self {
            Self::Add { source, page, rects, by } => (
                MarkChange::Add {
                    source: *source,
                    page: *page,
                    rects: rects.clone(),
                },
                *by,
            ),
            Self::Remove { id, by } => (MarkChange::Remove { id: *id }, *by),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> MarkRect {
        MarkRect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 12.0,
        }
    }

    /// Failure: a highlight grows past what the module's notes say it costs, and with it every unsaved highlight's memory.
    #[test]
    fn a_mark_stays_small() {
        assert!(size_of::<Mark>() <= 32, "{}", size_of::<Mark>());
        assert_eq!(size_of::<MarkRect>(), 16);
        assert!(size_of::<MarkStep>() <= 16, "{}", size_of::<MarkStep>());
    }

    /// Failure: a rectangle with a NaN, an infinite or enormous edge, or no area, from a hostile journal or a bad pick, reaches the drawing or the save.
    #[test]
    fn only_sound_rectangles_make_a_highlight() {
        let mut marks = Marks::default();
        for bad in [
            MarkRect { x: f32::NAN, ..rect() },
            MarkRect {
                width: f32::INFINITY,
                ..rect()
            },
            MarkRect { y: MAX_EDGE * 2.0, ..rect() },
            MarkRect { width: 0.0, ..rect() },
            MarkRect { height: -1.0, ..rect() },
        ] {
            let change = MarkChange::Add {
                source: 0,
                page: 0,
                rects: vec![bad],
            };
            assert_eq!(marks.step_for(change, Author::Owner), Err(MarkRefusal::Unsound), "{bad:?}");
        }
        let none = MarkChange::Add {
            source: 0,
            page: 0,
            rects: vec![],
        };
        assert_eq!(marks.step_for(none, Author::Owner), Err(MarkRefusal::Unsound));
        let many = MarkChange::Add {
            source: 0,
            page: 0,
            rects: vec![rect(); MAX_MARK_RECTS + 1],
        };
        assert_eq!(marks.step_for(many, Author::Owner), Err(MarkRefusal::Unsound));
    }

    /// Failure: a record naming a highlight twice, or with a broken rectangle, is taken, and two highlights share a number or a NaN reaches the host.
    #[test]
    fn a_record_with_a_repeated_number_or_a_bad_rectangle_is_refused() {
        let good = MarkRecord {
            id: 3,
            source: 0,
            page: 1,
            rects: vec![rect()],
            by: AuthorName::Owner,
        };
        assert_eq!(Marks::from_record(std::slice::from_ref(&good)).map(|marks| marks.next_id), Some(4));
        assert!(Marks::from_record(&[good.clone(), good.clone()]).is_none());
        let bad = MarkRecord {
            rects: vec![MarkRect { x: f32::NAN, ..rect() }],
            ..good.clone()
        };
        assert!(Marks::from_record(&[bad]).is_none());
        let last = MarkRecord { id: u32::MAX, ..good };
        assert!(Marks::from_record(&[last]).is_none(), "a number with none after it can't be the list's");
    }

    /// Failure: a step from a hostile record naming a highlight that isn't there, or one that already is, panics or makes two of one number.
    #[test]
    fn a_step_naming_nothing_does_nothing() {
        let mut marks = Marks::default();
        let step = marks
            .step_for(
                MarkChange::Add {
                    source: 0,
                    page: 0,
                    rects: vec![rect()],
                },
                Author::Agent,
            )
            .unwrap();
        let undo = marks.apply(step.clone());
        assert_eq!(marks.len(), 1);
        assert_eq!(marks.apply(step), MarkStep::Remove(u32::MAX), "the same number twice is nothing");
        assert_eq!(marks.len(), 1);
        assert_eq!(marks.apply(MarkStep::Remove(77)), MarkStep::Remove(u32::MAX));
        let redo = marks.apply(undo);
        assert!(marks.is_empty());
        assert_eq!(marks.apply(redo), MarkStep::Remove(0));
        assert_eq!(
            marks.iter().next().map(|mark| mark.by),
            Some(Author::Agent),
            "a highlight stays its author's through undo and redo"
        );
    }

    /// Failure: past the cap a list goes on growing from a journal or an agent's loop. Held at the cap plus one, with the cap set small.
    #[test]
    fn the_count_is_capped() {
        const CAP: usize = 3;
        let mut marks = Marks::default();
        for _ in 0..CAP {
            let step = marks
                .step_within(
                    MarkChange::Add {
                        source: 0,
                        page: 0,
                        rects: vec![rect()],
                    },
                    Author::Owner,
                    CAP,
                )
                .unwrap();
            marks.apply(step);
        }
        let one_more = MarkChange::Add {
            source: 0,
            page: 0,
            rects: vec![rect()],
        };
        assert_eq!(marks.step_within(one_more, Author::Owner, CAP), Err(MarkRefusal::TooMany));
    }

    fn add(edits: &mut super::super::PageEdits, page: u32) {
        let change = MarkChange::Add {
            source: 0,
            page,
            rects: vec![rect()],
        };
        edits.mark(change).unwrap();
    }

    /// Failure: a highlight isn't a step of the one history, so ⌘Z undoes a turn made before it, or a highlight survives the undo of everything, or
    /// the tab reads as saved with one on it.
    #[test]
    fn a_highlight_is_a_step_like_a_turn() {
        let mut edits = super::super::PageEdits::new(3);
        edits.rotate(0, 1, 1).unwrap();
        add(&mut edits, 2);
        assert!(edits.is_edited());
        assert_eq!(edits.marks().len(), 1);
        assert!(edits.undo());
        assert!(edits.marks().is_empty(), "the highlight was the last step, so it's the first undone");
        assert_eq!(edits.slots()[0].quarter_turns, 1, "and the turn before it stays");
        assert!(edits.undo());
        assert!(!edits.is_edited(), "back at the file");
        assert!(edits.redo() && edits.redo());
        assert_eq!(
            edits.marks().iter().map(|mark| (mark.page, mark.id)).collect::<Vec<_>>(),
            vec![(2, 0)],
            "it comes back with its number"
        );
        assert_eq!(
            edits.mark(MarkChange::Add {
                source: 0,
                page: 3,
                rects: vec![rect()]
            }),
            Err(super::super::EditError::OutOfRange)
        );
        assert_eq!(edits.mark(MarkChange::Remove { id: 9 }), Err(super::super::EditError::NoSuchHighlight));
        edits.mark(MarkChange::Remove { id: 0 }).unwrap();
        assert!(edits.marks().is_empty());
    }

    /// Failure: what a journal or a record says of highlights doesn't bring the list back as it was, so a crash, a sleep or a restart loses a highlight,
    /// changes its author, or gives a new one the number of one the history can bring back.
    #[test]
    fn highlights_come_back_from_the_journal_and_the_record() {
        let mut edits = super::super::PageEdits::new(4);
        edits.journal_from_now();
        add(&mut edits, 1);
        edits.acting = Author::Agent;
        add(&mut edits, 3);
        edits.acting = Author::Owner;
        edits.mark(MarkChange::Remove { id: 0 }).unwrap();
        add(&mut edits, 0);
        assert!(edits.undo() && edits.undo());
        let ops = edits.journal_waiting().map(|(_, ops)| ops.to_vec()).unwrap();
        let (replayed, applied) = super::super::PageEdits::replay(4, None, &ops).unwrap();
        assert_eq!(applied, ops.len());
        assert_eq!(replayed.to_record(), edits.to_record());
        let mut back = super::super::PageEdits::from_record(&edits.to_record()).unwrap();
        assert_eq!(back.to_record(), edits.to_record());
        assert_eq!(back.marks().get(1).map(|mark| mark.by), Some(Author::Agent));
        assert!(back.redo() && back.redo());
        assert_eq!(back.marks().iter().map(|mark| mark.id).collect::<Vec<_>>(), vec![1, 2]);
        add(&mut back, 2);
        assert_eq!(
            back.marks().iter().last().map(|mark| mark.id),
            Some(3),
            "a new one goes past every number the history named"
        );
    }

    /// Failure (E1, coordinator 2026-11-14): after a save, an undo takes a highlight that's in the file now as if it were an edit, a redo of an undone
    /// highlight lands on the page that had its old number, or the history's bytes and dropped flags are wrong for what's left.
    #[test]
    fn a_save_drops_only_the_steps_that_would_take_a_saved_highlight_out() {
        let mut edits = super::super::PageEdits::new(3);
        add(&mut edits, 0);
        edits.move_pages(2, 1, 0).unwrap();
        add(&mut edits, 2);
        assert!(edits.undo(), "the highlight on what was page 3 is undone, and waits to be redone");
        let rebased = edits.rebase_on_saved(3, None).unwrap();
        assert_eq!(rebased.rolled_off, 0);
        assert!(edits.marks().is_empty(), "every highlight that was on is in the file now");
        assert!(!edits.is_edited());
        let held: usize = edits.history.undo.iter().chain(&edits.history.redo).map(|entry| entry.step.held_bytes()).sum();
        assert_eq!(edits.history.bytes, held);
        assert!(!edits.history.dropped && !edits.history.owner_dropped);
        assert_eq!(
            edits.history.undo.len(),
            1,
            "the move is left to undo, and nothing takes the saved highlight out"
        );
        assert!(edits.redo());
        let mark = edits.marks().iter().next().unwrap();
        assert_eq!(
            (mark.source, mark.page),
            (0, 0),
            "the file's page 3 was saved first, so its highlight comes back on page 1"
        );
        assert!(edits.undo() && edits.undo(), "the redone highlight, then the move");
        assert!(!edits.can_undo());

        let mut only = super::super::PageEdits::new(2);
        add(&mut only, 1);
        only.rebase_on_saved(2, None).unwrap();
        assert!(
            !only.can_undo() && !only.can_redo(),
            "a saved highlight's step was all there was, so there's nothing to undo"
        );
    }

    /// Failure (E2, coordinator 2026-11-14): an app from before highlights reads a journal that has them as if it had none, and its next compaction
    /// writes them away; or a journal goes on saying the newer version once none are left, which locks that app out of the person's other edits.
    #[test]
    fn a_journal_says_the_newer_version_only_while_it_holds_highlights() {
        use crate::record::{CURRENT_VERSION, JournalHeader, MARKED_JOURNAL_VERSION, RecordError, journal_start, parse_journal};
        let header = JournalHeader::new(1, std::path::Path::new("/d/a.pdf"), None, 3, 0);
        let mut edits = super::super::PageEdits::new(3);
        edits.rotate(0, 1, 1).unwrap();
        let plain = journal_start(&header, Some(&edits.to_record()));
        assert_eq!(
            parse_journal(&plain, CURRENT_VERSION).unwrap().header.version,
            CURRENT_VERSION,
            "an app from before reads it"
        );
        add(&mut edits, 2);
        // The highlight's own step rolls off the history: its record holds it only in the list, which an older app would read past.
        for _ in 0..super::super::MAX_UNDO {
            edits.rotate(1, 1, 2).unwrap();
        }
        assert!(edits.history.dropped);
        let marked = journal_start(&header, Some(&edits.to_record()));
        assert_eq!(
            parse_journal(&marked, CURRENT_VERSION),
            Err(RecordError::Newer(MARKED_JOURNAL_VERSION)),
            "an app from before leaves it alone"
        );
        let read = parse_journal(&marked, MARKED_JOURNAL_VERSION).unwrap();
        let (back, _) = super::super::PageEdits::replay(3, read.base.as_ref(), &read.ops).unwrap();
        assert_eq!(back.marks().len(), 1, "and this one reads its highlight back");
        edits.mark(MarkChange::Remove { id: 0 }).unwrap();
        let again = journal_start(&header, Some(&edits.to_record()));
        assert_eq!(
            parse_journal(&again, CURRENT_VERSION).unwrap().header.version,
            CURRENT_VERSION,
            "none left, so the older app reads it again"
        );
    }
}
