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
        self.held_by().next().is_none()
    }

    /// **The checks that hold the window,** by name, none when they [`Self::pass`]: what the record of the first showing says held it.
    pub fn held_by(self) -> impl Iterator<Item = &'static str> {
        [
            (!self.state_read, "state_not_read"),
            (self.holding_tab, "holding_tab"),
            (!self.session_free, "session_busy"),
            (self.has_tab && !self.on_place && !self.showable, "page_not_ready"),
        ]
        .into_iter()
        .filter_map(|(holds, name)| holds.then_some(name))
    }

    /// The six checks as bits of one byte, in the order of the fields, for an ask kept in the app's state.
    const fn bits(self) -> u8 {
        self.state_read as u8
            | (self.holding_tab as u8) << 1
            | (self.session_free as u8) << 2
            | (self.has_tab as u8) << 3
            | (self.on_place as u8) << 4
            | (self.showable as u8) << 5
    }

    /// The checks [`Self::bits`] made a byte of.
    const fn from_bits(bits: u8) -> Self {
        Self {
            state_read: bits & 1 != 0,
            holding_tab: bits & 1 << 1 != 0,
            session_free: bits & 1 << 2 != 0,
            has_tab: bits & 1 << 3 != 0,
            on_place: bits & 1 << 4 != 0,
            showable: bits & 1 << 5 != 0,
        }
    }
}

/// **One ask of the page checks,** as the rule saw it: when (since the window was put up), the checks, and what the rule said. Eight bytes, as the app's
/// state keeps two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asked {
    /// Microseconds since the window was put up, at most `u32::MAX`; 0 with none put up.
    pub micros: u32,
    checks: u8,
    pub said: Show,
}

impl Asked {
    /// The page checks as they were.
    #[must_use]
    pub const fn checks(self) -> PageChecks {
        PageChecks::from_bits(self.checks)
    }
}

const _: () = assert!(size_of::<Option<Asked>>() <= 8, "an ask the app's state keeps grew");

/// **A window's asks before its first showing:** how many, the last (the one that decided, when it said yes), and the last that said wait. What the record
/// says the decision saw, where the checks gathered after the show's layout say what that layout made of the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Asks {
    pub count: u32,
    pub last: Option<Asked>,
    pub last_wait: Option<Asked>,
}

impl Asks {
    /// An ask, `since` the window was put up.
    pub fn note(&mut self, since: Option<Duration>, checks: PageChecks, said: Show) {
        let micros = since.map_or(0, |since| u32::try_from(since.as_micros()).unwrap_or(u32::MAX));
        let asked = Asked {
            micros,
            checks: checks.bits(),
            said,
        };
        self.count = self.count.saturating_add(1);
        self.last = Some(asked);
        if said == Show::Wait {
            self.last_wait = Some(asked);
        }
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

    /// Failure: the record names a check that didn't hold the window, or leaves out one that did, so a show by patience is blamed on the wrong thing.
    #[test]
    fn what_held_the_window_is_named() {
        let held = |checks: PageChecks| checks.held_by().collect::<Vec<_>>();
        assert_eq!(held(covered()), Vec::<&str>::new());
        assert_eq!(held(not_covered()), ["page_not_ready"]);
        assert_eq!(
            held(PageChecks {
                session_free: false,
                has_tab: false,
                ..not_covered()
            }),
            ["session_busy"],
            "no tab's page to wait for"
        );
        assert_eq!(held(PageChecks::default()), ["state_not_read", "session_busy"]);
        assert_eq!(
            held(PageChecks {
                holding_tab: true,
                ..covered()
            }),
            ["holding_tab"]
        );
    }

    /// Failure: the last wait is lost when the deciding ask says yes, so the record can't say what held the window till then.
    #[test]
    fn the_asks_keep_the_last_and_the_last_wait() {
        let mut asks = Asks::default();
        asks.note(Some(ms(10)), not_covered(), Show::Wait);
        asks.note(Some(ms(400)), not_covered(), Show::Wait);
        asks.note(Some(ms(900)), covered(), Show::Ready);
        assert_eq!(asks.count, 3);
        assert_eq!(asks.last.map(|asked| (asked.micros, asked.said)), Some((900_000, Show::Ready)));
        assert_eq!(asks.last_wait.map(|asked| (asked.micros, asked.checks())), Some((400_000, not_covered())));
    }

    /// Failure: a check is kept under another's bit, so the record says the wrong one held the window.
    #[test]
    fn each_check_keeps_its_own_bit() {
        for at in 0..6 {
            let checks = PageChecks::from_bits(1 << at);
            assert_eq!(checks.bits(), 1 << at);
            let set = [
                checks.state_read,
                checks.holding_tab,
                checks.session_free,
                checks.has_tab,
                checks.on_place,
                checks.showable,
            ];
            assert_eq!(set.iter().filter(|set| **set).count(), 1, "{checks:?}");
            assert!(set[at], "{checks:?}");
        }
    }
}
