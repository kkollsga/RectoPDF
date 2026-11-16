//! **An image picked with a click in read mode, from the engine's callers** ([`crate::image_pick`]): the click waiting for its image, what the app found
//! for it, the image picked for a Copy, and letting it go. The app (a click's look, its answer, Copy, Escape, the snapshot tool turned on) takes it alone.
//!
//! Methods of the selection's facade ([`Selection`]), as an image picked is the other kind of selection; each takes the page view's lock once.

use super::lock;
use super::selection::Selection;
use crate::image_pick::ImagePick;
use crate::tiles::image_pick as tiles;
use recto_index::select::{PagePoint, Rect};

impl Selection<'_> {
    /// The click waiting for its image, with the tab it's on. A short lock.
    pub fn image_wanted(&self) -> Option<(u32, PagePoint)> {
        tiles::wanted(&lock(&self.engine.shared.reader))
    }

    /// **The image found for the click at `point` on `tab`:** held, and its box drawn once it's known not to be under text that shows, if that click is
    /// still the one waiting (the last thing the person asked for wins). Says whether it's held. Off the UI thread, as the look's answer lands.
    pub fn image_found(&self, tab: u32, point: PagePoint, number: u32, rect: Rect) -> bool {
        let mut reader = lock(&self.engine.shared.reader);
        let held = tiles::found(&mut reader, tab, point, number, rect);
        self.engine.after(reader);
        held
    }

    /// No image for the click at `point` on `tab`, or none could be looked for: it's no longer waiting.
    pub fn image_not_found(&self, tab: u32, point: PagePoint) {
        tiles::not_found(&mut lock(&self.engine.shared.reader), tab, point);
    }

    /// The image picked on the tab showing, with that tab. A short lock, for Copy and its menu item.
    pub fn image_picked(&self) -> Option<(u32, ImagePick)> {
        tiles::picked(&lock(&self.engine.shared.reader))
    }

    /// The image picked, and any click waiting, let go, its box taken off. Says whether there was either.
    pub fn let_go_image(&self) -> bool {
        let mut reader = lock(&self.engine.shared.reader);
        let was = tiles::let_go(&mut reader);
        self.engine.after(reader);
        was
    }
}
