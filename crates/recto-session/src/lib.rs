//! Tabs, tiers and the sidebar: every rule the host needs, with no UI toolkit in sight.
//!
//! **Cost follows activity.** A cold tab is a [`TabRecord`] and nothing else: no handle, no contents, no pages. Opening
//! reads a PDF's facts and closes it. Only the active tab and the most recent others keep their contents, up to
//! [`MAX_WARM`], and anything evicted is rebuilt from disk when it's needed again.

pub mod change;
pub mod compare;
pub mod edit;
pub mod palette;
pub mod record;
pub mod sleep;
pub mod trail;

mod editing;
mod pages;
mod saving;
mod tabs;
mod tiers;

pub use change::Closed;
pub use edit::Author;
use edit::{EditError, PageEdits};
use record::{
    EditsSummary, FileNotice, FileStatus, FindRecord, JournalHeader, JournalOp, MAX_TAB_ID, PaperName, PathField, PlaceDelta, PlaceRecord, RecordError,
    Selection, SessionRecord, TabEntry, ViewRecord,
};
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
/// Files kept in File > Open Recent and offered to ⌘K: twice what a Mac app lists by default, and a few kilobytes at most.
pub const MAX_RECENT: usize = 20;

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
    /// Its file was opened with a password the person typed, this session or one before (it's in the record). Unlike `encrypted`, which a file anyone may open has too, this means what's known of it was decrypted for the person, so nothing of it goes to an agent: see [`Session::withheld_from_agents`].
    password: bool,
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
    /// Quarter turns of the view: every page drawn turned, on top of what its edits turn. Set by QA's `view_rotation` and kept in the record.
    pub view_rotation: u8,
    /// How its pages are laid out: a page to a row, or two side by side.
    pub spread: record::Spread,
    /// What's selected, as the page view last said: kept in the record, so it comes back across a restart.
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
    /// The place, such as "32 of 96", which the page number in the lower right shows. Empty with no PDF open.
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
    /// The same, with the system's Increase Contrast on: about twice as strong, so the thumb stands out against the sidebar and the page as that setting asks of every control, and still clear of the text under it.
    pub light_contrast: (u32, f64),
    pub light_contrast_hover: (u32, f64),
    pub dark_contrast: (u32, f64),
    pub dark_contrast_hover: (u32, f64),
}

/// The one scroll thumb.
pub const SCROLL_THUMB: ScrollThumb = ScrollThumb {
    width: 4.0,
    hover_width: 6.0,
    hit_width: 12.0,
    inset: 3.0,
    min_length: 24.0,
    light: (0x000000, 0.22),
    light_hover: (0x000000, 0.42),
    dark: (0xFFFFFF, 0.24),
    dark_hover: (0xFFFFFF, 0.45),
    light_contrast: (0x000000, 0.55),
    light_contrast_hover: (0x000000, 0.75),
    dark_contrast: (0xFFFFFF, 0.6),
    dark_contrast_hover: (0xFFFFFF, 0.8),
};

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
    SidebarHeights {
        contents,
        note,
        tabs_scroll: f64::from(tabs) * TAB_ROW_HEIGHT + note + contents > room,
    }
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
        Self {
            paused: false,
            pointer_inside: false,
            last_user_scroll: f64::NEG_INFINITY,
            current: None,
            placed: false,
            moving: None,
            resume: None,
        }
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
        *self = Self {
            pointer_inside,
            ..Self::default()
        };
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
        self.moving = Some(Motion {
            target,
            aim: view.scroll_y,
            at: Some(view.scroll_y),
            velocity: 0.0,
            rate: follow_rate(target - view.scroll_y, view.height),
        });
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
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
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
    /// Edit mode: the panel opens on its tools, and Rotate turns the page being read rather than every page.
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
    /// Files opened lately, newest first, at most [`MAX_RECENT`]. In the state record.
    recent: VecDeque<PathBuf>,
    /// Moves whenever `recent` does, so the app rebuilds what it shows of it only then.
    recent_version: u64,
    /// Jumps the page view hasn't taken yet, with how far down the page to land: one per tab at most, the latest.
    jumps: Vec<(TabId, trail::Stop)>,
}

/// A file opened lately, for File > Open Recent and ⌘K.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentFile {
    pub path: PathBuf,
    /// Its name as a tab shows it: the file's name without `.pdf`.
    pub name: String,
    /// The tab it's open in now, if it is.
    pub open: Option<TabId>,
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
        [Self::Generate, Self::ShowGenerated, Self::ShowOwn, Self::Stop]
            .into_iter()
            .find(|action| action.id() == id)
    }
}

/// The note over a tab's contents: a short line at rest, and what it means on hover. `None` for contents that are fine and shown as they are. Owner, 2026-11-06: "the left sidebar menu is a bit too complicated (visually noisy)", so it's one quiet line, and the rest waits for the pointer.
pub fn contents_note(
    verdict: ContentsVerdict,
    view: Option<&GeneratedView>,
    choose_generated: bool,
    own_entries: usize,
) -> Option<(&'static str, &'static str, Option<ContentsAction>)> {
    const MADE: &str =
        "Made by RectoPDF from the headings distillPDF found on the pages. They aren't part of the file, which isn't changed, and they can be wrong.";
    match view {
        Some(GeneratedView::Ready(_)) if choose_generated => Some((
            "Generated contents",
            if own_entries > 0 {
                "Made by RectoPDF from the headings distillPDF found on the pages. They aren't part of the file, which isn't changed, and they can be wrong. Click to show the file's own."
            } else {
                MADE
            },
            (own_entries > 0).then_some(ContentsAction::ShowOwn),
        )),
        Some(GeneratedView::Ready(_)) => Some(("Show generated contents", MADE, Some(ContentsAction::ShowGenerated))),
        Some(GeneratedView::Making) => Some((
            "Generating contents\u{2026}",
            "Finding the headings on the pages, in the background. On a long document this can take a little while. Click to stop.",
            Some(ContentsAction::Stop),
        )),
        Some(GeneratedView::Failed(words)) => Some(("Couldn't generate contents", words, Some(ContentsAction::Generate))),
        Some(GeneratedView::NoneFound) => Some((
            "No headings found",
            "RectoPDF looked for headings on the pages and found none to list. A scan with no text has none to find.",
            None,
        )),
        _ if verdict.offers() || choose_generated => Some((
            "Generate contents",
            match verdict {
                ContentsVerdict::Missing => {
                    "This file has no contents of its own. RectoPDF can make some from the headings on its pages. The file isn't changed."
                }
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
    pub fn view(&self) -> ViewSettings {
        self.view
    }

    pub fn set_view(&mut self, view: ViewSettings) {
        self.view = view;
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
        self.warm
            .iter()
            .map(|(id, _)| *id)
            .chain(self.edits.keys().copied())
            .filter(move |id| !self.on_screen(*id) && !self.held.contains(id) && self.edits.get(id).is_none_or(|list| list.saving().is_none()))
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
        self.dormant_candidates()
            .filter_map(|id| self.tab(id))
            .map(|tab| tab.touched.saturating_add(after))
            .min()
    }

    /// The tabs due to go dormant now.
    pub fn due_dormant(&self, now: u32, after: u32) -> Vec<TabId> {
        let mut due: Vec<TabId> = self
            .dormant_candidates()
            .filter(|id| self.tab(*id).is_some_and(|tab| tab.touched.saturating_add(after) <= now))
            .collect();
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

    /// What's asleep, by journal, for reading back: each has to read as what's known of it here.
    pub fn asleep_by_journal(&self) -> HashMap<String, EditsSummary> {
        self.asleep
            .iter()
            .filter_map(|(id, summary)| Some((self.journals.get(id)?.to_string(), *summary)))
            .collect()
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
        if tab.encrypted
            || tab.from_sleep.is_some()
            || self.asleep.contains_key(&id)
            || matches!(
                tab.notice.as_deref(),
                Some(FileNotice::Changed | FileNotice::Missing | FileNotice::NeedsPassword)
            )
        {
            return None;
        }
        let list = self.edits.get(&id);
        let (pages, list_seq) = list.map_or((tab.page_count, 0), |list| (list.len(), list.order_mark()));
        let sleep = sleep::SleepFile {
            file: stamp,
            file_pages: tab.page_count,
            pages,
            list_seq,
            renderer_only: tab.renderer_only,
            version: None,
            toc: toc.clone(),
            usual: None,
            sizes: Vec::new(),
            pictures: Vec::new(),
        };
        // A page from a protected file on any page means pictures of none, and it's the same for each of them.
        Some(SleepFacts {
            id,
            path: self.path_of(id)?,
            sleep,
            pictures_allowed: !list.is_some_and(PageEdits::has_protected_pages),
        })
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
        let Some(sleep) = self.history.as_ref().and_then(|source| source.read_sleep(stamp, &path)) else {
            return false;
        };
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
        let facts = DocFacts {
            page_count: sleep.file_pages,
            byte_len: sleep.file.size,
            encrypted: false,
            version: "".into(),
            title: None,
            toc: sleep.toc.clone(),
            renderer_only: sleep.renderer_only,
        };
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
        let ids: Vec<TabId> = self
            .edits
            .iter()
            .filter(|(id, edits)| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()) || self.fresh_journals.iter().any(|(tab, _, _)| tab == *id))
            .map(|(id, _)| *id)
            .collect();
        let mut appends = self.departing.clone();
        for id in ids {
            if let Some(append) = self.append_for(id) {
                appends.push(append);
            }
        }
        JournalWork {
            retired: std::mem::take(&mut self.retired_journals).into_iter().map(String::from).collect(),
            appends,
        }
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
            return Some(JournalAppend {
                tab: id,
                name,
                header,
                through,
                ops: Vec::new(),
                closing: false,
                base: Some(edits.to_record()),
                replaces: replaces.clone(),
            });
        }
        Some(JournalAppend {
            tab: id,
            name,
            header,
            through,
            ops: ops.to_vec(),
            closing: false,
            base: None,
            replaces: None,
        })
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
            let (Some(tab), Some(path), Some(list)) = (self.tab(id), self.path_of(id), self.edits_record(id)) else {
                continue;
            };
            out.push((name, JournalHeader::new(id.0, &path, tab.stamp, tab.page_count, 0), list));
        }
        out
    }

    /// Every list in full by its journal's name, for reading back.
    pub fn lists_by_journal(&self) -> HashMap<String, record::EditsRecord> {
        self.journals
            .iter()
            .filter_map(|(id, name)| Some((name.to_string(), self.edits.get(id)?.to_record())))
            .collect()
    }

    /// Whether there's anything for [`Self::journal_work`], which is asked often and costs a look per edited tab.
    pub fn has_journal_work(&self) -> bool {
        !self.retired_journals.is_empty()
            || !self.departing.is_empty()
            || !self.fresh_journals.is_empty()
            || self.edits.values().any(|edits| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()))
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

    /// The name of this tab's journal, if it has one, for checking what's reached the disk.
    pub fn journal_of(&self, id: TabId) -> Option<&str> {
        self.journals.get(&id).map(|name| &**name)
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
        let (current, current_tab) = if query == self.find.query {
            (self.find.current, self.find.current_tab)
        } else {
            (None, None)
        };
        self.find = FindRecord {
            open,
            query: cut(query),
            draft: cut(draft),
            current,
            current_tab,
        };
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

    /// Turns the view of a tab, by quarter turns.
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
        let Self {
            loader: _,
            view,
            tabs,
            active,
            next_id,
            warm: _,
            dirs: _,
            dir_ids: _,
            edits,
            wake_failed: _,
            asleep,
            started: _,
            held: _,
            sleep_files: _,
            just_read: _,
            woken_from_file: _,
            left_warm: _,
            draws: _,
            woken: _,
            place_lost: _,
            find,
            edit_mode,
            journaling: _,
            journals,
            fresh_journals: _,
            retired_journals: _,
            departing: _,
            history: _,
            agent_at_full: _,
            generated: _,
            comparison,
            saving: _,
            saved_over: _,
            jumps: _,
            recent,
            recent_version: _,
        } = self;
        let ViewSettings {
            paper,
            sidebar_hidden,
            agent_icon_hidden,
        } = *view;
        let tabs = tabs
            .iter()
            .map(|tab| {
                let TabRecord {
                    from_sleep: _,
                    encrypted: _,
                    password,
                    touched: _,
                    id,
                    dir: _,
                    file_name: _,
                    name: _,
                    page,
                    page_count,
                    toc_open,
                    problem: _,
                    renderer_only,
                    stamp,
                    place,
                    view_rotation,
                    spread,
                    selection,
                    notice,
                    place_dirty: _,
                    contents_verdict: _,
                    generated_contents,
                    folds,
                    trail,
                } = tab;
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
                    password: *password,
                    back: trail.as_ref().map(|trail| trail.back().to_vec()).unwrap_or_default(),
                    forward: trail.as_ref().map(|trail| trail.forward().to_vec()).unwrap_or_default(),
                }
            })
            .collect();
        let comparison = comparison.as_ref().map(|comparison| record::ComparisonRecord {
            tabs: comparison.tabs.iter().map(|tab| tab.0).collect(),
            synced: comparison.synced,
            sync_from: comparison.sync_from.clone(),
        });
        let recent = recent.iter().map(|path| PathField::from_path(path)).collect();
        SessionRecord {
            app: record::AppRecord::default(),
            view: ViewRecord {
                paper: paper.map(Paper::to_record),
                focus: false,
                sidebar_hidden,
                agent_icon_hidden,
            },
            find: find.clone(),
            edit_mode: *edit_mode,
            active: active.map(|id| id.0),
            next_id: *next_id,
            tabs,
            comparison,
            recent,
        }
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
        PlaceDelta {
            seq,
            active: self.active.map(|id| id.0),
            places,
            contents_scroll: None,
            focus: None,
        }
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
        self.recent = record
            .recent
            .iter()
            .filter(|path| path.fits())
            .filter_map(PathField::to_path)
            .filter(|path| path.is_absolute())
            .take(MAX_RECENT)
            .collect();
        self.recent_version += 1;
        self.edit_mode = record.edit_mode;
        let sidebar_hidden = record.view.sidebar_comes_back_hidden();
        let ViewRecord {
            paper,
            focus: _,
            sidebar_hidden: _,
            agent_icon_hidden,
        } = record.view;
        self.view = ViewSettings {
            paper: paper.map(Paper::from_record),
            sidebar_hidden,
            agent_icon_hidden,
        };
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
                password: entry.password,
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
                comparison.sync_from = saved
                    .tabs
                    .iter()
                    .zip(&saved.sync_from)
                    .filter(|(id, _)| comparison.contains(TabId(**id)))
                    .map(|(_, page)| *page)
                    .collect();
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

    /// Whether the last edit removed the page the reader was on, so its place moved to the nearest one. Asking clears it.
    pub fn take_place_lost(&mut self) -> bool {
        std::mem::take(&mut self.place_lost)
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

    /// Builds the sidebar. Cost depends on the visible rows and the active tab's contents, never on idle tabs' PDFs.
    pub fn sidebar(&self) -> SidebarModel<'_> {
        let mut rows = Vec::with_capacity(self.tabs.len());
        let mut rail_filled = 0;
        let mut place = String::new();
        let mut toc_open = false;
        for tab in &self.tabs {
            let is_active = self.active == Some(tab.id);
            rows.push(SidebarRow {
                kind: RowKind::Tab,
                tab: tab.id,
                text: &tab.name,
                page: None,
                depth: 0,
                current: is_active,
                notice: tab_notice_words(tab, self.has_list(tab.id)),
                action: None,
                entry: NONE,
                expandable: false,
                expanded: false,
            });
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
                    rows.push(SidebarRow {
                        kind: RowKind::Note,
                        tab: tab.id,
                        text,
                        page: None,
                        depth: 0,
                        current: false,
                        notice: Some(hover),
                        action,
                        entry: NONE,
                        expandable: false,
                        expanded: false,
                    });
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
        SidebarModel {
            rows,
            rail_filled,
            toc_open,
            place,
            edited,
            undone_owner_steps,
        }
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
            let Some(oldest) = self.generated.iter().position(|(tab, _)| Some(*tab) != self.active) else {
                break;
            };
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
        self.generated
            .iter()
            .filter(|(_, view)| *view == GeneratedView::Making)
            .map(|(tab, _)| *tab)
            .collect()
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
    pub fn compare(&mut self, tabs: &[TabId], reading: Option<TabId>) -> Result<bool, compare::CompareError> {
        if tabs.iter().any(|id| self.tab(*id).is_none()) {
            return Err(compare::CompareError::NoSuchTab);
        }
        let comparison = compare::Comparison::new(tabs.to_vec())?;
        let was = self.comparison.clone();
        for id in tabs {
            self.wake_list(*id);
        }
        let reading = reading
            .filter(|id| tabs.contains(id))
            .or(self.active.filter(|id| tabs.contains(id)))
            .unwrap_or(tabs[0]);
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
        Ok(self.comparison != was)
    }

    /// What a ⌘-click on this tab would wake from cold, and its file, to read with the session free: the tab joining, or the pane read next if the one being read leaves. `None` if nothing needs reading.
    pub fn wake_for_compare_click(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        let joining = !self.comparison.as_ref().is_some_and(|comparison| comparison.contains(id));
        let next = if joining {
            id
        } else if self.active == Some(id) {
            self.pane_after(id)?
        } else {
            return None;
        };
        if self.active == Some(next) {
            return None;
        }
        self.file_to_wake_now(next)
    }

    /// The panes a comparison of these tabs would wake from cold, with their files, to read with the session free before [`Self::compare`].
    pub fn wake_for_compare(&mut self, tabs: &[TabId]) -> Vec<(TabId, PathBuf)> {
        let active = self.active;
        tabs.iter()
            .filter(|id| active != Some(**id))
            .filter_map(|id| self.file_to_wake_now(*id))
            .collect()
    }

    /// **⌘-click and ⌃-click on a tab** (owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view"). A tab not in it joins at the right and is the one read; one in it leaves; down to one, that one's alone. On the only tab showing, nothing.
    pub fn toggle_compare(&mut self, id: TabId) -> Result<compare::CompareChange, compare::CompareError> {
        if self.tab(id).is_none() {
            return Err(compare::CompareError::NoSuchTab);
        }
        let Some(mut comparison) = self.comparison.clone() else {
            let Some(active) = self.active.filter(|active| *active != id) else {
                return Ok(compare::CompareChange::Nothing);
            };
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

    /// Synced scroll on, from where each pane is now, or separate. Says nothing if there was no comparison to set, and otherwise whether it changed.
    pub fn set_compare_synced(&mut self, on: bool) -> Option<bool> {
        let pages: HashMap<TabId, u32> = self.tabs.iter().map(|tab| (tab.id, tab.page)).collect();
        self.comparison.as_mut().map(|comparison| {
            let was = comparison.clone();
            comparison.set_synced(on, |id| pages.get(&id).copied().unwrap_or(0));
            *comparison != was
        })
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
        self.comparison
            .as_ref()
            .map(|comparison| comparison.sync_targets(leader, page, |id| self.pages_in(id)))
            .unwrap_or_default()
    }

    /// What's said about the panes' lengths, if anything.
    pub fn compare_note(&self) -> Option<String> {
        self.comparison.as_ref()?.length_note(|id| self.pages_in(id))
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
    folds
        .and_then(|folds| folds.iter().rev().find(|(at, _)| *at == entry))
        .map_or_else(|| chain.contains(&entry), |(_, open)| *open)
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
            Self::EditsUnreadable => {
                "This tab's edits can't be read back just now, so nothing more was done to them. They're kept, and it tries again each time. Discarding them is what gives up on them."
            }
            Self::UnsavedChanges => "This PDF has unsaved changes.",
            Self::Edit(error) => error.message(),
            Self::RendererOnly => "RectoPDF can draw this file but can't read its structure, so its pages can't be changed.",
            Self::Saving => "This PDF is being saved. Nothing changes until that's done, which is a moment.",
        }
    }
}

#[cfg(test)]
mod tests;
