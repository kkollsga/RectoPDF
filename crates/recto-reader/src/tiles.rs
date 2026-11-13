//! What's wanted on screen, what's kept, and what the host is told. All the policy for the page view, as plain state: the UI thread calls [`Reader::set_viewport`] on every scroll, so nothing here waits. The one lock
//! around this is never held across a render or a disk read, which [`crate::engine`] does outside it. Numbers are from the platform report, §2.6.
//!
//! **Three levels on a page,** so a page seen once never goes blank: a preview at a quarter of the fitted scale, tiles of another zoom, and exact tiles. A page shows what it has, and when its visible tiles are all there, the
//! rest come off and go discardable.
//!
//! **What's in memory.** Only tiles on layers count. Anything off-screen is marked discardable the moment the host says it's off the layer: it leaves the footprint at once, and is usually still there on the way back.

use crate::layout::{PageBox, PageLayout, Spread};
use crate::paper::PaperKind;
use recto_platform::FileStamp;
use recto_proto::{MAX_SCALE, MIN_SCALE, TileRect};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod cache;
mod doc;
mod geometry;
mod host;
mod jobs;
mod place;
mod plan;
mod pointer;
mod refresh;
mod report;
mod scratch;
pub(crate) mod sleep_draw;
mod wake;
mod zoom;

use cache::{Entry, TileCache};
pub(crate) use doc::DocView;
use doc::Issue;
use host::HostQueue;
pub(crate) use jobs::Work;
pub use jobs::{DrawKind, Job, JobKind, JobPlan, Outcome};
use jobs::{MAX_KEYLESS, PlanKind};
use place::Place;
pub use place::place_stamp;
use pointer::Pointer;
use scratch::Scratch;
use wake::Waking;
pub use wake::{RawPicture, SleepMaterial, SleepShot, SleepView, WakeReport, WakeStock};

pub use zoom::{MAX_ZOOM, MIN_ZOOM, ZOOM_STEP, Zoom, typed_zoom, zoom_after_scroll, zoom_percent};

/// Tiles attached in one turn of the UI thread. Platform report §2.6: it stays under budget, and the next turn takes the rest.
pub const MAX_ATTACH_PER_TURN: usize = 8;
/// How far from where it was sent the host can put the view, on its own pixels: half a point at 2x, a point at 1x. Within it, the view is still on the place.
const HOST_ROUNDING: f64 = 1.0;
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
/// Pages remembered as drawn at least once, which only the QA count of blank steps on pages seen before goes by. A file can claim 8 million pages, and a long scroll draws page after page, so past this they're all forgotten at once, as `heavy` is. **What that costs is the count's accuracy:** a page drawn before then counts as never seen until it's drawn again. As many as the layout keeps sizes for, far past any book read through in one sitting, and about 0.6 MiB a held tab at the most.
const MAX_SEEN_REMEMBERED: usize = crate::layout::MAX_OVERRIDES;
/// Pages whose problem is remembered, so they aren't asked for again. Past it, the problems of pages away from the view go: such a page is asked for once more when it's back in view, and its answer remembered again, so it's never asked on every look. 4,096 failing pages is far past what a damaged real file has, and about 0.5 MiB a held tab at the most.
const MAX_ISSUES_REMEMBERED: usize = 4096;
/// Pages either side of what's visible whose sizes are asked for, so layout near the reader settles early.
const SIZES_AROUND: u32 = 2;
/// The most pages counted as in view, from the top of the view. A row is at least the gap between rows tall, 10 points, so the tallest Mac display's window, 3,384 points, at the least zoom shows at most about 2,700 pages side by side, however small a hostile file makes them; past this only a window taller than any display could. Everything sized by the pages in view is held by it: the layers placed, the tiles on them, a step's ops, and the problems of pages near the view, which [`MAX_ISSUES_REMEMBERED`] keeps past its cap.
const MAX_PAGES_IN_VIEW: usize = 4_000;
const _: () = assert!(
    MAX_PAGES_IN_VIEW + 2 * SIZES_AROUND as usize <= MAX_ISSUES_REMEMBERED,
    "the problems of the pages near the view would pass their cap"
);

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
        self.file_of(position).map(|(file, page)| (file.clone(), page))
    }

    /// Which file and which page a position is, borrowed: asked on every tile put on a layer, so it copies no path.
    fn file_of(&self, position: u32) -> Option<(&PathBuf, u32)> {
        match &self.slots {
            None => self.files.first().map(|file| (file, position)),
            Some(slots) => {
                let &(file, page) = slots.get(position as usize)?;
                self.files.get(usize::from(file)).map(|file| (file, page))
            }
        }
    }
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
    cache: TileCache,
    host: HostQueue,
    /// What's asked of the renderer: the engine notes what it starts and takes what it must open again through this field's own methods.
    pub(crate) work: Work,
    /// Where the reader is: the engine takes the reader's own scroll and the page moved to through this field's own methods.
    pub(crate) place: Place,
    counts: ReaderCounts,
    note: Option<String>,
    /// What the pointer does on a page: the engine calls its methods for the selection's and the links' own state, which reach nothing else.
    pub(crate) pointer: Pointer,
    /// The wake and sleep pictures: the engine gives a tab's sleep file and says the window's up through this field's own methods.
    pub(crate) wake: Waking,
    /// The least room either side of the pages: a comparison's pane has less than a window.
    margins: (f64, f64),
    /// The lists a scroll step fills, kept between steps.
    scratch: Scratch,
}

// size: held at what it was on 2026-11-13 (macOS, debug and release alike) before R3.6 moved `Reader`'s methods onto its parts, so the move, or a field added later, can't grow it unseen.
#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<Reader>() <= 2096, "the page view grew");

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
            cache: TileCache::new(),
            host: HostQueue::default(),
            work: Work::new(),
            place: Place::new(),
            counts: ReaderCounts::default(),
            note: None,
            pointer: Pointer::new(),
            wake: Waking::new(),
            margins: (crate::layout::MIN_SIDE_MARGIN, crate::layout::CORNER_MARGIN),
            scratch: Scratch::default(),
        }
    }

    /// The last scroll the host said it had reached, for a test to go on from.
    #[cfg(test)]
    pub(crate) fn viewport_scrolled_to(&self) -> u64 {
        self.viewport.scrolled_to
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
        let (file, _) = doc.map.file_of(page)?;
        doc.drawing.versions.get(file).copied()
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
            let was = doc.drawing.versions.insert(file.to_path_buf(), version);
            if was == Some(version) {
                continue;
            }
            // Of the version it was left with.
            doc.drawing.left_pictures = None;
            let stale: Vec<u64> = self
                .cache
                .entries
                .values()
                .filter(|entry| entry.tab == tab && !matches!((entry.version, was), (Some(drawn), _) if drawn == version))
                .filter(|entry| {
                    was.is_some()
                        || entry.version.is_some_and(|drawn| drawn != version)
                        || entry.version.is_none() && entry.preview && self.cache.from_sleep.contains(&entry.id)
                })
                .map(|entry| entry.id)
                .collect();
            if stale.is_empty() {
                continue;
            }
            for id in stale {
                if let Some(entry) = self.cache.entries.get(&id)
                    && entry.attached
                {
                    let page = entry.page;
                    if self.hot == Some(tab) {
                        self.detach(id, page);
                    }
                }
                self.cache.forget(id, &mut self.host);
            }
            if self.hot == Some(tab) {
                self.refresh(MAX_ATTACH_PER_TURN);
            }
        }
    }

    /// **A password's been given for a file:** its renderer opens again with it, and what an open without it found, that it needs one, is forgotten, so its pages are asked for afresh.
    pub fn key_given(&mut self, path: &std::path::Path) {
        self.work.keyless.retain(|file| file != path);
        self.work.reopen_files.push(path.to_path_buf());
        for doc in self.docs.values_mut() {
            if doc.map.files.iter().any(|file| file == path) {
                doc.drawing.problem = None;
                doc.drawing.issues.clear();
            }
        }
        self.refresh(MAX_ATTACH_PER_TURN);
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

    pub fn hot_tab(&self) -> Option<u32> {
        self.hot
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// The tab showing's view, if one is: where the engine reads its points, pages, place and zoom from ([`DocView`]'s own methods, in `tiles/geometry.rs`).
    pub(crate) fn doc(&self) -> Option<&DocView> {
        self.hot.and_then(|tab| self.docs.get(&tab))
    }

    /// Makes a tab the one on screen, with its place. A tab seen before keeps its layout and zoom.
    #[allow(clippy::too_many_arguments)]
    pub fn activate(&mut self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32], saved: Option<SavedPlace>, spread: Spread) {
        self.keep_only(warm, tab);
        // The tab showing taken up again, as a turn of its pages does: the reader stays where they are.
        let staying = self.hot == Some(tab);
        // What was selected when a renderer saw the file replaced, for a save that turns out to have written what was shown.
        let selected_before_change = self.pointer.selection_before_change.take().filter(|(of, _)| *of == tab).map(|(_, range)| range);
        if staying && let Some(doc) = self.docs.get_mut(&tab) {
            // Across the page, as the view has it now: what's taken up again goes back there.
            doc.place.scroll_x = self.viewport.x;
        }
        if self.hot != Some(tab) {
            self.leave_hot(tab);
        }
        let floor = self.work.next_generation;
        let margins = self.margins;
        let doc = self
            .docs
            .entry(tab)
            .or_insert_with(|| DocView::from_saved(tab, pages, page, &map, saved, spread, margins, floor));
        // A save wrote the pages that were shown, each as it was shown: another version of the file, with nothing on the screen to move.
        let carried = !doc.map.same_pages(&map) && map.follows.is_some() && doc.map.own == map.follows && doc.layout.count() == pages;
        // Back on the page it was left on, and it's back at the same place in that page. A tall page's reader isn't sent to its top.
        let same_pages = (doc.map.same_pages(&map) || carried) && doc.layout.count() == pages;
        let back_at = if staying && same_pages {
            Some(doc.place.anchor)
        } else {
            doc.place
                .left_at
                .take()
                .filter(|(left, _)| *left == page && same_pages)
                .map(|(_, anchor)| anchor)
        };
        if back_at.is_none() {
            doc.place.scroll_x = 0.0;
            doc.place.exact_y = None;
        }
        doc.place.anchor = back_at.unwrap_or((page.min(pages.saturating_sub(1)), 0.0));
        if doc.map.turns != map.turns && doc.map.same_pages(&map) && doc.layout.count() == pages {
            // Turned, and nothing else: what's drawn stays, and the pages sit the new way.
            doc.layout.set_turns(map.turns.clone());
            doc.map.turns = map.turns.clone();
            doc.place.exact_y = None;
        }
        let mut carried_selection = None;
        if !doc.map.same_pages(&map) || doc.layout.count() != pages {
            // Another version of its file: a worker that's running has the old one open, and goes on drawing that.
            if doc.map.own != map.own {
                self.work.reopen_files.extend(map.files.first().cloned());
            }
            if carried {
                // **A save leaves the view as it was** (owner: "perfect" state recovery): the pages keep their sizes as shown, so the fit, the zoom shown, the place and the layout don't move. Only what's drawn is of the new file, and is drawn again.
                doc.layout = doc.layout.as_saved();
                carried_selection = self.pointer.selecting.range().or(selected_before_change);
            } else {
                doc.layout = PageLayout::with_spread(pages, spread);
            }
            doc.layout.set_turns(map.turns.clone());
            doc.map = map;
            doc.drawing.issues.clear();
            doc.drawing.heavy.clear();
            doc.drawing.problem = None;
            doc.drawing.seen.clear();
            doc.drawing.floor = floor;
            if let Some(running) = &self.work.running
                && running.tab == tab
                && let Some(cancel) = &running.cancel
            {
                cancel.cancel_before(running.generation + 1);
            }
            self.drop_tab_entries(tab);
            self.clear_screen();
            // Its pages are other pages now, so what was selected on them isn't there. After a save they're the same pages, so it is, and its text is read again from the new file.
            self.pointer.selecting.reset();
            self.pointer.linking.clear();
            if let Some(range) = carried_selection {
                self.pointer.selecting.set_range(range);
            }
        }
        // On the screen again: it's drawn again, and what it was left with is out of date as soon as it moves.
        if let Some(doc) = self.docs.get_mut(&tab) {
            doc.drawing.left_pictures = None;
        }
        self.take_sleep_view(tab);
        // As the session has it for this tab: a tab held warm keeps its layout from when it was shown, which may have been changed since by an agent.
        if let Some(doc) = self.docs.get_mut(&tab)
            && doc.layout.set_spread(spread)
        {
            doc.place.exact_y = None;
        }
        let (magnification, across) = self.docs.get(&tab).map_or((1.0, 0.0), |doc| (doc.place.magnification, doc.place.scroll_x));
        self.host.ops.push_back(Op::Magnify { value: magnification });
        self.lay_out();
        self.jump_to_anchor(page);
        self.host.ops.push_back(Op::ScrollAcross { x: across });
        self.recount_wanted_files(warm);
        wake::Waking::note_stock(self, tab);
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// Leaves the tab that was showing for `tab`: what it showed and where it was are kept with it, and the screen is cleared.
    fn leave_hot(&mut self, tab: u32) {
        // While what it showed is still on the screen, and so all there.
        if let Some(left) = self.hot.filter(|hot| self.docs.contains_key(hot)) {
            let pictures = self.pictures_in_view(left);
            if let Some(doc) = self.docs.get_mut(&left) {
                doc.drawing.left_pictures = Some(pictures);
            }
        }
        let last_page = self.place.last_view_page;
        let (left_x, left_y) = (self.viewport.x, self.viewport.y);
        if let Some(doc) = self.hot.and_then(|hot| self.docs.get_mut(&hot)) {
            doc.place.left_at = last_page.map(|page| (page, doc.place.anchor));
            doc.place.scroll_x = left_x;
            doc.place.exact_y = Some((left_y, doc.layout.document_height()));
        }
        // With its place and its pictures noted: what of its place it has no picture of is drawn once the tab coming has what it needs.
        if let Some(left) = self.hot {
            sleep_draw::SleepDraws::left(self, left);
        }
        self.clear_screen();
        self.hot = Some(tab);
        self.place.last_view_page = None;
        self.place.moved_page = None;
        // A selection is in what's on screen, and a tab off screen holds none.
        self.pointer.selecting.reset();
        self.pointer.linking.clear();
    }

    /// Everything of tabs that have gone cold goes, as a cold tab holds nothing.
    fn keep_only(&mut self, warm: &[u32], hot: u32) {
        let gone: Vec<u32> = self.docs.keys().copied().filter(|tab| *tab != hot && !warm.contains(tab)).collect();
        for tab in gone {
            self.docs.remove(&tab);
            self.drop_tab_entries(tab);
        }
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
        self.work.wanted_files = Arc::new(Vec::new());
        self.set_note(None);
        self.pointer.selecting.reset();
        self.let_lists_go();
    }

    /// The room kept for scroll steps goes, when there's nothing to scroll: no tab, or no window.
    fn let_lists_go(&mut self) {
        self.scratch = Scratch::default();
        self.cache.spare = Vec::new();
    }

    /// Takes everything off the screen, in one op.
    fn clear_screen(&mut self) {
        self.host.ops.push_back(Op::Clear);
        for ids in std::mem::take(&mut self.cache.attached).into_values() {
            for id in ids {
                if let Some(entry) = self.cache.entries.get_mut(&id) {
                    entry.attached = false;
                    self.host.to_discard.push(id);
                }
            }
        }
        self.host.shown.clear();
        self.pointer.selecting.layers_gone();
    }

    /// The host's view moved. Called on every scroll, on the UI thread: a few map lookups per visible tile, and at most [`MAX_ATTACH_PER_TURN`] surfaces reclaimed. What it returns is for the host to apply now.
    pub fn set_viewport(&mut self, viewport: Viewport) -> Vec<Op> {
        let was = self.viewport;
        self.viewport = viewport;
        self.counts.viewport_steps += 1;
        place::tick();
        self.place.scrolled_to = viewport.scrolled_to.max(self.place.scrolled_to);
        // A new window's first word, before it's been told anything: the top of the document at no zoom. That's nobody's place.
        if !self.window_open {
            return self.host.take_ops();
        }
        // The reader's own scroll: the view moved down or up, with nothing the host was sent arriving, no change of zoom or room, and no pinch or resize under way.
        self.place.note_viewport(was, viewport);
        let mut moved_layout = false;
        if let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) {
            if doc.layout.set_frame(viewport.frame_width) && was.frame_width != viewport.frame_width {
                moved_layout = true;
            }
            // A jump under way says the host hasn't caught up with what it's been told, and that includes the zoom.
            if !viewport.live && self.place.jump.is_none() {
                doc.place.magnification = viewport.magnification;
            }
        }
        if moved_layout {
            self.lay_out();
        } else {
            let viewport = self.viewport;
            if let Some(doc) = self.hot.and_then(|tab| self.docs.get_mut(&tab)) {
                self.place.track(doc, viewport);
            }
            self.widen_for_view();
        }
        // After the refresh, which is what moves the tiles' scale on.
        self.refresh(MAX_ATTACH_PER_TURN);
        self.cancel_if_unwanted();
        self.count_blanks();
        self.host.take_ops()
    }

    fn visible_pages(&self, doc: &DocView) -> std::ops::RangeInclusive<u32> {
        if doc.layout.count() == 0 || self.viewport.height <= 0.0 {
            #[allow(clippy::reversed_empty_ranges)]
            return 1..=0;
        }
        let pages = doc
            .layout
            .pages_between(self.viewport.y.max(0.0), self.viewport.y.max(0.0) + self.viewport.height);
        // Held to the most a real display's view has: a hostile file's slivers past it aren't in view.
        let (first, last) = (*pages.start(), *pages.end());
        first..=last.min(first.saturating_add(MAX_PAGES_IN_VIEW as u32 - 1))
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

    /// The ops for the host, attaching at most `budget` more first.
    pub fn drain(&mut self, budget: usize) -> Vec<Op> {
        self.refresh(budget);
        self.host.take_ops()
    }

    /// Whether the host should come back for more.
    pub fn has_ops(&self) -> bool {
        self.host.has_ops()
    }

    /// The host has applied what it was given. What's now off every layer can be marked discardable, and it leaves the footprint.
    pub fn applied(&mut self) {
        self.host.unapplied = false;
        self.track_wake();
        // Emptied and kept: it fills again on the next step that takes something off.
        for id in self.host.handed_to_host.drain(..) {
            if let Some(entry) = self.cache.entries.get_mut(&id)
                && !entry.attached
                && !entry.discardable
            {
                if let Some(surface) = &entry.surface {
                    surface.set_discardable();
                }
                entry.discardable = true;
            }
        }
        self.host.trim();
        self.cache.evict(&mut self.host);
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
            doc.drawing.left_pictures = None;
        }
        let ids: Vec<u64> = self.cache.entries.keys().copied().collect();
        for id in ids {
            self.cache.forget(id, &mut self.host);
        }
        self.host.close();
        if let Some(running) = &self.work.running
            && let Some(cancel) = &running.cancel
        {
            cancel.cancel_before(running.generation + 1);
        }
        self.work.wanted_files = Arc::new(Vec::new());
        self.pointer.selecting.drop_texts();
        self.let_lists_go();
    }

    /// The window is back: everything is asked for again.
    pub fn window_opened(&mut self) {
        self.window_open = true;
        let hot = self.hot;
        let warm: Vec<u32> = self.docs.keys().copied().filter(|tab| Some(*tab) != hot).collect();
        self.recount_wanted_files(&warm);
        // The window is new, at the top of the document and no zoom. It's sent back to the place and zoom this tab was left at, and until it's got there, what it says about where it is isn't the reader moving.
        if let Some(magnification) = self.doc().map(|doc| doc.place.magnification) {
            self.viewport.magnification = magnification;
            self.host.ops.push_back(Op::Magnify { value: magnification });
            self.place.jump = Some(u64::MAX);
        }
        self.lay_out();
        if let Some(across) = self.doc().map(|doc| doc.place.scroll_x) {
            self.host.ops.push_back(Op::ScrollAcross { x: across });
        }
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    /// The thread that draws panicked and is starting over, or, with `give_up`, isn't. What it was doing is gone, and everything it would come back to is asked for again.
    pub fn thread_restarted(&mut self, give_up: bool) {
        self.work.running = None;
        self.pointer.selecting.thread_restarted();
        self.work.gave_up |= give_up;
        self.refresh(MAX_ATTACH_PER_TURN);
    }

    fn note_issue(&mut self, tab: u32, page: u32, message: String, may_pass: bool) {
        // What the view is on, and the pages around it whose sizes are asked for, keep theirs past the cap: those are the pages that would be asked for again at once.
        let near = self.doc().filter(|doc| doc.tab == tab).map(|doc| {
            let visible = self.visible_pages(doc);
            visible.start().saturating_sub(SIZES_AROUND)..=visible.end().saturating_add(SIZES_AROUND)
        });
        if let Some(doc) = self.docs.get_mut(&tab) {
            if doc.drawing.issues.len() >= MAX_ISSUES_REMEMBERED && !doc.drawing.issues.contains_key(&page) {
                doc.drawing.issues.retain(|known, _| near.as_ref().is_some_and(|near| near.contains(known)));
            }
            doc.drawing.issues.insert(
                page,
                Issue {
                    message,
                    try_again_at: may_pass.then(|| Instant::now() + TRY_AGAIN_AFTER),
                },
            );
        }
    }

    /// A job has ended, with whatever it found.
    pub fn finish(&mut self, job: Job, outcome: Outcome) {
        self.work.running = None;
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
                Outcome::Links(links) if self.hot == Some(tab) && !self.docs.get(&tab).is_some_and(|doc| generation < doc.drawing.floor) => {
                    self.pointer.linking.take(position, links)
                }
                _ => self.pointer.linking.cancelled(position),
            }
            return;
        }
        // Asked about the pages as they were. An edit has changed what's at this position since.
        if self.docs.get(&tab).is_some_and(|doc| generation < doc.drawing.floor) {
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
                    if doc.drawing.heavy.len() >= MAX_HEAVY_REMEMBERED {
                        doc.drawing.heavy.clear();
                    }
                    doc.drawing.heavy.insert(position, heavy);
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
                    let id = self.keep_drawn(tab, position, scale, rect, surface, what, paper, kind, low_res);
                    if what == DrawKind::Preview {
                        sleep_draw::SleepDraws::drawn(self, tab, position, id);
                    }
                    kept = Some(id);
                }
            }
            (JobKind::Draw { .. }, Outcome::Cancelled) => {
                self.counts.cancelled += 1;
            }
            (_, Outcome::Unstarted { heavy }) => sleep_draw::SleepDraws::unstarted(self, tab, position, heavy),
            (_, Outcome::PageFailed { message, may_pass }) => {
                self.note_issue(tab, position, message, may_pass);
            }
            (_, Outcome::DocumentFailed { message, needs_key }) => {
                if let Some(doc) = self.docs.get_mut(&tab) {
                    doc.drawing.problem = Some(message);
                }
                if needs_key && !self.work.keyless.contains(&file) && self.work.keyless.len() < MAX_KEYLESS {
                    self.work.keyless.push(file);
                }
            }
            _ => {
                self.counts.cancelled += 1;
            }
        }
        self.refresh(MAX_ATTACH_PER_TURN);
        // It came too late for the view, or for another zoom, paper or a warm tab. Nothing's showing it, so it counts for nothing: drawn is no reason to hold memory.
        if let Some(id) = kept
            && let Some(entry) = self.cache.entries.get_mut(&id)
            && !entry.attached
            && !entry.discardable
        {
            if let Some(surface) = &entry.surface {
                surface.set_discardable();
            }
            entry.discardable = true;
            self.cache.evict(&mut self.host);
        }
    }

    /// Whether everything visible is drawn and on its layers, or marked as it is.
    pub fn is_settled(&self) -> bool {
        if self.work.gave_up {
            return self.host.ops.is_empty() && !self.host.unapplied;
        }
        if self.work.running.is_some() || !self.host.ops.is_empty() || self.host.unapplied || self.host.more_to_attach {
            return false;
        }
        let Some(doc) = self.doc() else { return true };
        if doc.drawing.problem.is_some() {
            return true;
        }
        if !self.window_open {
            return true;
        }
        if self.viewport.height <= 0.0 {
            return false;
        }
        let scale = doc.drawing.tile_scale;
        if scale <= 0.0 {
            return false;
        }
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        if !self
            .pointer
            .selecting
            .is_settled(&visible, doc.layout.count(), |page| doc.drawing.heavy.get(&page).copied().unwrap_or(false))
        {
            return false;
        }
        for page in self.visible_pages(doc) {
            if doc.drawing.issues.contains_key(&page) {
                continue;
            }
            if !doc.layout.is_known(page) {
                return false;
            }
            let Ok(cells) = self.visible_cells(doc, page, scale) else { continue };
            for cell in cells {
                let key = self.key(doc, page, scale, cell);
                if !self
                    .cache
                    .cover
                    .get(&key)
                    .and_then(|id| self.cache.entries.get(id))
                    .is_some_and(|entry| entry.attached)
                {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests;
