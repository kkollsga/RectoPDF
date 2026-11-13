//! **When the window may be shown the first time** (owner's rule in SPEC: a window waits for its page, up to a patience, and then shows whatever is on it). One pure function, so the rule is tested with numbers and not by starting the app.
//!
//! On 2026-11-13 under CI's limits a slow start put the window up after the process was 2.6 s old, and the old rule, a cap measured from the process's start, answered yes at once: the window appeared 9 ms after it opened with no page on it. The patience is the window's own, so it counts from when the window was opened.

use std::time::Duration;

use super::{RESTORE_PATIENCE, SHOW_PATIENCE};

/// What the page needs, each answered by Rust's state when the window asks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PageChecks {
    /// The state has been read and taken in.
    pub state_read: bool,
    /// A tab is being held back while its file is read.
    pub holding_tab: bool,
    /// The session answered at once (the UI thread doesn't wait for it).
    pub session_free: bool,
    /// A tab is showing; with none (home) there is nothing to wait for.
    pub has_tab: bool,
    pub on_place: bool,
    pub showable: bool,
}

impl PageChecks {
    /// The page is as it should be, or there is no page to wait for.
    #[must_use]
    pub fn pass(self) -> bool {
        self.state_read && !self.holding_tab && self.session_free && (!self.has_tab || self.on_place || self.showable)
    }
}

/// What the rule says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    Wait,
    /// The page checks passed.
    Ready,
    /// They didn't, and the cap has run out.
    Cap,
}

impl Show {
    /// The word the record keeps.
    #[must_use]
    pub fn why(self) -> &'static str {
        match self {
            Self::Wait => "wait",
            Self::Ready => "ready",
            Self::Cap => "cap",
        }
    }
}

/// **The rule.** Shown when the page is right (`Ready`), or when the cap has run out (`Cap`): [`SHOW_PATIENCE`] since the window was opened, or, before one has been opened, the old backstop of [`SHOW_PATIENCE`] + [`RESTORE_PATIENCE`] since the process started.
#[must_use]
pub fn decide(since_window_opened: Option<Duration>, since_process_start: Duration, checks: PageChecks) -> Show {
    if checks.pass() {
        return Show::Ready;
    }
    let capped = match since_window_opened {
        Some(opened) => opened >= SHOW_PATIENCE,
        None => since_process_start >= SHOW_PATIENCE + RESTORE_PATIENCE,
    };
    if capped { Show::Cap } else { Show::Wait }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn not_covered() -> PageChecks {
        PageChecks {
            state_read: true,
            session_free: true,
            has_tab: true,
            ..PageChecks::default()
        }
    }

    fn covered() -> PageChecks {
        PageChecks {
            showable: true,
            ..not_covered()
        }
    }

    /// Failure (CI, 2026-11-13, under `--like-ci`): on a slow start the window opens when the process is 2.5 s old, and the process-age cap answers yes at once, so it shows 9 ms after opening with no page on it. The window waits for its page, and shows as `cap` only 1.2 s after it opened.
    #[test]
    fn a_window_opened_late_still_waits_for_its_page() {
        assert_eq!(decide(Some(ms(9)), ms(2_509), not_covered()), Show::Wait);
        assert_eq!(decide(Some(ms(1_199)), ms(3_699), not_covered()), Show::Wait);
        assert_eq!(decide(Some(ms(1_200)), ms(3_700), not_covered()), Show::Cap);
    }

    /// Failure: the page is covered and the window waits anyway, or shows as a cap.
    #[test]
    fn a_covered_page_shows_at_once_as_ready() {
        assert_eq!(decide(Some(ms(9)), ms(2_509), covered()), Show::Ready);
        assert_eq!(
            decide(Some(ms(5_000)), ms(9_000), covered()),
            Show::Ready,
            "ready even when the cap has run out too"
        );
    }

    /// Failure: a window opened early changes from how it was: it waits, then shows at the same time as before.
    #[test]
    fn a_window_opened_early_behaves_as_it_did() {
        assert_eq!(decide(Some(ms(100)), ms(300), not_covered()), Show::Wait);
        assert_eq!(decide(Some(ms(1_200)), ms(1_400), not_covered()), Show::Cap);
    }

    /// Failure: before any window has been opened the backstop is gone and something shows at once, or never.
    #[test]
    fn with_no_window_opened_the_old_cap_stands() {
        assert_eq!(decide(None, ms(2_199), not_covered()), Show::Wait);
        assert_eq!(decide(None, ms(2_200), not_covered()), Show::Cap);
    }

    /// Failure: a part of the page check is skipped. No tab to show is ready; the state unread, a held tab or a busy session is not.
    #[test]
    fn each_page_check_holds_the_window() {
        let home = PageChecks {
            has_tab: false,
            ..not_covered()
        };
        assert!(home.pass());
        assert!(
            !PageChecks {
                state_read: false,
                ..covered()
            }
            .pass()
        );
        assert!(
            !PageChecks {
                holding_tab: true,
                ..covered()
            }
            .pass()
        );
        assert!(
            !PageChecks {
                session_free: false,
                ..covered()
            }
            .pass()
        );
        assert!(
            PageChecks {
                on_place: true,
                ..not_covered()
            }
            .pass()
        );
    }
}
