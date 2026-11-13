//! **What's drawn next,** most wanted first: sizes, the links under the pointer, the text a selection is waiting on, then each visible page's preview and tiles, and text when there's nothing else; and whether what's running is still wanted.

use super::{DocView, DrawKind, JobPlan, MAX_PREVIEW_BYTES, PREVIEW_DIVISOR, PlanKind, Reader, SIZES_AROUND, TRY_AGAIN_AFTER};
use crate::selection::Urgency;
use recto_proto::{BYTES_PER_PIXEL, MAX_SCALE, MIN_SCALE, TILE_EDGE, TileRect, device_size};
use recto_render_service::plan_regions;
use std::time::Instant;

impl Reader {
    /// Whether what's running is still of use: on the tab showing, its page in view, and if it's a tile, at the zoom the view is at now and touching the view.
    pub fn running_is_wanted(&self) -> bool {
        let Some(running) = &self.work.running else { return true };
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
    pub(super) fn view_needs_something(&self) -> bool {
        let Some(doc) = self.doc() else { return false };
        let scale = doc.tile_scale;
        if scale <= 0.0 {
            return false;
        }
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        if self
            .pointer
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

    /// The next thing to do, most wanted first, or `None`, when the engine parks. Only what's visible is drawn: drawing ahead comes in a later phase.
    pub fn next_plan(&mut self) -> Option<JobPlan> {
        if !self.window_open || self.work.hold_drawing {
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
            let generation = self.work.fresh_generation();
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
        if let Some(page) = self.pointer.linking.next_wanted()
            && let Some(doc) = self.docs.get(&tab)
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
                let generation = self.work.fresh_generation();
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
                let generation = self.work.fresh_generation();
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
                let generation = self.work.fresh_generation();
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
            let generation = self.work.fresh_generation();
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
    pub(super) fn text_plan(&mut self, tab: u32, urgency: Urgency) -> Option<JobPlan> {
        let doc = self.docs.get(&tab)?;
        let visible: Vec<u32> = self.visible_pages(doc).collect();
        let wanted = self
            .pointer
            .selecting
            .wanted(urgency, &visible, doc.layout.count(), |page| doc.heavy.get(&page).copied().unwrap_or(false))?;
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
        (u64::from(width) * u64::from(height) * u64::from(BYTES_PER_PIXEL) <= MAX_PREVIEW_BYTES)
            .then_some((preview_scale, TileRect { x: 0, y: 0, width, height }))
    }
}
