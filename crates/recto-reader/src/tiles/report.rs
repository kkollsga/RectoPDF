//! **What QA reads:** numbers and states of the page view, and the counts the budgets go by. No text from a document.

use super::{Reader, ReaderReport};
use crate::paper::PaperKind;
use recto_platform::surface::PixelSurface;
use serde_json::json;
use std::collections::HashSet;

impl Reader {
    /// What QA reads, and the budgets. Numbers and states: no text from a document.
    pub fn report(&self) -> ReaderReport {
        let doc = self.docs.shown();
        let attached = self.cache.attached.values().map(Vec::len).sum::<usize>();
        let attached_bytes: usize = self
            .cache
            .attached
            .values()
            .flatten()
            .filter_map(|id| self.cache.entries.get(id))
            .filter_map(|entry| entry.surface.as_ref())
            .map(PixelSurface::len)
            .sum();
        let previews_attached = self
            .cache
            .attached
            .values()
            .flatten()
            .filter_map(|id| self.cache.entries.get(id))
            .filter(|entry| entry.preview)
            .count();
        let low_res_pages: HashSet<u32> = self
            .cache
            .attached
            .iter()
            .filter(|(_, ids)| ids.iter().filter_map(|id| self.cache.entries.get(id)).any(|entry| entry.low_res.is_some()))
            .map(|(page, _)| *page)
            .collect();
        let mut low_res_pages: Vec<u32> = low_res_pages.into_iter().map(|page| page + 1).collect();
        low_res_pages.sort_unstable();
        let visible: Vec<u32> = doc.map(|doc| self.visible_pages(doc).map(|page| page + 1).collect()).unwrap_or_default();
        ReaderReport(json!({
            "tab": self.docs.hot,
            "pages": doc.map(|doc| doc.layout.count()),
            "visible_pages": visible,
            "page": self.place.last_view_page.map(|page| page + 1),
            "magnification": self.viewport.magnification,
            "fit": doc.map(|doc| doc.layout.fit()),
            "spread": doc.map(|doc| format!("{:?}", doc.layout.spread())),
            "tile_scale": doc.map(|doc| doc.drawing.tile_scale),
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
                "kept": self.cache.entries.len() - attached.min(self.cache.entries.len()),
                "kept_bytes": self.cache.entries.values().filter(|entry| !entry.attached).filter_map(|entry| entry.surface.as_ref()).map(PixelSurface::len).sum::<usize>(),
                "page_layers": self.host.shown.len(),
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
            "documents_held": self.docs.views.len(),
            "left_picture_bytes": self.docs.views.values().filter_map(|doc| doc.drawing.left_pictures.as_ref()).flatten().map(|picture| picture.bgra.len()).sum::<usize>(),
            "last_wake": self.wake.last_wake().map(|wake| json!({ "tab": wake.tab, "from_sleep": wake.from_sleep.is_some(), "first_frame_us": wake.first_frame_us, "sharp_us": wake.sharp_us, "blank_frames": wake.blank_frames, "had": wake.had.word(), "sleep_pictures": wake.sleep_pictures, "sleep_in_view": wake.sleep_in_view, "bare_with_picture": wake.bare_with_picture })),
            "sleep_draws": super::sleep_draw::SleepDraws::report(self),
            "window_open": self.window_open,
            "selection": self.pointer.selecting.report(),
        }))
    }

    /// The counts, for a test: the engine and QA read them in [`Self::report`]. Found unused outside the tests once `Reader` was the crate's own (R3.6).
    #[cfg(test)]
    pub fn counts(&self) -> &super::ReaderCounts {
        &self.counts
    }
}
