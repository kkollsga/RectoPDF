//! Tabs, tiers and the sidebar: every rule the host needs, with no UI toolkit in sight.
//!
//! **Cost follows activity.** A cold tab is a [`TabRecord`] and nothing else: no handle, no contents, no pages. Opening
//! reads a PDF's facts and closes it. Only the active tab and the most recent others keep their contents, up to
//! [`MAX_WARM`], and anything evicted is rebuilt from disk when it's needed again.

pub mod edit;

pub use edit::Author;
use edit::{EditError, PageEdits};
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Tab,
    Contents,
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

/// A tab's row in the sidebar, in points.
pub const TAB_ROW_HEIGHT: f64 = 32.0;
/// A contents row in the sidebar, in points.
pub const CONTENTS_ROW_HEIGHT: f64 = 28.0;
/// The most of the sidebar's room the contents take: about 60%, by the owner's yes of 2026-11-07. The rest is for the other tabs, and it's what keeps them from being pushed off by one long outline.
pub const CONTENTS_MAX_SHARE: f64 = 0.6;

/// How much of the sidebar's list goes where, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarHeights {
    /// The height of the contents' own scrolling area. Less than their rows' total means they scroll.
    pub contents: f64,
    /// Whether the list of tabs doesn't fit, and scrolls by itself.
    pub tabs_scroll: bool,
}

/// **The sidebar's height rule.** Owner, 2026-11-07: "The max height of toc does not seem to be working", then yes to a true maximum: the contents take what they need, up to [`CONTENTS_MAX_SHARE`] of the room, and scroll inside that. The other tabs have what's left, and a long list of them scrolls by itself in it. It takes the room it's told of, so anything else docked in the sidebar comes off first.
pub fn sidebar_heights(room: f64, tabs: u32, contents_rows: u32) -> SidebarHeights {
    let room = if room.is_finite() { room.max(0.0) } else { 0.0 };
    let natural = f64::from(contents_rows) * CONTENTS_ROW_HEIGHT;
    // With a window too short for the rest, the active tab's own row comes first.
    let contents = natural.min(room * CONTENTS_MAX_SHARE).min(room - if tabs > 0 { TAB_ROW_HEIGHT } else { 0.0 }).max(0.0);
    SidebarHeights { contents, tabs_scroll: f64::from(tabs) * TAB_ROW_HEIGHT + contents > room }
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
        Self { paused: false, pointer_inside: false, last_user_scroll: f64::NEG_INFINITY, current: None, placed: false, moving: None }
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
    /// Sidebar hidden, page number only, agent icon kept.
    pub focus: bool,
    /// Owner, 2026-11-07: "Add an option in the macos top bar to turn off agent bar (hides the icon)".
    pub agent_icon_hidden: bool,
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
    /// The latest pages drawn or tried, newest last, at most [`MAX_DRAWS_KEPT`]. Lets a waiter find out about a page finished a moment before it asked.
    draws: VecDeque<DrawNote>,
    /// Cold tabs' files, read ahead of waking them with this session free. Each is taken by the next [`Self::activate`] of that tab, and goes if the tab stops being cold. At most [`MAX_READ_AHEAD`].
    woken: Vec<(TabId, Result<DocFacts, OpenError>, Option<FileStamp>)>,
    /// The last edit removed the page a tab was on, so its place moved to the nearest one. Read once with [`Session::take_place_lost`].
    place_lost: bool,
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
        Self { loader, view: ViewSettings::default(), tabs: Vec::new(), active: None, next_id: 1, warm: VecDeque::new(), woken: Vec::new(), dirs: Vec::new(), dir_ids: HashMap::new(), edits: HashMap::new(), draws: VecDeque::new(), place_lost: false }
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

    /// Opens a PDF as a new active tab. Opening a file that already has a tab goes to that tab instead.
    pub fn open(&mut self, path: &Path) -> Result<TabId, SessionError> {
        if let Some(id) = self.go_to_open_file(path) {
            return Ok(id);
        }
        // Looked at first, so a change during the read shows as a change.
        let stamp = file_stamp(path).ok();
        let facts = self.loader.load(path).map_err(SessionError::Open)?;
        self.open_with_facts(path, facts, stamp)
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

    /// The second half of [`Self::open`]: makes the tab from facts already read. Someone may have opened the same file meanwhile, and then that's the tab.
    pub fn open_with_facts(&mut self, path: &Path, facts: DocFacts, stamp: Option<FileStamp>) -> Result<TabId, SessionError> {
        if let Some(id) = self.go_to_open_file(path) {
            return Ok(id);
        }
        let dir_path = path.parent().unwrap_or(Path::new(""));
        let file_name = path.file_name().ok_or(SessionError::Open(OpenError::NotFound))?;
        if self.tabs.len() >= MAX_TABS {
            return Err(SessionError::TooManyTabs);
        }
        let dir = self.intern_dir(dir_path);
        let id = TabId(self.next_id);
        self.next_id += 1;
        // The name comes from outside: a downloaded file can carry anything in it, and this reaches the sidebar and agents.
        let stem = Path::new(file_name).file_stem().unwrap_or(file_name).to_string_lossy();
        let name = match recto_core::clean_line(&stem) {
            name if name.is_empty() => "Untitled".to_string(),
            name => name,
        };
        self.tabs.push(TabRecord {
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
        });
        self.remember_warm(id, Arc::new(facts.toc));
        self.active = Some(id);
        Ok(id)
    }

    /// The tab tapping this one would wake from cold, and its file, so that can be read with this session free: a file on a dead volume mustn't hold up everything else that asks about tabs.
    pub fn wake_for_tap(&self, id: TabId) -> Option<(TabId, PathBuf)> {
        (self.active != Some(id)).then(|| self.file_to_wake(id)).flatten()
    }

    /// The same, for closing this tab: what becomes active next.
    pub fn wake_for_close(&self, id: TabId) -> Option<(TabId, PathBuf)> {
        if self.active != Some(id) {
            return None;
        }
        let index = self.tabs.iter().position(|tab| tab.id == id)?;
        let next = self.tabs.get(index + 1).or_else(|| index.checked_sub(1).and_then(|before| self.tabs.get(before)))?;
        self.file_to_wake(next.id)
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
        tab.page = page.min(pages.saturating_sub(1));
        Ok(())
    }

    /// How many pages a tab has now, counting unsaved edits.
    pub fn pages_in(&self, id: TabId) -> u32 {
        self.edits.get(&id).map(PageEdits::len).or_else(|| self.tab(id).map(|t| t.page_count)).unwrap_or(0)
    }

    /// Whether a tab has unsaved changes.
    pub fn is_edited(&self, id: TabId) -> bool {
        self.edits.get(&id).is_some_and(PageEdits::is_edited)
    }

    /// Whether the owner's edits on this tab have been undone and can still be redone. Closing would lose them.
    pub fn has_undone_owner_steps(&self, id: TabId) -> bool {
        self.edits.get(&id).is_some_and(PageEdits::owner_steps_undone)
    }

    /// A tab's page edits, if it has any.
    pub fn page_edits(&self, id: TabId) -> Option<&PageEdits> {
        self.edits.get(&id)
    }

    /// Applies the owner's page edit: rearranging, rotating or deleting. This is what the app's own edits call, with nothing to say whose they are. Everything from the agent interface goes through [`Self::edit_pages_as`] with [`Author::Agent`].
    pub fn edit_pages(&mut self, id: TabId, change: impl FnOnce(&mut PageEdits) -> Result<(), EditError>) -> Result<(), SessionError> {
        self.edit_pages_as(id, Author::Owner, change)
    }

    /// Applies a page edit, noting who made it. The list is made the first time a tab is edited, and goes again if that edit was refused. The reader's place follows its page.
    pub fn edit_pages_as(&mut self, id: TabId, by: Author, change: impl FnOnce(&mut PageEdits) -> Result<(), EditError>) -> Result<(), SessionError> {
        let tab = self.tab(id).ok_or(SessionError::NoSuchTab)?;
        if tab.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        let page_count = tab.page_count;
        let place = self.place_of(id);
        let edits = self.edits.entry(id).or_insert_with(|| PageEdits::new(page_count));
        edits.act_as(by);
        let result = change(edits);
        edits.act_as(Author::Owner);
        if result.is_ok() {
            self.after_change(id, place);
        } else {
            self.drop_list_if_idle(id);
        }
        result.map_err(SessionError::Edit)
    }

    /// What the reader is looking at: which file, which page there, and where it is now.
    fn place_of(&self, id: TabId) -> Option<(u16, u32, u32)> {
        let position = self.tab(id)?.page;
        match self.edits.get(&id) {
            Some(edits) => edits.slots().get(position as usize).map(|slot| (slot.source, slot.page, position)),
            None => Some((0, position, position)),
        }
    }

    /// Everything a change to the page order means: a drawn page's number no longer names that page, and the reader stays on what they were reading.
    fn after_change(&mut self, id: TabId, place: Option<(u16, u32, u32)>) {
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
        }
    }

    /// An edit list with nothing in it costs 8 bytes a page for nothing, and would hold the tab to the file's length as it was. One that's back at the file with steps to go back through stays until those are gone.
    fn drop_list_if_idle(&mut self, id: TabId) {
        if self.edits.get(&id).is_some_and(|edits| !edits.is_edited() && !edits.has_history()) {
            self.edits.remove(&id);
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
        self.edit_pages_as(id, by, |edits| edits.insert_pdf_stamped(at, path, facts.page_count, stamp))
    }

    /// Where a page of a tab comes from now, counting unsaved edits. Zero-based.
    pub fn page_source(&self, id: TabId, position: u32) -> Option<PageSource> {
        let tab = self.tab(id)?;
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
        if !self.edits.contains_key(&id) {
            return true;
        }
        tab.stamp.is_none() || self.path_of(id).and_then(|path| file_stamp(&path).ok()) == tab.stamp
    }

    /// What a tab's own file was when its page numbers were recorded, for a caller listing pages by file.
    pub fn own_stamp(&self, id: TabId) -> Option<FileStamp> {
        self.tab(id).and_then(|tab| tab.stamp)
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
        let place = self.place_of(id);
        let undone = self.edits.get_mut(&id).is_some_and(PageEdits::undo);
        if undone {
            self.after_change(id, place);
        }
        undone
    }

    /// Repeats the last undone page edit. Returns false with nothing to redo.
    pub fn redo_edit(&mut self, id: TabId) -> bool {
        let place = self.place_of(id);
        let redone = self.edits.get_mut(&id).is_some_and(PageEdits::redo);
        if redone {
            self.after_change(id, place);
        }
        redone
    }

    /// Throws away a tab's unsaved page edits.
    pub fn discard_edits(&mut self, id: TabId) {
        let place = self.place_of(id);
        if self.edits.remove(&id).is_some() {
            self.after_change(id, place);
        }
    }

    /// Closes a tab and releases what it held. The next tab along, or the previous one, becomes active.
    /// Refuses while there are unsaved changes, or undone edits of the owner's, so nothing is lost by accident. [`Self::discard_edits`] is how to say it's meant, as the app does when told "Don't Save".
    pub fn close(&mut self, id: TabId) -> Result<(), SessionError> {
        if self.is_edited(id) || self.has_undone_owner_steps(id) {
            return Err(SessionError::UnsavedChanges);
        }
        self.edits.remove(&id);
        let index = self.tabs.iter().position(|t| t.id == id).ok_or(SessionError::NoSuchTab)?;
        self.tabs.remove(index);
        self.draws.retain(|d| d.tab != id);
        self.warm.retain(|(tab, _)| *tab != id);
        self.woken.retain(|(tab, _, _)| *tab != id);
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
            rows.push(SidebarRow { kind: RowKind::Tab, tab: tab.id, text: &tab.name, page: None, depth: 0, current: is_active });
            if !is_active {
                continue;
            }
            toc_open = tab.toc_open;
            place = format!("{} of {}", tab.page + 1, self.pages_in(tab.id));
            if tab.toc_open
                && let Some((_, toc)) = self.warm.iter().find(|(id, _)| *id == tab.id)
            {
                rail_filled = contents_rows(toc, tab, self.edits.get(&tab.id), &mut rows);
            }
        }
        let edited = self.active.is_some_and(|id| self.is_edited(id));
        let undone_owner_steps = self.active.is_some_and(|id| self.has_undone_owner_steps(id));
        SidebarModel { rows, rail_filled, toc_open, place, edited, undone_owner_steps }
    }

    fn activate(&mut self, id: TabId) {
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
        // An edited tab's length is its edit list's, not the file's: added pages must stay reachable.
        let pages_with_edits = self.edits.get(&id).map(PageEdits::len);
        let tab = self.tab_mut(id);
        match loaded {
            Ok(facts) => {
                if let Some(tab) = tab {
                    tab.page_count = facts.page_count;
                    tab.page = tab.page.min(pages_with_edits.unwrap_or(facts.page_count).saturating_sub(1));
                    tab.problem = None;
                    tab.renderer_only = facts.renderer_only;
                    // With edits, the numbers in them are about the version they were made on, and that must stay known.
                    if pages_with_edits.is_none() {
                        tab.stamp = stamp;
                    }
                }
                self.remember_warm(id, Arc::new(facts.toc));
            }
            Err(problem) => {
                if let Some(tab) = tab {
                    tab.problem = Some(problem);
                }
            }
        }
    }

    fn remember_warm(&mut self, id: TabId, toc: Arc<Toc>) {
        self.warm.push_front((id, toc));
        self.warm.truncate(MAX_WARM);
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
    let original_page = match edits {
        Some(edits) => edits.slots().get(tab.page as usize).filter(|s| s.source == 0).map(|s| s.page),
        None => Some(tab.page),
    };
    let here = original_page.and_then(|page| toc.section_at(page));
    let mut open = HashSet::new();
    let mut cursor = here.map_or(NONE, |i| u32::try_from(i).unwrap_or(NONE));
    while cursor != NONE && open.insert(cursor) {
        cursor = toc.entries().get(cursor as usize).map_or(NONE, |e| e.parent);
    }
    let first_row = rows.len();
    let mut filled = 0;
    for (index, entry) in toc.entries().iter().enumerate() {
        if entry.parent != NONE && !open.contains(&entry.parent) {
            continue;
        }
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
        });
        if current {
            filled = rows.len() - first_row;
        }
    }
    filled
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
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Open(error) => error.message(),
            Self::NoSuchTab => "That tab isn't open.",
            Self::TooManyTabs => "Too many PDFs are open. Close some and try again.",
            Self::UnsavedChanges => "This PDF has unsaved changes.",
            Self::Edit(error) => error.message(),
            Self::RendererOnly => "RectoPDF can draw this file but can't read its structure, so its pages can't be changed.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
