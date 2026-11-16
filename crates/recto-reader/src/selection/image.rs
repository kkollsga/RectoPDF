//! **What an image picked with a click reads of the text selection** ([`crate::image_pick`]), apart from the selection's own methods: a click's serial
//! from its clock, and a page's text as held, with whether its hidden-text check is done with.

use super::Selecting;
use recto_index::select::PageText;

impl Selecting {
    /// **A click's serial:** the next tick of the clock the pages' texts are kept by, which only goes forward.
    pub(crate) fn tick(&mut self) -> u32 {
        self.clock += 1;
        self.clock as u32
    }

    /// A page's text as held, and whether nothing more will be known of its hidden text (its check tried however it went, or the page unreadable).
    pub(crate) fn held_text(&self, page: u32) -> Option<(&PageText, bool)> {
        self.held(page).map(|held| (&*held.text, held.check_tried || held.failed))
    }
}
