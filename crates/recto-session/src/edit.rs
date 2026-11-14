//! Page edits as a small list, applied only when the user saves.
//!
//! Rearranging, rotating, deleting and adding pages touch no PDF bytes and read no page content, so they're instant and
//! cost about 8 bytes a page, whatever the size of the PDFs. A tab with no edits has no list at all. Undo stores the
//! inverse of each step rather than copies of the list. The original files stay as they are until Save.
//!
//! **History rolls, and never reaches a PDF** (owner, 2026-11-07: "We don't want to add the undo redo history to the file itself, but we can have a rolling save redo history on the system"). A tab's undo and redo keep the last [`MAX_UNDO`] steps or [`MAX_HISTORY_BYTES`], whichever comes first, oldest out. Past that, edits go on: dropping a step drops only the way back, as what it did is in the page order. Below Full, an agent's steps can't push the owner's out: see [`EditError::HistoryFull`].
//!
//! **What's on disk.** With a journal, each thing asked for is noted as a [`JournalOp`] for the writer to append, and replaying those through these same checked calls brings the whole list back, history and authors included, ending exactly where the live list is, as the same caps apply on the way.

use crate::record::{AddedRecord, AuthorName, EditsRecord, EditsSummary, EntryRecord, FileRecord, JournalOp, PathField, RecordError, SlotRun, StepRecord};
use marks::{MarkChange, MarkOp, MarkRefusal, MarkStep, Marks};
use recto_platform::FileStamp;
use std::path::{Path, PathBuf};

pub mod marks;

/// Most pages one edited document may have.
pub const MAX_PAGES: usize = 200_000;
/// Most PDFs that may be added to one document.
pub const MAX_SOURCES: usize = 1024;
/// Undo steps kept. The oldest go first. A named number because the owner's isn't final: "a limit of 1,000 steps", 2026-11-07.
pub const MAX_UNDO: usize = 1000;
/// Bytes of pages held for undo and redo, per PDF. A step is usually a few bytes, but one that removes 200,000 pages holds 1.6 MB of them. The oldest go first, and the last step always stays.
pub const MAX_HISTORY_BYTES: usize = 64 * 1024 * 1024;

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
    /// An agent's next step would push the owner's steps out of the history, so they couldn't be undone.
    HistoryFull,
    /// No highlight made as an edit has that number.
    NoSuchHighlight,
    /// The document holds as many unsaved highlights as it may.
    TooManyHighlights,
}

impl EditError {
    pub fn code(self) -> &'static str {
        match self {
            Self::OutOfRange => "page_out_of_range",
            Self::LastPage => "last_page",
            Self::TooManyPages => "too_many_pages",
            Self::TooManySources => "too_many_sources",
            Self::SourceChanged => "source_changed",
            Self::HistoryFull => "history_full",
            Self::NoSuchHighlight => "no_such_highlight",
            Self::TooManyHighlights => "too_many_highlights",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::OutOfRange => "That page isn't in this document.",
            Self::LastPage => "A PDF has to keep at least one page.",
            Self::TooManyPages => "That would make this document too long.",
            Self::TooManySources => "Too many PDFs have been added to this document.",
            Self::SourceChanged => "That PDF has changed since it was added, so it can't be added again.",
            Self::NoSuchHighlight => "No highlight made in this tab, and not yet saved, has that number.",
            Self::TooManyHighlights => "This document has as many unsaved highlights as it can hold. Save, and then go on.",
            Self::HistoryFull => {
                "The undo history is full, and this edit would push the user's own steps out of it, so it wasn't made. Ask them to save or to undo, which makes room."
            }
        }
    }
}

/// A step and how to reverse it. Each direction of undo and redo produces the other.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Rotate {
        start: u32,
        len: u32,
        quarter_turns: u8,
    },
    Move {
        start: u32,
        len: u32,
        to: u32,
    },
    Remove {
        start: u32,
        len: u32,
    },
    Restore {
        start: u32,
        slots: Box<[PageSlot]>,
    },
    /// A highlight added or taken away, which no page step depends on (see [`marks`]).
    Mark(Box<MarkStep>),
}

impl Step {
    fn to_record(&self) -> StepRecord {
        match self {
            Self::Rotate { start, len, quarter_turns } => StepRecord::Rotate {
                start: *start,
                len: *len,
                quarter_turns: *quarter_turns,
            },
            Self::Move { start, len, to } => StepRecord::Move {
                start: *start,
                len: *len,
                to: *to,
            },
            Self::Remove { start, len } => StepRecord::Remove { start: *start, len: *len },
            Self::Restore { start, slots } => StepRecord::Restore {
                start: *start,
                runs: runs_of(slots),
            },
            Self::Mark(step) => StepRecord::Mark(step.to_record()),
        }
    }

    /// From a record already checked by `validate_edits`, whose runs have been counted.
    fn from_record(record: &StepRecord) -> Self {
        match record {
            StepRecord::Rotate { start, len, quarter_turns } => Self::Rotate {
                start: *start,
                len: *len,
                quarter_turns: *quarter_turns,
            },
            StepRecord::Move { start, len, to } => Self::Move {
                start: *start,
                len: *len,
                to: *to,
            },
            StepRecord::Remove { start, len } => Self::Remove { start: *start, len: *len },
            StepRecord::Restore { start, runs } => Self::Restore {
                start: *start,
                slots: slots_of(runs).into_boxed_slice(),
            },
            StepRecord::Mark(step) => Self::Mark(Box::new(MarkStep::from_record(step))),
        }
    }

    /// What its inverse would hold beyond its own size.
    fn uses_source(&self, source: u16) -> bool {
        match self {
            Self::Restore { slots, .. } => slots.iter().any(|slot| slot.source == source),
            // A highlight on a page of the tab's own file needs nothing kept: a save moves it onto the page's new number (`marks::saved`).
            Self::Mark(step) => source != 0 && step.adds().iter().any(|mark| mark.source == source),
            _ => false,
        }
    }

    fn inverse_held_bytes(&self) -> usize {
        match self {
            Self::Remove { len, .. } => *len as usize * size_of::<PageSlot>(),
            _ => 0,
        }
    }

    /// What it holds beyond its own size.
    fn held_bytes(&self) -> usize {
        match self {
            Self::Restore { slots, .. } => size_of_val::<[PageSlot]>(slots),
            Self::Mark(step) => step.held_bytes(),
            _ => 0,
        }
    }
}

/// Pages as runs: a long document with one page turned is three of them, whatever its length.
fn runs_of(slots: &[PageSlot]) -> Vec<SlotRun> {
    let mut runs: Vec<SlotRun> = Vec::new();
    for slot in slots {
        match runs.last_mut() {
            Some(run) if run.0 == slot.source && run.3 == slot.quarter_turns && run.1 + run.2 == slot.page => run.2 += 1,
            _ => runs.push(SlotRun(slot.source, slot.page, 1, slot.quarter_turns)),
        }
    }
    runs
}

/// The pages of runs that have been counted.
fn slots_of(runs: &[SlotRun]) -> Vec<PageSlot> {
    runs.iter()
        .flat_map(|run| {
            (0..run.2).map(move |at| PageSlot {
                source: run.0,
                page: run.1 + at,
                quarter_turns: run.3,
            })
        })
        .collect()
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
    /// A file of the history's own, an earlier version of this PDF, which goes when nothing needs it.
    pub kept: bool,
    /// It's protected, so nothing of its pages is written anywhere in the clear: a document with one has no pictures in its sleep file.
    pub protected: bool,
    /// A copy kept for a save holds only these pages of the file as it was, in this order (owner, 2026-11-07: undoing a delete past a Save keeps only the removed pages). The save's rebase moves the history's page numbers onto it and drops this. `None` for a whole file.
    pub holds: Option<Box<[u32]>>,
}

/// What came of a save, for a list that goes on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rebased {
    /// Steps that had to go, as the pages they need couldn't be kept.
    pub rolled_off: usize,
    /// Files kept for this history that nothing needs now.
    pub let_go: Vec<Box<Path>>,
}

/// The edited page order of one document.
#[derive(Clone, Debug)]
pub struct PageEdits {
    /// How long the file was when this list was made.
    file_pages: usize,
    slots: Vec<PageSlot>,
    added: Vec<AddedSource>,
    /// The undo and redo steps, and what they hold.
    history: History,
    /// Highlights made as edits.
    marks: Marks,
    /// Whether the document differs from the file, whatever steps took it there. Worked out after each step, so asking is free. See [`Self::is_edited`].
    differs: bool,
    /// Whose steps are being made now. Only the session sets this, and the owner is the default: a path that forgets to say gets the more protected answer.
    acting: Author,
    /// Operations ever asked of this list, counting those in its journal's base, and the ones the journal doesn't have yet. `None` is a list with no journal, whose oldest steps are lost past the cap.
    journal: Option<Outbox>,
    /// Whether an agent may push the owner's steps out of the history. Only at Full, for one call.
    may_push_owner: bool,
    /// A save said it was about to put this in place, and hasn't been confirmed. Goes in a journal's header when it's compacted.
    saving: Option<FileStamp>,
    /// And what it kept to undo past it.
    keeping: Option<AddedSource>,
    /// Goes up with every change to the pages, so what was found on them can tell it's of another arrangement.
    changes: u64,
    /// Saving writes the file anew: see [`Self::rewrites`].
    rewrites: bool,
    /// A journal is being replayed, so `differs` and `rewrites` are worked out once at its end rather than after each step.
    replaying: bool,
}

/// **A list's undo and redo,** and what they hold against the caps. One part of [`PageEdits`], so the list's fields say what it is (R4.3: the
/// history's five fields were folded here unchanged, to make room for a kind of step that isn't a page's).
#[derive(Clone, Debug, Default)]
struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    /// What `undo` and `redo` hold, against [`MAX_HISTORY_BYTES`].
    bytes: usize,
    /// A step fell off the capped undo list, so this document can't get back to the original by undoing.
    dropped: bool,
    /// One of those was the owner's. Its effect is still in the page order: nothing has lost it, but throwing the list away would, so it counts for as long as the list does.
    owner_dropped: bool,
}

/// What a journal hasn't got yet.
#[derive(Clone, Debug, Default)]
struct Outbox {
    /// How many operations this list has had, all told.
    seq: u64,
    /// The last of those, in order. Taken out only once they're in the file.
    ops: Vec<JournalOp>,
}

impl AddedSource {
    pub fn to_record(&self) -> AddedRecord {
        AddedRecord {
            path: PathField::from_path(&self.path),
            page_count: self.page_count,
            file: self.stamp.map(FileRecord::from),
            kept: self.kept,
            protected: self.protected,
            holds: self.holds.as_ref().map(|holds| holds.to_vec()),
        }
    }

    pub fn from_record(record: &AddedRecord) -> Option<Self> {
        // A record can say anything: what it holds is ascending, within limits, and as long as the copy.
        let holds = match &record.holds {
            None => None,
            Some(holds) if holds.len() == record.page_count as usize && holds.len() <= MAX_PAGES && holds.windows(2).all(|pair| pair[0] < pair[1]) => {
                Some(holds.clone().into_boxed_slice())
            }
            Some(_) => return None,
        };
        Some(Self {
            path: record.path.to_path()?.into_boxed_path(),
            page_count: record.page_count,
            stamp: record.file.map(FileStamp::from),
            kept: record.kept,
            protected: record.protected,
            holds,
        })
    }
}

/// **A list's edits said to be someone's,** given by [`PageEdits::acting`]: the owner's again, with none of the owner's steps to push out, when it's
/// dropped, by returning or by a panic. A step that panics is caught by the server and the session's lock recovers, so a reset after the step would
/// leave the next edits counted as the agent's, and may push the owner's steps out.
pub(crate) struct ActingOn<'e>(&'e mut PageEdits);

impl std::ops::Deref for ActingOn<'_> {
    type Target = PageEdits;
    fn deref(&self) -> &PageEdits {
        self.0
    }
}

impl std::ops::DerefMut for ActingOn<'_> {
    fn deref_mut(&mut self) -> &mut PageEdits {
        self.0
    }
}

impl Drop for ActingOn<'_> {
    fn drop(&mut self) {
        self.0.acting = Author::Owner;
        self.0.may_push_owner = false;
    }
}

impl PageEdits {
    pub fn new(page_count: u32) -> Self {
        let slots = (0..page_count)
            .map(|page| PageSlot {
                source: 0,
                page,
                quarter_turns: 0,
            })
            .collect();
        Self {
            file_pages: page_count as usize,
            slots,
            added: Vec::new(),
            history: History::default(),
            marks: Marks::default(),
            differs: false,
            acting: Author::Owner,
            journal: None,
            may_push_owner: false,
            saving: None,
            keeping: None,
            rewrites: false,
            changes: 0,
            replaying: false,
        }
    }

    /// Keeps this list's operations for a journal, from now on.
    pub fn journal_from_now(&mut self) {
        if self.journal.is_none() {
            self.journal = Some(Outbox::default());
        }
    }

    /// The operations the journal hasn't got yet, and how many there have been in all up to the last of them.
    pub fn journal_waiting(&self) -> Option<(u64, &[JournalOp])> {
        self.journal.as_ref().map(|outbox| (outbox.seq, outbox.ops.as_slice()))
    }

    /// Those up to this many in all are in the file now.
    pub fn journal_written(&mut self, through: u64) {
        if let Some(outbox) = &mut self.journal {
            let waiting = outbox.ops.len() as u64;
            let first = outbox.seq - waiting;
            let done = through.saturating_sub(first).min(waiting);
            outbox.ops.drain(..usize::try_from(done).unwrap_or(0));
            if outbox.ops.capacity() > 256 && outbox.ops.len() < 64 {
                outbox.ops.shrink_to_fit();
            }
        }
    }

    /// How many operations this list has had, all told.
    pub fn journal_seq(&self) -> u64 {
        self.journal.as_ref().map_or(0, |outbox| outbox.seq)
    }

    fn note(&mut self, op: impl FnOnce() -> JournalOp) {
        if let Some(outbox) = &mut self.journal {
            outbox.seq += 1;
            outbox.ops.push(op());
        }
    }

    /// What a list comes to, without opening it.
    pub fn summary(&self) -> EditsSummary {
        EditsSummary {
            pages: self.len(),
            edited: self.is_edited(),
            holds_owner_edits: self.holds_owner_edits(),
            owner_steps_undone: self.owner_steps_undone(),
            undo: u32::try_from(self.history.undo.len()).unwrap_or(u32::MAX),
            redo: u32::try_from(self.history.redo.len()).unwrap_or(u32::MAX),
            rewrites: self.rewrites(),
        }
    }

    /// The list as a whole, for a journal's base and for QA.
    pub fn to_record(&self) -> EditsRecord {
        let entry = |entry: &Entry| EntryRecord {
            by: entry.by.into(),
            step: entry.step.to_record(),
        };
        EditsRecord {
            file_pages: u32::try_from(self.file_pages).unwrap_or(u32::MAX),
            pages: runs_of(&self.slots),
            added: self.added.iter().map(AddedSource::to_record).collect(),
            undo: self.history.undo.iter().map(entry).collect(),
            redo: self.history.redo.iter().map(entry).collect(),
            history_dropped: self.history.dropped,
            owner_step_dropped: self.history.owner_dropped,
            marks: self.marks.to_record(),
        }
    }

    /// A list from a record that's been through [`crate::record::validate_edits`], which is what makes this safe: every step in it is in range. Refuses one that isn't, however it got here.
    pub fn from_record(record: &EditsRecord) -> Result<Self, RecordError> {
        crate::record::validate_edits(record)?;
        let bad = RecordError::Damaged("a source out of range");
        let added: Vec<AddedSource> = record.added.iter().map(AddedSource::from_record).collect::<Option<_>>().ok_or(bad)?;
        let mut history_bytes = 0;
        let entries = |list: &[EntryRecord], bytes: &mut usize| -> Vec<Entry> {
            list.iter()
                .map(|entry| {
                    let step = Step::from_record(&entry.step);
                    *bytes += step.held_bytes();
                    Entry { step, by: entry.by.into() }
                })
                .collect()
        };
        let undo = entries(&record.undo, &mut history_bytes);
        let redo = entries(&record.redo, &mut history_bytes);
        let mut marks = Marks::from_record(&record.marks).ok_or(RecordError::Damaged("a highlight that can't be"))?;
        // A highlight the history can bring back keeps its number: a new one starts past every number named.
        for entry in undo.iter().chain(&redo) {
            if let Step::Mark(step) = &entry.step {
                marks.count_past(step);
            }
        }
        let mut edits = Self {
            file_pages: record.file_pages as usize,
            slots: slots_of(&record.pages),
            added,
            history: History {
                undo,
                redo,
                bytes: history_bytes,
                dropped: record.history_dropped,
                owner_dropped: record.owner_step_dropped,
            },
            marks,
            differs: false,
            acting: Author::Owner,
            journal: None,
            may_push_owner: false,
            saving: None,
            keeping: None,
            rewrites: false,
            changes: 0,
            replaying: false,
        };
        edits.differs = edits.differs_from_the_file();
        edits.rewrites = edits.rewrites_the_file();
        Ok(edits)
    }

    /// What a save said it was about to put in place, if one has.
    pub fn saving(&self) -> Option<FileStamp> {
        self.saving
    }

    /// Whether the document has a page from a protected file.
    pub fn has_protected_pages(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.source != 0 && self.added.get(usize::from(slot.source) - 1).is_some_and(|added| added.protected))
    }

    /// What's on each page of the document, as a number: pictures of it are about this order of these files, and no other. Zero when it's the file as it is, however it got back there, so a discard and a new edit can't come to match by counting.
    pub fn order_mark(&self) -> u64 {
        if !self.differs {
            return 0;
        }
        let mut hash = crc32fast::Hasher::new();
        hash.update(&(self.slots.len() as u64).to_le_bytes());
        for slot in &self.slots {
            hash.update(&slot.source.to_le_bytes());
            hash.update(&slot.page.to_le_bytes());
            hash.update(&[slot.quarter_turns]);
        }
        let low = hash.finalize();
        let mut hash = crc32fast::Hasher::new_with_initial(low ^ 0x5bd1_e995);
        for added in &self.added {
            hash.update(added.path.as_os_str().as_encoded_bytes());
            if let Some(stamp) = added.stamp {
                hash.update(&stamp.size.to_le_bytes());
                hash.update(&stamp.inode.to_le_bytes());
            }
        }
        (u64::from(hash.finalize()) << 32) | u64::from(low) | 1
    }

    /// What a save said it had kept, if it did.
    pub fn keeping(&self) -> Option<&AddedSource> {
        self.keeping.as_ref()
    }

    /// **Brings a list back from its journal,** through the same checked calls that made it, so a hostile journal can do no more than an agent could, and with the same caps, so it ends up just as the live list is. Stops at the first operation that doesn't apply, and says how many did. Who made each step decides what's refused only when it was first asked, so nothing's refused here for that.
    pub fn replay(file_pages: u32, base: Option<&EditsRecord>, ops: &[JournalOp]) -> Result<(Self, usize), RecordError> {
        let mut edits = match base {
            Some(base) => Self::from_record(base)?,
            None => Self::new(file_pages),
        };
        edits.may_push_owner = true;
        edits.replaying = true;
        let mut applied = 0;
        let mut work = 0u64;
        for op in ops {
            // What each costs at the most: a look along the document, for what differs, and what it moves.
            work += u64::from(edits.len()) + 1;
            if work > crate::record::MAX_REPLAY_WORK || edits.apply_op(op).is_err() {
                break;
            }
            applied += 1;
        }
        edits.may_push_owner = false;
        edits.replaying = false;
        edits.journal = Some(Outbox::default());
        edits.acting = Author::Owner;
        edits.differs = edits.differs_from_the_file();
        edits.rewrites = edits.rewrites_the_file();
        Ok((edits, applied))
    }

    /// Goes on with operations that hadn't reached the file, with their count kept right.
    pub fn apply_replayed(&mut self, op: &JournalOp) -> Result<(), EditError> {
        let journal = self.journal.take();
        self.may_push_owner = true;
        let result = self.apply_op(op);
        self.may_push_owner = false;
        self.journal = journal;
        self.acting = Author::Owner;
        result
    }

    /// This list isn't kept in a journal, here.
    pub fn no_journal(&mut self) {
        self.journal = None;
    }

    /// Says where the count stands, once a list has been brought back.
    pub fn set_journal_seq(&mut self, seq: u64) {
        self.journal = Some(Outbox { seq, ops: Vec::new() });
    }

    /// Puts operations waiting back, as ones the file hasn't got.
    pub fn keep_waiting(&mut self, ops: Vec<JournalOp>) {
        if let Some(outbox) = &mut self.journal {
            outbox.ops = ops;
        }
    }

    fn apply_op(&mut self, op: &JournalOp) -> Result<(), EditError> {
        match op {
            JournalOp::Rotate(start, len, turns, by) => {
                self.acting = (*by).into();
                self.rotate(*start, *len, i32::from(*turns))
            }
            JournalOp::Move(start, len, to, by) => {
                self.acting = (*by).into();
                self.move_pages(*start, *len, *to)
            }
            JournalOp::Delete(start, len, by) => {
                self.acting = (*by).into();
                self.delete(*start, *len)
            }
            JournalOp::Insert(at, source, count, by) => {
                self.acting = (*by).into();
                self.insert_source_pages(*at, *source, *count)
            }
            JournalOp::AddSource(added) => {
                if self.added.len() >= MAX_SOURCES {
                    return Err(EditError::TooManySources);
                }
                if !crate::record::added_fits(added) {
                    return Err(EditError::OutOfRange);
                }
                self.added.push(AddedSource::from_record(added).ok_or(EditError::OutOfRange)?);
                Ok(())
            }
            JournalOp::Mark(op) => {
                let (change, by) = op.change();
                self.acting = by.into();
                self.mark(change)
            }
            JournalOp::Undo => self.undo().then_some(()).ok_or(EditError::OutOfRange),
            JournalOp::Redo => self.redo().then_some(()).ok_or(EditError::OutOfRange),
            JournalOp::Saving(target) => {
                self.saving = Some((*target).into());
                Ok(())
            }
            JournalOp::Keeping(kept) => {
                if !kept.kept || !crate::record::added_fits(kept) {
                    return Err(EditError::OutOfRange);
                }
                self.keeping = Some(AddedSource::from_record(kept).ok_or(EditError::OutOfRange)?);
                Ok(())
            }
        }
    }

    fn insert_source_pages(&mut self, at: u32, source: u16, count: u32) -> Result<(), EditError> {
        if at > self.len() {
            return Err(EditError::OutOfRange);
        }
        if self.slots.len() + count as usize > MAX_PAGES {
            return Err(EditError::TooManyPages);
        }
        if source == 0 || usize::from(source) > self.added.len() {
            return Err(EditError::OutOfRange);
        }
        // No more pages than the file has: a journal can say any count, and a page past the end is one nothing can draw, and a list that doesn't read back.
        if count > self.added[usize::from(source) - 1].page_count {
            return Err(EditError::OutOfRange);
        }
        let slots: Box<[PageSlot]> = (0..count)
            .map(|page| PageSlot {
                source,
                page,
                quarter_turns: 0,
            })
            .collect();
        self.commit(Step::Restore { start: at, slots })
    }

    pub fn slots(&self) -> &[PageSlot] {
        &self.slots
    }

    /// How long the file this list is about was.
    pub fn file_pages(&self) -> u32 {
        u32::try_from(self.file_pages).unwrap_or(u32::MAX)
    }

    /// How many changes the pages have had, in this process: anything found on them before a change may be on another page now.
    pub fn changes(&self) -> u64 {
        self.changes
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

    /// Whether the document differs from the file, and so whether anything is waiting to be saved. Undoing everything brings this back to false, and so does getting back to the file by other steps: four quarter turns, or a page moved and moved back. Costs a look along the list after each step, stopping at the first page out of place, which is at most about 100 µs at the 200,000 pages a document may have. See `docs/engineering/agent-interface.md`.
    pub fn is_edited(&self) -> bool {
        self.differs
    }

    /// Whether there's anything to go back through, or anything lost to the cap on it. A list with none of this and no difference from the file has nothing left to keep it.
    pub fn has_history(&self) -> bool {
        !self.history.undo.is_empty() || !self.history.redo.is_empty() || (self.history.dropped && self.differs)
    }

    /// Whether saving writes the file anew: each of the file's pages isn't there exactly once. `recto_core::save::writer_for`'s rule, worked out when the list changes rather than each time it's asked.
    fn rewrites_the_file(&self) -> bool {
        #[cfg(test)]
        tests::count_look();
        let mut seen = vec![false; self.file_pages];
        for slot in self.slots.iter().filter(|slot| slot.source == 0) {
            match seen.get_mut(slot.page as usize) {
                Some(there) if !*there => *there = true,
                _ => return true,
            }
        }
        !seen.iter().all(|there| *there)
    }

    /// Whether saving these edits writes the file anew. Kept up to date as the list changes.
    pub fn rewrites(&self) -> bool {
        self.differs && self.rewrites
    }

    fn differs_from_the_file(&self) -> bool {
        #[cfg(test)]
        tests::count_look();
        // Every page in its own place, from its own file, with no turn.
        self.slots
            .iter()
            .enumerate()
            .any(|(position, slot)| slot.source != 0 || slot.quarter_turns != 0 || slot.page as usize != position)
            || self.slots.len() != self.file_pages
            || !self.marks.is_empty()
    }

    /// Whether there's a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    /// Whether there's an undone step to redo.
    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// Whether a step that's been undone is the owner's. A new edit, or closing the tab, would lose it.
    pub fn owner_steps_undone(&self) -> bool {
        self.history.redo.iter().any(|entry| entry.by == Author::Owner)
    }

    /// Whether throwing this list away would lose anything of the owner's: a step still in the history, undone or not, or one that fell off the capped list and whose effect is still in the page order.
    pub fn holds_owner_edits(&self) -> bool {
        (self.history.owner_dropped && self.differs) || self.owner_steps_undone() || self.history.undo.iter().any(|entry| entry.by == Author::Owner)
    }

    /// Whether some step fell off the capped undo list, so `undo` can't take everything off.
    pub fn history_dropped(&self) -> bool {
        self.history.dropped && self.differs
    }

    /// Notes, ahead of it, what a save is about to put in place, and what was kept for undoing past it. See [`JournalOp::Saving`].
    pub(crate) fn note_saving(&mut self, target: FileStamp, keeping: Option<&AddedSource>) {
        self.saving = Some(target);
        if let Some(kept) = keeping {
            self.keeping = Some(kept.clone());
            self.note(|| JournalOp::Keeping(kept.to_record()));
        }
        self.note(|| JournalOp::Saving(target.into()));
    }

    /// Whether a step in the history needs pages of the file as it is, which a save may take out of it. Only those need a copy kept: a turn or a move goes by position, and needs none.
    pub fn needs_own_pages(&self) -> bool {
        self.history.undo.iter().chain(&self.history.redo).any(|entry| entry.step.uses_source(0))
    }

    /// **The file's pages the history can put back,** ascending: what a copy kept for a save has to hold, and all it holds. Bounded by the history, which is capped.
    pub fn own_pages_needed(&self) -> Vec<u32> {
        let mut pages: Vec<u32> = self
            .history
            .undo
            .iter()
            .chain(&self.history.redo)
            .filter_map(|entry| match &entry.step {
                Step::Restore { slots, .. } => Some(slots.iter().filter(|slot| slot.source == 0).map(|slot| slot.page)),
                _ => None,
            })
            .flatten()
            .collect();
        pages.sort_unstable();
        pages.dedup();
        pages
    }

    /// **What a confirmed save does to a list: it marks the saved point.** Everything that's been done is in the new file, in this order, so page `n` of the document is page `n` of the file. The list starts again from there, with every page in its own place, which is what "reads as saved" is: [`Self::is_edited`] is whether the document differs from the file, so undoing away from this point makes it unsaved, and coming back reads as saved, by any steps.
    ///
    /// **Steps stay meaningful, as they're about positions.** A turn, a move or a removal is, and the document's positions are what they were. The only steps that name pages are those that put pages back, and those are of pages that the document doesn't have now, so not in the new file, unless they're from an added PDF, which is still its own file. So they go over to `kept`, a copy of the file as it was, in the history's own folder. Where that isn't there, or doesn't fit, they roll off, with all older ones, as a step's way back depends on those after it: undo stops short and says so, rather than undoing wrongly.
    ///
    /// `pages` not being this list's length means what was written isn't this list, so none of it is about that file. `None` says so.
    pub fn rebase_on_saved(&mut self, pages: u32, kept: Option<AddedSource>) -> Option<Rebased> {
        if pages as usize != self.slots.len() {
            return None;
        }
        let mut out = Rebased::default();
        let own_pages_at = match kept {
            Some(kept) if self.needs_own_pages() && self.added.len() < MAX_SOURCES => {
                self.added.push(kept);
                Some(u16::try_from(self.added.len()).unwrap_or(u16::MAX))
            }
            Some(kept) => {
                if kept.kept {
                    out.let_go.push(kept.path);
                }
                None
            }
            None => None,
        };
        // Copies kept are held to the limit a PDF's history has, oldest out.
        let mut kept_bytes: u64 = self
            .added
            .iter()
            .filter(|added| added.kept)
            .map(|added| added.stamp.map_or(0, |stamp| stamp.size))
            .sum();
        for at in 0..self.added.len() {
            if kept_bytes <= MAX_HISTORY_BYTES as u64 {
                break;
            }
            if self.added[at].kept {
                kept_bytes -= self.added[at].stamp.map_or(0, |stamp| stamp.size);
                out.rolled_off += self.roll_off_source(u16::try_from(at + 1).unwrap_or(u16::MAX));
            }
        }
        match own_pages_at {
            Some(source) => {
                // A copy of only some pages has them at its own numbers: each step's page goes to where it is there. One it doesn't hold can't be put back, so it rolls off, with what's older.
                let holds = self.added.last_mut().and_then(|kept| kept.holds.take());
                let mut missing = false;
                for entry in self.history.undo.iter_mut().chain(&mut self.history.redo) {
                    if let Step::Restore { slots, .. } = &mut entry.step {
                        for slot in slots.iter_mut().filter(|slot| slot.source == 0) {
                            match holds.as_deref().map(|holds| holds.binary_search(&slot.page)) {
                                None => slot.source = source,
                                Some(Ok(at)) => {
                                    slot.source = source;
                                    slot.page = u32::try_from(at).unwrap_or(u32::MAX);
                                }
                                Some(Err(_)) => missing = true,
                            }
                        }
                    }
                }
                if missing {
                    out.rolled_off += self.roll_off_source(0);
                }
            }
            None => out.rolled_off += self.roll_off_source(0),
        }
        out.rolled_off += marks::saved(self);
        self.file_pages = pages as usize;
        self.changes += 1;
        self.slots = (0..pages)
            .map(|page| PageSlot {
                source: 0,
                page,
                quarter_turns: 0,
            })
            .collect();
        out.let_go.extend(self.drop_unused_sources());
        // What fell off before this is in the file, so nothing's lost with it.
        self.history.dropped = false;
        self.history.owner_dropped = false;
        self.differs = false;
        self.rewrites = false;
        self.saving = None;
        self.keeping = None;
        self.history_starts_again();
        Some(out)
    }

    /// A save noted with [`Self::note_saving`] didn't put its file in place: what it noted goes, and the edits stand. Gives back the copy it kept, if any, which nothing needs now.
    pub(crate) fn save_abandoned(&mut self) -> Option<Box<Path>> {
        self.saving = None;
        self.keeping.take().filter(|kept| kept.kept).map(|kept| kept.path)
    }

    /// Takes the history away and keeps the page order, for Clear Edit History with unsaved edits. Says which kept files nothing needs now.
    pub(crate) fn drop_history(&mut self) -> Vec<Box<Path>> {
        self.history.owner_dropped |= self.history.undo.iter().any(|entry| entry.by == Author::Owner);
        self.history.dropped = true;
        self.history.undo.clear();
        self.history.redo.clear();
        self.history.bytes = 0;
        self.saving = None;
        self.keeping = None;
        self.history_starts_again();
        self.drop_unused_sources()
    }

    /// What's waiting for the journal is in the whole list now, which is what goes next. The count steps on, to tell this from any writing under way.
    fn history_starts_again(&mut self) {
        if let Some(outbox) = &mut self.journal {
            outbox.seq += 1;
            outbox.ops.clear();
        }
    }

    /// Takes off the steps that need this source, and all older ones, from both ends. Says how many.
    fn roll_off_source(&mut self, source: u16) -> usize {
        let mut gone = 0;
        for list in [&mut self.history.undo, &mut self.history.redo] {
            if let Some(last) = list.iter().rposition(|entry| entry.step.uses_source(source)) {
                for entry in list.drain(..=last) {
                    self.history.bytes -= entry.step.held_bytes();
                    gone += 1;
                }
            }
        }
        gone
    }

    /// Kept files this list names, for checking they're still what they were.
    pub fn kept_sources(&self) -> impl Iterator<Item = (u16, &AddedSource)> {
        self.added
            .iter()
            .enumerate()
            .filter(|(_, added)| added.kept)
            .map(|(at, added)| (u16::try_from(at + 1).unwrap_or(u16::MAX), added))
    }

    /// A kept file isn't what it was, so the steps that need it can't be undone. Says how many went.
    pub fn roll_off_kept(&mut self, source: u16) -> usize {
        self.roll_off_source(source)
    }

    /// Forgets added files nothing uses now, in the document or its history, and says which of them were kept files, for those to go. Only when the journal's about to get the whole list, as its later lines go by these numbers.
    fn drop_unused_sources(&mut self) -> Vec<Box<Path>> {
        let mut used = vec![false; self.added.len()];
        let mut note = |source: u16| {
            if source != 0
                && let Some(entry) = used.get_mut(usize::from(source) - 1)
            {
                *entry = true;
            }
        };
        self.slots.iter().for_each(|slot| note(slot.source));
        self.marks.iter().for_each(|mark| note(mark.source));
        for entry in self.history.undo.iter().chain(&self.history.redo) {
            match &entry.step {
                Step::Restore { slots, .. } => slots.iter().for_each(|slot| note(slot.source)),
                Step::Mark(step) => step.adds().iter().for_each(|mark| note(mark.source)),
                _ => {}
            }
        }
        if used.iter().all(|used| *used) {
            return Vec::new();
        }
        let mut new_number = Vec::with_capacity(used.len());
        let mut next = 0u16;
        for used in &used {
            next += u16::from(*used);
            new_number.push(next);
        }
        let renumber = |source: &mut u16| {
            if *source != 0
                && let Some(number) = new_number.get(usize::from(*source) - 1)
            {
                *source = *number;
            }
        };
        self.slots.iter_mut().for_each(|slot| renumber(&mut slot.source));
        self.marks.sources_mut().for_each(renumber);
        for entry in self.history.undo.iter_mut().chain(&mut self.history.redo) {
            match &mut entry.step {
                Step::Restore { slots, .. } => slots.iter_mut().for_each(|slot| renumber(&mut slot.source)),
                Step::Mark(step) => step.adds_mut().iter_mut().for_each(|mark| renumber(&mut mark.source)),
                _ => {}
            }
        }
        let mut let_go = Vec::new();
        for (added, used) in std::mem::take(&mut self.added).into_iter().zip(used) {
            if used {
                self.added.push(added);
            } else if added.kept {
                let_go.push(added.path);
            }
        }
        let_go
    }

    /// **Whose steps the next edits are,** and whether they may push the owner's steps out of the history (at Full, where that's the holder's call),
    /// for as long as what this gives is held. For the session alone.
    pub(crate) fn acting(&mut self, by: Author, may_push_owner: bool) -> ActingOn<'_> {
        self.acting = by;
        self.may_push_owner = may_push_owner;
        ActingOn(self)
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
        self.commit(Step::Rotate {
            start,
            len,
            quarter_turns: turns,
        })
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

    /// Adds a highlight to a page, or takes one made as an edit away: a step like any other, the acting author's.
    pub fn mark(&mut self, change: MarkChange) -> Result<(), EditError> {
        if let MarkChange::Add { pages } = &change {
            let count = |source: u16| match source {
                0 => u32::try_from(self.file_pages).unwrap_or(u32::MAX),
                source => self.added.get(usize::from(source) - 1).map_or(0, |added| added.page_count),
            };
            if pages.iter().any(|on| on.page >= count(on.source)) {
                return Err(EditError::OutOfRange);
            }
        }
        let step = self.marks.step_for(change, self.acting).map_err(|refusal| match refusal {
            MarkRefusal::NoSuch => EditError::NoSuchHighlight,
            MarkRefusal::TooMany => EditError::TooManyHighlights,
            MarkRefusal::Unsound => EditError::OutOfRange,
        })?;
        self.commit(Step::Mark(Box::new(step)))
    }

    /// The highlights made as edits, unsaved.
    pub fn marks(&self) -> &Marks {
        &self.marks
    }

    /// Adds every page of another PDF at a position, as dropping it into the page view does, noting which version of the file this is (`None` if it
    /// couldn't be looked at) and whether it's a protected file.
    pub fn insert_pdf_noting(&mut self, at: u32, path: &Path, page_count: u32, stamp: Option<FileStamp>, protected: bool) -> Result<(), EditError> {
        if at > self.len() {
            return Err(EditError::OutOfRange);
        }
        if self.slots.len() + page_count as usize > MAX_PAGES {
            return Err(EditError::TooManyPages);
        }
        // Nothing to add, and a step of nothing can't be replayed.
        if page_count == 0 {
            return Ok(());
        }
        let (sources_before, noted_before) = (self.added.len(), self.journal.as_ref().map(|outbox| (outbox.seq, outbox.ops.len())));
        let source = self.source_for(path, page_count, stamp, protected)?;
        let slots: Box<[PageSlot]> = (0..page_count)
            .map(|page| PageSlot {
                source,
                page,
                quarter_turns: 0,
            })
            .collect();
        let committed = self.commit(Step::Restore { start: at, slots });
        // A refused insert leaves nothing behind: the file it would have added isn't one of the list's, nor in its journal, so it neither counts towards the limit on files nor takes a number the next one should have.
        if committed.is_err() && self.added.len() > sources_before {
            self.added.truncate(sources_before);
            if let (Some(outbox), Some((seq, len))) = (&mut self.journal, noted_before) {
                outbox.seq = seq;
                outbox.ops.truncate(len);
            }
        }
        committed
    }

    /// [`Self::undo`], or says it would push the owner's steps out of the history. An undo can hold more than the step did: undoing an insert holds its pages.
    pub fn try_undo(&mut self) -> Result<bool, EditError> {
        let Some(top) = self.history.undo.last() else { return Ok(false) };
        let bytes = self.history.bytes - top.step.held_bytes() + top.step.inverse_held_bytes();
        if self.pushes_owner(self.history.undo.len() - 1, bytes) {
            return Err(EditError::HistoryFull);
        }
        Ok(self.undo())
    }

    /// [`Self::redo`], with the same check.
    pub fn try_redo(&mut self) -> Result<bool, EditError> {
        let Some(top) = self.history.redo.last() else { return Ok(false) };
        let bytes = self.history.bytes - top.step.held_bytes() + top.step.inverse_held_bytes();
        if self.pushes_owner(self.history.undo.len() + 1, bytes) {
            return Err(EditError::HistoryFull);
        }
        Ok(self.redo())
    }

    /// Reverses the last step. Returns false with nothing left to undo.
    pub fn undo(&mut self) -> bool {
        let Some(Entry { step, by }) = self.history.undo.pop() else { return false };
        self.note(|| JournalOp::Undo);
        self.history.bytes -= step.held_bytes();
        let inverse = self.apply(step);
        self.history.bytes += inverse.held_bytes();
        self.history.redo.push(Entry { step: inverse, by });
        self.trim_history();
        self.settle();
        true
    }

    /// Repeats the last undone step. Returns false with nothing left to redo.
    pub fn redo(&mut self) -> bool {
        let Some(Entry { step, by }) = self.history.redo.pop() else { return false };
        self.note(|| JournalOp::Redo);
        self.history.bytes -= step.held_bytes();
        let inverse = self.apply(step);
        self.history.bytes += inverse.held_bytes();
        self.history.undo.push(Entry { step: inverse, by });
        self.trim_history();
        self.settle();
        true
    }

    /// Whether the history coming to this many steps and bytes would push the owner's out, for an agent. Only the oldest ever go, as a step's way back depends on those after it, so an agent's own oldest steps go first simply by being oldest, and an owner's at the bottom stops it. Counts the same as [`Self::trim_history`] does.
    fn pushes_owner(&self, mut steps: usize, mut bytes: usize) -> bool {
        if self.acting != Author::Agent || self.may_push_owner {
            return false;
        }
        for entry in &self.history.undo {
            if steps <= 1 || (steps <= MAX_UNDO && bytes <= MAX_HISTORY_BYTES) {
                return false;
            }
            if entry.by == Author::Owner {
                return true;
            }
            steps -= 1;
            bytes = bytes.saturating_sub(entry.step.held_bytes());
        }
        false
    }

    fn commit(&mut self, step: Step) -> Result<(), EditError> {
        // What the step clears from redo isn't held any more.
        let held_by_redo: usize = self.history.redo.iter().map(|entry| entry.step.held_bytes()).sum();
        if self.pushes_owner(self.history.undo.len() + 1, self.history.bytes - held_by_redo + step.inverse_held_bytes()) {
            return Err(EditError::HistoryFull);
        }
        let by: AuthorName = self.acting.into();
        match &step {
            Step::Rotate { start, len, quarter_turns } => self.note(|| JournalOp::Rotate(*start, *len, *quarter_turns, by)),
            Step::Move { start, len, to } => self.note(|| JournalOp::Move(*start, *len, *to, by)),
            Step::Remove { start, len } => self.note(|| JournalOp::Delete(*start, *len, by)),
            // Only ever a PDF being added: all of it, from its first page.
            Step::Restore { start, slots } => {
                let (source, count) = (slots.first().map_or(0, |slot| slot.source), u32::try_from(slots.len()).unwrap_or(0));
                self.note(|| JournalOp::Insert(*start, source, count, by));
            }
            Step::Mark(step) => self.note(|| JournalOp::Mark(MarkOp::of(step, by))),
        }
        let inverse = self.apply(step);
        self.history.bytes += inverse.held_bytes();
        self.history.undo.push(Entry {
            step: inverse,
            by: self.acting,
        });
        for dropped in self.history.redo.drain(..) {
            self.history.bytes -= dropped.step.held_bytes();
        }
        self.trim_history();
        self.settle();
        Ok(())
    }

    /// Works out what a step changed about the list as a whole: whether it differs from the file, and whether saving rewrites it. Each is a look along the whole document, so a replay leaves them to its end, where it works both out once: two looks at 200,000 pages after each step made a long journal's replay take minutes.
    fn settle(&mut self) {
        if !self.replaying {
            self.differs = self.differs_from_the_file();
            self.rewrites = self.rewrites_the_file();
        }
    }

    /// Holds the history to its caps, oldest undo first. Anything dropped means the original can't be reached by undoing, and `is_edited` keeps saying so. A dropped undo step's effect stays in the page order, so nothing of it is lost. A dropped redo step is gone, so an owner's isn't dropped, and the steps deeper than it can't be either, since a redo step only applies to the state before it. That can't go far over the cap: the pages a list can hold to redo are the pages of its steps, which a document's length and this cap bound to [`MAX_HISTORY_BYTES`] plus one document.
    fn trim_history(&mut self) {
        while self.history.undo.len() > MAX_UNDO || (self.history.bytes > MAX_HISTORY_BYTES && self.history.undo.len() > 1) {
            let dropped = self.history.undo.remove(0);
            self.history.bytes -= dropped.step.held_bytes();
            self.history.dropped = true;
            self.history.owner_dropped |= dropped.by == Author::Owner;
        }
        while self.history.bytes > MAX_HISTORY_BYTES && self.history.redo.len() > 1 && self.history.redo[0].by == Author::Agent {
            let dropped = self.history.redo.remove(0);
            self.history.bytes -= dropped.step.held_bytes();
        }
    }

    /// What `undo` and `redo` hold now, in bytes.
    pub fn history_bytes(&self) -> usize {
        self.history.bytes
    }

    /// The position of a page, from `source`, nearest to `near`. Nearest, because the same page can appear twice once a PDF has been added twice.
    pub fn nearest_position(&self, source: u16, page: u32, near: u32) -> Option<u32> {
        let near = near as usize;
        let found = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.source == source && slot.page == page)
            .map(|(position, _)| position)
            .min_by_key(|position| position.abs_diff(near))?;
        u32::try_from(found).ok()
    }

    /// Applies a step that has already been checked, and returns its inverse.
    fn apply(&mut self, step: Step) -> Step {
        self.changes += 1;
        match step {
            Step::Rotate { start, len, quarter_turns } => {
                for slot in &mut self.slots[start as usize..(start + len) as usize] {
                    slot.quarter_turns = (slot.quarter_turns + quarter_turns) % 4;
                }
                Step::Rotate {
                    start,
                    len,
                    quarter_turns: (4 - quarter_turns) % 4,
                }
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
            Step::Mark(step) => Step::Mark(Box::new(self.marks.apply(*step))),
        }
    }

    fn check_range(&self, start: u32, len: u32) -> Result<(), EditError> {
        if len == 0 || u64::from(start) + u64::from(len) > u64::from(self.len()) {
            return Err(EditError::OutOfRange);
        }
        Ok(())
    }

    fn source_for(&mut self, path: &Path, page_count: u32, stamp: Option<FileStamp>, protected: bool) -> Result<u16, EditError> {
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
        let added = AddedSource {
            path: PathBuf::from(path).into_boxed_path(),
            page_count,
            stamp,
            kept: false,
            protected,
            holds: None,
        };
        self.note(|| JournalOp::AddSource(added.to_record()));
        self.added.push(added);
        u16::try_from(self.added.len()).map_err(|_| EditError::TooManySources)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    std::thread_local! {
        /// Looks along the whole document, on this thread, for tests that count them.
        static LOOKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    pub(super) fn count_look() {
        LOOKS.with(|looks| looks.set(looks.get() + 1));
    }

    fn looks() -> usize {
        LOOKS.with(std::cell::Cell::get)
    }

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
        edits.insert_pdf_noting(1, Path::new("/d/tide.pdf"), 2, None, false).unwrap();
        let rows: Vec<_> = edits.slots().iter().map(|s| (s.source, s.page)).collect();
        assert_eq!(rows, [(0, 0), (1, 0), (1, 1), (0, 1), (0, 2)]);
        assert_eq!(edits.added_sources().len(), 1);
        assert_eq!(edits.position_of(1), Some(3));
    }

    /// Failure: dropping the same PDF twice stores it twice, or rotation of one copy changes the other.
    #[test]
    fn the_same_pdf_dropped_twice_shares_a_source() {
        let mut edits = PageEdits::new(1);
        edits.insert_pdf_noting(1, Path::new("/d/a.pdf"), 1, None, false).unwrap();
        edits.insert_pdf_noting(2, Path::new("/d/a.pdf"), 1, None, false).unwrap();
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
        edits.insert_pdf_noting(2, Path::new("/d/x.pdf"), 4, None, false).unwrap();
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
        assert!(!edits.is_edited() && !edits.can_undo(), "{:?}", edits.history.undo);
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
        added.insert_pdf_noting(2, Path::new("/d/a.pdf"), 1, None, false).unwrap();
        assert!(added.is_edited(), "an added page is a difference");
        added.delete(2, 1).unwrap();
        assert!(!added.is_edited(), "and taking it away again isn't");
        let mut swapped = PageEdits::new(2);
        swapped.delete(1, 1).unwrap();
        swapped.insert_pdf_noting(1, Path::new("/d/same.pdf"), 1, None, false).unwrap();
        assert!(swapped.is_edited(), "a page from another file isn't this file's, whatever its number");
        // Past the cap, an odd number of swaps differs and an even number doesn't, whatever fell off.
        let mut past = PageEdits::new(2);
        for _ in 0..MAX_UNDO + 2 {
            past.move_pages(0, 1, 1).unwrap();
        }
        assert!(!past.is_edited() && !past.history_dropped(), "it's back at the file, so nothing has been lost");
        assert!(past.history.undo.len() == MAX_UNDO);
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
        assert_eq!(edits.insert_pdf_noting(9, Path::new("/d/a.pdf"), 1, None, false), Err(EditError::OutOfRange));
        assert_eq!(
            edits.insert_pdf_noting(0, Path::new("/d/a.pdf"), MAX_PAGES as u32, None, false),
            Err(EditError::TooManyPages)
        );
        assert_eq!(order(&edits), [0, 1]);
        assert!(!edits.is_edited());
    }

    /// Failure: undo is capped by steps but not bytes, so 256 steps that each remove 200,000 pages hold about 400 MiB per tab. Past the cap the oldest go, and the document says it can't get back.
    #[test]
    fn undo_history_is_capped_in_bytes_and_says_so() {
        let pages = MAX_PAGES as u32;
        let mut heavy = PageEdits::new(pages);
        for round in 0..96 {
            heavy.delete(0, pages - 1).unwrap();
            heavy.insert_pdf_noting(0, Path::new("/d/full.pdf"), pages - 1, None, false).unwrap();
            assert!(
                heavy.history_bytes() <= MAX_HISTORY_BYTES + MAX_PAGES * 8,
                "round {round}: {}",
                heavy.history_bytes()
            );
        }
        assert!(heavy.is_edited());
        while heavy.undo() {}
        assert!(heavy.is_edited(), "steps fell off, so undoing everything doesn't reach the original");
        assert!(heavy.history_bytes() <= MAX_HISTORY_BYTES + MAX_PAGES * 8, "{}", heavy.history_bytes());
        assert_eq!(heavy.history.undo.len(), 0);
    }

    /// Failure: who made a step costs something per page, or enough per step to matter across a tab's history.
    #[test]
    fn an_edit_step_stays_small() {
        assert!(std::mem::size_of::<Entry>() <= 32, "{}", std::mem::size_of::<Entry>());
    }

    fn as_agent(edits: &mut PageEdits, change: impl FnOnce(&mut PageEdits)) {
        change(&mut edits.acting(Author::Agent, false));
    }

    /// An agent at Full, whose edits may push the owner's steps out of the history.
    fn as_agent_at_full(edits: &mut PageEdits, change: impl FnOnce(&mut PageEdits)) {
        change(&mut edits.acting(Author::Agent, true));
    }

    /// **Failure: an edit that panics while it's an agent's leaves the list's next edits the agent's,** and free to push the owner's steps out: the
    /// server catches the panic and the session's lock recovers, so the person's next step would be recorded as the agent's, which an agent may then
    /// throw away.
    #[test]
    fn an_edit_that_panics_leaves_the_next_edits_the_owners() {
        let mut edits = PageEdits::new(3);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            as_agent_at_full(&mut edits, |edits| {
                edits.rotate(0, 1, 1).unwrap();
                panic!("an edit's own bug");
            });
        }));
        assert!(caught.is_err(), "the edit panicked");
        assert!(!edits.may_push_owner, "nothing may push the owner's steps out after it");
        edits.rotate(1, 1, 1).unwrap();
        assert_eq!(
            edits.history.undo.last().map(|step| step.by),
            Some(Author::Owner),
            "the next step is the owner's"
        );
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
        assert!(
            !edits.owner_steps_undone(),
            "whoever redoes it, it's still the owner's: it's just not undone now"
        );
        assert!(edits.holds_owner_edits());
        let mut only_agent = PageEdits::new(5);
        as_agent(&mut only_agent, |edits| edits.rotate(0, 1, 1).unwrap());
        only_agent.undo();
        assert!(!only_agent.holds_owner_edits() && !only_agent.owner_steps_undone());
    }

    /// Failure: past the cap, an owner's step falls off the list and the list forgets it was ever there, while its effect is still in the page order, so the owner's edit looks like an agent's to throw away. Or an agent's edit is refused when it needn't be: only the owner's steps at the bottom stop it.
    #[test]
    fn an_owners_step_past_the_cap_still_counts() {
        let mut edits = PageEdits::new(3);
        edits.rotate(0, 1, 1).unwrap();
        for _ in 0..MAX_UNDO {
            edits.move_pages(1, 1, 2).unwrap();
        }
        assert!(edits.history_dropped());
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
        assert!(
            agents.history_dropped() && !agents.holds_owner_edits(),
            "the cap dropping an agent's step is nothing of the owner's"
        );
    }

    /// Failure: a run of heavy agent steps under an owner's undone step pushes the redo list over its byte cap, so undoing them all drops the owner's step for good, with no edit that says so: an agent erases the owner's edit by undoing. The list may pass the cap by one document, and no further.
    #[test]
    fn undoing_never_drops_an_owners_undone_step() {
        let mut edits = PageEdits::new(1);
        as_agent(&mut edits, |edits| {
            // An insert holds nothing until it's undone, and a delete holds its pages until it's undone. So undoing each pair below leaves the bytes where they were, and the two inserts under them go past the cap with nothing older left to let go of.
            for _ in 0..2 {
                edits.insert_pdf_noting(0, Path::new("/d/part.pdf"), 4_999, None, false).unwrap();
            }
            for _ in 0..44 {
                edits.insert_pdf_noting(0, Path::new("/d/part.pdf"), 190_000, None, false).unwrap();
                edits.delete(0, 190_000).unwrap();
            }
        });
        edits.rotate(0, 1, 1).unwrap();
        while edits.undo() {
            assert!(edits.history_bytes() <= MAX_HISTORY_BYTES + MAX_PAGES * 8, "{}", edits.history_bytes());
        }
        assert!(edits.owner_steps_undone(), "the owner's step is still there to redo");
        assert_eq!(edits.history.redo[0].by, Author::Owner);
        while edits.redo() {}
        assert_eq!(edits.slots()[0].quarter_turns, 1, "and it comes back whole");
    }

    /// Failure: an agent pushes the owner's steps off the bottom of the history by making enough of its own, so what the owner did can no longer be undone. That's the gap in `docs/PLAN.md`: `page delete` of a page the owner rotated, then editing past the cap so the delete can't be undone either. It's refused with words saying why, the document unchanged, and it goes on working once the owner's steps aren't the ones at the bottom. At Full it's the holder's call, and an agent's own steps roll off freely.
    #[test]
    fn an_agent_cant_push_the_owners_steps_out_of_the_history() {
        let mut edits = PageEdits::new(3);
        edits.rotate(0, 1, 1).unwrap();
        edits.delete(2, 1).unwrap();
        as_agent(&mut edits, |edits| {
            for _ in 0..MAX_UNDO - 2 {
                edits.rotate(1, 1, 1).unwrap();
            }
            let before = edits.slots().to_vec();
            assert_eq!(edits.rotate(1, 1, 1), Err(EditError::HistoryFull));
            assert_eq!(edits.slots(), before, "a refused edit changes nothing");
        });
        assert!(EditError::HistoryFull.message().contains("save"));
        assert!(!edits.history_dropped());
        assert_eq!(edits.history.undo.len(), MAX_UNDO);
        // At Full, it goes through.
        as_agent_at_full(&mut edits, |edits| edits.rotate(1, 1, 1).unwrap());
        assert!(edits.history_dropped() && edits.holds_owner_edits());
        // With nothing of the owner's in it, an agent's steps simply roll.
        let mut agents = PageEdits::new(3);
        as_agent(&mut agents, |edits| {
            for _ in 0..MAX_UNDO * 2 {
                edits.rotate(0, 1, 1).unwrap();
            }
        });
        assert_eq!(agents.history.undo.len(), MAX_UNDO);
        // The owner's own steps roll too: they're never refused for it.
        let mut owners = PageEdits::new(3);
        for _ in 0..MAX_UNDO * 2 {
            owners.rotate(0, 1, 1).unwrap();
        }
        assert_eq!(owners.history.undo.len(), MAX_UNDO);
    }

    /// Failure (review B8): an agent's undo of its own insert holds that insert's pages, which goes over the byte cap and drops the owner's step at the bottom, though the same agent's next edit would have been refused for it. An undo and a redo take the same check as an edit, and at Full it's the holder's call.
    #[test]
    fn an_agents_undo_cant_push_the_owners_steps_out_either() {
        let pages = MAX_PAGES as u32;
        let mut edits = PageEdits::new(pages);
        edits.rotate(0, 1, 1).unwrap();
        let mut refused = false;
        as_agent(&mut edits, |edits| {
            for _ in 0..1000 {
                let count = edits.len() - 1;
                if edits.delete(0, count).is_err() {
                    break;
                }
                edits.insert_pdf_noting(0, Path::new("/d/full.pdf"), count, None, false).unwrap();
            }
            // The steps on top are its own inserts, and undoing one takes on all its pages.
            while edits.history.undo.len() > 1 {
                match edits.try_undo() {
                    Err(EditError::HistoryFull) => {
                        refused = true;
                        break;
                    }
                    Ok(true) => {}
                    other => panic!("{other:?}"),
                }
            }
        });
        assert!(refused, "the bytes reach the cap");
        assert_eq!(edits.history.undo[0].by, Author::Owner, "the owner's step is still at the bottom");
        assert!(!edits.history.owner_dropped);
        as_agent_at_full(&mut edits, |edits| assert_eq!(edits.try_undo(), Ok(true)));
    }

    /// Failure (review T21): a PDF of no pages added makes a step of nothing, which a replay refuses, so everything after it was lost.
    #[test]
    fn adding_nothing_is_no_step() {
        let mut edits = PageEdits::new(3);
        edits.journal_from_now();
        edits.insert_pdf_noting(0, Path::new("/d/none.pdf"), 0, None, false).unwrap();
        assert!(!edits.can_undo() && edits.journal_waiting().unwrap().1.is_empty());
    }

    /// Failure (CI run 42471845371): a replay looked along the whole document after every step, for whether it differs from the file and whether saving rewrites it, and threw both answers away, as the replay works them out once at its end. So a journal of one-page turns on a long document cost two looks at 200,000 pages a step, and `a_replay_has_a_budget` ran for minutes on a slower machine. A replay looks along the document a fixed number of times, however many steps it has, and comes to the same answers.
    #[test]
    fn a_replay_looks_along_the_document_once_not_per_step() {
        let by = AuthorName::Owner;
        let ops: Vec<JournalOp> = (0..100).map(|at| JournalOp::Rotate(at, 1, 1, by)).collect();
        let before = looks();
        let (edits, applied) = PageEdits::replay(MAX_PAGES as u32, None, &ops).unwrap();
        let looked = looks() - before;
        assert_eq!(applied, ops.len());
        assert!(looked <= 4, "{looked} looks along the document for {} steps", ops.len());
        assert!(edits.is_edited() && !edits.rewrites(), "the answers are worked out at the end");
        // And the same as a list that was looked at after every step.
        let mut live = PageEdits::new(MAX_PAGES as u32);
        for at in 0..100 {
            live.rotate(at, 1, 1).unwrap();
        }
        assert_eq!(edits.summary(), live.summary());
        assert_eq!(edits.order_mark(), live.order_mark());
    }

    /// Failure (review T18): a forged journal of a million inserts and deletes of a whole long document holds a launch for hours. A replay stops at its budget, and says how far it got.
    #[test]
    fn a_replay_has_a_budget() {
        let by = AuthorName::Owner;
        let ops: Vec<JournalOp> = std::iter::repeat_with(|| JournalOp::Rotate(0, 1, 1, by)).take(200_000).collect();
        let (_, applied) = PageEdits::replay(MAX_PAGES as u32, None, &ops).unwrap();
        // The budget counts work, not time, so where it stops is the same on any machine: each step is charged a look along the document, whatever it really took.
        let affordable = crate::record::MAX_REPLAY_WORK / (MAX_PAGES as u64 + 1);
        assert!(affordable < ops.len() as u64, "the journal must be past the budget to test it");
        assert_eq!(applied as u64, affordable);
    }

    /// Failure: the bytes of what an agent's delete would hold are forgotten, so a single huge delete pushes the owner's steps out where the count wouldn't have.
    #[test]
    fn an_agents_big_delete_counts_towards_the_owners_steps() {
        let pages = MAX_PAGES as u32;
        let mut edits = PageEdits::new(pages);
        edits.rotate(0, 1, 1).unwrap();
        let mut refused = false;
        as_agent(&mut edits, |edits| {
            for _ in 0..512 {
                let count = edits.len() - 1;
                if edits.delete(0, count) == Err(EditError::HistoryFull) {
                    refused = true;
                    break;
                }
                edits.insert_pdf_noting(0, Path::new("/d/full.pdf"), count, None, false).unwrap();
            }
        });
        assert!(refused, "the bytes reach the cap");
        assert_eq!(edits.history.undo[0].by, Author::Owner);
        assert!(!edits.history_dropped());
    }

    fn sample_ops(edits: &mut PageEdits) {
        edits.rotate(1, 2, 1).unwrap();
        as_agent(edits, |edits| edits.delete(0, 1).unwrap());
        edits
            .insert_pdf_noting(
                1,
                Path::new("/d/added.pdf"),
                3,
                Some(FileStamp {
                    size: 9,
                    mtime_ns: 8,
                    device: 7,
                    inode: 6,
                }),
                false,
            )
            .unwrap();
        edits.move_pages(0, 2, 3).unwrap();
        as_agent(edits, |edits| edits.rotate(0, 1, 3).unwrap());
        edits.undo();
        edits.undo();
        edits.redo();
    }

    /// Failure: what a journal holds doesn't bring back what was there: the order, the turns, the added file, the steps' way back, whose each is, or what's undone. Every operation goes through it, and the list that comes back behaves the same from then on, with undo and redo taken all the way both ways.
    #[test]
    fn a_journal_brings_back_the_whole_list() {
        let mut live = PageEdits::new(6);
        live.journal_from_now();
        sample_ops(&mut live);
        let (seq, ops) = live.journal_waiting().map(|(seq, ops)| (seq, ops.to_vec())).unwrap();
        assert_eq!(seq, ops.len() as u64);
        let (mut back, applied) = PageEdits::replay(6, None, &ops).unwrap();
        assert_eq!(applied, ops.len());
        assert_eq!(back.to_record(), live.to_record());
        assert_eq!(back.is_edited(), live.is_edited());
        assert_eq!(back.summary(), live.summary());
        assert_eq!(back.history_bytes(), live.history_bytes());
        while live.undo() {
            assert!(back.undo());
            assert_eq!(back.slots(), live.slots());
        }
        assert!(!back.undo());
        while live.redo() {
            assert!(back.redo());
            assert_eq!(back.slots(), live.slots());
        }
        assert!(!back.redo());
    }

    /// Failure: going through a whole record and back loses something, which is what a journal's base is once it's compacted: the same list must come out, with the rolled-off flags, the same bytes held, and each step still whoever's it was.
    #[test]
    fn a_list_survives_being_made_a_record() {
        let mut live = PageEdits::new(6);
        sample_ops(&mut live);
        for _ in 0..MAX_UNDO + 5 {
            live.rotate(0, 1, 1).unwrap();
        }
        assert!(live.history_dropped());
        let record = live.to_record();
        let mut back = PageEdits::from_record(&record).unwrap();
        assert_eq!(back.to_record(), record);
        assert_eq!(
            (back.is_edited(), back.history_bytes(), back.holds_owner_edits()),
            (live.is_edited(), live.history_bytes(), live.holds_owner_edits())
        );
        while live.undo() {
            assert!(back.undo());
            assert_eq!(back.slots(), live.slots());
        }
    }

    /// Failure: a list's runs are longer than its pages need: a document of 200,000 pages with one page turned takes the same as it did before. A run holds everything in a row.
    #[test]
    fn a_long_list_comes_to_a_few_runs() {
        let mut edits = PageEdits::new(MAX_PAGES as u32);
        edits.rotate(1000, 1, 1).unwrap();
        assert_eq!(
            edits.to_record().pages,
            vec![SlotRun(0, 0, 1000, 0), SlotRun(0, 1000, 1, 1), SlotRun(0, 1001, MAX_PAGES as u32 - 1001, 0)]
        );
    }

    /// Failure: replaying a journal panics on an operation that's out of range, or on an undo with nothing to undo. Those are what a hostile journal holds. It stops there, says how far it got, and what's left is the good part.
    #[test]
    fn a_bad_operation_ends_a_replay() {
        let by = AuthorName::Owner;
        for bad in [
            JournalOp::Rotate(9, 1, 1, by),
            JournalOp::Move(0, 1, 9, by),
            JournalOp::Delete(0, 99, by),
            JournalOp::Insert(99, 1, 1, by),
            JournalOp::Insert(0, 0, 1, by),
            JournalOp::Insert(0, 9, 1, by),
            JournalOp::Redo,
        ] {
            let ops = vec![JournalOp::Rotate(0, 1, 1, by), bad.clone(), JournalOp::Rotate(1, 1, 1, by)];
            let (edits, applied) = PageEdits::replay(3, None, &ops).unwrap();
            assert_eq!(applied, 1, "{bad:?}");
            assert_eq!(edits.slots()[0].quarter_turns, 1);
            assert_eq!(edits.slots()[1].quarter_turns, 0);
        }
    }

    /// Failure: a journal inserts more pages of an added PDF than it has, and replay takes it, so the list holds a page past the file's end: nothing can draw it, and what the list writes doesn't read back ("a page out of range"), which is what every hand-off and compaction checks. Found by the `record` fuzz target, in a journal that adds a one-page file and inserts two of its pages.
    #[test]
    fn a_journal_cant_insert_more_pages_than_a_file_has() {
        let added = crate::record::AddedRecord {
            path: crate::record::PathField::Text("/docs/added.pdf".into()),
            page_count: 1,
            file: None,
            kept: false,
            protected: false,
            holds: None,
        };
        let (agent, owner) = (AuthorName::Agent, AuthorName::Owner);
        let ops = vec![
            JournalOp::Rotate(1, 1, 1, owner),
            JournalOp::AddSource(added),
            JournalOp::Insert(3, 1, 2, agent),
            JournalOp::Delete(0, 1, agent),
            JournalOp::Move(0, 1, 2, owner),
            JournalOp::Undo,
            JournalOp::Undo,
            JournalOp::Redo,
        ];
        let (edits, applied) = PageEdits::replay(3, None, &ops).unwrap();
        assert_eq!(applied, 2, "the insert past the file's end is where it stops");
        PageEdits::from_record(&edits.to_record()).expect("what a replay gives writes what reads");
        // One that fits goes in.
        let ops = [&ops[..2], &[JournalOp::Insert(3, 1, 1, agent)]].concat();
        assert_eq!(PageEdits::replay(3, None, &ops).unwrap().1, 3);
    }

    /// Failure: an agent's insert is refused, as it would push the owner's steps out of the history, and the file it would have added stays on the list anyway, unused: it counts towards the limit on files, it's in the journal, and the next file added gets the number after it. Found by the `edits` fuzz target, whose model numbers files as they're first taken.
    #[test]
    fn a_refused_insert_leaves_no_file_behind() {
        let mut edits = PageEdits::new(2);
        edits.journal = Some(Outbox::default());
        for _ in 0..MAX_UNDO {
            edits.rotate(0, 1, 1).unwrap();
        }
        let journaled = edits.journal.as_ref().map(|outbox| (outbox.seq, outbox.ops.len()));
        edits.acting = Author::Agent;
        assert_eq!(edits.insert_pdf_noting(0, Path::new("/d/a.pdf"), 1, None, false), Err(EditError::HistoryFull));
        assert!(edits.added.is_empty(), "the refused file stayed on the list");
        assert_eq!(
            edits.journal.as_ref().map(|outbox| (outbox.seq, outbox.ops.len())),
            journaled,
            "and in the journal"
        );
        edits.acting = Author::Owner;
        edits.insert_pdf_noting(0, Path::new("/d/b.pdf"), 1, None, false).unwrap();
        assert_eq!(edits.slots()[0].source, 1, "the next file is the first");
    }

    /// Failure: the place can't be followed to the right copy of a page added twice.
    #[test]
    fn the_nearest_copy_of_a_page_is_found() {
        let mut edits = PageEdits::new(1);
        edits.insert_pdf_noting(1, Path::new("/d/a.pdf"), 1, None, false).unwrap();
        edits.insert_pdf_noting(2, Path::new("/d/b.pdf"), 1, None, false).unwrap();
        edits.insert_pdf_noting(3, Path::new("/d/a.pdf"), 1, None, false).unwrap();
        assert_eq!(edits.nearest_position(1, 0, 0), Some(1));
        assert_eq!(edits.nearest_position(1, 0, 3), Some(3));
        assert_eq!(edits.nearest_position(9, 0, 0), None);
    }
}
