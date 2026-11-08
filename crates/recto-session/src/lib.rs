//! Tabs, tiers and the sidebar: every rule the host needs, with no UI toolkit in sight.
//!
//! **Cost follows activity.** A cold tab is a [`TabRecord`] and nothing else: no handle, no contents, no pages. Opening
//! reads a PDF's facts and closes it. Only the active tab and the most recent others keep their contents, up to
//! [`MAX_WARM`], and anything evicted is rebuilt from disk when it's needed again.

pub mod compare;
pub mod edit;
pub mod record;
pub mod sleep;
pub mod trail;

pub use edit::Author;
use edit::{EditError, PageEdits};
use record::{EditsSummary, FileNotice, FileStatus, FindRecord, JournalHeader, JournalOp, MAX_TAB_ID, PaperName, PathField, PlaceDelta, PlaceRecord, RecordError, Selection, SessionRecord, TabEntry, ViewRecord};
use recto_core::verdict::ContentsVerdict;
use recto_core::{DocFacts, NONE, OpenError, Toc};
use recto_platform::{FileStamp, file_stamp};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Tabs that may keep their contents in memory: the active one plus two recent ones.
pub const MAX_WARM: usize = 3;
/// A cap on tabs, because everything grown from outside has one.
pub const MAX_TABS: usize = 10_000;

/// Reads a document's facts. A seam so tests can stand in for the disk.
pub trait DocLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError>;
}

/// Reads from disk through the lazy reader.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiskLoader;

impl DocLoader for DiskLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        recto_core::inspect(path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u32);

/// How much of a tab is in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The record only.
    Cold,
    /// The record plus contents.
    Warm,
    /// The tab on screen.
    Hot,
}

/// One tab. Everything else about it can be rebuilt from disk.
// size: asserted <= 256 by `tab_record_stays_small`. Held per tab, including cold ones.
#[derive(Debug)]
pub struct TabRecord {
    /// What's known of its file came from a sleep file, and the PDF hasn't said so yet: which version that file said it was of, or zero for none said.
    from_sleep: Option<u64>,
    /// Its file's protected. Its contents and pages are never written anywhere, so it has no sleep file.
    encrypted: bool,
    /// When it was last active, edited or worked on, in seconds of the session. What going dormant goes by, with no timer of its own.
    touched: u32,
    pub id: TabId,
    dir: u32,
    file_name: Box<OsStr>,
    /// The name shown in the sidebar: the file's name without its extension.
    pub name: Box<str>,
    /// Zero-based page the reader is on.
    pub page: u32,
    pub page_count: u32,
    pub toc_open: bool,
    /// Why this tab's file can't be read at the moment, if so.
    pub problem: Option<OpenError>,
    /// Only the renderer could read this file. See [`DocFacts::renderer_only`].
    pub renderer_only: bool,
    /// Which version of the file this tab's facts and any edits are about. `None` if it couldn't be looked at.
    stamp: Option<FileStamp>,
    /// Exactly where the reader was, so a tab that goes cold or across a restart comes back there. It's what's asked for on showing the tab, and what the page view says on leaving one.
    place: Option<PlaceRecord>,
    /// Quarter turns of the view. Nothing turns the view yet.
    pub view_rotation: u8,
    /// How its pages are laid out: a page to a row, or two side by side.
    pub spread: record::Spread,
    /// Selection work: what's selected. Nothing sets it yet.
    selection: Option<Box<Selection>>,
    /// What the reader should be told about this tab's file, which is rare enough to be a pointer.
    notice: Option<Box<FileNotice>>,
    /// Its place has changed since it was last written, so one small line says so.
    place_dirty: bool,
    /// What's thought of its own contents, worked out once when they're read, so the sidebar needn't at every page.
    contents_verdict: ContentsVerdict,
    /// The reader chose generated contents over the file's own. Kept in the record.
    pub generated_contents: bool,
    /// Sections the reader opened or closed by hand, as `(entry, open)`, against what following the place opens. Nothing for a tab nobody's folded, so a cold tab pays a pointer.
    folds: Option<Box<[(u32, bool)]>>,
    /// Back and Forward, once someone's jumped in it: see [`trail`]. A pointer for a tab nobody has, and at most 100 stops of 12 bytes for one somebody has, counted in a cold tab's cost (`a_cold_tab_with_a_full_trail_stays_in_budget`).
    trail: Option<Box<trail::Trail>>,
}

/// A trail stop's page, and which file and file page that was, if it was any.
type StopSource = (u32, Option<(u16, u32)>);

/// What an edit needs of before it: where the reader's page came from, and each trail stop's.
struct Before {
    place: Option<(u16, u32, u32)>,
    stops: Option<Vec<StopSource>>,
}

/// Where a page of a tab comes from now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSource {
    pub path: PathBuf,
    /// Zero-based page in that file.
    pub page: u32,
    /// Unsaved quarter turns on it.
    pub quarter_turns: u8,
    /// It came from another PDF added to this one, rather than the tab's own file.
    pub added: bool,
    /// What the file was when its page numbers were recorded, if this page's number depends on that: an added file, or the tab's own file once it's been edited.
    pub expected: Option<Option<FileStamp>>,
}

impl PageSource {
    /// Whether the file is still the one the page number is about. A file that's gone isn't. One that couldn't be looked at when it was recorded makes no claim.
    pub fn is_current(&self) -> bool {
        match self.expected {
            None | Some(None) => true,
            Some(Some(stamp)) => file_stamp(&self.path).ok() == Some(stamp),
        }
    }
}

/// A row in the sidebar. Borrows its text from the session, so building the model allocates little.
// size: asserted <= 64 by `sidebar_row_stays_small`. Held per visible row while a model is alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarRow<'a> {
    pub kind: RowKind,
    pub tab: TabId,
    pub text: &'a str,
    /// One-based page number, shown on hover.
    pub page: Option<u32>,
    /// Nesting within the contents.
    pub depth: u8,
    /// The active PDF, or the section being read.
    pub current: bool,
    /// Something's wrong with this tab's file, in words of Rust's own. On a note, what it means.
    pub notice: Option<&'static str>,
    /// What tapping a note does.
    pub action: Option<ContentsAction>,
    /// A contents row's entry in its list.
    pub entry: u32,
    /// It has sections under it, so it has a chevron to fold them.
    pub expandable: bool,
    /// Those sections show.
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Tab,
    Contents,
    /// A line of Rust's over a tab's contents: generated contents on offer, being made, or shown.
    Note,
}

/// Everything the host draws in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarModel<'a> {
    pub rows: Vec<SidebarRow<'a>>,
    /// How many rows the rail fills: contents rows up to and including the section being read.
    pub rail_filled: usize,
    /// Whether the active PDF's contents are showing, which decides the chevron.
    pub toc_open: bool,
    /// The footer line, such as "32 of 96". Empty with no PDF open.
    pub place: String,
    /// Whether the active PDF has unsaved changes, which shows its dot and the Save buttons.
    pub edited: bool,
    /// The owner's edits on the active PDF have been undone, by anyone, and can still be redone. The page matches the file, so `edited` is false, but closing would lose them, so the app has this to show.
    pub undone_owner_steps: bool,
}

/// How much of a tab's row in the host is what it's told about the file.
pub fn tab_notice_words(tab: &TabRecord, has_edits: bool) -> Option<&'static str> {
    tab.notice.as_deref().map(|notice| notice.message(has_edits))
}

/// A tab's row in the sidebar, in points.
pub const TAB_ROW_HEIGHT: f64 = 32.0;
/// A contents row in the sidebar, in points.
pub const CONTENTS_ROW_HEIGHT: f64 = 28.0;
/// The note over the contents, in points: a line of small text.
pub const NOTE_ROW_HEIGHT: f64 = 24.0;
/// The most of the sidebar's room the contents take: about 60%, by the owner's yes of 2026-11-07. The rest is for the other tabs, and it's what keeps them from being pushed off by one long outline.
pub const CONTENTS_MAX_SHARE: f64 = 0.6;

/// **The scroll thumb**, for the sidebar's lists and the page area alike, so they match. Owner, 2026-11-07: "The sidebar thumb should always be visible, and lets hide the rail", then "the thumb should be thinner", and for both scrollers "the scroll thumb always visible, the rail always hidden", whatever the system's setting. It shows only when there's something to scroll, and draws over the content's right margin, taking no width from it.
///
/// **Why 4 points:** the Mac's own overlay thumb is about 7 points at rest and 11 under the pointer. The owner asked for thinner, and 4 is the thinnest that still reads as a thumb on a 1x screen (2 device pixels either side of its centre line), with rounded ends. Under the pointer it widens to 6, as the system's does, and it's grabbed anywhere in a strip [`ScrollThumb::hit_width`] wide, so a slim look doesn't make it hard to drag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollThumb {
    /// Its width at rest, and under the pointer or while dragged, in points.
    pub width: f64,
    pub hover_width: f64,
    /// The strip along the edge that takes the pointer, in points: wider than the thumb looks.
    pub hit_width: f64,
    /// From the thumb's right side to the edge, in points.
    pub inset: f64,
    /// The shortest it gets, in points.
    pub min_length: f64,
    /// Its colour as `0xRRGGBB` and an opacity, on light and dark, at rest and under the pointer. Black or white over whatever's behind, as the system's thumb is, so it sits on the sidebar's colour and the page area's alike.
    pub light: (u32, f64),
    pub light_hover: (u32, f64),
    pub dark: (u32, f64),
    pub dark_hover: (u32, f64),
}

/// The one scroll thumb.
pub const SCROLL_THUMB: ScrollThumb = ScrollThumb { width: 4.0, hover_width: 6.0, hit_width: 12.0, inset: 3.0, min_length: 24.0, light: (0x000000, 0.22), light_hover: (0x000000, 0.42), dark: (0xFFFFFF, 0.24), dark_hover: (0xFFFFFF, 0.45) };

/// How much of the sidebar's list goes where, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarHeights {
    /// The height of the contents' own scrolling area. Less than their rows' total means they scroll.
    pub contents: f64,
    /// The note's height over them, or nothing.
    pub note: f64,
    /// Whether the list of tabs doesn't fit, and scrolls by itself.
    pub tabs_scroll: bool,
}

/// **The sidebar's height rule.** Owner, 2026-11-07: "The max height of toc does not seem to be working", then yes to a true maximum: the contents take what they need, up to [`CONTENTS_MAX_SHARE`] of the room, and scroll inside that. The other tabs have what's left, and a long list of them scrolls by itself in it. It takes the room it's told of, so anything else docked in the sidebar comes off first.
pub fn sidebar_heights(room: f64, tabs: u32, contents_rows: u32) -> SidebarHeights {
    sidebar_heights_with_note(room, tabs, contents_rows, false)
}

/// The same, with a note over the contents, such as "Generated contents". It stays put above them while they scroll, so a generated list is labelled wherever it's scrolled to, and it comes out of the same share.
pub fn sidebar_heights_with_note(room: f64, tabs: u32, contents_rows: u32, note: bool) -> SidebarHeights {
    let room = if room.is_finite() { room.max(0.0) } else { 0.0 };
    let tab_row = if tabs > 0 { TAB_ROW_HEIGHT } else { 0.0 };
    let note = if note && room - tab_row >= NOTE_ROW_HEIGHT { NOTE_ROW_HEIGHT } else { 0.0 };
    let natural = f64::from(contents_rows) * CONTENTS_ROW_HEIGHT;
    // With a window too short for the rest, the active tab's own row comes first.
    let contents = natural.min(room * CONTENTS_MAX_SHARE - note).min(room - tab_row - note).max(0.0);
    SidebarHeights { contents, note, tabs_scroll: f64::from(tabs) * TAB_ROW_HEIGHT + note + contents > room }
}

/// How long after the reader stops using the contents, with the pointer out of them, before they may follow again.
pub const FOLLOW_RESUME_AFTER: f64 = 3.0;
/// Where the place being read comes to rest, as a part of the contents' height from the top. Above the middle, as reading goes downwards: there's more ahead to see than behind, and a longer rest before it needs to move.
const FOLLOW_ANCHOR: f64 = 0.3;
/// The dead zone, as parts of the contents' height from the top. Inside it the list doesn't move by one pixel, so reading slowly gets long rests. Its lower edge is what sets how long and how far: from the anchor to here is 45% of the height, about 12 rows in a 776 point list, and each move covers that much rather than nudging at every section. Its upper edge lets the reader step back a few rows with nothing moving.
const FOLLOW_DEAD_ZONE: (f64, f64) = (0.12, 0.75);
/// How long a move takes to settle, in seconds, shortest and longest: it grows with the distance and stays bounded, so a far jump is a fast whoosh that still starts and ends softly. Owner, 2026-11-07: "It can whoosh with smooth acceleration at both ends."
const FOLLOW_SECONDS: (f64, f64) = (0.6, 1.1);
/// The integration step, so a move's shape doesn't depend on the frame rate. At most a few of these a frame.
const FOLLOW_STEP: f64 = 1.0 / 240.0;
/// Close enough to be there, in points, and slow enough to stop, in points a second.
const FOLLOW_ARRIVED: (f64, f64) = (0.25, 3.0);

/// The contents' scrolling area, as it is when it's asked about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentsView {
    pub scroll_y: f64,
    pub height: f64,
    pub rows: u32,
}

/// What the host should do with the contents' scroll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FollowMove {
    Stay,
    /// Go there at once: a tab's first showing, or Reduce Motion.
    SetTo(f64),
    /// Run frames and call [`ContentsFollow::step`].
    Move,
}

/// **How the contents follow the place being read.** Owner, 2026-11-07: "It can whoosh with smooth acceleration at both ends. It can have a "dead-zone" so it doesn't react to small scrolls within the window, so basically it repositions, stays still (while user is reading slowly scrolling) then repositions after the user is scrolling a bit." A plain state machine with its times passed in, so the host only runs frames.
///
/// - **Reposition, then rest.** The place sits at [`FOLLOW_ANCHOR`]. While it stays in [`FOLLOW_DEAD_ZONE`], nothing moves. Leaving it starts one move, back to the anchor.
/// - **Every move starts and ends softly,** with its length bounded by [`FOLLOW_SECONDS`]. It's a chain: a lag on the target, followed by a critically damped spring. That's what makes acceleration build from nothing at the start, and fade at the end.
/// - **A new target during a move only changes where the chain is heading,** so nothing steps, in position, speed or acceleration. Fast reading through many sections carries on as one long move until it settles. It only changes once the place has left the dead zone around where the move is going.
/// - **A reader who scrolls the contents is left alone,** and so is a pointer resting in them. Following resumes on the next change of section once the pointer's out and [`FOLLOW_RESUME_AFTER`] has passed.
/// - **With Reduce Motion nothing animates:** it goes straight there, and only when the place has left the view.
///
/// Costs a few comparisons a section, and nothing per row. The host's frames run only during a move.
#[derive(Clone, Copy, Debug)]
pub struct ContentsFollow {
    paused: bool,
    pointer_inside: bool,
    last_user_scroll: f64,
    current: Option<u32>,
    placed: bool,
    moving: Option<Motion>,
    /// Where to go on the first showing, as a restore found it, and whether it was following.
    resume: Option<(f64, bool)>,
}

/// Which rows of the contents closed and opened, and how far that carried the place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentsRowChange {
    pub from: u32,
    pub removed: u32,
    pub inserted: u32,
    pub place_shift: i32,
}

/// How much faster a move gets each time a section goes by with the place out of the view.
const FOLLOW_CATCH_UP: f64 = 1.15;
/// The chain's fastest rate, per second: its lag behind what it's chasing is about 3 divided by this.
const FOLLOW_FASTEST_RATE: f64 = 120.0;

/// A move under way.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Motion {
    target: f64,
    /// The lagged target the spring chases, which is what makes it start softly.
    aim: f64,
    /// Where it really is, in the chain. A scroll view rounds to whole device pixels, and a slow end reading that back would never arrive. Taken from the host on the first frame.
    at: Option<f64>,
    velocity: f64,
    /// Rate of the chain, per second, from the distance.
    rate: f64,
}

impl Default for ContentsFollow {
    fn default() -> Self {
        Self { paused: false, pointer_inside: false, last_user_scroll: f64::NEG_INFINITY, current: None, placed: false, moving: None, resume: None }
    }
}

/// The chain's rate for a distance: the further, the longer it takes, up to the most, so far jumps are quick and short ones aren't abrupt.
fn follow_rate(distance: f64, height: f64) -> f64 {
    let heights = distance.abs() / height.max(1.0);
    let share = (heights / 4.0).clamp(0.0, 1.0).sqrt();
    let seconds = FOLLOW_SECONDS.0 + (FOLLOW_SECONDS.1 - FOLLOW_SECONDS.0) * share;
    // A chain like this takes this many of its natural periods to close from this far to the last quarter of a point, so a long way is no longer than the time above.
    let periods = 6.0 + (distance.abs().max(1.0) / FOLLOW_ARRIVED.0).ln();
    periods / seconds
}

impl ContentsFollow {
    /// The reader scrolled the contents. Whatever was moving stops, right where it is.
    pub fn user_scrolled(&mut self, now: f64) {
        self.paused = true;
        self.last_user_scroll = now;
        self.moving = None;
    }

    /// The pointer came into the contents or left. Nothing's pulled from under it: a move under way stops.
    pub fn pointer(&mut self, inside: bool) {
        self.pointer_inside = inside;
        if inside {
            self.moving = None;
        }
    }

    /// Another tab: its contents start fresh, in the right place.
    pub fn new_contents(&mut self) {
        let pointer_inside = self.pointer_inside;
        *self = Self { pointer_inside, ..Self::default() };
    }

    /// The first showing goes here, as it was left, and not to where the place would put it. Whether they were following or left by the reader comes back too.
    pub fn resume_at(&mut self, y: f64, following: bool) {
        if y.is_finite() {
            self.resume = Some((y.max(0.0), following));
            self.paused = !following;
        }
    }

    /// The contents' area changed height, as the window did. The place is looked at again next, and goes straight to where it should be, since nothing about a resize wants moving.
    pub fn view_changed(&mut self) {
        self.moving = None;
        // A list the reader has scrolled themselves stays where they left it, resized or not. Clearing this would count as a change of section, and snap it back once they'd stopped.
        if self.paused {
            return;
        }
        self.current = None;
        self.placed = false;
    }

    pub fn is_following(self) -> bool {
        !self.paused
    }

    pub fn is_moving(self) -> bool {
        self.moving.is_some()
    }

    /// Rows closed or opened. Following, the place stays where it is on screen, so that a chapter closing above it isn't a second move straight after the first: what's above it changes, and the scroll takes it up. Otherwise it's what's at the top that stays, where the reader's looking. Says how many points to move the scroll by, with no animation, or `None` if the rows are to slide, since they're in the view and there's nothing to take up.
    pub fn row_change(&self, change: ContentsRowChange, scroll_y: f64) -> Option<f64> {
        let following = self.placed && !self.paused && !self.pointer_inside;
        if following && change.place_shift != 0 {
            return Some(f64::from(change.place_shift) * CONTENTS_ROW_HEIGHT);
        }
        let above = f64::from(change.from) + f64::from(change.removed);
        (scroll_y / CONTENTS_ROW_HEIGHT >= above).then(|| (f64::from(change.inserted) - f64::from(change.removed)) * CONTENTS_ROW_HEIGHT)
    }

    /// Rows above something were added or removed, by this many points, and the host moved its scroll to match so nothing appears to jump. The whole of a move goes with them.
    pub fn shifted(&mut self, by: f64) {
        if let Some(motion) = &mut self.moving {
            motion.target += by;
            motion.aim += by;
            motion.at = motion.at.map(|at| at + by);
        }
    }

    /// The place being read is now on this row. `now` is in seconds, from any fixed moment.
    pub fn reached(&mut self, row: Option<u32>, view: ContentsView, now: f64, reduce_motion: bool) -> FollowMove {
        if self.current == row {
            return FollowMove::Stay;
        }
        self.current = row;
        // Folded: there's nothing to follow, and what's under way stops. Opened again, the place is straight where it should be, and nothing whooshes from the top.
        let Some(row) = row else {
            self.moving = None;
            self.placed = false;
            self.paused = false;
            return FollowMove::Stay;
        };
        if self.paused {
            if self.pointer_inside || now - self.last_user_scroll < FOLLOW_RESUME_AFTER {
                return FollowMove::Stay;
            }
            self.paused = false;
        }
        let place = (f64::from(row) + 0.5) * CONTENTS_ROW_HEIGHT;
        let furthest = (f64::from(view.rows) * CONTENTS_ROW_HEIGHT - view.height).max(0.0);
        let target = (place - view.height * FOLLOW_ANCHOR).clamp(0.0, furthest);
        if !self.placed {
            self.placed = true;
            if let Some((y, following)) = self.resume.take() {
                self.paused = !following;
                return FollowMove::SetTo(y.min(furthest));
            }
            return FollowMove::SetTo(target);
        }
        if self.pointer_inside {
            return FollowMove::Stay;
        }
        if reduce_motion {
            self.moving = None;
            let in_view = place >= view.scroll_y && place <= view.scroll_y + view.height;
            return if in_view { FollowMove::Stay } else { FollowMove::SetTo(target) };
        }
        // A move under way counts from where it's going, so sections going by fast are one long move rather than a row of them, and sections going by slowly are a move and then a rest. If the place has got out of the view itself, the view is trailing: it goes for where the place is now, and faster each time, or scrolling past the speed of the move would lose the place for good.
        if let Some(motion) = &mut self.moving {
            let zone = (motion.target + view.height * FOLLOW_DEAD_ZONE.0)..=(motion.target + view.height * FOLLOW_DEAD_ZONE.1);
            let at = motion.at.unwrap_or(view.scroll_y);
            let in_view = (at..=at + view.height).contains(&place);
            if zone.contains(&place) && in_view {
                return FollowMove::Move;
            }
            motion.target = target;
            if !in_view {
                motion.rate = (motion.rate * FOLLOW_CATCH_UP).min(FOLLOW_FASTEST_RATE);
            }
            return FollowMove::Move;
        }
        let zone = (view.scroll_y + view.height * FOLLOW_DEAD_ZONE.0)..=(view.scroll_y + view.height * FOLLOW_DEAD_ZONE.1);
        if zone.contains(&place) || (target - view.scroll_y).abs() < 1.0 {
            return FollowMove::Stay;
        }
        self.moving = Some(Motion { target, aim: view.scroll_y, at: Some(view.scroll_y), velocity: 0.0, rate: follow_rate(target - view.scroll_y, view.height) });
        FollowMove::Move
    }

    /// One frame of a move. Returns where the scroll should be, or `None` when it's over and the frames should stop. A long pause between frames is taken as a short one, so a stalled frame doesn't look like a jump.
    pub fn step(&mut self, position: f64, dt: f64) -> Option<f64> {
        let motion = self.moving.as_mut()?;
        let mut left = dt.clamp(0.0, 0.05);
        let mut at = *motion.at.get_or_insert(position);
        // Whole steps of one size, and the rest carried in `left` by the last one, so the shape is the same at 60 or 120 frames a second.
        while left > 0.0 {
            let step = left.min(FOLLOW_STEP);
            left -= step;
            motion.aim += (motion.target - motion.aim) * (motion.rate * step).min(1.0);
            let acceleration = motion.rate * motion.rate * (motion.aim - at) - 2.0 * motion.rate * motion.velocity;
            motion.velocity += acceleration * step;
            at += motion.velocity * step;
        }
        motion.at = Some(at);
        if (at - motion.target).abs() < FOLLOW_ARRIVED.0 && motion.velocity.abs() < FOLLOW_ARRIVED.1 {
            let target = motion.target;
            self.moving = None;
            return Some(target);
        }
        Some(at)
    }
}

/// The paper a page is shown on, which has nothing to do with the window's own light or dark. Owner, 2026-11-07: "We have two paper color modes: white and dark, let's add a yellowed white option that's better for the eyes".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    White,
    Warm,
    Dark,
}

/// Warm paper's colour, `0xRRGGBB`. Soft cream, as aged book paper is: warm at a glance without reading as yellow. Taken towards the eye from the first idea of `#F5EEDC`, as a multiply over a page leaves black black, and it holds well over WCAG AA for body text: see the test.
pub const WARM_PAPER: u32 = 0xF5_EE_DC;

impl Paper {
    fn to_record(self) -> PaperName {
        match self {
            Self::White => PaperName::White,
            Self::Warm => PaperName::Warm,
            Self::Dark => PaperName::Dark,
        }
    }

    fn from_record(name: PaperName) -> Self {
        match name {
            PaperName::White => Self::White,
            PaperName::Warm => Self::Warm,
            PaperName::Dark => Self::Dark,
        }
    }

    /// Every paper, in the order they're offered.
    pub const ALL: [Self; 3] = [Self::White, Self::Warm, Self::Dark];

    pub fn name(self) -> &'static str {
        match self {
            Self::White => "white",
            Self::Warm => "warm",
            Self::Dark => "dark",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|paper| paper.name() == name)
    }

    /// What it's called on a menu, when that exists.
    pub fn label(self) -> &'static str {
        match self {
            Self::White => "White",
            Self::Warm => "Warm",
            Self::Dark => "Dark",
        }
    }

    /// With nothing chosen, the paper goes with the window.
    pub fn resolved(chosen: Option<Self>, window_is_dark: bool) -> Self {
        chosen.unwrap_or(if window_is_dark { Self::Dark } else { Self::White })
    }
}

/// WCAG's contrast ratio between two colours, `0xRRGGBB`: from 1 to 21. AA for body text is 4.5.
pub fn contrast_ratio(one: u32, two: u32) -> f64 {
    fn luminance(rgb: u32) -> f64 {
        let channel = |shift: u32| {
            let value = f64::from((rgb >> shift) & 0xFF) / 255.0;
            if value <= 0.039_28 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    let (one, two) = (luminance(one), luminance(two));
    (one.max(two) + 0.05) / (one.min(two) + 0.05)
}

/// What hovering over a row shows: its full text, cleaned and bounded however long or strange it is. A title comes from the file, so it may be 100,000 characters of newlines, and this is the one place the host gets it from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowFullText {
    pub text: String,
    /// It's the file's name, as opposed to a title from inside it.
    pub is_file_name: bool,
}

/// The most of a row's full text that's shown, in characters. Longer is no longer reading a name.
pub const MAX_ROW_FULL_TEXT: usize = 400;

pub fn row_full_text(text: &str, is_file_name: bool) -> RowFullText {
    let clean = recto_core::clean_line(text);
    let mut chars = clean.char_indices().skip(MAX_ROW_FULL_TEXT);
    let text = match chars.next() {
        Some((at, _)) => format!("{}…", clean[..at].trim_end()),
        None => clean,
    };
    RowFullText { text, is_file_name }
}

/// How pages are shown, which is the same for every tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewSettings {
    /// `None` goes with the window: white with a light one, dark with a dark one.
    pub paper: Option<Paper>,
    /// The sidebar's hidden. Owner, 2026-11-08: "simply have a sidebar icon to the right of the traffic light buttons that toggles sidebar on and of. No need mor a separate mode".
    pub sidebar_hidden: bool,
    /// Owner, 2026-11-07: "Add an option in the macos top bar to turn off agent bar (hides the icon)".
    pub agent_icon_hidden: bool,
}

/// A tab's name from its path. It comes from outside: a downloaded file can carry anything in its name, and this reaches the sidebar and agents.
fn tab_name(path: &Path) -> String {
    let file_name = path.file_name().unwrap_or_default();
    let stem = Path::new(file_name).file_stem().unwrap_or(file_name).to_string_lossy();
    match recto_core::clean_line(&stem) {
        name if name.is_empty() => "Untitled".to_string(),
        name => name,
    }
}

/// A closed PDF's history, read for its being opened again. A seam, as this crate has no disk of its own.
pub trait HistorySource: Send + Sync {
    /// What's left of a PDF's history, if any. On whatever thread reads the file, with the session free.
    fn history_of(&self, stamp: Option<FileStamp>, path: &Path) -> Option<TakenHistory>;
    /// A journal of an open tab, read back as a list. Touches nothing. With the session held, so it's a bounded read of a private file.
    fn read_list(&self, name: &str) -> Option<RestoredEdits>;
    /// A tab's sleep file, for this version of its file. Asks nothing of the PDF. With the session held, so it's a bounded read of a private file.
    fn read_sleep(&self, _stamp: FileStamp, _path: &Path) -> Option<sleep::SleepFile> {
        None
    }
}

/// A PDF's history, taken up by a tab.
#[derive(Debug)]
pub struct TakenHistory {
    pub name: String,
    pub restored: RestoredEdits,
}

/// What's waiting to go in journals.
#[derive(Debug, Default)]
pub struct JournalWork {
    pub retired: Vec<String>,
    pub appends: Vec<JournalAppend>,
}

/// How that went. A journal's failing doesn't stop the others, and says nothing about them.
#[derive(Debug, Default)]
pub struct JournalOutcome {
    /// Operations in the file now: which tab, up to how many, and the name it went under, which may not be the one asked for if something's there that isn't to be touched.
    pub written: Vec<(TabId, u64, String)>,
    /// Retired journals that couldn't go, to try again.
    pub not_retired: Vec<String>,
}

/// Operations for one journal, up to this many in all.
#[derive(Clone, Debug)]
pub struct JournalAppend {
    pub tab: TabId,
    pub name: String,
    /// What a new file starts with.
    pub header: JournalHeader,
    pub through: u64,
    pub ops: Vec<JournalOp>,
    /// Its tab has closed, with history and nothing unsaved. Once these are written, it waits for the PDF's next opening, and no launch reads it.
    pub closing: bool,
    /// The whole list, at `through`, for a journal that starts again from it: in one step, in place of anything at this name. A save does this, and clearing a history. `ops` is empty then.
    pub base: Option<record::EditsRecord>,
    /// The journal this one takes over from, which goes once this has got there and not before.
    pub replaces: Option<String>,
}

/// A list brought back from a journal.
#[derive(Debug)]
pub struct RestoredEdits {
    pub edits: PageEdits,
    /// How many operations there have been in all, so what comes next is counted on from there.
    pub seq: u64,
    /// Part of the journal couldn't be read, more than a tail torn by a crash.
    pub lost_lines: bool,
    /// Steps had to go, as a file kept for them isn't what it was.
    pub rolled_off: bool,
}

/// Why a history's being cleared. SPEC decision 25. A confirmed redaction or hidden-data removal, or what's been removed could be got back from it. A file changed outside the app, as its steps no longer apply. A discard, which is all of a tab's edits. And the user's asking for it, with Clear Edit History.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearReason {
    Redaction,
    HiddenDataRemoval,
    FileChanged,
    Discard,
    UserAsked,
}

/// What a tab's sleep file would say, from here.
#[derive(Clone, Debug)]
pub struct SleepFacts {
    pub id: TabId,
    pub path: PathBuf,
    pub sleep: sleep::SleepFile,
    /// No page of the document's from a protected file.
    pub pictures_allowed: bool,
}

/// What's kept of a tab's edits while it's left alone, for a look without opening it.
pub fn edits_summary_of(edits: &PageEdits) -> EditsSummary {
    edits.summary()
}

/// All open tabs and which one is active.
pub struct Session<L = DiskLoader> {
    loader: L,
    view: ViewSettings,
    tabs: Vec<TabRecord>,
    active: Option<TabId>,
    next_id: u32,
    /// Most recently used first, at most [`MAX_WARM`].
    warm: VecDeque<(TabId, Arc<Toc>)>,
    dirs: Vec<Box<Path>>,
    dir_ids: HashMap<Box<Path>, u32>,
    /// Only for tabs with page edits. A tab with none costs nothing here.
    edits: HashMap<TabId, PageEdits>,
    /// Lists that couldn't be read back, to try again, and whose tab says so meanwhile.
    wake_failed: Vec<TabId>,
    /// Lists that have gone to disk, SPEC decision 25: all there is of them in memory is what a look at the tab needs, a cold tab's cost. Woken by [`Self::wake_list`] from their journal.
    asleep: HashMap<TabId, EditsSummary>,
    /// What tabs' `touched` count from.
    started: std::time::Instant,
    /// Tabs being worked on now, once for each thing working on it. Next to none.
    held: Vec<TabId>,
    /// Cold tabs wake from their sleep files here.
    sleep_files: bool,
    just_read: Option<(TabId, sleep::SleepFile)>,
    woken_from_file: Vec<TabId>,
    left_warm: Vec<(TabId, Arc<Toc>)>,
    /// The latest pages drawn or tried, newest last, at most [`MAX_DRAWS_KEPT`]. Lets a waiter find out about a page finished a moment before it asked.
    draws: VecDeque<DrawNote>,
    /// Cold tabs' files, read ahead of waking them with this session free. Each is taken by the next [`Self::activate`] of that tab, and goes if the tab stops being cold. At most [`MAX_READ_AHEAD`].
    woken: Vec<(TabId, Result<DocFacts, OpenError>, Option<FileStamp>)>,
    /// The last edit removed the page a tab was on, so its place moved to the nearest one. Read once with [`Session::take_place_lost`].
    place_lost: bool,
    /// Find, as the reader left it.
    find: FindRecord,
    /// Edit mode. Nothing draws it yet.
    edit_mode: bool,
    /// Whether edits go to a journal. A session with a state record has this, and one without has nothing writing them out.
    journaling: bool,
    /// Each edited tab's journal, by name. Nothing for a tab with no edits.
    journals: HashMap<TabId, Box<str>>,
    /// Lists whose journal has to start again from the whole list: which tab, as of which count, and the journal it takes over from. Next to none, and only until they're written.
    fresh_journals: Vec<(TabId, u64, Option<String>)>,
    /// Journals to remove, as what's in them has been discarded or saved.
    retired_journals: Vec<Box<str>>,
    /// Journals of tabs that have closed, with what hadn't gone out yet. Nothing's lost by closing.
    departing: Vec<JournalAppend>,
    /// Where a PDF's history is, for one being opened.
    history: Option<Arc<dyn HistorySource>>,
    /// An agent is acting at Full, so losing the owner's history is the holder's call. Set for each command.
    agent_at_full: bool,
    /// Tabs being saved now. Nothing changes their edits until the save's done: an edit made while the file was being written would read as saved and not be in it (review). Next to none.
    saving: Vec<TabId>,
    /// Tabs side by side, if there's a comparison: see [`compare`]. The active tab is one of them, the pane being read.
    comparison: Option<compare::Comparison>,
    /// Generated contents the sidebar knows of, for tabs someone asked about: at most [`MAX_GENERATED_VIEWS`], the oldest out, so a thousand tabs can't make this grow.
    generated: Vec<(TabId, GeneratedView)>,
    /// Tabs a save has moved to another version of their file, as `(tab, before, after)`: the pages the new version has are the ones that were shown, each as it was shown, so the page view keeps its layout, place and selection. At most [`MAX_SAVED_OVER`], the oldest out; one is only believed while its tab is still on `after`.
    saved_over: Vec<(TabId, FileStamp, FileStamp)>,
    /// Jumps the page view hasn't taken yet, with how far down the page to land: one per tab at most, the latest.
    jumps: Vec<(TabId, trail::Stop)>,
}

/// Saves remembered for the page view to carry its view over: one per tab saved lately is plenty, as it's asked for straight after.
pub const MAX_SAVED_OVER: usize = 8;

/// Generated contents held for the sidebar. A tab whose contents went out can always look again: they're kept on disk.
pub const MAX_GENERATED_VIEWS: usize = 8;

/// Where a tab's generated contents stand, as the sidebar shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeneratedView {
    /// Asked for, and being made.
    Making,
    /// They couldn't be, for this reason, in Rust's words.
    Failed(&'static str),
    Ready(Arc<Toc>),
    /// Made, and there were no headings to list: a scan with no text, or a document set all in one size.
    NoneFound,
    /// Looked for, for a tab that had chosen them, and they weren't kept. They're offered again.
    NotKept,
}

/// What a tap on the contents' note does. The note is Rust's: its words and what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentsAction {
    Generate,
    ShowGenerated,
    ShowOwn,
    /// Stop making them.
    Stop,
}

impl ContentsAction {
    pub fn id(self) -> &'static str {
        match self {
            Self::Generate => "contents.generate",
            Self::ShowGenerated => "contents.show_generated",
            Self::ShowOwn => "contents.show_own",
            Self::Stop => "contents.stop",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Self::Generate, Self::ShowGenerated, Self::ShowOwn, Self::Stop].into_iter().find(|action| action.id() == id)
    }
}

/// The note over a tab's contents: a short line at rest, and what it means on hover. `None` for contents that are fine and shown as they are. Owner, 2026-11-06: "the left sidebar menu is a bit too complicated (visually noisy)", so it's one quiet line, and the rest waits for the pointer.
pub fn contents_note(verdict: ContentsVerdict, view: Option<&GeneratedView>, choose_generated: bool, own_entries: usize) -> Option<(&'static str, &'static str, Option<ContentsAction>)> {
    const MADE: &str = "Made by RectoPDF from the headings distillPDF found on the pages. They aren't part of the file, which isn't changed, and they can be wrong.";
    match view {
        Some(GeneratedView::Ready(_)) if choose_generated => Some((
            "Generated contents",
            if own_entries > 0 { "Made by RectoPDF from the headings distillPDF found on the pages. They aren't part of the file, which isn't changed, and they can be wrong. Click to show the file's own." } else { MADE },
            (own_entries > 0).then_some(ContentsAction::ShowOwn),
        )),
        Some(GeneratedView::Ready(_)) => Some(("Show generated contents", MADE, Some(ContentsAction::ShowGenerated))),
        Some(GeneratedView::Making) => Some(("Generating contents\u{2026}", "Finding the headings on the pages, in the background. On a long document this can take a little while. Click to stop.", Some(ContentsAction::Stop))),
        Some(GeneratedView::Failed(words)) => Some(("Couldn't generate contents", words, Some(ContentsAction::Generate))),
        Some(GeneratedView::NoneFound) => Some(("No headings found", "RectoPDF looked for headings on the pages and found none to list. A scan with no text has none to find.", None)),
        _ if verdict.offers() || choose_generated => Some((
            "Generate contents",
            match verdict {
                ContentsVerdict::Missing => "This file has no contents of its own. RectoPDF can make some from the headings on its pages. The file isn't changed.",
                _ => "The file's own contents look incomplete. RectoPDF can make some from the headings on its pages, and the file's own stay as they are.",
            },
            Some(ContentsAction::Generate),
        )),
        _ => None,
    }
}

/// What a save writes from: the document as edited, and the file it's about, copied out so the writing needn't hold the session.
#[derive(Clone, Debug)]
pub struct SaveSnapshot {
    pub path: PathBuf,
    pub stamp: Option<FileStamp>,
    pub file_pages: u32,
    pub slots: Vec<edit::PageSlot>,
    pub added: Vec<edit::AddedSource>,
    /// Some unsaved step, done or undone, is the owner's.
    pub holds_owner_edits: bool,
    /// The history needs pages of the file as it is now, which a save may take out of it.
    pub needs_own_pages: bool,
}

/// Files read ahead of waking their tabs, held at once. More than wakes can be under way together.
const MAX_READ_AHEAD: usize = 4;

/// Page results remembered across every tab, so nothing here grows with idle tabs.
pub const MAX_DRAWS_KEPT: usize = 256;

/// That a page was drawn, or couldn't be.
// size: asserted <= 32 by `draw_note_stays_small`. Held per recent draw, at most MAX_DRAWS_KEPT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawNote {
    pub tab: TabId,
    /// Zero-based page.
    pub page: u32,
    /// The failure's stable code, or `None` if it was drawn.
    pub failure: Option<&'static str>,
}

impl Default for Session<DiskLoader> {
    fn default() -> Self {
        Self::new(DiskLoader)
    }
}

impl<L: DocLoader> Session<L> {
    pub fn new(loader: L) -> Self {
        Self { loader, view: ViewSettings::default(), tabs: Vec::new(), active: None, next_id: 1, warm: VecDeque::new(), woken: Vec::new(), dirs: Vec::new(), dir_ids: HashMap::new(), edits: HashMap::new(), wake_failed: Vec::new(), asleep: HashMap::new(), started: std::time::Instant::now(), held: Vec::new(), sleep_files: false, just_read: None, woken_from_file: Vec::new(), left_warm: Vec::new(), draws: VecDeque::new(), place_lost: false, find: FindRecord::default(), edit_mode: false, journaling: false, journals: HashMap::new(), fresh_journals: Vec::new(), retired_journals: Vec::new(), departing: Vec::new(), history: None, agent_at_full: false, generated: Vec::new(), comparison: None, saving: Vec::new(), saved_over: Vec::new(), jumps: Vec::new() }
    }

    pub fn view(&self) -> ViewSettings {
        self.view
    }

    pub fn set_view(&mut self, view: ViewSettings) {
        self.view = view;
    }

    /// How documents are read. Cheap to reach, so a caller can read a file without holding the session.
    pub fn loader(&self) -> &L {
        &self.loader
    }

    pub fn tabs(&self) -> &[TabRecord] {
        &self.tabs
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    pub fn tab(&self, id: TabId) -> Option<&TabRecord> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// The tab's full path, rebuilt from its interned folder and name.
    pub fn path_of(&self, id: TabId) -> Option<PathBuf> {
        let tab = self.tab(id)?;
        Some(self.dirs.get(tab.dir as usize)?.join(&*tab.file_name))
    }

    /// The tabs that keep their contents: the active one and the most recent others, at most [`MAX_WARM`].
    pub fn warm_tabs(&self) -> impl Iterator<Item = TabId> + '_ {
        self.warm.iter().map(|(tab, _)| *tab)
    }

    pub fn tier(&self, id: TabId) -> Tier {
        if self.active == Some(id) {
            Tier::Hot
        } else if self.warm.iter().any(|(tab, _)| *tab == id) {
            Tier::Warm
        } else {
            Tier::Cold
        }
    }

    /// Opens a PDF as a new active tab. **A PDF can only be opened once** (SPEC decision 26): opening a file that already has a tab goes to that tab instead, by its path or as the same file on disk, so a link goes to its tab and a copy is another file.
    pub fn open(&mut self, path: &Path) -> Result<TabId, SessionError> {
        if let Some(id) = self.go_to_open_file(path) {
            return Ok(id);
        }
        // Looked at first, so a change during the read shows as a change.
        let stamp = file_stamp(path).ok();
        if let Some(id) = self.tab_for_file(path, stamp) {
            self.reach_open_file(id, path);
            return Ok(id);
        }
        let facts = self.loader.load(path).map_err(SessionError::Open)?;
        let history = self.history.clone().and_then(|source| source.history_of(stamp, path));
        self.open_with_facts(path, facts, stamp, history)
    }

    /// Goes to the tab this file is open in, if it is, however it's reached. The first half of opening one.
    pub fn go_to_file(&mut self, path: &Path, stamp: Option<FileStamp>) -> Option<TabId> {
        let id = self.tab_for_file(path, stamp)?;
        self.reach_open_file(id, path);
        Some(id)
    }

    /// The tab already showing this file: by its path, or as the same file on disk, which a link is and a copy isn't. A tab whose file couldn't be looked at goes by its path alone.
    ///
    /// **A number is only the same file if the rest says so.** A move and a rewrite in place keep one, and another volume can have the same number under the same device. So its contents have to be what the tab knows, or the tab's own path has to lead to that number now, as it does for a file changed in place. Otherwise it's another PDF, and a tab's edits don't follow it. That stat is only for a tab with this very number, of which there's next to none. A link also leads where it leads: a tab whose file has been replaced under it, with a new number, is still what its own path is.
    pub fn tab_for_file(&self, path: &Path, stamp: Option<FileStamp>) -> Option<TabId> {
        self.tab_for_path(path).or_else(|| {
            let stamp = stamp.filter(|stamp| stamp.inode != 0)?;
            let same_number = self.tabs.iter().find(|tab| {
                tab.stamp.is_some_and(|known| known.inode == stamp.inode && known.device == stamp.device && (known.same_contents(stamp) || self.path_of(tab.id).and_then(|own| file_stamp(&own).ok()).is_some_and(|now| now.inode == stamp.inode && now.device == stamp.device)))
            });
            same_number.map(|tab| tab.id).or_else(|| path.canonicalize().ok().and_then(|real| self.tab_for_path(&real)))
        })
    }

    /// Goes to the tab a file is open in, reached by another name. If the name it was open under has gone, the file moved, and the tab goes with it. A stat on the volume this path has just answered from, so it can't be a dead one.
    fn reach_open_file(&mut self, id: TabId, path: &Path) {
        let gone = self.path_of(id).is_some_and(|known| known != path && !known.exists());
        if gone {
            self.relocate(id, path);
        }
        self.activate(id);
    }

    /// Activates the tab already showing this file, if there is one. The first half of [`Self::open`], for a caller that reads the file without holding the session.
    pub fn go_to_open_file(&mut self, path: &Path) -> Option<TabId> {
        let id = self.tab_for_path(path)?;
        self.activate(id);
        Some(id)
    }

    /// The tab already showing this file, if there is one.
    pub fn tab_for_path(&self, path: &Path) -> Option<TabId> {
        let dir = *self.dir_ids.get(path.parent().unwrap_or(Path::new("")))?;
        let file_name = path.file_name()?;
        self.tabs.iter().find(|t| t.dir == dir && &*t.file_name == file_name).map(|t| t.id)
    }

    /// The second half of [`Self::open`]: makes the tab from facts already read, and what's left of the PDF's history, if a [`HistorySource`] found any. It's the one way a tab comes to exist, so every route has to say. Someone may have opened the same file meanwhile, and then that's the tab, and the history stays where it is.
    pub fn open_with_facts(&mut self, path: &Path, facts: DocFacts, stamp: Option<FileStamp>, history: Option<TakenHistory>) -> Result<TabId, SessionError> {
        if let Some(id) = self.tab_for_file(path, stamp) {
            // Read already, so no read with the session held if it's cold.
            self.give_woken(id, Ok(facts), stamp);
            self.reach_open_file(id, path);
            return Ok(id);
        }
        let dir_path = path.parent().unwrap_or(Path::new(""));
        let file_name = path.file_name().ok_or(SessionError::Open(OpenError::NotFound))?;
        if self.tabs.len() >= MAX_TABS {
            return Err(SessionError::TooManyTabs);
        }
        let dir = self.intern_dir(dir_path);
        if self.next_id >= MAX_TAB_ID {
            return Err(SessionError::TooManyTabs);
        }
        let id = TabId(self.next_id);
        self.next_id += 1;
        let name = tab_name(path);
        self.tabs.push(TabRecord {
            from_sleep: None,
            encrypted: false,
            touched: 0,
            id,
            dir,
            file_name: file_name.into(),
            name: name.into(),
            page: 0,
            page_count: facts.page_count,
            toc_open: true,
            problem: None,
            renderer_only: facts.renderer_only,
            stamp,
            place: None,
            view_rotation: 0,
            spread: record::Spread::Single,
            selection: None,
            notice: None,
            place_dirty: true,
            contents_verdict: ContentsVerdict::Fine,
            generated_contents: false,
            folds: None,
            trail: None,
        });
        if let Some(tab) = self.tab_mut(id) {
            tab.encrypted = facts.encrypted;
        }
        self.remember_warm(id, Arc::new(facts.toc));
        self.active = Some(id);
        self.generated.retain(|(tab, view)| *tab == id || !matches!(view, GeneratedView::Ready(_)));
        // A new PDF shows alone, as going to any tab that isn't side by side does.
        self.comparison = None;
        if let Some(history) = history {
            let rolled_off = history.restored.rolled_off;
            self.take_up_history(id, &history.name, history.restored.edits, history.restored.seq);
            if rolled_off && let Some(tab) = self.tab_mut(id) {
                tab.notice = Some(Box::new(FileNotice::HistoryRolledOff));
            }
        }
        Ok(id)
    }

    /// The tab tapping this one would wake from cold, and its file, so that can be read with this session free: a file on a dead volume mustn't hold up everything else that asks about tabs.
    pub fn wake_for_tap(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        let file = (self.active != Some(id)).then(|| self.file_to_wake(id)).flatten()?;
        // A sleep file says all a read would, with nothing asked of the PDF.
        if self.wake_from_sleep_file(file.0) {
            return None;
        }
        Some(file)
    }

    /// The same, for closing this tab: what becomes active next.
    pub fn wake_for_close(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        if self.active != Some(id) {
            return None;
        }
        let next = match self.pane_after(id) {
            Some(pane) => pane,
            None => {
                let index = self.tabs.iter().position(|tab| tab.id == id)?;
                self.tabs.get(index + 1).or_else(|| index.checked_sub(1).and_then(|before| self.tabs.get(before)))?.id
            }
        };
        self.file_to_wake_now(next)
    }

    /// The pane that's read next when the one being read leaves a comparison, by closing or a ⌘-click: the first of those left. `None` without a comparison, or one that ends with it.
    fn pane_after(&self, leaving: TabId) -> Option<TabId> {
        let comparison = self.comparison.as_ref()?;
        comparison.contains(leaving).then(|| comparison.tabs.iter().copied().find(|tab| *tab != leaving)).flatten()
    }

    /// A cold tab's file to read ahead, unless its sleep file says all a read would.
    fn file_to_wake_now(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        let file = self.file_to_wake(id)?;
        if self.wake_from_sleep_file(file.0) {
            return None;
        }
        Some(file)
    }

    fn file_to_wake(&self, id: TabId) -> Option<(TabId, PathBuf)> {
        if self.warm.iter().any(|(tab, _)| *tab == id) {
            return None;
        }
        self.path_of(id).map(|path| (id, path))
    }

    /// What was read for [`Self::wake_for_tap`] or [`Self::wake_for_close`], kept for that tab's waking. A tab closed or already woken while it was being read has no use for it. Several can be read at once, so they don't take each other's place, and the oldest goes past the cap.
    pub fn give_woken(&mut self, id: TabId, loaded: Result<DocFacts, OpenError>, stamp: Option<FileStamp>) {
        self.woken.retain(|(tab, _, _)| *tab != id);
        if self.file_to_wake(id).is_none() {
            return;
        }
        if self.woken.len() >= MAX_READ_AHEAD {
            drop(self.woken.remove(0));
        }
        self.woken.push((id, loaded, stamp));
    }

    /// **The sidebar's tap rule.** Tapping another PDF switches to it. Tapping the active PDF folds or opens its contents.
    pub fn tap(&mut self, id: TabId) -> Result<(), SessionError> {
        if self.tab(id).is_none() {
            return Err(SessionError::NoSuchTab);
        }
        // A plain click on a tab during a comparison returns to one, that one (owner, 2026-11-08: comparison is by ⌘-click).
        if self.comparison.take().is_some() {
            self.activate(id);
            return Ok(());
        }
        if self.active == Some(id) {
            if let Some(tab) = self.tab_mut(id) {
                tab.toc_open = !tab.toc_open;
            }
        } else {
            self.activate(id);
        }
        Ok(())
    }

    /// Moves a tab to a zero-based page, clamped to the document.
    pub fn go_to(&mut self, id: TabId, page: u32) -> Result<(), SessionError> {
        let pages = self.pages_in(id);
        let tab = self.tab_mut(id).ok_or(SessionError::NoSuchTab)?;
        let page = page.min(pages.saturating_sub(1));
        if tab.page != page {
            tab.page = page;
            tab.place_dirty = true;
        }
        Ok(())
    }

    /// **Where the reader is in a tab,** for a jump to mark: the page, and how far down it if the page view has said lately.
    pub fn here(&self, id: TabId) -> Option<trail::Stop> {
        let tab = self.tab(id)?;
        let within = tab.place.filter(|place| place.page == tab.page && place.anchor_page == tab.page).map_or(0.0, |place| place.within);
        Some(trail::Stop::new(tab.page, within))
    }

    /// **A jump:** following a link, a contents entry, or going to a page by its number or label. Where the reader was is marked for Back, and the way forward goes. The page is held to the document, and the page view lands `within` the way down it.
    pub fn jump(&mut self, id: TabId, page: u32, within: f64) -> Result<(), SessionError> {
        self.jump_as(id, page, within, Author::Owner)
    }

    /// A jump, by the person or an agent. An agent's marks give way to the person's: see [`trail`].
    pub fn jump_as(&mut self, id: TabId, page: u32, within: f64, by: Author) -> Result<(), SessionError> {
        let here = self.here(id).ok_or(SessionError::NoSuchTab)?;
        let to = trail::Stop::new(page.min(self.pages_in(id).saturating_sub(1)), within);
        let tab = self.tab_mut(id).ok_or(SessionError::NoSuchTab)?;
        tab.trail.get_or_insert_default().jumped(here, to, by == Author::Agent);
        self.land(id, to)
    }

    /// Back in a tab's trail, by the person. Says whether there was anywhere to go.
    pub fn go_back(&mut self, id: TabId) -> Result<bool, SessionError> {
        self.step_trail(id, true, Author::Owner)
    }

    /// Forward in a tab's trail, the same way.
    pub fn go_forward(&mut self, id: TabId) -> Result<bool, SessionError> {
        self.step_trail(id, false, Author::Owner)
    }

    /// Back or forward, by the person or an agent.
    pub fn step_as(&mut self, id: TabId, back: bool, by: Author) -> Result<bool, SessionError> {
        self.step_trail(id, back, by)
    }

    fn step_trail(&mut self, id: TabId, back: bool, by: Author) -> Result<bool, SessionError> {
        let here = self.here(id).ok_or(SessionError::NoSuchTab)?;
        let agent = by == Author::Agent;
        let Some(trail) = self.tab_mut(id).and_then(|tab| tab.trail.as_deref_mut()) else { return Ok(false) };
        let Some(to) = (if back { trail.go_back(here, agent) } else { trail.go_forward(here, agent) }) else { return Ok(false) };
        // Pages may have gone since it was marked.
        let to = trail::Stop { page: to.page.min(self.pages_in(id).saturating_sub(1)), ..to };
        self.land(id, to)?;
        Ok(true)
    }

    fn land(&mut self, id: TabId, to: trail::Stop) -> Result<(), SessionError> {
        self.go_to(id, to.page)?;
        self.jumps.retain(|(tab, _)| *tab != id);
        self.jumps.push((id, to));
        Ok(())
    }

    /// Whether a tab can go back, and forward.
    pub fn can_go_back_and_forward(&self, id: TabId) -> (bool, bool) {
        self.trail_of(id).map_or((false, false), |trail| (!trail.back().is_empty(), !trail.forward().is_empty()))
    }

    /// A warm tab's contents and labels, shared: what the host needs to say a page by its label without asking the session again.
    pub fn warm_toc(&self, id: TabId) -> Option<Arc<Toc>> {
        self.warm.iter().find(|(tab, _)| *tab == id).map(|(_, toc)| Arc::clone(toc))
    }

    /// A tab's trail, for QA and `recto where`.
    pub fn trail_of(&self, id: TabId) -> Option<&trail::Trail> {
        self.tab(id)?.trail.as_deref()
    }

    /// The last jump in a tab the page view hasn't taken, with how far down to land. Taken once.
    pub fn take_jump(&mut self, id: TabId) -> Option<trail::Stop> {
        let at = self.jumps.iter().position(|(tab, _)| *tab == id)?;
        Some(self.jumps.swap_remove(at).1)
    }

    /// **What the file calls a page,** zero-based, counting unsaved edits: `iv` for a page of front matter. Nothing for a file without labels, a page added from another file, or a tab whose file isn't read, which shows its number. A moved page keeps its own label, as its file names it.
    pub fn page_label(&self, id: TabId, position: u32) -> Option<String> {
        let (_, toc) = self.warm.iter().find(|(tab, _)| *tab == id)?;
        if toc.labels.is_empty() {
            return None;
        }
        let page = match self.edits.get(&id) {
            None => position,
            Some(edits) => {
                let slot = edits.slots().get(position as usize)?;
                if slot.source != 0 {
                    return None;
                }
                slot.page
            }
        };
        toc.labels.label(page)
    }

    /// A page's label only where it says more than its number: `iv`, but not `4` for the fourth page.
    pub fn page_label_differs(&self, id: TabId, position: u32) -> Option<String> {
        self.page_label(id, position).filter(|label| *label != (u64::from(position) + 1).to_string())
    }

    /// **The words for where a tab is,** as the corner shows them: `12 of 96`, or `iv (4 of 96)` where the file calls the page something else.
    pub fn place_words(&self, id: TabId, page: u32) -> String {
        let plain = format!("{} of {}", page + 1, self.pages_in(id));
        match self.page_label_differs(id, page) {
            Some(label) => format!("{label} ({plain})"),
            None => plain,
        }
    }

    /// **The page typed in the go-to field,** zero-based: a label the file uses first, as Preview and Acrobat read it, then a plain number. Nothing if it's neither.
    pub fn page_for_typed(&self, id: TabId, typed: &str) -> Option<u32> {
        let typed = typed.trim();
        let pages = self.pages_in(id);
        if let Some((_, toc)) = self.warm.iter().find(|(tab, _)| *tab == id)
            && !toc.labels.is_empty()
        {
            let file_pages = self.tab(id).map_or(0, |tab| tab.page_count);
            if let Some(file_page) = toc.labels.find(typed, file_pages) {
                match self.edits.get(&id) {
                    None => return Some(file_page),
                    Some(edits) => {
                        if let Some(position) = edits.slots().iter().position(|slot| slot.source == 0 && slot.page == file_page) {
                            return u32::try_from(position).ok();
                        }
                    }
                }
            }
        }
        typed.parse::<u32>().ok().filter(|page| *page >= 1 && *page <= pages).map(|page| page - 1)
    }

    /// **Where a link's page goes now,** with edits counted: `file_page` of the same file as the page at position `from`, which is what a link on that page means. Nothing if that page isn't in the document any more.
    pub fn position_in_same_file(&self, id: TabId, from: u32, file_page: u32) -> Option<u32> {
        match self.edits.get(&id) {
            None => (file_page < self.pages_in(id)).then_some(file_page),
            Some(edits) => {
                let source = edits.slots().get(from as usize)?.source;
                edits.slots().iter().position(|slot| slot.source == source && slot.page == file_page).and_then(|at| u32::try_from(at).ok())
            }
        }
    }

    /// How many pages a tab has now, counting unsaved edits.
    pub fn pages_in(&self, id: TabId) -> u32 {
        self.edits.get(&id).map(PageEdits::len).or_else(|| self.asleep.get(&id).map(|summary| summary.pages)).or_else(|| self.tab(id).map(|t| t.page_count)).unwrap_or(0)
    }

    /// Whether a tab has unsaved changes.
    pub fn is_edited(&self, id: TabId) -> bool {
        self.edits.get(&id).is_some_and(PageEdits::is_edited) || self.asleep.get(&id).is_some_and(|summary| summary.edited)
    }

    /// **Whether saving this tab's edits writes the file anew:** a page of the file isn't there once, as a delete leaves it. The writer's own rule, `recto_core::save::writer_for`, kept as the list changes, and for a list that's asleep, in its summary.
    pub fn needs_rewrite(&self, id: TabId) -> bool {
        match self.edits.get(&id) {
            Some(list) => list.rewrites(),
            None => self.asleep.get(&id).is_some_and(|summary| summary.edited && summary.rewrites),
        }
    }

    /// Whether the owner's edits on this tab have been undone and can still be redone. Closing would lose them.
    pub fn has_undone_owner_steps(&self, id: TabId) -> bool {
        self.edits.get(&id).is_some_and(PageEdits::owner_steps_undone) || self.asleep.get(&id).is_some_and(|summary| summary.owner_steps_undone)
    }

    /// A tab's page edits, if it has any and they're in memory. A list that's asleep has to be woken first, with [`Self::wake_list`], which everything that's about a tab does: this says `None` of one that's on disk, so anything that reads a tab from here is after that.
    pub fn page_edits(&self, id: TabId) -> Option<&PageEdits> {
        self.edits.get(&id)
    }

    /// The file's pages this tab's history can put back, ascending: what a save keeps a copy of. Nothing for a tab without a history.
    pub fn own_pages_needed(&self, id: TabId) -> Vec<u32> {
        self.edits.get(&id).map(PageEdits::own_pages_needed).unwrap_or_default()
    }

    /// Applies the owner's page edit: rearranging, rotating or deleting. This is what the app's own edits call, with nothing to say whose they are. Everything from the agent interface goes through [`Self::edit_pages_as`] with [`Author::Agent`].
    pub fn edit_pages(&mut self, id: TabId, change: impl FnOnce(&mut PageEdits) -> Result<(), EditError>) -> Result<(), SessionError> {
        self.edit_pages_as(id, Author::Owner, change)
    }

    /// Applies a page edit, noting who made it. The list is made the first time a tab is edited, and goes again if that edit was refused. The reader's place follows its page.
    pub fn edit_pages_as(&mut self, id: TabId, by: Author, change: impl FnOnce(&mut PageEdits) -> Result<(), EditError>) -> Result<(), SessionError> {
        if self.is_saving(id) {
            return Err(SessionError::Saving);
        }
        if !self.wake_list(id) {
            return Err(SessionError::EditsUnreadable);
        }
        self.touch(id);
        let tab = self.tab(id).ok_or(SessionError::NoSuchTab)?;
        if tab.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        let page_count = tab.page_count;
        // Its list is 8 bytes a page, so a file or a record claiming billions has to be stopped here.
        if page_count as usize > edit::MAX_PAGES && !self.edits.contains_key(&id) {
            return Err(SessionError::Edit(EditError::TooManyPages));
        }
        let place = self.place_of(id);
        let journaling = self.journaling;
        let may_push_owner = by == Author::Agent && self.agent_at_full;
        let edits = self.edits.entry(id).or_insert_with(|| PageEdits::new(page_count));
        if journaling {
            edits.journal_from_now();
        }
        edits.act_as(by);
        edits.let_owner_steps_go(may_push_owner);
        let result = change(edits);
        edits.act_as(Author::Owner);
        edits.let_owner_steps_go(false);
        if result.is_ok() {
            self.after_change(id, place);
        } else {
            self.drop_list_if_idle(id);
        }
        result.map_err(SessionError::Edit)
    }

    /// What the reader is looking at: which file, which page there, and where it is now.
    fn place_of(&self, id: TabId) -> Before {
        let source_of = |position: u32| match self.edits.get(&id) {
            Some(edits) => edits.slots().get(position as usize).map(|slot| (slot.source, slot.page)),
            None => Some((0, position)),
        };
        let place = self.tab(id).and_then(|tab| source_of(tab.page).map(|(source, page)| (source, page, tab.page)));
        // Where each stop of the trail's page comes from, so it can follow it through the edit.
        let stops = self.trail_of(id).map(|trail| trail.back().iter().chain(trail.forward()).map(|stop| (stop.page, source_of(stop.page))).collect());
        Before { place, stops }
    }

    /// Everything a change to the page order means: a drawn page's number no longer names that page, and the reader stays on what they were reading.
    fn after_change(&mut self, id: TabId, before: Before) {
        let Before { place, stops } = before;
        // The trail's stops go where their pages went, and a stop whose page went with the edit goes too.
        if let Some(stops) = stops {
            let pages = self.pages_in(id);
            let now: HashMap<u32, Option<u32>> = stops
                .into_iter()
                .map(|(was, source)| {
                    let found = source.and_then(|(source, page)| match self.edits.get(&id) {
                        Some(edits) => edits.slots().iter().position(|slot| slot.source == source && slot.page == page).and_then(|at| u32::try_from(at).ok()),
                        None => (source == 0 && page < pages).then_some(page),
                    });
                    (was, found)
                })
                .collect();
            if let Some(trail) = self.tab_mut(id).and_then(|tab| tab.trail.as_deref_mut()) {
                trail.follow_pages(|page| now.get(&page).copied().flatten());
            }
        }
        self.draws.retain(|draw| draw.tab != id);
        self.drop_list_if_idle(id);
        let pages = self.pages_in(id);
        let found = place.and_then(|(source, page, near)| match self.edits.get(&id) {
            Some(edits) => edits.nearest_position(source, page, near),
            None => (source == 0 && page < pages).then_some(page),
        });
        let near = place.map_or(0, |(_, _, near)| near);
        self.place_lost = place.is_some() && found.is_none();
        let last = pages.saturating_sub(1);
        if let Some(tab) = self.tab_mut(id) {
            tab.page = found.unwrap_or(near).min(last);
            tab.place_dirty = true;
        }
    }

    /// An edit list with nothing in it costs 8 bytes a page for nothing, and would hold the tab to the file's length as it was. One that's back at the file with steps to go back through stays until those are gone.
    fn drop_list_if_idle(&mut self, id: TabId) {
        if self.edits.get(&id).is_some_and(|edits| !edits.is_edited() && !edits.has_history()) {
            self.let_go_of_list(id);
        }
    }

    /// A tab's list is done with: its journal goes, and any files kept for it.
    fn let_go_of_list(&mut self, id: TabId) {
        if let Some(list) = self.edits.remove(&id) {
            // With one that a save said it kept and that hasn't been confirmed, which nothing else names.
            let kept: Vec<Box<Path>> = list.kept_sources().map(|(_, added)| added.path.clone()).chain(list.keeping().filter(|kept| kept.kept).map(|kept| kept.path.clone())).collect();
            self.retire_kept(kept);
        }
        self.fresh_journals.retain(|(tab, _, replaces)| {
            if *tab == id {
                if let Some(old) = replaces {
                    self.retired_journals.push(old.as_str().into());
                }
                return false;
            }
            true
        });
        self.retire_journal(id);
    }

    /// A tab's sleep file goes, as what's in it is done with: its tab's closed, or the file's been changed, and a redaction's what it would otherwise keep.
    fn retire_sleep_file(&mut self, id: TabId) {
        if let (Some(stamp), Some(path)) = (self.tab(id).and_then(|tab| tab.stamp), self.path_of(id)) {
            self.retired_journals.push(record::sleep_file_name(stamp, &path).into());
        }
    }

    fn retire_kept(&mut self, kept: Vec<Box<Path>>) {
        for path in kept {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()).filter(|name| record::is_kept_name(name)) {
                self.retired_journals.push(name.into());
            }
        }
    }

    /// What's in a tab's journal is done with, by a discard or a save, so its file goes: what's been removed mustn't stay recoverable.
    fn retire_journal(&mut self, id: TabId) {
        if let Some(name) = self.journals.remove(&id) {
            self.retired_journals.push(name);
        }
    }

    /// Whether a tab has a list of edits, in memory or on disk.
    fn has_list(&self, id: TabId) -> bool {
        self.edits.contains_key(&id) || self.asleep.contains_key(&id)
    }

    /// Whether a tab's list is on disk now.
    pub fn list_is_asleep(&self, id: TabId) -> bool {
        self.asleep.contains_key(&id)
    }

    /// Seconds of this session so far.
    pub fn seconds_now(&self) -> u32 {
        u32::try_from(self.started.elapsed().as_secs()).unwrap_or(u32::MAX)
    }

    /// Somebody's using this tab, so it's not to go dormant for a while. Agents' commands say so too.
    pub fn touch(&mut self, id: TabId) {
        let now = self.seconds_now();
        self.touch_at(id, now);
    }

    /// [`Self::touch`], at a given time.
    pub fn touch_at(&mut self, id: TabId, now: u32) {
        if let Some(tab) = self.tab_mut(id) {
            tab.touched = now;
        }
    }

    /// The tabs that could go dormant, as they hold something: not the one on screen, and not one a save's under way on. Next to none however many tabs there are: the warm ones, and those with a list in memory.
    fn dormant_candidates(&self) -> impl Iterator<Item = TabId> + '_ {
        self.warm.iter().map(|(id, _)| *id).chain(self.edits.keys().copied()).filter(move |id| !self.on_screen(*id) && !self.held.contains(id) && self.edits.get(id).is_none_or(|list| list.saving().is_none()))
    }

    /// Something's working on this tab, a command of an agent's or an export, so it doesn't go dormant, whatever's asking. As many holds as there are, each let go by [`Self::let_go`].
    pub fn hold(&mut self, id: TabId) {
        self.held.push(id);
    }

    pub fn let_go(&mut self, id: TabId) {
        if let Some(at) = self.held.iter().position(|held| *held == id) {
            self.held.swap_remove(at);
        }
        self.touch(id);
    }

    /// When the next tab comes due to go dormant, in seconds of the session, this long after it was last touched. One deadline for all of them, kept by whoever's waiting anyway. There's no timer per tab.
    pub fn next_dormant_at(&self, after: u32) -> Option<u32> {
        self.dormant_candidates().filter_map(|id| self.tab(id)).map(|tab| tab.touched.saturating_add(after)).min()
    }

    /// The tabs due to go dormant now.
    pub fn due_dormant(&self, now: u32, after: u32) -> Vec<TabId> {
        let mut due: Vec<TabId> = self.dormant_candidates().filter(|id| self.tab(*id).is_some_and(|tab| tab.touched.saturating_add(after) <= now)).collect();
        due.sort_unstable_by_key(|id| id.0);
        due.dedup();
        due
    }

    /// Every tab that could go dormant, oldest touched first, for when a budget or the system won't wait.
    pub fn dormant_oldest_first(&self) -> Vec<TabId> {
        let mut all: Vec<TabId> = self.dormant_candidates().collect();
        all.sort_unstable_by_key(|id| (self.tab(*id).map_or(0, |tab| tab.touched), id.0));
        all.dedup();
        all
    }

    /// Whether a tab may go dormant, whatever's asking: not the one on screen, one being worked on, or one being saved.
    pub fn can_go_dormant(&self, id: TabId) -> bool {
        !self.on_screen(id) && !self.held.contains(&id) && self.edits.get(&id).is_none_or(|list| list.saving().is_none())
    }

    /// What a dormant tab's journal has to say for its list to go to disk: the whole list, to compact to and compare with. `None` if there's no list, if it's not all in the file, or if nobody keeps journals here.
    pub fn list_to_sleep(&self, id: TabId) -> Option<(String, JournalHeader, record::EditsRecord)> {
        if !self.journaling || self.history.is_none() {
            return None;
        }
        let (header, base) = self.checkpoint_if_caught_up(id)?;
        Some((self.journals.get(&id)?.to_string(), header, base))
    }

    /// **A tab goes dormant, SPEC decision 25: it costs a cold tab's memory from here on.** Its contents go, and its list goes if [`Self::list_to_sleep`] says it can, as all of it's on disk. Said once for each, and read back only if it's touched again. Says whether the list went, or there wasn't one.
    pub fn go_dormant(&mut self, id: TabId) -> bool {
        // A dormant tab holds no list: its generated contents are on disk, and found again when it's shown.
        self.generated.retain(|(tab, _)| *tab != id);
        if !self.can_go_dormant(id) {
            return false;
        }
        self.warm.retain(|(tab, _)| *tab != id);
        self.woken.retain(|(tab, _, _)| *tab != id);
        self.draws.retain(|draw| draw.tab != id);
        !self.edits.contains_key(&id) || self.put_list_to_sleep(id)
    }

    fn put_list_to_sleep(&mut self, id: TabId) -> bool {
        if self.list_to_sleep(id).is_none() {
            return false;
        }
        let Some(list) = self.edits.remove(&id) else { return false };
        self.asleep.insert(id, list.summary());
        true
    }

    /// **Brings a tab's list back from its journal, if it's on disk.** Everything about the tab comes here first, so nothing can act on a list it can't see. **If it can't be read just now, it stays asleep, its name kept, to be tried again by whatever's next,** and says false. Nothing goes on that hasn't got a list to go on: a new edit laid on a journal it never read would land on the wrong pages. The tab says so till then. Only a discard or a confirmed save gives up on it.
    pub fn wake_list(&mut self, id: TabId) -> bool {
        let Some(&summary) = self.asleep.get(&id) else { return true };
        let restored = self.journals.get(&id).and_then(|name| self.history.as_ref()?.read_list(name));
        match restored {
            Some(restored) if restored.edits.summary() == summary || restored.rolled_off => {
                self.asleep.remove(&id);
                let mut edits = restored.edits;
                edits.set_journal_seq(restored.seq);
                self.edits.insert(id, edits);
                let said_unreadable = self.wake_failed.contains(&id);
                self.wake_failed.retain(|failed| *failed != id);
                if let Some(tab) = self.tab_mut(id) {
                    if said_unreadable && matches!(tab.notice.as_deref(), Some(FileNotice::EditsUnreadable)) {
                        tab.notice = None;
                    }
                    if restored.rolled_off {
                        tab.notice = Some(Box::new(FileNotice::HistoryRolledOff));
                    }
                }
                // Steps have gone that the journal has, so it has to start again from the list, or it wouldn't read back as this.
                if restored.rolled_off {
                    self.fresh_journals.retain(|(tab, _, _)| *tab != id);
                    self.fresh_journals.push((id, restored.seq, None));
                }
                true
            }
            _ => {
                if !self.wake_failed.contains(&id) {
                    self.wake_failed.push(id);
                }
                if let Some(tab) = self.tab_mut(id) {
                    tab.notice = Some(Box::new(FileNotice::EditsUnreadable));
                }
                false
            }
        }
    }

    /// A list that couldn't be read is given up on, as what's being done says it's done with: its journal goes.
    fn give_up_on_asleep(&mut self, id: TabId) {
        if self.asleep.remove(&id).is_some() {
            self.wake_failed.retain(|failed| *failed != id);
            self.retire_journal(id);
        }
    }

    /// Whether there's a step to undo and one to redo, wherever its list is. With nothing read.
    pub fn undo_and_redo(&self, id: TabId) -> (bool, bool) {
        self.edits.get(&id).map(|list| (list.can_undo(), list.can_redo())).or_else(|| self.asleep.get(&id).map(|summary| (summary.undo > 0, summary.redo > 0))).unwrap_or((false, false))
    }

    /// A tab's list whole, wherever it is, for QA and a look: one on disk is read for this and stays there.
    pub fn list_record_anywhere(&self, id: TabId) -> Option<record::EditsRecord> {
        self.edits_record(id).or_else(|| {
            self.asleep.contains_key(&id).then_some(())?;
            Some(self.history.as_ref()?.read_list(self.journals.get(&id)?)?.edits.to_record())
        })
    }

    /// What's asleep, by journal, for reading back: each has to read as what's known of it here.
    pub fn asleep_by_journal(&self) -> HashMap<String, EditsSummary> {
        self.asleep.iter().filter_map(|(id, summary)| Some((self.journals.get(id)?.to_string(), *summary))).collect()
    }

    /// What a tab's sleep file would say of it, apart from what the page view knows, if it can have one: its file's been read this session, so its contents are here, and it isn't protected, as this would be those in the clear.
    pub fn sleep_facts(&self, id: TabId) -> Option<SleepFacts> {
        let (_, toc) = self.warm.iter().find(|(tab, _)| *tab == id)?;
        self.sleep_facts_with(id, toc)
    }

    fn sleep_facts_with(&self, id: TabId, toc: &Toc) -> Option<SleepFacts> {
        let tab = self.tab(id)?;
        let stamp = tab.stamp?;
        // What's come from a sleep file and hasn't been checked against the PDF isn't written again as if it had. Nor anything of a file that's not what it was.
        if tab.encrypted || tab.from_sleep.is_some() || self.asleep.contains_key(&id) || matches!(tab.notice.as_deref(), Some(FileNotice::Changed | FileNotice::Missing | FileNotice::NeedsPassword)) {
            return None;
        }
        let list = self.edits.get(&id);
        let (pages, list_seq) = list.map_or((tab.page_count, 0), |list| (list.len(), list.order_mark()));
        let sleep = sleep::SleepFile { file: stamp, file_pages: tab.page_count, pages, list_seq, renderer_only: tab.renderer_only, version: None, toc: toc.clone(), usual: None, sizes: Vec::new(), pictures: Vec::new() };
        // A page from a protected file on any page means pictures of none, and it's the same for each of them.
        Some(SleepFacts { id, path: self.path_of(id)?, sleep, pictures_allowed: !list.is_some_and(PageEdits::has_protected_pages) })
    }

    /// What a tab's pictures are about, as the session has it, for telling if a sleep file's are.
    pub fn list_mark(&self, id: TabId) -> u64 {
        self.edits.get(&id).map_or(0, PageEdits::order_mark)
    }

    /// The same for a tab that's just been woken some other way, for a picture for its first frame.
    pub fn sleep_to_read_awake(&self, id: TabId) -> Option<(FileStamp, PathBuf)> {
        let tab = self.tab(id)?;
        if !self.sleep_files || tab.encrypted {
            return None;
        }
        Some((tab.stamp?, self.path_of(id)?))
    }

    /// What's on hand to wake a cold tab from its sleep file: which file, as far as anything's known, without looking at it. `None` for a tab that isn't cold.
    pub fn sleep_to_read(&self, id: TabId) -> Option<(FileStamp, PathBuf)> {
        if self.warm.iter().any(|(tab, _)| *tab == id) {
            return None;
        }
        let tab = self.tab(id)?;
        if tab.encrypted || matches!(tab.notice.as_deref(), Some(FileNotice::NeedsPassword)) {
            return None;
        }
        Some((tab.stamp?, self.path_of(id)?))
    }

    /// Whether cold tabs wake from their sleep files here: only with someone to look at the file on the way past, and tell [`Self::apply_file_status`] if it's not what it was, by [`Self::take_woken_from_file`].
    pub fn set_sleep_files(&mut self, on: bool) {
        self.sleep_files = on;
    }

    /// **Wakes a cold tab from its sleep file, if it has one about its file as last known.** Says whether it did. Every route to a cold tab's through here, or through a read of its PDF if not.
    pub fn wake_from_sleep_file(&mut self, id: TabId) -> bool {
        if !self.sleep_files {
            return false;
        }
        let Some((stamp, path)) = self.sleep_to_read(id) else { return false };
        let Some(sleep) = self.history.as_ref().and_then(|source| source.read_sleep(stamp, &path)) else { return false };
        self.wake_from_sleep(id, &sleep);
        self.just_read = Some((id, sleep));
        self.woken_from_file.push(id);
        true
    }

    /// The tabs woken from sleep files, whose files nobody's looked at yet.
    pub fn take_woken_from_file(&mut self) -> Vec<TabId> {
        std::mem::take(&mut self.woken_from_file)
    }

    /// The sleep file a tab was just woken from, for its first frame.
    pub fn take_sleep_read(&mut self, id: TabId) -> Option<sleep::SleepFile> {
        self.just_read.take().and_then(|(tab, sleep)| (tab == id).then_some(sleep))
    }

    pub(crate) fn wake_from_sleep(&mut self, id: TabId, sleep: &sleep::SleepFile) {
        let facts = DocFacts { page_count: sleep.file_pages, byte_len: sleep.file.size, encrypted: false, version: "".into(), title: None, toc: sleep.toc.clone(), renderer_only: sleep.renderer_only };
        self.give_woken(id, Ok(facts), Some(sleep.file));
        if let Some(tab) = self.tab_mut(id) {
            tab.from_sleep = Some(sleep.version.unwrap_or(0));
        }
    }

    /// **The worker has the file open, and says how long it is.** A sleep file's word goes only until now. If it's said something else, it's gone, and the file's read: says so, for that to be done. A list's own length is compared with what that read finds, on its way in. A file read in the usual way isn't looked at, as the two readers of a damaged PDF can differ, and neither is wrong.
    pub fn file_length_is(&mut self, id: TabId, pages: u32, version: u64) -> bool {
        let Some(tab) = self.tab_mut(id) else { return false };
        let Some(said) = tab.from_sleep.take() else { return false };
        // The same length and, if it said, the same version: it was right. A sleep file that didn't say is right about the length only, which is all it's been used for, its pictures having gone if they weren't.
        if tab.page_count == pages && (said == 0 || said == version) {
            return false;
        }
        self.retire_sleep_file(id);
        self.forget_read(id);
        true
    }

    /// What was read of a tab goes, so it's read again: its file's turned out to be something else.
    pub fn forget_read(&mut self, id: TabId) {
        self.generated.retain(|(tab, _)| *tab != id);
        if let Some(tab) = self.tab_mut(id) {
            tab.from_sleep = None;
        }
        self.warm.retain(|(tab, _)| *tab != id);
        self.woken.retain(|(tab, _, _)| *tab != id);
    }

    /// Whether edits go to a journal from now on. For whoever keeps the state record.
    pub fn set_journaling(&mut self, on: bool) {
        self.journaling = on;
    }

    /// An agent's next edits are at Full, where the owner's history can be pushed out, or not.
    pub fn set_agent_at_full(&mut self, full: bool) {
        self.agent_at_full = full;
    }

    /// What's waiting to go in journals: which to remove, and what to append to which. Copied, not taken: operations stay until [`Self::journal_outcome`] says they're in the file, so nothing's lost if the disk refuses, and nothing goes twice if one journal fails and another doesn't. A list's first time here gets its journal's name.
    pub fn journal_work(&mut self) -> JournalWork {
        let ids: Vec<TabId> = self.edits.iter().filter(|(id, edits)| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()) || self.fresh_journals.iter().any(|(tab, _, _)| tab == *id)).map(|(id, _)| *id).collect();
        let mut appends = self.departing.clone();
        for id in ids {
            if let Some(append) = self.append_for(id) {
                appends.push(append);
            }
        }
        JournalWork { retired: std::mem::take(&mut self.retired_journals).into_iter().map(String::from).collect(), appends }
    }

    fn append_for(&mut self, id: TabId) -> Option<JournalAppend> {
        let name = self.journal_name_for(id);
        let (tab, path, edits) = (self.tab(id)?, self.path_of(id)?, self.edits.get(&id)?);
        let (through, ops) = edits.journal_waiting()?;
        let mut header = JournalHeader::new(id.0, &path, tab.stamp, tab.page_count, 0);
        header.saving = edits.saving().map(Into::into);
        // A journal starting again has all of it in its first line, whatever's come since.
        if let Some((_, _, replaces)) = self.fresh_journals.iter().find(|(tab, _, _)| *tab == id) {
            header.keeping = edits.keeping().map(edit::AddedSource::to_record);
            return Some(JournalAppend { tab: id, name, header, through, ops: Vec::new(), closing: false, base: Some(edits.to_record()), replaces: replaces.clone() });
        }
        Some(JournalAppend { tab: id, name, header, through, ops: ops.to_vec(), closing: false, base: None, replaces: None })
    }

    /// What came of [`Self::journal_work`]: what's in the file can go from here.
    pub fn journal_outcome(&mut self, outcome: JournalOutcome) {
        for (id, through, name) in outcome.written {
            self.departing.retain(|append| !(append.tab == id && append.through == through));
            self.fresh_journals.retain(|(tab, seq, _)| !(*tab == id && *seq <= through));
            if let Some(edits) = self.edits.get_mut(&id) {
                edits.journal_written(through);
                self.journals.insert(id, name.into());
            }
        }
        self.retired_journals.extend(outcome.not_retired.into_iter().map(String::into_boxed_str));
    }

    /// A list as a whole for a journal's new checkpoint, with its header. Only if nothing's still waiting to go in the file, which the checkpoint would pass.
    pub fn checkpoint_if_caught_up(&self, id: TabId) -> Option<(JournalHeader, record::EditsRecord)> {
        let edits = self.edits.get(&id)?;
        let (seq, waiting) = edits.journal_waiting()?;
        if !waiting.is_empty() || self.fresh_journals.iter().any(|(tab, _, _)| *tab == id) {
            return None;
        }
        let tab = self.tab(id)?;
        let mut header = JournalHeader::new(id.0, &self.path_of(id)?, tab.stamp, tab.page_count, seq);
        // A save may be about to land, and a checkpoint mustn't lose that it said so.
        header.saving = edits.saving().map(Into::into);
        header.keeping = edits.keeping().map(edit::AddedSource::to_record);
        Some((header, edits.to_record()))
    }

    /// Every list as a whole, with its journal's name, for a session that keeps no journal as it goes and is told to save.
    pub fn checkpoints(&mut self) -> Vec<(String, JournalHeader, record::EditsRecord)> {
        let ids: Vec<TabId> = self.edits.keys().copied().collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let name = self.journal_name_for(id);
            let (Some(tab), Some(path), Some(list)) = (self.tab(id), self.path_of(id), self.edits_record(id)) else { continue };
            out.push((name, JournalHeader::new(id.0, &path, tab.stamp, tab.page_count, 0), list));
        }
        out
    }

    /// Every list in full by its journal's name, for reading back.
    pub fn lists_by_journal(&self) -> HashMap<String, record::EditsRecord> {
        self.journals.iter().filter_map(|(id, name)| Some((name.to_string(), self.edits.get(id)?.to_record()))).collect()
    }

    /// Whether there's anything for [`Self::journal_work`], which is asked often and costs a look per edited tab.
    pub fn has_journal_work(&self) -> bool {
        !self.retired_journals.is_empty() || !self.departing.is_empty() || !self.fresh_journals.is_empty() || self.edits.values().any(|edits| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()))
    }

    /// Whether a journal has been retired, or a tab with one has closed, which wants writing at once.
    pub fn has_retired_journals(&self) -> bool {
        !self.retired_journals.is_empty() || !self.departing.is_empty() || !self.fresh_journals.is_empty()
    }

    /// Those operations are in the file.
    pub fn journal_written(&mut self, id: TabId, through: u64) {
        if let Some(edits) = self.edits.get_mut(&id) {
            edits.journal_written(through);
        }
    }

    /// A journal's name for this tab, from its PDF's identity.
    fn journal_name_for(&mut self, id: TabId) -> String {
        if let Some(name) = self.journals.get(&id) {
            return name.to_string();
        }
        let (stamp, path) = (self.tab(id).and_then(|tab| tab.stamp), self.path_of(id).unwrap_or_default());
        let name = record::journal_file_name(stamp, &path, None);
        self.journals.insert(id, name.as_str().into());
        name
    }

    /// Where a PDF's history is read from, for one being opened on any route.
    pub fn set_history_source(&mut self, source: Arc<dyn HistorySource>) {
        self.history = Some(source);
    }

    /// Reads what's left of a PDF's history, for [`Self::open_with_facts`], with the session free: hold this and call it outside. `None` if there's no such thing here.
    pub fn history_source(&self) -> Option<Arc<dyn HistorySource>> {
        self.history.clone()
    }

    /// A PDF opened again picks up the history it was left with. A tab with a list of its own keeps that, and a journal has one owner.
    pub fn take_up_history(&mut self, id: TabId, name: &str, mut edits: PageEdits, seq: u64) {
        if self.edits.contains_key(&id) || self.tab(id).is_none() || self.journals.values().any(|taken| **taken == *name) {
            return;
        }
        if self.journaling {
            edits.set_journal_seq(seq);
        } else {
            edits.no_journal();
        }
        self.edits.insert(id, edits);
        self.journals.insert(id, name.into());
    }

    /// **The one method a confirmed Save or Save As calls, and it marks a saved point.** Owner, 2026-11-07: "after a save you might still have regrets and want to make changes. But you definitely don't want history to leak into the pdf file". What was written is the file now, so the document reads as saved, and the history goes on: undoing past this makes it unsaved again, and back to here reads as saved. A failed save calls nothing, and everything stays. `saved_as` moves the tab if it went to another file, and the history goes with it. `kept` is what [`Self::save_prepared`] was told, and a Save As has the old file itself for that. What a step can't get pages from rolls off: see [`PageEdits::rebase_on_saved`], and the tab says so. The steps around it, and what a crash at each does, are in `docs/ENGINEERING.md`. Says how many steps rolled off.
    pub fn saved(&mut self, id: TabId, now: FileStamp, pages: u32, saved_as: Option<&Path>, kept: Option<edit::AddedSource>) -> usize {
        // Another version of the file now: the old one's generated contents and folds aren't about it.
        self.generated.retain(|(tab, _)| *tab != id);
        if let Some(tab) = self.tab_mut(id) {
            tab.folds = None;
        }
        if !self.wake_list(id) {
            self.give_up_on_asleep(id);
        }
        let kept = kept.or_else(|| {
            saved_as?;
            let tab = self.tab(id)?;
            Some(edit::AddedSource { path: self.path_of(id)?.into_boxed_path(), page_count: tab.page_count, stamp: tab.stamp, kept: false, protected: tab.encrypted, holds: None })
        });
        self.retire_sleep_file(id);
        let rebased = self.edits.get_mut(&id).map(|list| list.rebase_on_saved(pages, kept.clone()));
        let mut rolled_off = 0;
        match rebased {
            // There's no list, so no history. A file kept for it isn't needed.
            None => self.retire_kept(kept.filter(|kept| kept.kept).map(|kept| kept.path).into_iter().collect()),
            // Something else was written, so nothing in the history's about that file.
            Some(None) => {
                rolled_off = self.edits.get(&id).map_or(0, |list| list.summary().undo as usize + list.summary().redo as usize);
                self.retire_kept(kept.filter(|kept| kept.kept).map(|kept| kept.path).into_iter().collect());
                self.let_go_of_list(id);
            }
            Some(Some(rebased)) => {
                rolled_off = rebased.rolled_off;
                self.retire_kept(rebased.let_go);
            }
        }
        if let Some(path) = saved_as {
            self.relocate(id, path);
        }
        // What's in the new version is what was shown, page for page, so the page view goes on from where it was.
        self.saved_over.retain(|(tab, _, _)| *tab != id);
        if let Some(before) = self.tab(id).and_then(|tab| tab.stamp) {
            if self.saved_over.len() >= MAX_SAVED_OVER {
                self.saved_over.remove(0);
            }
            self.saved_over.push((id, before, now));
        }
        if let Some(tab) = self.tab_mut(id) {
            tab.stamp = Some(now);
            tab.page_count = pages;
            tab.page = tab.page.min(pages.saturating_sub(1));
            tab.notice = (rolled_off > 0).then(|| Box::new(FileNotice::HistoryRolledOff));
            tab.problem = None;
            tab.place_dirty = true;
        }
        self.draws.retain(|draw| draw.tab != id);
        if self.edits.get(&id).is_some_and(|list| !list.has_history()) {
            self.let_go_of_list(id);
        } else if self.edits.contains_key(&id) {
            // A journal goes by its file's identity, which this has changed. The old one goes only once the new one's there, so a crash in between has one of them.
            let replaces = self.journals.remove(&id).map(String::from);
            self.fresh_journals.retain(|(tab, _, _)| *tab != id);
            if self.journaling {
                let seq = self.edits.get(&id).map_or(0, PageEdits::journal_seq);
                self.journal_name_for(id);
                self.fresh_journals.push((id, seq, replaces));
            } else if let Some(old) = replaces {
                self.retired_journals.push(old.into());
            }
        }
        rolled_off
    }

    /// **Starts a save of this tab:** what's to be written, copied out, and the tab held so that nothing changes its edits until [`Self::end_save`]. `None` if there's nothing unsaved. Refused while another save of it runs, for a file only the renderer can read, and for edits that can't be read back.
    pub fn begin_save(&mut self, id: TabId, even_unedited: bool) -> Result<Option<SaveSnapshot>, SessionError> {
        if self.is_saving(id) {
            return Err(SessionError::Saving);
        }
        let tab = self.tab(id).ok_or(SessionError::NoSuchTab)?;
        if tab.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        if !self.wake_list(id) {
            return Err(SessionError::EditsUnreadable);
        }
        let tab = self.tab(id).ok_or(SessionError::NoSuchTab)?;
        let path = self.path_of(id).ok_or(SessionError::NoSuchTab)?;
        let snapshot = match self.edits.get(&id).filter(|list| list.is_edited()) {
            Some(list) => SaveSnapshot { path, stamp: tab.stamp, file_pages: list.file_pages(), slots: list.slots().to_vec(), added: list.added_sources().to_vec(), holds_owner_edits: list.holds_owner_edits(), needs_own_pages: list.needs_own_pages() },
            // A Save As of the document as it is: every page in its place.
            None if even_unedited => {
                let pages = tab.page_count;
                SaveSnapshot { path, stamp: tab.stamp, file_pages: pages, slots: (0..pages).map(|page| edit::PageSlot { source: 0, page, quarter_turns: 0 }).collect(), added: Vec::new(), holds_owner_edits: false, needs_own_pages: false }
            }
            None => return Ok(None),
        };
        self.saving.push(id);
        self.hold(id);
        Ok(Some(snapshot))
    }

    /// A save of this tab has finished, however it went. One that didn't put a file in place calls this without [`Self::saved`], and what [`Self::save_prepared`] noted goes: the edits stand, as they were.
    pub fn end_save(&mut self, id: TabId) {
        if let Some(at) = self.saving.iter().position(|saving| *saving == id) {
            self.saving.remove(at);
            self.let_go(id);
            if let Some(list) = self.edits.get_mut(&id)
                && list.saving().is_some()
            {
                let kept = list.save_abandoned();
                self.retire_kept(kept.into_iter().collect());
            }
        }
    }

    /// The name of this tab's journal, if it has one, for checking what's reached the disk.
    pub fn journal_of(&self, id: TabId) -> Option<&str> {
        self.journals.get(&id).map(|name| &**name)
    }

    /// **The close a prompt's Save asked for, once the save is done.** If an edit or an undo came in since, the file doesn't have it, so the tab stays open and this says `false`: nothing is lost unasked. Otherwise it closes, letting go of undone steps, as the prompt said they would go.
    pub fn close_after_save(&mut self, id: TabId) -> Result<bool, SessionError> {
        if self.is_edited(id) {
            return Ok(false);
        }
        if self.has_undone_owner_steps(id) {
            self.discard_edits(id);
        }
        self.close(id).map(|()| true)
    }

    /// Whether a save of this tab is under way.
    pub fn is_saving(&self, id: TabId) -> bool {
        self.saving.contains(&id)
    }

    /// Whether any save is under way, which a hand-off waits for.
    pub fn any_saving(&self) -> bool {
        !self.saving.is_empty()
    }

    /// The first step of a save, before anything's put in place: notes what the file will be if it works, and what's been kept for undoing past it, so that a crash between that and [`Self::saved`] can tell. Goes in the journal. **A save that removes something for good, as a redaction does, keeps nothing:** a copy of the file would hold what it's removing.
    pub fn save_prepared(&mut self, id: TabId, target: FileStamp, keeping: Option<&edit::AddedSource>) {
        self.wake_list(id);
        if let Some(edits) = self.edits.get_mut(&id) {
            edits.note_saving(target, keeping);
        }
    }

    /// **Clears a tab's history, and removes its files rather than only forgetting them.** With unsaved edits, the page order stays, as only the way back goes, unless it's a discard, which is all of it. With none, there's nothing left to hold, so the list goes too. Says whether there was any.
    pub fn clear_history(&mut self, id: TabId, reason: ClearReason) -> bool {
        // Nothing's taken from under a save: what it writes is what it was given, and its `saved` rebases this list.
        if self.is_saving(id) {
            return false;
        }
        if !self.wake_list(id) {
            self.give_up_on_asleep(id);
            if let Some(tab) = self.tab_mut(id) {
                tab.notice = None;
            }
            return true;
        }
        // What's gone mustn't stay in a picture of it.
        if matches!(reason, ClearReason::Redaction | ClearReason::HiddenDataRemoval) {
            self.retire_sleep_file(id);
        }
        let Some(list) = self.edits.get_mut(&id) else { return false };
        if reason != ClearReason::Discard && list.is_edited() {
            let unconfirmed = list.keeping().filter(|kept| kept.kept).map(|kept| kept.path.clone());
            let let_go: Vec<Box<Path>> = list.drop_history().into_iter().chain(unconfirmed).collect();
            let seq = list.journal_seq();
            let journaled = self.journaling;
            self.retire_kept(let_go);
            if journaled {
                self.fresh_journals.retain(|(tab, _, _)| *tab != id);
                // The same name as it has now, as the file's the same, replaced in one step.
                self.fresh_journals.push((id, seq, None));
            }
            return true;
        }
        let place = self.place_of(id);
        self.let_go_of_list(id);
        if let Some(tab) = self.tab_mut(id)
            && matches!(tab.notice.as_deref(), Some(FileNotice::Changed | FileNotice::EditsUnreadable | FileNotice::HistoryRolledOff))
        {
            tab.notice = None;
        }
        self.after_change(id, place);
        true
    }

    fn relocate(&mut self, id: TabId, path: &Path) {
        let dir = self.intern_dir(path.parent().unwrap_or(Path::new("")));
        let name = tab_name(path);
        if let (Some(tab), Some(file_name)) = (self.tab_mut(id), path.file_name()) {
            tab.dir = dir;
            tab.file_name = file_name.into();
            tab.name = name.into();
        }
    }

    /// Find, as the reader left it.
    pub fn find(&self) -> &FindRecord {
        &self.find
    }

    /// Sets what's in Find. Longer text than [`record::MAX_FIND_BYTES`] is cut at a character.
    pub fn set_find(&mut self, open: bool, query: &str, draft: &str) {
        let cut = |text: &str| {
            let mut end = text.len().min(record::MAX_FIND_BYTES);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text[..end].to_string()
        };
        let (current, current_tab) = if query == self.find.query { (self.find.current, self.find.current_tab) } else { (None, None) };
        self.find = FindRecord { open, query: cut(query), draft: cut(draft), current, current_tab };
    }

    /// The hit Find went to last, by its place, for the record.
    pub fn set_find_current(&mut self, current: Option<(u32, u32)>, tab: Option<u32>) {
        self.find.current = current;
        self.find.current_tab = tab;
    }

    pub fn edit_mode(&self) -> bool {
        self.edit_mode
    }

    pub fn set_edit_mode(&mut self, on: bool) {
        self.edit_mode = on;
    }

    /// Turns the view of a tab, by quarter turns. Nothing draws it yet.
    pub fn set_view_rotation(&mut self, id: TabId, quarter_turns: u8) {
        if let Some(tab) = self.tab_mut(id) {
            tab.view_rotation = quarter_turns % 4;
        }
    }

    /// Lays a tab's pages out another way. Says whether that changed anything.
    pub fn set_spread(&mut self, id: TabId, spread: record::Spread) -> bool {
        match self.tab_mut(id) {
            Some(tab) if tab.spread != spread => {
                tab.spread = spread;
                true
            }
            _ => false,
        }
    }

    /// How a tab's pages are laid out.
    pub fn spread_of(&self, id: TabId) -> Option<record::Spread> {
        self.tab(id).map(|tab| tab.spread)
    }

    /// What's selected in a tab. Carried across a restart, and nothing else here reads it.
    pub fn set_selection(&mut self, id: TabId, selection: Option<Selection>) {
        if let Some(tab) = self.tab_mut(id) {
            tab.selection = selection.map(Box::new);
        }
    }

    /// What's selected in the tab showing, as the page view says. Only that one holds a selection: one that's left has lost its.
    pub fn note_selection(&mut self, showing: TabId, selection: Option<Selection>) {
        for tab in &mut self.tabs {
            if tab.selection.is_some() && tab.id != showing {
                tab.selection = None;
            }
        }
        self.set_selection(showing, selection);
    }

    /// What's selected in a tab.
    pub fn selection_of(&self, id: TabId) -> Option<Selection> {
        self.tab(id)?.selection.as_deref().copied()
    }

    /// Exactly where a tab's reader is, as the page view says. Marked to be written only if it's moved.
    pub fn note_place(&mut self, id: TabId, place: PlaceRecord) {
        if let Some(tab) = self.tab_mut(id)
            && tab.place != Some(place)
        {
            tab.place = Some(place);
            tab.place_dirty = true;
        }
    }

    /// Where to bring a tab back to: only if it's still about the page the tab is on, as a `goto` while it was cold leaves it not.
    pub fn place_to_show(&self, id: TabId) -> Option<PlaceRecord> {
        let tab = self.tab(id)?;
        tab.place.filter(|place| place.page == tab.page)
    }

    /// The tab's notice, if there is one.
    pub fn tab_notice(&self, id: TabId) -> Option<&FileNotice> {
        self.tab(id)?.notice.as_deref()
    }

    /// Words for what's wrong with a tab's file, if anything is.
    pub fn tab_notice_message(&self, id: TabId) -> Option<&'static str> {
        tab_notice_words(self.tab(id)?, self.has_list(id))
    }

    /// What a look at a tab's file found, from [`record::look_at_file`], made on whatever thread could afford it. Nothing's written to any PDF. A file that moved is followed. One that's changed under edits keeps the stamp they were made on, so the edits' own checks go on saying so.
    pub fn apply_file_status(&mut self, id: TabId, status: FileStatus) {
        // A file changed or gone takes its generated contents with it, so the sidebar shows what an agent's `contents --generated` finds, which goes by the file as it is now.
        if matches!(status, FileStatus::Changed(_) | FileStatus::Missing) {
            self.generated.retain(|(tab, _)| *tab != id);
            // Folds are by entry number, which another version's contents don't share.
            if let Some(tab) = self.tab_mut(id) {
                tab.folds = None;
            }
        }
        let has_edits = self.has_list(id);
        match status {
            FileStatus::NotLooked => {}
            FileStatus::Same(now) => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.stamp = Some(now);
                    // Back to what edits are about. With none, the stamp's moved on to what the file is now, so being the same says nothing: it goes on saying it changed until it's been read, and left.
                    if matches!(tab.notice.as_deref(), Some(FileNotice::Missing)) || (has_edits && matches!(tab.notice.as_deref(), Some(FileNotice::Changed))) {
                        tab.notice = None;
                    }
                }
            }
            FileStatus::Moved(path, now) => {
                // Another tab having it means this one isn't a move of that file so much as a second look at it.
                if self.tab_for_path(&path).is_some_and(|other| other != id) {
                    if let Some(tab) = self.tab_mut(id) {
                        tab.notice = Some(Box::new(FileNotice::Missing));
                    }
                    return;
                }
                let from = self.path_of(id).map(|from| PathField::from_path(&from));
                self.relocate(id, &path);
                if let Some(tab) = self.tab_mut(id) {
                    tab.stamp = Some(now);
                    tab.notice = from.map(|from| Box::new(FileNotice::Moved { from }));
                }
            }
            FileStatus::Changed(now) => {
                self.retire_sleep_file(id);
                // With nothing unsaved there's only history, and its steps don't apply to this. Unsaved edits stay, and say so.
                if has_edits && !self.is_edited(id) {
                    self.clear_history(id, ClearReason::FileChanged);
                }
                let has_edits = self.has_list(id);
                if let Some(tab) = self.tab_mut(id) {
                    if !has_edits {
                        tab.stamp = Some(now);
                    }
                    tab.notice = Some(Box::new(FileNotice::Changed));
                }
            }
            FileStatus::Missing => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.notice = Some(Box::new(FileNotice::Missing));
                }
            }
        }
    }

    /// Every tab's file and what's known of it, for one pass by one thread after a restore. No thread or handle per tab.
    pub fn files_to_look_at(&self) -> Vec<(TabId, PathBuf, Option<FileStamp>)> {
        self.tabs.iter().filter_map(|tab| Some((tab.id, self.path_of(tab.id)?, tab.stamp))).collect()
    }

    /// The whole session as a record, with the host's own part at its defaults for the host to fill in. Every field is named here, with no `..`, so one added to a tab or to the view doesn't compile until it's carried or said not to be. A tab's edits go by journal, and [`Self::edits_record`] gives them whole.
    pub fn to_record(&self) -> SessionRecord {
        let Self { loader: _, view, tabs, active, next_id, warm: _, dirs: _, dir_ids: _, edits, wake_failed: _, asleep, started: _, held: _, sleep_files: _, just_read: _, woken_from_file: _, left_warm: _, draws: _, woken: _, place_lost: _, find, edit_mode, journaling: _, journals, fresh_journals: _, retired_journals: _, departing: _, history: _, agent_at_full: _, generated: _, comparison , saving: _, saved_over: _, jumps: _ } = self;
        let ViewSettings { paper, sidebar_hidden, agent_icon_hidden } = *view;
        let tabs = tabs
            .iter()
            .map(|tab| {
                let TabRecord { from_sleep: _, encrypted: _, touched: _, id, dir: _, file_name: _, name: _, page, page_count, toc_open, problem: _, renderer_only, stamp, place, view_rotation, spread, selection, notice, place_dirty: _, contents_verdict: _, generated_contents, folds, trail } = tab;
                TabEntry {
                    id: id.0,
                    path: PathField::from_path(&self.path_of(*id).unwrap_or_default()),
                    file: stamp.map(Into::into),
                    page: *page,
                    page_count: *page_count,
                    place: *place,
                    view_rotation: *view_rotation,
                    toc_open: *toc_open,
                    layout: *spread,
                    renderer_only: *renderer_only,
                    selection: selection.as_deref().copied(),
                    notice: notice.as_deref().cloned(),
                    journal: journals.get(id).map(ToString::to_string),
                    edits: edits.get(id).map(PageEdits::summary).or_else(|| asleep.get(id).copied()),
                    generated_contents: *generated_contents,
                    contents_folds: folds.as_deref().map(<[(u32, bool)]>::to_vec).unwrap_or_default(),
                    back: trail.as_ref().map(|trail| trail.back().to_vec()).unwrap_or_default(),
                    forward: trail.as_ref().map(|trail| trail.forward().to_vec()).unwrap_or_default(),
                }
            })
            .collect();
        let comparison = comparison.as_ref().map(|comparison| record::ComparisonRecord { tabs: comparison.tabs.iter().map(|tab| tab.0).collect(), synced: comparison.synced, sync_from: comparison.sync_from.clone() });
        SessionRecord { app: record::AppRecord::default(), view: ViewRecord { paper: paper.map(Paper::to_record), focus: false, sidebar_hidden, agent_icon_hidden }, find: find.clone(), edit_mode: *edit_mode, active: active.map(|id| id.0), next_id: *next_id, tabs, comparison }
    }

    /// A tab's edits whole, as QA reads them and for a journal's checkpoint.
    pub fn edits_record(&self, id: TabId) -> Option<record::EditsRecord> {
        self.edits.get(&id).map(PageEdits::to_record)
    }

    /// What's changed that a line of the log of places can say. Marks it as said.
    pub fn take_place_delta(&mut self, seq: u64) -> PlaceDelta {
        let mut places = Vec::new();
        for tab in &mut self.tabs {
            if std::mem::take(&mut tab.place_dirty) {
                places.push((tab.id.0, tab.page, tab.place));
            }
        }
        PlaceDelta { seq, active: self.active.map(|id| id.0), places, contents_scroll: None, focus: None }
    }

    /// **Puts the session back as a record has it.** No file is opened: every tab comes back cold, and its name and place are all it costs. Replaces what's here. The caller reads journals, and gets the active tab's file read, and checks what's become of the rest, each off this lock.
    pub fn restore(&mut self, record: &SessionRecord, mut journals: HashMap<String, RestoredEdits>) -> Result<(), RecordError> {
        record::validate_record(record)?;
        // A PDF is open once, even if the record says otherwise.
        let mut deduped = record.clone();
        record::one_tab_per_file(&mut deduped);
        let record = &deduped;
        self.tabs.clear();
        self.warm.clear();
        self.woken.clear();
        self.edits.clear();
        self.asleep.clear();
        self.wake_failed.clear();
        self.draws.clear();
        self.journals.clear();
        self.fresh_journals.clear();
        self.jumps.clear();
        self.active = None;
        self.comparison = None;
        self.next_id = record.next_id;
        self.find = record.find.clone();
        self.edit_mode = record.edit_mode;
        let sidebar_hidden = record.view.sidebar_comes_back_hidden();
        let ViewRecord { paper, focus: _, sidebar_hidden: _, agent_icon_hidden } = record.view;
        self.view = ViewSettings { paper: paper.map(Paper::from_record), sidebar_hidden, agent_icon_hidden };
        self.tabs.reserve_exact(record.tabs.len());
        for entry in &record.tabs {
            let Some(path) = entry.path.to_path() else { continue };
            let dir = self.intern_dir(path.parent().unwrap_or(Path::new("")));
            let Some(file_name) = path.file_name() else { continue };
            let id = TabId(entry.id);
            let mut notice = entry.notice.clone();
            if let Some(name) = &entry.journal {
                match journals.remove(name) {
                    Some(restored) => {
                        if restored.rolled_off {
                            notice = Some(FileNotice::HistoryRolledOff);
                        }
                        if restored.lost_lines {
                            notice = Some(FileNotice::EditsUnreadable);
                        }
                        let mut edits = restored.edits;
                        if self.journaling {
                            edits.set_journal_seq(restored.seq);
                        } else {
                            edits.no_journal();
                        }
                        self.edits.insert(id, edits);
                        self.journals.insert(id, name.as_str().into());
                    }
                    None => notice = Some(FileNotice::EditsUnreadable),
                }
            }
            let pages = self.edits.get(&id).map_or(entry.page_count, PageEdits::len);
            let trail = (!entry.back.is_empty() || !entry.forward.is_empty()).then(|| Box::new(trail::Trail::from_record(&entry.back, &entry.forward, pages)));
            self.tabs.push(TabRecord {
                from_sleep: None,
                encrypted: false,
                touched: 0,
                id,
                dir,
                file_name: file_name.into(),
                name: tab_name(&path).into(),
                page: entry.page.min(pages.saturating_sub(1)),
                page_count: entry.page_count,
                toc_open: entry.toc_open,
                problem: None,
                renderer_only: entry.renderer_only,
                stamp: entry.file.map(Into::into),
                place: entry.place,
                view_rotation: entry.view_rotation,
                spread: entry.layout,
                selection: entry.selection.map(Box::new),
                notice: notice.map(Box::new),
                place_dirty: false,
                contents_verdict: ContentsVerdict::Fine,
                generated_contents: entry.generated_contents,
                folds: (!entry.contents_folds.is_empty()).then(|| entry.contents_folds.iter().copied().take(record::MAX_CONTENTS_FOLDS).collect()),
                trail,
            });
        }
        self.active = record.active.map(TabId).filter(|id| self.tab(*id).is_some());
        // Side by side as it was, of the tabs that came back. A record's been checked, but one tab per file can take one out.
        self.comparison = record.comparison.as_ref().and_then(|saved| {
            let mut comparison = compare::Comparison::new(saved.tabs.iter().copied().map(TabId).filter(|id| self.tab(*id).is_some()).collect()).ok()?;
            if saved.synced && saved.sync_from.len() == saved.tabs.len() {
                comparison.synced = true;
                comparison.sync_from = saved.tabs.iter().zip(&saved.sync_from).filter(|(id, _)| comparison.contains(TabId(**id))).map(|(_, page)| *page).collect();
            }
            Some(comparison)
        });
        if let Some(comparison) = &self.comparison
            && !self.active.is_some_and(|active| comparison.contains(active))
        {
            self.active = comparison.tabs.first().copied();
        }
        // What's come back for tabs nobody's looking at is on disk already, and needn't be here too.
        if self.journaling && self.history.is_some() {
            let others: Vec<TabId> = self.edits.keys().copied().filter(|id| !self.on_screen(*id)).collect();
            for id in others {
                self.put_list_to_sleep(id);
            }
        }
        Ok(())
    }

    /// Whether putting the session back would lose anything of the owner's: unsaved edits or their history, which an agent below Full can't throw away. SPEC decision 20.
    pub fn holds_owner_edits(&self) -> bool {
        self.edits.values().any(PageEdits::holds_owner_edits) || self.asleep.values().any(|summary| summary.holds_owner_edits)
    }

    /// Makes a restored active tab what it says it is, once its file has been read. No tap, so its fold isn't touched.
    pub fn wake_active(&mut self) {
        if let Some(id) = self.active {
            self.activate(id);
        }
    }

    /// Whether the last edit removed the page the reader was on, so its place moved to the nearest one. Asking clears it.
    pub fn take_place_lost(&mut self) -> bool {
        std::mem::take(&mut self.place_lost)
    }

    /// Adds every page of another PDF at a position, as dropping it into the page view does. Only its facts are read.
    pub fn insert_pdf(&mut self, id: TabId, at: u32, path: &Path) -> Result<(), SessionError> {
        let stamp = file_stamp(path).ok();
        let facts = self.loader.load(path).map_err(SessionError::Open)?;
        self.insert_pdf_with_facts(id, Author::Owner, at, path, &facts, stamp)
    }

    /// Adds another PDF's pages from facts already read, for a caller that reads the file without holding the session. A file only the renderer can read can't be added: Save couldn't write it.
    pub fn insert_pdf_with_facts(&mut self, id: TabId, by: Author, at: u32, path: &Path, facts: &DocFacts, stamp: Option<FileStamp>) -> Result<(), SessionError> {
        if facts.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        if facts.page_count == 0 {
            return Err(SessionError::Open(OpenError::NotPdf));
        }
        self.edit_pages_as(id, by, |edits| edits.insert_pdf_noting(at, path, facts.page_count, stamp, facts.encrypted))
    }

    /// Where a page of a tab comes from now, counting unsaved edits. Zero-based.
    pub fn page_source(&self, id: TabId, position: u32) -> Option<PageSource> {
        let tab = self.tab(id)?;
        if self.asleep.contains_key(&id) {
            return None;
        }
        let Some(edits) = self.edits.get(&id) else {
            return (position < tab.page_count).then(|| Some(PageSource { path: self.path_of(id)?, page: position, quarter_turns: 0, added: false, expected: None })).flatten();
        };
        let slot = *edits.slots().get(position as usize)?;
        match slot.source {
            0 => Some(PageSource { path: self.path_of(id)?, page: slot.page, quarter_turns: slot.quarter_turns, added: false, expected: Some(tab.stamp) }),
            source => {
                let added = edits.added_sources().get(usize::from(source) - 1)?;
                Some(PageSource { path: added.path.to_path_buf(), page: slot.page, quarter_turns: slot.quarter_turns, added: true, expected: Some(added.stamp) })
            }
        }
    }

    /// Whether a tab with edits still has the file those edits are about. A tab with none has no page numbers to get wrong: reading from the file as it is now is what's wanted.
    pub fn own_file_is_current(&self, id: TabId) -> bool {
        let Some(tab) = self.tab(id) else { return true };
        if !self.has_list(id) {
            return true;
        }
        tab.stamp.is_none() || self.path_of(id).and_then(|path| file_stamp(&path).ok()) == tab.stamp
    }

    /// What a tab's own file was when its page numbers were recorded, for a caller listing pages by file.
    pub fn own_stamp(&self, id: TabId) -> Option<FileStamp> {
        self.tab(id).and_then(|tab| tab.stamp)
    }

    /// **The version of its file a save moved this tab on from,** while it's still on the version that save wrote: every page of that is a page that was shown before, as it was shown, in the same place. `None` once anything else has happened to the file.
    pub fn saved_from(&self, id: TabId) -> Option<FileStamp> {
        let now = self.own_stamp(id)?;
        self.saved_over.iter().rev().find(|(tab, _, after)| *tab == id && *after == now).map(|(_, before, _)| *before)
    }

    /// Notes that a page finished drawing, or couldn't be.
    pub fn note_drawn(&mut self, note: DrawNote) {
        self.draws.retain(|d| (d.tab, d.page) != (note.tab, note.page));
        self.draws.push_back(note);
        while self.draws.len() > MAX_DRAWS_KEPT {
            self.draws.pop_front();
        }
    }

    /// What's known about a page's latest draw, if it's still remembered.
    pub fn draw_note(&self, tab: TabId, page: u32) -> Option<DrawNote> {
        self.draws.iter().rev().find(|d| (d.tab, d.page) == (tab, page)).copied()
    }

    /// Reverses the last page edit. Returns false with nothing to undo.
    pub fn undo_edit(&mut self, id: TabId) -> bool {
        if self.is_saving(id) || !self.wake_list(id) {
            return false;
        }
        self.touch(id);
        let place = self.place_of(id);
        let undone = self.edits.get_mut(&id).is_some_and(PageEdits::undo);
        if undone {
            self.after_change(id, place);
        }
        undone
    }

    /// [`Self::undo_edit`] for whoever's asking: an agent's can be refused, if it would push the owner's steps out of the history.
    pub fn undo_edit_as(&mut self, id: TabId, by: Author) -> Result<bool, SessionError> {
        self.step_back_or_on(id, by, PageEdits::try_undo)
    }

    /// Whether a tab's pages can be edited now: it's there, more than the renderer can read its file, and its edits can be read. What greys out the panel's Rotate. Cheap: no list is woken for it.
    pub fn can_edit_pages(&self, id: TabId) -> bool {
        self.tab(id).is_some_and(|tab| !tab.renderer_only && !matches!(tab.notice.as_deref(), Some(FileNotice::EditsUnreadable)))
    }

    /// The panel's Rotate, by the owner. Outside edit mode it turns every page together, as one step that one undo takes back; in edit mode, the page being read. Owner, 2026-11-07: "When not in edit mode rotate page will rotate all pages together (it still enables save to commit the adjustment). Usefull when there is a scan issue or similar, for a quick fix."
    pub fn rotate_from_panel(&mut self, id: TabId, quarter_turns: i32) -> Result<(), SessionError> {
        let page = self.tab(id).ok_or(SessionError::NoSuchTab)?.page;
        let one_page = self.edit_mode;
        self.edit_pages_as(id, Author::Owner, |edits| if one_page { edits.rotate(page, 1, quarter_turns) } else { edits.rotate(0, edits.len(), quarter_turns) })
    }

    /// [`Self::redo_edit`] for whoever's asking.
    pub fn redo_edit_as(&mut self, id: TabId, by: Author) -> Result<bool, SessionError> {
        self.step_back_or_on(id, by, PageEdits::try_redo)
    }

    fn step_back_or_on(&mut self, id: TabId, by: Author, step: fn(&mut PageEdits) -> Result<bool, EditError>) -> Result<bool, SessionError> {
        if self.is_saving(id) {
            return Err(SessionError::Saving);
        }
        if !self.wake_list(id) {
            return Err(SessionError::EditsUnreadable);
        }
        self.touch(id);
        let place = self.place_of(id);
        let may_push_owner = by == Author::Agent && self.agent_at_full;
        let Some(edits) = self.edits.get_mut(&id) else { return Ok(false) };
        edits.act_as(by);
        edits.let_owner_steps_go(may_push_owner);
        let result = step(edits);
        edits.act_as(Author::Owner);
        edits.let_owner_steps_go(false);
        let stepped = result.map_err(SessionError::Edit)?;
        if stepped {
            self.after_change(id, place);
        }
        Ok(stepped)
    }

    /// Repeats the last undone page edit. Returns false with nothing to redo.
    pub fn redo_edit(&mut self, id: TabId) -> bool {
        if self.is_saving(id) || !self.wake_list(id) {
            return false;
        }
        self.touch(id);
        let place = self.place_of(id);
        let redone = self.edits.get_mut(&id).is_some_and(PageEdits::redo);
        if redone {
            self.after_change(id, place);
        }
        redone
    }

    /// Throws away a tab's unsaved page edits, and its history with them.
    pub fn discard_edits(&mut self, id: TabId) {
        self.clear_history(id, ClearReason::Discard);
    }

    /// Closes a tab and releases what it held. The next tab along, or the previous one, becomes active.
    /// Refuses while there are unsaved changes, or undone edits of the owner's, so nothing is lost by accident. [`Self::discard_edits`] is how to say it's meant, as the app does when told "Don't Save".
    pub fn close(&mut self, id: TabId) -> Result<(), SessionError> {
        if self.is_saving(id) {
            return Err(SessionError::Saving);
        }
        if self.is_edited(id) || self.has_undone_owner_steps(id) {
            return Err(SessionError::UnsavedChanges);
        }
        // History with nothing unsaved stays in its journal, for the PDF's next opening, as long as what's in the file is up to date: what hasn't gone out goes with it. Without any, or with no journal being kept, there's nothing to keep.
        // A list that won't read: nothing unsaved is at stake, or this wouldn't have got here. Its journal's left for a launch to look at.
        if !self.wake_list(id) {
            self.asleep.remove(&id);
            self.wake_failed.retain(|failed| *failed != id);
            self.journals.remove(&id);
        }
        let keeps_history = self.journaling && self.edits.get(&id).is_some_and(PageEdits::has_history);
        if keeps_history {
            if let Some(append) = self.append_for(id) {
                self.departing.push(JournalAppend { closing: true, ..append });
            }
            self.journals.remove(&id);
        } else {
            self.let_go_of_list(id);
        }
        self.fresh_journals.retain(|(tab, _, _)| *tab != id);
        self.edits.remove(&id);
        self.retire_sleep_file(id);
        let index = self.tabs.iter().position(|t| t.id == id).ok_or(SessionError::NoSuchTab)?;
        self.tabs.remove(index);
        self.draws.retain(|d| d.tab != id);
        self.warm.retain(|(tab, _)| *tab != id);
        self.generated.retain(|(tab, _)| *tab != id);
        self.woken.retain(|(tab, _, _)| *tab != id);
        self.jumps.retain(|(tab, _)| *tab != id);
        // Out of a comparison too. With one left it's that one alone, and the reader goes to another pane rather than the next tab along.
        let mut next_pane = None;
        if let Some(comparison) = &mut self.comparison
            && comparison.contains(id)
        {
            comparison.remove(id);
            next_pane = comparison.tabs.first().copied();
            if comparison.tabs.len() < compare::MIN_PANES {
                self.comparison = None;
            }
        }
        if self.active == Some(id)
            && let Some(next) = next_pane
        {
            self.active = None;
            self.activate(next);
        }
        if self.active == Some(id) {
            self.active = None;
            let next = self.tabs.get(index).or_else(|| index.checked_sub(1).and_then(|i| self.tabs.get(i))).map(|t| t.id);
            if let Some(next) = next {
                self.activate(next);
            }
        }
        Ok(())
    }

    /// Builds the sidebar. Cost depends on the visible rows and the active tab's contents, never on idle tabs' PDFs.
    pub fn sidebar(&self) -> SidebarModel<'_> {
        let mut rows = Vec::with_capacity(self.tabs.len());
        let mut rail_filled = 0;
        let mut place = String::new();
        let mut toc_open = false;
        for tab in &self.tabs {
            let is_active = self.active == Some(tab.id);
            rows.push(SidebarRow { kind: RowKind::Tab, tab: tab.id, text: &tab.name, page: None, depth: 0, current: is_active, notice: tab_notice_words(tab, self.has_list(tab.id)), action: None, entry: NONE, expandable: false, expanded: false });
            if !is_active {
                continue;
            }
            toc_open = tab.toc_open;
            place = self.place_words(tab.id, tab.page);
            if tab.toc_open
                && let Some((_, toc)) = self.warm.iter().find(|(id, _)| *id == tab.id)
            {
                let view = self.generated_view(tab.id);
                let note = contents_note(tab.contents_verdict, view, tab.generated_contents, toc.entries().len());
                if let Some((text, hover, action)) = note {
                    rows.push(SidebarRow { kind: RowKind::Note, tab: tab.id, text, page: None, depth: 0, current: false, notice: Some(hover), action, entry: NONE, expandable: false, expanded: false });
                }
                let shown = match view {
                    Some(GeneratedView::Ready(generated)) if tab.generated_contents => generated,
                    _ => toc,
                };
                // The note isn't an entry: the host draws it over the contents, and the rail counts entries.
                rail_filled = contents_rows(shown, tab, self.edits.get(&tab.id), &mut rows);
            }
        }
        let edited = self.active.is_some_and(|id| self.is_edited(id));
        let undone_owner_steps = self.active.is_some_and(|id| self.has_undone_owner_steps(id));
        SidebarModel { rows, rail_filled, toc_open, place, edited, undone_owner_steps }
    }

    /// Where this tab's generated contents stand, if anyone's asked.
    pub fn generated_view(&self, id: TabId) -> Option<&GeneratedView> {
        self.generated.iter().find(|(tab, _)| *tab == id).map(|(_, view)| view)
    }

    /// Notes where a tab's generated contents stand. The oldest note gives way past [`MAX_GENERATED_VIEWS`], but never the active tab's.
    pub fn set_generated_view(&mut self, id: TabId, view: GeneratedView) {
        if self.tab(id).is_none() {
            return;
        }
        self.generated.retain(|(tab, _)| *tab != id);
        self.generated.push((id, view));
        while self.generated.len() > MAX_GENERATED_VIEWS {
            let Some(oldest) = self.generated.iter().position(|(tab, _)| Some(*tab) != self.active) else { break };
            self.generated.remove(oldest);
        }
    }

    /// Forgets where a tab's generated contents stand, so it offers them again.
    pub fn forget_generated(&mut self, id: TabId) {
        self.generated.retain(|(tab, _)| *tab != id);
    }

    /// The same, from a look at the file as it was at `version`: dropped if the tab's file has changed since, as a look that comes back late is about the old one.
    pub fn set_generated_view_for(&mut self, id: TabId, view: GeneratedView, version: Option<FileStamp>) {
        if version.is_none() || self.own_stamp(id) == version {
            self.set_generated_view(id, view);
        }
    }

    /// Tabs whose generated contents are being made.
    pub fn generated_making(&self) -> Vec<TabId> {
        self.generated.iter().filter(|(_, view)| *view == GeneratedView::Making).map(|(tab, _)| *tab).collect()
    }

    /// Shows a tab's generated contents, or its own. The choice is kept in the record, and the file is never changed. What's folded goes, as it was about the other list's entries.
    pub fn choose_generated(&mut self, id: TabId, generated: bool) {
        if let Some(tab) = self.tab_mut(id)
            && tab.generated_contents != generated
        {
            tab.generated_contents = generated;
            tab.folds = None;
        }
    }

    /// Opens a contents section that's closed, or closes one that's open, as its chevron does. Owner, 2026-11-07: "there should be a collapse toggle button for sections with sub chapters on the left side of the headers". What following the place opens is the default, and only what differs from it is kept, at most [`record::MAX_CONTENTS_FOLDS`], in the record.
    pub fn toggle_contents_section(&mut self, id: TabId, entry: u32) {
        let Some(toc) = self.shown_contents(id) else { return };
        let Some(tab) = self.tab(id) else { return };
        let chain = following_chain(&toc, tab, self.edits.get(&id));
        let has_children = toc.entries().get(entry as usize + 1).is_some_and(|next| next.parent == entry);
        if !has_children {
            return;
        }
        let Some(tab) = self.tab_mut(id) else { return };
        let open_now = section_open(tab.folds.as_deref(), &chain, entry);
        let mut folds: Vec<(u32, bool)> = tab.folds.take().map(Vec::from).unwrap_or_default();
        folds.retain(|(at, _)| *at != entry);
        // Back to what following does is no fold at all.
        if chain.contains(&entry) == open_now {
            folds.push((entry, !open_now));
        }
        let over = folds.len().saturating_sub(record::MAX_CONTENTS_FOLDS);
        folds.drain(..over);
        tab.folds = (!folds.is_empty()).then(|| folds.into_boxed_slice());
        tab.place_dirty = true;
    }

    /// The contents the sidebar shows for a tab: its generated ones if they're chosen and ready, or its own.
    fn shown_contents(&self, id: TabId) -> Option<Arc<Toc>> {
        let (_, own) = self.warm.iter().find(|(tab, _)| *tab == id)?;
        Some(match (self.tab(id).is_some_and(|tab| tab.generated_contents), self.generated_view(id)) {
            (true, Some(GeneratedView::Ready(generated))) => Arc::clone(generated),
            _ => Arc::clone(own),
        })
    }

    /// What's thought of a tab's own contents, once they've been read.
    pub fn contents_verdict(&self, id: TabId) -> Option<ContentsVerdict> {
        let known = self.warm.iter().any(|(tab, _)| *tab == id);
        self.tab(id).filter(|_| known).map(|tab| tab.contents_verdict)
    }

    /// Tabs side by side, if there are.
    pub fn comparison(&self) -> Option<&compare::Comparison> {
        self.comparison.as_ref()
    }

    /// What's on screen, a pane each, left to right: a comparison's tabs, or the active tab alone, or nothing.
    pub fn panes(&self) -> Vec<TabId> {
        match &self.comparison {
            Some(comparison) => comparison.tabs.clone(),
            None => self.active.into_iter().collect(),
        }
    }

    /// Whether a tab is on screen: active, or a pane of a comparison. Such a tab doesn't go dormant, whatever's asking.
    pub fn on_screen(&self, id: TabId) -> bool {
        self.active == Some(id) || self.comparison.as_ref().is_some_and(|comparison| comparison.contains(id))
    }

    /// **Puts these tabs side by side,** in this order, separate scroll, with `reading` the pane being read, or the active tab if it's one of them, or the first. Their edits are woken, as a pane shows them. A tab that's cold is read here if nobody's read it ahead with the session free, as [`Self::wake_for_tap`] lets a caller do.
    pub fn compare(&mut self, tabs: &[TabId], reading: Option<TabId>) -> Result<(), compare::CompareError> {
        if tabs.iter().any(|id| self.tab(*id).is_none()) {
            return Err(compare::CompareError::NoSuchTab);
        }
        let comparison = compare::Comparison::new(tabs.to_vec())?;
        for id in tabs {
            self.wake_list(*id);
        }
        let reading = reading.filter(|id| tabs.contains(id)).or(self.active.filter(|id| tabs.contains(id))).unwrap_or(tabs[0]);
        // Synced scroll stays on through a change of tabs, starting again from where each is now.
        let synced = self.comparison.as_ref().is_some_and(|was| was.synced);
        self.comparison = Some(comparison);
        if synced {
            self.set_compare_synced(true);
        }
        // Every pane is a live document, so each is one of the warm tabs, the one read last and so first: a tab warm before that isn't a pane goes cold, and the live documents stay at [`MAX_WARM`] (SPEC §2), as `compare::MAX_PANES` is.
        // Those warm already first, so a pane coming in from cold pushes out a tab that isn't a pane, and never another pane, which would then be read with the session held.
        let mut others: Vec<TabId> = tabs.iter().copied().filter(|id| *id != reading).collect();
        others.sort_by_key(|id| !self.warm.iter().any(|(tab, _)| tab == id));
        for id in others {
            self.activate(id);
        }
        self.activate(reading);
        Ok(())
    }

    /// What a ⌘-click on this tab would wake from cold, and its file, to read with the session free: the tab joining, or the pane read next if the one being read leaves. `None` if nothing needs reading.
    pub fn wake_for_compare_click(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        let joining = !self.comparison.as_ref().is_some_and(|comparison| comparison.contains(id));
        let next = if joining { id } else if self.active == Some(id) { self.pane_after(id)? } else { return None };
        if self.active == Some(next) {
            return None;
        }
        self.file_to_wake_now(next)
    }

    /// The panes a comparison of these tabs would wake from cold, with their files, to read with the session free before [`Self::compare`].
    pub fn wake_for_compare(&mut self, tabs: &[TabId]) -> Vec<(TabId, PathBuf)> {
        let active = self.active;
        tabs.iter().filter(|id| active != Some(**id)).filter_map(|id| self.file_to_wake_now(*id)).collect()
    }

    /// **⌘-click and ⌃-click on a tab** (owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view"). A tab not in it joins at the right and is the one read; one in it leaves; down to one, that one's alone. On the only tab showing, nothing.
    pub fn toggle_compare(&mut self, id: TabId) -> Result<compare::CompareChange, compare::CompareError> {
        if self.tab(id).is_none() {
            return Err(compare::CompareError::NoSuchTab);
        }
        let Some(mut comparison) = self.comparison.clone() else {
            let Some(active) = self.active.filter(|active| *active != id) else { return Ok(compare::CompareChange::Nothing) };
            self.compare(&[active, id], Some(id))?;
            return Ok(compare::CompareChange::Added);
        };
        if comparison.contains(id) {
            comparison.remove(id);
            if comparison.tabs.len() < compare::MIN_PANES {
                let last = comparison.tabs.first().copied();
                self.comparison = None;
                if let Some(last) = last {
                    self.activate(last);
                }
                return Ok(compare::CompareChange::Ended);
            }
            let reading = comparison.tabs.first().copied();
            self.comparison = Some(comparison);
            if self.active == Some(id)
                && let Some(reading) = reading
            {
                self.activate(reading);
            }
            return Ok(compare::CompareChange::Removed);
        }
        let page = self.tab(id).map_or(0, |tab| tab.page);
        comparison.add(id, page)?;
        self.wake_list(id);
        self.comparison = Some(comparison);
        self.activate(id);
        Ok(compare::CompareChange::Added)
    }

    /// Back to one tab: the one being read.
    pub fn end_compare(&mut self) -> bool {
        self.comparison.take().is_some()
    }

    /// Synced scroll on, from where each pane is now, or separate. Says whether there was a comparison to set.
    pub fn set_compare_synced(&mut self, on: bool) -> bool {
        let pages: HashMap<TabId, u32> = self.tabs.iter().map(|tab| (tab.id, tab.page)).collect();
        match &mut self.comparison {
            Some(comparison) => {
                comparison.set_synced(on, |id| pages.get(&id).copied().unwrap_or(0));
                true
            }
            None => false,
        }
    }

    /// The reader's moved into another pane: it's the one read now, for the contents and the page number. Nothing changes on screen.
    pub fn focus_pane(&mut self, id: TabId) -> bool {
        if self.active == Some(id) || !self.comparison.as_ref().is_some_and(|comparison| comparison.contains(id)) {
            return false;
        }
        self.activate(id);
        true
    }

    /// Where synced scroll sends the other panes when `leader` is on `page`: see [`compare::Comparison::sync_targets`].
    pub fn sync_targets(&self, leader: TabId, page: u32) -> Vec<(TabId, u32)> {
        self.comparison.as_ref().map(|comparison| comparison.sync_targets(leader, page, |id| self.pages_in(id))).unwrap_or_default()
    }

    /// What's said about the panes' lengths, if anything.
    pub fn compare_note(&self) -> Option<String> {
        self.comparison.as_ref()?.length_note(|id| self.pages_in(id))
    }

    fn activate(&mut self, id: TabId) {
        // Going to a tab that isn't side by side, by opening it, an agent's `goto`, or closing what was showing, shows it alone.
        if self.comparison.as_ref().is_some_and(|comparison| !comparison.contains(id)) {
            self.comparison = None;
        }
        // Only the tab on screen holds a generated list in memory. The rest are on disk, and a cold tab stays a cold tab's size.
        self.generated.retain(|(tab, view)| *tab == id || !matches!(view, GeneratedView::Ready(_)));
        self.wake_list(id);
        // A moved or changed file with nothing riding on it has been said on the way past.
        if let Some(left) = self.active.filter(|left| *left != id) {
            let clean = !self.has_list(left);
            self.touch(left);
            if let Some(tab) = self.tab_mut(left)
                && ((clean && matches!(tab.notice.as_deref(), Some(FileNotice::Moved { .. } | FileNotice::Changed))) || matches!(tab.notice.as_deref(), Some(FileNotice::HistoryRolledOff)))
            {
                tab.notice = None;
            }
        }
        self.active = Some(id);
        if let Some(position) = self.warm.iter().position(|(tab, _)| *tab == id) {
            if let Some(entry) = self.warm.remove(position) {
                self.warm.push_front(entry);
            }
            return;
        }
        // Woken from cold: rebuild from disk, or from what was read ahead with the session free. If that fails, the tab stays and says why.
        let (loaded, stamp) = match self.woken.iter().position(|(tab, _, _)| *tab == id) {
            Some(place) => {
                let (_, loaded, stamp) = self.woken.remove(place);
                (loaded, stamp)
            }
            None => {
                let stamp = self.path_of(id).and_then(|path| file_stamp(&path).ok());
                (self.path_of(id).ok_or(OpenError::NotFound).and_then(|path| self.loader.load(&path)), stamp)
            }
        };
        // The file is what the list was made on, and is another length than it says: the list can't be about it, however it got here. A journal can say anything, as a save having happened.
        if let (Ok(facts), Some(known), Some(list)) = (&loaded, stamp, self.edits.get(&id))
            && self.tab(id).is_some_and(|tab| tab.stamp == Some(known))
            && list.file_pages() != facts.page_count
        {
            self.let_go_of_list(id);
            if let Some(tab) = self.tab_mut(id) {
                tab.notice = Some(Box::new(FileNotice::EditsUnreadable));
            }
        }
        // Woken to another file, with only history: its steps don't apply to this, whichever way it's noticed. Unsaved edits stay.
        if let (Some(now), Some(list)) = (stamp, self.edits.get(&id))
            && !list.is_edited()
            && self.tab(id).and_then(|tab| tab.stamp).is_some_and(|known| !known.same_contents(now))
        {
            self.clear_history(id, ClearReason::FileChanged);
            if let Some(tab) = self.tab_mut(id) {
                tab.notice = Some(Box::new(FileNotice::Changed));
            }
        }
        // An edited tab's length is its edit list's, not the file's: added pages must stay reachable.
        let pages_with_edits = self.edits.get(&id).map(PageEdits::len);
        let mut replaced = None;
        let tab = self.tab_mut(id);
        match loaded {
            Ok(facts) => {
                if let Some(tab) = tab {
                    // With edits, the numbers in them go by the length the file had, as a changed file's isn't what they're about.
                    if pages_with_edits.is_none() {
                        tab.page_count = facts.page_count;
                    }
                    tab.page = tab.page.min(pages_with_edits.unwrap_or(facts.page_count).saturating_sub(1));
                    tab.problem = None;
                    tab.renderer_only = facts.renderer_only;
                    tab.encrypted = facts.encrypted;
                    tab.place_dirty = true;
                    if matches!(tab.notice.as_deref(), Some(FileNotice::Missing | FileNotice::NeedsPassword)) {
                        tab.notice = None;
                    }
                    // With edits, the numbers in them are about the version they were made on, and that must stay known.
                    if pages_with_edits.is_none() {
                        replaced = tab.stamp.filter(|known| stamp.is_some_and(|now| !known.same_contents(now)));
                        tab.stamp = stamp;
                    }
                }
                // Another version of the file than the sleep file's about, which won't be read again.
                if let (Some(known), Some(path)) = (replaced, self.path_of(id)) {
                    self.retired_journals.push(record::sleep_file_name(known, &path).into());
                }
                self.remember_warm(id, Arc::new(facts.toc));
            }
            Err(problem) => {
                if let Some(tab) = tab {
                    tab.problem = Some(problem);
                    // A password is never kept, so a protected file asks again, and says that's why.
                    if problem == OpenError::PasswordNeeded {
                        tab.notice = Some(Box::new(FileNotice::NeedsPassword));
                    }
                }
            }
        }
    }

    fn remember_warm(&mut self, id: TabId, toc: Arc<Toc>) {
        if let Some(tab) = self.tab_mut(id) {
            tab.contents_verdict = recto_core::verdict::judge(&toc, tab.page_count);
        }
        self.warm.push_front((id, toc));
        while self.warm.len() > MAX_WARM {
            // The least recent's gone cold, so what its sleep file needs is kept long enough to write that.
            if let Some(left) = self.warm.pop_back()
                && self.sleep_files
            {
                self.left_warm.push(left);
            }
        }
    }

    /// Tabs just pushed out of the warm ones, and what their sleep files would say. There's a sleep file for any tab that's gone cold, by whatever way.
    pub fn take_left_warm(&mut self) -> Vec<SleepFacts> {
        let left = std::mem::take(&mut self.left_warm);
        left.into_iter().filter_map(|(id, toc)| self.sleep_facts_with(id, &toc)).collect()
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut TabRecord> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn intern_dir(&mut self, dir: &Path) -> u32 {
        if let Some(&id) = self.dir_ids.get(dir) {
            return id;
        }
        let id = u32::try_from(self.dirs.len()).unwrap_or(u32::MAX);
        self.dirs.push(dir.into());
        self.dir_ids.insert(dir.into(), id);
        id
    }
}

/// Adds the visible contents rows: every top-level entry, plus the chapter being read opened down to the current
/// section. Returns how many rows the rail fills.
///
/// With unsaved page edits, entries follow their pages to their new positions. An entry whose page was deleted stays
/// without a number, and pages added from another PDF belong to no section.
fn contents_rows<'a>(toc: &'a Toc, tab: &TabRecord, edits: Option<&PageEdits>, rows: &mut Vec<SidebarRow<'a>>) -> usize {
    let here = section_here(toc, tab, edits);
    let chain = following_chain(toc, tab, edits);
    let folds = tab.folds.as_deref();
    // Whether each entry's own children show: it's shown, and open. Entries come in document order, so a parent is always decided first.
    let mut children_show = vec![false; toc.entries().len()];
    let first_row = rows.len();
    let mut filled = 0;
    for (index, entry) in toc.entries().iter().enumerate() {
        if entry.parent != NONE && !children_show.get(entry.parent as usize).copied().unwrap_or(false) {
            continue;
        }
        let at = u32::try_from(index).unwrap_or(NONE);
        let expandable = toc.entries().get(index + 1).is_some_and(|next| next.parent == at);
        let expanded = expandable && section_open(folds, &chain, at);
        children_show[index] = expanded;
        let current = here == Some(index);
        rows.push(SidebarRow {
            kind: RowKind::Contents,
            tab: tab.id,
            text: toc.title(entry),
            page: (entry.page != NONE)
                .then(|| edits.map_or(Some(entry.page), |e| e.position_of(entry.page)))
                .flatten()
                .map(|page| page + 1),
            depth: entry.depth,
            current,
            notice: None,
            action: None,
            entry: at,
            expandable,
            expanded,
        });
        if current {
            filled = rows.len() - first_row;
        }
    }
    filled
}

/// The entry for the section being read, in the file's own page numbers.
fn section_here(toc: &Toc, tab: &TabRecord, edits: Option<&PageEdits>) -> Option<usize> {
    let original_page = match edits {
        Some(edits) => edits.slots().get(tab.page as usize).filter(|s| s.source == 0).map(|s| s.page),
        None => Some(tab.page),
    };
    original_page.and_then(|page| toc.section_at(page))
}

/// The section being read and every section it's in, which following the place keeps open.
fn following_chain(toc: &Toc, tab: &TabRecord, edits: Option<&PageEdits>) -> HashSet<u32> {
    let mut open = HashSet::new();
    let mut cursor = section_here(toc, tab, edits).map_or(NONE, |i| u32::try_from(i).unwrap_or(NONE));
    while cursor != NONE && open.insert(cursor) {
        cursor = toc.entries().get(cursor as usize).map_or(NONE, |e| e.parent);
    }
    open
}

/// Whether a section's children show: as the reader folded it, or else as following the place has it.
fn section_open(folds: Option<&[(u32, bool)]>, chain: &HashSet<u32>, entry: u32) -> bool {
    folds.and_then(|folds| folds.iter().rev().find(|(at, _)| *at == entry)).map_or_else(|| chain.contains(&entry), |(_, open)| *open)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionError {
    Open(OpenError),
    NoSuchTab,
    TooManyTabs,
    UnsavedChanges,
    Edit(EditError),
    /// Only the renderer can read this file, so its pages can't be changed.
    RendererOnly,
    /// Its edits can't be read back just now, so nothing's laid on them.
    EditsUnreadable,
    /// It's being saved, and nothing changes its edits until that's done.
    Saving,
}

impl SessionError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Open(error) => error.code(),
            Self::NoSuchTab => "no_such_tab",
            Self::TooManyTabs => "too_many_tabs",
            Self::UnsavedChanges => "unsaved_changes",
            Self::Edit(error) => error.code(),
            Self::RendererOnly => "renderer_only",
            Self::EditsUnreadable => "edits_unreadable",
            Self::Saving => "saving",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Open(error) => error.message(),
            Self::NoSuchTab => "That tab isn't open.",
            Self::TooManyTabs => "Too many PDFs are open. Close some and try again.",
            Self::EditsUnreadable => "This tab's edits can't be read back just now, so nothing more was done to them. They're kept, and it tries again each time. Discarding them is what gives up on them.",
            Self::UnsavedChanges => "This PDF has unsaved changes.",
            Self::Edit(error) => error.message(),
            Self::RendererOnly => "RectoPDF can draw this file but can't read its structure, so its pages can't be changed.",
            Self::Saving => "This PDF is being saved. Nothing changes until that's done, which is a moment.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_core::TocLine;
    use std::cell::Cell;

    struct FakeLoader {
        pages: u32,
        loads: Cell<u32>,
    }

    impl DocLoader for FakeLoader {
        fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
            self.loads.set(self.loads.get() + 1);
            if path.to_string_lossy().contains("missing") {
                return Err(OpenError::NotFound);
            }
            Ok(DocFacts { page_count: self.pages, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false })
        }
    }

    fn session(pages: u32) -> Session<FakeLoader> {
        Session::new(FakeLoader { pages, loads: Cell::new(0) })
    }

    /// Failure (review round 2): the close a prompt's Save asked for goes ahead though an edit came in after the save, so that edit is lost unasked; or undone steps hold the close up after a Save that was asked to close. An edit or an undo since keeps the tab open with it standing; with none, it closes over undone steps.
    #[test]
    fn a_close_after_save_keeps_what_came_in_since() {
        let mut s = session(3);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(id, |edits| edits.rotate(0, 1, 1)).unwrap();
        s.saved(id, FileStamp { size: 2, mtime_ns: 2, device: 2, inode: 2 }, 3, None, None);
        s.edit_pages(id, |edits| edits.rotate(1, 1, 1)).unwrap();
        assert_eq!(s.close_after_save(id), Ok(false), "an edit since: it stays");
        assert!(s.tab(id).is_some() && s.is_edited(id), "and the edit stands");
        s.undo_edit(id);
        s.undo_edit(id);
        assert!(s.is_edited(id), "an undo past the save is a change from the file too");
        assert_eq!(s.close_after_save(id), Ok(false));
        s.redo_edit(id);
        assert!(!s.is_edited(id) && s.has_undone_owner_steps(id), "back at the saved point, with a step undone");
        assert_eq!(s.close_after_save(id), Ok(true), "closes over the undone step, as the prompt said");
        assert!(s.tab(id).is_none());
    }

    /// Failure (the owner's bench file, 2026-11-08): after a save the page view can't tell the new version of the file from any other, so it lays the pages out afresh and the zoom shown and the place change. The session says which version a save moved the tab on from while the tab's still on what that save wrote, and stops saying it once the file is anything else: a carry-over onto someone else's file would show it at the wrong size.
    #[test]
    fn a_save_says_which_version_it_moved_on_from_and_only_while_it_holds() {
        let stamp = |n: u64| FileStamp { size: n, mtime_ns: n as i64, device: 1, inode: 1 };
        let mut s = session(3);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.apply_file_status(id, FileStatus::Same(stamp(1)));
        assert_eq!(s.saved_from(id), None, "nothing's been saved");
        s.edit_pages(id, |edits| edits.rotate(0, 1, 1)).unwrap();
        s.saved(id, stamp(2), 3, None, None);
        assert_eq!(s.saved_from(id), Some(stamp(1)));
        s.saved(id, stamp(3), 3, None, None);
        assert_eq!(s.saved_from(id), Some(stamp(2)), "the last save's");
        s.apply_file_status(id, FileStatus::Changed(stamp(4)));
        assert_eq!(s.saved_from(id), None, "the file changed outside since");
        // However many tabs are saved, what's kept stays small.
        for n in 0..(MAX_SAVED_OVER as u64 * 3) {
            let other = s.open(Path::new(&format!("/d/o{n}.pdf"))).unwrap();
            s.apply_file_status(other, FileStatus::Same(stamp(100 + n)));
            s.saved(other, stamp(1000 + n), 3, None, None);
        }
        assert!(s.saved_over.len() <= MAX_SAVED_OVER);
    }

    /// Failure (review): an edit made while a save writes comes back reading as saved, with its turn not in the file. Every way of changing a tab's edits waits while it's saved; a save that fails lets go and forgets what it noted, and the edits stand.
    #[test]
    fn nothing_changes_a_tab_while_it_is_saved() {
        let mut s = session(3);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert!(s.begin_save(id, false).unwrap().is_none(), "nothing unsaved, nothing to write");
        s.edit_pages(id, |edits| edits.rotate(0, 1, 1)).unwrap();
        s.edit_pages(id, |edits| edits.rotate(2, 1, 1)).unwrap();
        s.undo_edit(id);
        let snapshot = s.begin_save(id, false).unwrap().unwrap();
        assert_eq!((snapshot.slots.len(), snapshot.slots[0].quarter_turns, snapshot.file_pages), (3, 1, 3));
        assert!(snapshot.holds_owner_edits);
        assert_eq!(s.edit_pages(id, |edits| edits.rotate(1, 1, 1)), Err(SessionError::Saving));
        assert_eq!(s.edit_pages_as(id, Author::Agent, |edits| edits.delete(0, 1)), Err(SessionError::Saving));
        assert!(!s.undo_edit(id) && !s.redo_edit(id));
        assert_eq!(s.undo_edit_as(id, Author::Agent), Err(SessionError::Saving));
        assert_eq!(s.redo_edit_as(id, Author::Owner), Err(SessionError::Saving));
        assert!(!s.clear_history(id, ClearReason::Discard), "a discard waits too");
        assert_eq!(s.close(id), Err(SessionError::Saving));
        assert_eq!(s.begin_save(id, true).err(), Some(SessionError::Saving), "one save at a time");
        assert!(s.any_saving() && !s.can_go_dormant(id), "and it's held, so it can't go dormant");
        s.save_prepared(id, FileStamp { size: 9, mtime_ns: 9, device: 9, inode: 9 }, None);
        assert!(s.page_edits(id).unwrap().saving().is_some());
        s.end_save(id);
        assert!(!s.is_saving(id) && !s.any_saving());
        assert!(s.page_edits(id).unwrap().saving().is_none(), "what a failed save noted goes");
        assert!(s.is_edited(id), "and the edits stand");
        s.edit_pages(id, |edits| edits.rotate(1, 1, 1)).unwrap();
    }

    /// Failure: a downloaded file's name carries tag characters or a line break, and they reach the sidebar, `tabs` and error candidates: an instruction no eye can see. The path must
    /// stay exact, or the file couldn't be opened again.
    #[test]
    fn tab_names_are_cleaned_and_paths_stay_exact() {
        let mut session = Session::new(NameLoader);
        let hidden: String = "ignore".chars().map(|c| char::from_u32(0xE0000 + c as u32).unwrap()).collect();
        let raw = format!("/d/Report{hidden}\n2026\u{202E}.pdf");
        let id = session.open(Path::new(&raw)).unwrap();
        let tab = session.tab(id).unwrap();
        assert_eq!(&*tab.name, "Report 2026");
        assert_eq!(session.path_of(id).unwrap(), PathBuf::from(&raw));
        assert_eq!(session.sidebar().rows[0].text, "Report 2026");
        let invisible = session.open(Path::new("/d/\u{200B}.pdf")).unwrap();
        assert_eq!(&*session.tab(invisible).unwrap().name, "Untitled", "a name of nothing still needs a row");
    }

    struct NameLoader;

    impl DocLoader for NameLoader {
        fn load(&self, _path: &Path) -> Result<recto_core::DocFacts, recto_core::OpenError> {
            Ok(recto_core::DocFacts { page_count: 1, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: recto_core::Toc::default(), renderer_only: false })
        }
    }

    /// Failure: warm paper is too dim to read on, by WCAG AA, or it's lost its warmth and reads as white, or gone yellow or orange. Holds the value: text at the page's usual dark greys stays above AA, and each channel falls a little more than the last without a big drop in blue.
    #[test]
    fn warm_paper_keeps_body_text_readable_and_reads_as_warm() {
        for ink in [0x00_00_00, 0x22_22_22, 0x44_44_44, 0x59_59_59] {
            assert!(contrast_ratio(ink, WARM_PAPER) >= 4.5, "{ink:06x} on {WARM_PAPER:06x}: {}", contrast_ratio(ink, WARM_PAPER));
        }
        let (red, green, blue) = ((WARM_PAPER >> 16) & 0xFF, (WARM_PAPER >> 8) & 0xFF, WARM_PAPER & 0xFF);
        assert!(red >= green && green >= blue, "warm means red over green over blue");
        assert!(red - blue >= 12, "or it's white");
        assert!(red - blue <= 40, "or it's yellow");
        assert!(red >= 0xEE, "or it's dim");
    }

    /// Failure: the paper follows the window once it's been chosen, or doesn't when it hasn't. Every combination works: with nothing chosen it goes with the window, and a choice holds in either window.
    #[test]
    fn all_six_papers_and_windows_work() {
        assert_eq!(Paper::resolved(None, false), Paper::White);
        assert_eq!(Paper::resolved(None, true), Paper::Dark);
        for paper in Paper::ALL {
            assert_eq!(Paper::resolved(Some(paper), false), paper);
            assert_eq!(Paper::resolved(Some(paper), true), paper);
            assert_eq!(Paper::from_name(paper.name()), Some(paper));
            assert!(!paper.label().is_empty());
        }
        assert_eq!(Paper::from_name("sepia"), None);
    }

    /// Failure: a shortened name has no way to be read in full, or a hostile title takes over the hover: 100,000 characters, newlines and invisible characters. It comes back on one clean line, bounded, and says what it is.
    #[test]
    fn a_rows_full_text_is_clean_and_bounded() {
        let plain = row_full_text("QUIC: A UDP-Based Multiplexed and Secure Transport", false);
        assert_eq!(plain.text, "QUIC: A UDP-Based Multiplexed and Secure Transport");
        assert!(!plain.is_file_name);
        assert!(row_full_text("report", true).is_file_name);
        let hostile = format!("{}{}", "line\n".repeat(20_000), "\u{202E}\u{E0041}end");
        let shown = row_full_text(&hostile, false);
        assert!(shown.text.chars().count() <= MAX_ROW_FULL_TEXT + 1, "{}", shown.text.chars().count());
        assert!(shown.text.ends_with('…'));
        assert!(!shown.text.chars().any(char::is_control), "one line");
        assert!(!shown.text.contains('\u{202E}'));
        assert_eq!(row_full_text("\n\n", false).text, "");
    }

    /// Failure: a long outline pushes the other tabs or the footer off screen, the reason the owner asked for this. Holds at every window height down to the smallest, for every mix of tabs and rows: the tabs kept never lose their room, and nothing goes past the room there is unless the tab list itself is too long, when that scrolls.
    #[test]
    fn a_long_outline_never_pushes_the_other_tabs_away() {
        for room in [0.0, 50.0, 100.0, 208.0, 328.0, 700.0, 1400.0] {
            for tabs in [1u32, 2, 3, 6, 9, 200, 10_000] {
                for rows in [0u32, 1, 3, 40, 10_000, 8_000_000] {
                    let heights = sidebar_heights(room, tabs, rows);
                    assert!(heights.contents >= 0.0 && heights.contents <= f64::from(rows) * CONTENTS_ROW_HEIGHT);
                    assert!(heights.contents <= room * CONTENTS_MAX_SHARE + 1e-9, "{room} {tabs} {rows}: {heights:?}");
                    if f64::from(tabs) * TAB_ROW_HEIGHT + heights.contents <= room {
                        assert!(!heights.tabs_scroll, "{room} {tabs} {rows}: all of it fits, so nothing scrolls but the contents");
                    }
                }
            }
        }
    }

    /// Failure: a long outline pushes the other tabs and the footer off, as a rule of "what the tabs leave" did with one tab: the owner saw no maximum at all. It stops at the maximum however few tabs there are, a short outline takes only what it needs, and the tabs scroll only if they don't fit in what's left.
    #[test]
    fn the_contents_have_a_true_maximum() {
        let most = 700.0 * CONTENTS_MAX_SHARE;
        assert_eq!(sidebar_heights(700.0, 1, 1000).contents, most, "one tab, and it still stops");
        assert_eq!(sidebar_heights(700.0, 9, 1000).contents, most);
        assert_eq!(sidebar_heights(700.0, 2, 3).contents, 3.0 * CONTENTS_ROW_HEIGHT, "only what it needs");
        assert!(!sidebar_heights(700.0, 5, 1000).tabs_scroll, "what's left holds these");
        assert!(sidebar_heights(700.0, 40, 1000).tabs_scroll, "and these scroll in it");
        assert!(sidebar_heights(100.0, 1, 1000).contents <= 100.0 - TAB_ROW_HEIGHT, "a short window keeps the tab's own row");
        assert_eq!(sidebar_heights(f64::NAN, 2, 1000).contents, 0.0);
        assert_eq!(sidebar_heights(10.0, 1, 1000).contents, 0.0);
        // A note over them comes out of the same share, so the tabs keep what they had.
        let noted = sidebar_heights_with_note(700.0, 1, 1000, true);
        assert_eq!(noted.note, NOTE_ROW_HEIGHT);
        assert!((noted.contents + noted.note - most).abs() < 1e-9, "{noted:?}");
        assert_eq!(sidebar_heights_with_note(700.0, 1, 0, true).contents, 0.0, "a note alone, for a file with no contents");
        assert_eq!(sidebar_heights_with_note(40.0, 1, 10, true).note, 0.0, "no room for it");
    }

    /// Failure: in a short contents area, a place that's just been brought to its anchor is outside the dead zone, so it moves again at every section, or for ever. Wherever the list is short, landing on the anchor is at rest.
    #[test]
    fn the_dead_zone_holds_in_a_short_contents_area() {
        for height in [CONTENTS_ROW_HEIGHT, 56.0, 100.0, 168.0, 300.0, 420.0] {
            let mut follow = ContentsFollow::default();
            let view = |scroll_y: f64| ContentsView { scroll_y, height, rows: 2000 };
            follow.reached(Some(0), view(0.0), 0.0, false);
            assert_eq!(follow.reached(Some(500), view(0.0), 1.0, false), FollowMove::Move, "height {height}");
            let landed = *run_frames(&mut follow, 0.0).last().unwrap();
            assert!(!follow.is_moving());
            // The same section again, and the next: nothing moves until the place has really left.
            assert_eq!(follow.reached(Some(500), view(landed), 2.0, false), FollowMove::Stay, "height {height}");
            let place = 500.5 * CONTENTS_ROW_HEIGHT;
            assert!(place >= landed + height * FOLLOW_DEAD_ZONE.0 && place <= landed + height * FOLLOW_DEAD_ZONE.1, "height {height}: {place} from {landed}");
        }
    }

    const HEIGHT: f64 = 560.0;
    const ROWS: u32 = 1000;

    fn contents_view(scroll_y: f64) -> ContentsView {
        ContentsView { scroll_y, height: HEIGHT, rows: ROWS }
    }

    fn anchor_y(row: u32) -> f64 {
        (f64::from(row) + 0.5) * CONTENTS_ROW_HEIGHT - HEIGHT * FOLLOW_ANCHOR
    }

    /// Runs a move to its end, at 60 frames a second, and returns each position.
    /// Failure: a window's resize counts as a change of place, so a list the reader had scrolled themselves snaps back once they've stopped. It stays where they put it.
    #[test]
    fn a_resize_leaves_a_list_the_reader_scrolled_alone() {
        let mut follow = ContentsFollow::default();
        let view = ContentsView { scroll_y: 0.0, height: 400.0, rows: 200 };
        follow.reached(Some(50), view, 0.0, false);
        follow.user_scrolled(1.0);
        follow.view_changed();
        let later = ContentsView { scroll_y: 2000.0, height: 300.0, rows: 200 };
        assert_eq!(follow.reached(Some(50), later, 10.0, false), FollowMove::Stay, "the same section, and it's been left alone");
        assert!(!follow.is_following(), "still theirs until the section changes");
    }

    /// Failure: folding leaves a move running, so opening the contents again whooshes from the top. Opened, the place is straight where it should be.
    #[test]
    fn folding_ends_a_move_and_opening_goes_straight_there() {
        let mut follow = ContentsFollow::default();
        let view = |scroll_y: f64| ContentsView { scroll_y, height: 400.0, rows: 200 };
        follow.reached(Some(0), view(0.0), 0.0, false);
        assert_eq!(follow.reached(Some(100), view(0.0), 1.0, false), FollowMove::Move);
        assert_eq!(follow.reached(None, view(0.0), 1.1, false), FollowMove::Stay);
        assert!(!follow.is_moving(), "nothing's left running");
        assert_eq!(follow.step(0.0, 1.0 / 60.0), None);
        assert!(matches!(follow.reached(Some(100), view(0.0), 2.0, false), FollowMove::SetTo(_)));
        assert!(!follow.is_moving());
    }

    /// The share of steps with the place in the view, with sections going by at this many rows a second.
    fn place_in_view(rows_per_second: f64, seconds: f64) -> f64 {
        const DT: f64 = 1.0 / 120.0;
        let mut follow = ContentsFollow::default();
        let (height, rows) = (680.0, 100_000);
        let mut position = 0.0;
        follow.reached(Some(0), ContentsView { scroll_y: position, height, rows }, 0.0, false);
        let steps = (seconds / DT) as u32;
        let mut shown = 0;
        for step in 0..steps {
            let now = f64::from(step) * DT;
            let row = (now * rows_per_second) as u32;
            follow.reached(Some(row), ContentsView { scroll_y: position, height, rows }, now, false);
            if let Some(next) = follow.step(position, DT) {
                position = next;
            }
            let place = (f64::from(row) + 0.5) * CONTENTS_ROW_HEIGHT;
            if (position..=position + height).contains(&place) {
                shown += 1;
            }
        }
        f64::from(shown) / f64::from(steps)
    }

    /// Failure: scrolling fast, the view trails past the place: the target only moves once the place has passed the last target's zone, and a move never gets quicker, so at 40 pages a second the place is out of the list about one step in ten, on `rfc9110` as measured. The first moment aside, it stays in view at any speed a reader can scroll.
    #[test]
    fn the_place_stays_in_view_at_any_speed() {
        for rows_per_second in [20.0, 120.0, 400.0] {
            let share = place_in_view(rows_per_second, 5.0);
            assert!(share >= 0.97, "{rows_per_second} rows a second: in view {share}");
        }
    }

    /// Failure: two tabs being woken at once share one place to put what was read, so one is lost and loaded with the session locked. Or a tab closed mid-read keeps its facts, or another tab's are taken for it. Each is kept for its own tab, and only while that tab is cold.
    #[test]
    fn what_was_read_ahead_stays_with_its_tab() {
        struct Counted(std::rc::Rc<std::cell::Cell<u32>>);
        impl DocLoader for Counted {
            fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
                self.0.set(self.0.get() + 1);
                Ok(DocFacts { page_count: 5, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false })
            }
        }
        let loads = std::rc::Rc::new(std::cell::Cell::new(0));
        let mut s = Session::new(Counted(loads.clone()));
        let ids: Vec<TabId> = (0..6).map(|index| s.open(Path::new(&format!("/d/{index}.pdf"))).unwrap()).collect();
        let facts = |pages| Ok(DocFacts { page_count: pages, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false });
        let base = loads.get();
        // Two wakes done together: both kept, each for its own tab.
        s.give_woken(ids[0], facts(11), None);
        s.give_woken(ids[1], facts(22), None);
        s.tap(ids[1]).unwrap();
        assert_eq!(s.pages_in(ids[1]), 22);
        assert_eq!(loads.get(), base, "no read with the session held");
        // Closed mid-read: nothing's kept for it.
        s.give_woken(ids[2], facts(33), None);
        s.close(ids[2]).unwrap();
        assert!(s.woken.iter().all(|(tab, _, _)| *tab != ids[2]));
        s.give_woken(ids[2], facts(33), None);
        assert!(s.woken.iter().all(|(tab, _, _)| *tab != ids[2]), "it's gone, so what arrives late isn't kept");
        // Already warm: it's not needed.
        s.give_woken(ids[5], facts(55), None);
        assert!(s.woken.iter().all(|(tab, _, _)| *tab != ids[5]));
        s.tap(ids[0]).unwrap();
        assert_eq!(s.pages_in(ids[0]), 11, "the first's own");
        assert_eq!(loads.get(), base);
    }

    /// Failure: a chapter closes just after a reposition. The place drops up the list by the rows that went, past the dead zone, and the contents move straight back, so one change of section makes two moves in opposite directions.
    #[test]
    fn a_chapter_closing_above_the_place_makes_no_second_move() {
        let mut follow = ContentsFollow::default();
        let (height, rows) = (680.0, 40);
        let view = |scroll_y: f64, rows: u32| ContentsView { scroll_y, height, rows };
        assert!(matches!(follow.reached(Some(0), view(0.0, rows), 0.0, false), FollowMove::SetTo(_)));
        assert_eq!(follow.reached(Some(25), view(0.0, rows), 1.0, false), FollowMove::Move);
        let landed = *run_frames(&mut follow, 0.0).last().unwrap();
        // Chapter at row 14 closes its 11 sections, and the next chapter, row 26, becomes row 15.
        let change = ContentsRowChange { from: 15, removed: 12, inserted: 1, place_shift: -11 };
        let by = follow.row_change(change, landed).expect("the scroll takes it up, with no slide to see");
        assert!(by < 0.0);
        follow.shifted(by);
        assert_eq!(follow.reached(Some(15), view(landed + by, rows - 11), 2.0, false), FollowMove::Stay, "the place is where it was on screen");
    }

    /// Failure: rows change under a reader who has scrolled the contents themselves, and what they're looking at is taken from them. Only rows above the view move the scroll then, and only by what went.
    #[test]
    fn a_reader_in_the_contents_keeps_what_they_are_looking_at() {
        let mut follow = ContentsFollow::default();
        follow.reached(Some(0), ContentsView { scroll_y: 0.0, height: 680.0, rows: 40 }, 0.0, false);
        follow.user_scrolled(1.0);
        let closes_in_view = ContentsRowChange { from: 15, removed: 12, inserted: 1, place_shift: -11 };
        assert_eq!(follow.row_change(closes_in_view, 280.0), None, "in the view, they slide");
        assert_eq!(follow.row_change(closes_in_view, 28.0 * 30.0), Some(-11.0 * 28.0), "above the view, it can't be seen");
    }

    fn run_frames(follow: &mut ContentsFollow, mut position: f64) -> Vec<f64> {
        let mut path = vec![position];
        for _ in 0..2000 {
            let Some(next) = follow.step(position, 1.0 / 60.0) else { return path };
            position = next;
            path.push(position);
        }
        panic!("a move never ended, so frames would run for ever: {path:?}");
    }

    fn placed_at(row: u32) -> (ContentsFollow, f64) {
        let mut follow = ContentsFollow::default();
        let FollowMove::SetTo(y) = follow.reached(Some(row), contents_view(0.0), 100.0, false) else { panic!("the first showing goes straight there") };
        (follow, y)
    }

    /// Failure: the list moves at every page turn, which is what the owner asked not to see. Across a slow scripted scroll, with the place inside the dead zone, it doesn't move by one pixel: asked at every section, the answer is always to stay.
    #[test]
    fn inside_the_dead_zone_nothing_moves_at_all() {
        let (mut follow, y) = placed_at(100);
        assert!((y - anchor_y(100)).abs() < 0.001);
        // Forward to the zone's edge. 45% of 560 points is 9 rows.
        for row in 101..=108 {
            assert_eq!(follow.reached(Some(row), contents_view(y), 100.0, false), FollowMove::Stay, "row {row}");
            assert!(!follow.is_moving());
        }
        // And back, by a little.
        for row in (97..108).rev() {
            assert_eq!(follow.reached(Some(row), contents_view(y), 100.0, false), FollowMove::Stay, "row {row}");
        }
    }

    /// Failure: reading on nudges the list at every section, or it never comes back. Leaving the dead zone starts one move, back to the anchor, and then it rests again for a useful stretch.
    #[test]
    fn leaving_the_dead_zone_makes_one_move_and_then_rests() {
        let (mut follow, mut y) = placed_at(100);
        let mut moves = Vec::new();
        for row in 101..200 {
            if follow.reached(Some(row), contents_view(y), 100.0, false) == FollowMove::Move {
                moves.push(row);
                let path = run_frames(&mut follow, y);
                y = *path.last().unwrap();
                assert!((y - anchor_y(row)).abs() < 0.001, "row {row}: at {y}");
            }
        }
        assert!(moves.len() >= 5, "{moves:?}");
        for pair in moves.windows(2) {
            assert!(pair[1] - pair[0] >= 8, "a rest of only {} rows: {moves:?}", pair[1] - pair[0]);
        }
    }

    /// Failure: a move starts or stops with a jolt. Over every move, near and far, speed builds from about nothing and fades to about nothing, and never changes much from one frame to the next, as a share of its peak.
    #[test]
    fn a_move_starts_and_ends_softly() {
        for row in [112, 150, 600] {
            let (mut follow, y) = placed_at(100);
            assert_eq!(follow.reached(Some(row), contents_view(y), 100.0, false), FollowMove::Move, "row {row}");
            let path = run_frames(&mut follow, y);
            let speeds: Vec<f64> = path.windows(2).map(|pair| (pair[1] - pair[0]) * 60.0).collect();
            let peak = speeds.iter().fold(0.0f64, |peak, speed| peak.max(speed.abs()));
            let (first, last) = (speeds[0].abs(), speeds[speeds.len() - 2].abs());
            assert!(first < peak * 0.15, "row {row}: starts at {first} of {peak}");
            assert!(last < peak * 0.15, "row {row}: ends at {last} of {peak}");
            // No step: how much the speed changes doesn't itself change suddenly from one frame to the next.
            let changes: Vec<f64> = speeds.windows(2).map(|pair| pair[1] - pair[0]).collect();
            for pair in changes.windows(2).take(changes.len().saturating_sub(2)) {
                assert!((pair[1] - pair[0]).abs() < peak * 0.2, "row {row}: {} then {}, of {peak}", pair[0], pair[1]);
            }
            assert!(speeds.iter().all(|speed| *speed >= -peak * 0.01), "row {row}: it went backwards");
            let seconds = path.len() as f64 / 60.0;
            assert!(seconds >= FOLLOW_SECONDS.0 * 0.7 && seconds <= FOLLOW_SECONDS.1 * 1.25, "row {row}: {seconds} s");
        }
    }

    /// Failure: reading quickly restarts the move at each section, so it stutters to a stop and starts again. A new target mid-move leaves the speed where it was, and many in a row are one long move with no stop in it.
    #[test]
    fn a_new_target_mid_move_has_no_step_in_speed() {
        let (mut follow, mut y) = placed_at(100);
        assert_eq!(follow.reached(Some(115), contents_view(y), 100.0, false), FollowMove::Move);
        let mut speed = 0.0;
        for _ in 0..12 {
            let next = follow.step(y, 1.0 / 60.0).unwrap();
            speed = (next - y) * 60.0;
            y = next;
        }
        assert!(speed > 50.0, "it's moving by now: {speed}");
        assert_eq!(follow.reached(Some(140), contents_view(y), 100.0, false), FollowMove::Move);
        let next = follow.step(y, 1.0 / 60.0).unwrap();
        let after = (next - y) * 60.0;
        assert!((after - speed).abs() < speed * 0.25 + 30.0, "{speed} then {after}");

        // 40 sections a second for two seconds is one move, not forty.
        let (mut follow, mut y) = placed_at(100);
        let mut stopped_on = 0;
        let mut row = 100.0;
        for _ in 0..120 {
            row += 40.0 / 60.0;
            follow.reached(Some(row as u32), contents_view(y), 100.0, false);
            match follow.step(y, 1.0 / 60.0) {
                Some(next) => y = next,
                None => stopped_on += u32::from(row > 110.0),
            }
        }
        assert_eq!(stopped_on, 0, "the move kept going");
        let place = (row + 0.5) * CONTENTS_ROW_HEIGHT;
        assert!(place >= y && place <= y + HEIGHT, "and the place is showing: {place} in {y}");
    }

    /// Failure: a far jump ends somewhere near, or stops short. It's allowed to whoosh, and it lands exactly on the anchor, in the bounded time.
    #[test]
    fn a_far_jump_lands_exactly_on_the_anchor() {
        let (mut follow, y) = placed_at(10);
        assert_eq!(follow.reached(Some(800), contents_view(y), 100.0, false), FollowMove::Move);
        let path = run_frames(&mut follow, y);
        assert_eq!(*path.last().unwrap(), anchor_y(800));
        assert!(path.len() as f64 / 60.0 <= FOLLOW_SECONDS.1 * 1.25, "{} frames", path.len());
    }

    /// Failure: at the ends of the list, where the anchor can't be reached, a move runs past what can be scrolled, or keeps trying and never ends, or the first sections move at all.
    #[test]
    fn the_first_and_last_sections_stop_at_the_end_of_the_list() {
        let (mut follow, y) = placed_at(0);
        assert_eq!(y, 0.0, "the top of the list");
        for row in 1..=8 {
            assert_eq!(follow.reached(Some(row), contents_view(0.0), 100.0, false), FollowMove::Stay, "row {row}");
        }
        let furthest = f64::from(ROWS) * CONTENTS_ROW_HEIGHT - HEIGHT;
        let (mut follow, y) = placed_at(900);
        assert_eq!(follow.reached(Some(999), contents_view(y), 100.0, false), FollowMove::Move);
        let path = run_frames(&mut follow, y);
        assert_eq!(*path.last().unwrap(), furthest);
        assert!(path.iter().all(|at| *at <= furthest + 0.5), "it ran past the end");
        assert_eq!(follow.reached(Some(998), contents_view(furthest), 100.0, false), FollowMove::Stay, "and it's done");
    }

    /// Failure: a slow end never arrives, because a scroll view rounds each position to a whole device pixel, and the small step from there rounds back to the same place. Frames ran for ever, seen in the first build. Given only rounded positions, it must still end, in place.
    #[test]
    fn rounding_by_the_host_doesnt_stop_a_move() {
        let (mut follow, mut y) = placed_at(100);
        assert_eq!(follow.reached(Some(112), contents_view(y), 100.0, false), FollowMove::Move);
        for _ in 0..2000 {
            let Some(next) = follow.step(y, 1.0 / 60.0) else {
                assert!((y - anchor_y(112)).abs() <= 0.5, "{y}");
                return;
            };
            y = (next * 2.0).round() / 2.0;
        }
        panic!("it never arrived: {y}");
    }

    /// Failure: reading pulls the list out from under someone looking through it. A scroll stops any move where it is, and nothing moves afterwards, until they've stopped using it.
    #[test]
    fn a_reader_scrolling_the_contents_pauses_following() {
        let (mut follow, y) = placed_at(100);
        follow.reached(Some(115), contents_view(y), 100.0, false);
        assert!(follow.is_moving());
        follow.user_scrolled(101.0);
        assert!(!follow.is_moving() && !follow.is_following());
        assert_eq!(follow.step(y, 1.0 / 60.0), None, "stopped right there");
        assert_eq!(follow.reached(Some(300), contents_view(y), 102.0, false), FollowMove::Stay, "still using it");
    }

    /// Failure: following never comes back, or comes back while the pointer is still on the list. It takes the pointer out, the time passed, and a new section, all three.
    #[test]
    fn following_resumes_only_when_they_have_stopped_and_the_section_changes() {
        let (mut follow, y) = placed_at(100);
        follow.pointer(true);
        follow.user_scrolled(100.0);
        let later = 100.0 + FOLLOW_RESUME_AFTER + 5.0;
        assert_eq!(follow.reached(Some(120), contents_view(y), later, false), FollowMove::Stay, "the pointer's on it");
        follow.pointer(false);
        assert!(!follow.is_following(), "not until the section changes");
        assert_eq!(follow.reached(Some(120), contents_view(y), later, false), FollowMove::Stay, "the same section, so nothing's changed");
        let (mut soon, y) = placed_at(100);
        soon.user_scrolled(100.0);
        assert_eq!(soon.reached(Some(120), contents_view(y), 100.0 + FOLLOW_RESUME_AFTER - 0.5, false), FollowMove::Stay, "too soon");
        assert_eq!(follow.reached(Some(125), contents_view(y), later, false), FollowMove::Move);
        assert!(follow.is_following());
    }

    /// Failure: a pointer resting on the list has it move from under it, though they haven't scrolled.
    #[test]
    fn nothing_moves_under_a_resting_pointer() {
        let (mut follow, y) = placed_at(100);
        follow.reached(Some(115), contents_view(y), 100.0, false);
        follow.pointer(true);
        assert!(!follow.is_moving(), "what was moving stops");
        assert_eq!(follow.reached(Some(130), contents_view(y), 100.0, false), FollowMove::Stay);
        follow.pointer(false);
        assert_eq!(follow.reached(Some(135), contents_view(y), 100.0, false), FollowMove::Move, "and picks up on the next section");
    }

    /// Failure: Reduce Motion gets an animation anyway, or the place is lost. No frames at all, nothing moves while the place shows, and it goes straight there when it doesn't.
    #[test]
    fn reduce_motion_never_animates() {
        let (mut follow, y) = placed_at(100);
        assert_eq!(follow.reached(Some(112), contents_view(y), 100.0, true), FollowMove::Stay, "past the dead zone, but still showing");
        assert!(matches!(follow.reached(Some(400), contents_view(y), 100.0, true), FollowMove::SetTo(_)));
        assert!(!follow.is_moving());
    }

    /// Failure: another tab's contents swoop in from wherever the last tab's were. They start in place.
    #[test]
    fn new_contents_start_in_place() {
        let (mut follow, _) = placed_at(100);
        follow.new_contents();
        assert!(matches!(follow.reached(Some(500), contents_view(0.0), 100.0, false), FollowMove::SetTo(_)));
    }

    /// Failure: the contents come back where the place would put them, not where they were, or forget the reader had scrolled them. And a hostile number isn't taken.
    #[test]
    fn the_contents_come_back_as_they_were() {
        let mut follow = ContentsFollow::default();
        follow.resume_at(280.0, false);
        assert_eq!(follow.reached(Some(100), contents_view(0.0), 0.0, false), FollowMove::SetTo(280.0));
        assert!(!follow.is_following(), "they were the reader's");
        let mut follow = ContentsFollow::default();
        follow.resume_at(f64::NAN, false);
        assert!(follow.is_following());
        follow.resume_at(1e12, true);
        let furthest = f64::from(ROWS) * CONTENTS_ROW_HEIGHT - HEIGHT;
        assert_eq!(follow.reached(Some(100), contents_view(0.0), 0.0, false), FollowMove::SetTo(furthest));
    }

    /// Failure: a shorter window leaves the place below the bottom of the contents, with nothing to bring it back until the section changes. Seen in the first short-window shot.
    #[test]
    fn a_change_of_height_brings_the_place_back() {
        let (mut follow, y) = placed_at(100);
        follow.view_changed();
        assert!(matches!(follow.reached(Some(100), ContentsView { scroll_y: y, height: 120.0, rows: 1000 }, 100.0, false), FollowMove::SetTo(_)));
    }

    /// Failure: rows opening above the place throw a move off, by the number of rows the host just moved past.
    #[test]
    fn rows_changing_above_carry_the_move_with_them() {
        let (mut follow, y) = placed_at(100);
        follow.reached(Some(115), contents_view(y), 100.0, false);
        follow.shifted(-112.0);
        let path = run_frames(&mut follow, y - 112.0);
        assert!((path.last().unwrap() - (anchor_y(115) - 112.0)).abs() < 1.0, "{path:?}");
    }

    /// Failure: a field added carelessly raises the cost of every idle tab.
    #[test]
    fn tab_record_stays_small() {
        assert!(std::mem::size_of::<TabRecord>() <= 256);
    }

    /// Failure: a field added carelessly raises the cost of every remembered draw.
    #[test]
    fn draw_note_stays_small() {
        assert!(std::mem::size_of::<DrawNote>() <= 32);
    }

    /// Failure: after a move, the place stays at the same number and shows a different page, which no reader would do. It stays on the page, and if that page goes, it lands on the nearest and says so once.
    #[test]
    fn the_place_follows_the_page_being_read() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.go_to(id, 3).unwrap();
        s.edit_pages(id, |e| e.move_pages(3, 1, 0)).unwrap();
        assert_eq!(s.tab(id).unwrap().page, 0, "page 4 moved to the front, and the reader with it");
        assert!(!s.take_place_lost());
        s.edit_pages(id, |e| e.move_pages(1, 1, 3)).unwrap();
        assert_eq!(s.tab(id).unwrap().page, 0, "another page moved, so nothing changes");
        s.edit_pages(id, |e| e.delete(0, 1)).unwrap();
        assert!(s.take_place_lost(), "the page being read was deleted");
        assert!(!s.take_place_lost(), "it's only said once");
        assert_eq!(s.tab(id).unwrap().page, 0, "the nearest there is");
        s.go_to(id, 3).unwrap();
        s.edit_pages(id, |e| e.delete(3, 1)).unwrap();
        assert!(s.take_place_lost());
        assert_eq!(s.tab(id).unwrap().page, 2, "the last page, since that was the end");
        let before = s.tab(id).unwrap().page;
        assert!(s.undo_edit(id));
        assert_eq!(s.tab(id).unwrap().page, before, "undo keeps the page in view while one comes back behind it");
    }

    /// Failure: wait says a page is drawn when a different page now has that number, because notes are by position and outlive edits.
    #[test]
    fn draw_notes_go_when_the_pages_move() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        let note = DrawNote { tab: id, page: 0, failure: None };
        s.note_drawn(note);
        s.edit_pages(id, |e| e.move_pages(0, 1, 2)).unwrap();
        assert_eq!(s.draw_note(id, 0), None, "after an edit");
        s.note_drawn(note);
        assert!(s.undo_edit(id));
        assert_eq!(s.draw_note(id, 0), None, "after an undo");
        s.note_drawn(note);
        assert!(s.redo_edit(id));
        assert_eq!(s.draw_note(id, 0), None, "after a redo");
        s.note_drawn(note);
        s.discard_edits(id);
        assert_eq!(s.draw_note(id, 0), None, "after a discard");
        s.note_drawn(note);
        assert_eq!(s.edit_pages(id, |e| e.rotate(99, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
        assert_eq!(s.draw_note(id, 0), Some(note), "a refused edit changes nothing");
    }

    /// Failure: a tab back at its file through four quarter turns still reads as edited, and the sidebar's dot and the refusal to close stay with it. Its steps can still be undone, because the list isn't dropped while it has any. And a `page move` to where it already is must leave no list behind at all.
    #[test]
    fn a_tab_back_at_its_file_isnt_edited() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(id, |e| e.move_pages(2, 1, 2)).unwrap();
        assert!(s.page_edits(id).is_none() && !s.sidebar().edited, "nothing happened");
        for _ in 0..4 {
            s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
        }
        assert!(!s.is_edited(id) && !s.sidebar().edited);
        assert!(s.page_edits(id).unwrap().can_undo(), "it can still step back through them");
        assert!(s.undo_edit(id));
        assert!(s.is_edited(id));
        assert!(s.redo_edit(id));
        assert!(s.close(id).is_ok(), "it matches the file, and nothing's been undone");
    }

    /// Failure: a refused edit leaves an 8-byte-a-page list behind forever, over the budget for an idle tab, and the tab's length stops following its file.
    #[test]
    fn a_refused_first_edit_leaves_no_list() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert_eq!(s.edit_pages(id, |e| e.rotate(999, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
        assert!(s.page_edits(id).is_none(), "a tab with no edits must cost nothing here");
        s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
        assert_eq!(s.edit_pages(id, |e| e.rotate(999, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
        assert!(s.page_edits(id).is_some(), "a real edit stays");
        assert!(s.undo_edit(id));
        assert!(s.page_edits(id).is_some(), "it can be redone");
        assert!(s.redo_edit(id));
        s.discard_edits(id);
        assert!(s.page_edits(id).is_none());
    }

    /// Failure: a file replaced behind an edited tab, or behind an added PDF, is read at the old page numbers as if nothing happened. A tab with no edits has nothing to get wrong, and reads the file as it is.
    #[test]
    fn a_page_knows_whether_its_file_is_the_one_it_was_numbered_from() {
        let dir = std::env::temp_dir().join(format!("recto-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (own, added) = (dir.join("own.pdf"), dir.join("added.pdf"));
        std::fs::write(&own, b"one").unwrap();
        std::fs::write(&added, b"two").unwrap();
        let mut s = session(3);
        let id = s.open(&own).unwrap();
        assert_eq!(s.page_source(id, 0).unwrap().expected, None, "no edits, so no claim");
        s.insert_pdf(id, 0, &added).unwrap();
        assert!(s.page_source(id, 0).unwrap().added);
        assert!(s.page_source(id, 0).unwrap().is_current());
        assert!(s.page_source(id, 3).unwrap().is_current());
        assert!(s.own_file_is_current(id));
        std::fs::write(&added, b"two, longer").unwrap();
        assert!(!s.page_source(id, 0).unwrap().is_current(), "the added file changed");
        assert!(s.page_source(id, 3).unwrap().is_current(), "the tab's own file hasn't");
        assert_eq!(s.insert_pdf(id, 0, &added), Err(SessionError::Edit(EditError::SourceChanged)), "two versions can't share page numbers");
        std::fs::remove_file(&added).unwrap();
        assert!(!s.page_source(id, 0).unwrap().is_current(), "the added file's gone");
        std::fs::write(&own, b"one, longer").unwrap();
        assert!(!s.page_source(id, 3).unwrap().is_current());
        assert!(!s.own_file_is_current(id));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a tab with added pages wakes from cold and lands back inside the file's own length, so the reader jumps away from the page they were on.
    #[test]
    fn waking_an_edited_tab_keeps_its_place_in_the_edited_length() {
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.insert_pdf(a, 5, Path::new("/d/added.pdf")).unwrap();
        s.go_to(a, 8).unwrap();
        for i in 0..MAX_WARM + 1 {
            s.open(Path::new(&format!("/d/other{i}.pdf"))).unwrap();
        }
        assert_eq!(s.tier(a), Tier::Cold);
        s.tap(a).unwrap();
        assert_eq!(s.tab(a).unwrap().page, 8);
    }

    /// Failure: after a move or a delete, reading "page 2" reads the file's second page rather than the one now in that place.
    #[test]
    fn a_pages_source_follows_edits() {
        let mut s = session(3);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let three = s.page_source(a, 2).unwrap();
        assert_eq!((three.path.as_path(), three.page, three.quarter_turns, three.added), (Path::new("/d/a.pdf"), 2, 0, false));
        assert_eq!(s.page_source(a, 3), None);
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        let first = s.page_source(a, 0).unwrap();
        assert_eq!((first.path.as_path(), first.page, first.quarter_turns, first.added), (Path::new("/d/a.pdf"), 1, 1, false));
        s.insert_pdf(a, 0, Path::new("/d/b.pdf")).unwrap();
        let second = s.page_source(a, 1).unwrap();
        assert_eq!((second.path.as_path(), second.page, second.added), (Path::new("/d/b.pdf"), 1, true));
    }

    struct RendererOnlyLoader;

    impl DocLoader for RendererOnlyLoader {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts { page_count: 4, byte_len: 1, encrypted: false, version: "".into(), title: None, toc: Toc::default(), renderer_only: true })
        }
    }

    /// Failure: a file only the renderer can read gets edited or added to another, though nothing could ever save the result. One writer: it must be readable and drawable, never editable.
    #[test]
    fn a_renderer_only_file_can_be_opened_but_not_edited_or_added() {
        let mut s = Session::new(RendererOnlyLoader);
        let id = s.open(Path::new("/d/damaged.pdf")).unwrap();
        assert_eq!(s.pages_in(id), 4);
        assert!(s.tab(id).unwrap().renderer_only);
        assert_eq!(s.edit_pages(id, |e| e.rotate(0, 1, 1)), Err(SessionError::RendererOnly));
        assert!(!s.is_edited(id));
        let mut other = session(3);
        let target = other.open(Path::new("/d/fine.pdf")).unwrap();
        let facts = RendererOnlyLoader.load(Path::new("/d/damaged.pdf")).unwrap();
        assert_eq!(other.insert_pdf_with_facts(target, Author::Owner, 0, Path::new("/d/damaged.pdf"), &facts, None), Err(SessionError::RendererOnly));
        assert!(!other.is_edited(target));
    }

    /// Failure: the draw notes grow with a long session, or an old result survives its tab, so a waiter is told a closed tab's page is drawn.
    #[test]
    fn draw_notes_are_capped_and_go_with_their_tab() {
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        for page in 0..(MAX_DRAWS_KEPT as u32 * 2) {
            s.note_drawn(DrawNote { tab: a, page, failure: None });
        }
        assert_eq!(s.draws.len(), MAX_DRAWS_KEPT);
        assert!(s.draw_note(a, 0).is_none());
        let last = MAX_DRAWS_KEPT as u32 * 2 - 1;
        assert!(s.draw_note(a, last).is_some());
        s.close(a).unwrap();
        assert!(s.draw_note(a, last).is_none());
    }

    /// Failure: a file with no contents of its own gets no offer, or the offer, the list while it's made and the generated list are told apart only by guessing. And a generated list must never take the place of the file's own once the reader goes back to them, nor follow a closed tab.
    #[test]
    fn generated_contents_are_offered_labelled_and_never_replace_the_files_own() {
        let mut s = session(40);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert_eq!(s.contents_verdict(id), Some(ContentsVerdict::Missing));
        fn note(s: &Session<FakeLoader>) -> Option<(String, Option<ContentsAction>)> {
            s.sidebar().rows.iter().find(|row| row.kind == RowKind::Note).map(|row| (row.text.to_string(), row.action))
        }
        assert_eq!(note(&s), Some(("Generate contents".into(), Some(ContentsAction::Generate))));
        s.choose_generated(id, true);
        s.set_generated_view(id, GeneratedView::Making);
        assert_eq!(note(&s).map(|(_, action)| action), Some(Some(ContentsAction::Stop)), "the reader can stop them");
        let lines = [TocLine { title: "1 Introduction", page: 0, parent: NONE, depth: 0 }, TocLine { title: "2 Method", page: 5, parent: NONE, depth: 0 }];
        s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
        let model = s.sidebar();
        assert_eq!(model.rows.iter().filter(|row| row.kind == RowKind::Contents).map(|row| row.text).collect::<Vec<_>>(), ["1 Introduction", "2 Method"]);
        assert_eq!(note(&s), Some(("Generated contents".into(), None)), "labelled, and with no contents of its own there's nothing to go back to");
        assert_eq!(model.rail_filled, 1, "the rail counts entries, not the note");
        assert!(s.to_record().tabs[0].generated_contents, "the choice is kept");
        s.choose_generated(id, false);
        assert_eq!(note(&s), Some(("Show generated contents".into(), Some(ContentsAction::ShowGenerated))));
        assert!(s.sidebar().rows.iter().all(|row| row.kind != RowKind::Contents), "the file's own, which are none");
        // A file changed underneath: the old version's list goes, as an agent's `contents --generated` would find none for the new one.
        s.choose_generated(id, true);
        s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
        s.apply_file_status(id, FileStatus::Changed(FileStamp { size: 8, mtime_ns: 8, device: 1, inode: 2 }));
        let before = s.own_stamp(id);
        s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
        s.apply_file_status(id, FileStatus::Changed(FileStamp { size: 9, mtime_ns: 9, device: 1, inode: 2 }));
        assert!(s.generated_view(id).is_none(), "an old version's list stayed");
        // A look that started before the change comes back after it: about the old version, so it's dropped.
        assert!(before.is_some() && s.own_stamp(id) != before);
        s.set_generated_view_for(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))), before);
        assert!(s.generated_view(id).is_none(), "a late answer about the old version was shown");
        s.set_generated_view_for(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))), s.own_stamp(id));
        assert!(s.generated_view(id).is_some(), "an answer about this version is shown");

        s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
        s.go_dormant(id);
        assert!(s.generated_view(id).is_none(), "a dormant tab held its list");
        // Another tab shown: this one's list leaves memory.
        s.set_generated_view(id, GeneratedView::Ready(Arc::new(Toc::from_lines(&lines, false))));
        let other = s.open(Path::new("/d/b.pdf")).unwrap();
        s.tap(other).unwrap();
        assert!(s.generated_view(id).is_none(), "a tab off screen held its list");
        s.close(id).unwrap();
        assert!(s.generated_view(id).is_none());
    }

    /// Failure: a section's chevron doesn't fold it, or opens one following didn't, or the fold is lost across a restart, or a fold on a list survives switching to the other list, where its entry number means something else. Owner, 2026-11-07: "there should be a collapse toggle button for sections with sub chapters on the left side of the headers".
    #[test]
    fn contents_sections_fold_and_their_folds_come_back() {
        struct Outlined;
        impl DocLoader for Outlined {
            fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
                let lines = [TocLine { title: "One", page: 0, parent: NONE, depth: 0 }, TocLine { title: "One a", page: 1, parent: 0, depth: 1 }, TocLine { title: "Two", page: 5, parent: NONE, depth: 0 }, TocLine { title: "Two a", page: 6, parent: 2, depth: 1 }];
                Ok(DocFacts { page_count: 10, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::from_lines(&lines, false), renderer_only: false })
            }
        }
        fn shown(s: &Session<Outlined>) -> Vec<(String, bool, bool)> {
            s.sidebar().rows.iter().filter(|row| row.kind == RowKind::Contents).map(|row| (row.text.to_string(), row.expandable, row.expanded)).collect()
        }
        let mut s = Session::new(Outlined);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        // On page 1, in "One": it's open by following, and "Two" isn't.
        assert_eq!(shown(&s), [("One".into(), true, true), ("One a".into(), false, false), ("Two".into(), true, false)]);
        s.toggle_contents_section(id, 0);
        s.toggle_contents_section(id, 2);
        assert_eq!(shown(&s), [("One".into(), true, false), ("Two".into(), true, true), ("Two a".into(), false, false)]);
        s.toggle_contents_section(id, 1);
        assert_eq!(shown(&s).len(), 3, "an entry with nothing under it doesn't fold");
        let record = s.to_record();
        assert_eq!(record.tabs[0].contents_folds, [(0, false), (2, true)]);
        let mut again = Session::new(Outlined);
        again.restore(&record, HashMap::new()).unwrap();
        again.wake_active();
        assert_eq!(shown(&again), shown(&s), "the folds come back");
        // Another version of the file: its entries aren't these, so the folds go.
        again.apply_file_status(again.active().unwrap(), FileStatus::Changed(FileStamp { size: 9, mtime_ns: 9, device: 1, inode: 2 }));
        assert!(again.to_record().tabs[0].contents_folds.is_empty(), "folds by entry number stayed for another version");
        // Back to what following does is no fold at all.
        s.toggle_contents_section(id, 0);
        s.toggle_contents_section(id, 2);
        assert!(s.to_record().tabs[0].contents_folds.is_empty());
        s.toggle_contents_section(id, 2);
        s.choose_generated(id, true);
        assert!(s.to_record().tabs[0].contents_folds.is_empty(), "the other list's entries aren't these");
    }

    /// Failure: a thousand tabs that each had generated contents asked for hold every list in memory.
    #[test]
    fn generated_lists_held_are_capped() {
        let mut s = session(40);
        let ids: Vec<TabId> = (0..20).map(|n| s.open(Path::new(&format!("/d/{n}.pdf"))).unwrap()).collect();
        for id in &ids {
            s.set_generated_view(*id, GeneratedView::Making);
        }
        assert!(s.generated.len() <= MAX_GENERATED_VIEWS);
        assert!(s.generated_view(*ids.last().unwrap()).is_some());
    }

    /// Failure: contents that are fine get an offer, so the sidebar nags about a good file.
    #[test]
    fn fine_contents_get_no_note() {
        assert_eq!(contents_note(ContentsVerdict::Fine, None, false, 12), None);
        assert!(contents_note(ContentsVerdict::LooksBad(recto_core::verdict::BadReason::TooFew), None, false, 1).is_some());
    }

    /// Failure: a field added carelessly raises the cost of every visible row.
    #[test]
    fn sidebar_row_stays_small() {
        assert!(std::mem::size_of::<SidebarRow<'_>>() <= 64);
    }

    /// Failure: tapping the active PDF switches tabs or does nothing, instead of folding its contents.
    #[test]
    fn tapping_the_active_pdf_toggles_contents() {
        let mut s = session(10);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert!(s.tab(id).unwrap().toc_open);
        s.tap(id).unwrap();
        assert!(!s.tab(id).unwrap().toc_open);
        assert_eq!(s.active(), Some(id));
        s.tap(id).unwrap();
        assert!(s.tab(id).unwrap().toc_open);
    }

    /// Failure: tapping another PDF folds the current one instead of switching, or forgets its folded state.
    #[test]
    fn tapping_another_pdf_switches_and_keeps_each_fold_state() {
        let mut s = session(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.tap(a).unwrap();
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        s.tap(a).unwrap();
        assert_eq!(s.active(), Some(a));
        assert!(!s.tab(a).unwrap().toc_open);
        assert!(s.tab(b).unwrap().toc_open);
    }

    /// Failure: contents stay in memory for every tab, so many large PDFs cost memory while idle.
    #[test]
    fn only_a_few_tabs_stay_warm() {
        let mut s = session(10);
        let ids: Vec<_> = (0..20).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
        assert_eq!(s.tier(*ids.last().unwrap()), Tier::Hot);
        let warm = ids.iter().filter(|id| s.tier(**id) == Tier::Warm).count();
        assert!(warm < MAX_WARM);
        assert!(ids[..15].iter().all(|id| s.tier(*id) == Tier::Cold));
    }

    /// Failure: returning to a recent tab re-reads the file, or a cold tab isn't rebuilt from disk.
    #[test]
    fn switching_rebuilds_only_cold_tabs() {
        let mut s = session(10);
        let ids: Vec<_> = (0..5).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
        let before = s.loader.loads.get();
        s.tap(ids[3]).unwrap();
        assert_eq!(s.loader.loads.get(), before, "warm tab read the file again");
        s.tap(ids[0]).unwrap();
        assert_eq!(s.loader.loads.get(), before + 1, "cold tab wasn't rebuilt");
    }

    /// Failure: opening the same file twice makes a second tab rather than going to the first.
    #[test]
    fn opening_a_file_twice_reuses_its_tab() {
        let mut s = session(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.open(Path::new("/d/b.pdf")).unwrap();
        assert_eq!(s.open(Path::new("/d/a.pdf")).unwrap(), a);
        assert_eq!(s.tabs().len(), 2);
        assert_eq!(s.active(), Some(a));
    }

    /// Failure: a page past the end leaves the reader nowhere, or a zero-page file underflows.
    #[test]
    fn going_to_a_page_clamps_to_the_document() {
        let mut s = session(10);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.go_to(id, 500).unwrap();
        assert_eq!(s.tab(id).unwrap().page, 9);
        assert_eq!(s.sidebar().place, "10 of 10");
        let mut empty = session(0);
        let id = empty.open(Path::new("/d/empty.pdf")).unwrap();
        empty.go_to(id, 3).unwrap();
        assert_eq!(empty.tab(id).unwrap().page, 0);
    }

    /// Failure: closing the active tab leaves nothing active while others are open, or leaves its contents in memory.
    #[test]
    fn closing_the_active_tab_activates_a_neighbour() {
        let mut s = session(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        s.close(b).unwrap();
        assert_eq!(s.active(), Some(a));
        assert!(s.warm.iter().all(|(id, _)| *id != b));
        s.close(a).unwrap();
        assert_eq!(s.active(), None);
        assert_eq!(s.sidebar().place, "");
    }

    /// Failure: closing a tab loses page edits without warning.
    #[test]
    fn closing_refuses_while_there_are_unsaved_edits() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
        assert_eq!(s.close(id), Err(SessionError::UnsavedChanges));
        assert!(s.sidebar().edited);
        assert!(s.undo_edit(id));
        assert!(!s.sidebar().edited);
        // Failure: an agent's `undo` takes the owner's edit off the page, so the dot goes and this close goes through with no prompt, and the edit is gone for good.
        assert!(s.sidebar().undone_owner_steps, "the app must be able to show it");
        assert_eq!(s.close(id), Err(SessionError::UnsavedChanges), "the owner's edit can still be redone, so closing would lose it");
        s.discard_edits(id);
        assert!(!s.sidebar().undone_owner_steps);
        assert!(s.close(id).is_ok());
    }

    /// Failure: undone steps of an agent's own hold up the app's close, or read as the owner's. They're an agent's to lose.
    #[test]
    fn an_agents_undone_steps_dont_stop_a_close() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages_as(id, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
        assert!(s.undo_edit(id));
        assert!(!s.has_undone_owner_steps(id) && !s.sidebar().undone_owner_steps);
        assert!(s.close(id).is_ok());
    }

    /// Failure: the footer and page clamp ignore added or deleted pages, so the reader lands past the end.
    #[test]
    fn page_count_and_position_follow_edits() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.go_to(id, 4).unwrap();
        s.edit_pages(id, |e| e.delete(1, 3)).unwrap();
        assert_eq!(s.sidebar().place, "2 of 2");
        s.insert_pdf(id, 2, Path::new("/d/b.pdf")).unwrap();
        assert_eq!(s.pages_in(id), 7);
        assert_eq!(s.sidebar().place, "2 of 7");
        s.discard_edits(id);
        assert_eq!(s.sidebar().place, "5 of 5", "the reader stays on the page they were reading");
    }

    /// Failure: every open tab pays for page edits, so idle tabs cost more once one PDF is edited.
    #[test]
    fn only_edited_tabs_have_an_edit_list() {
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.open(Path::new("/d/b.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        assert_eq!(s.edits.len(), 1);
    }

    /// Failure: a dropped PDF that can't be read changes the document anyway.
    #[test]
    fn a_dropped_pdf_that_cant_be_read_changes_nothing() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert_eq!(s.insert_pdf(id, 0, Path::new("/d/missing.pdf")), Err(SessionError::Open(OpenError::NotFound)));
        assert!(!s.is_edited(id));
        assert_eq!(s.pages_in(id), 5);
    }

    /// Failure: a PDF moved away after opening crashes or disappears, rather than staying open and saying why.
    #[test]
    fn a_tab_whose_file_vanished_stays_and_says_why() {
        let mut s = session(10);
        let gone = s.open(Path::new("/d/missing-later.pdf"));
        assert!(gone.is_err());
        let ids: Vec<_> = (0..5).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
        // Rename the first tab's file underneath the session, then wake it from cold.
        s.tab_mut(ids[0]).unwrap().file_name = OsStr::new("missing.pdf").into();
        s.tap(ids[0]).unwrap();
        assert_eq!(s.tab(ids[0]).unwrap().problem, Some(OpenError::NotFound));
        assert_eq!(s.active(), Some(ids[0]));
    }

    fn journaled(pages: u32) -> Session<FakeLoader> {
        let mut s = session(pages);
        s.set_journaling(true);
        s
    }

    /// What the journal-writing thread does, into plain memory: what comes back from a crash is whatever it had got to.
    fn write_journal(s: &mut Session<FakeLoader>, files: &mut HashMap<String, Vec<u8>>) {
        let work = s.journal_work();
        for name in work.retired {
            files.remove(&name);
        }
        let mut outcome = JournalOutcome::default();
        for append in work.appends {
            if let Some(base) = &append.base {
                let header = JournalHeader { base_seq: append.through, ..append.header.clone() };
                files.insert(append.name.clone(), record::journal_start(&header, Some(base)));
                if let Some(old) = append.replaces.as_ref().filter(|old| **old != append.name) {
                    files.remove(old);
                }
            } else {
                let file = files.entry(append.name.clone()).or_insert_with(|| record::journal_start(&append.header, None));
                file.extend_from_slice(&record::journal_lines(&append.ops));
            }
            outcome.written.push((append.tab, append.through, append.name));
        }
        s.journal_outcome(outcome);
    }

    fn back_from(files: &HashMap<String, Vec<u8>>) -> HashMap<String, RestoredEdits> {
        files
            .iter()
            .map(|(name, bytes)| {
                let parsed = record::parse_journal(bytes, record::CURRENT_VERSION).unwrap();
                let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
                assert_eq!(applied, parsed.ops.len());
                (name.clone(), RestoredEdits { edits, seq: parsed.header.base_seq + applied as u64, rolled_off: false, lost_lines: parsed.lost_lines })
            })
            .collect()
    }

    fn with_owner_and_agent_edits(s: &mut Session<FakeLoader>) -> (TabId, TabId) {
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        s.edit_pages_as(a, Author::Agent, |e| e.delete(4, 1)).unwrap();
        s.edit_pages(a, |e| e.move_pages(0, 1, 1)).unwrap();
        s.edit_pages_as(a, Author::Agent, |e| e.rotate(2, 1, 3)).unwrap();
        assert!(s.undo_edit(a));
        assert!(s.undo_edit(a));
        s.go_to(a, 2).unwrap();
        s.tap(a).unwrap();
        s.tap(a).unwrap();
        (a, b)
    }

    /// Failure: the panel's Rotate outside edit mode turns one page, or takes a step a page, so one undo doesn't take it back; or in edit mode it turns every page. Owner, 2026-11-07: "When not in edit mode rotate page will rotate all pages together (it still enables save to commit the adjustment)".
    #[test]
    fn the_panels_rotate_turns_every_page_as_one_step() {
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        assert!(s.can_edit_pages(a));
        s.rotate_from_panel(a, 1).unwrap();
        assert!(s.page_edits(a).unwrap().slots().iter().all(|slot| slot.quarter_turns == 1), "every page");
        assert!(s.is_edited(a), "it's an unsaved edit, for Save to commit");
        assert!(s.undo_edit_as(a, Author::Owner).unwrap());
        assert!(s.page_edits(a).unwrap().slots().iter().all(|slot| slot.quarter_turns == 0), "one undo takes it all back");
        assert!(!s.undo_and_redo(a).0, "it was one step");
        s.set_edit_mode(true);
        s.go_to(a, 3).unwrap();
        s.rotate_from_panel(a, -1).unwrap();
        let turns: Vec<u8> = s.page_edits(a).unwrap().slots().iter().map(|slot| slot.quarter_turns).collect();
        assert_eq!(turns, [0, 0, 0, 3, 0], "edit mode turns the page being read");
    }

    /// Failure: a record from a build with focus view, written with it on, comes back with the sidebar showing, or the next record still says focus, so it can't be turned off. Owner, 2026-11-08: focus view became the sidebar's button.
    #[test]
    fn an_older_records_focus_view_comes_back_as_a_hidden_sidebar() {
        let old: record::ViewRecord = serde_json::from_str(r#"{"paper":null,"focus":true,"agent_icon_hidden":false}"#).unwrap();
        let mut saved = journaled(1).to_record();
        saved.view = old;
        let mut back = journaled(1);
        back.restore(&saved, HashMap::new()).unwrap();
        assert!(back.view().sidebar_hidden);
        let written = serde_json::to_string(&back.to_record().view).unwrap();
        assert!(!written.contains("focus") && written.contains("\"sidebar_hidden\":true"), "{written}");
        back.set_view(ViewSettings { sidebar_hidden: false, ..back.view() });
        let mut again = journaled(1);
        again.restore(&back.to_record(), HashMap::new()).unwrap();
        assert!(!again.view().sidebar_hidden, "shown again stays shown");
    }

    /// Failure: anything about the session is different after going through its record and journals: a tab, its order, the active one, a place, a fold, unsaved edits, the steps undone, or who made them. Compares the whole of both.
    #[test]
    fn a_session_comes_back_whole_from_its_record_and_journals() {
        let mut s = journaled(5);
        let (a, _b) = with_owner_and_agent_edits(&mut s);
        s.note_place(a, PlaceRecord { page: 2, anchor_page: 2, within: 0.25, x: 3.5, y: 1234.5, document_height: 9000.0, zoom: 1.5625, page_width: 612.0, page_height: 792.0 });
        s.set_find(true, "ran", "typed but not run");
        s.set_edit_mode(true);
        s.set_view_rotation(a, 1);
        assert!(s.set_spread(a, record::Spread::TwoUpCover));
        assert!(!s.set_spread(a, record::Spread::TwoUpCover), "the same again changes nothing");
        s.set_selection(a, Some(Selection { from_page: 1, from_char: 2, to_page: 2, to_char: 9 }));
        s.set_view(ViewSettings { paper: Some(Paper::Warm), sidebar_hidden: true, agent_icon_hidden: true });
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        let bytes = record::encode_record(&s.to_record(), 7);
        let read = record::decode_record(&bytes, record::CURRENT_VERSION).unwrap();
        let mut back = journaled(5);
        back.restore(&read.record, back_from(&files)).unwrap();
        assert_eq!(back.to_record(), s.to_record());
        assert_eq!(back.edits_record(a), s.edits_record(a));
        assert_eq!(back.view(), s.view());
        assert_eq!(back.spread_of(a), Some(record::Spread::TwoUpCover), "a tab's layout comes back with it");
        assert_eq!(back.active(), s.active());
        assert_eq!(back.tier(a), Tier::Hot, "it's the active one");
        assert!(back.warm_tabs().next().is_none(), "and nothing's been opened");
        assert_eq!(back.page_edits(a).unwrap().slots(), s.page_edits(a).unwrap().slots());
        // Redo works, after a restart, and whose each step is holds for what's then allowed.
        assert!(back.page_edits(a).unwrap().can_redo());
        assert!(back.has_undone_owner_steps(a), "the owner's move is undone, and still theirs");
        assert!(back.redo_edit(a));
        assert!(!back.has_undone_owner_steps(a));
        assert!(back.redo_edit(a));
        assert!(s.redo_edit(a) && s.redo_edit(a));
        assert_eq!(back.page_edits(a).unwrap().slots(), s.page_edits(a).unwrap().slots());
        // What comes next goes on the same journal, with its count following on.
        back.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
        s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
        write_journal(&mut back, &mut files);
        let mut third = journaled(5);
        third.restore(&back.to_record(), back_from(&files)).unwrap();
        assert_eq!(third.edits_record(a), s.edits_record(a));
    }

    /// Failure: an unsaved edit depends on a step that's rolled off, so going past the limit and crashing brings back a different document. Also the step-by-step promise: past the limit, undoing goes back exactly as far as the limit, and at each point the document is what it was then. Long, as real use isn't: 3,000 steps, against the limit of 1,000.
    #[test]
    fn past_the_history_limit_the_document_is_still_exact() {
        const STEPS: u32 = 3000;
        let mut s = journaled(40);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let mut seen: Vec<Vec<edit::PageSlot>> = vec![(0..40).map(|page| edit::PageSlot { source: 0, page, quarter_turns: 0 }).collect()];
        for step in 0..STEPS {
            let at = step % 39;
            match step % 3 {
                0 => s.edit_pages(a, |e| e.rotate(at, 1, 1)).unwrap(),
                1 => s.edit_pages(a, |e| e.move_pages(at, 1, 39 - at)).unwrap_or(()),
                // An agent's turn is refused while the owner's step is what would go, and that's a step not taken.
                _ => match s.edit_pages_as(a, Author::Agent, |e| e.rotate(39 - at, 1, 3)) {
                    Ok(()) | Err(SessionError::Edit(EditError::HistoryFull)) => {}
                    Err(error) => panic!("{error:?}"),
                },
            }
            seen.push(s.page_edits(a).unwrap().slots().to_vec());
        }
        assert!(s.page_edits(a).unwrap().history_dropped());
        // Crash here, with the journal as far as it's got, and compare the document exactly.
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        let mut back = journaled(40);
        back.restore(&s.to_record(), back_from(&files)).unwrap();
        assert_eq!(back.edits_record(a), s.edits_record(a), "the document and the rolled history");
        // Undoing goes back exactly the limit's worth, each stop as it was.
        let mut undone = 0;
        let mut at = seen.len() - 1;
        while back.undo_edit(a) {
            undone += 1;
            at -= 1;
            // A step that did nothing isn't a step, so what's being compared moves back past those.
            while seen[at] == seen[at + 1] {
                at -= 1;
            }
            assert_eq!(back.page_edits(a).unwrap().slots(), seen[at], "after {undone} undone");
        }
        assert_eq!(undone, edit::MAX_UNDO, "exactly the limit");
        assert!(back.is_edited(a), "what rolled off stays in the document");
    }

    /// Failure: a journal grows without bound through undo and redo with no edit, or a checkpoint moved loses the document or the steps within the limit. A checkpoint is only a tighter way of writing the same thing: what comes back from it is the same list.
    #[test]
    fn moving_a_checkpoint_keeps_everything_within_the_limit() {
        let mut s = journaled(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        for step in 0..edit::MAX_UNDO as u32 + 50 {
            s.edit_pages(a, |e| e.rotate(step % 10, 1, 1)).unwrap();
        }
        assert!(s.undo_edit(a));
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        let old_bytes = files.values().next().unwrap().len();
        let name = files.keys().next().unwrap().clone();
        let checkpoint = s.edits_record(a).unwrap();
        let seq = s.page_edits(a).unwrap().journal_seq();
        let header = JournalHeader::new(a.0, Path::new("/d/a.pdf"), None, 10, seq);
        files.insert(name, record::journal_start(&header, Some(&checkpoint)));
        let mut back = journaled(10);
        back.restore(&s.to_record(), back_from(&files)).unwrap();
        assert_eq!(back.edits_record(a), s.edits_record(a));
        assert!(back.page_edits(a).unwrap().can_redo());
        assert_eq!(back.page_edits(a).unwrap().journal_seq(), seq);
        assert!(files.values().next().unwrap().len() < old_bytes * 4, "no bigger than a few times what it was");
    }

    /// Failure: discarding leaves the journal behind, so what was removed stays recoverable, or a reopened PDF gets it back. Or a tab's edits undone back to the file, with no steps left, keep a file for nothing. Or a tab closed with history still to go back through loses it, which is what's kept for the PDF's next opening.
    #[test]
    fn a_journal_goes_when_its_edits_do() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let mut files = HashMap::new();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        write_journal(&mut s, &mut files);
        assert_eq!(files.len(), 1);
        s.discard_edits(a);
        assert!(s.has_retired_journals());
        write_journal(&mut s, &mut files);
        assert!(files.is_empty(), "gone with the discard");
        // An agent's step, undone: closes with no prompt, and its way forward stays for next time.
        s.edit_pages_as(a, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
        assert!(s.undo_edit(a));
        write_journal(&mut s, &mut files);
        s.close(a).unwrap();
        write_journal(&mut s, &mut files);
        assert_eq!(files.len(), 1, "kept for the PDF's next opening");
        let reopened = s.open(Path::new("/d/a.pdf")).unwrap();
        let (name, restored) = back_from(&files).into_iter().next().unwrap();
        s.take_up_history(reopened, &name, restored.edits, restored.seq);
        assert!(s.page_edits(reopened).unwrap().can_redo(), "picked up");
        // A journal has one owner: another tab asking for it gets nothing.
        let other = s.open(Path::new("/d/b.pdf")).unwrap();
        let (name, restored) = back_from(&files).into_iter().next().unwrap();
        s.take_up_history(other, &name, restored.edits, restored.seq);
        assert!(s.page_edits(other).is_none());
        assert!(s.redo_edit(reopened));
    }

    fn kept(name: &str, size: u64) -> edit::AddedSource {
        edit::AddedSource { path: PathBuf::from(format!("/state/journals/{name}")).into_boxed_path(), page_count: 5, stamp: Some(FileStamp { size, mtime_ns: 1, device: 1, inode: 7 }), kept: true, protected: false, holds: None }
    }

    const KEPT_A: &str = "kept-00000000000000aa.pdf";
    const KEPT_B: &str = "kept-00000000000000bb.pdf";

    /// Failure (decision 25): a Save ends the history, so there's no changing your mind after it, or the tab still reads as edited, or undoing past the save reads as saved, or coming back reads as unsaved. A failed save, which calls nothing, changes nothing.
    #[test]
    fn a_save_marks_a_point_that_undo_goes_past_and_comes_back_to() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        s.edit_pages_as(a, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
        let mut files = HashMap::new();
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        let keeping = kept(KEPT_A, 100);
        assert!(s.page_edits(a).unwrap().needs_own_pages(), "the delete's way back names a page of this file");
        s.save_prepared(a, target, Some(&keeping));
        write_journal(&mut s, &mut files);
        let parsed = record::parse_journal(files.values().next().unwrap(), record::CURRENT_VERSION).unwrap();
        assert_eq!(parsed.ops.last(), Some(&JournalOp::Saving(target.into())), "said ahead of it, in the journal");
        assert!(matches!(parsed.ops.iter().rev().nth(1), Some(JournalOp::Keeping(said)) if said.kept), "and what's kept for it");
        // A failed save: nothing's called, and all of it is as it was.
        assert!(s.is_edited(a) && s.page_edits(a).unwrap().can_undo());
        let before: Vec<_> = (0..4).map(|at| s.page_source(a, at).unwrap().page).collect();
        assert_eq!(s.saved(a, target, 4, None, Some(keeping)), 0, "nothing had to go");
        assert!(!s.is_edited(a), "it reads as saved");
        assert!(!s.sidebar().edited);
        assert_eq!(s.pages_in(a), 4);
        assert_eq!(s.own_stamp(a), Some(target));
        assert_eq!(s.tab(a).unwrap().page_count, 4);
        assert_eq!(before, [1, 2, 3, 4], "those were pages of the old file");
        assert_eq!((0..4).map(|at| s.page_source(a, at).unwrap().page).collect::<Vec<_>>(), [0, 1, 2, 3], "and the new file has them in order");
        assert!(s.page_edits(a).unwrap().can_undo(), "the history's still there");
        // Past it, one step at a time, and back.
        assert!(s.undo_edit(a));
        assert!(s.is_edited(a), "undone past the saved point, it's unsaved again");
        assert_eq!(s.page_source(a, 0).unwrap().quarter_turns, 3, "the turn's in the file now, and undoing it is a turn back");
        assert!(s.undo_edit(a));
        assert_eq!(s.pages_in(a), 5);
        let back = s.page_source(a, 0).unwrap();
        assert_eq!((back.path.as_path(), back.page, back.added), (Path::new("/state/journals/kept-00000000000000aa.pdf"), 0, true), "the page the save took out comes from what was kept, not the new file");
        assert_eq!((s.page_source(a, 1).unwrap().path.as_path(), s.page_source(a, 1).unwrap().page), (Path::new("/d/a.pdf"), 0), "and the rest from the file");
        assert!(s.is_edited(a));
        assert!(!s.undo_edit(a));
        assert!(s.redo_edit(a));
        assert!(s.is_edited(a));
        assert!(s.redo_edit(a));
        assert!(!s.is_edited(a), "back at the saved point, it reads as saved");
        assert_eq!((0..4).map(|at| s.page_source(a, at).unwrap().page).collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert_eq!(s.page_source(a, 0).unwrap().quarter_turns, 0);
    }

    /// Failure: after a save, the journal's still named for the old file and its page numbers, or a crash loses the history, so a PDF opened again gets the wrong steps or none. The new journal has the whole list under the new file's name, and replays to exactly what's in memory, and more edits go on from it.
    #[test]
    fn a_saved_history_is_a_new_journal_that_comes_back_the_same() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.delete(1, 2)).unwrap();
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        let old = files.keys().next().unwrap().clone();
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        s.saved(a, target, 3, None, Some(kept(KEPT_A, 100)));
        assert!(s.has_retired_journals(), "it wants writing at once, as a crash would otherwise find the old one");
        // Not written yet: the old one's still the only one, and still there.
        assert!(files.contains_key(&old));
        write_journal(&mut s, &mut files);
        assert!(!files.contains_key(&old), "it goes once the new one's there");
        assert_eq!(files.len(), 1);
        assert!(!s.has_retired_journals());
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        write_journal(&mut s, &mut files);
        let parsed = record::parse_journal(files.values().next().unwrap(), record::CURRENT_VERSION).unwrap();
        assert_eq!(parsed.header.file, Some(target.into()), "it's about the new file");
        assert_eq!(parsed.header.file_pages, 3);
        let (replayed, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
        assert_eq!(applied, parsed.ops.len());
        assert_eq!(replayed.to_record(), s.edits_record(a).unwrap());
        assert_eq!(replayed.is_edited(), s.is_edited(a));
    }

    /// Failure (decision 25): a step undoes wrongly because the pages it needs are gone from the file, or the app says nothing about undo stopping short. It rolls off, with everything older, as a step's way back depends on those after it, and the tab says so. What came after it stays, and nothing about the document changes.
    #[test]
    fn a_step_whose_pages_cant_be_kept_rolls_off_and_says_so() {
        let mut s = journaled(6);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        s.edit_pages(a, |e| e.delete(5, 1)).unwrap();
        s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        assert_eq!(s.saved(a, target, 5, None, None), 2, "the delete, and the older rotation with it");
        assert!(!s.is_edited(a));
        assert_eq!(s.tab(a).unwrap().notice.as_ref().map(|notice| notice.code()), Some("history_rolled_off"));
        assert!(s.tab_notice_message(a).is_some_and(|words| words.contains("Undo")));
        assert_eq!(s.page_edits(a).unwrap().summary().undo, 1);
        assert!(s.undo_edit(a), "the newer step still works");
        assert!(!s.undo_edit(a), "and undo stops rather than undoing wrongly");
        assert_eq!(s.pages_in(a), 5);
        assert_eq!(s.page_source(a, 1).unwrap().quarter_turns, 3);
        assert!(s.redo_edit(a));
        assert!(!s.is_edited(a));
        // Going to another tab has said it.
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        s.tap(a).unwrap();
        assert!(s.tab(a).unwrap().notice.is_some(), "not yet");
        s.tap(b).unwrap();
        assert_eq!(s.tab(a).unwrap().notice, None);
        // Only a turn and a move in the history: no pages named, so no copy's needed, and nothing rolls off.
        let mut s = journaled(6);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.move_pages(0, 1, 2)).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        assert!(!s.page_edits(a).unwrap().needs_own_pages());
        assert_eq!(s.saved(a, target, 6, None, None), 0);
        assert_eq!(s.tab(a).unwrap().notice, None);
        assert!(s.undo_edit(a) && s.undo_edit(a) && s.is_edited(a));
    }

    /// Failure: copies kept for a PDF go past the limit a history has, over many saves. The oldest goes, with the steps that need it, and its file's let go of.
    #[test]
    fn copies_kept_stay_within_the_limit() {
        let mut s = journaled(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let big = edit::MAX_HISTORY_BYTES as u64 / 2 + 1;
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        assert_eq!(s.saved(a, target, 9, None, Some(kept(KEPT_A, big))), 0);
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        assert_eq!(s.page_edits(a).unwrap().kept_sources().count(), 1);
        assert_eq!(s.saved(a, target, 8, None, Some(kept(KEPT_B, big))), 1, "two wouldn't fit, so the first's step goes");
        let list = s.page_edits(a).unwrap();
        assert_eq!(list.kept_sources().map(|(_, added)| added.path.file_name().unwrap().to_str().unwrap().to_string()).collect::<Vec<_>>(), [KEPT_B]);
        assert_eq!(list.summary().undo, 1);
        assert!(s.journal_work().retired.iter().any(|name| name == KEPT_A), "and its file goes");
    }

    /// Failure: Save As loses the history, or its way back names the new file, or the tab's not on the new file. The old file is still where it was, so that's where the pages come from, with no copy.
    #[test]
    fn save_as_takes_the_history_along() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        assert_eq!(s.saved(a, target, 4, Some(Path::new("/e/copy.pdf")), None), 0);
        assert_eq!(s.path_of(a).unwrap(), Path::new("/e/copy.pdf"));
        assert_eq!(&*s.tab(a).unwrap().name, "copy");
        assert!(!s.is_edited(a));
        assert!(s.undo_edit(a));
        assert_eq!((s.page_source(a, 0).unwrap().path.as_path(), s.page_source(a, 0).unwrap().page), (Path::new("/d/a.pdf"), 0));
        assert_eq!((s.page_source(a, 1).unwrap().path.as_path(), s.page_source(a, 1).unwrap().page), (Path::new("/e/copy.pdf"), 0));
        assert_eq!(s.page_edits(a).unwrap().kept_sources().count(), 0, "the old file isn't the history's to remove");
    }

    /// Failure: what was written isn't this list, as its length says, and the steps get laid on a file they're not about. All of it goes, and it says so.
    #[test]
    fn a_save_of_something_else_ends_the_history() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        assert_eq!(s.saved(a, target, 3, None, Some(kept(KEPT_A, 100))), 1);
        assert!(s.page_edits(a).is_none());
        assert_eq!(s.pages_in(a), 3);
        assert_eq!(s.tab(a).unwrap().notice.as_ref().map(|notice| notice.code()), Some("history_rolled_off"));
        assert_eq!(s.journal_work().retired.iter().filter(|name| *name == KEPT_A).count(), 1);
        // A tab with no list has nothing to carry, and a file kept for it goes.
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        assert_eq!(s.saved(b, target, 5, None, Some(kept(KEPT_B, 100))), 0);
        assert_eq!(s.journal_work().retired, [KEPT_B]);
    }

    /// Failure (decision 25): clearing only forgets a history, so its file stays on disk and what was removed can be got back; or unsaved work goes with it; or an agent or a changed file takes what shouldn't go. Each reason does what it's for, and the files go.
    #[test]
    fn clearing_a_history_removes_its_files_and_keeps_unsaved_work() {
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        // After a confirmed redaction, at the saved point: nothing's left.
        for reason in [ClearReason::Redaction, ClearReason::HiddenDataRemoval, ClearReason::UserAsked, ClearReason::Discard] {
            let mut s = journaled(5);
            let a = s.open(Path::new("/d/a.pdf")).unwrap();
            s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
            let mut files = HashMap::new();
            write_journal(&mut s, &mut files);
            s.saved(a, target, 4, None, Some(kept(KEPT_A, 100)));
            write_journal(&mut s, &mut files);
            assert!(s.page_edits(a).is_some_and(PageEdits::can_undo));
            assert_eq!(files.len(), 1);
            assert!(s.clear_history(a, reason));
            assert!(s.page_edits(a).is_none(), "{reason:?}");
            assert!(s.has_retired_journals());
            let work = s.journal_work();
            assert_eq!(work.retired.iter().filter(|name| !record::is_sleep_name(name)).count(), 2, "the journal and the kept file: {reason:?}");
            assert!(work.retired.iter().any(|name| name == KEPT_A));
            assert_eq!(work.retired.iter().any(|name| record::is_sleep_name(name)), matches!(reason, ClearReason::Redaction | ClearReason::HiddenDataRemoval), "a picture of what's removed goes with it: {reason:?}");
            assert_eq!(s.to_record().tabs[0].journal, None);
        }
        // Unsaved edits and Clear Edit History: the page order stays, and the way back goes. The journal starts again in one step, with nothing in it that was only history.
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        s.insert_pdf(a, 0, Path::new("/d/secret.pdf")).unwrap();
        assert!(s.undo_edit(a));
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        assert!(files.values().next().unwrap().windows(10).any(|bytes| bytes == b"secret.pdf"), "it was in the journal");
        assert!(s.clear_history(a, ClearReason::UserAsked));
        assert!(s.is_edited(a), "unsaved work stays");
        assert_eq!(s.pages_in(a), 4);
        assert!(!s.page_edits(a).unwrap().can_undo() && !s.page_edits(a).unwrap().can_redo());
        assert_eq!(s.page_edits(a).unwrap().added_sources().len(), 0, "the added file was only history");
        write_journal(&mut s, &mut files);
        assert_eq!(files.len(), 1, "the same name, replaced");
        let bytes = files.values().next().unwrap();
        assert!(!bytes.windows(10).any(|bytes| bytes == b"secret.pdf"), "it's gone from the file, not only from memory");
        let parsed = record::parse_journal(bytes, record::CURRENT_VERSION).unwrap();
        let (replayed, _) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
        assert_eq!(replayed.to_record(), s.edits_record(a).unwrap());
        let none = s.open(Path::new("/d/none.pdf")).unwrap();
        assert!(!s.clear_history(none, ClearReason::UserAsked), "nothing to clear");
        // A file changed outside: with only history, its steps don't apply, and they go, whichever way it was noticed.
        let now = FileStamp { size: 1, mtime_ns: 2, device: 1, inode: 3 };
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        s.saved(a, target, 5, None, None);
        assert!(s.page_edits(a).is_some_and(PageEdits::can_undo));
        s.apply_file_status(a, FileStatus::Changed(now));
        assert!(s.page_edits(a).is_none());
        assert_eq!(s.own_stamp(a), Some(now));
        assert_eq!(s.tab(a).unwrap().notice.as_ref().map(|notice| notice.code()), Some("file_changed"));
        // With unsaved edits, they stay and say so, as decision 23 has it.
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        s.apply_file_status(a, FileStatus::Changed(target));
        assert!(s.is_edited(a) && s.page_edits(a).unwrap().can_undo());
        assert_eq!(s.own_stamp(a), Some(now), "they're about the file they were made on");
    }

    /// Failure (decision 25): a history stays on a file changed outside the app, when a tab's woken with nothing else having looked, and its steps get laid on the wrong pages.
    #[test]
    fn a_file_changed_under_a_history_clears_it_on_waking() {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-changed-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.pdf");
        std::fs::write(&file, b"as it was").unwrap();
        let mut s = journaled(5);
        let a = s.open(&file).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        assert!(s.undo_edit(a), "so there's history and nothing unsaved");
        write_journal(&mut s, &mut HashMap::new());
        let b = s.open(&dir.join("b.pdf")).unwrap();
        s.tap(b).unwrap();
        std::fs::write(&file, b"another version altogether").unwrap();
        s.warm.clear();
        s.tap(a).unwrap();
        assert!(s.page_edits(a).is_none());
        assert_eq!(s.tab(a).unwrap().notice.as_ref().map(|notice| notice.code()), Some("file_changed"));
        assert_eq!(s.journal_work().retired.iter().filter(|name| record::is_journal_name(name)).count(), 1, "and its file goes");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What a state record's store does, in memory: journals by name, read back as a launch would.
    struct MemorySource(Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>);

    impl HistorySource for MemorySource {
        fn history_of(&self, _stamp: Option<FileStamp>, _path: &Path) -> Option<TakenHistory> {
            None
        }
        fn read_list(&self, name: &str) -> Option<RestoredEdits> {
            let files = self.0.lock().unwrap();
            let parsed = record::parse_journal(files.get(name)?, record::CURRENT_VERSION).ok()?;
            let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).ok()?;
            Some(RestoredEdits { edits, seq: parsed.header.base_seq + applied as u64, lost_lines: false, rolled_off: false })
        }
    }

    /// A session with a list on tab `a`, in its journal, and another tab showing.
    fn with_a_list(files: &Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>) -> (Session<FakeLoader>, TabId, TabId) {
        let mut s = journaled(5);
        s.set_history_source(Arc::new(MemorySource(Arc::clone(files))));
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        s.edit_pages_as(a, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
        assert!(s.undo_edit(a), "with something to redo as well");
        let mut plain = files.lock().unwrap().clone();
        write_journal(&mut s, &mut plain);
        *files.lock().unwrap() = plain;
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        (s, a, b)
    }

    /// Failure (decision 25): a list gone to disk reads as if the tab had none, so its edits look saved, or a close loses them, or an undo finds nothing, or pages come from the wrong place. Every route about a tab finds the list as it was left, and what's in memory till then is only a summary.
    #[test]
    fn a_dormant_tabs_list_goes_to_disk_and_comes_back_by_every_route() {
        let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (mut s, a, _b) = with_a_list(&files);
        let as_it_was = s.edits_record(a).unwrap();
        assert!(s.go_dormant(a));
        assert!(s.list_is_asleep(a));
        assert!(s.page_edits(a).is_none(), "out of memory");
        // A look at it, with nothing read.
        assert_eq!(s.pages_in(a), 4);
        assert!(s.is_edited(a));
        assert!(s.holds_owner_edits());
        assert_eq!(s.to_record().tabs[0].edits.unwrap().pages, 4);
        assert_eq!(s.undo_and_redo(a), (true, true), "and what can be undone, with nothing read");
        assert!(s.to_record().tabs[0].journal.is_some());
        assert_eq!(s.close(a), Err(SessionError::UnsavedChanges), "and a close is refused as it would be: it wakes it to see");
        assert_eq!(s.list_record_anywhere(a), Some(as_it_was.clone()));
        // Each way in.
        type Route = fn(&mut Session<FakeLoader>, TabId);
        let routes: [(&str, Route); 9] = [
            ("tap", |s, a| s.tap(a).unwrap()),
            ("edit", |s, a| s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap()),
            ("undo", |s, a| assert!(s.undo_edit(a))),
            ("redo", |s, a| assert!(s.redo_edit(a))),
            ("agent's undo", |s, a| assert!(s.undo_edit_as(a, Author::Agent).unwrap())),
            ("discard", |s, a| s.discard_edits(a)),
            ("clear", |s, a| assert!(s.clear_history(a, ClearReason::UserAsked))),
            ("save", |s, a| assert_eq!(s.saved(a, FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 }, 4, None, None), 1)),
            ("wake", |s, a| assert!(s.wake_list(a))),
        ];
        for (route, go) in routes {
            let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
            let (mut s, a, _b) = with_a_list(&files);
            let mut twin = {
                let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
                with_a_list(&files).0
            };
            assert!(s.go_dormant(a));
            go(&mut s, a);
            go(&mut twin, a);
            assert!(!s.list_is_asleep(a), "{route}");
            assert_eq!(s.edits_record(a), twin.edits_record(a), "{route}");
            assert_eq!(s.is_edited(a), twin.is_edited(a), "{route}");
            assert_eq!(s.pages_in(a), twin.pages_in(a), "{route}");
        }
        // And what it goes on to write follows on from the journal, so a crash after that replays exactly.
        let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (mut s, a, _b) = with_a_list(&files);
        assert!(s.go_dormant(a));
        s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
        let mut plain = files.lock().unwrap().clone();
        write_journal(&mut s, &mut plain);
        let parsed = record::parse_journal(plain.values().next().unwrap(), record::CURRENT_VERSION).unwrap();
        let (replayed, _) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
        assert_eq!(replayed.to_record(), s.edits_record(a).unwrap());
    }

    /// Failure: a list goes out of memory with some of it not on disk, or its journal's gone and it's lost in silence. It stays until it's all there, and if the journal can't be read the tab says so.
    #[test]
    fn a_list_only_sleeps_with_all_of_it_on_disk() {
        let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (mut s, a, _b) = with_a_list(&files);
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        assert!(s.list_to_sleep(a).is_none(), "a step hasn't gone out");
        assert!(!s.go_dormant(a));
        assert!(s.page_edits(a).is_some());
        let mut plain = files.lock().unwrap().clone();
        write_journal(&mut s, &mut plain);
        *files.lock().unwrap() = plain;
        assert!(s.go_dormant(a));
        // The journal can't be read just now. **It stays asleep, by its name, and nothing's laid on what can't be seen,** which would land on the wrong pages (review L3). The tab says so, and every look at it tries again.
        let (name, bytes) = files.lock().unwrap().drain().next().unwrap();
        assert!(!s.wake_list(a));
        assert!(s.list_is_asleep(a));
        assert_eq!(s.tab(a).unwrap().notice.as_ref().map(|notice| notice.code()), Some("edits_unreadable"));
        assert!(s.is_edited(a), "what's known of it stands");
        assert_eq!(s.edit_pages(a, |e| e.rotate(0, 1, 1)), Err(SessionError::EditsUnreadable), "a new edit isn't taken");
        assert!(!s.undo_edit(a));
        assert_eq!(s.undo_edit_as(a, Author::Owner), Err(SessionError::EditsUnreadable));
        assert_eq!(s.close(a), Err(SessionError::UnsavedChanges));
        assert!(s.to_record().tabs[0].journal.as_deref() == Some(name.as_str()), "and its name's kept, so nothing else takes it");
        // It's back.
        files.lock().unwrap().insert(name.clone(), bytes);
        assert!(s.wake_list(a));
        assert!(s.page_edits(a).is_some());
        assert_eq!(s.tab(a).unwrap().notice, None, "and it's stopped saying so");
        // Gone for good, and given up on by saying so.
        assert!(s.go_dormant(a));
        files.lock().unwrap().clear();
        assert!(!s.wake_list(a));
        s.discard_edits(a);
        assert!(!s.list_is_asleep(a) && !s.is_edited(a));
        assert_eq!(s.tab(a).unwrap().notice, None);
        assert!(s.journal_work().retired.contains(&name));
        // A session that keeps no journal keeps its lists.
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        s.open(Path::new("/d/b.pdf")).unwrap();
        assert!(!s.go_dormant(a));
        assert!(s.page_edits(a).is_some());
    }

    /// Failure (review): a sleep file's pictures match a list they're not about, as a count of edits starts again after a discard. What's compared is what's on the pages, so one order is one mark, however it was reached, and back at the file it's the mark of no list at all.
    #[test]
    fn a_list_is_known_by_whats_on_its_pages() {
        let mut one = PageEdits::new(5);
        assert_eq!(one.order_mark(), 0);
        one.rotate(0, 1, 1).unwrap();
        let rotated = one.order_mark();
        assert_ne!(rotated, 0);
        let mut other = PageEdits::new(5);
        other.delete(0, 1).unwrap();
        assert_ne!(other.order_mark(), rotated, "the same number of edits, and another document");
        assert!(one.undo());
        assert_eq!(one.order_mark(), 0, "back at the file");
        assert!(one.redo());
        assert_eq!(one.order_mark(), rotated, "the same as it was");
        // A move and a move back, by other steps.
        one.move_pages(1, 1, 2).unwrap();
        one.move_pages(2, 1, 1).unwrap();
        assert_eq!(one.order_mark(), rotated);
    }

    /// Failure (review): `session sleep` puts a tab to sleep from under whatever's working on it, as only the deadline looks at holds.
    #[test]
    fn nothing_sends_a_tab_to_sleep_from_under_a_hold() {
        let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (mut s, a, b) = with_a_list(&files);
        s.hold(a);
        assert!(!s.can_go_dormant(a));
        assert!(!s.go_dormant(a));
        assert!(s.page_edits(a).is_some() && !s.list_is_asleep(a));
        assert!(s.warm.iter().any(|(id, _)| *id == a), "its contents stay too");
        s.let_go(a);
        assert!(!s.go_dormant(b), "nor the one on screen");
        s.save_prepared(a, FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 }, None);
        assert!(!s.go_dormant(a), "nor one being saved");
    }

    /// Failure (review): a sleep file's word on a file's length goes on being believed once the worker has said otherwise, so a phantom page takes a rotation, or it's written into the next sleep file as if it had been checked. And a file read the usual way is left alone, as two readers of a damaged file can differ.
    #[test]
    fn the_workers_word_on_a_files_length_goes_over_a_sleep_files() {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-file-length-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.pdf");
        std::fs::write(&path, b"three pages").unwrap();
        let stamp = file_stamp(&path).unwrap();
        let mut s = session(3);
        let a = s.open(&path).unwrap();
        assert!(!s.file_length_is(a, 2, 5), "read the usual way, that's nothing to go on");
        s.open(Path::new("/d/b.pdf")).unwrap();
        s.warm.retain(|(id, _)| *id != a);
        // Forged: a hundred pages.
        let forged = |file_pages: u32| sleep::SleepFile { file: stamp, file_pages, pages: file_pages, list_seq: 0, renderer_only: false, version: Some(5), toc: Toc::default(), usual: None, sizes: Vec::new(), pictures: Vec::new() };
        s.wake_from_sleep(a, &forged(100));
        s.tap(a).unwrap();
        assert_eq!(s.pages_in(a), 100, "believed for now");
        assert!(s.sleep_facts(a).is_none(), "and not written on, unchecked");
        assert!(s.file_length_is(a, 3, 5), "it says to read the file");
        assert!(!s.warm.iter().any(|(id, _)| *id == a));
        assert!(s.journal_work().retired.iter().any(|name| *name == record::sleep_file_name(stamp, &path)), "and the file's gone");
        s.give_woken(a, Ok(DocFacts { page_count: 3, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false }), Some(stamp));
        s.wake_active();
        assert_eq!(s.pages_in(a), 3);
        assert_eq!(s.edit_pages(a, |e| e.rotate(50, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)), "no phantom page takes a rotation");
        // Right about it: nothing to do, and from here it's been checked.
        s.discard_edits(a);
        s.open(Path::new("/d/c.pdf")).unwrap();
        s.warm.retain(|(id, _)| *id != a);
        s.wake_from_sleep(a, &forged(3));
        s.tap(a).unwrap();
        assert!(!s.file_length_is(a, 3, 5));
        assert!(s.sleep_facts(a).is_some());
        // The right length, and another version: a rewrite that kept its size, time and number. Read after all.
        s.open(Path::new("/d/d.pdf")).unwrap();
        s.warm.retain(|(id, _)| *id != a);
        s.wake_from_sleep(a, &forged(3));
        s.tap(a).unwrap();
        assert!(s.file_length_is(a, 3, 6), "another version");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure (review, against decision 23): a changed file with no edits stops saying so as soon as it's been read, as the stamp's moved on and being the same says nothing. With edits, it's about what they were made on, so being back at that is news.
    #[test]
    fn a_changed_file_goes_on_saying_so_when_its_read() {
        let now = FileStamp { size: 2, mtime_ns: 2, device: 1, inode: 3 };
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.apply_file_status(a, FileStatus::Changed(now));
        assert_eq!(s.tab_notice(a).map(FileNotice::code), Some("file_changed"));
        s.apply_file_status(a, FileStatus::Same(now));
        assert_eq!(s.tab_notice(a).map(FileNotice::code), Some("file_changed"), "it's the new one, and that's what it's saying");
        // Left, it's been said.
        s.open(Path::new("/d/b.pdf")).unwrap();
        s.tap(a).unwrap();
        s.tap(s.tabs()[1].id).unwrap();
        assert_eq!(s.tab_notice(a), None);
        // With edits, being the same again is the news.
        s.tap(a).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        let was = s.own_stamp(a);
        s.apply_file_status(a, FileStatus::Changed(FileStamp { size: 9, ..now }));
        assert_eq!(s.tab_notice(a).map(FileNotice::code), Some("file_changed"));
        s.apply_file_status(a, FileStatus::Same(was.unwrap_or(now)));
        assert_eq!(s.tab_notice(a), None);
    }

    /// Failure: going dormant takes a timer per tab, or a thousand cold tabs make it look along them, or the tab on screen or one being saved goes, or a tab being used goes anyway. There's one deadline, the soonest of the few tabs that hold anything.
    #[test]
    fn going_dormant_has_one_deadline_and_goes_by_being_left_alone() {
        const AFTER: u32 = 300;
        let files = Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (mut s, a, b) = with_a_list(&files);
        s.touch_at(a, 100);
        assert_eq!(s.next_dormant_at(AFTER), Some(400));
        assert!(s.due_dormant(399, AFTER).is_empty());
        assert_eq!(s.due_dormant(400, AFTER), [a]);
        assert!(!s.due_dormant(10_000, AFTER).contains(&b), "the tab on screen doesn't go");
        s.touch_at(a, 200);
        assert_eq!(s.next_dormant_at(AFTER), Some(500), "being used puts it off");
        // Being worked on.
        s.hold(a);
        assert!(s.due_dormant(10_000, AFTER).is_empty());
        assert!(s.dormant_oldest_first().is_empty(), "whatever's asking");
        s.let_go(a);
        assert_eq!(s.due_dormant(10_000, AFTER), [a]);
        // A save under way.
        s.save_prepared(a, FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 }, None);
        assert!(s.due_dormant(10_000, AFTER).is_empty());
        // Cold tabs aren't candidates, however many there are.
        let mut s = session(5);
        let ids: Vec<TabId> = (0..1000).map(|at| s.open(Path::new(&format!("/d/{at}.pdf"))).unwrap()).collect();
        assert!(s.dormant_candidates().count() <= MAX_WARM + 1);
        s.go_dormant(ids[998]);
        assert!(!s.warm.iter().any(|(id, _)| *id == ids[998]), "its contents go");
        assert!(s.dormant_oldest_first().len() < MAX_WARM);
    }

    /// Failure (review R2): a copy kept for a save that was never confirmed is named only by the note of that save, so clearing the history leaves it. It holds the whole old file for up to 30 days, and a later redaction wouldn't touch it.
    #[test]
    fn clearing_takes_a_copy_kept_for_an_unconfirmed_save() {
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        // With unsaved edits, so the list stays and its history goes; and with none, so the list goes too.
        for unsaved in [true, false] {
            let mut s = journaled(5);
            let a = s.open(Path::new("/d/a.pdf")).unwrap();
            s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
            if !unsaved {
                assert!(s.undo_edit(a));
            }
            write_journal(&mut s, &mut HashMap::new());
            let keeping = kept(KEPT_A, 100);
            s.save_prepared(a, target, Some(&keeping));
            assert!(s.clear_history(a, ClearReason::UserAsked));
            assert!(s.journal_work().retired.iter().any(|name| name == KEPT_A), "unsaved: {unsaved}");
            // Once it's gone, the next journal's first line doesn't name it either.
            if let Some(list) = s.page_edits(a) {
                assert!(list.keeping().is_none());
            }
        }
    }

    /// Failure: a protected PDF's contents go in a sleep file, in the clear, or a tab pushed out of the warm ones goes cold with no sleep file, so it wakes the slow way.
    #[test]
    fn a_tab_gone_cold_has_what_a_sleep_file_needs_unless_its_protected() {
        struct Protected;
        impl DocLoader for Protected {
            fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
                Ok(DocFacts { page_count: 3, byte_len: 1, encrypted: path.ends_with("locked.pdf"), version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false })
            }
        }
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-sleep-facts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = |name: &str| {
            let path = dir.join(name);
            std::fs::write(&path, b"a file").unwrap();
            path
        };
        let mut s = Session::new(Protected);
        s.set_sleep_files(true);
        let open = s.open(&file("open.pdf")).unwrap();
        let locked = s.open(&file("locked.pdf")).unwrap();
        assert!(s.sleep_facts(open).is_some());
        assert!(s.sleep_facts(locked).is_none(), "its contents don't go on disk");
        assert!(s.sleep_to_read(locked).is_none());
        for at in 0..MAX_WARM {
            s.open(&file(&format!("{at}.pdf"))).unwrap();
        }
        let left = s.take_left_warm();
        assert_eq!(left.iter().map(|facts| facts.id).collect::<Vec<_>>(), [open], "the unprotected one that left: {left:?}");
        assert_eq!(left[0].sleep.file_pages, 3);
        assert!(s.take_left_warm().is_empty());
        // A protected file's pages in a plain document: its facts are fine, and no picture is.
        let plain = s.open(&file("plain.pdf")).unwrap();
        assert!(s.sleep_facts(plain).unwrap().pictures_allowed);
        let locked_file = file("locked.pdf");
        let facts = s.loader().load(&locked_file).unwrap();
        s.insert_pdf_with_facts(plain, Author::Owner, 0, &locked_file, &facts, file_stamp(&locked_file).ok()).unwrap();
        assert!(!s.sleep_facts(plain).unwrap().pictures_allowed, "a page from a protected file means no pictures");
        assert!(s.page_edits(plain).unwrap().added_sources()[0].protected);
        // Carried through its journal, so it's known after a restart.
        let record = s.edits_record(plain).unwrap();
        assert!(record.added[0].protected);
        assert!(PageEdits::from_record(&record).unwrap().has_protected_pages());
        assert!(s.undo_edit(plain));
        assert!(s.sleep_facts(plain).unwrap().pictures_allowed, "with those pages gone from the document, it's fine again");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a list naming a file of a length it's not about is believed, as a forged journal can say a save happened, and pages past the end are drawn or written. A file read at its own stamp and another length than the list says shows the list isn't about it.
    #[test]
    fn a_list_of_another_length_than_its_file_isnt_believed() {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-forged-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.pdf");
        std::fs::write(&file, b"five pages").unwrap();
        let mut s = journaled(5);
        let a = s.open(&file).unwrap();
        let other = s.open(&dir.join("other.pdf")).unwrap();
        // As a journal could say: a list about twelve pages, on this file.
        let mut list = PageEdits::new(12);
        list.journal_from_now();
        list.rotate(11, 1, 1).unwrap();
        s.take_up_history(a, "journal-00000000000000aa.log", list, 1);
        s.warm.clear();
        s.tap(a).unwrap();
        assert!(s.page_edits(a).is_none(), "it's gone");
        assert_eq!(s.pages_in(a), 5);
        assert_eq!(s.tab(a).unwrap().notice.as_ref().map(|notice| notice.code()), Some("edits_unreadable"));
        let _ = other;
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure (decision 26): a link to an open PDF, or a file moved and opened by its new name, makes a second tab. A copy is another file, and a PDF added as pages isn't a tab. Each case on real files, as what's the same is the file on disk.
    #[test]
    fn a_pdf_can_only_be_opened_once() {
        let dir = std::env::temp_dir().join(format!("recto-once-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (file, link, hard, copy, moved) = (dir.join("a.pdf"), dir.join("link.pdf"), dir.join("hard.pdf"), dir.join("copy.pdf"), dir.join("moved.pdf"));
        std::fs::write(&file, b"one").unwrap();
        std::os::unix::fs::symlink(&file, &link).unwrap();
        std::fs::hard_link(&file, &hard).unwrap();
        std::fs::write(&copy, b"one").unwrap();
        let mut s = session(5);
        let a = s.open(&file).unwrap();
        let other = s.open(&copy).unwrap();
        assert_ne!(a, other, "a copy is another file");
        assert_eq!(s.open(&link).unwrap(), a, "a link is the same PDF");
        assert_eq!(s.open(&hard).unwrap(), a, "so is a hard link");
        assert_eq!(s.active(), Some(a), "and it goes there");
        assert_eq!(s.path_of(a).unwrap(), file, "still there under its name, so that stays");
        assert_eq!(s.tabs().len(), 2);
        // From facts read off the session, as the app's route has them.
        s.tap(other).unwrap();
        let stamp = file_stamp(&link).ok();
        let facts = FakeLoader { pages: 5, loads: Cell::new(0) }.load(&link).unwrap();
        assert_eq!(s.open_with_facts(&link, facts, stamp, None).unwrap(), a);
        assert_eq!(s.tabs().len(), 2);
        // Moved, and every other name gone: the tab goes with it.
        std::fs::remove_file(&link).unwrap();
        std::fs::remove_file(&hard).unwrap();
        std::fs::rename(&file, &moved).unwrap();
        assert_eq!(s.open(&moved).unwrap(), a);
        assert_eq!(s.path_of(a).unwrap(), moved);
        assert_eq!(&*s.tab(a).unwrap().name, "moved");
        // As pages of another document, it's allowed.
        let stamp = file_stamp(&moved).ok();
        let facts = FakeLoader { pages: 5, loads: Cell::new(0) }.load(&moved).unwrap();
        s.insert_pdf_with_facts(other, Author::Owner, 0, &moved, &facts, stamp).unwrap();
        assert_eq!(s.pages_in(other), 10);
        assert_eq!(s.tabs().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure (final review N2): a different PDF with the inode a tab remembers took the tab over: after a move and a rewrite in place, or on another volume under the same device number. The tab's path became the other file, with edits for 6 pages on one of 3 and no notice. The number alone isn't the file: it's a different one, and gets its own tab, and the edits stay. A file changed in place at the tab's own path is still that tab. And (final review) a file replaced under a tab, with a new inode, is reached through a link all the same.
    #[test]
    fn the_same_inode_isnt_enough_to_be_the_same_pdf() {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-hijack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (y, z) = (dir.join("y.pdf"), dir.join("z.pdf"));
        std::fs::write(&y, b"six pages").unwrap();
        let mut s = session(6);
        let tab = s.open(&y).unwrap();
        s.edit_pages(tab, |e| e.delete(0, 1)).unwrap();
        // Moved, and written over in place: the same inode, and not what the edits are about.
        std::fs::rename(&y, &z).unwrap();
        std::fs::write(&z, b"three, and longer than it was").unwrap();
        let opened = s.open(&z).unwrap();
        assert_ne!(opened, tab, "it's another PDF");
        assert_eq!(s.path_of(tab).unwrap(), y, "and the tab with edits stays on what they're about");
        assert!(s.is_edited(tab));
        assert_eq!(s.pages_in(opened), 6, "nothing of the edits on it");
        assert!(!s.is_edited(opened));
        // Changed in place under its own name, by a link: that's still the tab.
        let (own, link) = (dir.join("own.pdf"), dir.join("link.pdf"));
        std::fs::write(&own, b"before").unwrap();
        std::os::unix::fs::symlink(&own, &link).unwrap();
        let mut s = session(6);
        let tab = s.open(&own).unwrap();
        std::fs::write(&own, b"after, and longer").unwrap();
        assert_eq!(s.open(&link).unwrap(), tab, "the same file, changed");
        assert_eq!(s.tabs().len(), 1);
        // Replaced under the tab with a new inode, as a save from another app does.
        let fresh = dir.join("fresh.pdf");
        std::fs::write(&fresh, b"replaced").unwrap();
        std::fs::rename(&fresh, &own).unwrap();
        assert_eq!(s.open(&link).unwrap(), tab, "the link leads to the tab's own path");
        assert_eq!(s.tabs().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure (review B9): a file claiming 50 million pages makes a list of that many at its first edit, 400 MiB. It's refused.
    #[test]
    fn a_huge_claim_of_pages_makes_no_list() {
        let mut s = session(edit::MAX_PAGES as u32 + 1);
        let id = s.open(Path::new("/d/huge.pdf")).unwrap();
        assert_eq!(s.edit_pages(id, |e| e.rotate(0, 1, 1)), Err(SessionError::Edit(EditError::TooManyPages)));
        assert!(s.page_edits(id).is_none());
    }

    /// Failure (review B3): a tab closing drops what hadn't gone to its journal, while the file stays, so an undone edit comes back live on a later launch. It all goes out, marked as closing.
    #[test]
    fn closing_keeps_what_hasnt_gone_out() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages_as(a, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        assert!(s.undo_edit(a));
        s.close(a).unwrap();
        assert!(s.has_journal_work());
        let work = s.journal_work();
        assert_eq!(work.appends.len(), 1);
        assert!(work.appends[0].closing);
        assert_eq!(work.appends[0].ops, vec![JournalOp::Undo]);
        // The disk refused: it's asked for again.
        s.journal_outcome(JournalOutcome::default());
        let again = s.journal_work();
        assert_eq!(again.appends.len(), 1, "not lost");
        s.journal_outcome(JournalOutcome { written: vec![(a, again.appends[0].through, again.appends[0].name.clone())], ..JournalOutcome::default() });
        assert!(!s.has_journal_work());
    }

    /// Failure (review B6): one journal failing makes the others go out again, as nothing says they got there. Each goes by what happened to it.
    #[test]
    fn a_journal_failing_doesnt_send_the_others_again() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        s.edit_pages(b, |e| e.rotate(0, 1, 1)).unwrap();
        let work = s.journal_work();
        let done = work.appends.iter().find(|append| append.tab == a).unwrap();
        s.journal_outcome(JournalOutcome { written: vec![(a, done.through, done.name.clone())], not_retired: Vec::new() });
        let next = s.journal_work();
        assert_eq!(next.appends.len(), 1);
        assert_eq!(next.appends[0].tab, b);
    }

    /// Failure (review T10): a checkpoint taken just as a save is under way loses that it was, and the edits get laid on the saved file after a crash.
    #[test]
    fn a_checkpoint_keeps_a_save_in_progress() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        let target = FileStamp { size: 9, mtime_ns: 1, device: 1, inode: 2 };
        let keeping = kept(KEPT_A, 100);
        s.save_prepared(a, target, Some(&keeping));
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        let (header, base) = s.checkpoint_if_caught_up(a).unwrap();
        assert_eq!(header.saving, Some(target.into()));
        assert_eq!(header.keeping, Some(keeping.to_record()), "nor what it kept");
        let parsed = record::parse_journal(&record::journal_start(&header, Some(&base)), record::CURRENT_VERSION).unwrap();
        assert_eq!(parsed.header.saving, Some(target.into()));
    }

    /// Failure (review T17): the next tab number wraps round, and a tab comes up with a number already in use.
    #[test]
    fn tab_numbers_dont_wrap() {
        let mut s = session(5);
        s.next_id = MAX_TAB_ID;
        assert_eq!(s.open(Path::new("/d/a.pdf")), Err(SessionError::TooManyTabs));
    }

    /// Failure: restoring a thousand tabs reads a file for each, or holds their contents, or a restored tab costs what an opened one doesn't. Every one of them comes back cold with no load at all.
    #[test]
    fn restoring_opens_no_file() {
        let mut s = session(5);
        let ids: Vec<TabId> = (0..1000).map(|at| s.open(Path::new(&format!("/d/{at}.pdf"))).unwrap()).collect();
        s.tap(ids[500]).unwrap();
        let record = s.to_record();
        let mut back = session(5);
        back.restore(&record, HashMap::new()).unwrap();
        assert_eq!(back.loader.loads.get(), 0, "nothing read");
        assert_eq!(back.tabs().len(), 1000);
        assert_eq!(back.active(), Some(ids[500]));
        assert!(ids.iter().filter(|id| **id != ids[500]).all(|id| back.tier(*id) == Tier::Cold));
        assert_eq!(back.to_record(), record);
        assert_eq!(back.warm_tabs().count(), 0);
    }

    /// Failure: a place for an old page is used after a `goto` moved a cold tab, so the reader lands on the wrong page. And a place that's the same doesn't count as a change, so nothing's written for it.
    #[test]
    fn a_place_is_only_used_on_its_own_page_and_only_written_when_it_moves() {
        let mut s = session(9);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.take_place_delta(1);
        let place = PlaceRecord { page: 0, anchor_page: 0, within: 0.5, x: 0.0, y: 10.0, document_height: 900.0, zoom: 1.0, page_width: 0.0, page_height: 0.0 };
        s.note_place(a, place);
        assert_eq!(s.take_place_delta(2).places.len(), 1);
        s.note_place(a, place);
        assert!(s.take_place_delta(3).places.is_empty(), "it didn't move");
        assert_eq!(s.place_to_show(a), Some(place));
        s.go_to(a, 4).unwrap();
        assert_eq!(s.place_to_show(a), None, "it's for page 1, and the reader's on page 5");
        assert_eq!(s.take_place_delta(4).places.len(), 1);
    }

    fn at(path: &str) -> FileStatus {
        FileStatus::Moved(PathBuf::from(path), FileStamp { size: 1, mtime_ns: 1, device: 1, inode: 1 })
    }

    /// Failure: what became of a file isn't said, or its edits go, or the tab's still pointing at where the file isn't. Each case, with the words a reader sees and the edits kept. A moved file with edits keeps its journal's name, as that's by its identity.
    #[test]
    fn a_tab_says_what_became_of_its_file_and_keeps_its_edits() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        let now = FileStamp { size: 9, mtime_ns: 9, device: 9, inode: 9 };
        s.apply_file_status(a, at("/e/moved.pdf"));
        assert_eq!(s.path_of(a).unwrap(), Path::new("/e/moved.pdf"));
        assert_eq!(&*s.tab(a).unwrap().name, "moved");
        assert!(s.tab_notice_message(a).unwrap().contains("moved"));
        assert!(s.is_edited(a));
        s.apply_file_status(a, FileStatus::Changed(now));
        assert_eq!(s.tab_notice(a).unwrap().code(), "file_changed");
        assert_ne!(s.own_stamp(a), Some(now), "edits keep the version they were made on");
        s.apply_file_status(a, FileStatus::Missing);
        assert!(s.tab_notice_message(a).unwrap().contains("edits"));
        assert!(s.is_edited(a));
        assert_eq!(s.sidebar().rows[0].notice, s.tab_notice_message(a));
        s.apply_file_status(a, FileStatus::Same(now));
        assert_eq!(s.tab_notice(a), None, "it came back");
        assert_eq!(s.own_stamp(a), Some(now));
        s.apply_file_status(a, FileStatus::NotLooked);
        assert_eq!(s.own_stamp(a), Some(now), "that says nothing");
        // Clean: changed takes the new version, and it's said once, on the way past.
        let mut clean = session(5);
        let b = clean.open(Path::new("/d/b.pdf")).unwrap();
        clean.open(Path::new("/d/c.pdf")).unwrap();
        clean.apply_file_status(b, FileStatus::Changed(now));
        assert_eq!(clean.own_stamp(b), Some(now));
        clean.tap(b).unwrap();
        assert!(clean.tab_notice(b).is_some());
        let c = clean.tabs()[1].id;
        clean.tap(c).unwrap();
        assert_eq!(clean.tab_notice(b), None, "left, so it's been said");
        // Moved onto a file another tab has: this one's the one that's gone.
        clean.apply_file_status(b, FileStatus::Moved(PathBuf::from("/d/c.pdf"), now));
        assert_eq!(clean.tab_notice(b).unwrap().code(), "file_missing");
        assert_eq!(clean.path_of(b).unwrap(), Path::new("/d/b.pdf"));
    }

    /// Failure: a journal that can't be read costs the tab silently. The tab comes back and says so, with no edits it can't vouch for.
    #[test]
    fn a_tab_whose_journal_cant_be_read_says_so() {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        let mut back = journaled(5);
        back.restore(&s.to_record(), HashMap::new()).unwrap();
        assert_eq!(back.tab_notice(a).unwrap().code(), "edits_unreadable");
        assert!(!back.is_edited(a));
        let mut torn = back_from(&files);
        torn.values_mut().next().unwrap().lost_lines = true;
        let mut partly = journaled(5);
        partly.restore(&s.to_record(), torn).unwrap();
        assert_eq!(partly.tab_notice(a).unwrap().code(), "edits_unreadable");
        assert!(partly.is_edited(a), "what could be read is here");
    }

    /// Failure: a protected PDF just says it needs a password, with nothing to say why it's asking again, or a password is written down. The words say it isn't kept, and nothing in a record's fields could hold one.
    #[test]
    fn a_protected_pdf_asks_again() {
        struct Protected;
        impl DocLoader for Protected {
            fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
                Err(OpenError::PasswordNeeded)
            }
        }
        let mut record = session(5);
        let a = record.open(Path::new("/d/a.pdf")).unwrap();
        let saved = record.to_record();
        let text = serde_json::to_string(&saved).unwrap();
        assert!(!text.contains("password"), "{text}");
        let mut back = Session::new(Protected);
        back.restore(&saved, HashMap::new()).unwrap();
        back.wake_active();
        assert_eq!(back.tab(a).unwrap().problem, Some(OpenError::PasswordNeeded));
        assert!(back.tab_notice_message(a).unwrap().contains("password"));
    }

    /// Failure: restoring turns a hostile record into something that panics later: a page past the end, or an active tab that isn't there. Or a record's tabs with no journal cost it a tab.
    #[test]
    fn a_restored_page_is_held_to_the_document() {
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let mut saved = s.to_record();
        saved.tabs[0].page = u32::MAX;
        let mut back = session(5);
        back.restore(&saved, HashMap::new()).unwrap();
        assert_eq!(back.tab(a).unwrap().page, 4);
        saved.active = Some(999);
        assert!(back.restore(&saved, HashMap::new()).is_err());
    }

    /// Failure: ⌘-click doesn't build a comparison the way Finder's does: a second tab doesn't join, one in it doesn't leave, the last one left isn't alone, a fourth gets in, or a plain click doesn't return to one. And the pane being read isn't the one clicked.
    #[test]
    fn command_click_builds_a_comparison_and_a_plain_click_ends_it() {
        let mut s = session(10);
        let ids: Vec<TabId> = ["a", "b", "c", "d"].iter().map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap()).collect();
        s.tap(ids[0]).unwrap();
        assert_eq!(s.toggle_compare(ids[0]), Ok(compare::CompareChange::Nothing), "the only tab showing");
        assert_eq!(s.panes(), vec![ids[0]]);
        assert_eq!(s.toggle_compare(ids[1]), Ok(compare::CompareChange::Added));
        assert_eq!(s.panes(), vec![ids[0], ids[1]]);
        assert_eq!(s.active(), Some(ids[1]), "the one clicked is read");
        assert_eq!(s.toggle_compare(ids[2]), Ok(compare::CompareChange::Added));
        assert_eq!(s.toggle_compare(ids[3]), Err(compare::CompareError::TooMany));
        assert_eq!(s.panes().len(), compare::MAX_PANES);
        assert!(s.focus_pane(ids[0]));
        assert_eq!(s.active(), Some(ids[0]));
        assert!(!s.focus_pane(ids[3]), "not a pane");
        assert_eq!(s.toggle_compare(ids[0]), Ok(compare::CompareChange::Removed));
        assert_eq!(s.panes(), vec![ids[1], ids[2]]);
        assert_eq!(s.active(), Some(ids[1]), "reading moves to a pane that's left");
        assert_eq!(s.toggle_compare(ids[2]), Ok(compare::CompareChange::Ended));
        assert_eq!(s.panes(), vec![ids[1]]);
        assert!(s.comparison().is_none());
        s.compare(&[ids[0], ids[3]], None).unwrap();
        s.tap(ids[2]).unwrap();
        assert!(s.comparison().is_none(), "a plain click returns to one");
        assert_eq!(s.panes(), vec![ids[2]]);
        assert!(s.tab(ids[2]).unwrap().toc_open, "and doesn't fold its contents");
        s.compare(&[ids[0], ids[1]], None).unwrap();
        s.open(&PathBuf::from("/d/e.pdf")).unwrap();
        assert!(s.comparison().is_none(), "opening another PDF shows it alone");
        assert_eq!(s.panes().len(), 1);
    }

    /// Failure: a tab on screen in a comparison pane goes dormant, by time, budget or pressure, and its pane goes blank or its edits leave memory under it. Panes are on screen, and only what's off screen goes.
    #[test]
    fn panes_on_screen_dont_go_dormant() {
        let mut s = session(10);
        let ids: Vec<TabId> = ["a", "b", "c"].iter().map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap()).collect();
        s.compare(&[ids[0], ids[1]], Some(ids[0])).unwrap();
        assert!(!s.can_go_dormant(ids[1]), "a pane not being read is still on screen");
        assert!(!s.dormant_oldest_first().contains(&ids[1]));
        assert!(s.dormant_oldest_first().contains(&ids[2]), "the tab off screen can");
        assert!(!s.go_dormant(ids[1]));
    }

    /// Failure: closing a pane's tab leaves a pane for a tab that's gone, or a comparison of one, or the reader on a tab that's off screen rather than a pane that's left.
    #[test]
    fn closing_a_panes_tab_takes_its_pane() {
        let mut s = session(10);
        let ids: Vec<TabId> = ["a", "b", "c", "d"].iter().map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap()).collect();
        s.compare(&[ids[1], ids[2], ids[3]], Some(ids[2])).unwrap();
        s.close(ids[2]).unwrap();
        assert_eq!(s.panes(), vec![ids[1], ids[3]]);
        assert_eq!(s.active(), Some(ids[1]), "to a pane, not the next tab along");
        s.close(ids[1]).unwrap();
        assert!(s.comparison().is_none(), "one left is alone");
        assert_eq!(s.panes(), vec![ids[3]]);
        s.compare(&[ids[0], ids[3]], Some(ids[0])).unwrap();
        s.close(ids[3]).unwrap();
        assert_eq!(s.panes(), vec![ids[0]]);
    }

    /// Failure: a comparison doesn't come back after a restart exactly: its tabs, their order, synced scroll and each pane's start; or a record naming a tab that isn't there, or of one, or of four, gets in.
    #[test]
    fn a_comparison_comes_back_from_the_record_exactly() {
        let mut s = session(40);
        let ids: Vec<TabId> = ["a", "b", "c"].iter().map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap()).collect();
        s.go_to(ids[2], 7).unwrap();
        s.go_to(ids[0], 3).unwrap();
        s.compare(&[ids[2], ids[0]], Some(ids[0])).unwrap();
        assert!(s.set_compare_synced(true));
        let bytes = record::encode_record(&s.to_record(), 3);
        let read = record::decode_record(&bytes, record::CURRENT_VERSION).unwrap();
        let mut back = session(40);
        back.restore(&read.record, HashMap::new()).unwrap();
        assert_eq!(back.comparison(), s.comparison());
        assert_eq!(back.comparison().unwrap().sync_from, vec![7, 3]);
        assert_eq!(back.active(), Some(ids[0]));
        assert_eq!(back.to_record(), s.to_record());
        for tabs in [vec![ids[0].0], vec![ids[0].0, 99], vec![ids[0].0, ids[0].0], vec![1, 2, 3, 3]] {
            let mut bad = s.to_record();
            bad.comparison = Some(record::ComparisonRecord { tabs, synced: false, sync_from: Vec::new() });
            assert!(record::validate_record(&bad).is_err(), "{:?}", bad.comparison);
        }
        let mut uneven = s.to_record();
        uneven.comparison.as_mut().unwrap().sync_from = vec![1];
        assert!(record::validate_record(&uneven).is_err(), "a start for each pane, or none");
        // An older record has none, and reads.
        let mut older = s.to_record();
        older.comparison = None;
        let json = serde_json::to_string(&older).unwrap();
        assert!(!json.contains("comparison"), "nothing written for an older app to trip on");
    }

    /// Failure (review 1): a comparison with cold tabs leaves more live documents than SPEC §2 allows, as only the pane being read was made warm and the tabs warm before stayed; or making them warm reads their files with the session held. Each pane is one of the warm tabs, the rest go cold, and what's read ahead is all that's read.
    #[test]
    fn a_comparison_of_cold_tabs_is_the_live_documents_and_reads_nothing_held() {
        let mut s = session(10);
        let ids: Vec<TabId> = ["a", "b", "c", "d", "e"].iter().map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap()).collect();
        assert_eq!(s.active(), Some(ids[4]));
        let panes = [ids[0], ids[1], ids[2]];
        let cold = s.wake_for_compare(&panes);
        assert_eq!(cold.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![ids[0], ids[1]], "the warm one needs no read");
        for (id, _) in cold {
            let facts = s.loader().load(Path::new("/d/x.pdf"));
            s.give_woken(id, facts, None);
        }
        let loads = s.loader().loads.get();
        s.compare(&panes, None).unwrap();
        assert_eq!(s.loader().loads.get(), loads, "nothing read with the session held");
        let mut warm: Vec<TabId> = s.warm_tabs().collect();
        warm.sort();
        assert_eq!(warm, panes.to_vec(), "the panes are the live documents, and only they");
        assert!(warm.len() <= MAX_WARM);
    }

    /// Failure (review 1): the pane read next, when the one being read leaves by closing or a ⌘-click, is read from disk with the session held, as nobody read it ahead. Both say which file to read first.
    #[test]
    fn the_pane_read_next_is_read_ahead() {
        let mut s = session(10);
        let ids: Vec<TabId> = ["a", "b", "c"].iter().map(|name| s.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap()).collect();
        s.compare(&[ids[0], ids[1], ids[2]], Some(ids[1])).unwrap();
        // Come back from a record, every pane cold.
        let record = s.to_record();
        let mut back = session(10);
        back.restore(&record, HashMap::new()).unwrap();
        assert_eq!(back.active(), Some(ids[1]));
        back.wake_active();
        assert_eq!(back.wake_for_close(ids[1]).map(|(id, _)| id), Some(ids[0]), "closing the read pane wakes the first left, not the next tab along");
        assert_eq!(back.wake_for_compare_click(ids[1]).map(|(id, _)| id), Some(ids[0]), "and so does ⌘-clicking it out");
        assert_eq!(back.wake_for_compare_click(ids[2]), None, "a pane that isn't read leaving wakes nothing");
        let loads = back.loader().loads.get();
        let (next, _) = back.wake_for_compare_click(ids[1]).unwrap();
        let facts = back.loader().load(Path::new("/d/a.pdf"));
        back.give_woken(next, facts, None);
        let read_ahead = back.loader().loads.get();
        back.toggle_compare(ids[1]).unwrap();
        assert_eq!(back.loader().loads.get(), read_ahead, "nothing read with the session held");
        assert!(read_ahead > loads);
        assert_eq!(back.active(), Some(ids[0]));
    }
}

#[cfg(test)]
mod trail_tests {
    use super::*;
    use recto_core::nav::{LabelRange, LabelStyle, PageLabels};

    /// Twelve pages: `i` to `iv`, then `1` to `8`.
    struct Labelled;

    impl DocLoader for Labelled {
        fn load(&self, _: &Path) -> Result<DocFacts, OpenError> {
            let mut toc = Toc::default();
            toc.labels = PageLabels::from_ranges(vec![LabelRange { start: 0, first: 1, style: LabelStyle::RomanLower, prefix: "".into() }, LabelRange { start: 4, first: 1, style: LabelStyle::Decimal, prefix: "".into() }], false);
            Ok(DocFacts { page_count: 12, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc, renderer_only: false })
        }
    }

    /// Failure: Back after following a link doesn't return to where the reader was, to the depth on the page; or the trail isn't in the record, so it's gone after a quit or an update; or a closed tab's trail stays in memory.
    #[test]
    fn back_and_forward_come_back_after_a_restart() {
        let mut s = Session::new(Labelled);
        let id = s.open(Path::new("/d/book.pdf")).unwrap();
        s.go_to(id, 2).unwrap();
        s.note_place(id, PlaceRecord { page: 2, anchor_page: 2, within: 0.4, x: 0.0, y: 0.0, document_height: 1.0, zoom: 1.0, page_width: 0.0, page_height: 0.0 });
        s.jump(id, 9, 0.25).unwrap();
        assert_eq!(s.tab(id).unwrap().page, 9);
        assert_eq!(s.take_jump(id), Some(trail::Stop::new(9, 0.25)), "the page view's told where to land");
        assert_eq!(s.take_jump(id), None, "once");
        assert_eq!(s.can_go_back_and_forward(id), (true, false));
        let record = s.to_record();
        let mut again = Session::new(Labelled);
        again.restore(&record, HashMap::new()).unwrap();
        assert_eq!(again.trail_of(id), s.trail_of(id), "the trail comes back as it was");
        assert!(again.go_back(id).unwrap());
        assert_eq!(again.take_jump(id), Some(trail::Stop::new(2, 0.4)), "back to the page and the depth it was left at");
        assert_eq!(again.tab(id).unwrap().page, 2);
        assert_eq!(again.can_go_back_and_forward(id), (false, true));
        assert!(again.go_forward(id).unwrap());
        assert_eq!(again.tab(id).unwrap().page, 9);
        assert!(!again.go_forward(id).unwrap(), "nowhere further");
        again.close(id).unwrap();
        assert!(again.jumps.is_empty());
    }

    /// Failure: the corner shows `4 of 12` for a page the book calls `iv`, or `5 (5 of 12)` where the label says nothing new; or typing `iv` or `3` in the go-to field goes to the wrong page.
    #[test]
    fn labels_show_in_the_corner_and_find_pages() {
        let mut s = Session::new(Labelled);
        let id = s.open(Path::new("/d/book.pdf")).unwrap();
        assert_eq!(s.place_words(id, 3), "iv (4 of 12)");
        assert_eq!(s.place_words(id, 4), "1 (5 of 12)");
        assert_eq!(s.page_for_typed(id, "iv"), Some(3));
        assert_eq!(s.page_for_typed(id, "3"), Some(6), "the page labelled 3, as Preview reads it");
        assert_eq!(s.page_for_typed(id, "12"), Some(11), "a number no page is labelled is the page");
        assert_eq!(s.page_for_typed(id, "13"), None);
        assert_eq!(s.page_for_typed(id, "xyz"), None);
        // A moved page keeps the label its file gives it.
        s.edit_pages(id, |edits| edits.move_pages(3, 1, 0)).unwrap();
        assert_eq!(s.page_label(id, 0).as_deref(), Some("iv"));
        assert_eq!(s.page_for_typed(id, "iv"), Some(0));
        s.go_to(id, 0).unwrap();
        assert_eq!(s.sidebar().place, "iv (1 of 12)");
    }

    /// Failure: a tab's trail makes a cold tab cost more than its budget of 4 KiB, as nothing counts it.
    #[test]
    fn a_cold_tab_with_a_full_trail_stays_in_budget() {
        let mut s = Session::new(Labelled);
        let id = s.open(Path::new("/d/book.pdf")).unwrap();
        for round in 0..(trail::MAX_TRAIL as u32 * 3) {
            s.jump(id, round % 12, 0.0).unwrap();
        }
        s.go_back(id).unwrap();
        for _ in 0..trail::MAX_TRAIL {
            s.go_back(id).unwrap();
        }
        let tab = s.tab(id).unwrap();
        let cost = std::mem::size_of::<TabRecord>() + tab.name.len() + tab.file_name.len() + tab.trail.as_ref().map_or(0, |trail| std::mem::size_of::<trail::Trail>() + trail.heap_bytes());
        assert!(cost <= 4096, "{cost} bytes");
        assert!(tab.trail.as_ref().is_some_and(|trail| !trail.back().is_empty() || !trail.forward().is_empty()));
    }

    /// Failure: after a page is deleted or moved, Back goes to the wrong page, or to where a deleted page used to be.
    #[test]
    fn the_trail_follows_page_edits() {
        let mut s = Session::new(Labelled);
        let id = s.open(Path::new("/d/book.pdf")).unwrap();
        s.go_to(id, 2).unwrap();
        s.jump(id, 8, 0.0).unwrap();
        s.jump(id, 5, 0.0).unwrap();
        // Page 3 (position 2) moves to the front; page 9 (position 8) is deleted.
        s.edit_pages(id, |edits| edits.move_pages(2, 1, 0)).unwrap();
        s.edit_pages(id, |edits| edits.delete(8, 1)).unwrap();
        let back: Vec<u32> = s.trail_of(id).unwrap().back().iter().map(|stop| stop.page).collect();
        assert_eq!(back, [0], "the moved page's stop followed it, and the deleted page's went");
    }

    /// Failure: an agent's `goto` pushes the person's own places out of the trail, so after an agent has been busy Back no longer reaches where they were.
    #[test]
    fn an_agents_jumps_give_way_to_the_persons() {
        let mut s = Session::new(Labelled);
        let id = s.open(Path::new("/d/book.pdf")).unwrap();
        for round in 0..10 {
            s.jump(id, (round % 2) * 5 + 1, 0.0).unwrap();
        }
        let persons = s.trail_of(id).unwrap().back().len();
        for round in 0..200u32 {
            s.jump_as(id, round % 11, 0.0, Author::Agent).unwrap();
        }
        let trail = s.trail_of(id).unwrap();
        assert_eq!(trail.back().iter().filter(|stop| !stop.by_agent).count(), persons);
        assert!(trail.back().len() <= trail::MAX_TRAIL);
        let record = s.to_record();
        assert!(record.tabs[0].back.iter().any(|stop| stop.by_agent), "who left each is kept");
    }
}
