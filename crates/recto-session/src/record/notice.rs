//! **What's said about a tab's file,** as a code and the words a person reads: moved, changed, gone, and what became of its edits. Moved out of
//! `record.rs` unchanged when the page matcher added the notice for edits that followed their pages.

use super::PathField;
use serde::{Deserialize, Serialize};

/// What's said about a tab's file. The words are in [`FileNotice::message`], not the record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileNotice {
    /// It moved, and the tab followed it.
    Moved { from: PathField },
    /// It isn't what it was. Any edits are kept, and nothing has been written to it.
    Changed,
    /// It isn't there.
    Missing,
    /// It needs a password, which is never kept.
    NeedsPassword,
    /// Some or all of its journal couldn't be read. What could be is here, and the file's been kept to one side.
    EditsUnreadable,
    /// A record had two tabs on this one file, both with edits. A PDF is open once, so this tab's are here, and the other's are kept to one side.
    EditsNotMerged,
    /// Some of its history had to go, as the pages it needed couldn't be kept within the limits, or aren't there now. Undo stops short of them, rather than undoing wrongly.
    HistoryRolledOff,
    /// It was changed by another program, and every unsaved edit followed its page into the new version (the page matcher, `crate::version_change`).
    /// **Never written into a record** (`restore.rs`): an app from before it can't read it, and would take the whole record for damaged.
    EditsFollowed {
        /// How many edits followed.
        edits: u32,
    },
}

impl FileNotice {
    /// A stable code for agents and tests.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Moved { .. } => "file_moved",
            Self::Changed => "file_changed",
            Self::Missing => "file_missing",
            Self::NeedsPassword => "password_again",
            Self::EditsUnreadable => "edits_unreadable",
            Self::EditsNotMerged => "edits_not_merged",
            Self::HistoryRolledOff => "history_rolled_off",
            Self::EditsFollowed { .. } => "edits_followed",
        }
    }

    /// A plain sentence for the reader. A tab's name and path come from outside, so there's none of either in here.
    pub fn message(&self, has_edits: bool) -> &'static str {
        match (self, has_edits) {
            (Self::Moved { .. }, true) => {
                "This file was moved, and the tab followed it. Your unsaved edits are still here, and nothing has been written to the file."
            }
            (Self::Moved { .. }, false) => "This file was moved, and the tab followed it.",
            (Self::Changed, true) => {
                "This file changed on disk. Your unsaved edits are kept, but they were made on the earlier version and can't be saved onto this one, here or as a copy. Nothing has been written to the file."
            }
            (Self::Changed, false) => "This file changed on disk since it was last read here, so this is the new version.",
            (Self::Missing, true) => {
                "This file can't be found: it may have been moved or deleted, or its disk may not be connected. Your unsaved edits are still here, and this tab keeps them until you discard them."
            }
            (Self::Missing, false) => "This file can't be found: it may have been moved or deleted, or its disk may not be connected.",
            (Self::NeedsPassword, _) => "This PDF needs its password again. Passwords aren't kept between launches.",
            (Self::HistoryRolledOff, _) => {
                "Undo can't go back as far as it could: pages it would need couldn't be kept within the history's limits, or aren't there now. Nothing in this document has changed."
            }
            (Self::EditsNotMerged, _) => {
                "This PDF was open in two tabs with different unsaved edits, which can't be merged. These are one tab's, and the other's have been kept in the state folder. Nothing has been written to the file."
            }
            // The owner's words, 2026-11-15.
            (Self::EditsFollowed { .. }, _) => {
                "This file was changed by another program. Your unsaved edits were moved onto the same pages of the new version."
            }
            (Self::EditsUnreadable, _) => {
                "Some of this tab's unsaved edits couldn't be read back, so it shows what could be. What couldn't has been kept in the state folder, and nothing has been written to the file."
            }
        }
    }
}
