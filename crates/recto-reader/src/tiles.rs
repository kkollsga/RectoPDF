//! What's wanted on screen, what's kept, and what the host is told. All the policy for the page view, as plain state: the UI thread calls [`Reader::set_viewport`] on every scroll, so nothing here waits. The one lock
//! around this is never held across a render or a disk read, which [`crate::engine`] does outside it. Numbers are from the platform report, §2.6.
//!
//! **Three levels on a page,** so a page seen once never goes blank: a preview at a quarter of the fitted scale, tiles of another zoom, and exact tiles. A page shows what it has, and when its visible tiles are all there, the
//! rest come off and go discardable.
//!
//! **What's in memory.** Only tiles on layers count. Anything off-screen is marked discardable the moment the host says it's off the layer: it leaves the footprint at once, and is usually still there on the way back.

use crate::layout::{PageBox, PageLayout, Spread};
use crate::paper::PaperKind;
use crate::selection::{Notice, Selecting, Urgency};
use recto_index::select::{PagePoint, Range};
use recto_platform::FileStamp;
use recto_platform::surface::{PixelSurface, Purge};
use recto_proto::{BYTES_PER_PIXEL, MAX_REGION_BYTES, MAX_SCALE, MIN_SCALE, TILE_EDGE, TileRect, device_size};
use recto_render_service::{CancelHandle, LowResWhy, TileKind, plan_regions};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod zoom;

pub use zoom::{MAX_ZOOM, MIN_ZOOM, ZOOM_STEP, Zoom, typed_zoom, zoom_after_scroll, zoom_percent};

/// Tiles attached in one turn of the UI thread. Platform report §2.6: it stays under budget, and the next turn takes the rest.
pub const MAX_ATTACH_PER_TURN: usize = 8;
/// How far from where it was sent the host can put the view, on its own pixels: half a point at 2x, a point at 1x. Within it, the view is still on the place.
const HOST_ROUNDING: f64 = 1.0;
/// Files remembered as needing a password the renderer has none for, until the app asks: no more than tabs that could be showing.
const MAX_KEYLESS: usize = 8;
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
/// Pages either side of what's visible whose sizes are asked for, so layout near the reader settles early.
const SIZES_AROUND: u32 = 2;

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
    /// The document's size, and how far the view may scroll past its top and its end, which is the rest of the margin at each end ([`crate::layout::PageLayout::end_room`]).
    Document {
        width: f64,
        height: f64,
        top_room: f64,
        bottom_room: f64,
    },
    Magnify {
        value: f64,
    },
    ScrollTo {
        y: f64,
        seq: u64,
    },
    /// Across, when zoomed in. With no jump of its own: a view going across isn't a place to wait for.
    ScrollAcross {
        x: f64,
    },
    /// A page's layer, placed or moved. Its own coordinates are PDF points from its top left, as the page is before it's turned, and `fit` scales them to the document's. `turns` quarter turns clockwise turn it, and `x` and `y` are where its turned top left lands.
    ShowPage {
        page: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        fit: f64,
        paper_rgb: u32,
        turns: u8,
    },
    HidePage {
        page: u32,
    },
    /// A tile on a page. `surface` is an extra reference that's now the host's, and zero if `rgb` says the colour. `crop_*` is how much of the surface it uses.
    Attach {
        id: u64,
        page: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        surface: u64,
        rgb: Option<u32>,
        crop_width: f64,
        crop_height: f64,
        below: bool,
    },
    Detach {
        id: u64,
    },
    /// What's selected on a page, in its own coordinates, all of it. Nothing clears it. It's a layer of the page's, so it scrolls and zooms with it and no tile knows.
    Highlight {
        page: u32,
        rects: Vec<recto_index::select::Rect>,
    },
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
    /// For each file of `files`, what it was when its pages were added, for the ones that aren't the tab's own: a page isn't drawn from an added PDF that's since changed, as its page numbers are about the version it was. Shorter than `files`, or `None`, says nothing.
    pub expected: Vec<Option<FileStamp>>,
    /// The version of the tab's own file a save moved it on from, if this is the version that save wrote: each of these pages is the page that was shown at its position, as it was shown, turned or added. What was laid out, where the reader was and what was selected go on as they were; only what's drawn is of the new file.
    pub follows: Option<FileStamp>,
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
    /// **What it showed when it was left,** a picture of each page in view then, for its sleep file. What it left on screen is marked discardable and the system takes that back whenever it needs the memory, without a word, so without these a tab put to sleep after that slept with no picture and woke to bare paper. Taken while it's still on the screen, so it's all there. At most [`MAX_LEFT_PICTURE_BYTES`] for each of the few tabs held off screen, the session's warm ones. Gone when it's shown again, its file's another version, the system says memory's short, or the window closes.
    left_pictures: Option<Vec<RawPicture>>,
    /// The document's width as the host was last told it: the window's, or a fold-out's while its row is in view.
    width_shown: f64,
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
/// What a tab held off screen keeps of the pictures it was left with, in all. A letter page at a Retina window's fit is 730 KiB, so a page to a row has its view kept, and a view of many small pages keeps the first few: the rest come from what's here when it goes to sleep, as before.
const MAX_LEFT_PICTURE_BYTES: usize = 4 * 1024 * 1024;

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
    Draw {
        scale: f32,
        rect: TileRect,
        surface: PixelSurface,
        what: DrawKind,
        paper: PaperKind,
    },
    /// Read a page's text for selecting. `check` also draws it, for the hidden-text check.
    Text { check: bool },
    /// Read a page's links, from the file's structure, with no worker.
    Links,
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
    Draw {
        scale: f32,
        rect: TileRect,
        what: DrawKind,
        paper: PaperKind,
    },
    Text {
        check: bool,
    },
    Links,
}

impl JobPlan {
    /// Makes its surface. A new one for each job: see [`Self::take_surface`]. `None` if it can't be made, which leaves things as if there'd been nothing to do.
    pub fn ready(self) -> Option<Job> {
        let Self {
            tab,
            position,
            file,
            source_page,
            generation,
            kind,
        } = self;
        let kind = match kind {
            PlanKind::Size => JobKind::Size,
            PlanKind::Traits => JobKind::Traits,
            PlanKind::Text { check } => JobKind::Text { check },
            PlanKind::Links => JobKind::Links,
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
                JobKind::Draw {
                    scale,
                    rect,
                    surface,
                    what,
                    paper,
                }
            }
        };
        Some(Job {
            tab,
            position,
            file,
            source_page,
            generation,
            kind,
        })
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
    Drawn {
        kind: TileKind,
        low_res: Option<(TileRect, LowResWhy)>,
    },
    Cancelled,
    /// The page can't be drawn now or ever, in these words.
    PageFailed {
        message: String,
        may_pass: bool,
    },
    /// The file can't be drawn, in these words. `needs_key`: because it needs a password the renderer wasn't given.
    DocumentFailed {
        message: String,
        needs_key: bool,
    },
    /// A page's text, for selecting. `checked` says a hidden-text check was asked for and has been tried, however it came out, and not stopped because nobody wanted it.
    Text {
        text: Arc<recto_index::select::PageText>,
        checked: bool,
    },
    /// A page's text can't be read. That's the page's text, and nothing about drawing it.
    TextFailed,
    /// A page's links, or none if they couldn't be read.
    Links(Option<Arc<recto_core::nav::PageLinks>>),
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
    /// What was selected on the tab showing when a renderer saw its file replaced, until the next activation says whether that was a save of what was shown.
    selection_before_change: Option<(u32, recto_index::select::Range)>,
    /// Find's hit, selected and on its way into view once its page's text says where on the page it is: the tab, the hit, and whether its page was jumped to for it, which then places it whatever it shows.
    revealing: Option<(u32, recto_index::select::Range, bool)>,
    more_to_attach: bool,
    counts: ReaderCounts,
    wanted_files: Arc<Vec<PathBuf>>,
    note: Option<String>,
    selecting: Selecting,
    /// Links on the tab showing, read as they're pointed at.
    linking: crate::links::Linking,
    /// The thread that draws has failed too many times to go on trying, and this says so.
    gave_up: bool,
    /// What's come from a tab's sleep file, for when it's made the hot tab.
    sleep_view: Option<(u32, SleepView)>,
    /// Files whose renderers have the wrong version open, to be let go of and opened again.
    reopen_files: Vec<PathBuf>,
    /// Files the renderer couldn't open for want of a password, which the app has none for: a protected file moved to another volume, or replaced, while it was open. The app asks for it again. At most [`MAX_KEYLESS`].
    keyless: Vec<PathBuf>,
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
    /// The last viewport was the host arriving at a scroll it was sent, as a change of layout sends one to keep the place. Where it arrived is the place, or as near as the view reaches, so the place isn't taken again from it.
    arrived: bool,
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
            viewport: Viewport {
                magnification: 1.0,
                backing: 2.0,
                ..Viewport::default()
            },
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
            selection_before_change: None,
            revealing: None,
            more_to_attach: false,
            counts: ReaderCounts::default(),
            wanted_files: Arc::new(Vec::new()),
            note: None,
            selecting: Selecting::default(),
            linking: crate::links::Linking::default(),
            gave_up: false,
            sleep_view: None,
            reopen_files: Vec::new(),
            keyless: Vec::new(),
            from_sleep: HashSet::new(),
            window_up: false,
            hold_drawing: false,
            waking: None,
            last_wake: None,
            share: 1,
            user_scrolled: false,
            arrived: false,
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
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else {
            return;
        };
        if doc.layout.set_margins(left, right) {
            doc.exact_y = None;
            self.lay_out();
            self.refresh(MAX_ATTACH_PER_TURN);
        }
    }

    /// The last scroll the host said it had reached, for a test to go on from.
    #[cfg(test)]
    pub(crate) fn viewport_scrolled_to(&self) -> u64 {
        self.viewport.scrolled_to
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
            self.waking = Some((
                began,
                WakeReport {
                    tab,
                    from_sleep: false,
                    ..WakeReport::default()
                },
            ));
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
        let tabs: Vec<u32> = self
            .docs
            .values()
            .filter(|doc| doc.map.files.iter().any(|known| known == file))
            .map(|doc| doc.tab)
            .collect();
        for tab in tabs {
            let Some(doc) = self.docs.get_mut(&tab) else { continue };
            let was = doc.versions.insert(file.to_path_buf(), version);
            if was == Some(version) {
                continue;
            }
            // Of the version it was left with.
            doc.left_pictures = None;
            let stale: Vec<u64> = self
                .entries
                .values()
                .filter(|entry| entry.tab == tab && !matches!((entry.version, was), (Some(drawn), _) if drawn == version))
                .filter(|entry| {
                    was.is_some()
                        || entry.version.is_some_and(|drawn| drawn != version)
                        || entry.version.is_none() && entry.preview && self.from_sleep.contains(&entry.id)
                })
                .map(|entry| entry.id)
                .collect();
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

    /// Files the renderer found need a password it has none for, since this was last asked. Asking clears them.
    pub fn take_keyless(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.keyless)
    }

    /// **A password's been given for a file:** its renderer opens again with it, and what an open without it found, that it needs one, is forgotten, so its pages are asked for afresh.
    pub fn key_given(&mut self, path: &std::path::Path) {
        self.keyless.retain(|file| file != path);
        self.reopen_files.push(path.to_path_buf());
        for doc in self.docs.values_mut() {
            if doc.map.files.iter().any(|file| file == path) {
                doc.problem = None;
                doc.issues.clear();
            }
        }
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// For QA: nothing's drawn till it's let go, so a first frame stays to be looked at.
    pub fn hold_drawing(&mut self, on: bool) {
        self.hold_drawing = on;
    }

    /// What a file was when its pages were added to this tab, if they were. Only this tab's: the same file can be another tab's own, which goes by its version, and saving that tab mustn't stop its own pages drawing.
    pub fn expected_stamp(&self, tab: u32, file: &std::path::Path) -> Option<FileStamp> {
        let doc = self.docs.get(&tab)?;
        doc.map
            .files
            .iter()
            .position(|known| known == file)
            .filter(|at| *at > 0)
            .and_then(|at| doc.map.expected.get(at).copied().flatten())
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
            let y = doc
                .exact_y
                .map_or_else(|| doc.left_at.map_or(0.0, |(_, anchor)| doc.layout.anchor_y(anchor)), |(y, _)| y);
            // Its own view: the window's height at the zoom it was left at, which the tab showing's view, at the tab showing's zoom, isn't.
            let window_height = self.viewport.height * self.viewport.magnification.max(f64::MIN_POSITIVE);
            let height = (window_height / doc.magnification.clamp(MIN_ZOOM, MAX_ZOOM)).max(1.0);
            if doc.layout.count() == 0 {
                Vec::new()
            } else {
                doc.layout.pages_between(y.max(0.0), y.max(0.0) + height).collect()
            }
        };
        // What it was left with first, page by page: what's here now may have been taken by the system since.
        let mut left = doc.left_pictures.clone().unwrap_or_default();
        let mut pictures = Vec::new();
        for page in pages.into_iter().take(MAX_SLEEP_SHOTS) {
            if version.is_none() {
                break;
            }
            if let Some(at) = left.iter().position(|picture| picture.page == page) {
                pictures.push(left.swap_remove(at));
            } else if let Some(picture) = self.picture_of(tab, page) {
                pictures.push(picture);
            }
        }
        Some(SleepMaterial {
            version,
            usual,
            sizes,
            pictures,
        })
    }

    /// A picture of each page of the tab showing that's in view, while it's on the screen. Nothing if no worker's said which version it's of, as there'd be no saying what they're of.
    fn pictures_in_view(&mut self, tab: u32) -> Vec<RawPicture> {
        let Some(doc) = self.docs.get(&tab) else { return Vec::new() };
        if doc.map.files.first().and_then(|file| doc.versions.get(file)).is_none() {
            return Vec::new();
        }
        let pages: Vec<u32> = self.visible_pages(doc).take(MAX_SLEEP_SHOTS).collect();
        let mut pictures: Vec<RawPicture> = Vec::new();
        let mut bytes = 0;
        for page in pages {
            let Some(picture) = self.picture_of(tab, page) else { continue };
            bytes += picture.bgra.len();
            if bytes > MAX_LEFT_PICTURE_BYTES {
                break;
            }
            pictures.push(picture);
        }
        pictures
    }

    fn picture_of(&mut self, tab: u32, page: u32) -> Option<RawPicture> {
        let paper = self.paper;
        let doc = self.docs.get(&tab)?;
        let preview_scale = ((doc.layout.fit() * self.viewport.backing / PREVIEW_DIVISOR) as f32).clamp(MIN_SCALE, MAX_SCALE);
        let size = doc.layout.raw_size(page);
        let paper_rgb = paper.rgb();
        // A preview as it is, if there's one.
        let preview = self
            .cover
            .get(&self.preview_key(doc, page))
            .copied()
            .filter(|id| self.entries.get(id).is_some_and(|entry| entry.version.is_some() && self.is_current(entry)));
        if let Some(id) = preview
            && let Some(picture) = self.copy_preview(id)
        {
            return Some(picture);
        }
        // Otherwise the sharpest tiles there are of this page, at whole steps down.
        let ids: Vec<u64> = self
            .entries
            .values()
            .filter(|entry| {
                entry.tab == tab
                    && entry.page == page
                    && !entry.preview
                    && entry.paper == paper
                    && entry.surface.is_some()
                    && entry.low_res.is_none()
                    && entry.version.is_some()
                    && self.is_current(entry)
            })
            .map(|entry| entry.id)
            .collect();
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
            *pixel = [
                (paper_rgb & 0xFF) as u8,
                ((paper_rgb >> 8) & 0xFF) as u8,
                ((paper_rgb >> 16) & 0xFF) as u8,
                0xFF,
            ];
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
                let copied = surface.with_pixels(|source| {
                    shrink_into(
                        source,
                        stride,
                        used_width,
                        used_height,
                        step,
                        (rect.x / step, rect.y / step),
                        &mut bgra,
                        width,
                        height,
                    )
                });
                any |= copied.is_ok();
                if was_discardable {
                    surface.set_discardable();
                    entry.discardable = true;
                }
            }
        }
        any.then_some(RawPicture {
            page,
            scale,
            width,
            height,
            paper,
            bgra,
        })
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
        let copied = usable
            .then(|| {
                surface
                    .with_pixels(|source| {
                        let mut bgra = Vec::with_capacity(row * rect.height as usize);
                        for line in source.chunks_exact(stride).take(rect.height as usize) {
                            bgra.extend_from_slice(line.get(..row)?);
                        }
                        (bgra.len() == row * rect.height as usize).then_some(bgra)
                    })
                    .ok()
                    .flatten()
            })
            .flatten();
        if was_discardable {
            surface.set_discardable();
            entry.discardable = true;
        }
        Some(RawPicture {
            page,
            scale,
            width: rect.width,
            height: rect.height,
            paper,
            bgra: copied?,
        })
    }

    /// Everything that can go without the page on screen going blank: what's off the layers, and at the worst, every picture too. Those that are discardable may have gone anyway, so this is what the system's asked of us, with nothing kept for later.
    pub fn memory_pressure(&mut self, critical: bool) {
        // Short enough that previews go: so do the pictures tabs were left with. Those that could go to sleep have, with them, by now.
        if critical {
            for doc in self.docs.values_mut() {
                doc.left_pictures = None;
            }
        }
        let ids: Vec<u64> = self
            .entries
            .values()
            .filter(|entry| !entry.attached && (critical || !entry.preview))
            .map(|entry| entry.id)
            .collect();
        for id in ids {
            self.forget_entry(id);
        }
    }

    /// For QA: what the system does when it takes back memory marked discardable, with no word to the app. Every surface marked so is emptied. Says how many.
    pub fn empty_discardable(&mut self) -> usize {
        let mut emptied = 0;
        for entry in self.entries.values().filter(|entry| entry.discardable) {
            if let Some(surface) = &entry.surface {
                surface.empty_now();
                emptied += 1;
            }
        }
        emptied
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
        // What was selected when a renderer saw the file replaced, for a save that turns out to have written what was shown.
        let selected_before_change = self.selection_before_change.take().filter(|(of, _)| *of == tab).map(|(_, range)| range);
        if staying && let Some(doc) = self.docs.get_mut(&tab) {
            // Across the page, as the view has it now: what's taken up again goes back there.
            doc.scroll_x = self.viewport.x;
        }
        if self.hot != Some(tab) {
            // While what it showed is still on the screen, and so all there.
            if let Some(left) = self.hot.filter(|hot| self.docs.contains_key(hot)) {
                let pictures = self.pictures_in_view(left);
                if let Some(doc) = self.docs.get_mut(&left) {
                    doc.left_pictures = Some(pictures);
                }
            }
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
            self.linking.clear();
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
            let magnification = saved.map_or(1.0, |saved| {
                if saved.magnification.is_finite() {
                    saved.magnification.clamp(MIN_ZOOM, MAX_ZOOM)
                } else {
                    1.0
                }
            });
            let within = |value: f64| if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
            let left_at = saved.map(|saved| (saved.page, (saved.anchor.0.min(pages.saturating_sub(1)), within(saved.anchor.1))));
            let scroll_x = saved.map_or(0.0, |saved| if saved.x.is_finite() { saved.x.max(0.0) } else { 0.0 });
            let exact_y = saved
                .filter(|saved| saved.y.is_finite() && saved.y >= 0.0 && saved.document_height.is_finite())
                .map(|saved| (saved.y, saved.document_height));
            DocView {
                tab,
                layout,
                map: map.clone(),
                magnification,
                anchor: (page, 0.0),
                tile_scale: 0.0,
                issues: HashMap::new(),
                heavy: HashMap::new(),
                problem: None,
                seen: HashSet::new(),
                left_at,
                scroll_x,
                exact_y,
                floor,
                size_on_trust: false,
                versions: HashMap::new(),
                left_pictures: None,
                width_shown: 0.0,
            }
        });
        // A save wrote the pages that were shown, each as it was shown: another version of the file, with nothing on the screen to move.
        let carried = !doc.map.same_pages(&map) && map.follows.is_some() && doc.map.own == map.follows && doc.layout.count() == pages;
        // Back on the page it was left on, and it's back at the same place in that page. A tall page's reader isn't sent to its top.
        let same_pages = (doc.map.same_pages(&map) || carried) && doc.layout.count() == pages;
        let back_at = if staying && same_pages {
            Some(doc.anchor)
        } else {
            doc.left_at.take().filter(|(left, _)| *left == page && same_pages).map(|(_, anchor)| anchor)
        };
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
        let mut carried_selection = None;
        if !doc.map.same_pages(&map) || doc.layout.count() != pages {
            // Another version of its file: a worker that's running has the old one open, and goes on drawing that.
            if doc.map.own != map.own {
                self.reopen_files.extend(map.files.first().cloned());
            }
            if carried {
                // **A save leaves the view as it was** (owner: "perfect" state recovery): the pages keep their sizes as shown, so the fit, the zoom shown, the place and the layout don't move. Only what's drawn is of the new file, and is drawn again.
                doc.layout = doc.layout.as_saved();
                carried_selection = self.selecting.range().or(selected_before_change);
            } else {
                doc.layout = PageLayout::with_spread(pages, spread);
            }
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
            // Its pages are other pages now, so what was selected on them isn't there. After a save they're the same pages, so it is, and its text is read again from the new file.
            self.selecting.reset();
            self.linking.clear();
            if let Some(range) = carried_selection {
                self.selecting.set_range(range);
            }
        }
        // On the screen again: it's drawn again, and what it was left with is out of date as soon as it moves.
        if let Some(doc) = self.docs.get_mut(&tab) {
            doc.left_pictures = None;
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
            let rect = TileRect {
                x: 0,
                y: 0,
                width: shot.surface.width(),
                height: shot.surface.height(),
            };
            let id = self.keep_drawn(
                tab,
                shot.page,
                shot.scale,
                rect,
                shot.surface,
                DrawKind::Preview,
                shot.paper,
                TileKind::Colour,
                None,
            );
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

    /// Exactly where each tab it holds is, for the session to keep. Hot and warm tabs: a few at most, and a cold tab's is already kept.
    pub fn places(&self) -> Vec<(u32, SavedPlace)> {
        self.docs
            .values()
            .filter_map(|doc| {
                let page_size = doc.layout.usual_size();
                if self.hot == Some(doc.tab) {
                    let page = self.last_view_page?;
                    // Not yet arrived where it's been sent: what the view says is about somewhere else.
                    let (x, y) = if self.jump.is_some() {
                        (doc.scroll_x, doc.exact_y.map_or_else(|| doc.layout.anchor_y(doc.anchor), |(y, _)| y))
                    } else {
                        (self.viewport.x, self.viewport.y)
                    };
                    Some((
                        doc.tab,
                        SavedPlace {
                            page,
                            anchor: doc.anchor,
                            x,
                            y,
                            document_height: doc.layout.document_height(),
                            magnification: doc.magnification,
                            page_size,
                        },
                    ))
                } else {
                    let (page, anchor) = doc.left_at?;
                    let y = doc.exact_y.map_or_else(|| doc.layout.anchor_y(anchor), |(y, _)| y);
                    Some((
                        doc.tab,
                        SavedPlace {
                            page,
                            anchor,
                            x: doc.scroll_x,
                            y,
                            document_height: doc.layout.document_height(),
                            magnification: doc.magnification,
                            page_size,
                        },
                    ))
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
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else {
            return;
        };
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
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else {
            return;
        };
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
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else {
            return;
        };
        doc.layout.set_margins(self.margins.0, self.margins.1);
        doc.layout.set_frame(self.viewport.frame_width);
        let (top_room, bottom_room) = doc.layout.end_room();
        let y = Self::place_y(doc);
        doc.width_shown = doc.layout.document_width_for(y, y + self.viewport.height);
        self.ops.push_back(Op::Document {
            width: doc.width_shown,
            height: doc.layout.document_height(),
            top_room,
            bottom_room,
        });
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
        self.user_scrolled = !viewport.live
            && self.jump.is_none()
            && viewport.y != was.y
            && viewport.scrolled_to == was.scrolled_to
            && viewport.magnification == was.magnification
            && viewport.frame_width == was.frame_width
            && viewport.height == was.height;
        self.arrived = viewport.scrolled_to > was.scrolled_to && viewport.scrolled_to != u64::MAX;
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
            self.widen_for_view();
        }
        // After the refresh, which is what moves the tiles' scale on.
        self.refresh(MAX_ATTACH_PER_TURN);
        self.cancel_if_unwanted();
        self.count_blanks();
        self.take_ops()
    }

    /// The document as wide as the rows in view need, which is the window's unless a fold-out's row is in view: told to the host when that changes, as the view reaches a fold-out or leaves it.
    fn widen_for_view(&mut self) {
        let viewport = self.viewport;
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else {
            return;
        };
        let width = doc.layout.document_width_for(viewport.y, viewport.y + viewport.height);
        if (width - doc.width_shown).abs() < 0.5 {
            return;
        }
        doc.width_shown = width;
        let (top_room, bottom_room) = doc.layout.end_room();
        self.ops.push_back(Op::Document {
            width,
            height: doc.layout.document_height(),
            top_room,
            bottom_room,
        });
    }

    /// Follows the reader's place. Only a change the reader made is the session's business: arriving where a jump sent them isn't.
    fn track_place(&mut self) {
        let viewport = self.viewport;
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else {
            return;
        };
        let bottom = doc.layout.document_height() - viewport.height;
        let page = if viewport.y >= bottom - 1.0 && viewport.y > 0.0 {
            doc.layout.count().saturating_sub(1)
        } else {
            doc.layout.page_at(viewport.y + viewport.height / 3.0)
        };
        // Side by side, the reader's page stays whichever of its row they were on: a jump to a right-hand page, or reading on in a row, isn't a move to the left one.
        let page = match self.last_view_page {
            Some(was) if doc.layout.same_row(was, page) => was,
            // The host arrived where a change of layout sent it, back to the place: the page the place was kept on, if it's in the row. A round trip through a view too small to reach the place, where the last page was taken for the reader's, otherwise lands on the row's left page (side-by-side review, finding 4).
            _ if self.arrived && doc.layout.same_row(doc.anchor.0, page) => doc.anchor.0,
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
        // Arriving where a change of layout sent it, the place stays as it was kept, even where the view couldn't reach it, as at the foot of a document smaller than the view.
        let moved = !self.arrived && doc.exact_y.is_none_or(|(y, _)| y != viewport.y);
        if moved {
            // The host puts the view on its own pixels, so a view this near where the place is is on it, said the host's way. Taken again from the rounded height, the top of a page would be the foot of the one above, from the gap between them.
            if (doc.layout.anchor_y(doc.anchor) - viewport.y).abs() >= HOST_ROUNDING {
                doc.anchor = doc.layout.anchor_for(viewport.y, page);
            }
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
                    && (running.preview
                        || running
                            .rect
                            .is_none_or(|rect| running.scale == doc.tile_scale && self.touches_view(doc, running.position, rect, running.scale)))
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
        if self
            .selecting
            .wanted(Urgency::Now, &visible, doc.layout.count(), |page| {
                doc.heavy.get(&page).copied().unwrap_or(false)
            })
            .is_some()
        {
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
        doc.layout
            .pages_between(self.viewport.y.max(0.0), self.viewport.y.max(0.0) + self.viewport.height)
    }

    /// Whether a device rectangle of a page at a scale shows in the viewport, however the page is turned.
    fn touches_view(&self, doc: &DocView, page: u32, rect: TileRect, scale: f32) -> bool {
        let per_point = 1.0 / f64::from(scale);
        let (left, top) = (f64::from(rect.x) * per_point, f64::from(rect.y) * per_point);
        let (right, bottom) = (left + f64::from(rect.width) * per_point, top + f64::from(rect.height) * per_point);
        let corners = [(left, top), (right, top), (left, bottom), (right, bottom)].map(|(u, v)| doc.layout.from_page(page, u, v));
        let (x0, x1) = corners
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), (x, _)| (low.min(*x), high.max(*x)));
        let (y0, y1) = corners
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), (_, y)| (low.min(*y), high.max(*y)));
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
        let corners = [
            (view_x, view_y),
            (view_x + view_w, view_y),
            (view_x, view_y + view_h),
            (view_x + view_w, view_y + view_h),
        ]
        .map(|(x, y)| doc.layout.to_page(page, x, y));
        let per_point = f64::from(scale);
        let (low_u, high_u) = corners
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), (u, _)| (low.min(*u), high.max(*u)));
        let (low_v, high_v) = corners
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), (_, v)| (low.min(*v), high.max(*v)));
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
        TileKey {
            tab: doc.tab,
            page,
            scale_bits: scale.to_bits(),
            column,
            row,
            paper: self.paper,
            preview: false,
        }
    }

    fn preview_key(&self, doc: &DocView, page: u32) -> TileKey {
        TileKey {
            tab: doc.tab,
            page,
            scale_bits: 0,
            column: 0,
            row: 0,
            paper: self.paper,
            preview: true,
        }
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
        let on_paper: Vec<u32> = if doc.size_on_trust {
            self.visible_pages(doc).collect()
        } else {
            visible.clone()
        };

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
            placements.push((
                page,
                Placement {
                    x: snap(rect.x),
                    y: snap(rect.y),
                    width: f64::from(size.width),
                    height: f64::from(size.height),
                    fit: doc.layout.fit(),
                    paper_rgb,
                    turns: doc.layout.turns_of(page),
                },
            ));
        }
        for (page, placement) in placements {
            if self.shown.get(&page) != Some(&placement) {
                self.shown.insert(page, placement);
                self.ops.push_back(Op::ShowPage {
                    page,
                    x: placement.x,
                    y: placement.y,
                    width: placement.width,
                    height: placement.height,
                    fit: placement.fit,
                    paper_rgb: placement.paper_rgb,
                    turns: placement.turns,
                });
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
                let done: Vec<u64> = self
                    .attached
                    .get(&page)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|id| !wanted.contains(id))
                    .collect();
                for id in done {
                    self.detach(id, page);
                }
            }
            // Tiles scrolled out of a tall page come off, even while others are still coming.
            if !missing {
                continue;
            }
            let Some(doc) = self.docs.get(&tab) else { return };
            let out: Vec<u64> = self
                .attached
                .get(&page)
                .into_iter()
                .flatten()
                .copied()
                .filter(|id| {
                    self.entries.get(id).is_some_and(|entry| {
                        !entry.preview && entry.scale == scale && entry.paper == self.paper && !self.touches_view(doc, page, entry.rect, scale)
                    })
                })
                .collect();
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
                let low = self
                    .attached
                    .get(&page)
                    .into_iter()
                    .flatten()
                    .filter_map(|id| self.entries.get(id))
                    .find_map(|entry| entry.low_res.as_ref());
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
            return vec![TileKey {
                tab: entry.tab,
                page: entry.page,
                scale_bits: 0,
                column: 0,
                row: 0,
                paper: entry.paper,
                preview: true,
            }];
        }
        let mut keys = Vec::new();
        for place in recto_proto::region_tiles(entry.rect, TILE_EDGE) {
            keys.push(TileKey {
                tab: entry.tab,
                page: entry.page,
                scale_bits: entry.scale.to_bits(),
                column: (place.x / TILE_EDGE) as u16,
                row: (place.y / TILE_EDGE) as u16,
                paper: entry.paper,
                preview: false,
            });
        }
        keys
    }

    /// Whether any pixel of a page is in view at the tiles' scale. The view can reach into a page by less than the part of a pixel its tiles leave off at its foot or its right edge, as
    /// [`recto_proto::device_size`] rounds those to whole pixels: then no tile is in view, so [`Self::refresh`] takes what was on the page off it, and nothing of it can be seen. A page whose
    /// size or scale isn't known yet counts as showing.
    fn shows_a_pixel(&self, doc: &DocView, page: u32) -> bool {
        let scale = doc.tile_scale;
        if scale <= 0.0 || !doc.layout.is_known(page) {
            return true;
        }
        !matches!(self.visible_cells(doc, page, scale), Ok(cells) if cells.is_empty())
    }

    /// Counts a step that left a page in view bare: one that shows a pixel, by the same cells drawing goes by, so a sliver of 0.004 points isn't a blank page.
    fn count_blanks(&mut self) {
        let Some(doc) = self.doc() else { return };
        let (mut blank, mut blank_seen) = (false, false);
        for page in self.visible_pages(doc) {
            let has_something = self.attached.get(&page).is_some_and(|ids| !ids.is_empty());
            if !has_something && !doc.issues.contains_key(&page) && self.shows_a_pixel(doc, page) {
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
            let victim = if previews > MAX_KEPT_PREVIEWS / share {
                oldest_preview
            } else if too_many_tiles {
                oldest_tile.or(oldest_preview)
            } else {
                None
            };
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
        for doc in self.docs.values_mut() {
            doc.left_pictures = None;
        }
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
        let waiting_on_issue = |page: &u32| {
            doc.issues
                .get(page)
                .is_some_and(|issue| !issue.try_again_at.is_some_and(|at| Instant::now() >= at))
        };
        if let Some(page) = visible
            .clone()
            .chain(around)
            .find(|page| !doc.layout.is_known(*page) && !waiting_on_issue(page))
        {
            let (file, source_page) = doc.map.source(page)?;
            let generation = self.fresh_generation();
            return Some(JobPlan {
                tab,
                position: page,
                file,
                source_page,
                generation,
                kind: PlanKind::Size,
            });
        }
        let now = Instant::now();
        let scale = doc.tile_scale;
        if scale <= 0.0 {
            return None;
        }
        // The links under the pointer come first: they're quick, and the pointer's waiting on them for its look.
        if let Some(page) = self.linking.next_wanted()
            && let Some(doc) = self.docs.get(&tab)
            && page < doc.layout.count()
            && let Some((file, source_page)) = doc.map.source(page)
        {
            let generation = self.fresh_generation();
            return Some(JobPlan {
                tab,
                position: page,
                file,
                source_page,
                generation,
                kind: PlanKind::Links,
            });
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
                self.note_issue(
                    tab,
                    page,
                    format!(
                        "Page {} is too large to draw at this zoom ({} by {} points).",
                        page + 1,
                        size.width,
                        size.height
                    ),
                    false,
                );
                continue;
            };
            let missing: Vec<(u16, u16)> = cells
                .iter()
                .copied()
                .filter(|cell| {
                    let key = self.key(doc, page, scale, *cell);
                    match self.cover.get(&key).and_then(|id| self.entries.get(id)) {
                        None => true,
                        Some(entry) => entry
                            .low_res
                            .as_ref()
                            .is_some_and(|low| low.may_clear && now.duration_since(low.at) >= TRY_AGAIN_AFTER),
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
                return Some(JobPlan {
                    tab,
                    position: page,
                    file,
                    source_page,
                    generation,
                    kind: PlanKind::Traits,
                });
            }
            let heavy = doc.heavy.get(&page).copied().unwrap_or(false);
            // A heavy page's cost is per pass, so a preview would cost it as much again. Any other page gets one however little of it shows: a page first seen as a sliver at the bottom of the window, with no preview, goes blank when that sliver's tile scrolls off.
            let preview_wanted = !heavy && !self.cover.contains_key(&self.preview_key(doc, page));
            if preview_wanted && let Some((preview_scale, rect)) = self.preview_rect(doc, page, scale) {
                let paper = self.paper;
                let generation = self.fresh_generation();
                return Some(JobPlan {
                    tab,
                    position: page,
                    file,
                    source_page,
                    generation,
                    kind: PlanKind::Draw {
                        scale: preview_scale,
                        rect,
                        what: DrawKind::Preview,
                        paper,
                    },
                });
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
                return Some(JobPlan {
                    tab,
                    position: page,
                    file,
                    source_page,
                    generation,
                    kind: PlanKind::Draw {
                        scale,
                        rect,
                        what: DrawKind::Region,
                        paper,
                    },
                });
            }
            let (column, row) = missing[0];
            let (x, y) = (u32::from(column) * TILE_EDGE, u32::from(row) * TILE_EDGE);
            let rect = TileRect {
                x,
                y,
                width: TILE_EDGE.min(width - x),
                height: TILE_EDGE.min(height - y),
            };
            let paper = self.paper;
            let generation = self.fresh_generation();
            return Some(JobPlan {
                tab,
                position: page,
                file,
                source_page,
                generation,
                kind: PlanKind::Draw {
                    scale,
                    rect,
                    what: DrawKind::Tile,
                    paper,
                },
            });
        }
        self.text_plan(tab, Urgency::WhenIdle)
    }

    /// Reading text for a selection, a copy or the pointer, if any is wanted at this urgency.
    fn text_plan(&mut self, tab: u32, urgency: Urgency) -> Option<JobPlan> {
        let doc = self.docs.get(&tab)?;
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        let wanted = self
            .selecting
            .wanted(urgency, &visible, doc.layout.count(), |page| doc.heavy.get(&page).copied().unwrap_or(false))?;
        let (file, source_page) = doc.map.source(wanted.page)?;
        let generation = self.fresh_generation();
        Some(JobPlan {
            tab,
            position: wanted.page,
            file,
            source_page,
            generation,
            kind: PlanKind::Text { check: wanted.check },
        })
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
        Some(PagePoint {
            page,
            x: u as f32,
            y: v as f32,
        })
    }

    /// Where a point in the document is in the coordinates of `page`, whichever page it's over: past the page's edges if it's off it, for the caller to hold to them. A snapshot's box dragged across a page break stays on the page it was pressed on this way.
    pub fn point_on_given_page(&self, page: u32, x: f64, y: f64) -> Option<PagePoint> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let doc = self.doc()?;
        if page >= doc.layout.count() {
            return None;
        }
        let (u, v) = doc.layout.to_page(page, x, y);
        Some(PagePoint {
            page,
            x: u as f32,
            y: v as f32,
        })
    }

    /// How many pages the tab shown has, edits and all: 0 with none shown.
    pub fn page_count(&self) -> u32 {
        self.doc().map_or(0, |doc| doc.layout.count())
    }

    /// A page's size as it's shown, in its own coordinates, before any unsaved turn: what a point from [`Self::point_on_page`] is held to.
    pub fn page_size_shown(&self, page: u32) -> Option<(f32, f32)> {
        let doc = self.doc()?;
        (page < doc.layout.count()).then(|| {
            let size = doc.layout.raw_size(page);
            (size.width, size.height)
        })
    }

    /// How far a page is shown turned, in quarter turns clockwise: its unsaved turn and the view's together. What a snapshot of it is turned by.
    pub fn page_turns(&self, page: u32) -> u8 {
        self.doc().map_or(0, |doc| doc.layout.turns_of(page))
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
        let point = self.point_on_page(x, y).filter(|point| self.is_on_its_page(*point));
        self.linking.press(if shift { None } else { point }, clicks);
        match self.point_on_page(x, y) {
            Some(point) => self.selecting.press(point, clicks, shift),
            None => self.selecting.clear(),
        }
        self.highlight_ops();
        self.cancel_if_unwanted();
    }

    pub fn selection_drag(&mut self, x: f64, y: f64) {
        self.linking.drag(self.point_on_page(x, y));
        if let Some(point) = self.point_on_page(x, y) {
            self.selecting.drag(point);
            self.highlight_ops();
            self.cancel_if_unwanted();
        }
    }

    pub fn selection_release(&mut self) {
        self.linking.release();
        self.selecting.release();
    }

    /// Whether a point is on its page, and not in the margin or the gap between pages.
    fn is_on_its_page(&self, point: PagePoint) -> bool {
        let Some(doc) = self.doc() else { return false };
        let size = doc.layout.raw_size(point.page);
        point.x >= 0.0 && point.y >= 0.0 && point.x <= size.width && point.y <= size.height
    }

    /// **The link under the pointer,** if its page's links are here. A page not read yet says nothing for now, and is read next.
    pub fn pointer_link(&mut self, x: f64, y: f64) -> Option<crate::links::LinkHit> {
        let point = self.point_on_page(x, y).filter(|point| self.is_on_its_page(*point))?;
        self.linking.at(point)
    }

    /// The link a click has just landed on, taken once.
    pub fn take_clicked_link(&mut self) -> Option<crate::links::LinkHit> {
        self.linking.take_clicked()
    }

    /// Where a file page of the same file as the page at `from` is now, counted as the view counts: what a link on that page means by its page. Nothing if it's not in the document.
    pub fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32> {
        let doc = self.doc()?;
        match &doc.map.slots {
            None => (file_page < doc.layout.count()).then_some(file_page),
            Some(slots) => {
                let (file, _) = *slots.get(from as usize)?;
                slots
                    .iter()
                    .position(|(other, page)| *other == file && *page == file_page)
                    .and_then(|at| u32::try_from(at).ok())
            }
        }
    }

    /// Whether a page of the view is from the tab's own file, and which page of it.
    pub fn own_file_page(&self, position: u32) -> Option<u32> {
        let doc = self.doc()?;
        match &doc.map.slots {
            None => (position < doc.layout.count()).then_some(position),
            Some(slots) => slots.get(position as usize).filter(|(file, _)| *file == 0).map(|(_, page)| *page),
        }
    }

    /// Pages whose links are held, for QA.
    pub fn link_pages_held(&self) -> usize {
        self.linking.pages_held()
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
        let Some(pages) = self.doc().map(|doc| doc.layout.count()).filter(|pages| *pages > 0) else {
            return;
        };
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

    /// **Find's hit:** selects it, as Find on a Mac does, and brings it into view. Its page is gone to at once if none of it shows, and once the page's text is here, which a selection's end asks for first, the hit is put a third of the way down the view. A hit already showing, away from the view's edges, stays where it is, so stepping through hits on one screen doesn't move it. Held to the document's pages.
    pub fn reveal_range(&mut self, range: recto_index::select::Range) {
        let Some(tab) = self.hot else { return };
        let pages = self.doc().map_or(0, |doc| doc.layout.count());
        if range.start.page >= pages || range.end.page >= pages || (range.start.page, range.start.at) > (range.end.page, range.end.at) {
            return;
        }
        self.selecting.set_range(range);
        self.highlight_ops();
        let shows = self.doc().is_some_and(|doc| self.visible_pages(doc).contains(&range.start.page));
        self.revealing = Some((tab, range, !shows));
        if !self.try_reveal() && !shows {
            self.follow(range.start.page, 0.0);
        }
    }

    /// Moves the view to Find's hit if its place on the page is known now. Says whether it's settled, gone there or not needed.
    fn try_reveal(&mut self) -> bool {
        let Some((tab, range, jumped)) = self.revealing else { return true };
        if self.hot != Some(tab) {
            self.revealing = None;
            return true;
        }
        let end = if range.end.page == range.start.page { range.end.at } else { u32::MAX };
        let Some(bounds) = self.selecting.bounds_of(range.start.page, range.start.at, end) else {
            return false;
        };
        self.revealing = None;
        let Some(bounds) = bounds else { return true };
        let Some(doc) = self.doc() else { return true };
        let rect = doc.layout.rect(range.start.page);
        let fit = doc.layout.fit();
        let (top, bottom) = (rect.y + f64::from(bounds.y) * fit, rect.y + f64::from(bounds.y + bounds.height) * fit);
        let (view_top, view_height) = (self.viewport.y, self.viewport.height);
        // Away from the edges: a tenth of the view at the top, a fifth at the bottom, where the page number and a notice sit.
        if !jumped && view_height > 0.0 && top >= view_top + view_height * 0.1 && bottom <= view_top + view_height * 0.8 {
            return true;
        }
        let want_top = (top - view_height / 3.0).max(0.0);
        let within = ((want_top - rect.y) / rect.height.max(1.0)).clamp(0.0, 1.0);
        let page = if want_top < rect.y { doc.layout.page_at(want_top) } else { range.start.page };
        let within = if page == range.start.page { within } else { doc.layout.anchor_at(want_top).1 };
        self.follow(page, within);
        true
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
        if self.revealing.is_some_and(|(_, range, _)| range.start.page == position) {
            self.try_reveal();
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
            self.linking.clear();
            // Kept aside until the session says what the new version is: a save of what was shown keeps it (see [`Self::activate`]), anything else lets it go.
            self.selection_before_change = self.selecting.range().map(|range| (tab, range));
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
        (u64::from(width) * u64::from(height) * u64::from(BYTES_PER_PIXEL) <= MAX_PREVIEW_BYTES)
            .then_some((preview_scale, TileRect { x: 0, y: 0, width, height }))
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
        self.running = Some(Running {
            tab: job.tab,
            position: job.position,
            rect,
            scale,
            preview,
            generation: job.generation,
            cancel,
            cancel_asked: false,
            text,
            check,
        });
    }

    /// What can stop the job now running, once there's something that can.
    pub fn set_cancel(&mut self, cancel: CancelHandle) {
        if let Some(running) = &mut self.running {
            running.cancel = Some(cancel);
        }
    }

    fn note_issue(&mut self, tab: u32, page: u32, message: String, may_pass: bool) {
        if let Some(doc) = self.docs.get_mut(&tab) {
            doc.issues.insert(
                page,
                Issue {
                    message,
                    try_again_at: may_pass.then(|| Instant::now() + TRY_AGAIN_AFTER),
                },
            );
        }
    }

    /// A job has ended, with whatever it found. Returns anything it hands back.
    pub fn finish(&mut self, job: Job, outcome: Outcome) {
        self.running = None;
        self.counts.jobs += 1;
        let mut kept = None;
        let Job {
            tab,
            position,
            kind,
            generation,
            file,
            ..
        } = job;
        if matches!(kind, JobKind::Links) {
            // About the tab showing, as it is, or about nothing.
            match outcome {
                Outcome::Links(links) if self.hot == Some(tab) && !self.docs.get(&tab).is_some_and(|doc| generation < doc.floor) => {
                    self.linking.take(position, links)
                }
                _ => self.linking.cancelled(position),
            }
            return;
        }
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
            (
                JobKind::Draw {
                    scale,
                    rect,
                    surface,
                    what,
                    paper,
                },
                Outcome::Drawn { kind, low_res },
            ) => {
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
            (_, Outcome::DocumentFailed { message, needs_key }) => {
                if let Some(doc) = self.docs.get_mut(&tab) {
                    doc.problem = Some(message);
                }
                if needs_key && !self.keyless.contains(&file) && self.keyless.len() < MAX_KEYLESS {
                    self.keyless.push(file);
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
    fn keep_drawn(
        &mut self,
        tab: u32,
        page: u32,
        scale: f32,
        rect: TileRect,
        surface: PixelSurface,
        what: DrawKind,
        paper: PaperKind,
        kind: TileKind,
        low_res: Option<(TileRect, LowResWhy)>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let preview = what == DrawKind::Preview;
        let drawn_rect = low_res.map_or(rect, |(drawn, _)| drawn);
        let crop = (
            f64::from(drawn_rect.width) / f64::from(surface.width().max(1)),
            f64::from(drawn_rect.height) / f64::from(surface.height().max(1)),
        );
        // A tile of one colour needs no surface, unless it's a whole region's or the only sign of a page.
        let (rgb, surface) = match (kind, what) {
            (TileKind::Blank { rgb }, DrawKind::Tile) if low_res.is_none() => (Some(rgb), None),
            _ => (None, Some(surface)),
        };
        let note = low_res.map(|(_, why)| LowResNote {
            message: why.message(),
            may_clear: why.may_clear(),
            at: Instant::now(),
        });
        self.clock += 1;
        let version = self.version_of_page(tab, page);
        let entry = Entry {
            id,
            tab,
            page,
            scale,
            paper,
            preview,
            rect,
            surface,
            rgb,
            crop,
            low_res: note,
            version,
            attached: false,
            discardable: false,
            last_used: self.clock,
        };
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
        if !self
            .selecting
            .is_settled(&visible, doc.layout.count(), |page| doc.heavy.get(&page).copied().unwrap_or(false))
        {
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
        let attached_bytes: usize = self
            .attached
            .values()
            .flatten()
            .filter_map(|id| self.entries.get(id))
            .filter_map(|entry| entry.surface.as_ref())
            .map(PixelSurface::len)
            .sum();
        let previews_attached = self
            .attached
            .values()
            .flatten()
            .filter_map(|id| self.entries.get(id))
            .filter(|entry| entry.preview)
            .count();
        let low_res_pages: HashSet<u32> = self
            .attached
            .iter()
            .filter(|(_, ids)| ids.iter().filter_map(|id| self.entries.get(id)).any(|entry| entry.low_res.is_some()))
            .map(|(page, _)| *page)
            .collect();
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
            "left_picture_bytes": self.docs.values().filter_map(|doc| doc.left_pictures.as_ref()).flatten().map(|picture| picture.bgra.len()).sum::<usize>(),
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
mod tests;
