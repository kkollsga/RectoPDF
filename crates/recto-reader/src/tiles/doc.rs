//! **The tabs the page view holds,** and which of them is showing ([`Docs`]), and **a tab it holds,** hot or warm: its pages and where they are, where the reader is in it ([`DocPlace`]), and what's drawn of it and known about drawing it ([`DocDrawing`]). One is held for each hot and warm tab, and none for a cold one.

use super::cache::Entry;
use super::{MAX_SEEN_REMEMBERED, MAX_ZOOM, MIN_ZOOM, PageMap, RawPicture, SavedPlace};
use crate::layout::{PageLayout, Spread};
use recto_platform::FileStamp;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

/// The tabs the page view holds, hot and warm, and which is showing. Built only in the tiles module ([`Docs::new`]), as each of [`Reader`]'s parts is (see there): the engine reaches it through `Reader`'s field for the lookups below.
#[derive(Debug)]
pub(crate) struct Docs {
    pub(super) views: HashMap<u32, DocView>,
    pub(super) hot: Option<u32>,
}

impl Docs {
    pub(super) fn new() -> Self {
        Self {
            views: HashMap::new(),
            hot: None,
        }
    }

    /// The tab showing's view, if one is: where the engine reads its points, pages, place and zoom from ([`DocView`]'s own methods, in `tiles/geometry.rs`).
    pub(crate) fn shown(&self) -> Option<&DocView> {
        self.hot.and_then(|tab| self.views.get(&tab))
    }

    pub(crate) fn hot_tab(&self) -> Option<u32> {
        self.hot
    }

    /// Which pages of the tab showing were found too heavy to draw in the ordinary way (the selection reads their text another way).
    pub(super) fn heavy_pages(&self) -> HashMap<u32, bool> {
        self.shown().map(|doc| doc.drawing.heavy.clone()).unwrap_or_default()
    }

    /// The files a tab it holds has.
    pub(crate) fn files_of(&self, tab: u32) -> Vec<PathBuf> {
        self.views.get(&tab).map_or_else(Vec::new, |doc| doc.map.files.clone())
    }

    /// Whether this tab's anything here: a view of it's held, so a sleep file has nothing to add.
    pub(crate) fn holds(&self, tab: u32) -> bool {
        self.views.contains_key(&tab)
    }

    /// The version of the file a page of a tab is from, as a worker last said.
    pub(super) fn version_of_page(&self, tab: u32, page: u32) -> Option<u64> {
        let doc = self.views.get(&tab)?;
        let (file, _) = doc.map.file_of(page)?;
        doc.drawing.versions.get(file).copied()
    }

    /// Whether something drawn is of the version of its file there is now. Unknown on either side passes: before a worker's said, what there is is all there is.
    pub(super) fn is_current(&self, entry: &Entry) -> bool {
        match (entry.version, self.version_of_page(entry.tab, entry.page)) {
            (Some(drawn), Some(now)) => drawn == now,
            _ => true,
        }
    }

    /// What a file was when its pages were added to this tab, if they were. Only this tab's: the same file can be another tab's own, which goes by its version, and saving that tab mustn't stop its own pages drawing.
    pub(crate) fn expected_stamp(&self, tab: u32, file: &std::path::Path) -> Option<FileStamp> {
        let doc = self.views.get(&tab)?;
        doc.map
            .files
            .iter()
            .position(|known| known == file)
            .filter(|at| *at > 0)
            .and_then(|at| doc.map.expected.get(at).copied().flatten())
    }

    /// Whether this file is the tab's own, held hot or warm: as its length is what a sleep file spoke for, and what a renderer given a password typed
    /// for the tab is kept for.
    pub(crate) fn is_own_file(&self, tab: u32, file: &std::path::Path) -> bool {
        self.views.get(&tab).is_some_and(|doc| doc.map.files.first().is_some_and(|own| own == file))
    }
}

// size: the page view's map of the tabs it holds and the tab showing, as `Reader` held them before R3.6 gathered them here.
#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<Docs>() <= 56, "the tabs the page view holds grew");

#[derive(Clone, Debug)]
pub(super) struct Issue {
    /// What the person's told, or `None` for a page waiting for its look on dark paper, which hasn't failed and isn't spoken of.
    pub(super) message: Option<String>,
    /// When it may have passed. `None` holds until the zoom changes or someone asks again.
    pub(super) try_again_at: Option<Instant>,
}

#[derive(Debug)]
pub(crate) struct DocView {
    pub(super) tab: u32,
    pub(super) layout: PageLayout,
    pub(super) map: PageMap,
    pub(super) place: DocPlace,
    pub(super) drawing: DocDrawing,
    /// **The stretch of a page in which the same contents entry is current,** as the core computed it for the page being read: the page, then the eye line's
    /// thousandths of it from `lo` up to but not including `hi`, then whether the view has woken the core since it was set. The core sets one with each
    /// pass for the tab shown. A scroll step that leaves it wakes the core once, by the wake a page change already has, and none after it does until the core
    /// sends a new band, so the compare on a scroll step is a layout lookup and two integer compares, on the view's own thread, under the lock the step already
    /// holds. Fractions of a page don't change with zoom, spread or window size, so a layout change needs no new band; a stale one (the contents changed, or a
    /// page moved by an edit) costs at most one wake the core answers with a new band, and can't hide a change for longer than the pass that follows what made
    /// it stale, which sends one every time.
    pub(super) band: Option<(u32, u16, u16, bool)>,
}

// size: one for each hot and warm tab. 672 bytes on 2026-11-13, before and after it was cut into its place and its drawing; 680 with the contents' band
// (2026-11-14, 8 bytes a tab held by the page view: the shown tab's is the only one set). Held there, so a field added to it, or to its layout or page map, is a deliberate change.
const _: () = assert!(size_of::<DocView>() <= 680, "a tab the page view holds grew");

/// Where the reader is in a tab, and how it's shown.
#[derive(Debug)]
pub(super) struct DocPlace {
    pub(super) magnification: f64,
    pub(super) anchor: (u32, f64),
    /// Which page the reader was on when this tab was left, and exactly where in it.
    pub(super) left_at: Option<(u32, (u32, f64))>,
    /// How far across the view is, when zoomed in.
    pub(super) scroll_x: f64,
    /// Exactly how far down it was, and the document's height then, so that while nothing about the layout has changed it comes back to the very pixel, where the anchor would come to within rounding.
    pub(super) exact_y: Option<(f64, f64)>,
    /// The document's width as the host was last told it: the window's, or a fold-out's while its row is in view.
    pub(super) width_shown: f64,
}

/// What's drawn of a tab, and what's known about drawing it.
#[derive(Debug)]
pub(super) struct DocDrawing {
    pub(super) tile_scale: f32,
    pub(super) issues: HashMap<u32, Issue>,
    pub(super) heavy: HashMap<u32, bool>,
    /// The whole file can't be drawn, in words the reader can read.
    pub(super) problem: Option<String>,
    pub(super) seen: HashSet<u32>,
    /// Jobs of a generation below this were about the pages as they were before the last edit, and what they found is thrown away.
    pub(super) floor: u64,
    /// It came up from a sleep file, so a page shows on what its size is taken to be before it's been asked.
    pub(super) size_on_trust: bool,
    /// Which version each of its files is, as a worker last said. **Nothing drawn of another version is shown, or goes in a sleep file:** that's checked as each goes on a layer, so however a file came to be replaced, and whatever was on its way, an old picture can't be what's showing.
    pub(super) versions: HashMap<PathBuf, u64>,
    /// **What it showed when it was left,** a picture of each page in view then, for its sleep file. What it left on screen is marked discardable and the system takes that back whenever it needs the memory, without a word, so without these a tab put to sleep after that slept with no picture and woke to bare paper. Taken while it's still on the screen, so it's all there. At most [`super::wake::MAX_LEFT_PICTURE_BYTES`] for each of the few tabs held off screen, the session's warm ones. Gone when it's shown again, its file's another version, the system says memory's short, or the window closes.
    pub(super) left_pictures: Option<Vec<RawPicture>>,
}

impl DocView {
    /// A tab it's never held, or that's gone cold, as the session has it: a saved place, if there is one, held to what a page can be.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn from_saved(
        tab: u32,
        pages: u32,
        page: u32,
        map: &PageMap,
        saved: Option<SavedPlace>,
        spread: Spread,
        margins: (f64, f64),
        floor: u64,
    ) -> Self {
        let mut layout = PageLayout::with_spread(pages, spread);
        layout.set_margins(margins.0, margins.1);
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
            place: DocPlace {
                magnification,
                anchor: (page, 0.0),
                left_at,
                scroll_x,
                exact_y,
                width_shown: 0.0,
            },
            drawing: DocDrawing {
                tile_scale: 0.0,
                issues: HashMap::new(),
                heavy: HashMap::new(),
                problem: None,
                seen: HashSet::new(),
                floor,
                size_on_trust: false,
                versions: HashMap::new(),
                left_pictures: None,
            },
            band: None,
        }
    }
}

impl DocDrawing {
    /// A page's been drawn, for the QA count of blank steps on pages seen before. Held to [`MAX_SEEN_REMEMBERED`].
    pub(super) fn note_seen(&mut self, page: u32) {
        if self.seen.len() >= MAX_SEEN_REMEMBERED && !self.seen.contains(&page) {
            self.seen.clear();
        }
        self.seen.insert(page);
    }
}
