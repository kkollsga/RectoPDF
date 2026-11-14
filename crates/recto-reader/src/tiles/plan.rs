//! **What's drawn next,** most wanted first: sizes, the links under the pointer, the text a selection is waiting on, then each visible page's preview and tiles, and text when there's nothing else; and whether what's running is still wanted.

use super::cache::TileKey;
use super::sleep_draw::SleepDraws;
use super::{DocView, DrawKind, JobPlan, PREVIEW_DIVISOR, PlanKind, Reader, SIZES_AROUND, TRY_AGAIN_AFTER, preview_fits};
use crate::selection::Urgency;
use recto_proto::{MAX_SCALE, MIN_SCALE, TILE_EDGE, TileRect, device_size};
use recto_render_service::plan_regions;
use std::time::Instant;

impl Reader {
    /// Whether what's running is still of use: on the tab showing, its page in view, and if it's a tile, at the zoom the view is at now and touching the view.
    pub fn running_is_wanted(&self) -> bool {
        let Some(running) = &self.work.running else { return true };
        // Drawn for a tab left at a place it has no picture of, as long as the view wants nothing: a size, a weight or a preview, the only jobs asked for that. A tile or a region for the tab just left is the tab showing's no longer, and goes.
        if !running.text && (running.preview || running.rect.is_none()) && SleepDraws::wants(self, running.tab, running.position) {
            return true;
        }
        if running.text {
            // Only a check goes for what's more wanted: it can take seconds, where reading is quick, and it's what's asked for when there's nothing better to do.
            return self.docs.hot == Some(running.tab) && self.window_open && !(running.check && self.view_needs_something());
        }
        self.docs.hot == Some(running.tab)
            && self.window_open
            && self.docs.shown().is_some_and(|doc| {
                self.visible_pages(doc).contains(&running.position)
                    && (running.preview
                        || running
                            .rect
                            .is_none_or(|rect| running.scale == doc.drawing.tile_scale && self.touches_view(doc, running.position, rect, running.scale)))
            })
    }

    /// Whether what's on screen wants something that isn't there yet: a tile, or the text under a selection being made. A document that failed whole
    /// (it needs its password, say) wants nothing until it's opened again, as nothing's planned for it, so it doesn't hold up a tab left owed pictures.
    pub(super) fn view_needs_something(&self) -> bool {
        let Some(doc) = self.docs.shown() else { return false };
        let scale = doc.drawing.tile_scale;
        if scale <= 0.0 || doc.drawing.problem.is_some() {
            return false;
        }
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        if self
            .pointer
            .selecting
            .wanted(Urgency::Now, &visible, doc.layout.count(), |page| {
                doc.drawing.heavy.get(&page).copied().unwrap_or(false)
            })
            .is_some()
        {
            return true;
        }
        visible.iter().any(|&page| {
            if doc.drawing.issues.contains_key(&page) {
                return false;
            }
            if !doc.layout.is_known(page) {
                return true;
            }
            let Ok(cells) = self.visible_cells(doc, page, scale) else { return false };
            cells
                .into_iter()
                .any(|cell| !self.cache.cover.contains_key(&TileKey::tile(doc.tab, page, scale, cell, self.paper)))
        })
    }

    pub(super) fn cancel_if_unwanted(&mut self) {
        if self.running_is_wanted() {
            return;
        }
        let Some(running) = &mut self.work.running else { return };
        if !running.cancel_asked {
            running.cancel_asked = true;
            self.counts.cancels_asked += 1;
            if let Some(cancel) = &running.cancel {
                cancel.cancel_before(running.generation + 1);
            }
        }
    }

    /// The next thing to do, most wanted first, or `None`, when the engine parks. Only what's visible is drawn: drawing ahead comes in a later phase. When the tab showing has nothing to do, a tab left at a place it has no picture of has that drawn ([`SleepDraws`]).
    pub fn next_plan(&mut self) -> Option<JobPlan> {
        if !self.window_open || self.work.hold_drawing {
            return None;
        }
        hot_plan(self).or_else(|| SleepDraws::plan(self))
    }

    /// Sizes first: nothing is shown or drawn before them. The first page known sets the fit, so where a reader lands is asked first, and then the pages around it. `None` when there's nothing to do now, as [`Self::next_plan`] says, and `Some(None)` to go on.
    fn size_plan(&mut self, tab: u32, visible: std::ops::RangeInclusive<u32>) -> Option<Option<JobPlan>> {
        let doc = self.docs.views.get(&tab)?;
        let (first, last) = (*visible.start(), *visible.end());
        let around = first.saturating_sub(SIZES_AROUND)..=(last + SIZES_AROUND).min(doc.layout.count().saturating_sub(1));
        // A page whose size couldn't be read has its reason on it, and isn't asked again: nothing about it will be different straight away.
        let waiting_on_issue = |page: &u32| {
            doc.drawing
                .issues
                .get(page)
                .is_some_and(|issue| !issue.try_again_at.is_some_and(|at| Instant::now() >= at))
        };
        if let Some(page) = visible
            .clone()
            .chain(around)
            .find(|page| !doc.layout.is_known(*page) && !waiting_on_issue(page))
        {
            let (file, source_page) = doc.map.source(page)?;
            let generation = self.work.fresh_generation();
            return Some(Some(JobPlan {
                tab,
                position: page,
                file,
                source_page,
                generation,
                kind: PlanKind::Size,
            }));
        }
        Some(None)
    }

    /// The links under the pointer come first: they're quick, and the pointer's waiting on them for its look.
    fn links_plan(&mut self, tab: u32) -> Option<JobPlan> {
        if let Some(page) = self.pointer.linking.next_wanted()
            && let Some(doc) = self.docs.views.get(&tab)
            && page < doc.layout.count()
            && let Some((file, source_page)) = doc.map.source(page)
        {
            let generation = self.work.fresh_generation();
            return Some(JobPlan {
                tab,
                position: page,
                file,
                source_page,
                generation,
                kind: PlanKind::Links,
            });
        }
        None
    }

    /// A visible page's heaviness, its preview or its next tiles, if it wants any. `None` when there's nothing to do now, as [`Self::next_plan`] says, and `Some(None)` to go on to the next page.
    fn page_plan(&mut self, tab: u32, page: u32, scale: f32, now: Instant) -> Option<Option<JobPlan>> {
        let doc = self.docs.views.get(&tab)?;
        if let Some(issue) = doc.drawing.issues.get(&page)
            && !issue.try_again_at.is_some_and(|at| now >= at)
        {
            return Some(None);
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
            return Some(None);
        };
        let missing: Vec<(u16, u16)> = cells
            .iter()
            .copied()
            .filter(|cell| {
                let key = TileKey::tile(doc.tab, page, scale, *cell, self.paper);
                match self.cache.cover.get(&key).and_then(|id| self.cache.entries.get(id)) {
                    None => true,
                    Some(entry) => entry
                        .low_res
                        .as_ref()
                        .is_some_and(|low| low.may_clear && now.duration_since(low.at) >= TRY_AGAIN_AFTER),
                }
            })
            .collect();
        if missing.is_empty() {
            return Some(None);
        }
        let (file, source_page) = doc.map.source(page)?;
        // Asked once, and it loads the page, which drawing would do anyway.
        if !doc.drawing.heavy.contains_key(&page) {
            let generation = self.work.fresh_generation();
            return Some(Some(JobPlan {
                tab,
                position: page,
                file,
                source_page,
                generation,
                kind: PlanKind::Traits,
            }));
        }
        let heavy = doc.drawing.heavy.get(&page).copied().unwrap_or(false);
        // A heavy page's cost is per pass, so a preview would cost it as much again. Any other page gets one however little of it shows: a page first seen as a sliver at the bottom of the window, with no preview, goes blank when that sliver's tile scrolls off.
        let preview_wanted = !heavy && !self.cache.cover.contains_key(&TileKey::preview(doc.tab, page, self.paper));
        if preview_wanted && let Some((preview_scale, rect)) = self.preview_rect(doc, page, scale) {
            let paper = self.paper;
            let generation = self.work.fresh_generation();
            return Some(Some(JobPlan {
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
            }));
        }
        let size = doc.layout.raw_size(page);
        let (width, height) = device_size(size.width, size.height, scale).ok()?;
        if heavy && missing.len() > 1 {
            let wanted: Vec<(u32, u32)> = missing.iter().map(|&(column, row)| (u32::from(column), u32::from(row))).collect();
            let mut sorted = wanted;
            sorted.sort_unstable();
            let rect = plan_regions(&sorted, (width, height)).into_iter().next()?;
            let paper = self.paper;
            let generation = self.work.fresh_generation();
            return Some(Some(JobPlan {
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
            }));
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
        let generation = self.work.fresh_generation();
        Some(Some(JobPlan {
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
        }))
    }

    /// Reading text for a selection, a copy or the pointer, if any is wanted at this urgency.
    pub(super) fn text_plan(&mut self, tab: u32, urgency: Urgency) -> Option<JobPlan> {
        let doc = self.docs.views.get(&tab)?;
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        let wanted = self.pointer.selecting.wanted(urgency, &visible, doc.layout.count(), |page| {
            doc.drawing.heavy.get(&page).copied().unwrap_or(false)
        })?;
        let (file, source_page) = doc.map.source(wanted.page)?;
        let generation = self.work.fresh_generation();
        Some(JobPlan {
            tab,
            position: wanted.page,
            file,
            source_page,
            generation,
            kind: PlanKind::Text { check: wanted.check },
        })
    }

    pub(super) fn preview_rect(&self, doc: &DocView, page: u32, scale: f32) -> Option<(f32, TileRect)> {
        let size = doc.layout.raw_size(page);
        let preview_scale = ((doc.layout.fit() * self.viewport.backing / PREVIEW_DIVISOR) as f32).clamp(MIN_SCALE, MAX_SCALE);
        // A zoom already that low has nothing to gain from it.
        if f64::from(preview_scale) * 1.5 >= f64::from(scale) {
            return None;
        }
        let (width, height) = device_size(size.width, size.height, preview_scale).ok()?;
        preview_fits(width, height).then_some((preview_scale, TileRect { x: 0, y: 0, width, height }))
    }
}

/// What the tab showing wants next, most wanted first.
// R3.6: a `Reader` operation the ratchet's method count can't see, as a free function taking the whole page view: it reaches every field `Reader`'s own methods do.
fn hot_plan(reader: &mut Reader) -> Option<JobPlan> {
    let tab = reader.docs.hot?;
    let doc = reader.docs.views.get(&tab)?;
    if doc.drawing.problem.is_some() || reader.viewport.height <= 0.0 {
        return None;
    }
    let visible = reader.visible_pages(doc);
    let (first, last) = (*visible.start(), *visible.end());
    if first > last {
        return None;
    }
    if let Some(plan) = reader.size_plan(tab, visible)? {
        return Some(plan);
    }
    let now = Instant::now();
    let scale = reader.docs.views.get(&tab)?.drawing.tile_scale;
    if scale <= 0.0 {
        return None;
    }
    if let Some(plan) = reader.links_plan(tab) {
        return Some(plan);
    }
    // A selection the reader is making comes before tiles: it answers their hand.
    if let Some(plan) = reader.text_plan(tab, Urgency::Now) {
        return Some(plan);
    }
    let doc = reader.docs.views.get(&tab)?;
    let pages: Vec<u32> = reader.visible_pages(doc).collect();
    for page in pages {
        if let Some(plan) = reader.page_plan(tab, page, scale, now)? {
            return Some(plan);
        }
    }
    reader.text_plan(tab, Urgency::WhenIdle)
}
