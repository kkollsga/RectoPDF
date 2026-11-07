//! Page edits as a small list, applied only when the user saves.
//!
//! Rearranging, rotating, deleting and adding pages touch no PDF bytes and read no page content, so they're instant and
//! cost about 8 bytes a page, whatever the size of the PDFs. A tab with no edits has no list at all. Undo stores the
//! inverse of each step rather than copies of the list. The original files stay as they are until Save.

use recto_platform::FileStamp;
use std::path::{Path, PathBuf};

/// Most pages one edited document may have.
pub const MAX_PAGES: usize = 200_000;
/// Most PDFs that may be added to one document.
pub const MAX_SOURCES: usize = 1024;
/// Undo steps kept. The oldest go first.
pub const MAX_UNDO: usize = 256;
/// Bytes of pages held for undo and redo, per tab. A step is usually a few bytes, but one that removes 200,000 pages holds 1.6 MB of them, and 256 of those would be 400 MiB. The oldest go first, and the last step always stays.
pub const MAX_HISTORY_BYTES: usize = 16 * 1024 * 1024;

/// Who made an edit step. Agents may throw away agents' edits, and only someone at Full may throw away the owner's. SPEC decision log, 2026-11-07.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Author {
    /// Anything that came through the agent interface.
    Agent,
    /// The person using the app, through its own edits.
    Owner,
}

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
    /// A PDF already in this document has changed since it was added, so its pages can't be mixed with what's there.
    SourceChanged,
}

impl EditError {
    pub fn code(self) -> &'static str {
        match self {
            Self::OutOfRange => "page_out_of_range",
            Self::LastPage => "last_page",
            Self::TooManyPages => "too_many_pages",
            Self::TooManySources => "too_many_sources",
            Self::SourceChanged => "source_changed",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::OutOfRange => "That page isn't in this document.",
            Self::LastPage => "A PDF has to keep at least one page.",
            Self::TooManyPages => "That would make this document too long.",
            Self::TooManySources => "Too many PDFs have been added to this document.",
            Self::SourceChanged => "That PDF has changed since it was added, so it can't be added again.",
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

impl Step {
    /// What it holds beyond its own size.
    fn held_bytes(&self) -> usize {
        match self {
            Self::Restore { slots, .. } => size_of_val::<[PageSlot]>(slots),
            _ => 0,
        }
    }
}

/// A step and who made it. The inverse of a step keeps its author, so an undone step is still whoever's it was.
// size: asserted <= 32 by `an_edit_step_stays_small`. Held per step, at most MAX_UNDO of them and their undone ones, edited tabs only, and nothing per page.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    step: Step,
    by: Author,
}

/// A PDF dropped into a document: where it is, how long it was, and which version of it its page numbers are about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddedSource {
    pub path: Box<Path>,
    pub page_count: u32,
    /// `None` if the file couldn't be looked at then.
    pub stamp: Option<FileStamp>,
}

/// The edited page order of one document.
#[derive(Clone, Debug)]
pub struct PageEdits {
    /// How long the file was when this list was made.
    file_pages: usize,
    slots: Vec<PageSlot>,
    added: Vec<AddedSource>,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// A step fell off the capped undo list, so this document can't get back to the original by undoing.
    history_dropped: bool,
    /// One of those was the owner's. Its effect is still in the page order: nothing has lost it, but throwing the list away would, so it counts for as long as the list does.
    owner_step_dropped: bool,
    /// Whether the document differs from the file, whatever steps took it there. Worked out after each step, so asking is free. See [`Self::is_edited`].
    differs: bool,
    /// Whose steps are being made now. Only the session sets this, and the owner is the default: a path that forgets to say gets the more protected answer.
    acting: Author,
    /// What `undo` and `redo` hold, against [`MAX_HISTORY_BYTES`].
    history_bytes: usize,
}

impl PageEdits {
    pub fn new(page_count: u32) -> Self {
        let slots = (0..page_count).map(|page| PageSlot { source: 0, page, quarter_turns: 0 }).collect();
        Self { file_pages: page_count as usize, slots, added: Vec::new(), undo: Vec::new(), redo: Vec::new(), history_dropped: false, owner_step_dropped: false, differs: false, acting: Author::Owner, history_bytes: 0 }
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

    /// The PDFs dropped into this document. Slot source `n` is entry `n - 1`.
    pub fn added_sources(&self) -> &[AddedSource] {
        &self.added
    }

    /// Whether the document differs from the file, and so whether anything is waiting to be saved. Undoing everything brings this back to false, and so does getting back to the file by other steps: four quarter turns, or a page moved and moved back. Costs a look along the list after each step, stopping at the first page out of place, which is at most about 100 µs at the 200,000 pages a document may have. See `docs/ENGINEERING.md`.
    pub fn is_edited(&self) -> bool {
        self.differs
    }

    /// Whether there's anything to go back through, or anything lost to the cap on it. A list with none of this and no difference from the file has nothing left to keep it.
    pub(crate) fn has_history(&self) -> bool {
        !self.undo.is_empty() || !self.redo.is_empty() || (self.history_dropped && self.differs)
    }

    fn differs_from_the_file(&self) -> bool {
        // Every page in its own place, from its own file, with no turn.
        self.slots.iter().enumerate().any(|(position, slot)| slot.source != 0 || slot.quarter_turns != 0 || slot.page as usize != position) || self.slots.len() != self.file_pages
    }

    /// Whether there's a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there's an undone step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Whether a step that's been undone is the owner's. A new edit, or closing the tab, would lose it.
    pub fn owner_steps_undone(&self) -> bool {
        self.redo.iter().any(|entry| entry.by == Author::Owner)
    }

    /// Whether throwing this list away would lose anything of the owner's: a step still in the history, undone or not, or one that fell off the capped list and whose effect is still in the page order.
    pub fn holds_owner_edits(&self) -> bool {
        (self.owner_step_dropped && self.differs) || self.owner_steps_undone() || self.undo.iter().any(|entry| entry.by == Author::Owner)
    }

    /// Whether some step fell off the capped undo list, so `undo` can't take everything off.
    pub fn history_dropped(&self) -> bool {
        self.history_dropped && self.differs
    }

    /// Sets whose steps the next edits are. For the session alone.
    pub(crate) fn act_as(&mut self, by: Author) {
        self.acting = by;
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
        // A whole turn is nothing done, and isn't a step.
        if turns == 0 {
            return Ok(());
        }
        self.commit(Step::Rotate { start, len, quarter_turns: turns })
    }

    /// Moves a run of pages so its first page ends up at position `to` in the resulting order.
    pub fn move_pages(&mut self, start: u32, len: u32, to: u32) -> Result<(), EditError> {
        self.check_range(start, len)?;
        if u64::from(to) + u64::from(len) > u64::from(self.len()) {
            return Err(EditError::OutOfRange);
        }
        // Already there, so nothing's done, and it isn't a step that clears what's been undone.
        if start == to {
            return Ok(());
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
        self.insert_pdf_stamped(at, path, page_count, None)
    }

    /// [`Self::insert_pdf`], noting which version of the file this is.
    pub fn insert_pdf_stamped(&mut self, at: u32, path: &Path, page_count: u32, stamp: Option<FileStamp>) -> Result<(), EditError> {
        if at > self.len() {
            return Err(EditError::OutOfRange);
        }
        if self.slots.len() + page_count as usize > MAX_PAGES {
            return Err(EditError::TooManyPages);
        }
        let source = self.source_for(path, page_count, stamp)?;
        let slots: Box<[PageSlot]> = (0..page_count).map(|page| PageSlot { source, page, quarter_turns: 0 }).collect();
        self.commit(Step::Restore { start: at, slots })
    }

    /// Reverses the last step. Returns false with nothing left to undo.
    pub fn undo(&mut self) -> bool {
        let Some(Entry { step, by }) = self.undo.pop() else { return false };
        self.history_bytes -= step.held_bytes();
        let inverse = self.apply(step);
        self.history_bytes += inverse.held_bytes();
        self.redo.push(Entry { step: inverse, by });
        self.trim_history();
        self.differs = self.differs_from_the_file();
        true
    }

    /// Repeats the last undone step. Returns false with nothing left to redo.
    pub fn redo(&mut self) -> bool {
        let Some(Entry { step, by }) = self.redo.pop() else { return false };
        self.history_bytes -= step.held_bytes();
        let inverse = self.apply(step);
        self.history_bytes += inverse.held_bytes();
        self.undo.push(Entry { step: inverse, by });
        self.trim_history();
        self.differs = self.differs_from_the_file();
        true
    }

    fn commit(&mut self, step: Step) -> Result<(), EditError> {
        let inverse = self.apply(step);
        self.history_bytes += inverse.held_bytes();
        self.undo.push(Entry { step: inverse, by: self.acting });
        for dropped in self.redo.drain(..) {
            self.history_bytes -= dropped.step.held_bytes();
        }
        self.trim_history();
        self.differs = self.differs_from_the_file();
        Ok(())
    }

    /// Holds the history to its caps, oldest undo first. Anything dropped means the original can't be reached by undoing, and `is_edited` keeps saying so. A dropped undo step's effect stays in the page order, so nothing of it is lost. A dropped redo step is gone, so an owner's isn't dropped, and the steps deeper than it can't be either, since a redo step only applies to the state before it. That can't go far over the cap: the pages a list can hold to redo are the pages of its steps, which a document's length and this cap bound to [`MAX_HISTORY_BYTES`] plus one document.
    fn trim_history(&mut self) {
        while self.undo.len() > MAX_UNDO || (self.history_bytes > MAX_HISTORY_BYTES && self.undo.len() > 1) {
            let dropped = self.undo.remove(0);
            self.history_bytes -= dropped.step.held_bytes();
            self.history_dropped = true;
            self.owner_step_dropped |= dropped.by == Author::Owner;
        }
        while self.history_bytes > MAX_HISTORY_BYTES && self.redo.len() > 1 && self.redo[0].by == Author::Agent {
            let dropped = self.redo.remove(0);
            self.history_bytes -= dropped.step.held_bytes();
        }
    }

    /// What `undo` and `redo` hold now, in bytes.
    pub fn history_bytes(&self) -> usize {
        self.history_bytes
    }

    /// The position of a page, from `source`, nearest to `near`. Nearest, because the same page can appear twice once a PDF has been added twice.
    pub fn nearest_position(&self, source: u16, page: u32, near: u32) -> Option<u32> {
        let near = near as usize;
        let found = self.slots.iter().enumerate().filter(|(_, slot)| slot.source == source && slot.page == page).map(|(position, _)| position).min_by_key(|position| position.abs_diff(near))?;
        u32::try_from(found).ok()
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

    fn source_for(&mut self, path: &Path, page_count: u32, stamp: Option<FileStamp>) -> Result<u16, EditError> {
        if let Some(found) = self.added.iter().position(|added| &*added.path == path) {
            // Slots already point at the old version's pages, so the new version can't share them.
            if self.added[found].stamp != stamp {
                return Err(EditError::SourceChanged);
            }
            return u16::try_from(found + 1).map_err(|_| EditError::TooManySources);
        }
        if self.added.len() >= MAX_SOURCES {
            return Err(EditError::TooManySources);
        }
        self.added.push(AddedSource { path: PathBuf::from(path).into_boxed_path(), page_count, stamp });
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

    /// Failure: `page move 3 --to 3` does nothing, and commits a step anyway, so the tab reads as edited and won't close, and what had been undone is lost for it. A whole turn is the same.
    #[test]
    fn a_step_that_does_nothing_isnt_a_step() {
        let mut edits = PageEdits::new(5);
        edits.move_pages(2, 1, 2).unwrap();
        edits.move_pages(1, 3, 1).unwrap();
        edits.rotate(0, 2, 4).unwrap();
        assert!(!edits.is_edited() && !edits.can_undo(), "{:?}", edits.undo);
        edits.rotate(0, 1, 1).unwrap();
        edits.undo();
        edits.move_pages(2, 1, 2).unwrap();
        assert!(edits.can_redo(), "doing nothing mustn't lose what's undone");
    }

    /// Failure: four quarter turns, or a page moved and moved back, leave a document that's the same as its file reading as edited, so it won't close and asks to be saved with nothing to save. And past the cap, `history_dropped` says it can't get back when it just has.
    #[test]
    fn edited_means_it_differs_from_the_file() {
        let mut turned = PageEdits::new(3);
        for turns_so_far in 1..=4 {
            turned.rotate(1, 1, 1).unwrap();
            assert_eq!(turned.is_edited(), turns_so_far != 4, "after {turns_so_far}");
        }
        assert!(turned.can_undo(), "the steps are still there to go back through");
        turned.undo();
        assert!(turned.is_edited(), "one turn back is three turns on");
        let mut moved = PageEdits::new(4);
        moved.move_pages(0, 1, 3).unwrap();
        assert!(moved.is_edited());
        moved.move_pages(3, 1, 0).unwrap();
        assert!(!moved.is_edited());
        let mut added = PageEdits::new(2);
        added.insert_pdf(2, Path::new("/d/a.pdf"), 1).unwrap();
        assert!(added.is_edited(), "an added page is a difference");
        added.delete(2, 1).unwrap();
        assert!(!added.is_edited(), "and taking it away again isn't");
        let mut swapped = PageEdits::new(2);
        swapped.delete(1, 1).unwrap();
        swapped.insert_pdf(1, Path::new("/d/same.pdf"), 1).unwrap();
        assert!(swapped.is_edited(), "a page from another file isn't this file's, whatever its number");
        // Past the cap, an odd number of swaps differs and an even number doesn't, whatever fell off.
        let mut past = PageEdits::new(2);
        for _ in 0..MAX_UNDO + 2 {
            past.move_pages(0, 1, 1).unwrap();
        }
        assert!(!past.is_edited() && !past.history_dropped(), "it's back at the file, so nothing has been lost");
        assert!(past.undo.len() == MAX_UNDO);
        past.move_pages(0, 1, 1).unwrap();
        assert!(past.is_edited() && past.history_dropped());
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

    /// Failure: past the undo cap, undoing everything reads as "no edits" while the document still differs, so the tab closes and silently loses them.
    #[test]
    fn a_document_past_the_undo_cap_stays_edited_after_undoing_everything() {
        let mut edits = PageEdits::new(2);
        for _ in 0..MAX_UNDO + 1 {
            edits.move_pages(0, 1, 1).unwrap();
        }
        while edits.undo() {}
        assert_ne!(order(&edits), [0, 1], "the oldest step can't be undone any more");
        assert!(edits.is_edited(), "it still differs from the file");
    }

    /// Failure: undo is capped by steps but not bytes, so 256 steps that each remove 200,000 pages hold about 400 MiB per tab. Past the cap the oldest go, and the document says it can't get back.
    #[test]
    fn undo_history_is_capped_in_bytes_and_says_so() {
        let pages = MAX_PAGES as u32;
        let mut heavy = PageEdits::new(pages);
        for round in 0..64 {
            heavy.delete(0, pages - 1).unwrap();
            heavy.insert_pdf(0, Path::new("/d/full.pdf"), pages - 1).unwrap();
            assert!(heavy.history_bytes() <= MAX_HISTORY_BYTES + MAX_PAGES * 8, "round {round}: {}", heavy.history_bytes());
        }
        assert!(heavy.is_edited());
        while heavy.undo() {}
        assert!(heavy.is_edited(), "steps fell off, so undoing everything doesn't reach the original");
        assert!(heavy.history_bytes() <= MAX_HISTORY_BYTES + MAX_PAGES * 8, "{}", heavy.history_bytes());
        assert_eq!(heavy.undo.len(), 0);
    }

    /// Failure: who made a step costs something per page, or enough per step to matter across a tab's history.
    #[test]
    fn an_edit_step_stays_small() {
        assert!(std::mem::size_of::<Entry>() <= 32, "{}", std::mem::size_of::<Entry>());
    }

    fn as_agent(edits: &mut PageEdits, change: impl FnOnce(&mut PageEdits)) {
        edits.act_as(Author::Agent);
        change(edits);
        edits.act_as(Author::Owner);
    }

    /// Failure: an undone step forgets whose it was, or a redo makes it someone else's, so the owner's edits read as an agent's once they've been undone and back, and an agent may then throw them away.
    #[test]
    fn a_step_stays_its_authors_through_undo_and_redo() {
        let mut edits = PageEdits::new(5);
        edits.rotate(0, 1, 1).unwrap();
        as_agent(&mut edits, |edits| edits.delete(1, 1).unwrap());
        assert!(edits.holds_owner_edits() && !edits.owner_steps_undone());
        edits.undo();
        assert!(!edits.owner_steps_undone(), "only the agent's step is undone");
        edits.undo();
        assert!(edits.owner_steps_undone());
        as_agent(&mut edits, |edits| {
            edits.redo();
        });
        assert!(!edits.owner_steps_undone(), "whoever redoes it, it's still the owner's: it's just not undone now");
        assert!(edits.holds_owner_edits());
        let mut only_agent = PageEdits::new(5);
        as_agent(&mut only_agent, |edits| edits.rotate(0, 1, 1).unwrap());
        only_agent.undo();
        assert!(!only_agent.holds_owner_edits() && !only_agent.owner_steps_undone());
    }

    /// Failure: past the cap, an owner's step falls off the list and the list forgets it was ever there, while its effect is still in the page order. An agent's flood then makes the owner's edit look like the agent's own to throw away.
    #[test]
    fn an_owners_step_past_the_cap_still_counts() {
        let mut edits = PageEdits::new(3);
        edits.rotate(0, 1, 1).unwrap();
        as_agent(&mut edits, |edits| {
            for _ in 0..MAX_UNDO {
                edits.move_pages(1, 1, 2).unwrap();
            }
        });
        assert!(edits.history_dropped());
        assert!(edits.undo.iter().all(|entry| entry.by == Author::Agent), "it's no longer on the list");
        assert!(edits.holds_owner_edits(), "its effect is still in the document");
        while edits.undo() {}
        assert_eq!(edits.slots()[0].quarter_turns, 1, "and the cap hasn't lost it");
        assert!(edits.is_edited());
        let mut agents = PageEdits::new(3);
        as_agent(&mut agents, |edits| {
            for _ in 0..=MAX_UNDO {
                edits.move_pages(1, 1, 2).unwrap();
            }
        });
        assert!(agents.history_dropped() && !agents.holds_owner_edits(), "the cap dropping an agent's step is nothing of the owner's");
    }

    /// Failure: a run of heavy agent steps under an owner's undone step pushes the redo list over its byte cap, so undoing them all drops the owner's step for good, with no edit that says so: an agent erases the owner's edit by undoing. The list may pass the cap by one document, and no further.
    #[test]
    fn undoing_never_drops_an_owners_undone_step() {
        let mut edits = PageEdits::new(1);
        as_agent(&mut edits, |edits| {
            // An insert holds nothing until it's undone, and a delete holds its pages until it's undone. So undoing each pair below leaves the bytes where they were, and the two inserts under them go past the cap with nothing older left to let go of.
            for _ in 0..2 {
                edits.insert_pdf(0, Path::new("/d/part.pdf"), 4_999).unwrap();
            }
            for _ in 0..11 {
                edits.insert_pdf(0, Path::new("/d/part.pdf"), 190_000).unwrap();
                edits.delete(0, 190_000).unwrap();
            }
        });
        edits.rotate(0, 1, 1).unwrap();
        while edits.undo() {
            assert!(edits.history_bytes() <= MAX_HISTORY_BYTES + MAX_PAGES * 8, "{}", edits.history_bytes());
        }
        assert!(edits.owner_steps_undone(), "the owner's step is still there to redo");
        assert_eq!(edits.redo[0].by, Author::Owner);
        while edits.redo() {}
        assert_eq!(edits.slots()[0].quarter_turns, 1, "and it comes back whole");
    }

    /// Failure: the place can't be followed to the right copy of a page added twice.
    #[test]
    fn the_nearest_copy_of_a_page_is_found() {
        let mut edits = PageEdits::new(1);
        edits.insert_pdf(1, Path::new("/d/a.pdf"), 1).unwrap();
        edits.insert_pdf(2, Path::new("/d/b.pdf"), 1).unwrap();
        edits.insert_pdf(3, Path::new("/d/a.pdf"), 1).unwrap();
        assert_eq!(edits.nearest_position(1, 0, 0), Some(1));
        assert_eq!(edits.nearest_position(1, 0, 3), Some(3));
        assert_eq!(edits.nearest_position(9, 0, 0), None);
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
