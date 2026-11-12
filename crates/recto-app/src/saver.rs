//! **When the app's state goes to disk** (SPEC decision 25: by what happened, not on a timer): edits go in their journals after [`JOURNAL_QUIET_MS`] of quiet, or every
//! [`JOURNAL_LONGEST_MS`] at most during a burst; places and the like at most every [`RECORD_EVERY_MS`]; and at once when something shouldn't wait.
//!
//! These are the saver's decisions alone, as plain functions of what's pending and `now` in milliseconds of the app's [`crate::clock::Clock`] (plus one, so zero means none),
//! with no lock, no thread and no callback, so they're tested on a fake clock. `recto-ffi`'s `Saver` keeps the state in atomics, takes the clock's time, and does what these say:
//! its writer thread, the disk, and the real waits stay there. **The intent** (restructuring step R1.4): the saver becomes a state machine, `handle(input, now) -> intents`, and
//! calls these unchanged.

/// How long edits wait for a pause.
pub const JOURNAL_QUIET_MS: u64 = 2_000;
/// The longest edits wait during a burst.
pub const JOURNAL_LONGEST_MS: u64 = 5_000;
/// The least time between writes of places and the like.
pub const RECORD_EVERY_MS: u64 = 60_000;

/// What's waiting to be written, as the saver's atomics say. Times are milliseconds of the clock plus one, and zero for none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pending {
    /// The first edit not yet in its journal.
    pub first_edit: u64,
    /// The latest edit.
    pub last_edit: u64,
    /// Something shouldn't wait: the app's going to the background, or a journal was retired.
    pub urgent: bool,
    /// A place, or something else in the record, changed.
    pub view_changed: bool,
    /// When the record was last written.
    pub last_record_write: u64,
}

/// What one visit to the disk writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pass {
    pub journals: bool,
    pub record: bool,
}

/// When the edits waiting are due: a pause after the last, or the longest wait after the first, whichever's sooner.
fn edits_due(pending: Pending) -> Option<u64> {
    (pending.first_edit != 0).then(|| (pending.last_edit + JOURNAL_QUIET_MS).min(pending.first_edit + JOURNAL_LONGEST_MS))
}

/// How long until something's due, in milliseconds, zero for now, or `None` with nothing to write.
#[must_use]
pub fn next_due(pending: Pending, now: u64) -> Option<u64> {
    let mut due = edits_due(pending);
    if pending.urgent {
        due = Some(now);
    } else if pending.view_changed {
        let at = pending.last_record_write + RECORD_EVERY_MS;
        due = Some(due.map_or(at, |due| due.min(at)));
    }
    due.map(|due| due.saturating_sub(now))
}

/// What a visit to the disk at `now` writes. `pending.urgent` is the urgency the visit took for itself: it writes the journals, and the record if that changed.
#[must_use]
pub fn pass(pending: Pending, now: u64) -> Pass {
    let edits = edits_due(pending).is_some_and(|due| now >= due);
    Pass {
        journals: edits || pending.urgent,
        record: pending.view_changed && (pending.urgent || now >= pending.last_record_write + RECORD_EVERY_MS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Clock;
    use crate::testkit::FakeClock;

    fn now(clock: &FakeClock) -> u64 {
        clock.now_ms() + 1
    }

    /// Failure (SPEC decision 25, the owner's SSD rule): each edit is written as it's made, or a long burst of edits waits for its pause and is never written while it goes on, so a crash in a burst loses all of it. Edits wait for 2 s of quiet, and no more than 5 s from the first. The app tests that reach this today: `crates/recto-cli/tests/restore.rs`, through the journals found after a kill (`killed_outright_it_comes_back_with_what_was_written`, and `wait_for_journal`'s callers).
    #[test]
    fn edits_wait_for_a_pause_but_not_past_the_longest() {
        let clock = FakeClock::at(10_000);
        let first = now(&clock);
        let mut pending = Pending {
            first_edit: first,
            last_edit: first,
            ..Pending::default()
        };
        assert_eq!(next_due(pending, now(&clock)), Some(JOURNAL_QUIET_MS), "a pause after one edit");
        // A burst: an edit every second.
        for _ in 0..4 {
            clock.advance(1_000);
            pending.last_edit = now(&clock);
            assert!(!pass(pending, now(&clock)).journals, "not during the burst, before the longest");
        }
        clock.advance(1_000);
        assert_eq!(now(&clock), first + JOURNAL_LONGEST_MS);
        assert!(pass(pending, now(&clock)).journals, "the longest wait is up");
        assert_eq!(next_due(pending, now(&clock)), Some(0));
    }

    /// Failure: a place changing writes the whole record at every scroll, or never; or something urgent (the app going to the background) waits out the minute. Places wait for a minute since the record was last written; urgent goes now, journals and record both.
    #[test]
    fn places_wait_a_minute_and_urgent_goes_now() {
        let clock = FakeClock::at(0);
        let written = now(&clock);
        let pending = Pending {
            view_changed: true,
            last_record_write: written,
            ..Pending::default()
        };
        clock.advance(30_000);
        assert_eq!(next_due(pending, now(&clock)), Some(RECORD_EVERY_MS - 30_000));
        assert_eq!(pass(pending, now(&clock)), Pass::default(), "not yet");
        let urgent = Pending { urgent: true, ..pending };
        assert_eq!(next_due(urgent, now(&clock)), Some(0));
        assert_eq!(pass(urgent, now(&clock)), Pass { journals: true, record: true });
        clock.advance(30_000);
        assert_eq!(pass(pending, now(&clock)), Pass { journals: false, record: true }, "the minute's up");
    }

    /// Failure: with nothing pending the session thread is given a deadline anyway, so it wakes for nothing (AGENTS rule 1: no timer at rest); or an edit and a place changed together wait for the later of the two.
    #[test]
    fn nothing_pending_is_no_deadline_and_two_take_the_sooner() {
        assert_eq!(next_due(Pending::default(), 5_000), None);
        let both = Pending {
            first_edit: 1,
            last_edit: 1,
            view_changed: true,
            last_record_write: 1,
            ..Pending::default()
        };
        assert_eq!(next_due(both, 1), Some(JOURNAL_QUIET_MS), "the edits' pause comes before the record's minute");
    }
}
