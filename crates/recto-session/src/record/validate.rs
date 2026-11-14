//! What a record's edits must be before a list is made of them, moved out of `record.rs` unchanged (R4.3), so the record had room for highlights.

use super::*;

fn runs_slots(runs: &[SlotRun]) -> u64 {
    runs.iter().map(|run| u64::from(run.2)).sum()
}

/// Whether every run is a real range of a real file: as many pages as it says that file has, not just as many as fit in a number.
fn runs_fit(runs: &[SlotRun], file_pages: u32, added: &[AddedRecord]) -> bool {
    runs.iter().all(|run| {
        let length = if run.0 == 0 {
            Some(file_pages)
        } else {
            added.get(usize::from(run.0) - 1).map(|source| source.page_count)
        };
        run.2 > 0 && run.3 < 4 && length.is_some_and(|length| u64::from(run.1) + u64::from(run.2) <= u64::from(length))
    })
}

/// Follows a list of steps, newest first, from a document this long, for the length alone. What's checked is exactly what `PageEdits::apply` relies on having been checked, so a step that passes can't index out of range there.
fn steps_fit(steps: &[EntryRecord], mut length: u64, file_pages: u32, added: &[AddedRecord]) -> Result<(), RecordError> {
    let bad = RecordError::Damaged;
    for entry in steps.iter().rev() {
        match &entry.step {
            StepRecord::Rotate { start, len, quarter_turns } => {
                if *len == 0 || u64::from(*start) + u64::from(*len) > length || *quarter_turns > 3 {
                    return Err(bad("a step out of range"));
                }
            }
            StepRecord::Move { start, len, to } => {
                if *len == 0 || u64::from(*start) + u64::from(*len) > length || u64::from(*to) + u64::from(*len) > length {
                    return Err(bad("a step out of range"));
                }
            }
            StepRecord::Remove { start, len } => {
                if u64::from(*start) + u64::from(*len) > length {
                    return Err(bad("a step out of range"));
                }
                length -= u64::from(*len);
            }
            StepRecord::Restore { start, runs } => {
                if !runs_fit(runs, file_pages, added) || u64::from(*start) > length {
                    return Err(bad("a step out of range"));
                }
                length += runs_slots(runs);
                if length > MAX_PAGES as u64 {
                    return Err(bad("too many pages"));
                }
            }
        }
    }
    Ok(())
}

/// Everything `PageEdits` relies on, checked before any run is expanded, as a run of a few bytes can claim billions of pages.
pub fn validate_edits(edits: &EditsRecord) -> Result<(), RecordError> {
    let bad = RecordError::Damaged;
    let sources = edits.added.len();
    if sources > MAX_SOURCES || edits.undo.len() > MAX_JOURNAL_STEPS || edits.redo.len() > MAX_JOURNAL_STEPS {
        return Err(bad("too long a list"));
    }
    if !runs_fit(&edits.pages, edits.file_pages, &edits.added) {
        return Err(bad("a page out of range"));
    }
    let length = runs_slots(&edits.pages);
    if length == 0 && edits.file_pages > 0 {
        return Err(bad("a document with no pages"));
    }
    if length > MAX_PAGES as u64 || edits.file_pages as usize > MAX_PAGES {
        return Err(bad("too many pages"));
    }
    let held = edits.undo.iter().chain(&edits.redo).fold(0u64, |sum, entry| match &entry.step {
        StepRecord::Restore { runs, .. } => sum.saturating_add(runs_slots(runs)),
        _ => sum,
    });
    if held > MAX_EXPANDED_SLOTS {
        return Err(bad("too much history"));
    }
    steps_fit(&edits.undo, length, edits.file_pages, &edits.added)?;
    steps_fit(&edits.redo, length, edits.file_pages, &edits.added)?;
    for added in &edits.added {
        if !added_fits(added) {
            return Err(bad("an added file"));
        }
    }
    Ok(())
}

/// Whether an added file's a possible one: a path that fits, a length that does, and a kept one named as those are.
pub fn added_fits(added: &AddedRecord) -> bool {
    added.path.fits()
        && added.page_count as usize <= MAX_PAGES
        && (!added.kept
            || added
                .path
                .to_path()
                .is_some_and(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(is_kept_name)))
}
