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

/// The same when nothing has ever said which version they were made on, and nothing's been found against them.
const NEVER_KNOWN_NEXT: &str = "Tell the user: which version of the file these edits were made on isn't known, so they can't be saved, here or as a copy. The edits are kept until the user discards them.";

/// **The refusal of `asked` on a tab whose edits are `fit`,** or nothing if they fit. An edit names its page by its number in the version it was made on, so
/// until the page matcher (owner, 2026-11-13) nothing found in or written from the file as it is can be put on a page of the edited document.
pub fn refusal(fit: EditsFit, asked: Asked) -> Option<AgentError> {
    let message = match (fit, asked) {
        (EditsFit::Fit, _) => return None,
        (EditsFit::FileChanged | EditsFit::FileChangedVersionUnknown, Asked::Save) => recto_core::save::SaveError::FileChanged.message(),
        (EditsFit::FileChanged | EditsFit::FileChangedVersionUnknown, Asked::Find) => {
            "Find can't search this file: it changed on disk after its unsaved edits were made.".into()
        }
        (EditsFit::FileChanged | EditsFit::FileChangedVersionUnknown, Asked::Search) => {
            "This file can't be searched: it changed on disk after its unsaved edits were made.".into()
        }
        (EditsFit::NotThisFile, Asked::Save) => {
            "These edits don't fit this file as it is on disk, so saving them could put them on the wrong pages. Nothing was written, and your edits are kept."
                .into()
        }
        (EditsFit::NotThisFile, Asked::Find) => "Find can't search this file: its unsaved edits don't fit it.".into(),
        (EditsFit::NotThisFile, Asked::Search) => "This file can't be searched: its unsaved edits don't fit it.".into(),
        (EditsFit::VersionUnknown, Asked::Save) => {
            "Which version of this file these edits were made on isn't known, so saving them could put them on the wrong pages. Nothing was written, and your edits are kept."
                .into()
        }
        (EditsFit::VersionUnknown, Asked::Find) => "Find can't search this file: which version its unsaved edits were made on isn't known.".into(),
        (EditsFit::VersionUnknown, Asked::Search) => "This file can't be searched: which version its unsaved edits were made on isn't known.".into(),
    };
    let next = match fit {
        EditsFit::FileChanged => CHANGED_UNDER_EDITS_NEXT,
        EditsFit::FileChangedVersionUnknown => VERSION_UNKNOWN_NEXT,
        EditsFit::VersionUnknown => NEVER_KNOWN_NEXT,
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
            let unknown_version = refusal(EditsFit::VersionUnknown, asked).unwrap();
            assert_eq!(unknown_version.code, "edits_version_unknown");
            for text in [&unknown_version.message, &unknown_version.next] {
                assert!(!text.contains("changed") && !text.contains("don't fit"), "{unknown_version:?}");
            }
            assert!(unknown_version.next.contains("discard"), "{unknown_version:?}");
            let journal = refusal(EditsFit::NotThisFile, asked).unwrap();
            assert_eq!(journal.code, "edits_dont_fit");
            assert!(!journal.message.contains("changed") && !journal.next.contains("changed"), "{journal:?}");
            assert!(journal.next.contains("discard"), "{journal:?}");
        }
    }

    fn pdf(who: &str, pages: usize) -> Vec<u8> {
        let pages: Vec<recto_testpdf::Page> = (1..=pages)
            .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{who} page {page}")])))
            .collect();
        recto_testpdf::pages(&pages)
    }

    /// Failure (the window after a restart): only a tab's notice goes in the record, so a tab whose edits were made on a version its file isn't any more, and
    /// which kept that version's stamp, came back with nothing saying they don't fit: until the restore's look or read landed, `fit` said they did. Save and an
    /// agent's text were still refused by the stamp, but Find and `search` went through, skipping the tab's own pages with no reason given. Restored, and asked
    /// at once, before any look or read: each is refused with its reason.
    #[test]
    fn a_restored_tab_whose_file_changed_under_its_edits_refuses_at_once() {
        use recto_session::{Session, edit::PageEdits, record};
        use std::sync::Mutex;
        let dir = recto_testpdf::TempDir::new("recto-unfit-restart-");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("doc.pdf");
        std::fs::write(&path, pdf("Mine", 10)).unwrap();
        let mut before = Session::new(crate::DrawableLoader);
        let id = before.open(&path).unwrap();
        before.edit_pages(id, |e| e.rotate(8, 1, 1)).unwrap();
        // Another program's save, of the same length, renamed over it; the app looks and hears.
        let incoming = dir.join("doc.incoming");
        std::fs::write(&incoming, pdf("Theirs", 10)).unwrap();
        std::fs::rename(&incoming, &path).unwrap();
        before.apply_file_status(id, record::look_at_file(&path, before.tabs().own_stamp(id)));
        assert_eq!(before.notices().fit(id), EditsFit::FileChanged);
        let mut saved = before.to_record();
        let journal = "journal-00000000000000ab.log";
        saved.tabs[0].journal = Some(journal.into());
        let mut edits = PageEdits::new(10);
        edits.rotate(8, 1, 1).unwrap();
        let restored = recto_session::RestoredEdits {
            edits,
            seq: 1,
            lost_lines: false,
            rolled_off: false,
        };
        let mut after = Session::new(crate::DrawableLoader);
        after
            .restore(&saved, std::collections::HashMap::from([(journal.to_string(), restored)]))
            .unwrap();
        let access = Mutex::new(after);
        let tab = format!("#{}", id.0);
        for (words, code) in [
            (vec!["find", "Theirs"], "file_changed"),
            (vec!["search", "Theirs", "--tab", &tab], "file_changed"),
            (vec!["save", "--tab", &tab], "file_changed"),
            (vec!["text", "--page", "3", "--tab", &tab], "source_changed"),
        ] {
            let owned: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
            let command = crate::parse(&crate::table::parse_cli(&owned).unwrap().wire()).unwrap();
            let refused =
                crate::execute(&access, crate::Actor::Agent(crate::Level::Full), &command, &mut Vec::new()).expect_err(&format!("{words:?} refused at once"));
            assert_eq!(refused.code, code, "{words:?}: {refused:?}");
            assert!(!refused.message.is_empty(), "{words:?}: with its reason");
            assert!(!format!("{refused:?}").contains("Theirs page"), "{words:?}: nothing of the other version");
        }
    }
}
