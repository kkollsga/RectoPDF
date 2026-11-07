//! What's wanted on screen, what's kept, and what the host is told. All the policy for the page view, as plain state: the UI thread calls [`Reader::set_viewport`] on every scroll, so nothing here waits. The one lock
//! around this is never held across a render or a disk read, which [`crate::engine`] does outside it. Numbers are from the platform report, §2.6.
//!
//! **Three levels on a page,** so a page seen once never goes blank: a preview at a quarter of the fitted scale, tiles of another zoom, and exact tiles. A page shows what it has, and when its visible tiles are all there, the
//! rest come off and go discardable.
//!
//! **What's in memory.** Only tiles on layers count. Anything off-screen is marked discardable the moment the host says it's off the layer: it leaves the footprint at once, and is usually still there on the way back.

use crate::layout::{PAGE_GAP, PageBox, PageLayout};
use crate::paper::PaperKind;
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
/// Free surfaces kept for reuse, all discardable. Platform report §2.6.
const MAX_POOL: usize = 64;
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
    /// A page's layer, placed or moved. Its own coordinates are PDF points from its top left, and `fit` scales them to the document's.
    ShowPage { page: u32, x: f64, y: f64, width: f64, height: f64, fit: f64, paper_rgb: u32 },
    HidePage { page: u32 },
    /// A tile on a page. `surface` is an extra reference that's now the host's, and zero if `rgb` says the colour. `crop_*` is how much of the surface it uses.
    Attach { id: u64, page: u32, x: f64, y: f64, width: f64, height: f64, surface: u64, rgb: Option<u32>, crop_width: f64, crop_height: f64, below: bool },
    Detach { id: u64 },
}

/// Which pages a tab's positions are, once it has been edited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageMap {
    /// The tab's own file first, then any added ones.
    pub files: Vec<PathBuf>,
    /// For each position, which file and which page there. `None` means position and page are the same, in the first file.
    pub slots: Option<Vec<(u16, u32)>>,
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawKind {
    Preview,
    Tile,
    /// One pass, cut on a grid of this many pixels.
    Region,
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
}

#[derive(Debug)]
struct Running {
    tab: u32,
    position: u32,
    rect: Option<TileRect>,
    scale: f32,
    generation: u64,
    cancel: Option<CancelHandle>,
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
}

/// What QA reads.
pub struct ReaderReport(pub Value);

pub struct Reader {
    docs: HashMap<u32, DocView>,
    hot: Option<u32>,
    viewport: Viewport,
    paper: PaperKind,
    focus: bool,
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
    to_discard: Vec<u64>,
    pool: Vec<PixelSurface>,
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
}

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
            focus: false,
            window_open: true,
            entries: HashMap::new(),
            cover: HashMap::new(),
            next_id: 1,
            attached: HashMap::new(),
            shown: HashMap::new(),
            ops: VecDeque::new(),
            unapplied: false,
            to_discard: Vec::new(),
            pool: Vec::new(),
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
        }
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
    pub fn activate(&mut self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32]) {
        self.keep_only(warm, tab);
        if self.hot != Some(tab) {
            self.clear_screen();
            self.hot = Some(tab);
            self.last_view_page = None;
            self.moved_page = None;
        }
        let doc = self.docs.entry(tab).or_insert_with(|| DocView { tab, layout: PageLayout::new(pages), map: map.clone(), magnification: 1.0, anchor: (page, 0.0), tile_scale: 0.0, issues: HashMap::new(), heavy: HashMap::new(), problem: None, seen: HashSet::new() });
        doc.anchor = (page.min(pages.saturating_sub(1)), 0.0);
        if doc.map != map || doc.layout.count() != pages {
            doc.map = map;
            doc.layout = PageLayout::new(pages);
            doc.issues.clear();
            doc.heavy.clear();
            doc.problem = None;
            doc.seen.clear();
            self.drop_tab_entries(tab);
            self.clear_screen();
        }
        let magnification = self.docs.get(&tab).map_or(1.0, |doc| doc.magnification);
        self.ops.push_back(Op::Magnify { value: magnification });
        self.lay_out();
        self.jump_to(page);
        self.recount_wanted_files(warm);
        self.refresh(MAX_ATTACH_PER_TURN);
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
        self.pool.clear();
        self.set_note(None);
    }

    fn drop_tab_entries(&mut self, tab: u32) {
        let ids: Vec<u64> = self.entries.values().filter(|entry| entry.tab == tab).map(|entry| entry.id).collect();
        for id in ids {
            self.forget_entry(id, true);
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
    }

    /// Asks the host to go to a page, as an agent's `goto` or a contents row does.
    pub fn jump_to(&mut self, page: u32) {
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else { return };
        let page = page.min(doc.layout.count().saturating_sub(1));
        doc.anchor = (page, 0.0);
        let y = doc.layout.top(page);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.jump = Some(seq);
        self.last_view_page = Some(page);
        self.ops.push_back(Op::ScrollTo { y, seq });
    }

    /// The document's size, then the place in it. Called when the fit or a page's size has changed.
    fn lay_out(&mut self) {
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else { return };
        doc.layout.set_frame(self.viewport.frame_width, self.focus);
        self.ops.push_back(Op::Document { width: doc.layout.document_width(), height: doc.layout.document_height() });
        let y = doc.layout.anchor_y(doc.anchor);
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
        let mut moved_layout = false;
        if let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) {
            if doc.layout.set_frame(viewport.frame_width, self.focus) && was.frame_width != viewport.frame_width {
                moved_layout = true;
            }
            if !viewport.live {
                doc.magnification = viewport.magnification;
            }
        }
        if moved_layout {
            self.lay_out();
        } else {
            self.track_place();
        }
        self.cancel_if_unwanted();
        self.refresh(MAX_ATTACH_PER_TURN);
        self.count_blanks();
        self.take_ops()
    }

    /// Follows the reader's place. Only a change the reader made is the session's business: arriving where a jump sent them isn't.
    fn track_place(&mut self) {
        let viewport = self.viewport;
        let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) else { return };
        doc.anchor = doc.layout.anchor_at(viewport.y);
        let bottom = doc.layout.document_height() - viewport.height;
        let page = if viewport.y >= bottom - 1.0 && viewport.y > 0.0 { doc.layout.count().saturating_sub(1) } else { doc.layout.page_at(viewport.y + viewport.height / 3.0) };
        if let Some(jump) = self.jump {
            if self.scrolled_to >= jump {
                self.jump = None;
                self.last_view_page = Some(page);
            }
            return;
        }
        if self.last_view_page != Some(page) {
            self.last_view_page = Some(page);
            self.moved_page = Some(page);
        }
    }

    fn cancel_if_unwanted(&mut self) {
        let Some(running) = &self.running else { return };
        let wanted = self.hot == Some(running.tab) && self.window_open && self.doc().is_some_and(|doc| self.visible_pages(doc).contains(&running.position) && running.rect.is_none_or(|rect| running.scale != doc.tile_scale || self.touches_view(doc, running.position, rect, running.scale)));
        if !wanted && let Some(cancel) = &running.cancel {
            cancel.cancel_before(running.generation + 1);
        }
    }

    fn visible_pages(&self, doc: &DocView) -> std::ops::RangeInclusive<u32> {
        if doc.layout.count() == 0 || self.viewport.height <= 0.0 {
            #[allow(clippy::reversed_empty_ranges)]
            return 1..=0;
        }
        doc.layout.pages_between(self.viewport.y.max(0.0), self.viewport.y.max(0.0) + self.viewport.height)
    }

    /// Whether a device rectangle of a page at a scale shows in the viewport.
    fn touches_view(&self, doc: &DocView, page: u32, rect: TileRect, scale: f32) -> bool {
        let place = doc.layout.rect(page);
        let per_point = doc.layout.fit() / f64::from(scale);
        let (left, top) = (place.x + f64::from(rect.x) * per_point, place.y + f64::from(rect.y) * per_point);
        let (right, bottom) = (left + f64::from(rect.width) * per_point, top + f64::from(rect.height) * per_point);
        right > self.viewport.x && left < self.viewport.x + self.viewport.width && bottom > self.viewport.y && top < self.viewport.y + self.viewport.height
    }

    /// The scale tiles should be, for this view.
    fn target_scale(&self, doc: &DocView) -> f32 {
        let target = doc.layout.fit() * self.viewport.magnification * self.viewport.backing;
        (target as f32).clamp(MIN_SCALE, MAX_SCALE)
    }

    /// Which tiles of a page the view touches, nearest the middle first.
    fn visible_cells(&self, doc: &DocView, page: u32, scale: f32) -> Result<Vec<(u16, u16)>, ()> {
        let size = doc.layout.size(page);
        let (width, height) = device_size(size.width, size.height, scale).map_err(|_| ())?;
        let place = doc.layout.rect(page);
        let per_doc = f64::from(scale) / doc.layout.fit();
        let (view_x, view_y, view_w, view_h) = (self.viewport.x, self.viewport.y, self.viewport.width, self.viewport.height);
        let left = ((view_x - place.x) * per_doc).max(0.0);
        let top = ((view_y - place.y) * per_doc).max(0.0);
        let right = ((view_x + view_w - place.x) * per_doc).min(f64::from(width));
        let bottom = ((view_y + view_h - place.y) * per_doc).min(f64::from(height));
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

        let stale: Vec<u32> = self.shown.keys().copied().filter(|page| !visible.contains(page)).collect();
        for page in stale {
            self.hide_page(page);
        }
        let Some(doc) = self.docs.get(&tab) else { return };
        let paper_rgb = self.paper.rgb();
        let device = self.viewport.magnification * self.viewport.backing;
        let mut placements = Vec::with_capacity(visible.len());
        for &page in &visible {
            let rect = doc.layout.rect(page);
            let size = doc.layout.size(page);
            // On whole device pixels, so a tile's pixels land on the screen's.
            let snap = |value: f64| if device > 0.0 { (value * device).round() / device } else { value };
            placements.push((page, Placement { x: snap(rect.x), y: snap(rect.y), width: f64::from(size.width), height: f64::from(size.height), fit: doc.layout.fit(), paper_rgb }));
        }
        for (page, placement) in placements {
            if self.shown.get(&page) != Some(&placement) {
                self.shown.insert(page, placement);
                self.ops.push_back(Op::ShowPage { page, x: placement.x, y: placement.y, width: placement.width, height: placement.height, fit: placement.fit, paper_rgb: placement.paper_rgb });
            }
        }
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
                wanted.extend(preview_id.filter(|_| false));
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
        self.set_note(note);
    }

    fn set_note(&mut self, note: Option<String>) {
        if self.note != note {
            self.note = note;
        }
    }

    fn attach(&mut self, id: u64, below: bool) -> bool {
        self.clock += 1;
        let clock = self.clock;
        let Some(entry) = self.entries.get_mut(&id) else { return false };
        if entry.discardable {
            let kept = entry.surface.as_ref().is_none_or(|surface| surface.reclaim() == Purge::Kept);
            entry.discardable = false;
            if !kept {
                self.counts.discards_lost += 1;
                self.forget_entry(id, false);
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
        for id in self.attached.remove(&page).unwrap_or_default() {
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.attached = false;
                self.to_discard.push(id);
            }
        }
    }

    /// Forgets an entry. A surface nobody's looking at goes back to the pool if it's a tile's.
    fn forget_entry(&mut self, id: u64, keep_surface: bool) {
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
        if keep_surface
            && !entry.attached
            && let Some(surface) = entry.surface
        {
            self.give_back(surface);
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

    fn give_back(&mut self, surface: PixelSurface) {
        if surface.width() == TILE_EDGE && surface.height() == TILE_EDGE && self.pool.len() < MAX_POOL {
            surface.set_discardable();
            self.pool.push(surface);
        }
    }

    fn take_surface(&mut self, width: u32, height: u32) -> Option<PixelSurface> {
        if width == TILE_EDGE && height == TILE_EDGE
            && let Some(surface) = self.pool.pop()
        {
            // Its pixels don't matter, so it doesn't matter if they were taken.
            surface.reclaim();
            return Some(surface);
        }
        PixelSurface::create(width, height).ok()
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
        self.ops.drain(..).collect()
    }

    /// Whether the host should come back for more.
    pub fn has_ops(&self) -> bool {
        !self.ops.is_empty() || self.more_to_attach
    }

    /// The host has applied what it was given. What's now off every layer can be marked discardable, and it leaves the footprint.
    pub fn applied(&mut self) {
        self.unapplied = false;
        for id in std::mem::take(&mut self.to_discard) {
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
            let too_many_tiles = tile_bytes > MAX_KEPT_BYTES || tile_surfaces > MAX_KEPT_SURFACES || self.entries.len() > MAX_ENTRIES;
            let victim = if previews > MAX_KEPT_PREVIEWS { oldest_preview } else if too_many_tiles { oldest_tile.or(oldest_preview) } else { None };
            let Some((_, id)) = victim else { return };
            self.forget_entry(id, true);
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

    pub fn set_focus(&mut self, focus: bool) {
        if self.focus == focus {
            return;
        }
        self.focus = focus;
        self.lay_out();
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// The window closed. Everything drawn goes, so a closed window costs nothing here. The tabs stay.
    pub fn window_closed(&mut self) {
        self.window_open = false;
        self.clear_screen();
        let ids: Vec<u64> = self.entries.keys().copied().collect();
        for id in ids {
            self.forget_entry(id, false);
        }
        self.pool.clear();
        if let Some(running) = &self.running
            && let Some(cancel) = &running.cancel
        {
            cancel.cancel_before(running.generation + 1);
        }
        self.wanted_files = Arc::new(Vec::new());
    }

    /// The window is back: everything is asked for again.
    pub fn window_opened(&mut self) {
        self.window_open = true;
        let hot = self.hot;
        let warm: Vec<u32> = self.docs.keys().copied().filter(|tab| Some(*tab) != hot).collect();
        self.recount_wanted_files(&warm);
        self.lay_out();
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// The next thing to do, most wanted first, or `None`, when the engine parks. Only what's visible is drawn: drawing ahead comes in a later phase.
    pub fn next_job(&mut self) -> Option<Job> {
        if !self.window_open {
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
        if let Some(page) = visible.clone().chain(around).find(|page| !doc.layout.is_known(*page)) {
            let (file, source_page) = doc.map.source(page)?;
            let generation = self.fresh_generation();
            return Some(Job { tab, position: page, file, source_page, generation, kind: JobKind::Size });
        }
        let now = Instant::now();
        let scale = doc.tile_scale;
        if scale <= 0.0 {
            return None;
        }
        let pages: Vec<u32> = visible.collect();
        for page in pages {
            let doc = self.docs.get(&tab)?;
            if let Some(issue) = doc.issues.get(&page)
                && !issue.try_again_at.is_some_and(|at| now >= at)
            {
                continue;
            }
            let Ok(cells) = self.visible_cells(doc, page, scale) else {
                let size = doc.layout.size(page);
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
            if missing.len() > 1 && !doc.heavy.contains_key(&page) {
                let generation = self.fresh_generation();
                return Some(Job { tab, position: page, file, source_page, generation, kind: JobKind::Traits });
            }
            let heavy = doc.heavy.get(&page).copied().unwrap_or(false);
            // A heavy page's cost is per pass, so a preview would cost it as much again.
            let preview_wanted = !heavy && missing.len() > 1 && !self.cover.contains_key(&self.preview_key(doc, page));
            if preview_wanted && let Some((preview_scale, rect)) = self.preview_rect(doc, page, scale) {
                let surface = self.take_surface(rect.width, rect.height)?;
                let paper = self.paper;
                let generation = self.fresh_generation();
                return Some(Job { tab, position: page, file, source_page, generation, kind: JobKind::Draw { scale: preview_scale, rect, surface, what: DrawKind::Preview, paper } });
            }
            let size = doc.layout.size(page);
            let (width, height) = device_size(size.width, size.height, scale).ok()?;
            if heavy && missing.len() > 1 {
                let wanted: Vec<(u32, u32)> = missing.iter().map(|&(column, row)| (u32::from(column), u32::from(row))).collect();
                let mut sorted = wanted;
                sorted.sort_unstable();
                let mut rect = plan_regions(&sorted, (width, height)).into_iter().next()?;
                let surface = loop {
                    let surface = self.take_surface(rect.width, rect.height)?;
                    // A surface's rows can be padded, which can take it over the cap. A row of tiles less is always under.
                    if u64::from(surface.stride()) * u64::from(rect.height) <= MAX_REGION_BYTES || rect.height <= TILE_EDGE {
                        break surface;
                    }
                    rect.height -= TILE_EDGE;
                };
                let paper = self.paper;
                let generation = self.fresh_generation();
                return Some(Job { tab, position: page, file, source_page, generation, kind: JobKind::Draw { scale, rect, surface, what: DrawKind::Region, paper } });
            }
            let (column, row) = missing[0];
            let (x, y) = (u32::from(column) * TILE_EDGE, u32::from(row) * TILE_EDGE);
            let rect = TileRect { x, y, width: TILE_EDGE.min(width - x), height: TILE_EDGE.min(height - y) };
            let surface = self.take_surface(TILE_EDGE, TILE_EDGE)?;
            let paper = self.paper;
            let generation = self.fresh_generation();
            return Some(Job { tab, position: page, file, source_page, generation, kind: JobKind::Draw { scale, rect, surface, what: DrawKind::Tile, paper } });
        }
        None
    }

    fn preview_rect(&self, doc: &DocView, page: u32, scale: f32) -> Option<(f32, TileRect)> {
        let size = doc.layout.size(page);
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
        let (rect, scale) = match &job.kind {
            JobKind::Draw { scale, rect, .. } => (Some(*rect), *scale),
            _ => (None, 0.0),
        };
        self.running = Some(Running { tab: job.tab, position: job.position, rect, scale, generation: job.generation, cancel });
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
        let Job { tab, position, kind, .. } = job;
        match (kind, outcome) {
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
                if !self.docs.contains_key(&tab) || !self.window_open {
                    self.give_back(surface);
                } else {
                    self.keep_drawn(tab, position, scale, rect, surface, what, paper, kind, low_res);
                }
            }
            (JobKind::Draw { surface, .. }, Outcome::Cancelled) => {
                self.counts.cancelled += 1;
                self.give_back(surface);
            }
            (kind, Outcome::PageFailed { message, may_pass }) => {
                if let JobKind::Draw { surface, .. } = kind {
                    self.give_back(surface);
                }
                self.note_issue(tab, position, message, may_pass);
            }
            (kind, Outcome::DocumentFailed(message)) => {
                if let JobKind::Draw { surface, .. } = kind {
                    self.give_back(surface);
                }
                if let Some(doc) = self.docs.get_mut(&tab) {
                    doc.problem = Some(message);
                }
            }
            (kind, _) => {
                self.counts.cancelled += 1;
                if let JobKind::Draw { surface, .. } = kind {
                    self.give_back(surface);
                }
            }
        }
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    #[allow(clippy::too_many_arguments)]
    fn keep_drawn(&mut self, tab: u32, page: u32, scale: f32, rect: TileRect, surface: PixelSurface, what: DrawKind, paper: PaperKind, kind: TileKind, low_res: Option<(TileRect, LowResWhy)>) {
        let id = self.next_id;
        self.next_id += 1;
        let preview = what == DrawKind::Preview;
        let drawn_rect = low_res.map_or(rect, |(drawn, _)| drawn);
        let crop = (f64::from(drawn_rect.width) / f64::from(surface.width().max(1)), f64::from(drawn_rect.height) / f64::from(surface.height().max(1)));
        // A tile of one colour needs no surface, unless it's a whole region's or the only sign of a page.
        let (rgb, surface) = match (kind, what) {
            (TileKind::Blank { rgb }, DrawKind::Tile) if low_res.is_none() => {
                self.give_back(surface);
                (Some(rgb), None)
            }
            _ => (None, Some(surface)),
        };
        let note = low_res.map(|(_, why)| LowResNote { message: why.message(), may_clear: why.may_clear(), at: Instant::now() });
        self.clock += 1;
        let entry = Entry { id, tab, page, scale, paper, preview, rect, surface, rgb, crop, low_res: note, attached: false, discardable: false, last_used: self.clock };
        for key in Self::keys_of(&entry) {
            if let Some(old) = self.cover.insert(key, id)
                && old != id
            {
                self.retire_replaced(old, id, tab);
            }
        }
        self.entries.insert(id, entry);
        if let Some(doc) = self.docs.get_mut(&tab) {
            doc.seen.insert(page);
        }
    }

    /// Something drawn over an older copy of the same place. The older one comes off when no key points at it.
    fn retire_replaced(&mut self, old: u64, _new: u64, tab: u32) {
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
                self.forget_entry(old, false);
            }
        }
    }

    /// Whether everything visible is drawn and on its layers, or marked as it is.
    pub fn is_settled(&self) -> bool {
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
        for page in self.visible_pages(doc) {
            if !doc.layout.is_known(page) {
                return false;
            }
            if doc.issues.contains_key(&page) {
                continue;
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
            "tile_scale": doc.map(|doc| doc.tile_scale),
            "document_height": doc.map(|doc| doc.layout.document_height()),
            "scroll_y": self.viewport.y,
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
                "pool": self.pool.len(),
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
            "window_open": self.window_open,
        }))
    }

    pub fn counts(&self) -> &ReaderCounts {
        &self.counts
    }

    /// The zoom limits and spacing the host needs.
    pub fn limits() -> (f64, f64, f64) {
        (MIN_ZOOM, MAX_ZOOM, PAGE_GAP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::PageBox;

    fn map() -> PageMap {
        PageMap { files: vec![PathBuf::from("/d/a.pdf")], slots: None }
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
                JobKind::Traits => Outcome::Heavy(false),
                JobKind::Draw { .. } => Outcome::Drawn { kind: TileKind::Colour, low_res: None },
            };
            drawn.push(Job { tab: job.tab, position: job.position, file: job.file.clone(), source_page: job.source_page, generation: job.generation, kind: match &job.kind { JobKind::Size => JobKind::Size, JobKind::Traits => JobKind::Traits, JobKind::Draw { scale, rect, surface, what, paper } => JobKind::Draw { scale: *scale, rect: *rect, surface: surface.clone(), what: *what, paper: *paper } } });
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
        reader.activate(1, pages, 0, map(), &[]);
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

    /// Failure: a burst of attachments stalls the UI thread, or the leftovers are forgotten.
    #[test]
    fn at_most_eight_tiles_are_attached_in_a_turn_and_the_rest_follow() {
        let mut reader = reader_on(100);
        settle(&mut reader);
        reader.set_viewport(view(5000.0));
        settle(&mut reader);
        reader.applied();
        let big = Viewport { width: 4000.0, height: 3000.0, frame_width: 1168.0, ..view(0.0) };
        let ops = reader.set_viewport(big);
        let attaches = ops.iter().filter(|op| matches!(op, Op::Attach { .. })).count();
        assert!(attaches <= MAX_ATTACH_PER_TURN, "{attaches}");
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

    /// Failure: a job for a place the reader has left runs to the end, so a fast scroll on a heavy page queues seconds of work. `cancel_before` must be called with a generation above the job's.
    #[test]
    fn a_job_nobody_wants_any_more_is_cancelled() {
        let mut reader = reader_on(100);
        settle(&mut reader);
        reader.set_viewport(view(40_000.0));
        let job = (0..8).find_map(|_| {
            let job = reader.next_job()?;
            if matches!(job.kind, JobKind::Draw { .. }) {
                return Some(job);
            }
            let outcome = match job.kind {
                JobKind::Size => Outcome::Size(letter()),
                _ => Outcome::Heavy(false),
            };
            reader.finish(job, outcome);
            None
        });
        let job = job.expect("a draw");
        reader.note_started(&job, None);
        assert!(!reader.is_settled());
        let running = reader.running.as_ref().unwrap();
        assert_eq!(running.generation, job.generation);
        reader.set_viewport(view(0.0));
        // With no handle in this test, what's checked is that a later job is always of a later generation, which is what cancelling relies on.
        reader.finish(job, Outcome::Cancelled);
        settle(&mut reader);
        let later = reader.next_generation;
        assert!(later > 1);
    }

    /// Failure: a cold tab keeps its tiles, layout or renderer's file. Budget: 0 tiles and 0 handles for a cold tab.
    #[test]
    fn a_tab_gone_cold_holds_nothing() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        assert!(!reader.entries.is_empty());
        reader.activate(2, 10, 0, PageMap { files: vec![PathBuf::from("/d/b.pdf")], slots: None }, &[]);
        assert!(reader.entries.values().all(|entry| entry.tab != 1), "tab 1 isn't warm");
        assert!(!reader.docs.contains_key(&1));
        assert_eq!(*reader.wanted_files(), vec![PathBuf::from("/d/b.pdf")]);
        reader.activate(3, 10, 0, PageMap { files: vec![PathBuf::from("/d/c.pdf")], slots: None }, &[2]);
        assert_eq!(reader.wanted_files().len(), 2, "a warm tab's file is kept");
    }

    /// Failure: the window closed still holds tiles, so a closed window stays over budget.
    #[test]
    fn a_closed_window_holds_no_tiles_and_comes_back() {
        let mut reader = reader_on(10);
        settle(&mut reader);
        reader.window_closed();
        assert!(reader.entries.is_empty() && reader.pool.is_empty());
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

    /// Failure: a page that couldn't be drawn is asked for on every scroll, or holds up the rest, or shows no reason.
    #[test]
    fn a_failed_page_says_why_and_isnt_asked_for_again() {
        let mut reader = reader_on(3);
        for _ in 0..16 {
            let Some(job) = reader.next_job() else { break };
            let outcome = match &job.kind {
                JobKind::Size => Outcome::Size(letter()),
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
        let edited = PageMap { files: vec![PathBuf::from("/d/a.pdf")], slots: Some(vec![(0, 1), (0, 0)]) };
        reader.activate(1, 2, 0, edited.clone(), &[]);
        assert!(reader.entries.is_empty());
        assert_eq!(reader.doc().unwrap().layout.count(), 2);
        assert_eq!(edited.source(0), Some((PathBuf::from("/d/a.pdf"), 1)));
        assert_eq!(edited.source(9), None);
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
