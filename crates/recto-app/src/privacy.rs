//! **What the app says about privacy, and where the statement is.** One quiet line on the empty window, never over a page, about the one thing that leaves the Mac by itself: the daily update check (SPEC §1's "Never", decision 24). The same fact under the *Update automatically* switch, as the check runs whichever way it's set. And a link in Settings to `PRIVACY.md`. No first-run sheet, banner or flag to remember: the empty window is what a first launch shows, so the line is there to read before the first check, which waits a minute after launch, and it costs no state. Wording only: the facts are `recto_update`'s, and the tests hold the words to them.

/// The empty window's line, for a build that checks by itself. A local build checks only when asked, so it says nothing.
#[must_use]
pub fn home_note(checks_by_itself: bool) -> Option<&'static str> {
    checks_by_itself.then_some("Once a day, RectoPDF asks GitHub whether there's a new version. Nothing about your documents is sent.")
}

/// What follows the switch's own hint: the check doesn't stop when the switch is off, and what happens instead.
pub const CHECK_EITHER_WAY: &str = "Off, it still asks GitHub once a day and offers a new version in the RectoPDF menu.";

/// The Settings link's words.
pub const STATEMENT_TITLE: &str = "What RectoPDF keeps and sends";

/// Where the statement is read: the repository's own copy, which a release is built from.
#[must_use]
pub fn statement_url() -> String {
    format!("{}/blob/main/PRIVACY.md", recto_update::REPO.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_agent::Level;
    use recto_index::{corpus, generated};
    use recto_update::rules::CHECK_EVERY_MS;
    use recto_update::source;

    /// Failure: the line tells the reader something the updater doesn't do: a check other than daily, a server other than GitHub, or a check from a build that never checks by itself. Or a release says nothing before its first check.
    #[test]
    fn the_home_note_says_what_the_updater_does() {
        assert_eq!(CHECK_EVERY_MS, 24 * 60 * 60 * 1000, "the note says once a day");
        let release = source::resolve(true, false, None, None);
        assert!(release.checks_by_itself());
        assert!(release.repo.starts_with("https://github.com/"), "the note names GitHub: {}", release.repo);
        let note = home_note(release.checks_by_itself()).expect("a release says it checks");
        assert!(note.contains("Once a day") && note.contains("GitHub"), "{note}");
        assert!(note.contains("Nothing about your documents"), "{note}");
        let local = source::resolve(false, false, None, None);
        assert!(!local.checks_by_itself());
        assert_eq!(home_note(local.checks_by_itself()), None, "a local build checks only when asked");
        assert!(CHECK_EITHER_WAY.contains("once a day") && CHECK_EITHER_WAY.contains("GitHub"));
    }

    /// Failure: Settings links to a statement that isn't in the repository, or somewhere other than this project.
    #[test]
    fn the_statement_is_where_the_link_goes() {
        let url = statement_url();
        assert_eq!(url, format!("{}/blob/main/PRIVACY.md", recto_update::REPO));
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../PRIVACY.md");
        let text = std::fs::read_to_string(&file).expect("PRIVACY.md at the repository's root");
        assert!(text.starts_with("# Privacy"), "{}", file.display());
    }

    /// Failure: the statement promises what the code no longer does: a level allowing other than what `recto permission show` says, or numbers kept that have since changed.
    #[test]
    fn the_statement_says_what_the_code_does() {
        let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../PRIVACY.md")).unwrap();
        for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
            assert!(
                text.contains(&format!("| {} | {} |", level.label(), level.allows())),
                "{level:?}'s row is `Level::allows`"
            );
        }
        let gib = 1024 * 1024 * 1024;
        assert_eq!(
            (corpus::MAX_AGE.as_secs() / 86_400, corpus::MAX_TOTAL_BYTES / gib),
            (90, 2),
            "page text: 90 days, within 2 GiB"
        );
        assert_eq!(
            (generated::MAX_AGE.as_secs() / 86_400, generated::MAX_TOTAL_BYTES / (1024 * 1024)),
            (90, 32),
            "generated contents"
        );
        assert_eq!(recto_session::MAX_RECENT, 20, "the 20 files opened last");
        assert_eq!(recto_core::save::BACKUP_DAYS, 30, "one backup, 30 days");
        assert_eq!(recto_core::save::MAX_BACKUP_BYTES, gib, "within 1 GiB");
        assert_eq!(recto_update::updater::KEEP_RECORD_COPY_MS, 7 * 86_400_000, "a copy of the session for a week");
        assert_eq!(recto_control::log::MAX_LOG_BYTES * 6, 6 * 1024 * 1024, "the activity log, at most 6 MiB");
        for words in [
            "90 days after it was last used, within 2 GiB",
            "90 days, within 32 MiB",
            "the 20 files you opened last",
            "kept for a week",
            "at most 6 MiB",
        ] {
            assert!(text.contains(words), "{words}");
        }
    }
}
