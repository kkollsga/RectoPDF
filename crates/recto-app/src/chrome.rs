//! **The window's marks that come and go** (SPEC section 3). The zoom level in the top right and the page number in the lower right show when they change and fade 3 s after their last change (owner, 2026-11-08: "Zoom level should auto hid 3s after last zoom change, and similar behaviour for page number"). With the sidebar hidden, the window's buttons and the sidebar's button beside them fade, and come back while the pointer is over their corner (owner, the same day: "When sidebar is hidden, I want the traffic light buttons and sidebar toggle to autohide. To show them again you hover above them").
//!
//! What shows, and until when, is decided here. The host tells what happened (a change, the pointer coming or going, a field open over a mark), draws what [`ChromeLook`] says with a fade of [`FADE_SECONDS`], and arms one timer for `next_change_in_ms`. Nothing runs between changes. The time is the [`Clock`]'s, so a test moves it.

use crate::clock::{Clock, SystemClock};
use crate::lock;
use std::sync::{Arc, Mutex};

/// How long a mark stays after its last change.
pub const MARK_SHOWN_MS: u64 = 3_000;
/// How long a mark takes to fade in or out. Reduce Motion makes it at once, which is the host's to know.
pub const FADE_SECONDS: f64 = 0.25;

/// A mark that comes and goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeMark {
    /// The zoom level, in the top right.
    Zoom,
    /// The page number, in the lower right.
    Place,
    /// The window's buttons and the sidebar's button beside them, in the top left.
    WindowButtons,
}

/// What shows now, and when that next changes with nothing else happening.
#[derive(Clone, Debug, PartialEq)]
pub struct ChromeLook {
    pub zoom_shown: bool,
    pub place_shown: bool,
    pub window_buttons_shown: bool,
    pub fade_seconds: f64,
    /// When to ask again: the soonest a mark's time runs out. None while nothing will change by itself.
    pub next_change_in_ms: Option<u64>,
}

/// What's happened to the marks, by the clock's milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    zoom_changed: Option<u64>,
    place_changed: Option<u64>,
    over_zoom: bool,
    over_place: bool,
    over_buttons: bool,
    held_zoom: bool,
    held_place: bool,
}

impl Marks {
    /// The mark changed: the zoom level's words, or the page number's, which a scroll by the reader counts as too. The window's buttons don't change.
    pub fn changed(&mut self, mark: ChromeMark, now: u64) {
        match mark {
            ChromeMark::Zoom => self.zoom_changed = Some(now),
            ChromeMark::Place => self.place_changed = Some(now),
            ChromeMark::WindowButtons => {}
        }
    }

    /// The pointer came over a mark's place, or left it. Leaving counts as a change, so it stays its 3 s after.
    pub fn pointer(&mut self, mark: ChromeMark, inside: bool, now: u64) {
        let over = match mark {
            ChromeMark::Zoom => &mut self.over_zoom,
            ChromeMark::Place => &mut self.over_place,
            ChromeMark::WindowButtons => &mut self.over_buttons,
        };
        let left = *over && !inside;
        *over = inside;
        if left {
            self.changed(mark, now);
        }
    }

    /// A field is open over the mark, the zoom level's or the go-to page's: it shows while it's being typed in, and its 3 s start when it closes.
    pub fn held(&mut self, mark: ChromeMark, held: bool, now: u64) {
        let was = match mark {
            ChromeMark::Zoom => std::mem::replace(&mut self.held_zoom, held),
            ChromeMark::Place => std::mem::replace(&mut self.held_place, held),
            ChromeMark::WindowButtons => return,
        };
        if was && !held {
            self.changed(mark, now);
        }
    }

    /// What shows at `now`, with the sidebar hidden or not.
    pub fn look(&self, now: u64, sidebar_hidden: bool) -> ChromeLook {
        let lasting = |at: Option<u64>| at.map(|at| at.saturating_add(MARK_SHOWN_MS)).filter(|until| *until > now);
        let zoom_until = lasting(self.zoom_changed).filter(|_| !self.over_zoom && !self.held_zoom);
        let place_until = lasting(self.place_changed).filter(|_| !self.over_place && !self.held_place);
        ChromeLook {
            zoom_shown: self.over_zoom || self.held_zoom || zoom_until.is_some(),
            place_shown: self.over_place || self.held_place || place_until.is_some(),
            window_buttons_shown: !sidebar_hidden || self.over_buttons,
            fade_seconds: FADE_SECONDS,
            next_change_in_ms: [zoom_until, place_until].into_iter().flatten().min().map(|until| until - now),
        }
    }
}

/// The marks and the clock they go by.
pub struct Chrome {
    marks: Mutex<Marks>,
    clock: Arc<dyn Clock>,
}

impl Default for Chrome {
    fn default() -> Self {
        Self::new(Arc::new(SystemClock::new()))
    }
}

impl Chrome {
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            marks: Mutex::new(Marks::default()),
            clock,
        }
    }

    /// See [`Marks::changed`].
    pub fn changed(&self, mark: ChromeMark) {
        lock(&self.marks).changed(mark, self.clock.now_ms());
    }

    /// See [`Marks::pointer`].
    pub fn pointer(&self, mark: ChromeMark, inside: bool) {
        lock(&self.marks).pointer(mark, inside, self.clock.now_ms());
    }

    /// See [`Marks::held`].
    pub fn held(&self, mark: ChromeMark, held: bool) {
        lock(&self.marks).held(mark, held, self.clock.now_ms());
    }

    /// What shows now. Cheap: a lock and a comparison or two.
    pub fn look(&self, sidebar_hidden: bool) -> ChromeLook {
        lock(&self.marks).look(self.clock.now_ms(), sidebar_hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the zoom level or the page number stays up for good, goes before 3 s, or comes back by itself; or the host isn't told when to look again, so it fades late or never. Owner, 2026-11-08: "Zoom level should auto hid 3s after last zoom change, and similar behaviour for page number".
    #[test]
    fn a_mark_shows_for_three_seconds_after_its_last_change() {
        let mut marks = Marks::default();
        let look = marks.look(0, false);
        assert!(
            !look.zoom_shown && !look.place_shown && look.next_change_in_ms.is_none(),
            "nothing's changed yet: {look:?}"
        );
        marks.changed(ChromeMark::Zoom, 1_000);
        let look = marks.look(1_000, false);
        assert!(look.zoom_shown && !look.place_shown, "{look:?}");
        assert_eq!(look.next_change_in_ms, Some(MARK_SHOWN_MS));
        // A second change starts the 3 s again.
        marks.changed(ChromeMark::Zoom, 2_500);
        assert!(marks.look(1_000 + MARK_SHOWN_MS, false).zoom_shown, "counted from the last change");
        assert_eq!(marks.look(4_000, false).next_change_in_ms, Some(1_500));
        let gone = marks.look(2_500 + MARK_SHOWN_MS, false);
        assert!(!gone.zoom_shown && gone.next_change_in_ms.is_none(), "{gone:?}");
        // Each mark has its own time.
        marks.changed(ChromeMark::Place, 10_000);
        marks.changed(ChromeMark::Zoom, 11_000);
        let both = marks.look(12_000, false);
        assert!(
            both.zoom_shown && both.place_shown && both.next_change_in_ms == Some(1_000),
            "the soonest: {both:?}"
        );
        let one = marks.look(13_000, false);
        assert!(one.zoom_shown && !one.place_shown && one.next_change_in_ms == Some(1_000), "{one:?}");
    }

    /// Failure: a mark fades from under the pointer, or from under a field being typed in, or goes at once when the pointer leaves, so the page number can't be read or clicked by hovering, and the go-to field's number vanishes mid-typing.
    #[test]
    fn the_pointer_and_an_open_field_hold_a_mark() {
        let mut marks = Marks::default();
        marks.pointer(ChromeMark::Place, true, 0);
        let look = marks.look(60_000, false);
        assert!(
            look.place_shown && look.next_change_in_ms.is_none(),
            "held as long as the pointer's there: {look:?}"
        );
        marks.pointer(ChromeMark::Place, false, 60_000);
        assert!(marks.look(60_000 + MARK_SHOWN_MS - 1, false).place_shown, "3 s after it leaves");
        assert!(!marks.look(60_000 + MARK_SHOWN_MS, false).place_shown);
        marks.held(ChromeMark::Zoom, true, 70_000);
        assert!(marks.look(100_000, false).zoom_shown, "typing a level");
        marks.held(ChromeMark::Zoom, false, 100_000);
        assert_eq!(marks.look(100_000, false).next_change_in_ms, Some(MARK_SHOWN_MS), "3 s after the field closes");
        // Leaving a place that wasn't entered isn't a change.
        let mut quiet = Marks::default();
        quiet.pointer(ChromeMark::Zoom, false, 5);
        assert!(!quiet.look(5, false).zoom_shown);
    }

    /// Failure: the window's buttons fade with the sidebar showing, stay with it hidden, or don't come back under the pointer, so a window with its sidebar hidden has no way to close it or show the sidebar again. Owner, 2026-11-08: "When sidebar is hidden, I want the traffic light buttons and sidebar toggle to autohide. To show them again you hover above them".
    #[test]
    fn the_window_buttons_go_with_the_sidebar_and_come_back_under_the_pointer() {
        let mut marks = Marks::default();
        assert!(marks.look(0, false).window_buttons_shown, "with the sidebar showing, always");
        assert!(!marks.look(0, true).window_buttons_shown, "with it hidden, gone");
        marks.pointer(ChromeMark::WindowButtons, true, 0);
        let over = marks.look(0, true);
        assert!(
            over.window_buttons_shown && over.next_change_in_ms.is_none(),
            "back under the pointer, with no timer: {over:?}"
        );
        marks.pointer(ChromeMark::WindowButtons, false, 10);
        assert!(
            !marks.look(10, true).window_buttons_shown,
            "gone as it leaves, as the system's full-screen buttons go"
        );
        marks.changed(ChromeMark::WindowButtons, 20);
        assert!(
            !marks.look(20, true).window_buttons_shown,
            "they don't change, so nothing brings them but the pointer"
        );
    }

    /// Failure: a time from far in the future or an overflowing clock panics or shows a mark for ever.
    #[test]
    fn the_clock_at_its_ends_is_safe() {
        let mut marks = Marks::default();
        marks.changed(ChromeMark::Zoom, u64::MAX - 1);
        let look = marks.look(u64::MAX, false);
        assert!(!look.zoom_shown, "{look:?}");
        marks.changed(ChromeMark::Place, 5);
        assert!(!marks.look(0, false).place_shown || marks.look(0, false).next_change_in_ms.is_some());
    }

    /// Failure: the marks go by a clock of their own rather than the one they're given, so a mark fades early or late against the app's time, or a test of it has to wait 3 s.
    #[test]
    fn the_marks_go_by_the_clock_they_are_given() {
        let clock = Arc::new(crate::testkit::FakeClock::at(50_000));
        let chrome = Chrome::new(Arc::clone(&clock) as Arc<dyn Clock>);
        chrome.changed(ChromeMark::Place);
        assert_eq!(chrome.look(false).next_change_in_ms, Some(MARK_SHOWN_MS));
        clock.advance(MARK_SHOWN_MS - 1);
        let look = chrome.look(false);
        assert!(look.place_shown && look.next_change_in_ms == Some(1), "{look:?}");
        clock.advance(1);
        assert!(!chrome.look(false).place_shown, "gone at 3 s on the given clock");
        chrome.held(ChromeMark::Zoom, true);
        clock.advance(60_000);
        assert!(chrome.look(true).zoom_shown, "held while the field is open");
        chrome.held(ChromeMark::Zoom, false);
        assert_eq!(chrome.look(true).next_change_in_ms, Some(MARK_SHOWN_MS), "3 s from the clock's now");
        chrome.pointer(ChromeMark::WindowButtons, true);
        assert!(chrome.look(true).window_buttons_shown);
    }
}
