//! **A copy that went back to an older app and came forward again** (`docs/RELEASING.md`, a bad release; `tools/release/check-carry.sh`, runs C to
//! F). An app from before highlights (0.1.0) leaves a journal of a newer version (one with highlights, [`record::MARKED_JOURNAL_VERSION`]) alone,
//! byte for byte, and writes the record with that tab naming no journal and saying `edits_unreadable`, which was so for it. Coming forward, the
//! journal is read whole and claimed by its file (the store's rule 2), so that notice isn't so any more.

use recto_session::record::{self, FileNotice, JournalHeader, TabEntry};

/// **A record's "couldn't be read" against the journal as it reads now** (`fault:D.notice.edits_unreadable`, found by `check-carry.sh` on
/// 2026-11-15). The tab named no journal, said its edits couldn't be read, and has now claimed one that read whole, of a version an app reading only
/// [`record::CURRENT_VERSION`]'s journals leaves alone: that app's words about this journal, and not so here. The caller says it again when what's
/// claimed has lines that couldn't be used.
///
/// Kept, as this app's own and still so: a record naming the journal it says this of (one that had lines it couldn't use, written clean by `claim`,
/// said until the person has seen it), and one claiming a journal of the version every app reads (a list started after a journal of this tab's
/// couldn't be read, its record not yet written: what couldn't be read is kept beside it, as the notice says).
pub(super) fn unreadable_answered(tab: &mut TabEntry, named_it: bool, header: &JournalHeader) {
    if !named_it && header.version > record::CURRENT_VERSION && matches!(tab.notice, Some(FileNotice::EditsUnreadable)) {
        tab.notice = None;
    }
}

#[cfg(test)]
mod tests;
