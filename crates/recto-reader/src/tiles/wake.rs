//! **Waking:** what a tab's first frame takes from its sleep file (its pages' sizes, so its place is on its paper from the first frame), and how its wake
//! goes until it's sharp. No picture of a page comes from a sleep file or goes into one (owner, 2026-11-15): a waking tab's pages are on their paper, the
//! right colour, size and place, and each fills in as it's drawn.

use super::{DocView, MAX_PAGES_IN_VIEW, MAX_ZOOM, MIN_ZOOM, PageMap, Reader};
use crate::layout::PageBox;
use std::time::{Duration, Instant};

/// What a page view takes from a sleep file for a tab's first frame: its pages' sizes. Its numbers have been held to what's possible on the way here, and
/// these are held to the layout's own.
#[derive(Debug, Default)]
pub struct SleepView {
    pub usual: Option<PageBox>,
    pub sizes: Vec<(u32, PageBox)>,
}

/// What a tab's sleep file gets from the page view: its pages' sizes, as it has them now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SleepMaterial {
    /// Which version of the tab's own file this is of, as a worker said. `None` if none has.
    pub version: Option<u64>,
    pub usual: Option<PageBox>,
    pub sizes: Vec<(u32, PageBox)>,
}

/// Less of the view than this isn't a page that has to be showing something for a frame to be right.
pub(super) const MIN_SHARE_OF_VIEW: f64 = 0.05;

/// How long a wake's followed, for how long it took.
pub(super) const WAKE_FOLLOWED_FOR: Duration = Duration::from_secs(5);

/// What a wake had to show when it began.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WakeStock {
    /// Not looked at yet: the tab hasn't been made the hot one.
    #[default]
    NotLooked,
    /// Something of every page in view was here, from what the tile cache kept: a warm tab.
    Cache,
    /// Not every page in view had something, and the tab came with its sleep file's facts (from the file, or handed back inside its cool-down): its
    /// pages are on their paper at their sizes from the first frame, and fill in as they're drawn.
    Facts,
    /// Neither: its pages go on their paper as their sizes are learned.
    Nothing,
}

impl WakeStock {
    pub fn word(self) -> &'static str {
        match self {
            Self::NotLooked => "not_looked",
            Self::Cache => "cache",
            Self::Facts => "facts",
            Self::Nothing => "nothing",
        }
    }
}

/// How a tab's wake went, for QA and the budgets: from being asked to the first frame showing something on every page, and to sharp. Written once for each wake, never for each frame, and small: [`WAKE_REPORT_BYTES`].
#[derive(Clone, Copy, Debug, Default)]
pub struct WakeReport {
    pub tab: u32,
    /// It came with a sleep file's facts ([`SleepView`]).
    pub from_facts: bool,
    pub first_frame_us: Option<u64>,
    pub sharp_us: Option<u64>,
    /// Frames the host applied with a visible page bare, once it was at its place.
    pub blank_frames: u32,
    /// What there was to show when the wake began.
    pub had: WakeStock,
    /// How it ended, or that it hasn't.
    pub ended: WakeEnd,
}

/// **How a wake's report came to an end,** so a report with no first frame says why: not yet, someone went elsewhere first, or it was still bare when
/// it stopped being followed, which is a slow first page and not one that never came (found 2026-11-15: under CI's limits at a load of 30, three wakes
/// of the long session read as "never got its first frame", and their tabs were drawn).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WakeEnd {
    /// It's still being followed.
    #[default]
    UnderWay,
    /// Every page in view drawn sharp.
    Sharp,
    /// The view was sent somewhere else (a jump, a follow), and what's bare from there is pages being reached.
    Moved,
    /// Past [`WAKE_FOLLOWED_FOR`]: what it had by the first frame the host applied after that is what it says.
    NoLongerFollowed,
}

impl WakeEnd {
    pub fn word(self) -> &'static str {
        match self {
            Self::UnderWay => "under_way",
            Self::Sharp => "sharp",
            Self::Moved => "moved",
            Self::NoLongerFollowed => "no_longer_followed",
        }
    }
}

/// The size of a [`WakeReport`], held: one for each page view, kept as of the last wake.
pub const WAKE_REPORT_BYTES: usize = 48;
const _: () = assert!(std::mem::size_of::<WakeReport>() <= WAKE_REPORT_BYTES);

/// Built only in the tiles module ([`Waking::new`]), with no `Default`, as each of [`Reader`]'s parts is (see there): the engine reaches it through `Reader`'s field for what a wake is given and told below.
#[derive(Debug)]
pub(crate) struct Waking {
    /// What's come from a tab's sleep file, for when it's made the hot tab.
    pub(super) sleep_view: Option<(u32, SleepView)>,
    /// When the wake under way was asked for, and how it's gone.
    pub(super) waking: Option<(Instant, WakeReport)>,
    pub(super) last_wake: Option<WakeReport>,
    /// The window's on the screen, so what the host applies is a frame someone can see.
    pub(super) window_up: bool,
}

// size: held at what it was on 2026-11-13 (macOS, debug and release alike) before R3.6 moved `Reader`'s methods onto its parts, so the move, or a field added later, can't grow it unseen.
#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<Waking>() <= 440, "the wake's state grew");

impl Waking {
    pub(super) fn new() -> Self {
        Self {
            sleep_view: None,
            waking: None,
            last_wake: None,
            window_up: false,
        }
    }

    /// **What a tab's first frame takes from its sleep file, ahead of [`Reader::activate`]:** its pages' sizes, so its place is on its paper at once.
    pub(crate) fn give_sleep(&mut self, tab: u32, view: SleepView) {
        self.sleep_view = Some((tab, view));
    }

    /// A tab's being woken, as of now, with `hot` the tab showing ([`Reader::note_wake`] says it, so a caller can't give another). For how long it takes.
    pub(super) fn note_wake(&mut self, hot: Option<u32>, tab: u32, began: Instant) {
        // Said when it was asked for, and said again on its way to the screen: the first is when it began, unless it's older than a wake's followed for,
        // when it's a note of a wake that never came to anything.
        let said_already = self
            .waking
            .is_some_and(|(at, report)| report.tab == tab && began.saturating_duration_since(at) <= WAKE_FOLLOWED_FOR);
        if hot != Some(tab) && !said_already {
            self.waking = Some((began, WakeReport { tab, ..WakeReport::default() }));
        }
    }

    /// The window's gone up. From here, a bare page is one that's seen.
    pub(crate) fn window_up(&mut self) {
        self.window_up = true;
    }

    /// How the last wake went: the one under way once its tab's been shown, or the one before. One noted and not yet shown isn't a wake yet.
    pub(crate) fn last_wake(&self) -> Option<WakeReport> {
        self.waking
            .map(|(_, report)| report)
            .filter(|report| report.had != WakeStock::NotLooked)
            .or(self.last_wake)
    }

    /// The wake under way is over, as someone's gone somewhere else. One whose tab was never shown was no wake, and leaves the last one as it was.
    pub(super) fn finish_wake(&mut self) {
        self.end_wake(WakeEnd::Moved);
    }

    /// The wake under way is over, for this reason, unless its tab was never shown.
    fn end_wake(&mut self, ended: WakeEnd) {
        if let Some((_, mut report)) = self.waking.take()
            && report.had != WakeStock::NotLooked
        {
            report.ended = ended;
            self.last_wake = Some(report);
        }
    }

    /// The tabs held now are the one showing and `warm`: a wake noted for a tab that's neither and not yet shown goes, its tab closed or gone cold.
    pub(super) fn keep_for(&mut self, hot: u32, warm: &[u32]) {
        if self
            .waking
            .is_some_and(|(_, report)| report.had == WakeStock::NotLooked && report.tab != hot && !warm.contains(&report.tab))
        {
            self.waking = None;
        }
    }

    /// **What the wake under way had to show,** once, as its tab is made the hot one and has been sent to its place: whether something of every page in
    /// view at its place is in the tile cache, and if not, whether it came with its sleep file's facts.
    // R3.6: a `Reader` operation the ratchet's method count can't see: an associated function of `Waking` taking the whole page view.
    pub(super) fn note_stock(reader: &mut Reader, tab: u32) {
        let Some((_, report)) = reader.wake.waking else { return };
        if report.tab != tab || report.had != WakeStock::NotLooked {
            return;
        }
        let Some(doc) = reader.docs.views.get(&tab) else { return };
        let y = doc.place_y().max(0.0);
        let height =
            (reader.viewport.height * reader.viewport.magnification.max(f64::MIN_POSITIVE) / doc.place.magnification.clamp(MIN_ZOOM, MAX_ZOOM)).max(1.0);
        let in_view: Vec<u32> = if doc.layout.count() == 0 {
            Vec::new()
        } else {
            doc.layout.pages_between(y, y + height).take(MAX_PAGES_IN_VIEW).collect()
        };
        let have: std::collections::HashSet<u32> = reader
            .cache
            .entries
            .values()
            // Only what goes on a page of this paper.
            .filter(|entry| entry.tab == tab && entry.paper == reader.paper && (entry.surface.is_some() || entry.rgb.is_some()))
            .map(|entry| entry.page)
            .collect();
        let had = if !in_view.is_empty() && in_view.iter().all(|page| have.contains(page)) {
            WakeStock::Cache
        } else if report.from_facts {
            WakeStock::Facts
        } else {
            WakeStock::Nothing
        };
        if let Some((_, report)) = &mut reader.wake.waking {
            report.had = had;
        }
    }
}

/// **Why a window can't go up yet with what's on the layers,** the first reason [`Reader::not_showable`] finds, for the record of the window's first
/// showing: a page not ready because it's still being drawn is a slow page, and one whose tiles wait for a host nobody told is a missed wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotShowable {
    /// Still being sent to its place.
    Jump,
    /// Ops wait for the host to take, and it's been told of them.
    OpsWaiting,
    /// Ops wait for the host to take, and it hasn't been told (the engine's word, [`crate::engine::waking::Wake::not_showable`]): for a moment as they're
    /// told, or for good if a telling was missed.
    OpsUntold,
    /// Given to the host, and not yet said to be applied.
    Unapplied,
    /// The view has no height yet.
    NoViewport,
    /// No document is shown.
    NoDocument,
    /// The document has a problem, which the page says.
    Problem,
    /// A page in view has nothing on it, and the thread that draws is at work.
    Drawing,
    /// A page in view has nothing on it, and nothing's drawing.
    NotCovered,
}

impl NotShowable {
    const ALL: [Self; 9] = [
        Self::Jump,
        Self::OpsWaiting,
        Self::OpsUntold,
        Self::Unapplied,
        Self::NoViewport,
        Self::NoDocument,
        Self::Problem,
        Self::Drawing,
        Self::NotCovered,
    ];

    /// The word the record keeps.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Jump => "jump",
            Self::OpsWaiting => "ops_waiting",
            Self::OpsUntold => "ops_untold",
            Self::Unapplied => "unapplied",
            Self::NoViewport => "no_viewport",
            Self::NoDocument => "no_document",
            Self::Problem => "problem",
            Self::Drawing => "drawing",
            Self::NotCovered => "not_covered",
        }
    }

    /// A byte for it, never 0, which is none: for a record kept in a few bytes.
    #[must_use]
    pub const fn code(reason: Option<Self>) -> u8 {
        match reason {
            None => 0,
            Some(reason) => reason as u8 + 1,
        }
    }

    /// What [`Self::code`] made a byte of.
    #[must_use]
    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code.checked_sub(1)?)).copied()
    }
}

impl Reader {
    /// Whether every visible page is showing something, at the place, with nothing on its way to the host: what a window can go up with. It'll sharpen from
    /// there. The engine asks why not ([`Self::not_showable`]); the tests ask this.
    #[cfg(test)]
    pub fn is_showable(&self) -> bool {
        self.not_showable().is_none()
    }

    /// **Why the window can't go up with what's on the layers,** the first reason found; none when it can ([`Self::is_showable`]).
    pub fn not_showable(&self) -> Option<NotShowable> {
        if self.place.jump.is_some() {
            return Some(NotShowable::Jump);
        }
        if !self.host.ops.is_empty() {
            return Some(NotShowable::OpsWaiting);
        }
        if self.host.unapplied {
            return Some(NotShowable::Unapplied);
        }
        if self.viewport.height <= 0.0 {
            return Some(NotShowable::NoViewport);
        }
        let Some(doc) = self.docs.shown() else { return Some(NotShowable::NoDocument) };
        if doc.drawing.problem.is_some() {
            return Some(NotShowable::Problem);
        }
        if self.pages_all_covered(doc) {
            return None;
        }
        Some(if self.work.running.is_some() {
            NotShowable::Drawing
        } else {
            NotShowable::NotCovered
        })
    }

    /// Whether every page that's really in view has something on it. A sliver of the page before, a few points at the top of the view, isn't what a frame's blank for: it has no picture if it wasn't drawn when the tab was left, and it comes a moment later with the rest.
    pub(super) fn pages_all_covered(&self, doc: &DocView) -> bool {
        let (top, bottom) = (self.viewport.y, self.viewport.y + self.viewport.height);
        self.visible_pages(doc).all(|page| {
            let rect = doc.layout.rect(page);
            let showing = rect.y.max(top)..(rect.y + rect.height).min(bottom);
            showing.end - showing.start < self.viewport.height * MIN_SHARE_OF_VIEW || self.cache.attached.get(&page).is_some_and(|ids| !ids.is_empty())
        })
    }

    /// Goes through how a wake's going, each time the host has applied something.
    pub(super) fn track_wake(&mut self) {
        let Some((began, mut report)) = self.wake.waking else { return };
        // Past this, it's not the wake that's being looked at any more: this frame is the last it's looked at for, so a first page that comes late is
        // said as late, and not as never.
        let past = began.elapsed() > WAKE_FOLLOWED_FOR;
        if self.docs.hot != Some(report.tab) {
            // Noted and not yet shown: the note and the showing are two steps, and a frame the host applies between them is the tab before's. Kept, and
            // looked at once its tab is shown; one never shown goes by the time above, its tab's closing, or the next note, and is never recorded.
            if report.had != WakeStock::NotLooked || past {
                self.wake.waking = None;
            }
            return;
        }
        let Some(doc) = self.docs.shown() else {
            if past {
                self.wake.end_wake(WakeEnd::NoLongerFollowed);
            }
            return;
        };
        if self.place.jump.is_none() && self.viewport.height > 0.0 {
            let covered = self.pages_all_covered(doc);
            if covered && report.first_frame_us.is_none() {
                report.first_frame_us = Some(u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX));
            } else if !covered && report.first_frame_us.is_none() && self.wake.window_up {
                report.blank_frames += 1;
            }
        }
        if report.first_frame_us.is_some() && self.is_settled() {
            report.sharp_us = Some(u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX));
            report.ended = WakeEnd::Sharp;
            self.wake.last_wake = Some(report);
            self.wake.waking = None;
            return;
        }
        self.wake.waking = Some((began, report));
        if past {
            self.wake.end_wake(WakeEnd::NoLongerFollowed);
        }
    }

    /// **What a tab's sleep file gets from what's here:** its pages' sizes and the version they're of. A short lock.
    ///
    /// `now` is what the session says the tab's pages are now. If its file isn't what this is a view of, what's here is of another file, and nothing goes in
    /// its sleep file. **If its pages aren't the pages here** (deleted, moved, added or turned while it was off screen, which a view held warm hears of only
    /// when it's shown again), what's here is filed under places that are other pages now: its facts go, and no size.
    pub fn sleep_material(&mut self, tab: u32, now: &PageMap) -> Option<SleepMaterial> {
        let doc = self.docs.views.get(&tab)?;
        if doc.map.own != now.own {
            return None;
        }
        if !doc.map.same_pages(now) || doc.map.turns != now.turns {
            return Some(SleepMaterial::default());
        }
        Some(SleepMaterial {
            version: doc.map.files.first().and_then(|file| doc.drawing.versions.get(file)).copied(),
            usual: doc.layout.usual_size(),
            sizes: doc.layout.other_sizes(),
        })
    }

    /// What came from a sleep file goes in, if it's for this tab: its pages' sizes, taken on trust till a worker says otherwise, so its place is on its paper
    /// from the first frame. Anything for another tab has been overtaken.
    pub(super) fn take_sleep_view(&mut self, tab: u32) {
        let Some((for_tab, view)) = self.wake.sleep_view.take() else { return };
        if for_tab != tab {
            return;
        }
        let Some(doc) = self.docs.views.get_mut(&tab) else { return };
        if let Some(usual) = view.usual {
            doc.layout.seed_usual(usual);
            doc.drawing.size_on_trust = true;
        }
        for (page, size) in view.sizes {
            doc.layout.set_page_size(page, size);
        }
        if let Some((_, report)) = &mut self.wake.waking
            && report.tab == tab
        {
            report.from_facts = true;
        }
    }
}
