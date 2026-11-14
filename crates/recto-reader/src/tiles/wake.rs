//! **Waking and sleep pictures:** what a tab's first frame shows from its sleep file, how its wake goes until it's sharp, and the pictures of its pages that go into the sleep file, from its previews or put together from its tiles.

use super::PageMap;
use super::cache::TileKey;
use super::{DocView, DrawKind, MAX_ZOOM, MIN_ZOOM, PREVIEW_DIVISOR, Reader, preview_fits};
use crate::layout::PageBox;
use crate::paper::PaperKind;
use recto_platform::surface::{PixelSurface, Purge};
use recto_proto::{BYTES_PER_PIXEL, MAX_SCALE, MIN_SCALE, TileRect, device_size};
use recto_render_service::TileKind;
use std::time::{Duration, Instant};

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
    /// The pages at the place the tab is at that a picture is owed to: the part of each in view being at least [`MIN_SHARE_OF_VIEW`] of the view, at most [`MAX_SLEEP_SHOTS`]. What a sleep file already holds is of use to a wake only if it has a picture of each.
    pub in_view: Vec<u32>,
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
pub(super) const MIN_SHARE_OF_VIEW: f64 = 0.05;

/// How long a wake's followed, for how long it took.
pub(super) const WAKE_FOLLOWED_FOR: Duration = Duration::from_secs(5);

/// Pictures that go in a sleep file: as many as `recto_session::sleep::MAX_SLEEP_PICTURES` says, which this crate doesn't know of. Six rows' worth of pages side by side, which are each a quarter of the pixels a page to a row has, so the bytes are about the same.
pub(super) const MAX_SLEEP_SHOTS: usize = 12;

/// What a tab held off screen keeps of the pictures it was left with, in all. A letter page at a Retina window's fit is 730 KiB, so a page to a row has its view kept, and a view of many small pages keeps the first few: the rest come from what's here when it goes to sleep, as before.
pub(super) const MAX_LEFT_PICTURE_BYTES: usize = 4 * 1024 * 1024;

/// What a wake had to show when it began: whether a picture covered the view at the place the tab was left at, and where it came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WakeStock {
    /// Not looked at yet: the tab hasn't been made the hot one.
    #[default]
    NotLooked,
    /// No picture covered the view: a bare page is what there is until the renderer answers, and no rule says otherwise.
    Nothing,
    /// A picture of every page in view was here, at least one of them from the tab's sleep file.
    SleepFile,
    /// A picture of every page in view was here, all of them from what the tile cache kept.
    Cache,
}

impl WakeStock {
    /// Whether the first frame was owed a page: the wake began with something covering the view.
    pub fn covered_the_view(self) -> bool {
        matches!(self, Self::SleepFile | Self::Cache)
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::NotLooked => "not_looked",
            Self::Nothing => "nothing",
            Self::SleepFile => "sleep_file",
            Self::Cache => "cache",
        }
    }
}

/// How a tab's wake went, for QA and the budgets: from being asked to the first frame showing something on every page, and to sharp. Written once for each wake, never for each frame, and small: [`WAKE_REPORT_BYTES`].
#[derive(Clone, Copy, Debug, Default)]
pub struct WakeReport {
    pub tab: u32,
    /// It came from a sleep file.
    pub from_sleep: bool,
    pub first_frame_us: Option<u64>,
    pub sharp_us: Option<u64>,
    /// Frames the host applied with a visible page bare, once it was at its place.
    pub blank_frames: u32,
    /// What there was to show when the wake began.
    pub had: WakeStock,
    /// How many pictures the tab's sleep file gave, and how many of those lie in the view at the place the tab was left at.
    pub sleep_pictures: u16,
    pub sleep_in_view: u16,
    /// Of the bare frames, those where a picture of every page in view was here and wasn't on a layer: the picture was there, and the frame was bare anyway.
    pub bare_with_picture: u16,
}

/// The size of a [`WakeReport`], held: one for each page view, kept as of the last wake.
pub const WAKE_REPORT_BYTES: usize = 48;
const _: () = assert!(std::mem::size_of::<WakeReport>() <= WAKE_REPORT_BYTES);

/// **Where a tab off screen is, as its sleep file and its wake have it:** the pages there (at most [`MAX_SLEEP_SHOTS`]), from where, and how tall its own view is: the window's height at the zoom it was left at, which the tab showing's view, at the tab showing's zoom, isn't. Where its wake sends it, by the one rule, [`DocView::place_y`]: its exact place while its layout is the one that place was taken in, and otherwise as far down its page as it was. A tab off screen's anchor is the one it was left at, as only the tab showing's moves, and coming back puts back the one noted as it left, which is the same. One reckoning, so what's drawn for a tab before it sleeps ([`super::sleep_draw`]) is what its sleep file takes and what its wake shows, a size learned meanwhile included.
// R3.6: a `Reader` operation the ratchet's method count can't see, as a free function taking the whole page view (it reads the viewport and the tab's view).
pub(super) fn left_place(reader: &Reader, doc: &DocView) -> (Vec<u32>, f64, f64) {
    let y = doc.place_y();
    let window_height = reader.viewport.height * reader.viewport.magnification.max(f64::MIN_POSITIVE);
    let height = (window_height / doc.place.magnification.clamp(MIN_ZOOM, MAX_ZOOM)).max(1.0);
    let pages = if doc.layout.count() == 0 {
        Vec::new()
    } else {
        // Only as many as a sleep file takes: a hostile file's slivers can put thousands in view.
        doc.layout.pages_between(y.max(0.0), y.max(0.0) + height).take(MAX_SLEEP_SHOTS).collect()
    };
    (pages, y.max(0.0), height)
}

/// Of these pages, those a picture is owed to: the part of each in the view from `from`, `height` tall, being at least [`MIN_SHARE_OF_VIEW`] of it.
pub(super) fn in_view_of(doc: &DocView, pages: &[u32], from: f64, height: f64) -> Vec<u32> {
    pages
        .iter()
        .copied()
        .filter(|page| {
            let rect = doc.layout.rect(*page);
            let showing = rect.y.max(from)..(rect.y + rect.height).min(from + height);
            showing.end - showing.start >= height * MIN_SHARE_OF_VIEW
        })
        .collect()
}

/// Copies a tile's pixels into a picture of its page, a step-sized box of them to each pixel. A pixel past the edge of either is left.
#[allow(clippy::too_many_arguments)]
pub(super) fn shrink_into(
    source: &[u8],
    stride: usize,
    used_width: u32,
    used_height: u32,
    step: u32,
    at: (u32, u32),
    picture: &mut [u8],
    width: u32,
    height: u32,
) {
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
    /// A tab left at a place it has no picture of, and what's drawn for it.
    pub(super) draws: super::sleep_draw::SleepDraws,
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
            draws: super::sleep_draw::SleepDraws::default(),
        }
    }

    /// **What a tab's first frame shows, from its sleep file, ahead of [`Reader::activate`].** Pictures of the pages at its place, kept as those pages' previews, so they go on as a preview does, and the same rules take them off. The tab's been asked for from here, for how long it takes.
    pub(crate) fn give_sleep(&mut self, tab: u32, view: SleepView) {
        self.sleep_view = Some((tab, view));
    }

    /// A tab's being woken, as of now, with `hot` the tab showing ([`Reader::note_wake`] says it, so a caller can't give another). For how long it takes.
    pub(super) fn note_wake(&mut self, hot: Option<u32>, tab: u32, began: Instant) {
        // Said when it was asked for, and said again on its way to the screen: the first is when it began.
        if hot != Some(tab) && !self.waking.is_some_and(|(_, report)| report.tab == tab) {
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

    /// The window's gone up. From here, a bare page is one that's seen.
    pub(crate) fn window_up(&mut self) {
        self.window_up = true;
    }

    /// How the last wake went.
    pub(crate) fn last_wake(&self) -> Option<WakeReport> {
        self.waking.map(|(_, report)| report).or(self.last_wake)
    }

    /// The wake under way is over: someone's gone somewhere else, or it's sharp.
    pub(super) fn finish_wake(&mut self) {
        if let Some((_, report)) = self.waking.take() {
            self.last_wake = Some(report);
        }
    }

    /// Whether a picture of each of these pages, the part of them that is in view being at least [`MIN_SHARE_OF_VIEW`] of it, is here for the tab, on a layer or not, from a sleep file or drawn. Looks at the cache once, and allocates only for the bare frames of a wake.
    // R3.6: a `Reader` operation the ratchet's method count can't see: an associated function of `Waking` taking the whole page view, which reaches every field `Reader`'s own methods do.
    pub(super) fn pictures_exist_for(reader: &Reader, doc: &DocView, pages: impl Iterator<Item = u32>) -> bool {
        let (top, bottom) = (reader.viewport.y, reader.viewport.y + reader.viewport.height);
        let have: std::collections::HashSet<u32> = reader
            .cache
            .entries
            .values()
            // Only what goes on a page of this paper: a picture for the other is never put on one.
            .filter(|entry| entry.tab == doc.tab && entry.paper == reader.paper && (entry.surface.is_some() || entry.rgb.is_some()))
            .map(|entry| entry.page)
            .collect();
        pages.into_iter().all(|page| {
            let rect = doc.layout.rect(page);
            let showing = rect.y.max(top)..(rect.y + rect.height).min(bottom);
            showing.end - showing.start < reader.viewport.height * MIN_SHARE_OF_VIEW || have.contains(&page)
        })
    }

    /// **What the wake under way had to show,** once, as its tab is made the hot one and has been sent to its place: the pages in view there, as a sleep file would have taken them, and whether a picture of each is here and where from.
    // R3.6: a `Reader` operation the ratchet's method count can't see, as `pictures_exist_for` above.
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
            doc.layout.pages_between(y, y + height).take(MAX_SLEEP_SHOTS).collect()
        };
        let have: std::collections::HashMap<u32, bool> = reader
            .cache
            .entries
            .values()
            // A picture for the other paper isn't one this wake can show (a sleep file written before the look changed).
            .filter(|entry| entry.tab == tab && entry.paper == reader.paper && (entry.surface.is_some() || entry.rgb.is_some()))
            .fold(std::collections::HashMap::new(), |mut have, entry| {
                *have.entry(entry.page).or_insert(false) |= reader.cache.from_sleep.contains(&entry.id);
                have
            });
        let sleep_in_view = in_view.iter().filter(|page| have.get(page) == Some(&true)).count();
        let whole = !in_view.is_empty() && in_view.iter().all(|page| have.contains_key(page));
        let had = match (whole, sleep_in_view > 0) {
            (false, _) => WakeStock::Nothing,
            (true, true) => WakeStock::SleepFile,
            (true, false) => WakeStock::Cache,
        };
        if let Some((_, report)) = &mut reader.wake.waking {
            report.had = had;
            report.sleep_in_view = u16::try_from(sleep_in_view).unwrap_or(u16::MAX);
        }
    }
}

impl Reader {
    /// Whether every visible page is showing something, at the place, with nothing on its way to the host: what a window can go up with. It'll sharpen from there.
    pub fn is_showable(&self) -> bool {
        if self.place.jump.is_some() || !self.host.ops.is_empty() || self.host.unapplied || self.viewport.height <= 0.0 {
            return false;
        }
        let Some(doc) = self.docs.shown() else { return false };
        doc.drawing.problem.is_none() && self.pages_all_covered(doc)
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
        if self.docs.hot != Some(report.tab) {
            self.wake.waking = None;
            return;
        }
        // Past this, it's not the wake that's being looked at any more.
        if began.elapsed() > WAKE_FOLLOWED_FOR {
            self.wake.finish_wake();
            return;
        }
        let Some(doc) = self.docs.shown() else { return };
        if self.place.jump.is_none() && self.viewport.height > 0.0 {
            let covered = self.pages_all_covered(doc);
            if covered && report.first_frame_us.is_none() {
                report.first_frame_us = Some(u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX));
            } else if !covered && report.first_frame_us.is_none() && self.wake.window_up {
                report.blank_frames += 1;
                if Waking::pictures_exist_for(self, doc, self.visible_pages(doc)) {
                    report.bare_with_picture = report.bare_with_picture.saturating_add(1);
                }
            }
        }
        if report.first_frame_us.is_some() && self.is_settled() {
            report.sharp_us = Some(u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX));
            self.wake.last_wake = Some(report);
            self.wake.waking = None;
            return;
        }
        self.wake.waking = Some((began, report));
    }

    /// **What a tab's sleep file gets from what's here.** Sizes, and a picture for each page at its place: its preview, if the system has left it, or else put together from whatever of its tiles are here, so a heavy page, which has no preview, has one too. A short lock: a picture's a few milliseconds at the worst.
    ///
    /// `now` is what the session says the tab's pages are now. If its file isn't what this is a view of, what's here is of another file, and nothing goes in its sleep file. **If its pages aren't the pages here** (deleted, moved, added or turned while it was off screen, which a view held warm hears of only when it's shown again), what's here is filed under places that are other pages now, and a picture of the wrong page is worse than bare paper: its facts go, and no picture or size.
    pub fn sleep_material(&mut self, tab: u32, now: &PageMap) -> Option<SleepMaterial> {
        let doc = self.docs.views.get(&tab)?;
        if doc.map.own != now.own {
            return None;
        }
        if !doc.map.same_pages(now) || doc.map.turns != now.turns {
            return Some(SleepMaterial {
                version: None,
                usual: None,
                sizes: Vec::new(),
                pictures: Vec::new(),
                in_view: Vec::new(),
            });
        }
        let usual = doc.layout.usual_size();
        let sizes = doc.layout.other_sizes();
        let version = doc.map.files.first().and_then(|file| doc.drawing.versions.get(file)).copied();
        let (pages, from, height): (Vec<u32>, f64, f64) = if self.docs.hot == Some(tab) {
            (self.visible_pages(doc).take(MAX_SLEEP_SHOTS).collect(), self.viewport.y, self.viewport.height)
        } else {
            left_place(self, doc)
        };
        let in_view = in_view_of(doc, &pages, from, height);
        // What it was left with first, page by page: what's here now may have been taken by the system since.
        let mut left = doc.drawing.left_pictures.clone().unwrap_or_default();
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
            in_view,
        })
    }

    /// A picture of each page of the tab showing that's in view, while it's on the screen. Nothing if no worker's said which version it's of, as there'd be no saying what they're of.
    pub(super) fn pictures_in_view(&mut self, tab: u32) -> Vec<RawPicture> {
        let Some(doc) = self.docs.views.get(&tab) else { return Vec::new() };
        if doc.map.files.first().and_then(|file| doc.drawing.versions.get(file)).is_none() {
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

    pub(super) fn picture_of(&mut self, tab: u32, page: u32) -> Option<RawPicture> {
        let paper = self.paper;
        let doc = self.docs.views.get(&tab)?;
        let preview_scale = ((doc.layout.fit() * self.viewport.backing / PREVIEW_DIVISOR) as f32).clamp(MIN_SCALE, MAX_SCALE);
        let size = doc.layout.raw_size(page);
        let paper_rgb = paper.rgb();
        // A preview as it is, if there's one.
        let preview = self.cache.cover.get(&TileKey::preview(doc.tab, page, self.paper)).copied().filter(|id| {
            self.cache
                .entries
                .get(id)
                .is_some_and(|entry| entry.version.is_some() && self.docs.is_current(entry))
        });
        if let Some(id) = preview
            && let Some(picture) = self.copy_preview(id)
        {
            return Some(picture);
        }
        // Otherwise the sharpest tiles there are of this page, at whole steps down.
        let ids: Vec<u64> = self
            .cache
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
                    && self.docs.is_current(entry)
            })
            .map(|entry| entry.id)
            .collect();
        let finest = ids
            .iter()
            .filter_map(|id| self.cache.entries.get(id))
            .map(|entry| entry.scale)
            .fold(0.0f32, f32::max);
        if finest <= 0.0 {
            return None;
        }
        let step = (finest / preview_scale).round().max(1.0) as u32;
        let scale = finest / step as f32;
        let (width, height) = device_size(size.width, size.height, scale).ok()?;
        if !preview_fits(width, height) {
            return None;
        }
        let bytes = u64::from(width) * u64::from(height) * u64::from(BYTES_PER_PIXEL);
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
            let Some(entry) = self.cache.entries.get(&id) else { continue };
            if entry.scale != finest {
                continue;
            }
            let was_discardable = entry.discardable;
            if was_discardable && entry.surface.as_ref().is_none_or(|surface| surface.reclaim() != Purge::Kept) {
                self.counts.discards_lost += 1;
                self.cache.forget(id, &mut self.host);
                continue;
            }
            let Some(entry) = self.cache.entries.get_mut(&id) else { continue };
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

    pub(super) fn copy_preview(&mut self, id: u64) -> Option<RawPicture> {
        let entry = self.cache.entries.get(&id)?;
        let was_discardable = entry.discardable;
        if was_discardable && entry.surface.as_ref().is_none_or(|surface| surface.reclaim() != Purge::Kept) {
            self.counts.discards_lost += 1;
            self.cache.forget(id, &mut self.host);
            return None;
        }
        let entry = self.cache.entries.get_mut(&id)?;
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

    /// What came from a sleep file goes in, if it's for this tab and there's nothing better here. Anything for another tab has been overtaken.
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
        let count = doc.layout.count();
        let mut any = false;
        let mut gave = 0u16;
        for shot in view.pictures {
            if shot.page >= count {
                continue;
            }
            // Its size is known, or it wouldn't be shown.
            if let Some(doc) = self.docs.views.get_mut(&tab) {
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
            if let Some(entry) = self.cache.entries.get_mut(&id) {
                entry.version = view.version;
            }
            self.cache.from_sleep.insert(id);
            gave += 1;
            any = true;
        }
        if let Some((_, report)) = &mut self.wake.waking
            && report.tab == tab
        {
            report.from_sleep = any;
            report.sleep_pictures = gave;
        }
    }
}
