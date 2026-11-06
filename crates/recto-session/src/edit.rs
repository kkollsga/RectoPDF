//! Page edits as a small list, applied only when the user saves.
//!
//! Rearranging, rotating, deleting and adding pages touch no PDF bytes and read no page content, so they're instant and
//! cost about 8 bytes a page, whatever the size of the PDFs. A tab with no edits has no list at all. Undo stores the
//! inverse of each step rather than copies of the list. The original files stay as they are until Save.

use std::path::{Path, PathBuf};

/// Most pages one edited document may have.
pub const MAX_PAGES: usize = 200_000;
/// Most PDFs that may be added to one document.
pub const MAX_SOURCES: usize = 1024;
/// Undo steps kept. The oldest go first.
pub const MAX_UNDO: usize = 256;

/// One page of the edited document: which PDF it comes from, which page there, and its added rotation.
// size: asserted <= 8 by `page_slot_stays_small`. Held per page, edited tabs only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSlot {
    /// 0 is the tab's own PDF; others index [`PageEdits::added_sources`] from 1.
    pub source: u16,
    /// Zero-based page in that PDF.
    pub page: u32,
    /// Quarter turns clockwise added to the page, 0 to 3.
    pub quarter_turns: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    OutOfRange,
    LastPage,
    TooManyPages,
    TooManySources,
}

impl EditError {
    pub fn code(self) -> &'static str {
        match self {
            Self::OutOfRange => "page_out_of_range",
            Self::LastPage => "last_page",
            Self::TooManyPages => "too_many_pages",
            Self::TooManySources => "too_many_sources",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::OutOfRange => "That page isn't in this document.",
            Self::LastPage => "A PDF has to keep at least one page.",
            Self::TooManyPages => "That would make this document too long.",
            Self::TooManySources => "Too many PDFs have been added to this document.",
        }
    }
}

/// A step and how to reverse it. Each direction of undo and redo produces the other.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Rotate { start: u32, len: u32, quarter_turns: u8 },
    Move { start: u32, len: u32, to: u32 },
    Remove { start: u32, len: u32 },
    Restore { start: u32, slots: Box<[PageSlot]> },
}

/// The edited page order of one document.
#[derive(Clone, Debug)]
pub struct PageEdits {
    slots: Vec<PageSlot>,
    added: Vec<(Box<Path>, u32)>,
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl PageEdits {
    pub fn new(page_count: u32) -> Self {
        let slots = (0..page_count).map(|page| PageSlot { source: 0, page, quarter_turns: 0 }).collect();
        Self { slots, added: Vec::new(), undo: Vec::new(), redo: Vec::new() }
    }

    pub fn slots(&self) -> &[PageSlot] {
        &self.slots
    }

    pub fn len(&self) -> u32 {
        // The list is capped at MAX_PAGES, far below u32::MAX.
        u32::try_from(self.slots.len()).unwrap_or(u32::MAX)
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The PDFs dropped into this document, with their page counts. Slot source `n` is entry `n - 1`.
    pub fn added_sources(&self) -> &[(Box<Path>, u32)] {
        &self.added
    }

    /// Whether anything is waiting to be saved. Undoing everything brings this back to false.
    pub fn is_edited(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Where a page of the tab's own PDF now sits, if it's still there.
    pub fn position_of(&self, original_page: u32) -> Option<u32> {
        let found = self.slots.iter().position(|s| s.source == 0 && s.page == original_page)?;
        u32::try_from(found).ok()
    }

    /// Turns pages clockwise by quarter turns. Negative turns go the other way.
    pub fn rotate(&mut self, start: u32, len: u32, quarter_turns: i32) -> Result<(), EditError> {
        self.check_range(start, len)?;
        let turns = u8::try_from(quarter_turns.rem_euclid(4)).unwrap_or(0);
        self.commit(Step::Rotate { start, len, quarter_turns: turns })
    }

    /// Moves a run of pages so its first page ends up at position `to` in the resulting order.
    pub fn move_pages(&mut self, start: u32, len: u32, to: u32) -> Result<(), EditError> {
        self.check_range(start, len)?;
        if u64::from(to) + u64::from(len) > u64::from(self.len()) {
            return Err(EditError::OutOfRange);
        }
        self.commit(Step::Move { start, len, to })
    }

    /// Deletes a run of pages. A document can't lose its last page.
    pub fn delete(&mut self, start: u32, len: u32) -> Result<(), EditError> {
        self.check_range(start, len)?;
        if len >= self.len() {
            return Err(EditError::LastPage);
        }
        self.commit(Step::Remove { start, len })
    }

    /// Adds every page of another PDF at a position, as dropping it into the page view does.
    pub fn insert_pdf(&mut self, at: u32, path: &Path, page_count: u32) -> Result<(), EditError> {
        if at > self.len() {
            return Err(EditError::OutOfRange);
        }
        if self.slots.len() + page_count as usize > MAX_PAGES {
            return Err(EditError::TooManyPages);
        }
        let source = self.source_for(path, page_count)?;
        let slots: Box<[PageSlot]> = (0..page_count).map(|page| PageSlot { source, page, quarter_turns: 0 }).collect();
        self.commit(Step::Restore { start: at, slots })
    }

    /// Reverses the last step. Returns false with nothing left to undo.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        let inverse = self.apply(step);
        self.redo.push(inverse);
        true
    }

    /// Repeats the last undone step. Returns false with nothing left to redo.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        let inverse = self.apply(step);
        self.undo.push(inverse);
        true
    }

    fn commit(&mut self, step: Step) -> Result<(), EditError> {
        let inverse = self.apply(step);
        self.undo.push(inverse);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
        Ok(())
    }

    /// Applies a step that has already been checked, and returns its inverse.
    fn apply(&mut self, step: Step) -> Step {
        match step {
            Step::Rotate { start, len, quarter_turns } => {
                for slot in &mut self.slots[start as usize..(start + len) as usize] {
                    slot.quarter_turns = (slot.quarter_turns + quarter_turns) % 4;
                }
                Step::Rotate { start, len, quarter_turns: (4 - quarter_turns) % 4 }
            }
            Step::Move { start, len, to } => {
                let (s, n, t) = (start as usize, len as usize, to as usize);
                if t > s {
                    self.slots[s..t + n].rotate_left(n);
                } else {
                    self.slots[t..s + n].rotate_right(n);
                }
                Step::Move { start: to, len, to: start }
            }
            Step::Remove { start, len } => {
                let (s, n) = (start as usize, len as usize);
                let slots: Box<[PageSlot]> = self.slots.drain(s..s + n).collect();
                Step::Restore { start, slots }
            }
            Step::Restore { start, slots } => {
                let len = u32::try_from(slots.len()).unwrap_or(0);
                let s = start as usize;
                self.slots.splice(s..s, slots.iter().copied());
                Step::Remove { start, len }
            }
        }
    }

    fn check_range(&self, start: u32, len: u32) -> Result<(), EditError> {
        if len == 0 || u64::from(start) + u64::from(len) > u64::from(self.len()) {
            return Err(EditError::OutOfRange);
        }
        Ok(())
    }

    fn source_for(&mut self, path: &Path, page_count: u32) -> Result<u16, EditError> {
        if let Some(found) = self.added.iter().position(|(p, _)| &**p == path) {
            return u16::try_from(found + 1).map_err(|_| EditError::TooManySources);
        }
        if self.added.len() >= MAX_SOURCES {
            return Err(EditError::TooManySources);
        }
        self.added.push((PathBuf::from(path).into_boxed_path(), page_count));
        u16::try_from(self.added.len()).map_err(|_| EditError::TooManySources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(edits: &PageEdits) -> Vec<u32> {
        edits.slots().iter().map(|s| s.page).collect()
    }

    /// Failure: a field added carelessly raises the cost of every page in an edited document.
    #[test]
    fn page_slot_stays_small() {
        assert!(std::mem::size_of::<PageSlot>() <= 8);
    }

    /// Failure: dragging a page forwards or backwards lands it one place off.
    #[test]
    fn moving_pages_lands_them_where_asked() {
        let mut edits = PageEdits::new(5);
        edits.move_pages(0, 1, 3).unwrap();
        assert_eq!(order(&edits), [1, 2, 3, 0, 4]);
        edits.move_pages(3, 1, 0).unwrap();
        assert_eq!(order(&edits), [0, 1, 2, 3, 4]);
        edits.move_pages(1, 2, 3).unwrap();
        assert_eq!(order(&edits), [0, 3, 4, 1, 2]);
    }

    /// Failure: rotating affects the wrong pages, or four quarter turns don't come back round.
    #[test]
    fn rotating_turns_only_the_chosen_pages_and_wraps() {
        let mut edits = PageEdits::new(3);
        edits.rotate(1, 1, 1).unwrap();
        assert_eq!(edits.slots().iter().map(|s| s.quarter_turns).collect::<Vec<_>>(), [0, 1, 0]);
        edits.rotate(1, 1, -1).unwrap();
        edits.rotate(1, 1, -1).unwrap();
        assert_eq!(edits.slots()[1].quarter_turns, 3);
        edits.rotate(1, 1, 5).unwrap();
        assert_eq!(edits.slots()[1].quarter_turns, 0);
    }

    /// Failure: a dropped PDF lands in the wrong place, or its pages get mixed up with the tab's own.
    #[test]
    fn dropping_a_pdf_inserts_its_pages_at_the_position() {
        let mut edits = PageEdits::new(3);
        edits.insert_pdf(1, Path::new("/d/tide.pdf"), 2).unwrap();
        let rows: Vec<_> = edits.slots().iter().map(|s| (s.source, s.page)).collect();
        assert_eq!(rows, [(0, 0), (1, 0), (1, 1), (0, 1), (0, 2)]);
        assert_eq!(edits.added_sources().len(), 1);
        assert_eq!(edits.position_of(1), Some(3));
    }

    /// Failure: dropping the same PDF twice stores it twice, or rotation of one copy changes the other.
    #[test]
    fn the_same_pdf_dropped_twice_shares_a_source() {
        let mut edits = PageEdits::new(1);
        edits.insert_pdf(1, Path::new("/d/a.pdf"), 1).unwrap();
        edits.insert_pdf(2, Path::new("/d/a.pdf"), 1).unwrap();
        assert_eq!(edits.added_sources().len(), 1);
        edits.rotate(1, 1, 1).unwrap();
        assert_eq!((edits.slots()[1].quarter_turns, edits.slots()[2].quarter_turns), (1, 0));
    }

    /// Failure: every step can't be reversed exactly, so undo leaves a document different from what the user had.
    #[test]
    fn undo_and_redo_reverse_every_kind_of_step() {
        let mut edits = PageEdits::new(6);
        let start = edits.slots().to_vec();
        edits.move_pages(0, 2, 3).unwrap();
        edits.rotate(2, 3, 1).unwrap();
        edits.delete(1, 2).unwrap();
        edits.insert_pdf(2, Path::new("/d/x.pdf"), 4).unwrap();
        let end = edits.slots().to_vec();
        assert!(edits.is_edited());
        while edits.undo() {}
        assert_eq!(edits.slots(), start);
        assert!(!edits.is_edited());
        while edits.redo() {}
        assert_eq!(edits.slots(), end);
    }

    /// Failure: a new step keeps the old redo list, so redo jumps to a state the user never had.
    #[test]
    fn a_new_step_clears_redo() {
        let mut edits = PageEdits::new(3);
        edits.rotate(0, 1, 1).unwrap();
        edits.undo();
        edits.delete(0, 1).unwrap();
        assert!(!edits.redo());
    }

    /// Failure: a document loses its last page, or a bad range panics rather than being refused.
    #[test]
    fn bad_requests_are_refused_and_change_nothing() {
        let mut edits = PageEdits::new(2);
        assert_eq!(edits.delete(0, 2), Err(EditError::LastPage));
        assert_eq!(edits.delete(5, 1), Err(EditError::OutOfRange));
        assert_eq!(edits.rotate(0, u32::MAX, 1), Err(EditError::OutOfRange));
        assert_eq!(edits.move_pages(0, 1, u32::MAX), Err(EditError::OutOfRange));
        assert_eq!(edits.insert_pdf(9, Path::new("/d/a.pdf"), 1), Err(EditError::OutOfRange));
        assert_eq!(edits.insert_pdf(0, Path::new("/d/a.pdf"), MAX_PAGES as u32), Err(EditError::TooManyPages));
        assert_eq!(order(&edits), [0, 1]);
        assert!(!edits.is_edited());
    }

    /// Failure: undo history grows without limit on a long editing session.
    #[test]
    fn undo_history_is_capped() {
        let mut edits = PageEdits::new(2);
        for _ in 0..MAX_UNDO * 2 {
            edits.rotate(0, 1, 1).unwrap();
        }
        assert_eq!(edits.undo.len(), MAX_UNDO);
    }
}
