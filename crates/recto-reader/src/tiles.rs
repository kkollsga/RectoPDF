//! What's wanted on screen, what's kept, and what the host is told. All the policy for the page view, as plain state: the UI thread calls [`Reader::set_viewport`] on every scroll, so nothing here waits. The one lock
//! around this is never held across a render or a disk read, which [`crate::engine`] does outside it. Numbers are from the platform report, §2.6.
//!
//! **Three levels on a page,** so a page seen once never goes blank: a preview at a quarter of the fitted scale, tiles of another zoom, and exact tiles. A page shows what it has, and when its visible tiles are all there, the
//! rest come off and go discardable.
//!
//! **What's in memory.** Only tiles on layers count. Anything off-screen is marked discardable the moment the host says it's off the layer: it leaves the footprint at once, and is usually still there on the way back.

use crate::layout::{PageBox, PageLayout, Spread};
use crate::selection::{Notice, Selecting, Urgency};
use recto_index::select::{PagePoint, Range};
use crate::paper::PaperKind;
use recto_platform::FileStamp;
use recto_platform::surface::{PixelSurface, Purge};
use recto_proto::{BYTES_PER_PIXEL, MAX_REGION_BYTES, MAX_SCALE, MIN_SCALE, TILE_EDGE, TileRect, device_size};
use recto_render_service::{CancelHandle, LowResWhy, TileKind, plan_regions};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tiles attached in one turn of the UI thread. Platform report §2.6: it stays under budget, and the next turn takes the rest.
pub const MAX_ATTACH_PER_TURN: usize = 8;
/// Off-screen surfaces kept, by bytes and by count. Least recently used go first. Platform report §2.6.
const MAX_KEPT_BYTES: usize = 256 * 1024 * 1024;
const MAX_KEPT_SURFACES: usize = 1024;
/// Previews kept off screen, about 0.75 MiB each and discardable. Platform report §2.6 said 16. More costs nothing while the Mac has memory, and each one is a page that doesn't come back blank.
const MAX_KEPT_PREVIEWS: usize = 128;
/// Entries of any kind, since a blank tile has no surface but is still remembered.
const MAX_ENTRIES: usize = 4096;
/// A preview is at most this: past it a page is too large for a preview to be cheap.
const MAX_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;
/// How much smaller a preview is than the fitted page. Platform report §2.5.
const PREVIEW_DIVISOR: f64 = 4.0;
/// A change of zoom smaller than this keeps the tiles there are.
const SCALE_SLACK: f64 = 0.005;
/// How soon a page that's only lower-resolution for lack of something passing may be tried again. It's only looked at when something else happens, so this costs nothing at rest.
const TRY_AGAIN_AFTER: Duration = Duration::from_secs(3);
/// Pages remembered as heavy or not.
const MAX_HEAVY_REMEMBERED: usize = 4096;
/// The zoom limits. Past these a pinch stops.
pub const MIN_ZOOM: f64 = 0.25;
pub const MAX_ZOOM: f64 = 8.0;
/// Pages either side of what's visible whose sizes are asked for, so layout near the reader settles early.
const SIZES_AROUND: u32 = 2;

/// What's asked of the zoom, from anywhere: a menu, a key, the panel, or `recto view --zoom`. All the same path and limits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    In,
    Out,
    /// The page fills the width. Magnification 1.
    Fit,
    /// A point of the page is a point on the screen.
    Actual,
    To(f64),
    /// A share of the page's real size, where 100 is [`Zoom::Actual`]: what the zoom level in the top left says and takes.
    Percent(f64),
}

/// The zoom level as the reader is told it: a share of the page's real size, so 100% is a point of the page to a point on the screen, as in any Mac app that shows a zoom. A magnification of 1 is the fitted width, whatever that is.
pub fn zoom_percent(magnification: f64, fit: f64) -> f64 {
    let percent = magnification * fit * 100.0;
    if percent.is_finite() { percent } else { 100.0 }
}

/// What a zoom level typed by the reader asks for, or nothing if it isn't one. `150`, `150%` and ` 150 % ` are all 150%. `decimal` is the reader's own decimal sign, as their Mac has it: the other of `.` and `,`, and spaces and `'`, group thousands, so `1,000` is a thousand with `.` and `87,5` is 87.5 with `,`. It's held to the limits when it's applied, like any zoom.
pub fn typed_zoom(text: &str, decimal: char) -> Option<Zoom> {
    let text = text.trim();
    let number = text.strip_suffix('%').unwrap_or(text).trim();
    // A number, and nothing a hostile paste could make long: past this it isn't a zoom level anyone typed.
    if number.is_empty() || number.chars().count() > 16 {
        return None;
    }
    let decimal = if decimal == ',' { ',' } else { '.' };
    let grouping = if decimal == ',' { '.' } else { ',' };
    let plain: String = number.chars().filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}' | '\'') && *c != grouping).map(|c| if c == decimal { '.' } else { c }).collect();
    if plain.is_empty() || plain.len() > 12 || !plain.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    plain.parse::<f64>().ok().filter(|value| value.is_finite() && *value > 0.0).map(Zoom::Percent)
}

/// How much one step of zoom is.
pub const ZOOM_STEP: f64 = 1.25;
/// How much a point of ⌘-scroll zooms, as a power: gentle enough that a flick of two fingers goes a step or two.
const ZOOM_PER_SCROLL_POINT: f64 = 0.006;

/// The zoom after this much ⌘-scroll, in points, upwards being in. Held to the limits, as a pinch is.
pub fn zoom_after_scroll(current: f64, points: f64) -> f64 {
    if !current.is_finite() || !points.is_finite() {
        return current.clamp(MIN_ZOOM, MAX_ZOOM);
    }
    (current * (points * ZOOM_PER_SCROLL_POINT).exp()).clamp(MIN_ZOOM, MAX_ZOOM)
}

/// The host's scrolled view, in document points, on each scroll or resize. The whole of it crosses in one call.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// The reading area, unmagnified, in window points.
    pub frame_width: f64,
    pub magnification: f64,
    /// Device pixels per point.
    pub backing: f64,
    /// A pinch or a resize is still going on. Old tiles stretch meanwhile, and nothing's drawn for a scale that's about to change.
    pub live: bool,
    /// The last [`Op::ScrollTo`] the host has applied.
    pub scrolled_to: u64,
}

/// What the host is told to draw. In order, in one transaction.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Everything comes off: another tab, or pages that changed.
    Clear,
    Document { width: f64, height: f64 },
    Magnify { value: f64 },
    ScrollTo { y: f64, seq: u64 },
    /// Across, when zoomed in. With no jump of its own: a view going across isn't a place to wait for.
    ScrollAcross { x: f64 },
    /// A page's layer, placed or moved. Its own coordinates are PDF points from its top left, as the page is before it's turned, and `fit` scales them to the document's. `turns` quarter turns clockwise turn it, and `x` and `y` are where its turned top left lands.
    ShowPage { page: u32, x: f64, y: f64, width: f64, height: f64, fit: f64, paper_rgb: u32, turns: u8 },
    HidePage { page: u32 },
    /// A tile on a page. `surface` is an extra reference that's now the host's, and zero if `rgb` says the colour. `crop_*` is how much of the surface it uses.
    Attach { id: u64, page: u32, x: f64, y: f64, width: f64, height: f64, surface: u64, rgb: Option<u32>, crop_width: f64, crop_height: f64, below: bool },
    Detach { id: u64 },
    /// What's selected on a page, in its own coordinates, all of it. Nothing clears it. It's a layer of the page's, so it scrolls and zooms with it and no tile knows.
    Highlight { page: u32, rects: Vec<recto_index::select::Rect> },
}

/// Which pages a tab's positions are, once it has been edited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageMap {
    /// The tab's own file first, then any added ones.
    pub files: Vec<PathBuf>,
    /// For each position, which file and which page there. `None` means position and page are the same, in the first file.
    pub slots: Option<Vec<(u16, u32)>>,
    /// Which version of the tab's own file this is. Another means every picture and tile of it is of something else, as a change of the page order does.
    pub own: Option<FileStamp>,
    /// Quarter turns clockwise an edit has added to each position, or none. They change how a page sits on the screen and nothing that's drawn of it.
    pub turns: Option<Arc<[u8]>>,
}

impl PageMap {
    /// The same pages from the same files, however they're turned: what's drawn of one is good for the other.
    fn same_pages(&self, other: &Self) -> bool {
        self.files == other.files && self.slots == other.slots && self.own == other.own
    }
}

impl PageMap {
    fn source(&self, position: u32) -> Option<(PathBuf, u32)> {
        match &self.slots {
            None => self.files.first().map(|file| (file.clone(), position)),
            Some(slots) => {
                let &(file, page) = slots.get(position as usize)?;
                self.files.get(usize::from(file)).map(|file| (file.clone(), page))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TileKey {
    tab: u32,
    page: u32,
    scale_bits: u32,
    column: u16,
    row: u16,
    paper: PaperKind,
    preview: bool,
}

#[derive(Debug)]
struct LowResNote {
    message: &'static str,
    may_clear: bool,
    at: Instant,
}

/// One drawn thing: a tile, a region of several, or a page's preview.
#[derive(Debug)]
struct Entry {
    id: u64,
    tab: u32,
    page: u32,
    scale: f32,
    paper: PaperKind,
    preview: bool,
    rect: TileRect,
    surface: Option<PixelSurface>,
    /// A tile of one colour, that needs no surface.
    rgb: Option<u32>,
    crop: (f64, f64),
    low_res: Option<LowResNote>,
    /// Which version of its file it was drawn from, as the worker said: see [`DocView::versions`]. `None` when nothing's said, as a picture from a sleep file written before this was known.
    version: Option<u64>,
    attached: bool,
    discardable: bool,
    last_used: u64,
}

#[derive(Clone, Debug)]
struct Issue {
    message: String,
    /// When it may have passed. `None` holds until the zoom changes or someone asks again.
    try_again_at: Option<Instant>,
}

#[derive(Debug)]
struct DocView {
    tab: u32,
    layout: PageLayout,
    map: PageMap,
    magnification: f64,
    anchor: (u32, f64),
    tile_scale: f32,
    issues: HashMap<u32, Issue>,
    heavy: HashMap<u32, bool>,
    /// The whole file can't be drawn, in words the reader can read.
    problem: Option<String>,
    seen: HashSet<u32>,
    /// Which page the reader was on when this tab was left, and exactly where in it.
    left_at: Option<(u32, (u32, f64))>,
    /// How far across the view is, when zoomed in.
    scroll_x: f64,
    /// Exactly how far down it was, and the document's height then, so that while nothing about the layout has changed it comes back to the very pixel, where the anchor would come to within rounding.
    exact_y: Option<(f64, f64)>,
    /// Jobs of a generation below this were about the pages as they were before the last edit, and what they found is thrown away.
    floor: u64,
    /// It came up from a sleep file, so a page shows on what its size is taken to be before it's been asked.
    size_on_trust: bool,
    /// Which version each of its files is, as a worker last said. **Nothing drawn of another version is shown, or goes in a sleep file:** that's checked as each goes on a layer, so however a file came to be replaced, and whatever was on its way, an old picture can't be what's showing.
    versions: HashMap<PathBuf, u64>,
}

/// Exactly where a tab's view is. For the session to keep, as a cold tab holds nothing here, and to bring back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SavedPlace {
    /// The page the place is about, as the session counts it.
    pub page: u32,
    pub anchor: (u32, f64),
    pub x: f64,
    pub y: f64,
    pub document_height: f64,
    pub magnification: f64,
    /// The size pages are taken to be, so there's no size to wait for on the first frame.
    pub page_size: Option<PageBox>,
}

/// What a page view takes from a sleep file for a tab's first frame: pages' sizes, and pictures to show until they're drawn. Its numbers have been held to what's possible on the way here, and these are held to the layout's own.
#[derive(Debug, Default)]
pub struct SleepView {
    /// Which version of the tab's own file its pictures are of, if its sleep file says.
    pub version: Option<u64>,
    pub usual: Option<PageBox>,
    pub sizes: Vec<(u32, PageBox)>,
    pub pictures: Vec<SleepShot>,
}

/// A picture for a page, in a surface of its own.
#[derive(Debug)]
pub struct SleepShot {
    pub page: u32,
    pub scale: f32,
    pub paper: PaperKind,
    pub surface: PixelSurface,
}

/// What a tab's sleep file gets from the page view: pages' sizes, and pictures of the pages at its place, as there are now.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SleepMaterial {
    /// Which version of the tab's own file this is all of, as a worker said. `None` if none has, and then there are no pictures, as there's no saying what they're of.
    pub version: Option<u64>,
    pub usual: Option<PageBox>,
    pub sizes: Vec<(u32, PageBox)>,
    pub pictures: Vec<RawPicture>,
}

/// A picture for a sleep file, in its own bytes. Rows are `width * 4` bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct RawPicture {
    pub page: u32,
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub paper: PaperKind,
    pub bgra: Vec<u8>,
}

/// Less of the view than this isn't a page that has to be showing something for a frame to be right.
const MIN_SHARE_OF_VIEW: f64 = 0.05;
/// How long a wake's followed, for how long it took.
const WAKE_FOLLOWED_FOR: Duration = Duration::from_secs(5);

/// Pictures that go in a sleep file: as many as [`recto_session::sleep::MAX_SLEEP_PICTURES`] says, which this crate doesn't know of. Six rows' worth of pages side by side, which are each a quarter of the pixels a page to a row has, so the bytes are about the same.
const MAX_SLEEP_SHOTS: usize = 12;

/// How a tab's wake went, for QA and the budgets: from being asked to the first frame showing something on every page, and to sharp.
#[derive(Clone, Copy, Debug, Default)]
pub struct WakeReport {
    pub tab: u32,
    /// It came from a sleep file.
    pub from_sleep: bool,
    pub first_frame_us: Option<u64>,
    pub sharp_us: Option<u64>,
    /// Frames the host applied with a visible page bare, once it was at its place.
    pub blank_frames: u32,
}

/// One thing for the engine to do. The surface, if any, is the job's until it comes back.
#[derive(Debug)]
pub struct Job {
    pub tab: u32,
    pub position: u32,
    pub file: PathBuf,
    pub source_page: u32,
    pub generation: u64,
    pub kind: JobKind,
}

#[derive(Debug)]
pub enum JobKind {
    /// Ask a page's size.
    Size,
    /// Ask whether a page is heavy.
    Traits,
    Draw { scale: f32, rect: TileRect, surface: PixelSurface, what: DrawKind, paper: PaperKind },
    /// Read a page's text for selecting. `check` also draws it, for the hidden-text check.
    Text { check: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawKind {
    Preview,
    Tile,
    /// One pass, cut on a grid of this many pixels.
    Region,
}

/// A job, before its surface is made. The reader's lock is held for as long as it takes the UI thread's `set_viewport` to get a word in, so what can take a while is done after it's let go.
#[derive(Debug)]
pub struct JobPlan {
    pub tab: u32,
    pub position: u32,
    pub file: PathBuf,
    pub source_page: u32,
    pub generation: u64,
    kind: PlanKind,
}

#[derive(Debug)]
enum PlanKind {
    Size,
    Traits,
    Draw { scale: f32, rect: TileRect, what: DrawKind, paper: PaperKind },
    Text { check: bool },
}

impl JobPlan {
    /// Makes its surface. A new one for each job: see [`Self::take_surface`]. `None` if it can't be made, which leaves things as if there'd been nothing to do.
    pub fn ready(self) -> Option<Job> {
        let Self { tab, position, file, source_page, generation, kind } = self;
        let kind = match kind {
            PlanKind::Size => JobKind::Size,
            PlanKind::Traits => JobKind::Traits,
            PlanKind::Text { check } => JobKind::Text { check },
            PlanKind::Draw { scale, mut rect, what, paper } => {
                let surface = match what {
                    DrawKind::Preview => Self::take_surface(rect.width, rect.height)?,
                    DrawKind::Tile => Self::take_surface(TILE_EDGE, TILE_EDGE)?,
                    DrawKind::Region => loop {
                        let surface = Self::take_surface(rect.width, rect.height)?;
                        // A surface's rows can be padded, which can take it over the cap. A row of tiles less is always under.
                        if u64::from(surface.stride()) * u64::from(rect.height) <= MAX_REGION_BYTES || rect.height <= TILE_EDGE {
                            break surface;
                        }
                        rect.height -= TILE_EDGE;
                    },
                };
                JobKind::Draw { scale, rect, surface, what, paper }
            }
        };
        Some(Job { tab, position, file, source_page, generation, kind })
    }

    /// A new surface for each job, never one that's been used. A worker can read and write a surface it was sent for as long as it lives, whatever it's told, so a surface used twice is a way from one file's worker to another file's pixels. It costs 51 µs more a tile than reusing one, nearly all of it the first touch of its pages: 67 against 16, p50.
    fn take_surface(width: u32, height: u32) -> Option<PixelSurface> {
        PixelSurface::create(width, height).ok()
    }
}

/// Copies a tile's pixels into a picture of its page, a step-sized box of them to each pixel. A pixel past the edge of either is left.
#[allow(clippy::too_many_arguments)]
fn shrink_into(source: &[u8], stride: usize, used_width: u32, used_height: u32, step: u32, at: (u32, u32), picture: &mut [u8], width: u32, height: u32) {
    let (step, picture_row) = (step.max(1) as usize, width as usize * 4);
    let area = (step * step) as u32;
    for y in 0..(used_height as usize / step) {
        let target_y = at.1 as usize + y;
        if target_y >= height as usize {
            break;
        }
        for x in 0..(used_width as usize / step) {
            let target_x = at.0 as usize + x;
            if target_x >= width as usize {
                break;
            }
            let mut sum = [0u32; 4];
            for row in 0..step {
                let start = (y * step + row) * stride + x * step * 4;
                let Some(pixels) = source.get(start..start + step * 4) else { continue };
                for pixel in pixels.as_chunks::<4>().0 {
                    for (total, byte) in sum.iter_mut().zip(pixel) {
                        *total += u32::from(*byte);
                    }
                }
            }
            let out = target_y * picture_row + target_x * 4;
            for (channel, total) in sum.iter().enumerate() {
                picture[out + channel] = (total / area) as u8;
            }
        }
    }
}

/// How a job came out.
#[derive(Debug)]
pub enum Outcome {
    Size(PageBox),
    Heavy(bool),
    Drawn { kind: TileKind, low_res: Option<(TileRect, LowResWhy)> },
    Cancelled,
    /// The page can't be drawn now or ever, in these words.
    PageFailed { message: String, may_pass: bool },
    /// The file can't be drawn, in these words.
    DocumentFailed(String),
    /// A page's text, for selecting. `checked` says a hidden-text check was asked for and has been tried, however it came out, and not stopped because nobody wanted it.
    Text { text: Arc<recto_index::select::PageText>, checked: bool },
    /// A page's text can't be read. That's the page's text, and nothing about drawing it.
    TextFailed,
}

#[derive(Debug)]
struct Running {
    tab: u32,
    position: u32,
    rect: Option<TileRect>,
    scale: f32,
    /// A preview's scale isn't the tiles', and it's wanted as long as its page is.
    preview: bool,
    generation: u64,
    cancel: Option<CancelHandle>,
    cancel_asked: bool,
    /// It's reading text. A selection or a copy can want a page that isn't in view, so where the view is says nothing about it.
    text: bool,
    /// It's drawing for a hidden-text check too, which can take seconds, and can go when there's something more wanted.
    check: bool,
}

/// How a page sits.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    fit: f64,
    paper_rgb: u32,
    turns: u8,
}

/// Counts for QA and the budgets.
#[derive(Clone, Debug, Default)]
pub struct ReaderCounts {
    pub jobs: u64,
    pub drawn: u64,
    pub cancelled: u64,
    pub viewport_steps: u64,
    /// Scroll steps that left a page in view with nothing on it, though it had been seen before. The budget says zero.
    pub blank_seen_steps: u64,
    /// The same, for any page. A page that's never been seen has nothing to show the first time.
    pub blank_steps: u64,
    pub discards_lost: u64,
    /// Jobs told to stop because nobody wanted them any more.
    pub cancels_asked: u64,
}

/// What QA reads.
pub struct ReaderReport(pub Value);

pub struct Reader {
    docs: HashMap<u32, DocView>,
    hot: Option<u32>,
    viewport: Viewport,
    paper: PaperKind,
    window_open: bool,
    entries: HashMap<u64, Entry>,
    cover: HashMap<TileKey, u64>,
    next_id: u64,
    /// Attached entries by page, on the hot tab only.
    attached: HashMap<u32, Vec<u64>>,
    shown: HashMap<u32, Placement>,
    ops: VecDeque<Op>,
    /// The host has been given ops it hasn't said it's applied.
    unapplied: bool,
    /// Gone off a layer, in ops the host hasn't been given yet.
    to_discard: Vec<u64>,
    /// Gone off a layer, in ops the host has been given. It's these that "applied" is about.
    handed_to_host: Vec<u64>,
    clock: u64,
    running: Option<Running>,
    next_generation: u64,
    next_seq: u64,
    jump: Option<u64>,
    scrolled_to: u64,
    last_view_page: Option<u32>,
    moved_page: Option<u32>,
    more_to_attach: bool,
    counts: ReaderCounts,
    wanted_files: Arc<Vec<PathBuf>>,
    note: Option<String>,
    selecting: Selecting,
    /// The thread that draws has failed too many times to go on trying, and this says so.
    gave_up: bool,
    /// What's come from a tab's sleep file, for when it's made the hot tab.
    sleep_view: Option<(u32, SleepView)>,
    /// Files whose renderers have the wrong version open, to be let go of and opened again.
    reopen_files: Vec<PathBuf>,
    /// Pictures that came from sleep files, which a worker's word on the version goes over.
    from_sleep: HashSet<u64>,
    /// The window's on the screen, so what the host applies is a frame someone can see.
    window_up: bool,
    /// For QA: nothing's drawn, so a first frame stays to be looked at.
    hold_drawing: bool,
    /// When the wake under way was asked for, and how it's gone.
    waking: Option<(Instant, WakeReport)>,
    last_wake: Option<WakeReport>,
    /// How many page views share the caps on what's kept: one, or each pane of a comparison, so several panes keep no more between them than one view does.
    share: usize,
    /// The least room either side of the pages: a comparison's pane has less than a window.
    margins: (f64, f64),
    /// The last viewport was the reader scrolling, and not the host arriving where it was sent, a zoom, a resize or a pinch: what synced scroll goes by.
    user_scrolled: bool,
}

/// What a page says once the thread that draws has given up.
const CANT_DRAW: &str = "RectoPDF can't draw pages right now, because something went wrong too many times. Quit and open it again.";

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl Reader {
    pub fn new() -> Self {
        Self {
            docs: HashMap::new(),
            hot: None,
            viewport: Viewport { magnification: 1.0, backing: 2.0, ..Viewport::default() },
            paper: PaperKind::White,
            window_open: true,
            entries: HashMap::new(),
            cover: HashMap::new(),
            next_id: 1,
            attached: HashMap::new(),
            shown: HashMap::new(),
            ops: VecDeque::new(),
            unapplied: false,
            to_discard: Vec::new(),
            handed_to_host: Vec::new(),
            clock: 0,
            running: None,
            next_generation: 1,
            next_seq: 1,
            jump: None,
            scrolled_to: 0,
            last_view_page: None,
            moved_page: None,
            more_to_attach: false,
            counts: ReaderCounts::default(),
            wanted_files: Arc::new(Vec::new()),
            note: None,
            selecting: Selecting::default(),
            gave_up: false,
            sleep_view: None,
            reopen_files: Vec::new(),
            from_sleep: HashSet::new(),
            window_up: false,
            hold_drawing: false,
            waking: None,
            last_wake: None,
            share: 1,
            user_scrolled: false,
            margins: (crate::layout::MIN_SIDE_MARGIN, crate::layout::CORNER_MARGIN),
        }
    }

    /// Shares the caps on what's kept off screen with this many page views in all, as a comparison's panes do: each keeps its share, so between them they keep what one does. Anything over the new share goes now.
    pub fn set_share(&mut self, views: usize) {
        let views = views.clamp(1, 8);
        if views != self.share {
            self.share = views;
            self.evict();
        }
    }

    /// The least room either side of the pages, as a comparison's pane has it. The place is kept.
    pub fn set_margins(&mut self, left: f64, right: f64) {
        self.margins = (left, right);
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else { return };
        if doc.layout.set_margins(left, right) {
            doc.exact_y = None;
            self.lay_out();
            self.refresh(MAX_ATTACH_PER_TURN);
        }
    }

    /// The tab showing's magnification, if one is.
    pub fn magnification(&self) -> Option<f64> {
        self.doc().map(|doc| doc.magnification)
    }

    /// Where the reader is in the tab showing: a page and how far down it, as a change of layout keeps it.
    pub fn reading_anchor(&self) -> Option<(u32, f64)> {
        self.doc().map(|doc| doc.anchor)
    }

    /// Whether the last viewport was the reader scrolling, which another pane follows with synced scroll. Asking clears it.
    pub fn take_user_scrolled(&mut self) -> bool {
        std::mem::take(&mut self.user_scrolled)
    }

    /// Sends the view to a place in a page, as another pane's reader is in theirs, for synced scroll. A jump, so arriving there isn't the reader moving, and neither is it this pane leading the others. Held to the document. Says which page it went to.
    pub fn follow(&mut self, page: u32, within: f64) -> Option<u32> {
        self.finish_wake();
        let doc = self.hot.and_then(|tab| self.docs.get_mut(&tab))?;
        let page = page.min(doc.layout.count().saturating_sub(1));
        let within = if within.is_finite() { within.clamp(0.0, 1.0) } else { 0.0 };
        doc.anchor = (page, within);
        let y = doc.layout.anchor_y((page, within));
        doc.exact_y = Some((y, doc.layout.document_height()));
        let seq = self.next_seq;
        self.next_seq += 1;
        self.jump = Some(seq);
        let moved = self.last_view_page != Some(page);
        self.last_view_page = Some(page);
        if moved {
            self.moved_page = Some(page);
        }
        self.ops.push_back(Op::ScrollTo { y, seq });
        Some(page)
    }

    /// **What a tab's first frame shows, from its sleep file, ahead of [`Self::activate`].** Pictures of the pages at its place, kept as those pages' previews, so they go on as a preview does, and the same rules take them off. The tab's been asked for from here, for how long it takes.
    pub fn give_sleep(&mut self, tab: u32, view: SleepView) {
        self.sleep_view = Some((tab, view));
    }

    /// A tab's being woken, as of now. For how long it takes.
    pub fn note_wake(&mut self, tab: u32, began: Instant) {
        // Said when it was asked for, and said again on its way to the screen: the first is when it began.
        if self.hot != Some(tab) && !self.waking.is_some_and(|(_, report)| report.tab == tab) {
            self.waking = Some((began, WakeReport { tab, from_sleep: false, ..WakeReport::default() }));
        }
    }

    /// The files a tab it holds has.
    pub fn files_of(&self, tab: u32) -> Vec<PathBuf> {
        self.docs.get(&tab).map_or_else(Vec::new, |doc| doc.map.files.clone())
    }

    /// Whether this tab's anything here: a view of it's held, so a sleep file has nothing to add.
    pub fn holds(&self, tab: u32) -> bool {
        self.docs.contains_key(&tab)
    }

    /// The version of the file a page of a tab is from, as a worker last said.
    fn version_of_page(&self, tab: u32, page: u32) -> Option<u64> {
        let doc = self.docs.get(&tab)?;
        let (file, _) = doc.map.source(page)?;
        doc.versions.get(&file).copied()
    }

    /// Whether something drawn is of the version of its file there is now. Unknown on either side passes: before a worker's said, what there is is all there is.
    fn is_current(&self, entry: &Entry) -> bool {
        match (entry.version, self.version_of_page(entry.tab, entry.page)) {
            (Some(drawn), Some(now)) => drawn == now,
            _ => true,
        }
    }

    /// **A worker's said which version of a file it has.** If that's another than was said before, everything drawn of the old one goes from every tab of it, on screen and off, and is drawn again. What came from a sleep file with no version said goes too, as there's no knowing.
    pub fn note_version(&mut self, file: &std::path::Path, version: u64) {
        let tabs: Vec<u32> = self.docs.values().filter(|doc| doc.map.files.iter().any(|known| known == file)).map(|doc| doc.tab).collect();
        for tab in tabs {
            let Some(doc) = self.docs.get_mut(&tab) else { continue };
            let was = doc.versions.insert(file.to_path_buf(), version);
            if was == Some(version) {
                continue;
            }
            let stale: Vec<u64> = self.entries.values().filter(|entry| entry.tab == tab && !matches!((entry.version, was), (Some(drawn), _) if drawn == version)).filter(|entry| was.is_some() || entry.version.is_some_and(|drawn| drawn != version) || entry.version.is_none() && entry.preview && self.from_sleep.contains(&entry.id)).map(|entry| entry.id).collect();
            if stale.is_empty() {
                continue;
            }
            for id in stale {
                if let Some(entry) = self.entries.get(&id)
                    && entry.attached
                {
                    let page = entry.page;
                    if self.hot == Some(tab) {
                        self.detach(id, page);
                    }
                }
                self.forget_entry(id);
            }
            if self.hot == Some(tab) {
                self.refresh(MAX_ATTACH_PER_TURN);
            }
        }
    }

    /// Files whose renderers have to start again.
    pub fn take_reopen_files(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.reopen_files)
    }

    /// For QA: nothing's drawn till it's let go, so a first frame stays to be looked at.
    pub fn hold_drawing(&mut self, on: bool) {
        self.hold_drawing = on;
    }

    /// Whether this file is the tab's own, as its length is what a sleep file spoke for.
    pub fn is_own_file(&self, tab: u32, file: &std::path::Path) -> bool {
        self.docs.get(&tab).is_some_and(|doc| doc.map.files.first().is_some_and(|own| own == file))
    }

    /// The window's gone up. From here, a bare page is one that's seen.
    pub fn window_up(&mut self) {
        self.window_up = true;
    }

    /// How the last wake went.
    pub fn last_wake(&self) -> Option<WakeReport> {
        self.waking.map(|(_, report)| report).or(self.last_wake)
    }

    /// Whether every visible page is showing something, at the place, with nothing on its way to the host: what a window can go up with. It'll sharpen from there.
    pub fn is_showable(&self) -> bool {
        if self.jump.is_some() || !self.ops.is_empty() || self.unapplied || self.viewport.height <= 0.0 {
            return false;
        }
        let Some(doc) = self.doc() else { return false };
        doc.problem.is_none() && self.pages_all_covered(doc)
    }

    /// Whether every page that's really in view has something on it. A sliver of the page before, a few points at the top of the view, isn't what a frame's blank for: it has no picture if it wasn't drawn when the tab was left, and it comes a moment later with the rest.
    fn pages_all_covered(&self, doc: &DocView) -> bool {
        let (top, bottom) = (self.viewport.y, self.viewport.y + self.viewport.height);
        self.visible_pages(doc).all(|page| {
            let rect = doc.layout.rect(page);
            let showing = rect.y.max(top)..(rect.y + rect.height).min(bottom);
            showing.end - showing.start < self.viewport.height * MIN_SHARE_OF_VIEW || self.attached.get(&page).is_some_and(|ids| !ids.is_empty())
        })
    }

    fn finish_wake(&mut self) {
        if let Some((_, report)) = self.waking.take() {
            self.last_wake = Some(report);
        }
    }

    /// Goes through how a wake's going, each time the host has applied something.
    fn track_wake(&mut self) {
        let Some((began, mut report)) = self.waking else { return };
        if self.hot != Some(report.tab) {
            self.waking = None;
            return;
        }
        // Past this, it's not the wake that's being looked at any more.
        if began.elapsed() > WAKE_FOLLOWED_FOR {
            self.finish_wake();
            return;
        }
        let Some(doc) = self.doc() else { return };
        if self.jump.is_none() && self.viewport.height > 0.0 {
            let covered = self.pages_all_covered(doc);
            if covered && report.first_frame_us.is_none() {
                report.first_frame_us = Some(u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX));
            } else if !covered && report.first_frame_us.is_none() && self.window_up {
                report.blank_frames += 1;
            }
        }
        if report.first_frame_us.is_some() && self.is_settled() {
            report.sharp_us = Some(u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX));
            self.last_wake = Some(report);
            self.waking = None;
            return;
        }
        self.waking = Some((began, report));
    }

    /// **What a tab's sleep file gets from what's here.** Sizes, and a picture for each page at its place: its preview, if the system has left it, or else put together from whatever of its tiles are here, so a heavy page, which has no preview, has one too. A short lock: a picture's a few milliseconds at the worst.
    ///
    /// `own` is what the session says the tab's file is now. If that's not what this is a view of, what's here is of another file, and nothing goes in its sleep file.
    pub fn sleep_material(&mut self, tab: u32, own: Option<FileStamp>) -> Option<SleepMaterial> {
        let doc = self.docs.get(&tab)?;
        if doc.map.own != own {
            return None;
        }
        let usual = doc.layout.usual_size();
        let sizes = doc.layout.other_sizes();
        let version = doc.map.files.first().and_then(|file| doc.versions.get(file)).copied();
        let pages: Vec<u32> = if self.hot == Some(tab) {
            self.visible_pages(doc).collect()
        } else {
            let y = doc.exact_y.map_or_else(|| doc.left_at.map_or(0.0, |(_, anchor)| doc.layout.anchor_y(anchor)), |(y, _)| y);
            // Its own view: the window's height at the zoom it was left at, which the tab showing's view, at the tab showing's zoom, isn't.
            let window_height = self.viewport.height * self.viewport.magnification.max(f64::MIN_POSITIVE);
            let height = (window_height / doc.magnification.clamp(MIN_ZOOM, MAX_ZOOM)).max(1.0);
            if doc.layout.count() == 0 { Vec::new() } else { doc.layout.pages_between(y.max(0.0), y.max(0.0) + height).collect() }
        };
        let mut pictures = Vec::new();
        for page in pages.into_iter().take(MAX_SLEEP_SHOTS) {
            if version.is_some()
                && let Some(picture) = self.picture_of(tab, page)
            {
                pictures.push(picture);
            }
        }
        Some(SleepMaterial { version, usual, sizes, pictures })
    }

    fn picture_of(&mut self, tab: u32, page: u32) -> Option<RawPicture> {
        let paper = self.paper;
        let doc = self.docs.get(&tab)?;
        let preview_scale = ((doc.layout.fit() * self.viewport.backing / PREVIEW_DIVISOR) as f32).clamp(MIN_SCALE, MAX_SCALE);
        let size = doc.layout.raw_size(page);
        let paper_rgb = paper.rgb();
        // A preview as it is, if there's one.
        let preview = self.cover.get(&self.preview_key(doc, page)).copied().filter(|id| self.entries.get(id).is_some_and(|entry| entry.version.is_some() && self.is_current(entry)));
        if let Some(id) = preview
            && let Some(picture) = self.copy_preview(id)
        {
            return Some(picture);
        }
        // Otherwise the sharpest tiles there are of this page, at whole steps down.
        let ids: Vec<u64> = self.entries.values().filter(|entry| entry.tab == tab && entry.page == page && !entry.preview && entry.paper == paper && entry.surface.is_some() && entry.low_res.is_none() && entry.version.is_some() && self.is_current(entry)).map(|entry| entry.id).collect();
        let finest = ids.iter().filter_map(|id| self.entries.get(id)).map(|entry| entry.scale).fold(0.0f32, f32::max);
        if finest <= 0.0 {
            return None;
        }
        let step = (finest / preview_scale).round().max(1.0) as u32;
        let scale = finest / step as f32;
        let (width, height) = device_size(size.width, size.height, scale).ok()?;
        let bytes = u64::from(width) * u64::from(height) * u64::from(BYTES_PER_PIXEL);
        if bytes > MAX_PREVIEW_BYTES {
            return None;
        }
        let mut bgra = vec![0u8; bytes as usize];
        for pixel in bgra.as_chunks_mut::<4>().0 {
            *pixel = [(paper_rgb & 0xFF) as u8, ((paper_rgb >> 8) & 0xFF) as u8, ((paper_rgb >> 16) & 0xFF) as u8, 0xFF];
        }
        let mut any = false;
        for id in ids {
            let Some(entry) = self.entries.get(&id) else { continue };
            if entry.scale != finest {
                continue;
            }
            let was_discardable = entry.discardable;
            if was_discardable && entry.surface.as_ref().is_none_or(|surface| surface.reclaim() != Purge::Kept) {
                self.counts.discards_lost += 1;
                self.forget_entry(id);
                continue;
            }
            let Some(entry) = self.entries.get_mut(&id) else { continue };
            entry.discardable = false;
            let rect = entry.rect;
            let (crop_width, crop_height) = entry.crop;
            if let Some(surface) = &entry.surface {
                let stride = surface.stride() as usize;
                let (surface_width, surface_height) = (surface.width(), surface.height());
                let used_width = ((f64::from(surface_width) * crop_width) as u32).min(rect.width);
                let used_height = ((f64::from(surface_height) * crop_height) as u32).min(rect.height);
                let copied = surface.with_pixels(|source| shrink_into(source, stride, used_width, used_height, step, (rect.x / step, rect.y / step), &mut bgra, width, height));
                any |= copied.is_ok();
                if was_discardable {
                    surface.set_discardable();
                    entry.discardable = true;
                }
            }
        }
        any.then_some(RawPicture { page, scale, width, height, paper, bgra })
    }

    fn copy_preview(&mut self, id: u64) -> Option<RawPicture> {
        let entry = self.entries.get(&id)?;
        let was_discardable = entry.discardable;
        if was_discardable && entry.surface.as_ref().is_none_or(|surface| surface.reclaim() != Purge::Kept) {
            self.counts.discards_lost += 1;
            self.forget_entry(id);
            return None;
        }
        let entry = self.entries.get_mut(&id)?;
        entry.discardable = false;
        // A preview that's only what could be drawn then, or of one colour with no surface, isn't what's wanted.
        let (rect, page, scale, paper) = (entry.rect, entry.page, entry.scale, entry.paper);
        let usable = entry.low_res.is_none();
        let surface = entry.surface.as_ref()?;
        let stride = surface.stride() as usize;
        let row = rect.width as usize * 4;
        let copied = usable.then(|| surface.with_pixels(|source| {
            let mut bgra = Vec::with_capacity(row * rect.height as usize);
            for line in source.chunks_exact(stride).take(rect.height as usize) {
                bgra.extend_from_slice(line.get(..row)?);
            }
            (bgra.len() == row * rect.height as usize).then_some(bgra)
        }).ok().flatten()).flatten();
        if was_discardable {
            surface.set_discardable();
            entry.discardable = true;
        }
        Some(RawPicture { page, scale, width: rect.width, height: rect.height, paper, bgra: copied? })
    }

    /// Everything that can go without the page on screen going blank: what's off the layers, and at the worst, every picture too. Those that are discardable may have gone anyway, so this is what the system's asked of us, with nothing kept for later.
    pub fn memory_pressure(&mut self, critical: bool) {
        let ids: Vec<u64> = self.entries.values().filter(|entry| !entry.attached && (critical || !entry.preview)).map(|entry| entry.id).collect();
        for id in ids {
            self.forget_entry(id);
        }
    }

    /// How many surfaces are held, for QA.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn hot_tab(&self) -> Option<u32> {
        self.hot
    }

    /// Files the engine should keep a renderer for: hot and warm tabs'.
    pub fn wanted_files(&self) -> Arc<Vec<PathBuf>> {
        Arc::clone(&self.wanted_files)
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    pub fn is_window_open(&self) -> bool {
        self.window_open
    }

    pub(crate) fn has_moved_page(&self) -> bool {
        self.moved_page.is_some()
    }

    /// A page the reader scrolled to, for the session to take as its place. Asking clears it.
    pub fn take_moved_page(&mut self) -> Option<u32> {
        self.moved_page.take()
    }

    fn doc(&self) -> Option<&DocView> {
        self.hot.and_then(|tab| self.docs.get(&tab))
    }

    /// Makes a tab the one on screen, with its place. A tab seen before keeps its layout and zoom.
    #[allow(clippy::too_many_arguments)]
    pub fn activate(&mut self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32], saved: Option<SavedPlace>, spread: Spread) {
        self.keep_only(warm, tab);
        // The tab showing taken up again, as a turn of its pages does: the reader stays where they are.
        let staying = self.hot == Some(tab);
        if self.hot != Some(tab) {
            let last_page = self.last_view_page;
            let (left_x, left_y) = (self.viewport.x, self.viewport.y);
            if let Some(doc) = self.hot.and_then(|hot| self.docs.get_mut(&hot)) {
                doc.left_at = last_page.map(|page| (page, doc.anchor));
                doc.scroll_x = left_x;
                doc.exact_y = Some((left_y, doc.layout.document_height()));
            }
            self.clear_screen();
            self.hot = Some(tab);
            self.last_view_page = None;
            self.moved_page = None;
            // A selection is in what's on screen, and a tab off screen holds none.
            self.selecting.reset();
        }
        let floor = self.next_generation;
        let doc = self.docs.entry(tab).or_insert_with(|| {
            let mut layout = PageLayout::with_spread(pages, spread);
            layout.set_margins(self.margins.0, self.margins.1);
            layout.set_turns(map.turns.clone());
            // A tab it's never held, or that's gone cold: as the session has it. The numbers come from a record, so they're held to what a page can be.
            let saved = saved.filter(|saved| saved.page < pages.max(1));
            if let Some(size) = saved.and_then(|saved| saved.page_size) {
                layout.seed_usual(size);
            }
            let magnification = saved.map_or(1.0, |saved| if saved.magnification.is_finite() { saved.magnification.clamp(MIN_ZOOM, MAX_ZOOM) } else { 1.0 });
            let within = |value: f64| if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
            let left_at = saved.map(|saved| (saved.page, (saved.anchor.0.min(pages.saturating_sub(1)), within(saved.anchor.1))));
            let scroll_x = saved.map_or(0.0, |saved| if saved.x.is_finite() { saved.x.max(0.0) } else { 0.0 });
            let exact_y = saved.filter(|saved| saved.y.is_finite() && saved.y >= 0.0 && saved.document_height.is_finite()).map(|saved| (saved.y, saved.document_height));
            DocView { tab, layout, map: map.clone(), magnification, anchor: (page, 0.0), tile_scale: 0.0, issues: HashMap::new(), heavy: HashMap::new(), problem: None, seen: HashSet::new(), left_at, scroll_x, exact_y, floor, size_on_trust: false, versions: HashMap::new() }
        });
        // Back on the page it was left on, and it's back at the same place in that page. A tall page's reader isn't sent to its top.
        let same_pages = doc.map.same_pages(&map) && doc.layout.count() == pages;
        let back_at = if staying && same_pages { Some(doc.anchor) } else { doc.left_at.take().filter(|(left, _)| *left == page && same_pages).map(|(_, anchor)| anchor) };
        if back_at.is_none() {
            doc.scroll_x = 0.0;
            doc.exact_y = None;
        }
        doc.anchor = back_at.unwrap_or((page.min(pages.saturating_sub(1)), 0.0));
        if doc.map.turns != map.turns && doc.map.same_pages(&map) && doc.layout.count() == pages {
            // Turned, and nothing else: what's drawn stays, and the pages sit the new way.
            doc.layout.set_turns(map.turns.clone());
            doc.map.turns = map.turns.clone();
            doc.exact_y = None;
        }
        if !doc.map.same_pages(&map) || doc.layout.count() != pages {
            // Another version of its file: a worker that's running has the old one open, and goes on drawing that.
            if doc.map.own != map.own {
                self.reopen_files.extend(map.files.first().cloned());
            }
            doc.layout = PageLayout::with_spread(pages, spread);
            doc.layout.set_turns(map.turns.clone());
            doc.map = map;
            doc.issues.clear();
            doc.heavy.clear();
            doc.problem = None;
            doc.seen.clear();
            doc.floor = floor;
            if let Some(running) = &self.running
                && running.tab == tab
                && let Some(cancel) = &running.cancel
            {
                cancel.cancel_before(running.generation + 1);
            }
            self.drop_tab_entries(tab);
            self.clear_screen();
            // Its pages are other pages now, so what was selected on them isn't there.
            self.selecting.reset();
        }
        self.take_sleep_view(tab);
        // As the session has it for this tab: a tab held warm keeps its layout from when it was shown, which may have been changed since by an agent.
        if let Some(doc) = self.docs.get_mut(&tab)
            && doc.layout.set_spread(spread)
        {
            doc.exact_y = None;
        }
        let (magnification, across) = self.docs.get(&tab).map_or((1.0, 0.0), |doc| (doc.magnification, doc.scroll_x));
        self.ops.push_back(Op::Magnify { value: magnification });
        self.lay_out();
        self.jump_to_anchor(page);
        self.ops.push_back(Op::ScrollAcross { x: across });
        self.recount_wanted_files(warm);
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// What came from a sleep file goes in, if it's for this tab and there's nothing better here. Anything for another tab has been overtaken.
    fn take_sleep_view(&mut self, tab: u32) {
        let Some((for_tab, view)) = self.sleep_view.take() else { return };
        if for_tab != tab {
            return;
        }
        let Some(doc) = self.docs.get_mut(&tab) else { return };
        if let Some(usual) = view.usual {
            doc.layout.seed_usual(usual);
            doc.size_on_trust = true;
        }
        for (page, size) in view.sizes {
            doc.layout.set_page_size(page, size);
        }
        let count = doc.layout.count();
        let mut any = false;
        for shot in view.pictures {
            if shot.page >= count {
                continue;
            }
            // Its size is known, or it wouldn't be shown.
            if let Some(doc) = self.docs.get_mut(&tab) {
                let size = doc.layout.raw_size(shot.page);
                doc.layout.set_page_size(shot.page, size);
            }
            let rect = TileRect { x: 0, y: 0, width: shot.surface.width(), height: shot.surface.height() };
            let id = self.keep_drawn(tab, shot.page, shot.scale, rect, shot.surface, DrawKind::Preview, shot.paper, TileKind::Colour, None);
            // Of the version its sleep file says, if it says. Either way, a worker's word goes over it.
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.version = view.version;
            }
            self.from_sleep.insert(id);
            any = true;
        }
        if let Some((_, report)) = &mut self.waking
            && report.tab == tab
        {
            report.from_sleep = any;
        }
    }

    fn recount_wanted_files(&mut self, warm: &[u32]) {
        let mut files: Vec<PathBuf> = Vec::new();
        for tab in warm.iter().copied().chain(self.hot) {
            if let Some(doc) = self.docs.get(&tab) {
                files.extend(doc.map.files.iter().cloned());
            }
        }
        files.sort();
        files.dedup();
        if *self.wanted_files != files {
            self.wanted_files = Arc::new(files);
        }
    }

    /// Everything of tabs that have gone cold goes, as a cold tab holds nothing.
    fn keep_only(&mut self, warm: &[u32], hot: u32) {
        let gone: Vec<u32> = self.docs.keys().copied().filter(|tab| *tab != hot && !warm.contains(tab)).collect();
        for tab in gone {
            self.docs.remove(&tab);
            self.drop_tab_entries(tab);
        }
    }

    /// Zooms, wherever it's asked from. The host keeps the middle of the view where it is, and tiles stretch until new ones come, as with any zoom.
    pub fn zoom(&mut self, zoom: Zoom) -> Option<f64> {
        let doc = self.hot.and_then(|tab| self.docs.get_mut(&tab))?;
        let value = match zoom {
            Zoom::In => doc.magnification * ZOOM_STEP,
            Zoom::Out => doc.magnification / ZOOM_STEP,
            Zoom::Fit => 1.0,
            Zoom::Actual => 1.0 / doc.layout.fit().max(f64::MIN_POSITIVE),
            Zoom::To(value) => value,
            Zoom::Percent(percent) => percent / 100.0 / doc.layout.fit().max(f64::MIN_POSITIVE),
        };
        let value = if value.is_finite() { value.clamp(MIN_ZOOM, MAX_ZOOM) } else { doc.magnification };
        doc.magnification = value;
        self.ops.push_back(Op::Magnify { value });
        Some(value)
    }

    /// The tab showing's fit, if one is.
    pub fn fit(&self) -> Option<f64> {
        self.doc().map(|doc| doc.layout.fit())
    }

    /// Exactly where each tab it holds is, for the session to keep. Hot and warm tabs: a few at most, and a cold tab's is already kept.
    pub fn places(&self) -> Vec<(u32, SavedPlace)> {
        self.docs
            .values()
            .filter_map(|doc| {
                let page_size = doc.layout.usual_size();
                if self.hot == Some(doc.tab) {
                    let page = self.last_view_page?;
                    // Not yet arrived where it's been sent: what the view says is about somewhere else.
                    let (x, y) = if self.jump.is_some() { (doc.scroll_x, doc.exact_y.map_or_else(|| doc.layout.anchor_y(doc.anchor), |(y, _)| y)) } else { (self.viewport.x, self.viewport.y) };
                    Some((doc.tab, SavedPlace { page, anchor: doc.anchor, x, y, document_height: doc.layout.document_height(), magnification: doc.magnification, page_size }))
                } else {
                    let (page, anchor) = doc.left_at?;
                    let y = doc.exact_y.map_or_else(|| doc.layout.anchor_y(anchor), |(y, _)| y);
                    Some((doc.tab, SavedPlace { page, anchor, x: doc.scroll_x, y, document_height: doc.layout.document_height(), magnification: doc.magnification, page_size }))
                }
            })
            .collect()
    }

    /// Whether what's on the layers is the place on the hot tab, with nothing still on its way there: what makes it right to show a window.
    pub fn is_on_place(&self) -> bool {
        self.hot.is_some() && self.jump.is_none() && self.is_settled()
    }

    /// Which tabs are warm now, the hot one staying as it is. A tab that's stopped being warm, closed perhaps, holds nothing from here on, and its renderer goes.
    pub fn set_warm(&mut self, warm: &[u32]) {
        let Some(hot) = self.hot else { return };
        self.keep_only(warm, hot);
        self.recount_wanted_files(warm);
        self.cancel_if_unwanted();
    }

    /// No PDF is open: the home state.
    pub fn deactivate(&mut self) {
        self.clear_screen();
        let tabs: Vec<u32> = self.docs.keys().copied().collect();
        for tab in tabs {
            self.drop_tab_entries(tab);
        }
        self.docs.clear();
        self.hot = None;
        self.wanted_files = Arc::new(Vec::new());
        self.set_note(None);
        self.selecting.reset();
    }

    fn drop_tab_entries(&mut self, tab: u32) {
        let ids: Vec<u64> = self.entries.values().filter(|entry| entry.tab == tab).map(|entry| entry.id).collect();
        for id in ids {
            self.forget_entry(id);
        }
    }

    /// Takes everything off the screen, in one op.
    fn clear_screen(&mut self) {
        self.ops.push_back(Op::Clear);
        for ids in std::mem::take(&mut self.attached).into_values() {
            for id in ids {
                if let Some(entry) = self.entries.get_mut(&id) {
                    entry.attached = false;
                    self.to_discard.push(id);
                }
            }
        }
        self.shown.clear();
        self.selecting.layers_gone();
    }

    /// Lays the tab showing out another way: a page to a row, or two side by side. The reader stays on the page they were on, as far down it, and the zoom stays.
    pub fn set_spread(&mut self, spread: Spread) {
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else { return };
        if !doc.layout.set_spread(spread) {
            return;
        }
        // The height the view was at is about the old layout. Where it is in its page is what holds.
        doc.exact_y = None;
        self.lay_out();
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// The tab showing's layout.
    pub fn spread(&self) -> Option<Spread> {
        self.doc().map(|doc| doc.layout.spread())
    }

    /// Asks the host to go to a page, as an agent's `goto` or a contents row does.
    pub fn jump_to(&mut self, page: u32) {
        // A wake's over once someone's gone somewhere else: what's bare from here is pages being reached, not the wake.
        self.finish_wake();
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else { return };
        let page = page.min(doc.layout.count().saturating_sub(1));
        doc.anchor = (page, 0.0);
        let y = doc.layout.jump_y(page);
        // To the pixel while the layout holds: the first page's jump goes above it, to the margin, which its anchor can't say.
        doc.exact_y = Some((y, doc.layout.document_height()));
        let seq = self.next_seq;
        self.next_seq += 1;
        self.jump = Some(seq);
        self.last_view_page = Some(page);
        self.ops.push_back(Op::ScrollTo { y, seq });
    }

    /// How far down the tab's place is: to the pixel if the layout's what it was, and otherwise as far down its page.
    fn place_y(doc: &DocView) -> f64 {
        match doc.exact_y {
            Some((y, height)) if height == doc.layout.document_height() => y,
            _ => doc.layout.anchor_y(doc.anchor),
        }
    }

    /// Goes where the tab's anchor says, which is on this page, as a jump.
    fn jump_to_anchor(&mut self, page: u32) {
        let Some(doc) = self.hot.and_then(|tab| self.docs.get(&tab)) else { return };
        let y = Self::place_y(doc);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.jump = Some(seq);
        self.last_view_page = Some(page.min(doc.layout.count().saturating_sub(1)));
        self.ops.push_back(Op::ScrollTo { y, seq });
    }

    /// The document's size, then the place in it. Called when the fit or a page's size has changed.
    fn lay_out(&mut self) {
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else { return };
        doc.layout.set_margins(self.margins.0, self.margins.1);
        doc.layout.set_frame(self.viewport.frame_width);
        self.ops.push_back(Op::Document { width: doc.layout.document_width(), height: doc.layout.document_height() });
        let y = Self::place_y(doc);
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.jump.is_some() {
            self.jump = Some(seq);
        }
        self.ops.push_back(Op::ScrollTo { y, seq });
    }

    /// The host's view moved. Called on every scroll, on the UI thread: a few map lookups per visible tile, and at most [`MAX_ATTACH_PER_TURN`] surfaces reclaimed. What it returns is for the host to apply now.
    pub fn set_viewport(&mut self, viewport: Viewport) -> Vec<Op> {
        let was = self.viewport;
        self.viewport = viewport;
        self.counts.viewport_steps += 1;
        self.scrolled_to = viewport.scrolled_to.max(self.scrolled_to);
        // A new window's first word, before it's been told anything: the top of the document at no zoom. That's nobody's place.
        if !self.window_open {
            return self.take_ops();
        }
        // The reader's own scroll: the view moved down or up, with nothing the host was sent arriving, no change of zoom or room, and no pinch or resize under way.
        self.user_scrolled = !viewport.live && self.jump.is_none() && viewport.y != was.y && viewport.scrolled_to == was.scrolled_to && viewport.magnification == was.magnification && viewport.frame_width == was.frame_width && viewport.height == was.height;
        let mut moved_layout = false;
        if let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) {
            if doc.layout.set_frame(viewport.frame_width) && was.frame_width != viewport.frame_width {
                moved_layout = true;
            }
            // A jump under way says the host hasn't caught up with what it's been told, and that includes the zoom.
            if !viewport.live && self.jump.is_none() {
                doc.magnification = viewport.magnification;
            }
        }
        if moved_layout {
            self.lay_out();
        } else {
            self.track_place();
        }
        // After the refresh, which is what moves the tiles' scale on.
        self.refresh(MAX_ATTACH_PER_TURN);
        self.cancel_if_unwanted();
        self.count_blanks();
        self.take_ops()
    }

    /// Follows the reader's place. Only a change the reader made is the session's business: arriving where a jump sent them isn't.
    fn track_place(&mut self) {
        let viewport = self.viewport;
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else { return };
        let bottom = doc.layout.document_height() - viewport.height;
        let page = if viewport.y >= bottom - 1.0 && viewport.y > 0.0 { doc.layout.count().saturating_sub(1) } else { doc.layout.page_at(viewport.y + viewport.height / 3.0) };
        // Side by side, the reader's page stays whichever of its row they were on: a jump to a right-hand page, or reading on in a row, isn't a move to the left one.
        let page = match self.last_view_page {
            Some(was) if doc.layout.same_row(was, page) => was,
            _ => page,
        };
        if let Some(jump) = self.jump {
            // Still on its way: where the view is now isn't where it's going, and taken for the place, it would be where it ends up on the next layout. On arrival the place is left as it was sent, to the pixel.
            if self.scrolled_to >= jump {
                self.jump = None;
                // Where it arrived can be another page than was asked, as when a pane's narrower than the view the place was taken in, or a place deep in a page puts the next one where the reader's eye is. The session's page goes with it, or the two disagree, and the place isn't brought back after a restart.
                if self.last_view_page != Some(page) {
                    self.moved_page = Some(page);
                }
                self.last_view_page = Some(page);
            }
            return;
        }
        let moved = doc.exact_y.is_none_or(|(y, _)| y != viewport.y);
        if moved {
            doc.anchor = doc.layout.anchor_for(viewport.y, page);
            doc.exact_y = None;
        }
        doc.scroll_x = viewport.x;
        if self.last_view_page != Some(page) {
            self.last_view_page = Some(page);
            self.moved_page = Some(page);
        }
    }

    /// Whether what's running is still of use: on the tab showing, its page in view, and if it's a tile, at the zoom the view is at now and touching the view.
    pub fn running_is_wanted(&self) -> bool {
        let Some(running) = &self.running else { return true };
        if running.text {
            // Only a check goes for what's more wanted: it can take seconds, where reading is quick, and it's what's asked for when there's nothing better to do.
            return self.hot == Some(running.tab) && self.window_open && !(running.check && self.view_needs_something());
        }
        self.hot == Some(running.tab)
            && self.window_open
            && self.doc().is_some_and(|doc| {
                self.visible_pages(doc).contains(&running.position)
                    && (running.preview || running.rect.is_none_or(|rect| running.scale == doc.tile_scale && self.touches_view(doc, running.position, rect, running.scale)))
            })
    }

    /// Whether what's on screen wants something that isn't there yet: a tile, or the text under a selection being made.
    fn view_needs_something(&self) -> bool {
        let Some(doc) = self.doc() else { return false };
        let scale = doc.tile_scale;
        if scale <= 0.0 {
            return false;
        }
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        if self.selecting.wanted(Urgency::Now, &visible, doc.layout.count(), |page| doc.heavy.get(&page).copied().unwrap_or(false)).is_some() {
            return true;
        }
        visible.iter().any(|&page| {
            if doc.issues.contains_key(&page) {
                return false;
            }
            if !doc.layout.is_known(page) {
                return true;
            }
            let Ok(cells) = self.visible_cells(doc, page, scale) else { return false };
            cells.into_iter().any(|cell| !self.cover.contains_key(&self.key(doc, page, scale, cell)))
        })
    }

    fn cancel_if_unwanted(&mut self) {
        if self.running_is_wanted() {
            return;
        }
        let Some(running) = &mut self.running else { return };
        if !running.cancel_asked {
            running.cancel_asked = true;
            self.counts.cancels_asked += 1;
            if let Some(cancel) = &running.cancel {
                cancel.cancel_before(running.generation + 1);
            }
        }
    }

    fn visible_pages(&self, doc: &DocView) -> std::ops::RangeInclusive<u32> {
        if doc.layout.count() == 0 || self.viewport.height <= 0.0 {
            #[allow(clippy::reversed_empty_ranges)]
            return 1..=0;
        }
        doc.layout.pages_between(self.viewport.y.max(0.0), self.viewport.y.max(0.0) + self.viewport.height)
    }

    /// Whether a device rectangle of a page at a scale shows in the viewport, however the page is turned.
    fn touches_view(&self, doc: &DocView, page: u32, rect: TileRect, scale: f32) -> bool {
        let per_point = 1.0 / f64::from(scale);
        let (left, top) = (f64::from(rect.x) * per_point, f64::from(rect.y) * per_point);
        let (right, bottom) = (left + f64::from(rect.width) * per_point, top + f64::from(rect.height) * per_point);
        let corners = [(left, top), (right, top), (left, bottom), (right, bottom)].map(|(u, v)| doc.layout.from_page(page, u, v));
        let (x0, x1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), (x, _)| (low.min(*x), high.max(*x)));
        let (y0, y1) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), (_, y)| (low.min(*y), high.max(*y)));
        x1 > self.viewport.x && x0 < self.viewport.x + self.viewport.width && y1 > self.viewport.y && y0 < self.viewport.y + self.viewport.height
    }

    /// The scale tiles should be, for this view.
    fn target_scale(&self, doc: &DocView) -> f32 {
        let target = doc.layout.fit() * self.viewport.magnification * self.viewport.backing;
        (target as f32).clamp(MIN_SCALE, MAX_SCALE)
    }

    /// Which tiles of a page the view touches, nearest the middle first.
    fn visible_cells(&self, doc: &DocView, page: u32, scale: f32) -> Result<Vec<(u16, u16)>, ()> {
        let size = doc.layout.raw_size(page);
        let (width, height) = device_size(size.width, size.height, scale).map_err(|_| ())?;
        // The view's corners on the page as it is before it's turned, in device pixels at this scale.
        let (view_x, view_y, view_w, view_h) = (self.viewport.x, self.viewport.y, self.viewport.width, self.viewport.height);
        let corners = [(view_x, view_y), (view_x + view_w, view_y), (view_x, view_y + view_h), (view_x + view_w, view_y + view_h)].map(|(x, y)| doc.layout.to_page(page, x, y));
        let per_point = f64::from(scale);
        let (low_u, high_u) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), (u, _)| (low.min(*u), high.max(*u)));
        let (low_v, high_v) = corners.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), (_, v)| (low.min(*v), high.max(*v)));
        let left = (low_u * per_point).max(0.0);
        let top = (low_v * per_point).max(0.0);
        let right = (high_u * per_point).min(f64::from(width));
        let bottom = (high_v * per_point).min(f64::from(height));
        if right <= left || bottom <= top {
            return Ok(Vec::new());
        }
        let edge = f64::from(TILE_EDGE);
        let (first_column, last_column) = ((left / edge) as u32, ((right - 1e-6) / edge) as u32);
        let (first_row, last_row) = ((top / edge) as u32, ((bottom - 1e-6) / edge) as u32);
        let (middle_x, middle_y) = ((left + right) / 2.0, (top + bottom) / 2.0);
        let mut cells: Vec<(u16, u16)> = Vec::with_capacity(((last_column - first_column + 1) * (last_row - first_row + 1)) as usize);
        for row in first_row..=last_row {
            for column in first_column..=last_column {
                cells.push((u16::try_from(column).map_err(|_| ())?, u16::try_from(row).map_err(|_| ())?));
            }
        }
        let distance = |&(column, row): &(u16, u16)| {
            let x = (f64::from(column) + 0.5) * edge - middle_x;
            let y = (f64::from(row) + 0.5) * edge - middle_y;
            x * x + y * y
        };
        cells.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
        Ok(cells)
    }

    fn key(&self, doc: &DocView, page: u32, scale: f32, (column, row): (u16, u16)) -> TileKey {
        TileKey { tab: doc.tab, page, scale_bits: scale.to_bits(), column, row, paper: self.paper, preview: false }
    }

    fn preview_key(&self, doc: &DocView, page: u32) -> TileKey {
        TileKey { tab: doc.tab, page, scale_bits: 0, column: 0, row: 0, paper: self.paper, preview: true }
    }

    /// Brings the screen into line with what's wanted, attaching at most `budget` tiles. The rest wait for the next turn.
    fn refresh(&mut self, mut budget: usize) {
        self.more_to_attach = false;
        let Some(tab) = self.hot else { return };
        if !self.window_open {
            return;
        }
        let Some(doc) = self.docs.get(&tab) else { return };
        let target = self.target_scale(doc);
        let change = f64::from(target) / f64::from(doc.tile_scale.max(f32::MIN_POSITIVE));
        let retarget = doc.tile_scale == 0.0 || (!self.viewport.live && (change - 1.0).abs() > SCALE_SLACK);
        if retarget && let Some(doc) = self.docs.get_mut(&tab) {
            doc.tile_scale = target;
            doc.issues.retain(|_, issue| issue.try_again_at.is_some());
        }
        let Some(doc) = self.docs.get(&tab) else { return };
        let scale = doc.tile_scale;
        let visible: Vec<u32> = self.visible_pages(doc).filter(|page| doc.layout.is_known(*page)).collect();
        // A tab that's come from a sleep file knows what size its pages are likely to be, so with or without a picture its place is there on the paper from the first frame, and real sizes follow.
        let on_paper: Vec<u32> = if doc.size_on_trust { self.visible_pages(doc).collect() } else { visible.clone() };

        let stale: Vec<u32> = self.shown.keys().copied().filter(|page| !on_paper.contains(page)).collect();
        for page in stale {
            self.hide_page(page);
        }
        let Some(doc) = self.docs.get(&tab) else { return };
        let paper_rgb = self.paper.rgb();
        let device = self.viewport.magnification * self.viewport.backing;
        let mut placements = Vec::with_capacity(on_paper.len());
        for &page in &on_paper {
            let rect = doc.layout.rect(page);
            let size = doc.layout.raw_size(page);
            // On whole device pixels, so a tile's pixels land on the screen's.
            let snap = |value: f64| if device > 0.0 { (value * device).round() / device } else { value };
            placements.push((page, Placement { x: snap(rect.x), y: snap(rect.y), width: f64::from(size.width), height: f64::from(size.height), fit: doc.layout.fit(), paper_rgb, turns: doc.layout.turns_of(page) }));
        }
        for (page, placement) in placements {
            if self.shown.get(&page) != Some(&placement) {
                self.shown.insert(page, placement);
                self.ops.push_back(Op::ShowPage { page, x: placement.x, y: placement.y, width: placement.width, height: placement.height, fit: placement.fit, paper_rgb: placement.paper_rgb, turns: placement.turns });
            }
        }
        self.highlight_ops();
        // Every visible page shows something before any page gets more: a preview is one attachment and fills a page. Without this, a tall page's first eight tiles leave the next page blank for a turn.
        for &page in &visible {
            let Some(doc) = self.docs.get(&tab) else { return };
            let preview = self.preview_key(doc, page);
            let bare = !self.attached.get(&page).is_some_and(|ids| !ids.is_empty());
            if bare
                && let Some(&id) = self.cover.get(&preview)
                && self.attach(id, true)
            {
                budget = budget.saturating_sub(1);
            }
        }
        let now = Instant::now();
        for page in visible {
            let Some(doc) = self.docs.get(&tab) else { return };
            if doc.issues.contains_key(&page) && !doc.issues[&page].try_again_at.is_some_and(|at| now >= at) {
                continue;
            }
            let Ok(cells) = self.visible_cells(doc, page, scale) else { continue };
            let keys: Vec<TileKey> = cells.iter().map(|cell| self.key(doc, page, scale, *cell)).collect();
            let preview = self.preview_key(doc, page);
            let mut missing = false;
            let mut wanted = Vec::with_capacity(keys.len());
            for key in keys {
                let Some(&id) = self.cover.get(&key) else {
                    missing = true;
                    continue;
                };
                wanted.push(id);
                let attached = self.entries.get(&id).is_some_and(|entry| entry.attached);
                if !attached {
                    if budget == 0 {
                        self.more_to_attach = true;
                        missing = true;
                    } else if self.attach(id, false) {
                        budget -= 1;
                    } else {
                        // The OS took it. Drawn again.
                        missing = true;
                    }
                }
            }
            let preview_id = self.cover.get(&preview).copied();
            if missing {
                if let Some(id) = preview_id {
                    let attached = self.entries.get(&id).is_some_and(|entry| entry.attached);
                    if !attached && budget > 0 && self.attach(id, true) {
                        budget -= 1;
                    }
                }
            } else {
                let done: Vec<u64> = self.attached.get(&page).into_iter().flatten().copied().filter(|id| !wanted.contains(id)).collect();
                for id in done {
                    self.detach(id, page);
                }
            }
            // Tiles scrolled out of a tall page come off, even while others are still coming.
            if !missing {
                continue;
            }
            let Some(doc) = self.docs.get(&tab) else { return };
            let out: Vec<u64> = self.attached.get(&page).into_iter().flatten().copied().filter(|id| self.entries.get(id).is_some_and(|entry| !entry.preview && entry.scale == scale && entry.paper == self.paper && !self.touches_view(doc, page, entry.rect, scale))).collect();
            for id in out {
                self.detach(id, page);
            }
        }
        self.update_note();
    }

    fn update_note(&mut self) {
        let note = self.doc().and_then(|doc| {
            if let Some(problem) = &doc.problem {
                return Some(problem.clone());
            }
            for page in self.visible_pages(doc) {
                if let Some(issue) = doc.issues.get(&page) {
                    return Some(issue.message.clone());
                }
                let low = self.attached.get(&page).into_iter().flatten().filter_map(|id| self.entries.get(id)).find_map(|entry| entry.low_res.as_ref());
                if let Some(low) = low {
                    return Some(low.message.to_string());
                }
            }
            None
        });
        self.set_note(if self.gave_up { Some(CANT_DRAW.to_string()) } else { note });
    }

    fn set_note(&mut self, note: Option<String>) {
        self.note = note;
    }

    fn attach(&mut self, id: u64, below: bool) -> bool {
        self.clock += 1;
        let clock = self.clock;
        // Of another version of its file than the one there is now: it goes, and its page is drawn again.
        if let Some(entry) = self.entries.get(&id)
            && !self.is_current(entry)
        {
            self.forget_entry(id);
            return false;
        }
        let Some(entry) = self.entries.get_mut(&id) else { return false };
        if entry.discardable {
            let kept = entry.surface.as_ref().is_none_or(|surface| surface.reclaim() == Purge::Kept);
            entry.discardable = false;
            if !kept {
                self.counts.discards_lost += 1;
                self.forget_entry(id);
                return false;
            }
        }
        entry.attached = true;
        entry.last_used = clock;
        let (page, rect, scale, crop, rgb) = (entry.page, entry.rect, f64::from(entry.scale), entry.crop, entry.rgb);
        let surface = entry.surface.as_ref().map_or(0, PixelSurface::retained_for_host);
        self.attached.entry(page).or_default().push(id);
        // A tile the colour of the paper needs nothing drawn at all.
        if surface == 0 && rgb.is_none_or(|rgb| rgb == self.paper.rgb()) {
            return true;
        }
        self.ops.push_back(Op::Attach {
            id,
            page,
            x: f64::from(rect.x) / scale,
            y: f64::from(rect.y) / scale,
            width: f64::from(rect.width) / scale,
            height: f64::from(rect.height) / scale,
            surface,
            rgb,
            crop_width: crop.0,
            crop_height: crop.1,
            below,
        });
        true
    }

    fn detach(&mut self, id: u64, page: u32) {
        if let Some(ids) = self.attached.get_mut(&page) {
            ids.retain(|other| *other != id);
            if ids.is_empty() {
                self.attached.remove(&page);
            }
        }
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.attached = false;
            if entry.surface.is_some() || entry.rgb.is_some_and(|rgb| rgb != self.paper.rgb()) {
                self.ops.push_back(Op::Detach { id });
            }
            self.to_discard.push(id);
        }
    }

    fn hide_page(&mut self, page: u32) {
        self.ops.push_back(Op::HidePage { page });
        self.shown.remove(&page);
        self.selecting.page_hidden(page);
        for id in self.attached.remove(&page).unwrap_or_default() {
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.attached = false;
                self.to_discard.push(id);
            }
        }
    }

    /// Forgets an entry, and its surface with it.
    fn forget_entry(&mut self, id: u64) {
        self.from_sleep.remove(&id);
        let Some(entry) = self.entries.remove(&id) else { return };
        for key in Self::keys_of(&entry) {
            if self.cover.get(&key) == Some(&id) {
                self.cover.remove(&key);
            }
        }
        if entry.attached
            && let Some(ids) = self.attached.get_mut(&entry.page)
        {
            ids.retain(|other| *other != id);
            if entry.surface.is_some() {
                self.ops.push_back(Op::Detach { id });
            }
        }
    }

    fn keys_of(entry: &Entry) -> Vec<TileKey> {
        if entry.preview {
            return vec![TileKey { tab: entry.tab, page: entry.page, scale_bits: 0, column: 0, row: 0, paper: entry.paper, preview: true }];
        }
        let mut keys = Vec::new();
        for place in recto_proto::region_tiles(entry.rect, TILE_EDGE) {
            keys.push(TileKey { tab: entry.tab, page: entry.page, scale_bits: entry.scale.to_bits(), column: (place.x / TILE_EDGE) as u16, row: (place.y / TILE_EDGE) as u16, paper: entry.paper, preview: false });
        }
        keys
    }

    fn count_blanks(&mut self) {
        let Some(doc) = self.doc() else { return };
        let (mut blank, mut blank_seen) = (false, false);
        for page in self.visible_pages(doc) {
            let has_something = self.attached.get(&page).is_some_and(|ids| !ids.is_empty());
            if !has_something && !doc.issues.contains_key(&page) {
                blank = true;
                blank_seen |= doc.seen.contains(&page);
            }
        }
        self.counts.blank_steps += u64::from(blank);
        self.counts.blank_seen_steps += u64::from(blank_seen);
    }

    /// The ops for the host, attaching at most `budget` more first.
    pub fn drain(&mut self, budget: usize) -> Vec<Op> {
        self.refresh(budget);
        self.take_ops()
    }

    fn take_ops(&mut self) -> Vec<Op> {
        if !self.ops.is_empty() {
            self.unapplied = true;
        }
        self.handed_to_host.append(&mut self.to_discard);
        self.ops.drain(..).collect()
    }

    /// Whether the host should come back for more.
    pub fn has_ops(&self) -> bool {
        !self.ops.is_empty() || self.more_to_attach
    }

    /// The host has applied what it was given. What's now off every layer can be marked discardable, and it leaves the footprint.
    pub fn applied(&mut self) {
        self.unapplied = false;
        self.track_wake();
        for id in std::mem::take(&mut self.handed_to_host) {
            if let Some(entry) = self.entries.get_mut(&id)
                && !entry.attached
                && !entry.discardable
            {
                if let Some(surface) = &entry.surface {
                    surface.set_discardable();
                }
                entry.discardable = true;
            }
        }
        self.evict();
    }

    /// Least recently used first, with a page's preview outliving its tiles: it's a fraction of their size, and it's what keeps a page seen a while ago from coming back blank. Bounded, as the entries are, and a look along them only when something has just gone off a layer.
    fn evict(&mut self) {
        loop {
            let (mut tile_bytes, mut tile_surfaces, mut previews) = (0usize, 0usize, 0usize);
            let mut oldest_tile: Option<(u64, u64)> = None;
            let mut oldest_preview: Option<(u64, u64)> = None;
            for entry in self.entries.values().filter(|entry| !entry.attached) {
                let age = (entry.last_used, entry.id);
                if entry.preview {
                    previews += 1;
                    oldest_preview = Some(oldest_preview.map_or(age, |known| known.min(age)));
                } else {
                    tile_bytes += entry.surface.as_ref().map_or(0, PixelSurface::len);
                    tile_surfaces += usize::from(entry.surface.is_some());
                    oldest_tile = Some(oldest_tile.map_or(age, |known| known.min(age)));
                }
            }
            let share = self.share.max(1);
            let too_many_tiles = tile_bytes > MAX_KEPT_BYTES / share || tile_surfaces > MAX_KEPT_SURFACES / share || self.entries.len() > MAX_ENTRIES / share;
            let victim = if previews > MAX_KEPT_PREVIEWS / share { oldest_preview } else if too_many_tiles { oldest_tile.or(oldest_preview) } else { None };
            let Some((_, id)) = victim else { return };
            self.forget_entry(id);
        }
    }

    /// Sets the paper. Tiles for the other paper stay in memory as discardable, so going back is quick.
    pub fn set_paper(&mut self, paper: PaperKind) {
        if self.paper == paper {
            return;
        }
        self.paper = paper;
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// The window closed. Everything drawn goes, so a closed window costs nothing here. The tabs stay.
    pub fn window_closed(&mut self) {
        self.window_open = false;
        self.clear_screen();
        let ids: Vec<u64> = self.entries.keys().copied().collect();
        for id in ids {
            self.forget_entry(id);
        }
        // Nothing's there to take them. What was waiting is thrown away, and each `Attach` holds a reference to a surface, which goes with it.
        for op in self.ops.drain(..) {
            if let Op::Attach { surface, .. } = op {
                PixelSurface::release_for_host(surface);
            }
        }
        self.to_discard.clear();
        self.handed_to_host.clear();
        self.unapplied = false;
        self.more_to_attach = false;
        if let Some(running) = &self.running
            && let Some(cancel) = &running.cancel
        {
            cancel.cancel_before(running.generation + 1);
        }
        self.wanted_files = Arc::new(Vec::new());
        self.selecting.drop_texts();
    }

    /// The window is back: everything is asked for again.
    pub fn window_opened(&mut self) {
        self.window_open = true;
        let hot = self.hot;
        let warm: Vec<u32> = self.docs.keys().copied().filter(|tab| Some(*tab) != hot).collect();
        self.recount_wanted_files(&warm);
        // The window is new, at the top of the document and no zoom. It's sent back to the place and zoom this tab was left at, and until it's got there, what it says about where it is isn't the reader moving.
        if let Some(magnification) = self.doc().map(|doc| doc.magnification) {
            self.viewport.magnification = magnification;
            self.ops.push_back(Op::Magnify { value: magnification });
            self.jump = Some(u64::MAX);
        }
        self.lay_out();
        if let Some(across) = self.doc().map(|doc| doc.scroll_x) {
            self.ops.push_back(Op::ScrollAcross { x: across });
        }
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// The next thing to do, most wanted first, or `None`, when the engine parks. Only what's visible is drawn: drawing ahead comes in a later phase.
    pub fn next_plan(&mut self) -> Option<JobPlan> {
        if !self.window_open || self.hold_drawing {
            return None;
        }
        let tab = self.hot?;
        let doc = self.docs.get(&tab)?;
        if doc.problem.is_some() || self.viewport.height <= 0.0 {
            return None;
        }
        // Sizes first: nothing is shown or drawn before them. The first page known sets the fit, so where a reader lands is asked first, and then the pages around it.
        let visible = self.visible_pages(doc);
        let (first, last) = (*visible.start(), *visible.end());
        if first > last {
            return None;
        }
        let around = first.saturating_sub(SIZES_AROUND)..=(last + SIZES_AROUND).min(doc.layout.count().saturating_sub(1));
        // A page whose size couldn't be read has its reason on it, and isn't asked again: nothing about it will be different straight away.
        let waiting_on_issue = |page: &u32| doc.issues.get(page).is_some_and(|issue| !issue.try_again_at.is_some_and(|at| Instant::now() >= at));
        if let Some(page) = visible.clone().chain(around).find(|page| !doc.layout.is_known(*page) && !waiting_on_issue(page)) {
            let (file, source_page) = doc.map.source(page)?;
            let generation = self.fresh_generation();
            return Some(JobPlan { tab, position: page, file, source_page, generation, kind: PlanKind::Size });
        }
        let now = Instant::now();
        let scale = doc.tile_scale;
        if scale <= 0.0 {
            return None;
        }
        // A selection the reader is making comes before tiles: it answers their hand.
        if let Some(plan) = self.text_plan(tab, Urgency::Now) {
            return Some(plan);
        }
        let doc = self.docs.get(&tab)?;
        let pages: Vec<u32> = self.visible_pages(doc).collect();
        for page in pages {
            let doc = self.docs.get(&tab)?;
            if let Some(issue) = doc.issues.get(&page)
                && !issue.try_again_at.is_some_and(|at| now >= at)
            {
                continue;
            }
            let Ok(cells) = self.visible_cells(doc, page, scale) else {
                let size = doc.layout.raw_size(page);
                self.note_issue(tab, page, format!("Page {} is too large to draw at this zoom ({} by {} points).", page + 1, size.width, size.height), false);
                continue;
            };
            let missing: Vec<(u16, u16)> = cells
                .iter()
                .copied()
                .filter(|cell| {
                    let key = self.key(doc, page, scale, *cell);
                    match self.cover.get(&key).and_then(|id| self.entries.get(id)) {
                        None => true,
                        Some(entry) => entry.low_res.as_ref().is_some_and(|low| low.may_clear && now.duration_since(low.at) >= TRY_AGAIN_AFTER),
                    }
                })
                .collect();
            if missing.is_empty() {
                continue;
            }
            let (file, source_page) = doc.map.source(page)?;
            // Asked once, and it loads the page, which drawing would do anyway.
            if !doc.heavy.contains_key(&page) {
                let generation = self.fresh_generation();
                return Some(JobPlan { tab, position: page, file, source_page, generation, kind: PlanKind::Traits });
            }
            let heavy = doc.heavy.get(&page).copied().unwrap_or(false);
            // A heavy page's cost is per pass, so a preview would cost it as much again. Any other page gets one however little of it shows: a page first seen as a sliver at the bottom of the window, with no preview, goes blank when that sliver's tile scrolls off.
            let preview_wanted = !heavy && !self.cover.contains_key(&self.preview_key(doc, page));
            if preview_wanted && let Some((preview_scale, rect)) = self.preview_rect(doc, page, scale) {
                let paper = self.paper;
                let generation = self.fresh_generation();
                return Some(JobPlan { tab, position: page, file, source_page, generation, kind: PlanKind::Draw { scale: preview_scale, rect, what: DrawKind::Preview, paper } });
            }
            let size = doc.layout.raw_size(page);
            let (width, height) = device_size(size.width, size.height, scale).ok()?;
            if heavy && missing.len() > 1 {
                let wanted: Vec<(u32, u32)> = missing.iter().map(|&(column, row)| (u32::from(column), u32::from(row))).collect();
                let mut sorted = wanted;
                sorted.sort_unstable();
                let rect = plan_regions(&sorted, (width, height)).into_iter().next()?;
                let paper = self.paper;
                let generation = self.fresh_generation();
                return Some(JobPlan { tab, position: page, file, source_page, generation, kind: PlanKind::Draw { scale, rect, what: DrawKind::Region, paper } });
            }
            let (column, row) = missing[0];
            let (x, y) = (u32::from(column) * TILE_EDGE, u32::from(row) * TILE_EDGE);
            let rect = TileRect { x, y, width: TILE_EDGE.min(width - x), height: TILE_EDGE.min(height - y) };
            let paper = self.paper;
            let generation = self.fresh_generation();
            return Some(JobPlan { tab, position: page, file, source_page, generation, kind: PlanKind::Draw { scale, rect, what: DrawKind::Tile, paper } });
        }
        self.text_plan(tab, Urgency::WhenIdle)
    }

    /// Reading text for a selection, a copy or the pointer, if any is wanted at this urgency.
    fn text_plan(&mut self, tab: u32, urgency: Urgency) -> Option<JobPlan> {
        let doc = self.docs.get(&tab)?;
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        let wanted = self.selecting.wanted(urgency, &visible, doc.layout.count(), |page| doc.heavy.get(&page).copied().unwrap_or(false))?;
        let (file, source_page) = doc.map.source(wanted.page)?;
        let generation = self.fresh_generation();
        Some(JobPlan { tab, position: wanted.page, file, source_page, generation, kind: PlanKind::Text { check: wanted.check } })
    }

    /// Tells the host what's changed about highlights. A short look at nothing, with nothing selected.
    fn highlight_ops(&mut self) {
        if self.selecting.nothing_to_draw() {
            return;
        }
        let pages: Vec<u32> = self.shown.keys().copied().collect();
        for page in pages {
            if let Some(rects) = self.selecting.layer_update(page) {
                self.ops.push_back(Op::Highlight { page, rects });
            }
        }
    }

    /// Where a point in the document is on a page as it's shown. It stays with the page it's nearest, past the page's edges if it must, which is what a drag off the side needs.
    pub fn point_on_page(&self, x: f64, y: f64) -> Option<PagePoint> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let doc = self.doc()?;
        if doc.layout.count() == 0 {
            return None;
        }
        let page = doc.layout.page_at_point(x, y);
        let (u, v) = doc.layout.to_page(page, x, y);
        Some(PagePoint { page, x: u as f32, y: v as f32 })
    }

    /// The other way, for QA, which says where a click goes as an agent found it from `recto select`, and then does what the mouse does.
    pub fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)> {
        let doc = self.doc()?;
        if page >= doc.layout.count() {
            return None;
        }
        Some(doc.layout.from_page(page, x, y))
    }

    /// The mouse went down at a point in the document. Cheap: at worst it looks through a page already here.
    pub fn selection_press(&mut self, x: f64, y: f64, clicks: u32, shift: bool) {
        match self.point_on_page(x, y) {
            Some(point) => self.selecting.press(point, clicks, shift),
            None => self.selecting.clear(),
        }
        self.highlight_ops();
        self.cancel_if_unwanted();
    }

    pub fn selection_drag(&mut self, x: f64, y: f64) {
        if let Some(point) = self.point_on_page(x, y) {
            self.selecting.drag(point);
            self.highlight_ops();
            self.cancel_if_unwanted();
        }
    }

    pub fn selection_release(&mut self) {
        self.selecting.release();
    }

    /// Whether the pointer's on text, and so an I-beam. A page not read yet is wanted, for when there's nothing else to do.
    pub fn pointer_is_over_text(&mut self, x: f64, y: f64) -> bool {
        let Some(point) = self.point_on_page(x, y) else { return false };
        let Some(doc) = self.doc() else { return false };
        // Only on a page: the margin and the gap between pages are neither.
        let size = doc.layout.raw_size(point.page);
        if point.x < 0.0 || point.y < 0.0 || point.x > size.width || point.y > size.height {
            return false;
        }
        self.selecting.is_over_text(point)
    }

    /// The whole document, as a Mac PDF reader takes ⌘A. It reads nothing until something's showing.
    pub fn select_all(&mut self) {
        let Some(pages) = self.doc().map(|doc| doc.layout.count()).filter(|pages| *pages > 0) else { return };
        self.selecting.set_range(Range::all(pages));
        self.highlight_ops();
    }

    pub fn clear_selection(&mut self) {
        self.selecting.clear();
        self.highlight_ops();
    }

    /// What's selected on the tab showing, for the state record.
    pub fn selection_range(&self) -> Option<recto_index::select::Range> {
        self.selecting.range()
    }

    /// Puts back what was selected, from the state record. It's held to the document's pages, as a record can say anything.
    pub fn restore_selection(&mut self, range: recto_index::select::Range) {
        let pages = self.doc().map_or(0, |doc| doc.layout.count());
        if range.start.page < pages && range.end.page < pages && (range.start.page, range.start.at) <= (range.end.page, range.end.at) {
            self.selecting.set_range(range);
            self.highlight_ops();
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selecting.has_selection()
    }

    pub fn has_document(&self) -> bool {
        self.doc().is_some_and(|doc| doc.layout.count() > 0)
    }

    fn heavy_pages(&self) -> HashMap<u32, bool> {
        self.doc().map(|doc| doc.heavy.clone()).unwrap_or_default()
    }

    /// Starts the selection on its way to the clipboard, a page at a time, off this thread. `anyway` is the reader having said so.
    pub fn start_copy(&mut self, anyway: bool) {
        let Some(pages) = self.doc().map(|doc| doc.layout.count()) else { return };
        let heavy = self.heavy_pages();
        self.selecting.start_copy(anyway, pages, |page| heavy.get(&page).copied().unwrap_or(false));
    }

    pub fn take_copy_signal(&mut self) -> bool {
        self.selecting.take_copy_signal()
    }

    pub fn take_copied(&mut self) -> Option<recto_index::select::CopiedText> {
        self.selecting.take_copied()
    }

    /// The selection's text if it's all here, for what can't wait for it.
    pub fn selection_text_now(&self) -> Option<String> {
        self.selecting.text_now(self.doc()?.layout.count())
    }

    pub fn selection_can_be_sent(&self) -> bool {
        self.doc().is_some_and(|doc| self.selecting.can_offer(doc.layout.count()))
    }

    pub fn selection_notice(&self) -> Option<Notice> {
        self.selecting.notice().cloned()
    }

    pub fn dismiss_selection_notice(&mut self) {
        self.selecting.dismiss_notice();
    }

    /// Text arrived, or couldn't be read.
    fn take_text(&mut self, tab: u32, position: u32, outcome: Option<(Arc<recto_index::select::PageText>, bool)>) {
        if self.hot != Some(tab) || !self.window_open {
            return;
        }
        let visible: Vec<u32> = self.doc().map(|doc| self.visible_pages(doc).collect()).unwrap_or_default();
        match outcome {
            Some((text, checked)) => self.selecting.page_read(position, text, checked, &visible),
            None => self.selecting.page_failed(position, &visible),
        }
        self.advance_copy();
    }

    fn advance_copy(&mut self) {
        let pages = self.doc().map_or(0, |doc| doc.layout.count());
        let heavy = self.heavy_pages();
        self.selecting.advance_copy(pages, |page| heavy.get(&page).copied().unwrap_or(false));
    }

    /// The file behind a tab has been seen to be another one, so places on its pages mean nothing now. It comes before the job's outcome, which may be about it.
    pub fn file_changed(&mut self, tab: u32) {
        if self.hot == Some(tab) {
            self.selecting.file_changed();
            self.highlight_ops();
        }
    }

    /// The thread that draws panicked and is starting over, or, with `give_up`, isn't. What it was doing is gone, and everything it would come back to is asked for again.
    pub fn thread_restarted(&mut self, give_up: bool) {
        self.running = None;
        self.selecting.thread_restarted();
        self.gave_up |= give_up;
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    fn preview_rect(&self, doc: &DocView, page: u32, scale: f32) -> Option<(f32, TileRect)> {
        let size = doc.layout.raw_size(page);
        let preview_scale = ((doc.layout.fit() * self.viewport.backing / PREVIEW_DIVISOR) as f32).clamp(MIN_SCALE, MAX_SCALE);
        // A zoom already that low has nothing to gain from it.
        if f64::from(preview_scale) * 1.5 >= f64::from(scale) {
            return None;
        }
        let (width, height) = device_size(size.width, size.height, preview_scale).ok()?;
        (u64::from(width) * u64::from(height) * u64::from(BYTES_PER_PIXEL) <= MAX_PREVIEW_BYTES).then_some((preview_scale, TileRect { x: 0, y: 0, width, height }))
    }

    fn fresh_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        generation
    }

    /// A job has started. Its cancel handle lets the UI thread stop it the moment it stops being wanted.
    pub fn note_started(&mut self, job: &Job, cancel: Option<CancelHandle>) {
        let (rect, scale, preview) = match &job.kind {
            JobKind::Draw { scale, rect, what, .. } => (Some(*rect), *scale, *what == DrawKind::Preview),
            _ => (None, 0.0, false),
        };
        let (text, check) = match job.kind {
            JobKind::Text { check } => (true, check),
            _ => (false, false),
        };
        self.running = Some(Running { tab: job.tab, position: job.position, rect, scale, preview, generation: job.generation, cancel, cancel_asked: false, text, check });
    }

    /// What can stop the job now running, once there's something that can.
    pub fn set_cancel(&mut self, cancel: CancelHandle) {
        if let Some(running) = &mut self.running {
            running.cancel = Some(cancel);
        }
    }

    fn note_issue(&mut self, tab: u32, page: u32, message: String, may_pass: bool) {
        if let Some(doc) = self.docs.get_mut(&tab) {
            doc.issues.insert(page, Issue { message, try_again_at: may_pass.then(|| Instant::now() + TRY_AGAIN_AFTER) });
        }
    }

    /// A job has ended, with whatever it found. Returns anything it hands back.
    pub fn finish(&mut self, job: Job, outcome: Outcome) {
        self.running = None;
        self.counts.jobs += 1;
        let mut kept = None;
        let Job { tab, position, kind, generation, .. } = job;
        // Asked about the pages as they were. An edit has changed what's at this position since.
        if self.docs.get(&tab).is_some_and(|doc| generation < doc.floor) {
            self.counts.cancelled += 1;
            self.refresh(MAX_ATTACH_PER_TURN);
            return;
        }
        match (kind, outcome) {
            (JobKind::Text { .. }, Outcome::Text { text, checked }) => self.take_text(tab, position, Some((text, checked))),
            // Nobody wanted it any more. It'll be asked for again if that changes.
            (JobKind::Text { .. }, Outcome::Cancelled) => self.counts.cancelled += 1,
            (JobKind::Text { .. }, _) => self.take_text(tab, position, None),
            (JobKind::Size, Outcome::Size(size)) => {
                let Some(doc) = self.docs.get_mut(&tab) else { return };
                let moved = doc.layout.set_page_size(position, size);
                if moved && self.hot == Some(tab) {
                    self.lay_out();
                }
            }
            (JobKind::Traits, Outcome::Heavy(heavy)) => {
                if let Some(doc) = self.docs.get_mut(&tab) {
                    if doc.heavy.len() >= MAX_HEAVY_REMEMBERED {
                        doc.heavy.clear();
                    }
                    doc.heavy.insert(position, heavy);
                }
            }
            (JobKind::Draw { scale, rect, surface, what, paper }, Outcome::Drawn { kind, low_res }) => {
                self.counts.drawn += 1;
                if self.docs.contains_key(&tab) && self.window_open {
                    kept = Some(self.keep_drawn(tab, position, scale, rect, surface, what, paper, kind, low_res));
                }
            }
            (JobKind::Draw { .. }, Outcome::Cancelled) => {
                self.counts.cancelled += 1;
            }
            (_, Outcome::PageFailed { message, may_pass }) => {
                self.note_issue(tab, position, message, may_pass);
            }
            (_, Outcome::DocumentFailed(message)) => {
                if let Some(doc) = self.docs.get_mut(&tab) {
                    doc.problem = Some(message);
                }
            }
            _ => {
                self.counts.cancelled += 1;
            }
        }
        self.refresh(MAX_ATTACH_PER_TURN);
        // It came too late for the view, or for another zoom, paper or a warm tab. Nothing's showing it, so it counts for nothing: drawn is no reason to hold memory.
        if let Some(id) = kept
            && let Some(entry) = self.entries.get_mut(&id)
            && !entry.attached
            && !entry.discardable
        {
            if let Some(surface) = &entry.surface {
                surface.set_discardable();
            }
            entry.discardable = true;
            self.evict();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn keep_drawn(&mut self, tab: u32, page: u32, scale: f32, rect: TileRect, surface: PixelSurface, what: DrawKind, paper: PaperKind, kind: TileKind, low_res: Option<(TileRect, LowResWhy)>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let preview = what == DrawKind::Preview;
        let drawn_rect = low_res.map_or(rect, |(drawn, _)| drawn);
        let crop = (f64::from(drawn_rect.width) / f64::from(surface.width().max(1)), f64::from(drawn_rect.height) / f64::from(surface.height().max(1)));
        // A tile of one colour needs no surface, unless it's a whole region's or the only sign of a page.
        let (rgb, surface) = match (kind, what) {
            (TileKind::Blank { rgb }, DrawKind::Tile) if low_res.is_none() => (Some(rgb), None),
            _ => (None, Some(surface)),
        };
        let note = low_res.map(|(_, why)| LowResNote { message: why.message(), may_clear: why.may_clear(), at: Instant::now() });
        self.clock += 1;
        let version = self.version_of_page(tab, page);
        let entry = Entry { id, tab, page, scale, paper, preview, rect, surface, rgb, crop, low_res: note, version, attached: false, discardable: false, last_used: self.clock };
        for key in Self::keys_of(&entry) {
            if let Some(old) = self.cover.insert(key, id)
                && old != id
            {
                self.retire_replaced(old, tab);
            }
        }
        self.entries.insert(id, entry);
        if let Some(doc) = self.docs.get_mut(&tab) {
            doc.seen.insert(page);
        }
        id
    }

    /// Something drawn over an older copy of the same place. The older one comes off when no key points at it.
    fn retire_replaced(&mut self, old: u64, tab: u32) {
        let Some(entry) = self.entries.get(&old) else { return };
        let page = entry.page;
        let still_used = Self::keys_of(entry).iter().any(|key| self.cover.get(key) == Some(&old));
        if !still_used {
            if entry.attached && self.hot == Some(tab) {
                self.detach(old, page);
            }
            if let Some(entry) = self.entries.get(&old)
                && !entry.attached
            {
                self.forget_entry(old);
            }
        }
    }

    /// Whether everything visible is drawn and on its layers, or marked as it is.
    pub fn is_settled(&self) -> bool {
        if self.gave_up {
            return self.ops.is_empty() && !self.unapplied;
        }
        if self.running.is_some() || !self.ops.is_empty() || self.unapplied || self.more_to_attach {
            return false;
        }
        let Some(doc) = self.doc() else { return true };
        if doc.problem.is_some() {
            return true;
        }
        if !self.window_open {
            return true;
        }
        if self.viewport.height <= 0.0 {
            return false;
        }
        let scale = doc.tile_scale;
        if scale <= 0.0 {
            return false;
        }
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        if !self.selecting.is_settled(&visible, doc.layout.count(), |page| doc.heavy.get(&page).copied().unwrap_or(false)) {
            return false;
        }
        for page in self.visible_pages(doc) {
            if doc.issues.contains_key(&page) {
                continue;
            }
            if !doc.layout.is_known(page) {
                return false;
            }
            let Ok(cells) = self.visible_cells(doc, page, scale) else { continue };
            for cell in cells {
                let key = self.key(doc, page, scale, cell);
                if !self.cover.get(&key).and_then(|id| self.entries.get(id)).is_some_and(|entry| entry.attached) {
                    return false;
                }
            }
        }
        true
    }

    /// What QA reads, and the budgets. Numbers and states: no text from a document.
    pub fn report(&self) -> ReaderReport {
        let doc = self.doc();
        let attached = self.attached.values().map(Vec::len).sum::<usize>();
        let attached_bytes: usize = self.attached.values().flatten().filter_map(|id| self.entries.get(id)).filter_map(|entry| entry.surface.as_ref()).map(PixelSurface::len).sum();
        let previews_attached = self.attached.values().flatten().filter_map(|id| self.entries.get(id)).filter(|entry| entry.preview).count();
        let low_res_pages: HashSet<u32> = self.attached.iter().filter(|(_, ids)| ids.iter().filter_map(|id| self.entries.get(id)).any(|entry| entry.low_res.is_some())).map(|(page, _)| *page).collect();
        let mut low_res_pages: Vec<u32> = low_res_pages.into_iter().map(|page| page + 1).collect();
        low_res_pages.sort_unstable();
        let visible: Vec<u32> = doc.map(|doc| self.visible_pages(doc).map(|page| page + 1).collect()).unwrap_or_default();
        ReaderReport(json!({
            "tab": self.hot,
            "pages": doc.map(|doc| doc.layout.count()),
            "visible_pages": visible,
            "page": self.last_view_page.map(|page| page + 1),
            "magnification": self.viewport.magnification,
            "fit": doc.map(|doc| doc.layout.fit()),
            "spread": doc.map(|doc| format!("{:?}", doc.layout.spread())),
            "tile_scale": doc.map(|doc| doc.tile_scale),
            "document_height": doc.map(|doc| doc.layout.document_height()),
            "scroll_y": self.viewport.y,
            "scroll_x": self.viewport.x,
            "paper": match self.paper { PaperKind::White => "white", PaperKind::Dark => "dark" },
            "settled": self.is_settled(),
            "note": self.note,
            "low_res_pages": low_res_pages,
            "tiles": {
                "attached": attached,
                "attached_bytes": attached_bytes,
                "previews_attached": previews_attached,
                "kept": self.entries.len() - attached.min(self.entries.len()),
                "kept_bytes": self.entries.values().filter(|entry| !entry.attached).filter_map(|entry| entry.surface.as_ref()).map(PixelSurface::len).sum::<usize>(),
                "page_layers": self.shown.len(),
            },
            "counts": {
                "jobs": self.counts.jobs,
                "drawn": self.counts.drawn,
                "cancelled": self.counts.cancelled,
                "viewport_steps": self.counts.viewport_steps,
                "blank_steps": self.counts.blank_steps,
                "blank_seen_steps": self.counts.blank_seen_steps,
                "discards_lost": self.counts.discards_lost,
            },
            "documents_held": self.docs.len(),
            "last_wake": self.last_wake().map(|wake| json!({ "tab": wake.tab, "from_sleep": wake.from_sleep, "first_frame_us": wake.first_frame_us, "sharp_us": wake.sharp_us, "blank_frames": wake.blank_frames })),
            "window_open": self.window_open,
            "selection": self.selecting.report(),
        }))
    }

    pub fn counts(&self) -> &ReaderCounts {
        &self.counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use crate::layout::PageBox;

    fn map() -> PageMap {
        PageMap { files: vec![PathBuf::from("/d/a.pdf")], slots: None, own: None, turns: None }
    }

    impl Reader {
        /// The next job with its surface made, as the engine has it.
        fn next_job(&mut self) -> Option<Job> {
            self.next_plan()?.ready()
        }
    }

    fn view(y: f64) -> Viewport {
        Viewport { x: 0.0, y, width: 1168.0, height: 900.0, frame_width: 1168.0, magnification: 1.0, backing: 2.0, live: false, scrolled_to: u64::MAX }
    }

    fn letter() -> PageBox {
        PageBox { width: 612.0, height: 792.0 }
    }

    /// Runs the job queue with a pretend renderer until there's nothing left to do, and applies what the host would.
    fn settle(reader: &mut Reader) -> Vec<Job> {
        let mut drawn = Vec::new();
        for _ in 0..10_000 {
            reader.take_ops();
            reader.applied();
            let Some(job) = reader.next_job() else { break };
            let outcome = match &job.kind {
                JobKind::Size => Outcome::Size(letter()),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => Outcome::Heavy(false),
                JobKind::Draw { .. } => Outcome::Drawn { kind: TileKind::Colour, low_res: None },
            };
            drawn.push(Job { tab: job.tab, position: job.position, file: job.file.clone(), source_page: job.source_page, generation: job.generation, kind: match &job.kind { JobKind::Size => JobKind::Size, JobKind::Traits => JobKind::Traits, JobKind::Text { check } => JobKind::Text { check: *check }, JobKind::Draw { scale, rect, surface, what, paper } => JobKind::Draw { scale: *scale, rect: *rect, surface: surface.clone(), what: *what, paper: *paper } } });
            reader.finish(job, outcome);
            let more = reader.drain(MAX_ATTACH_PER_TURN);
            drop(more);
            reader.applied();
        }
        for _ in 0..100 {
            if !reader.has_ops() {
                break;
            }
            reader.drain(MAX_ATTACH_PER_TURN);
            reader.applied();
        }
        drawn
    }

    fn reader_on(pages: u32) -> Reader {
        let mut reader = Reader::new();
        reader.set_viewport(view(0.0));
        reader.activate(1, pages, 0, map(), &[], None, Spread::Single);
        reader.set_viewport(view(0.0));
        reader
    }

    /// Failure: a tile for the wrong place, or none at all, reaches the screen, because the grid and the view don't agree. Every visible cell must be covered when it says it's settled, and every attached tile must be in view.
    #[test]
    fn it_settles_with_what_is_visible_on_layers() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        assert!(reader.is_settled(), "{}", reader.report().0);
        let report = reader.report().0;
        assert!(report["tiles"]["attached"].as_u64().unwrap() >= 1);
        assert_eq!(report["visible_pages"][0], 1);
    }

    /// Failure: scrolling through a long document keeps every tile on a layer, so memory grows with what's been read. Off-screen tiles must leave the layers and go discardable. Measured in surfaces, by the count on layers.
    #[test]
    fn scrolling_keeps_only_whats_visible_on_layers() {
        let mut reader = reader_on(400);
        settle(&mut reader);
        let first = reader.report().0["tiles"]["attached"].as_u64().unwrap();
        for step in 1..60 {
            reader.set_viewport(view(f64::from(step) * 900.0));
            settle(&mut reader);
        }
        let later = reader.report().0;
        assert!(later["tiles"]["attached"].as_u64().unwrap() <= first * 2, "{later}");
        assert!(later["tiles"]["kept_bytes"].as_u64().unwrap() > 0, "off-screen tiles stay cached");
        assert!(later["tiles"]["page_layers"].as_u64().unwrap() <= 3, "{later}");
    }

    /// Failure: coming back to a page drawn a moment ago shows nothing and draws it again, so a seen page goes blank. Holds the budget's "0 blank frames on seen pages", and proves the cache is really used.
    #[test]
    fn a_page_seen_before_comes_back_without_being_drawn() {
        let mut reader = reader_on(100);
        settle(&mut reader);
        reader.set_viewport(view(5000.0));
        settle(&mut reader);
        let before = reader.counts().drawn;
        let ops = reader.set_viewport(view(0.0));
        assert!(ops.iter().any(|op| matches!(op, Op::Attach { .. })), "something shows at once");
        assert_eq!(reader.counts().blank_seen_steps, 0);
        settle(&mut reader);
        assert_eq!(reader.counts().drawn, before, "nothing needed drawing again");
    }

    /// Failure: coming back to two pages at once, the first takes the whole turn's attachments, and the second is blank until the next turn. Measured on `qa sweep`: 32 such steps in 5 s at 40 pages a second.
    #[test]
    fn every_visible_page_shows_something_in_the_first_turn() {
        let mut reader = reader_on(100);
        let tall = |y: f64| Viewport { height: 1700.0, ..view(y) };
        reader.set_viewport(tall(0.0));
        settle(&mut reader);
        reader.set_viewport(tall(30_000.0));
        settle(&mut reader);
        let ops = reader.set_viewport(tall(0.0));
        let pages: HashSet<u32> = ops.iter().filter_map(|op| if let Op::Attach { page, .. } = op { Some(*page) } else { None }).collect();
        let visible = reader.report().0["visible_pages"].as_array().unwrap().len();
        assert!(visible >= 2, "this needs two pages in view");
        assert_eq!(pages.len(), visible, "{ops:?}");
        assert_eq!(reader.counts().blank_seen_steps, 0);
    }

    /// Failure: reading on through a long document pushes out what it just read, and a page's preview goes with its tiles, so coming back to it shows nothing at all. Measured at 40 pages a second on `rfc9110`: 84 blank steps on seen pages, all from this. Past the tile cap, tiles go and previews stay.
    #[test]
    fn previews_outlive_their_tiles() {
        let mut reader = reader_on(600);
        let step = reader.doc().map(|doc| doc.layout.top(1)).unwrap();
        settle(&mut reader);
        // Enough pages at 12 tiles each to pass the tile cap by a long way, and few enough to stay under the preview cap.
        for page in 1..60 {
            reader.set_viewport(view(f64::from(page) * step));
            settle(&mut reader);
        }
        let report = reader.report().0;
        assert!(report["tiles"]["kept_bytes"].as_u64().unwrap() <= (MAX_KEPT_BYTES + MAX_KEPT_PREVIEWS * 1024 * 1024) as u64, "{report}");
        let doc = reader.doc().unwrap();
        let key = reader.preview_key(doc, 0);
        assert!(reader.cover.contains_key(&key), "the first page's preview is still here");
        let ops = reader.set_viewport(view(0.0));
        assert!(ops.iter().any(|op| matches!(op, Op::Attach { below: true, .. })), "and shows at once: {ops:?}");
        assert_eq!(reader.counts().blank_seen_steps, 0);
    }

    /// Failure: a burst of attachments stalls the UI thread, or the leftovers are forgotten and part of a page stays blank until something else happens.
    #[test]
    fn at_most_eight_tiles_are_attached_in_a_turn_and_the_rest_follow() {
        let mut reader = reader_on(100);
        let big = Viewport { width: 4000.0, height: 3000.0, frame_width: 1168.0, ..view(0.0) };
        reader.set_viewport(big);
        settle(&mut reader);
        reader.set_viewport(view(40_000.0));
        settle(&mut reader);
        let ops = reader.set_viewport(big);
        reader.applied();
        let attaches = |ops: &[Op]| ops.iter().filter(|op| matches!(op, Op::Attach { .. })).count();
        assert_eq!(attaches(&ops), MAX_ATTACH_PER_TURN, "a full turn, or this tests nothing");
        assert!(reader.has_ops(), "and it says there's more");
        let mut turns = 0;
        while reader.has_ops() {
            let more = reader.drain(MAX_ATTACH_PER_TURN);
            reader.applied();
            assert!(attaches(&more) <= MAX_ATTACH_PER_TURN);
            turns += 1;
            assert!(turns < 100, "the leftovers never end");
        }
        assert!(turns >= 1);
        assert!(reader.is_settled(), "the leftovers all came: {}", reader.report().0);
    }

    /// Failure: a tile from before a zoom stays on screen once its replacements are all there, or the old tiles go before the new ones arrive, leaving a blank.
    #[test]
    fn a_zoom_keeps_the_old_tiles_until_the_new_ones_cover_them() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        let before = reader.report().0["tile_scale"].as_f64().unwrap();
        let ops = reader.set_viewport(Viewport { magnification: 1.5, ..view(0.0) });
        assert!(!ops.iter().any(|op| matches!(op, Op::Detach { .. } | Op::HidePage { .. })), "nothing comes off yet: {ops:?}");
        settle(&mut reader);
        let after = reader.report().0;
        assert!(after["tile_scale"].as_f64().unwrap() > before);
        assert!(reader.is_settled());
        let attached = after["tiles"]["attached"].as_u64().unwrap();
        let scale = after["tile_scale"].as_f64().unwrap() as f32;
        let exact = reader.attached.values().flatten().filter_map(|id| reader.entries.get(id)).filter(|entry| entry.scale == scale).count() as u64;
        assert_eq!(attached, exact, "only exact tiles left");
    }

    /// Failure: a pinch draws at every scale it passes through, so it stutters and throws work away.
    #[test]
    fn nothing_is_redrawn_during_a_pinch() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        let scale = reader.report().0["tile_scale"].clone();
        reader.set_viewport(Viewport { magnification: 1.7, live: true, ..view(0.0) });
        assert_eq!(reader.report().0["tile_scale"], scale);
        assert!(reader.next_job().is_none());
    }

    fn a_draw_job(reader: &mut Reader, what: DrawKind) -> Job {
        loop {
            let job = reader.next_job().expect("a job");
            match &job.kind {
                JobKind::Size => reader.finish(job, Outcome::Size(letter())),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
                JobKind::Draw { what: kind, .. } if *kind == what => return job,
                JobKind::Draw { .. } => reader.finish(job, Outcome::Drawn { kind: TileKind::Colour, low_res: None }),
            }
        }
    }

    /// Failure: a job for a place the reader has left runs to the end, so a fast scroll on a heavy page queues seconds of work. It's told to stop, once, and not while it's still in view.
    #[test]
    fn a_job_nobody_wants_any_more_is_cancelled() {
        let mut reader = reader_on(100);
        let job = a_draw_job(&mut reader, DrawKind::Tile);
        reader.note_started(&job, None);
        reader.set_viewport(view(0.0));
        assert_eq!(reader.counts().cancels_asked, 0, "still in view");
        reader.set_viewport(view(40_000.0));
        assert_eq!(reader.counts().cancels_asked, 1, "the reader left");
        reader.set_viewport(view(41_000.0));
        assert_eq!(reader.counts().cancels_asked, 1, "told once");
        reader.finish(job, Outcome::Cancelled);
    }

    /// Failure: a tile being drawn for the zoom before this one runs to the end, on a heavy page for seconds, since its scale not being the view's used to count as wanted, to spare previews. It stops, and a preview, whose scale never is the view's, doesn't.
    #[test]
    fn a_job_for_an_old_zoom_is_cancelled_and_a_preview_is_not() {
        let mut reader = reader_on(100);
        let job = a_draw_job(&mut reader, DrawKind::Tile);
        reader.note_started(&job, None);
        reader.set_viewport(Viewport { magnification: 1.5, ..view(0.0) });
        assert_eq!(reader.counts().cancels_asked, 1, "the zoom moved on");
        reader.finish(job, Outcome::Cancelled);

        let mut reader = reader_on(100);
        let job = a_draw_job(&mut reader, DrawKind::Preview);
        reader.note_started(&job, None);
        reader.set_viewport(Viewport { magnification: 1.5, ..view(0.0) });
        assert_eq!(reader.counts().cancels_asked, 0, "its page is still in view");
        reader.finish(job, Outcome::Cancelled);
    }

    /// Failure: a cold tab keeps its tiles, layout or renderer's file. Budget: 0 tiles and 0 handles for a cold tab.
    #[test]
    fn a_tab_gone_cold_holds_nothing() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        assert!(!reader.entries.is_empty());
        reader.activate(2, 10, 0, PageMap { files: vec![PathBuf::from("/d/b.pdf")], slots: None, own: None, turns: None }, &[], None, Spread::Single);
        assert!(reader.entries.values().all(|entry| entry.tab != 1), "tab 1 isn't warm");
        assert!(!reader.docs.contains_key(&1));
        assert_eq!(*reader.wanted_files(), vec![PathBuf::from("/d/b.pdf")]);
        reader.activate(3, 10, 0, PageMap { files: vec![PathBuf::from("/d/c.pdf")], slots: None, own: None, turns: None }, &[2], None, Spread::Single);
        assert_eq!(reader.wanted_files().len(), 2, "a warm tab's file is kept");
    }

    /// Failure: the window closed still holds tiles, so a closed window stays over budget.
    #[test]
    fn a_closed_window_holds_no_tiles_and_comes_back() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        reader.window_closed();
        assert!(reader.entries.is_empty());
        assert!(reader.next_job().is_none(), "nothing draws for a closed window");
        assert!(reader.wanted_files().is_empty(), "the renderer goes too");
        reader.window_opened();
        reader.set_viewport(view(0.0));
        settle(&mut reader);
        assert!(reader.is_settled());
        assert!(!reader.entries.is_empty());
    }

    /// Failure: dark paper shows white tiles, or the paper's tiles are thrown away on a change so going back draws everything again.
    #[test]
    fn a_change_of_paper_draws_for_that_paper_and_keeps_the_other() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        let before = reader.counts().drawn;
        reader.set_paper(PaperKind::Dark);
        let drawn = settle(&mut reader);
        assert!(drawn.iter().any(|job| matches!(job.kind, JobKind::Draw { paper: PaperKind::Dark, .. })));
        assert!(reader.counts().drawn > before);
        let dark = reader.counts().drawn;
        reader.set_paper(PaperKind::White);
        settle(&mut reader);
        assert_eq!(reader.counts().drawn, dark, "back again with nothing to draw");
    }

    /// Failure: an agent's `goto` to a page reads as the reader having scrolled there and then scrolled back, so the session's place fights the jump. Arriving isn't a move, and a real scroll afterwards is.
    #[test]
    fn a_jump_isnt_reported_back_as_a_move() {
        let mut reader = reader_on(100);
        reader.scrolled_to = 0;
        settle(&mut reader);
        let mut base = view(0.0);
        base.scrolled_to = 0;
        reader.set_viewport(base);
        reader.take_moved_page();
        reader.jump_to(50);
        let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().find(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("no scroll") };
        let mut there = view(y);
        there.scrolled_to = seq - 1;
        reader.set_viewport(there);
        assert_eq!(reader.take_moved_page(), None, "not applied yet");
        there.scrolled_to = seq;
        reader.set_viewport(there);
        assert_eq!(reader.take_moved_page(), None, "arrived");
        there.y = y + 5000.0;
        reader.set_viewport(there);
        assert!(reader.take_moved_page().is_some_and(|page| page > 50), "the reader moved");
    }

    /// Failure: synced scroll can't tell the reader's own scroll from a pane arriving where it was sent, so two panes keep sending each other back and forth, or a resize or a zoom drags the other pane along. Only a real scroll counts, and a pane sent somewhere by `follow` arrives there without leading.
    #[test]
    fn only_the_readers_own_scroll_leads_a_synced_pane() {
        let mut reader = reader_on(100);
        reader.scrolled_to = 0;
        settle(&mut reader);
        let mut base = view(0.0);
        base.scrolled_to = 0;
        reader.set_viewport(base);
        reader.take_user_scrolled();
        let mut scrolled = base;
        scrolled.y = 2000.0;
        reader.set_viewport(scrolled);
        assert!(reader.take_user_scrolled(), "the reader scrolled");
        assert!(!reader.take_user_scrolled(), "asking clears it");
        let page = reader.follow(40, 0.25).expect("a tab is showing");
        assert_eq!(page, 40);
        let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("no scroll") };
        let mut there = scrolled;
        there.y = y;
        there.scrolled_to = seq;
        reader.set_viewport(there);
        assert!(!reader.take_user_scrolled(), "arriving isn't leading");
        assert_eq!(reader.reading_anchor().map(|(page, _)| page), Some(40));
        let mut zoomed = there;
        zoomed.magnification = 1.5;
        zoomed.y += 300.0;
        reader.set_viewport(zoomed);
        assert!(!reader.take_user_scrolled(), "a zoom isn't a scroll");
        let mut pinch = zoomed;
        pinch.live = true;
        pinch.y += 50.0;
        reader.set_viewport(pinch);
        assert!(!reader.take_user_scrolled(), "nor is a pinch");
        assert_eq!(reader.follow(10_000, 2.0), Some(99), "held to the document");
    }

    /// Failure (found by restore's whole-state test, with comparison): a view that arrives where it was sent shows another page where the reader's eye is, as a narrower pane does, and the page view's place says one page while the session says another, so the place isn't brought back after a restart. Arriving on another page than was asked says so.
    #[test]
    fn arriving_on_another_page_than_asked_says_so() {
        let mut reader = reader_on(100);
        reader.scrolled_to = 0;
        settle(&mut reader);
        reader.take_moved_page();
        assert_eq!(reader.follow(40, 0.9), Some(40));
        assert_eq!(reader.take_moved_page(), Some(40), "sent there");
        let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("no scroll") };
        let mut there = view(y);
        there.scrolled_to = seq;
        reader.set_viewport(there);
        assert_eq!(reader.take_moved_page(), Some(41), "a third of the way down the view is the next page");
        let mut again = there;
        again.y += 1.0;
        reader.set_viewport(again);
        assert_eq!(reader.take_moved_page(), None, "and nothing more until it moves on");
    }

    /// Failure: three panes of a comparison keep three times what one view keeps off screen. Shared, each keeps its share, and what's over goes at once.
    #[test]
    fn panes_share_the_caps_on_what_is_kept() {
        let mut reader = reader_on(400);
        settle(&mut reader);
        for step in 0..60 {
            reader.set_viewport(view(f64::from(step) * 4000.0));
            settle(&mut reader);
        }
        let previews = |reader: &Reader| reader.entries.values().filter(|entry| entry.preview && !entry.attached).count();
        assert!(previews(&reader) > MAX_KEPT_PREVIEWS / 3, "enough kept to see the share: {}", previews(&reader));
        reader.set_share(3);
        assert!(previews(&reader) <= MAX_KEPT_PREVIEWS / 3, "{}", previews(&reader));
        reader.set_share(0);
        assert_eq!(reader.share, 1, "never none");
    }

    /// Failure (found by restore's whole-state test): side by side, going to a right-hand page arrives at a view whose top row starts with the left one, so the page view's place says the left page while the session says the right, and the place isn't brought back after a restart. And a change of layout moves the place to the other page of a row.
    #[test]
    fn side_by_side_keeps_the_page_of_its_row_the_reader_was_on() {
        let mut reader = reader_on(100);
        reader.scrolled_to = 0;
        reader.set_spread(Spread::TwoUp);
        settle(&mut reader);
        reader.take_moved_page();
        reader.jump_to(51);
        let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("no scroll") };
        let mut there = view(y);
        there.scrolled_to = seq;
        reader.set_viewport(there);
        assert_eq!(reader.take_moved_page(), None, "arrived");
        assert_eq!(reader.places()[0].1.page, 51, "the right-hand page it went to");
        there.y = y + 20.0;
        reader.set_viewport(there);
        assert_eq!(reader.take_moved_page(), None, "reading on in the row isn't going to the left page");
        reader.set_spread(Spread::Single);
        assert_eq!(reader.places()[0].1.page, 51);
    }

    /// The view the host goes to on the reader's next turn: wherever the last scroll it was told of says, as it would have applied it.
    fn follow_host(reader: &mut Reader, mut at: Viewport) -> Viewport {
        if let Some(Op::ScrollTo { y, seq }) = reader.take_ops().into_iter().rfind(|op| matches!(op, Op::ScrollTo { .. })) {
            at.y = y;
            at.scrolled_to = seq;
        }
        reader.set_viewport(at);
        at
    }

    /// Failure (review of f05c998, finding 5): side by side, a sleep file's pictures cover half as many rows of a view as a page to a row does, so waking a tab, or an update's first frame, shows bare pages where it didn't before. Every page of the view at a zoom that shows six rows of pairs gets one.
    #[test]
    fn a_sleep_file_covers_the_view_side_by_side_too() {
        let mut reader = reader_on(40);
        reader.note_version(Path::new("/d/a.pdf"), 1);
        reader.set_spread(Spread::TwoUp);
        let mut far = view(0.0);
        far.height = 2400.0;
        reader.set_viewport(far);
        settle(&mut reader);
        let shown: Vec<u32> = reader.visible_pages(reader.doc().unwrap()).collect();
        assert!(shown.len() > 6 && shown.len() <= 12, "the view shows {} pages", shown.len());
        let pictures: Vec<u32> = reader.sleep_material(1, None).unwrap().pictures.iter().map(|picture| picture.page).collect();
        assert_eq!(pictures, shown, "a picture for each page shown");
    }

    /// Failure (second review): a tab off screen gets its sleep pictures for the height of the tab showing's view at that tab's zoom, not its own, so a tab left side by side at a low zoom got 4 pictures of the 12 pages it showed, and woke with bare pages.
    #[test]
    fn a_tab_off_screen_gets_pictures_for_its_own_view() {
        let mut reader = reader_on(40);
        reader.note_version(Path::new("/d/a.pdf"), 1);
        reader.scrolled_to = 0;
        reader.set_spread(Spread::TwoUp);
        let mut low = view(0.0);
        low.magnification = 0.4;
        low.width /= 0.4;
        low.height /= 0.4;
        reader.set_viewport(low);
        settle(&mut reader);
        let shown: Vec<u32> = reader.visible_pages(reader.doc().unwrap()).take(12).collect();
        assert!(shown.len() > 6, "the low zoom shows {} pages", shown.len());
        // Another tab comes to the screen at its own zoom, and the first is held, off screen.
        reader.activate(2, 10, 0, PageMap { files: vec![PathBuf::from("/d/b.pdf")], ..map() }, &[1], None, Spread::Single);
        reader.set_viewport(view(0.0));
        settle(&mut reader);
        let pictures: Vec<u32> = reader.sleep_material(1, None).unwrap().pictures.iter().map(|picture| picture.page).collect();
        assert_eq!(pictures, shown, "a picture for each page it showed");
    }

    /// Failure (review of f05c998, finding 1): Rotate, Undo or Redo of a turn sends the reader to the top of their page and back to the left edge, as the tab showing was taken up again as if it had just been come to.
    #[test]
    fn a_turn_keeps_the_place_in_the_page_and_across() {
        let mut reader = reader_on(10);
        reader.scrolled_to = 0;
        settle(&mut reader);
        let page = reader.doc().unwrap().layout.rect(3);
        let mut at = view(page.y + page.height * 0.4);
        at.x = 30.0;
        at.magnification = 2.0;
        reader.set_viewport(at);
        reader.take_moved_page();
        let before = reader.places()[0].1;
        let turned = PageMap { turns: Some(Arc::from(vec![1u8; 10])), ..map() };
        reader.activate(1, 10, 3, turned, &[], None, Spread::Single);
        follow_host(&mut reader, at);
        let after = reader.places()[0].1;
        assert_eq!(after.page, 3);
        assert!((after.anchor.1 - before.anchor.1).abs() < 0.02, "as far down the page: {:?} then {:?}", before.anchor, after.anchor);
        assert_eq!(reader.docs[&1].scroll_x, 30.0, "and as far across");
    }

    /// Failure (review of f05c998, B2): side by side, after any scroll, a change of layout moves the reader off the page they were on: the place was kept on the first page of its row, so the right-hand page's reader lands on the left one, or two pages back going to the cover layout. Fed the host's next view, as the app is.
    #[test]
    fn a_change_of_layout_after_a_scroll_keeps_the_page() {
        for (from, to) in [(Spread::TwoUp, Spread::Single), (Spread::TwoUp, Spread::TwoUpCover), (Spread::TwoUpCover, Spread::TwoUp)] {
            let mut reader = reader_on(100);
            reader.scrolled_to = 0;
            reader.set_spread(from);
            settle(&mut reader);
            reader.jump_to(51);
            let at = follow_host(&mut reader, view(0.0));
            let mut scrolled = at;
            scrolled.y += 20.0;
            reader.set_viewport(scrolled);
            reader.take_moved_page();
            let was = reader.doc().unwrap().layout.rect(51);
            let down = (scrolled.y - was.y) / was.height;
            reader.set_spread(to);
            follow_host(&mut reader, scrolled);
            let place = reader.places()[0].1;
            assert_eq!(place.page, 51, "{from:?} to {to:?}");
            let rect = reader.doc().unwrap().layout.rect(51);
            assert!((reader.viewport.y - (rect.y + down * rect.height)).abs() < 1.0, "{from:?} to {to:?}: as far down the page: {} against {}", reader.viewport.y, rect.y + down * rect.height);
        }
    }

    /// Failure: a page that couldn't be drawn is asked for on every scroll, or holds up the rest, or shows no reason.
    #[test]
    fn a_failed_page_says_why_and_isnt_asked_for_again() {
        let mut reader = reader_on(3);
        for _ in 0..16 {
            let Some(job) = reader.next_job() else { break };
            let outcome = match &job.kind {
                JobKind::Size => Outcome::Size(letter()),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => Outcome::Heavy(false),
                JobKind::Draw { .. } if job.position == 0 => Outcome::PageFailed { message: "This page couldn't be drawn.".into(), may_pass: false },
                JobKind::Draw { .. } => Outcome::Drawn { kind: TileKind::Colour, low_res: None },
            };
            reader.finish(job, outcome);
        }
        reader.set_viewport(view(0.0));
        assert_eq!(reader.note(), Some("This page couldn't be drawn."));
        assert!(reader.next_job().is_none_or(|job| job.position != 0));
    }

    /// Failure: a lower-resolution page looks like a sharp one, with nothing to say so. SPEC §3: it says so. And one that may clear is tried again once its time is up.
    #[test]
    fn a_lower_resolution_page_is_labelled() {
        let mut reader = reader_on(1);
        let rect = TileRect { x: 0, y: 0, width: 100, height: 100 };
        let mut low_done = false;
        for _ in 0..64 {
            let Some(job) = reader.next_job() else { break };
            let outcome = match &job.kind {
                JobKind::Size => Outcome::Size(letter()),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => Outcome::Heavy(true),
                JobKind::Draw { .. } => {
                    low_done = true;
                    Outcome::Drawn { kind: TileKind::Colour, low_res: Some((rect, LowResWhy::TooHeavy)) }
                }
            };
            reader.finish(job, outcome);
            reader.drain(64);
            reader.applied();
        }
        assert!(low_done);
        assert_eq!(reader.note(), Some(LowResWhy::TooHeavy.message()));
        assert_eq!(reader.report().0["low_res_pages"][0], 1);
        assert!(reader.is_settled(), "{}", reader.report().0);
    }

    /// Failure: a heavy page is drawn a tile at a time, each pass costing it half a second, as the first version did on the USGS map.
    #[test]
    fn a_heavy_page_is_drawn_in_regions_with_no_preview() {
        let mut reader = reader_on(1);
        let mut kinds = Vec::new();
        for _ in 0..64 {
            let Some(job) = reader.next_job() else { break };
            let outcome = match &job.kind {
                JobKind::Size => Outcome::Size(PageBox { width: 1224.0, height: 1584.0 }),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => Outcome::Heavy(true),
                JobKind::Draw { what, .. } => {
                    kinds.push(*what);
                    Outcome::Drawn { kind: TileKind::Colour, low_res: None }
                }
            };
            reader.finish(job, outcome);
            reader.drain(64);
            reader.applied();
        }
        assert!(kinds.contains(&DrawKind::Region), "{kinds:?}");
        assert!(!kinds.contains(&DrawKind::Preview), "{kinds:?}");
        assert!(reader.is_settled(), "{}", reader.report().0);
    }

    /// Failure: edits that reorder pages leave tiles of the old order on screen.
    #[test]
    fn a_change_to_the_page_order_clears_what_was_drawn() {
        let mut reader = reader_on(5);
        settle(&mut reader);
        let edited = PageMap { files: vec![PathBuf::from("/d/a.pdf")], slots: Some(vec![(0, 1), (0, 0)]), own: None, turns: None };
        reader.activate(1, 2, 0, edited.clone(), &[], None, Spread::Single);
        assert!(reader.entries.is_empty());
        assert_eq!(reader.doc().unwrap().layout.count(), 2);
        assert_eq!(edited.source(0), Some((PathBuf::from("/d/a.pdf"), 1)));
        assert_eq!(edited.source(9), None);
    }

    /// Failure: a page whose size can't be read is asked about again at once, for ever: 100% of a core on a hostile page, nothing else is drawn, and everything waiting for it to settle times out.
    #[test]
    fn a_page_whose_size_cant_be_read_is_asked_once() {
        let mut reader = reader_on(3);
        let mut asked = 0;
        for _ in 0..200 {
            reader.take_ops();
            reader.applied();
            let Some(job) = reader.next_job() else { break };
            let outcome = match &job.kind {
                JobKind::Size if job.position == 0 => {
                    asked += 1;
                    Outcome::PageFailed { message: "This page couldn't be read.".into(), may_pass: false }
                }
                JobKind::Size => Outcome::Size(letter()),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => Outcome::Heavy(false),
                JobKind::Draw { .. } => Outcome::Drawn { kind: TileKind::Colour, low_res: None },
            };
            reader.finish(job, outcome);
            reader.drain(64);
            reader.applied();
        }
        assert_eq!(asked, 1);
        assert!(reader.is_settled(), "{}", reader.report().0);
        assert_eq!(reader.note(), Some("This page couldn't be read."));
    }

    /// Failure: a page is deleted while a job on it is running, and when the job ends its old picture goes in the place of the page now there, reading as settled. The same for a size or a heavy answer about what used to be there.
    #[test]
    fn what_a_job_found_before_an_edit_is_thrown_away() {
        let edited = PageMap { files: vec![PathBuf::from("/d/a.pdf")], slots: Some(vec![(0, 1), (0, 2)]), own: None, turns: None };
        // A job of each kind, caught running.
        let mut reader = reader_on(3);
        let size = reader.next_job().expect("a size");
        assert!(matches!(size.kind, JobKind::Size));
        reader.activate(1, 2, 0, edited.clone(), &[], None, Spread::Single);
        reader.finish(size, Outcome::Size(PageBox { width: 100.0, height: 100.0 }));
        assert!(!reader.doc().unwrap().layout.is_known(0), "that size was the old page's");

        let mut reader = reader_on(3);
        let traits = loop {
            let job = reader.next_job().expect("a job");
            match job.kind {
                JobKind::Size => reader.finish(job, Outcome::Size(letter())),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => break job,
                JobKind::Draw { .. } => panic!("traits come before drawing"),
            }
        };
        let position = traits.position;
        reader.activate(1, 2, 0, edited.clone(), &[], None, Spread::Single);
        reader.finish(traits, Outcome::Heavy(true));
        assert!(!reader.doc().unwrap().heavy.contains_key(&position), "that was the old page's");

        let mut reader = reader_on(3);
        let draw = loop {
            let job = reader.next_job().expect("a job");
            match job.kind {
                JobKind::Size => reader.finish(job, Outcome::Size(letter())),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
                JobKind::Draw { .. } => break job,
            }
        };
        reader.note_started(&draw, None);
        reader.activate(1, 2, 0, edited, &[], None, Spread::Single);
        reader.finish(draw, Outcome::Drawn { kind: TileKind::Colour, low_res: None });
        assert!(reader.entries.is_empty(), "the old page's picture stays out");
        assert!(reader.cover.is_empty());
    }

    /// Failure: a surface that held one file's page is handed to another file's worker, so a hostile file reads a page of the one before it. Both ways round: the next file's worker gets surfaces a cold tab's tiles had, and a hostile file that kept a reference, answered "blank" and got its surface taken back then sees the next file's pixels and can repaint them. No surface may ever serve two files. Each is held here, as a hostile worker holds it, so its name can't be given to a new one by chance.
    #[test]
    fn no_surface_serves_two_files() {
        fn run(reader: &mut Reader, answer: TileKind, held: &mut Vec<PixelSurface>) -> HashSet<u32> {
            let mut seen = HashSet::new();
            for _ in 0..1000 {
                reader.take_ops();
                reader.applied();
                let Some(job) = reader.next_job() else { break };
                let outcome = match &job.kind {
                    JobKind::Size => Outcome::Size(letter()),
                    JobKind::Text { .. } => panic!("no text was asked for"),
                    JobKind::Traits => Outcome::Heavy(false),
                    JobKind::Draw { surface, .. } => {
                        seen.insert(surface.id());
                        held.push(surface.clone());
                        Outcome::Drawn { kind: answer, low_res: None }
                    }
                };
                reader.finish(job, outcome);
            }
            seen
        }
        let other = |name: &str| PageMap { files: vec![PathBuf::from(name)], slots: None, own: None, turns: None };
        for answer in [TileKind::Colour, TileKind::Blank { rgb: 0xFF_FF_FF }] {
            let mut held = Vec::new();
            let mut reader = reader_on(3);
            let first = run(&mut reader, answer, &mut held);
            reader.activate(2, 3, 0, other("/d/b.pdf"), &[], None, Spread::Single);
            reader.set_viewport(view(0.0));
            let second = run(&mut reader, TileKind::Colour, &mut held);
            assert!(!first.is_empty() && !second.is_empty());
            assert!(first.is_disjoint(&second), "served both: {:?}", first.intersection(&second).collect::<Vec<_>>());
        }
    }

    /// Failure: a tile comes off in Rust just after the UI thread took its ops, and "applied" for those ops marks it discardable while its layer is still showing it, so the OS can empty a surface on screen. Only what the host was told about counts.
    #[test]
    fn a_surface_goes_discardable_only_after_the_host_was_told() {
        let mut reader = reader_on(3);
        settle(&mut reader);
        let (&page, ids) = reader.attached.iter().next().unwrap();
        let id = ids[0];
        reader.take_ops();
        // The engine's thread, between the host's taking its ops and saying it has applied them.
        reader.detach(id, page);
        reader.applied();
        assert!(!reader.entries[&id].discardable, "the layer still has it");
        reader.take_ops();
        reader.applied();
        assert!(reader.entries[&id].discardable, "now the host has been told");
    }

    /// Finishes everything that isn't text, with a pretend renderer, and hands back the first text job.
    fn settle_to_text(reader: &mut Reader) -> Option<Job> {
        loop {
            reader.take_ops();
            reader.applied();
            let job = reader.next_job()?;
            match &job.kind {
                JobKind::Text { .. } => return Some(job),
                JobKind::Size => reader.finish(job, Outcome::Size(letter())),
                JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
                JobKind::Draw { .. } => reader.finish(job, Outcome::Drawn { kind: TileKind::Colour, low_res: None }),
            }
        }
    }

    /// Failure: a hidden-text check that takes seconds keeps the page the reader has just scrolled to blank, over and over on a hostile file, as a running text job couldn't be told to stop. Its first read, which is quick and is what the reader's hand is waiting on, isn't given up for a scroll.
    #[test]
    fn a_check_goes_when_the_view_wants_something_and_a_read_does_not() {
        let mut reader = reader_on(100);
        settle_to_text(&mut reader);
        reader.select_all();
        let first = settle_to_text(&mut reader).expect("what shows is read");
        assert!(matches!(first.kind, JobKind::Text { check: false }), "{first:?}");
        reader.note_started(&first, None);
        reader.set_viewport(view(40_000.0));
        assert!(reader.running_is_wanted(), "a read stays, as what's showing needs it");
        reader.running = None;
        // Back where it was, with everything there, and the check runs.
        reader.set_viewport(view(0.0));
        let text = Arc::new(recto_index::select::PageText::default());
        reader.finish(first, Outcome::Text { text, checked: false });
        let check = settle_to_text(&mut reader).expect("selected and showing, so it's checked");
        assert!(matches!(check.kind, JobKind::Text { check: true }), "{check:?}");
        reader.note_started(&check, None);
        assert!(reader.running_is_wanted(), "nothing else is wanted");
        // A scroll to pages with nothing drawn.
        let asked = reader.counts.cancels_asked;
        reader.set_viewport(view(40_000.0));
        assert!(!reader.running_is_wanted(), "the view wants tiles");
        assert_eq!(reader.counts.cancels_asked, asked + 1);
    }

    /// Failure: a check that was stopped reads as one that was tried, so the page is never checked, or a stopped read holds a place in the cache as a page that failed.
    #[test]
    fn a_stopped_text_job_is_asked_for_again() {
        let mut reader = reader_on(10);
        settle_to_text(&mut reader);
        reader.select_all();
        let read = settle_to_text(&mut reader).expect("what shows is read");
        let page = read.position;
        reader.finish(read, Outcome::Cancelled);
        let again = settle_to_text(&mut reader).expect("it's still wanted");
        assert_eq!(again.position, page);
        assert!(matches!(again.kind, JobKind::Text { check: false }));
        reader.finish(again, Outcome::Text { text: Arc::new(recto_index::select::PageText::default()), checked: false });
        let check = settle_to_text(&mut reader).expect("not tried, so asked for");
        assert!(matches!(check.kind, JobKind::Text { check: true }));
    }

    /// Failure: a tile finishes after the reader has left it, or for an old zoom or paper or a warm tab, and it's held in the footprint with nothing showing it. Only what's on a layer may count, or memory has no bound the budgets can rely on.
    #[test]
    fn a_tile_nothing_is_showing_when_it_arrives_is_discardable() {
        let mut reader = reader_on(100);
        let draw = loop {
            let job = reader.next_job().expect("a job");
            match job.kind {
                JobKind::Size => reader.finish(job, Outcome::Size(letter())),
                JobKind::Text { .. } => panic!("no text was asked for"),
                JobKind::Traits => reader.finish(job, Outcome::Heavy(false)),
                JobKind::Draw { .. } => break job,
            }
        };
        reader.set_viewport(view(40_000.0));
        reader.finish(draw, Outcome::Drawn { kind: TileKind::Colour, low_res: None });
        let entry = reader.entries.values().next().expect("kept");
        assert!(!entry.attached && entry.discardable, "{entry:?}");
    }

    /// Failure: going to another tab and back sends the reader to the top of their page, so on a tall page, or any page read past its top, the place is lost. On the page it was left on, it comes back to the same place, and anywhere else it starts at the top.
    #[test]
    fn coming_back_to_a_tab_keeps_the_place_in_the_page() {
        let mut reader = reader_on(100);
        settle(&mut reader);
        let tops = |reader: &Reader, page| reader.doc().unwrap().layout.top(page);
        let inside = tops(&reader, 40) + 300.0;
        reader.set_viewport(view(inside));
        let page = reader.take_moved_page().expect("the reader moved");
        settle(&mut reader);
        let other = PageMap { files: vec![PathBuf::from("/d/b.pdf")], slots: None, own: None, turns: None };
        reader.activate(2, 10, 0, other.clone(), &[1], None, Spread::Single);
        reader.take_ops();
        reader.activate(1, 100, page, map(), &[2], None, Spread::Single);
        let scrolled: Vec<f64> = reader.take_ops().into_iter().filter_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None }).collect();
        assert!((scrolled.last().unwrap() - inside).abs() < 1.0, "{scrolled:?} against {inside}");

        reader.activate(2, 10, 0, other, &[1], None, Spread::Single);
        reader.take_ops();
        reader.activate(1, 100, page + 3, map(), &[2], None, Spread::Single);
        let scrolled: Vec<f64> = reader.take_ops().into_iter().filter_map(|op| if let Op::ScrollTo { y, .. } = op { Some(y) } else { None }).collect();
        let top = tops(&reader, page + 3);
        assert!((scrolled.last().unwrap() - top).abs() < 1.0, "sent somewhere else, it starts at the top: {scrolled:?} against {top}");
    }

    /// Failure: closing a warm tab leaves its layout, tiles, worker and open file until the next switch of tab, which may never come.
    #[test]
    fn a_warm_tab_that_goes_holds_nothing_at_once() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        reader.activate(2, 10, 0, PageMap { files: vec![PathBuf::from("/d/b.pdf")], slots: None, own: None, turns: None }, &[1], None, Spread::Single);
        assert!(reader.entries.values().any(|entry| entry.tab == 1), "still warm");
        assert_eq!(reader.wanted_files().len(), 2);
        reader.set_warm(&[]);
        assert!(reader.entries.values().all(|entry| entry.tab != 1), "closed");
        assert!(!reader.docs.contains_key(&1));
        assert_eq!(*reader.wanted_files(), vec![PathBuf::from("/d/b.pdf")], "and its worker goes");
    }

    /// Failure: a new window's first viewport, at the top and no zoom, comes before it's opened and is taken for the reader moving, so closing the window and opening it again loses the place and the zoom. Before and after it's told, what it says doesn't count till it's arrived.
    #[test]
    fn reopening_the_window_keeps_the_place_and_the_zoom() {
        let mut reader = reader_on(100);
        settle(&mut reader);
        reader.set_viewport(Viewport { magnification: 2.0, ..view(0.0) });
        let inside = reader.doc().unwrap().layout.top(40) + 300.0;
        reader.set_viewport(Viewport { magnification: 2.0, ..view(inside) });
        reader.take_moved_page();
        settle(&mut reader);
        // As the old window got to, with its own count of scrolls.
        reader.scrolled_to = 0;
        reader.window_closed();
        let new_window = Viewport { scrolled_to: 0, ..view(0.0) };
        reader.set_viewport(new_window);
        assert_eq!(reader.take_moved_page(), None, "the new window isn't the reader moving");
        reader.window_opened();
        let ops = reader.set_viewport(new_window);
        assert_eq!(reader.take_moved_page(), None, "nor is it before it has got there");
        assert!(ops.iter().any(|op| matches!(op, Op::Magnify { value } if *value == 2.0)), "{ops:?}");
        let Some(Op::ScrollTo { y, seq }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("{ops:?}") };
        assert!((y - inside).abs() < 1.0, "{y} against {inside}");
        reader.set_viewport(Viewport { magnification: 2.0, scrolled_to: *seq, ..view(*y) });
        assert_eq!(reader.take_moved_page(), None, "arriving isn't a move");
        assert_eq!(reader.doc().unwrap().magnification, 2.0);
    }

    /// Failure (decision 25): a tab woken from its sleep file comes up with bare pages until they're drawn, or a picture's used for the wrong page or paper, or lands on the wrong tab. It's on the layers in the first turn with nothing drawn, and it's kept as a preview, so sharp tiles replace it as usual.
    #[test]
    fn the_tabs_sleep_view_is_its_first_frame() {
        let mut reader = Reader::new();
        reader.set_viewport(view(0.0));
        let surface = PixelSurface::create(153, 198).unwrap();
        let shot = |page: u32, surface: PixelSurface| SleepShot { page, scale: 0.25, paper: PaperKind::White, surface };
        let saved = SavedPlace { page: 0, anchor: (0, 0.0), x: 0.0, y: 0.0, document_height: 0.0, magnification: 1.0, page_size: Some(letter()) };
        // For another tab: not used.
        reader.give_sleep(9, SleepView { version: None, usual: Some(letter()), sizes: Vec::new(), pictures: vec![shot(0, PixelSurface::create(153, 198).unwrap())] });
        reader.note_wake(1, Instant::now());
        reader.give_sleep(1, SleepView { version: None, usual: Some(letter()), sizes: vec![(3, PageBox { width: 300.0, height: 400.0 })], pictures: vec![shot(0, surface), shot(500, PixelSurface::create(153, 198).unwrap())] });
        reader.activate(1, 5, 0, map(), &[], Some(saved), Spread::Single);
        let doc = reader.doc().unwrap();
        assert!(doc.layout.is_known(0));
        assert!(doc.layout.is_known(3), "the page table's taken up");
        assert_eq!(doc.layout.size(3), PageBox { width: 300.0, height: 400.0 });
        assert_eq!(reader.entries.len(), 1, "the one for a page that's there");
        let scrolled = reader.jump.unwrap_or(0);
        let ops = reader.set_viewport(Viewport { scrolled_to: scrolled, ..view(0.0) });
        let attached = reader.attached.get(&0).is_some_and(|ids| !ids.is_empty());
        assert!(attached, "it's up with nothing drawn: {ops:?}");
        assert!(ops.iter().any(|op| matches!(op, Op::Attach { below: true, .. })));
        reader.take_ops();
        reader.unapplied = true;
        reader.applied();
        let wake = reader.last_wake().unwrap();
        assert!(wake.from_sleep);
        assert!(wake.first_frame_us.is_some(), "{wake:?}");
        assert_eq!(wake.sharp_us, None, "not sharp yet");
        assert_eq!(wake.blank_frames, 0);
        // With no sleep file and the window up, a bare page is counted, which is what the budget's about.
        let mut bare = Reader::new();
        bare.window_up();
        bare.set_viewport(view(0.0));
        bare.note_wake(1, Instant::now());
        bare.activate(1, 5, 0, map(), &[], Some(saved), Spread::Single);
        bare.take_ops();
        let scrolled = bare.jump.unwrap_or(0);
        bare.set_viewport(Viewport { scrolled_to: scrolled, ..view(0.0) });
        bare.take_ops();
        bare.unapplied = true;
        bare.applied();
        assert_eq!(bare.last_wake().unwrap().blank_frames, 1);
        // A worker's asked for what's sharp, as if there'd been none of this, and no preview as it's got one.
        let jobs = settle(&mut reader);
        assert!(!jobs.iter().any(|job| matches!(job.kind, JobKind::Draw { what: DrawKind::Preview, .. }) && job.position == 0));
    }

    /// Failure: a sleep file has no picture of a heavy page, which has no preview, so it wakes bare; or what's put together of its tiles is the wrong size or in the wrong place; or a page's picture comes from a preview the system's taken. It's a real picture of the page at a smaller scale, and pixels of a tile land where they should.
    #[test]
    fn a_picture_for_sleep_comes_from_a_preview_or_the_tiles() {
        let mut reader = reader_on(5);
        assert!(reader.sleep_material(1, None).unwrap().pictures.is_empty(), "no worker's said what version this is, so no picture can say");
        reader.note_version(Path::new("/d/a.pdf"), 1);
        settle(&mut reader);
        let material = reader.sleep_material(1, None).expect("a tab it holds");
        assert!(reader.sleep_material(1, Some(FileStamp { size: 1, mtime_ns: 1, device: 0, inode: 1 })).is_none(), "what's here is of another version of the file than that");
        assert_eq!(material.usual, Some(letter()));
        assert!(!material.pictures.is_empty());
        let picture = &material.pictures[0];
        assert_eq!(picture.page, 0);
        assert_eq!(picture.bgra.len(), picture.width as usize * picture.height as usize * 4);
        assert!(reader.sleep_material(99, None).is_none());
        // Pixels from a tile, shrunk by two: a block of 2 by 2 comes to one, and their average, and it lands at the tile's place.
        let mut tile = vec![0u8; 4 * 4 * 4];
        for pixel in tile.as_chunks_mut::<4>().0 {
            *pixel = [100, 100, 100, 255];
        }
        tile[0..4].copy_from_slice(&[200, 200, 200, 255]);
        let mut picture = vec![9u8; 4 * 4 * 4];
        shrink_into(&tile, 16, 4, 4, 2, (1, 1), &mut picture, 4, 4);
        let at = |x: usize, y: usize| picture[(y * 4 + x) * 4];
        assert_eq!(at(0, 0), 9, "outside it, left alone");
        assert_eq!(at(1, 1), 125, "(200 + 3 x 100) / 4");
        assert_eq!(at(2, 2), 100);
        assert_eq!(at(3, 3), 9);
        // Off the edge of the picture: nothing panics.
        shrink_into(&tile, 16, 4, 4, 2, (3, 3), &mut picture, 4, 4);
        shrink_into(&tile[..8], 16, 4, 4, 2, (0, 0), &mut picture, 4, 4);
    }

    /// Failure (review): a tab's file turns out to be another, by a change outside or a save, and its previews stay, so its next sleep file has the old file's pictures under the new file's name, for good, and with a redaction that picture's what was removed. Another version of the file is a change of what's on the pages, and everything of it goes, on screen and off.
    #[test]
    fn another_version_of_the_file_takes_every_picture_of_the_old_one() {
        let version = |size: u64| Some(FileStamp { size, mtime_ns: 1, device: 0, inode: 7 });
        let on = |own: Option<FileStamp>| PageMap { own, ..map() };
        let mut reader = Reader::new();
        reader.set_viewport(view(0.0));
        reader.activate(1, 5, 0, on(version(1)), &[], None, Spread::Single);
        reader.note_version(Path::new("/d/a.pdf"), 1);
        settle(&mut reader);
        assert!(reader.entries.values().any(|entry| entry.preview), "there's a preview");
        let material = reader.sleep_material(1, version(1)).unwrap();
        assert!(!material.pictures.is_empty());
        // Told it's another file: nothing of the old one's left, and the old one's not offered to the sleep file under the new name.
        assert!(reader.sleep_material(1, version(2)).is_none());
        reader.activate(1, 5, 0, on(version(2)), &[], None, Spread::Single);
        assert!(reader.entries.is_empty(), "every picture and tile of it goes");
        assert!(reader.sleep_material(1, version(2)).unwrap().pictures.is_empty());
        assert!(reader.take_ops().contains(&Op::Clear), "and off the screen");
        assert_eq!(reader.take_reopen_files(), [PathBuf::from("/d/a.pdf")], "and a worker that has the old one open goes, or it goes on drawing that");
    }

    /// Failure (review, the red frames): a tab's file is replaced while its tiles and previews are held, and they go on being shown, or written in its sleep file, as nothing that drew them says which version they're of. Each says, and as soon as a worker says the file's another version, whatever's of the old one goes, from the screen too, and nothing of it can be put back on a layer or into a sleep file.
    #[test]
    fn nothing_drawn_of_an_old_version_is_shown_or_kept() {
        let file = Path::new("/d/a.pdf");
        let mut reader = reader_on(5);
        reader.note_version(file, 1);
        settle(&mut reader);
        let drawn: Vec<u64> = reader.entries.keys().copied().collect();
        assert!(!drawn.is_empty() && reader.entries.values().all(|entry| entry.version == Some(1)));
        // Off screen and back: it would come straight back from what's kept.
        reader.set_viewport(view(20_000.0));
        settle(&mut reader);
        reader.note_version(file, 2);
        assert!(drawn.iter().all(|id| !reader.entries.contains_key(id)), "everything of version 1 is gone");
        reader.set_viewport(view(0.0));
        let ops = reader.take_ops();
        assert!(!ops.iter().any(|op| matches!(op, Op::Attach { id, .. } if drawn.contains(id))), "nothing of it goes back on a layer");
        assert!(reader.sleep_material(1, None).unwrap().pictures.is_empty(), "and nothing's offered for a sleep file till it's drawn again");
        settle(&mut reader);
        assert!(reader.entries.values().all(|entry| entry.version == Some(2)));
        assert!(!reader.sleep_material(1, None).unwrap().pictures.is_empty());
        // Something kept whose version is stale is refused at the moment it would go on a layer, however it got there.
        let stale = *reader.entries.keys().next().unwrap();
        if let Some(entry) = reader.entries.get_mut(&stale) {
            entry.version = Some(1);
            entry.attached = false;
        }
        assert!(!reader.attach(stale, false));
        assert!(!reader.entries.contains_key(&stale));
        // A picture from a sleep file, of a version a worker then says isn't this one, goes too.
        let mut woken = Reader::new();
        woken.set_viewport(view(0.0));
        let shot = SleepShot { page: 0, scale: 0.25, paper: PaperKind::White, surface: PixelSurface::create(153, 198).unwrap() };
        woken.give_sleep(1, SleepView { version: Some(7), usual: Some(letter()), sizes: Vec::new(), pictures: vec![shot] });
        woken.activate(1, 5, 0, map(), &[], None, Spread::Single);
        assert_eq!(woken.entries.len(), 1);
        woken.note_version(file, 8);
        assert!(woken.entries.is_empty());
    }

    /// Failure: memory pressure takes what's on screen, leaving a blank page, or leaves what could go. And at its worst, the previews go too.
    #[test]
    fn memory_pressure_leaves_only_whats_showing() {
        let mut reader = reader_on(30);
        settle(&mut reader);
        reader.set_viewport(view(5000.0));
        settle(&mut reader);
        let attached = reader.attached.values().map(Vec::len).sum::<usize>();
        assert!(reader.entries.len() > attached, "there's something off screen to go");
        reader.memory_pressure(false);
        let previews = reader.entries.values().filter(|entry| !entry.attached && entry.preview).count();
        assert!(reader.entries.values().all(|entry| entry.attached || entry.preview), "only what's showing, and previews");
        assert!(previews > 0);
        reader.memory_pressure(true);
        assert_eq!(reader.entries.len(), attached);
        assert_eq!(reader.attached.values().map(Vec::len).sum::<usize>(), attached, "nothing's come off the screen");
    }

    /// Failure: a window can't go up until every tile's drawn, which on a heavy page is most of a second, or it goes up over a bare page.
    #[test]
    fn a_window_can_go_up_with_a_picture_on_every_page() {
        let mut reader = Reader::new();
        reader.set_viewport(view(0.0));
        let saved = SavedPlace { page: 0, anchor: (0, 0.0), x: 0.0, y: 0.0, document_height: 0.0, magnification: 1.0, page_size: Some(letter()) };
        reader.give_sleep(1, SleepView { version: None, usual: Some(letter()), sizes: Vec::new(), pictures: Vec::new() });
        reader.activate(1, 5, 0, map(), &[], Some(saved), Spread::Single);
        reader.take_ops();
        assert!(!reader.is_showable(), "still being sent to its place, with nothing on it");
        let scrolled = reader.jump.unwrap_or(0);
        reader.set_viewport(Viewport { scrolled_to: scrolled, ..view(0.0) });
        reader.take_ops();
        reader.unapplied = false;
        assert!(!reader.is_showable(), "a bare page isn't what goes up");
        let shot = SleepShot { page: 0, scale: 0.25, paper: PaperKind::White, surface: PixelSurface::create(153, 198).unwrap() };
        reader.give_sleep(2, SleepView { version: None, usual: Some(letter()), sizes: Vec::new(), pictures: vec![shot] });
        reader.activate(2, 1, 0, map(), &[], Some(saved), Spread::Single);
        reader.take_ops();
        let scrolled = reader.jump.unwrap_or(0);
        reader.set_viewport(Viewport { scrolled_to: scrolled, ..view(0.0) });
        reader.take_ops();
        reader.unapplied = false;
        assert!(reader.is_showable());
        assert!(!reader.is_on_place(), "and that's sooner than sharp");
    }

    /// Failure: a tab comes back where the document's top is, or at no zoom, or near its place by rounding rather than on it. Or the place is taken from where the view still is on its way there, so the next layout goes somewhere else. What's said comes straight back out, to the pixel, and a place for another page, as a `goto` while cold leaves, isn't used.
    #[test]
    fn a_saved_place_comes_back_to_the_pixel() {
        let mut reader = Reader::new();
        reader.window_opened();
        reader.set_viewport(view(0.0));
        let frame_view = view(0.0);
        let mut layout = PageLayout::new(100);
        layout.seed_usual(letter());
        layout.set_frame(frame_view.frame_width);
        let height = layout.document_height();
        let y = layout.top(40) + 123.456_789;
        let saved = SavedPlace { page: 40, anchor: layout.anchor_at(y), x: 33.5, y, document_height: height, magnification: 1.75, page_size: Some(letter()) };
        reader.activate(1, 100, 40, map(), &[], Some(saved), Spread::Single);
        let ops = reader.take_ops();
        assert!(ops.iter().any(|op| matches!(op, Op::Magnify { value } if *value == 1.75)), "{ops:?}");
        assert!(ops.iter().any(|op| matches!(op, Op::ScrollAcross { x } if *x == 33.5)), "{ops:?}");
        let Some(Op::ScrollTo { y: went, seq }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("{ops:?}") };
        assert_eq!(*went, y, "exactly");
        // Not there yet: where the view says it is, isn't the place.
        reader.scrolled_to = 0;
        reader.set_viewport(Viewport { magnification: 1.75, scrolled_to: 0, ..view(0.0) });
        assert_eq!(reader.places()[0].1.y, y);
        assert_eq!(reader.places()[0].1.anchor, saved.anchor);
        // Arrived.
        reader.set_viewport(Viewport { magnification: 1.75, x: 33.5, scrolled_to: *seq, ..view(y) });
        let (_, place) = reader.places()[0];
        assert_eq!((place.page, place.y, place.x, place.magnification), (40, y, 33.5, 1.75));
        assert_eq!(place.anchor, saved.anchor);
        assert_eq!(reader.take_moved_page(), None, "arriving isn't a move");
        // Left and back in the same session.
        reader.activate(2, 10, 0, map(), &[1], None, Spread::Single);
        reader.activate(1, 100, 40, map(), &[2], None, Spread::Single);
        let ops = reader.take_ops();
        let Some(Op::ScrollTo { y: went, .. }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("{ops:?}") };
        assert_eq!(*went, y);
        // Another page since: its top, and no zoom it shouldn't have kept is the place's business.
        let mut fresh = Reader::new();
        fresh.window_opened();
        fresh.set_viewport(view(0.0));
        fresh.activate(1, 100, 7, map(), &[], Some(saved), Spread::Single);
        let ops = fresh.take_ops();
        let Some(Op::ScrollTo { y: went, .. }) = ops.iter().rev().find(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("{ops:?}") };
        assert_eq!(*went, fresh.doc().unwrap().layout.top(7));
        // What a record can say that isn't a place doesn't get as far as a host.
        let mut hostile = Reader::new();
        hostile.window_opened();
        hostile.set_viewport(view(0.0));
        hostile.activate(1, 100, 40, map(), &[], Some(SavedPlace { anchor: (9_000_000, f64::NAN), x: f64::NAN, y: f64::NAN, document_height: f64::INFINITY, magnification: f64::NAN, ..saved }), Spread::Single);
        for op in hostile.take_ops() {
            match op {
                Op::ScrollTo { y, .. } => assert!(y.is_finite(), "{y}"),
                Op::ScrollAcross { x } => assert!(x.is_finite(), "{x}"),
                Op::Magnify { value } => assert!(value.is_finite() && (MIN_ZOOM..=MAX_ZOOM).contains(&value), "{value}"),
                _ => {}
            }
        }
    }

    /// Failure: closing the window leaves ops queued with nobody to take them. Each `Attach` holds a surface's memory until the window's back, and `qa wait-settled` waits for ever. They're gone, and the surfaces with them, the moment it's closed.
    #[test]
    fn a_closed_window_keeps_no_ops_or_surfaces_for_them() {
        let mut reader = reader_on(10);
        let draw = a_draw_job(&mut reader, DrawKind::Preview);
        let JobKind::Draw { surface, .. } = &draw.kind else { panic!() };
        let held = surface.clone();
        reader.finish(draw, Outcome::Drawn { kind: TileKind::Colour, low_res: None });
        // Attached in Rust, and waiting for a host that hasn't come for it.
        assert!(reader.ops.iter().any(|op| matches!(op, Op::Attach { surface, .. } if *surface != 0)), "{:?}", reader.ops);
        let before = held.reference_count();
        reader.window_closed();
        assert!(reader.ops.is_empty(), "{:?}", reader.ops);
        assert!(held.reference_count() < before - 1, "the entry's and the waiting op's both gone: {} from {before}", held.reference_count());
        assert!(reader.is_settled(), "{}", reader.report().0);
    }

    /// Failure: a surface goes out to the engine with a tile's lock on the reader, as making one is what the plan's for. A plan can be made with no surface, and a region's rows are kept to the cap only once it has one.
    #[test]
    fn a_plan_makes_its_surface_after_the_lock() {
        let mut reader = reader_on(10);
        loop {
            let plan = reader.next_plan().expect("a plan");
            let is_draw = matches!(plan.kind, PlanKind::Draw { .. });
            let job = plan.ready().expect("made");
            if is_draw {
                assert!(matches!(job.kind, JobKind::Draw { .. }));
                break;
            }
            let outcome = if matches!(job.kind, JobKind::Size) { Outcome::Size(letter()) } else { Outcome::Heavy(false) };
            reader.finish(job, outcome);
        }
    }

    /// Failure: each way of zooming has its own step and limits, or one goes past them, or "actual size" is the same as "fit".
    #[test]
    fn every_way_of_zooming_is_one_path_with_one_set_of_limits() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        assert_eq!(reader.zoom(Zoom::In), Some(ZOOM_STEP));
        assert!(reader.take_ops().iter().any(|op| matches!(op, Op::Magnify { value } if *value == ZOOM_STEP)));
        assert_eq!(reader.zoom(Zoom::Out), Some(1.0));
        assert_eq!(reader.zoom(Zoom::To(1e9)), Some(MAX_ZOOM));
        assert_eq!(reader.zoom(Zoom::To(0.0)), Some(MIN_ZOOM));
        assert_eq!(reader.zoom(Zoom::To(f64::NAN)), Some(MIN_ZOOM), "a nonsense number changes nothing");
        assert_eq!(reader.zoom(Zoom::Fit), Some(1.0));
        let actual = reader.zoom(Zoom::Actual).unwrap();
        assert!((actual * reader.doc().unwrap().layout.fit() - 1.0).abs() < 1e-9 || actual == MIN_ZOOM || actual == MAX_ZOOM, "{actual}");
        assert!(zoom_after_scroll(1.0, 100.0) > 1.0 && zoom_after_scroll(1.0, -100.0) < 1.0, "upwards is in");
        assert_eq!(zoom_after_scroll(1.0, 1e9), MAX_ZOOM);
        assert_eq!(zoom_after_scroll(1.0, -1e9), MIN_ZOOM);
        assert_eq!(zoom_after_scroll(2.0, f64::NAN), 2.0);
    }

    /// Failure: the zoom level says one thing and does another: 100% isn't the page's real size, a typed level goes past the limits, something that isn't a number zooms, or (review of f05c998, finding 6) `1,000` typed with a Mac set to `.` is 1%. Owner, 2026-11-07: "clicking the zoom value enables you to type in zoom level or reset".
    #[test]
    fn a_zoom_level_is_a_share_of_the_real_size() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        let fit = reader.doc().unwrap().layout.fit();
        let actual = reader.zoom(Zoom::Percent(100.0)).unwrap();
        assert!((actual - reader.zoom(Zoom::Actual).unwrap()).abs() < 1e-9);
        assert!((zoom_percent(actual, fit) - 100.0).abs() < 1e-9);
        assert!((zoom_percent(1.0, fit) - fit * 100.0).abs() < 1e-9, "the fitted width says what it is");
        assert_eq!(reader.zoom(Zoom::Percent(1e6)), Some(MAX_ZOOM));
        assert_eq!(reader.zoom(Zoom::Percent(0.001)), Some(MIN_ZOOM));
        for (typed, decimal, wanted) in [("150", '.', 150.0), ("150%", '.', 150.0), (" 87.5 % ", '.', 87.5), ("1,000", '.', 1000.0), ("1 000", '.', 1000.0), ("80,5", ',', 80.5), ("1.000", ',', 1000.0), ("1.250,5 %", ',', 1250.5)] {
            assert_eq!(typed_zoom(typed, decimal), Some(Zoom::Percent(wanted)), "{typed} with {decimal}");
        }
        for nonsense in ["", "%", "abc", "-50", "0", "NaN", "inf", "1e400", "9999999999999", "1%%"] {
            assert_eq!(typed_zoom(nonsense, '.'), None, "{nonsense}");
        }
    }

    /// Failure: the host's size change moves the reader away from what they were reading.
    #[test]
    fn a_resize_keeps_the_reader_on_their_page() {
        let mut reader = reader_on(100);
        settle(&mut reader);
        let y = reader.doc().unwrap().layout.top(40);
        reader.set_viewport(view(y));
        settle(&mut reader);
        let ops = reader.set_viewport(Viewport { frame_width: 900.0, width: 900.0, ..view(y) });
        let Some(Op::ScrollTo { y: new_y, .. }) = ops.iter().find(|op| matches!(op, Op::ScrollTo { .. })) else { panic!("{ops:?}") };
        let top = reader.doc().unwrap().layout.top(40);
        assert!((*new_y - top).abs() < 1.0, "{new_y} against {top}");
    }
}
