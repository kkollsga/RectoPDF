//! **What's said when a tab's unsaved edits don't fit its file** ([`recto_session::Notices::fit`]): Save, Save As, Find and `search` are refused, in words that
//! say what's true of how they don't fit, and with the one way on there is. The same for the person and an agent: the app's Find and save show these words.

use crate::{AgentError, exit};
use recto_session::EditsFit;

/// What was asked of a tab whose edits don't fit its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Save or Save As: the edits would land on the other version's pages, in this file or a copy.
    Save,
    /// The person's Find or an agent's `find`.
    Find,
    /// An agent's `search`.
    Search,
}

/// What an agent's told to do when the version the edits were made on is known: for its save, and for its Find.
pub(crate) const CHANGED_UNDER_EDITS_NEXT: &str = "Tell the user: the file changed on disk after these edits were made, so they can't be saved, here or as a copy. The edits are kept. The user can put the earlier version back, or discard them.";

/// The same when nothing names the version they were made on, so no version put back can be known to be theirs.
const VERSION_UNKNOWN_NEXT: &str = "Tell the user: the file changed on disk after these edits were made, so they can't be saved, here or as a copy. Which version they were made on isn't known, so putting a version back won't make them fit. The edits are kept until the user discards them.";

/// The same when the file isn't known to have changed at all: the edits (their journal) don't fit it.
const NOT_THIS_FILE_NEXT: &str = "Tell the user: these unsaved edits don't fit the file on disk, so they can't be saved, here or as a copy. The edits are kept until the user discards them.";

/// **The refusal of `asked` on a tab whose edits are `fit`,** or nothing if they fit. An edit names its page by its number in the version it was made on, so
/// until the page matcher (owner, 2026-11-13) nothing found in or written from the file as it is can be put on a page of the edited document.
pub fn refusal(fit: EditsFit, asked: Asked) -> Option<AgentError> {
    let changed = match fit {
        EditsFit::Fit => return None,
        EditsFit::FileChanged | EditsFit::FileChangedVersionUnknown => true,
        EditsFit::NotThisFile => false,
    };
    let message = match (asked, changed) {
        (Asked::Save, true) => recto_core::save::SaveError::FileChanged.message(),
        (Asked::Save, false) => {
            "These edits don't fit this file as it is on disk, so saving them could put them on the wrong pages. Nothing was written, and your edits are kept."
                .into()
        }
        (Asked::Find, true) => "Find can't search this file: it changed on disk after its unsaved edits were made.".into(),
        (Asked::Find, false) => "Find can't search this file: its unsaved edits don't fit it.".into(),
        (Asked::Search, true) => "This file can't be searched: it changed on disk after its unsaved edits were made.".into(),
        (Asked::Search, false) => "This file can't be searched: its unsaved edits don't fit it.".into(),
    };
    let next = match fit {
        EditsFit::FileChanged => CHANGED_UNDER_EDITS_NEXT,
        EditsFit::FileChangedVersionUnknown => VERSION_UNKNOWN_NEXT,
        _ => NOT_THIS_FILE_NEXT,
    };
    let code = fit.said().map_or("file_changed", |said| said.code);
    Some(AgentError::new(code, message, exit::FAILED).then(next))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a refusal says the file changed when only the edits' journal doesn't fit it, or tells the agent the earlier version can be put back where
    /// nothing names it, so the user is sent after a version that can't make the edits fit. Each state says its own way on, and only the state where the file
    /// changed says it did.
    #[test]
    fn each_way_the_edits_dont_fit_says_what_is_true_and_its_one_way_on() {
        for asked in [Asked::Save, Asked::Find, Asked::Search] {
            assert!(refusal(EditsFit::Fit, asked).is_none());
            let known = refusal(EditsFit::FileChanged, asked).unwrap();
            assert_eq!(known.code, "file_changed");
            assert!(
                known.message.contains("changed on disk") && known.next.contains("earlier version back"),
                "{known:?}"
            );
            let unknown = refusal(EditsFit::FileChangedVersionUnknown, asked).unwrap();
            assert_eq!((unknown.code, &unknown.message), (known.code, &known.message), "the same of the file");
            assert!(
                !unknown.next.contains("earlier version back") && unknown.next.contains("discard"),
                "{unknown:?}"
            );
            let journal = refusal(EditsFit::NotThisFile, asked).unwrap();
            assert_eq!(journal.code, "edits_dont_fit");
            assert!(!journal.message.contains("changed") && !journal.next.contains("changed"), "{journal:?}");
            assert!(journal.next.contains("discard"), "{journal:?}");
        }
    }
}
