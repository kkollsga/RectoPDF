//! **The tile cache:** what's drawn and kept, by key, which of it is on a layer, and what's let go, least recently used first, within the byte and count budgets every view of a window shares.

use super::host::HostQueue;
use super::{DocView, DrawKind, Op, Reader};
use crate::paper::PaperKind;
use recto_platform::surface::{PixelSurface, Purge};
use recto_proto::{TILE_EDGE, TileRect};
use recto_render_service::{LowResWhy, TileKind};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

/// Off-screen surfaces kept, by bytes and by count. Least recently used go first. Platform report §2.6.
pub(super) const MAX_KEPT_BYTES: usize = 256 * 1024 * 1024;
pub(super) const MAX_KEPT_SURFACES: usize = 1024;
/// Previews kept off screen, about 0.75 MiB each and discardable. Platform report §2.6 said 16. More costs nothing while the Mac has memory, and each one is a page that doesn't come back blank.
pub(super) const MAX_KEPT_PREVIEWS: usize = 128;
/// Entries of any kind, since a blank tile has no surface but is still remembered.
pub(super) const MAX_ENTRIES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct TileKey {
    pub(super) tab: u32,
    pub(super) page: u32,
    pub(super) scale_bits: u32,
    pub(super) column: u16,
    pub(super) row: u16,
    pub(super) paper: PaperKind,
    pub(super) preview: bool,
}

// size: one per tile in `cover`, up to `MAX_ENTRIES` of them and more for a region's tiles. Held at what it is today, so a field added to it is a deliberate change.
const _: () = assert!(size_of::<TileKey>() <= 20, "a tile's key in the cache grew");

#[derive(Debug)]
pub(super) struct LowResNote {
    pub(super) message: &'static str,
    pub(super) may_clear: bool,
    pub(super) at: Instant,
}

/// One drawn thing: a tile, a region of several, or a page's preview.
#[derive(Debug)]
pub(super) struct Entry {
    pub(super) id: u64,
    pub(super) tab: u32,
    pub(super) page: u32,
    pub(super) scale: f32,
    pub(super) paper: PaperKind,
    pub(super) preview: bool,
    pub(super) rect: TileRect,
    pub(super) surface: Option<PixelSurface>,
    /// A tile of one colour, that needs no surface.
    pub(super) rgb: Option<u32>,
    pub(super) crop: (f64, f64),
    pub(super) low_res: Option<LowResNote>,
    /// Which version of its file it was drawn from, as the worker said: see [`DocView::versions`]. `None` when nothing's said, as a picture from a sleep file written before this was known.
    pub(super) version: Option<u64>,
    pub(super) attached: bool,
    pub(super) discardable: bool,
    pub(super) last_used: u64,
}

#[derive(Debug)]
pub(super) struct TileCache {
    pub(super) entries: HashMap<u64, Entry>,
    pub(super) cover: HashMap<TileKey, u64>,
    pub(super) next_id: u64,
    /// Attached entries by page, on the hot tab only.
    pub(super) attached: HashMap<u32, Vec<u64>>,
    pub(super) clock: u64,
    /// Pictures that came from sleep files, which a worker's word on the version goes over.
    pub(super) from_sleep: HashSet<u64>,
    /// How many page views share the caps on what's kept: one, or each pane of a comparison, so several panes keep no more between them than one view does.
    pub(super) share: usize,
    /// Lists of a page's tiles on its layer, emptied when the page went off the view and kept for the next page that comes on, so scrolling a page in or out makes no list. At most [`SPARE_LISTS`], each of at most [`SPARE_LIST_ROOM`].
    pub(super) spare: Vec<Vec<u64>>,
}

/// Emptied lists of a page's tiles kept for pages to come: about as many pages as come and go in a scroll step, side by side and zoomed out.
pub(super) const SPARE_LISTS: usize = 8;
/// The most tiles' room a kept list may have; a larger one goes.
pub(super) const SPARE_LIST_ROOM: usize = 64;

impl TileCache {
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            cover: HashMap::new(),
            next_id: 1,
            attached: HashMap::new(),
            clock: 0,
            from_sleep: HashSet::new(),
            share: 1,
            spare: Vec::new(),
        }
    }

    /// The list of a page's tiles on its layer, a kept one if it had none.
    pub(super) fn attached_list(&mut self, page: u32) -> &mut Vec<u64> {
        let spare = &mut self.spare;
        self.attached.entry(page).or_insert_with(|| spare.pop().unwrap_or_default())
    }

    /// A page's list, off its layer, kept for the next page if there's room for it.
    pub(super) fn give_back_list(&mut self, mut list: Vec<u64>) {
        if self.spare.len() < SPARE_LISTS && list.capacity() > 0 && list.capacity() <= SPARE_LIST_ROOM {
            list.clear();
            self.spare.push(list);
        }
    }

    /// The bytes the kept lists hold.
    #[cfg(test)]
    pub(super) fn spare_bytes(&self) -> usize {
        self.spare.capacity() * size_of::<Vec<u64>>() + self.spare.iter().map(|list| list.capacity() * size_of::<u64>()).sum::<usize>()
    }

    /// The tiles on a page's layer.
    pub(super) fn attached_on(&self, page: u32) -> impl Iterator<Item = u64> + '_ {
        self.attached.get(&page).into_iter().flatten().copied()
    }

    /// Forgets an entry, and its surface with it.
    pub(super) fn forget(&mut self, id: u64, host: &mut HostQueue) {
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
                host.ops.push_back(Op::Detach { id });
            }
        }
    }

    pub(super) fn keys_of(entry: &Entry) -> Vec<TileKey> {
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

    /// Least recently used first, with a page's preview outliving its tiles: it's a fraction of their size, and it's what keeps a page seen a while ago from coming back blank. Bounded, as the entries are, and a look along them only when something has just gone off a layer.
    pub(super) fn evict(&mut self, host: &mut HostQueue) {
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
            self.forget(id, host);
        }
    }
}

impl Reader {
    /// Shares the caps on what's kept off screen with this many page views in all, as a comparison's panes do: each keeps its share, so between them they keep what one does. Anything over the new share goes now.
    pub fn set_share(&mut self, views: usize) {
        let views = views.clamp(1, 8);
        if views != self.cache.share {
            self.cache.share = views;
            self.cache.evict(&mut self.host);
        }
    }

    /// For QA: what the system does when it takes back memory marked discardable, with no word to the app. Every surface marked so is emptied. Says how many.
    pub fn empty_discardable(&mut self) -> usize {
        let mut emptied = 0;
        for entry in self.cache.entries.values().filter(|entry| entry.discardable) {
            if let Some(surface) = &entry.surface {
                surface.empty_now();
                emptied += 1;
            }
        }
        emptied
    }

    /// How many surfaces are held, for QA.
    pub fn entry_count(&self) -> usize {
        self.cache.entries.len()
    }

    /// Everything that can go without the page on screen going blank: what's off the layers, and at the worst, every picture too. Those that are discardable may have gone anyway, so this is what the system's asked of us, with nothing kept for later.
    pub fn memory_pressure(&mut self, critical: bool) {
        // Short enough that previews go: so do the pictures tabs were left with. Those that could go to sleep have, with them, by now.
        if critical {
            for doc in self.docs.values_mut() {
                doc.drawing.left_pictures = None;
            }
        }
        // Nothing's drawn for a tab off screen for a while, and what's drawing for one now stops.
        self.wake.draws.pressure();
        self.cancel_if_unwanted();
        let ids: Vec<u64> = self
            .cache
            .entries
            .values()
            .filter(|entry| !entry.attached && (critical || !entry.preview))
            .map(|entry| entry.id)
            .collect();
        for id in ids {
            self.cache.forget(id, &mut self.host);
        }
    }

    pub(super) fn drop_tab_entries(&mut self, tab: u32) {
        let ids: Vec<u64> = self.cache.entries.values().filter(|entry| entry.tab == tab).map(|entry| entry.id).collect();
        for id in ids {
            self.cache.forget(id, &mut self.host);
        }
    }

    pub(super) fn key(&self, doc: &DocView, page: u32, scale: f32, (column, row): (u16, u16)) -> TileKey {
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

    pub(super) fn preview_key(&self, doc: &DocView, page: u32) -> TileKey {
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

    pub(super) fn attach(&mut self, id: u64, below: bool) -> bool {
        self.cache.clock += 1;
        let clock = self.cache.clock;
        // Of another version of its file than the one there is now: it goes, and its page is drawn again.
        if let Some(entry) = self.cache.entries.get(&id)
            && !self.is_current(entry)
        {
            self.cache.forget(id, &mut self.host);
            return false;
        }
        let Some(entry) = self.cache.entries.get_mut(&id) else { return false };
        if entry.discardable {
            let kept = entry.surface.as_ref().is_none_or(|surface| surface.reclaim() == Purge::Kept);
            entry.discardable = false;
            if !kept {
                self.counts.discards_lost += 1;
                self.cache.forget(id, &mut self.host);
                return false;
            }
        }
        entry.attached = true;
        entry.last_used = clock;
        let (page, rect, scale, crop, rgb) = (entry.page, entry.rect, f64::from(entry.scale), entry.crop, entry.rgb);
        let surface = entry.surface.as_ref().map_or(0, PixelSurface::retained_for_host);
        self.cache.attached_list(page).push(id);
        // A tile the colour of the paper needs nothing drawn at all.
        if surface == 0 && rgb.is_none_or(|rgb| rgb == self.paper.rgb()) {
            return true;
        }
        self.host.ops.push_back(Op::Attach {
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

    pub(super) fn detach(&mut self, id: u64, page: u32) {
        if let Some(ids) = self.cache.attached.get_mut(&page) {
            ids.retain(|other| *other != id);
            if ids.is_empty()
                && let Some(list) = self.cache.attached.remove(&page)
            {
                self.cache.give_back_list(list);
            }
        }
        if let Some(entry) = self.cache.entries.get_mut(&id) {
            entry.attached = false;
            if entry.surface.is_some() || entry.rgb.is_some_and(|rgb| rgb != self.paper.rgb()) {
                self.host.ops.push_back(Op::Detach { id });
            }
            self.host.to_discard.push(id);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn keep_drawn(
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
        let id = self.cache.next_id;
        self.cache.next_id += 1;
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
        self.cache.clock += 1;
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
            last_used: self.cache.clock,
        };
        for key in TileCache::keys_of(&entry) {
            if let Some(old) = self.cache.cover.insert(key, id)
                && old != id
            {
                self.retire_replaced(old, tab);
            }
        }
        self.cache.entries.insert(id, entry);
        if let Some(doc) = self.docs.get_mut(&tab) {
            doc.drawing.note_seen(page);
        }
        id
    }

    /// Something drawn over an older copy of the same place. The older one comes off when no key points at it.
    pub(super) fn retire_replaced(&mut self, old: u64, tab: u32) {
        let Some(entry) = self.cache.entries.get(&old) else { return };
        let page = entry.page;
        let still_used = TileCache::keys_of(entry).iter().any(|key| self.cache.cover.get(key) == Some(&old));
        if !still_used {
            if entry.attached && self.hot == Some(tab) {
                self.detach(old, page);
            }
            if let Some(entry) = self.cache.entries.get(&old)
                && !entry.attached
            {
                self.cache.forget(old, &mut self.host);
            }
        }
    }
}
