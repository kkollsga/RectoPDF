//! **The screen brought into line with what's wanted,** on every scroll and whenever something's drawn: the pages' layers placed or taken off, the tiles in view put on them from what's kept, at most a turn's worth, and those scrolled out of view taken off; with what the reader is told about the pages in view, and the count of steps that left one blank.

use super::host::Placement;
use super::scratch::Scratch;
use super::{CANT_DRAW, DocView, Op, Reader, SCALE_SLACK};
use recto_proto::{TILE_EDGE, device_size};
use std::time::Instant;

impl Reader {
    /// Which tiles of a page the view touches, nearest the middle first.
    pub(super) fn visible_cells(&self, doc: &DocView, page: u32, scale: f32) -> Result<Vec<(u16, u16)>, ()> {
        let mut cells = Vec::new();
        self.visible_cells_into(doc, page, scale, &mut cells)?;
        Ok(cells)
    }

    /// [`Self::visible_cells`], into a list that's kept, which is emptied first.
    pub(super) fn visible_cells_into(&self, doc: &DocView, page: u32, scale: f32, cells: &mut Vec<(u16, u16)>) -> Result<(), ()> {
        cells.clear();
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
            return Ok(());
        }
        let edge = f64::from(TILE_EDGE);
        let (first_column, last_column) = ((left / edge) as u32, ((right - 1e-6) / edge) as u32);
        let (first_row, last_row) = ((top / edge) as u32, ((bottom - 1e-6) / edge) as u32);
        let (middle_x, middle_y) = ((left + right) / 2.0, (top + bottom) / 2.0);
        cells.reserve(((last_column - first_column + 1) * (last_row - first_row + 1)) as usize);
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
        Ok(())
    }

    /// Brings the screen into line with what's wanted, attaching at most `budget` tiles. The rest wait for the next turn.
    pub(super) fn refresh(&mut self, budget: usize) {
        // The lists it fills are the reader's, kept from step to step: a step over pages already drawn allocates nothing.
        let mut scratch = std::mem::take(&mut self.scratch);
        self.refresh_with(budget, &mut scratch);
        scratch.reset();
        self.scratch = scratch;
    }

    /// [`Self::refresh`], with the lists it fills.
    pub(super) fn refresh_with(&mut self, mut budget: usize, scratch: &mut Scratch) {
        self.host.more_to_attach = false;
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
        scratch.visible.extend(self.visible_pages(doc).filter(|page| doc.layout.is_known(*page)));
        // A tab that's come from a sleep file knows what size its pages are likely to be, so with or without a picture its place is there on the paper from the first frame, and real sizes follow.
        if doc.size_on_trust {
            scratch.on_paper.extend(self.visible_pages(doc));
        } else {
            scratch.on_paper.extend_from_slice(&scratch.visible);
        }

        scratch
            .going_pages
            .extend(self.host.shown.keys().copied().filter(|page| !scratch.on_paper.contains(page)));
        for &page in &scratch.going_pages {
            self.hide_page(page);
        }
        let Some(doc) = self.docs.get(&tab) else { return };
        let paper_rgb = self.paper.rgb();
        let device = self.viewport.magnification * self.viewport.backing;
        for &page in &scratch.on_paper {
            let rect = doc.layout.rect(page);
            let size = doc.layout.raw_size(page);
            // On whole device pixels, so a tile's pixels land on the screen's.
            let snap = |value: f64| if device > 0.0 { (value * device).round() / device } else { value };
            scratch.placements.push((
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
        for &(page, placement) in &scratch.placements {
            if self.host.shown.get(&page) != Some(&placement) {
                self.host.shown.insert(page, placement);
                self.host.ops.push_back(Op::ShowPage {
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
        for &page in &scratch.visible {
            let Some(doc) = self.docs.get(&tab) else { return };
            let preview = self.preview_key(doc, page);
            let bare = !self.cache.attached.get(&page).is_some_and(|ids| !ids.is_empty());
            if bare
                && let Some(&id) = self.cache.cover.get(&preview)
                && self.attach(id, true)
            {
                budget = budget.saturating_sub(1);
            }
        }
        let now = Instant::now();
        for &page in &scratch.visible {
            let Some(doc) = self.docs.get(&tab) else { return };
            if doc.issues.contains_key(&page) && !doc.issues[&page].try_again_at.is_some_and(|at| now >= at) {
                continue;
            }
            let Ok(()) = self.visible_cells_into(doc, page, scale, &mut scratch.cells) else {
                continue;
            };
            scratch.keys.clear();
            scratch.keys.extend(scratch.cells.iter().map(|cell| self.key(doc, page, scale, *cell)));
            let preview = self.preview_key(doc, page);
            let mut missing = false;
            scratch.wanted.clear();
            for key in &scratch.keys {
                let Some(&id) = self.cache.cover.get(key) else {
                    missing = true;
                    continue;
                };
                scratch.wanted.push(id);
                let attached = self.cache.entries.get(&id).is_some_and(|entry| entry.attached);
                if !attached {
                    if budget == 0 {
                        self.host.more_to_attach = true;
                        missing = true;
                    } else if self.attach(id, false) {
                        budget -= 1;
                    } else {
                        // The OS took it. Drawn again.
                        missing = true;
                    }
                }
            }
            let preview_id = self.cache.cover.get(&preview).copied();
            if missing {
                if let Some(id) = preview_id {
                    let attached = self.cache.entries.get(&id).is_some_and(|entry| entry.attached);
                    if !attached && budget > 0 && self.attach(id, true) {
                        budget -= 1;
                    }
                }
            } else {
                scratch.going.clear();
                scratch.going.extend(self.cache.attached_on(page).filter(|id| !scratch.wanted.contains(id)));
                for &id in &scratch.going {
                    self.detach(id, page);
                }
            }
            // Tiles scrolled out of a tall page come off, even while others are still coming.
            if !missing {
                continue;
            }
            let Some(doc) = self.docs.get(&tab) else { return };
            scratch.going.clear();
            scratch.going.extend(self.cache.attached_on(page).filter(|id| {
                self.cache.entries.get(id).is_some_and(|entry| {
                    !entry.preview && entry.scale == scale && entry.paper == self.paper && !self.touches_view(doc, page, entry.rect, scale)
                })
            }));
            for &id in &scratch.going {
                self.detach(id, page);
            }
        }
        self.update_note();
    }

    pub(super) fn update_note(&mut self) {
        let note = self.doc().and_then(|doc| {
            if let Some(problem) = &doc.problem {
                return Some(problem.clone());
            }
            for page in self.visible_pages(doc) {
                if let Some(issue) = doc.issues.get(&page) {
                    return Some(issue.message.clone());
                }
                let low = self
                    .cache
                    .attached
                    .get(&page)
                    .into_iter()
                    .flatten()
                    .filter_map(|id| self.cache.entries.get(id))
                    .find_map(|entry| entry.low_res.as_ref());
                if let Some(low) = low {
                    return Some(low.message.to_string());
                }
            }
            None
        });
        self.set_note(if self.work.gave_up { Some(CANT_DRAW.to_string()) } else { note });
    }

    pub(super) fn set_note(&mut self, note: Option<String>) {
        self.note = note;
    }

    pub(super) fn hide_page(&mut self, page: u32) {
        self.host.ops.push_back(Op::HidePage { page });
        self.host.shown.remove(&page);
        self.pointer.selecting.page_hidden(page);
        let Some(ids) = self.cache.attached.remove(&page) else { return };
        for &id in &ids {
            if let Some(entry) = self.cache.entries.get_mut(&id) {
                entry.attached = false;
                self.host.to_discard.push(id);
            }
        }
        self.cache.give_back_list(ids);
    }

    /// Whether any pixel of a page is in view at the tiles' scale. The view can reach into a page by less than the part of a pixel its tiles leave off at its foot or its right edge, as
    /// [`recto_proto::device_size`] rounds those to whole pixels: then no tile is in view, so [`Self::refresh`] takes what was on the page off it, and nothing of it can be seen. A page whose
    /// size or scale isn't known yet counts as showing.
    pub(super) fn shows_a_pixel(&self, doc: &DocView, page: u32) -> bool {
        let scale = doc.tile_scale;
        if scale <= 0.0 || !doc.layout.is_known(page) {
            return true;
        }
        !matches!(self.visible_cells(doc, page, scale), Ok(cells) if cells.is_empty())
    }

    /// Counts a step that left a page in view bare: one that shows a pixel, by the same cells drawing goes by, so a sliver of 0.004 points isn't a blank page.
    pub(super) fn count_blanks(&mut self) {
        let Some(doc) = self.doc() else { return };
        let (mut blank, mut blank_seen) = (false, false);
        for page in self.visible_pages(doc) {
            let has_something = self.cache.attached.get(&page).is_some_and(|ids| !ids.is_empty());
            if !has_something && !doc.issues.contains_key(&page) && self.shows_a_pixel(doc, page) {
                blank = true;
                blank_seen |= doc.seen.contains(&page);
            }
        }
        self.counts.blank_steps += u64::from(blank);
        self.counts.blank_seen_steps += u64::from(blank_seen);
    }
}
