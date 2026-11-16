//! **An image picked with a click in read mode, from the engine's callers** ([`crate::image_pick`]): the click the app was told to look for, what the app
//! found for it, the image picked for a Copy, and letting it go. The app (a click's look, its answer, Copy, Escape, the snapshot tool turned on) takes it
//! alone. The app hears that there's a click to look for, or that one it's looking for has gone, by [`super::EngineEvents::image_wanted`].
//!
//! Methods of the selection's facade ([`Selection`]), as an image picked is the other kind of selection; each takes the page view's lock once.

use super::lock;
use super::selection::Selection;
use crate::image_pick::ImagePick;
use crate::tiles::image_pick as tiles;
use recto_index::select::{PagePoint, Rect};

impl Selection<'_> {
    /// The click the app was told to look for, with the tab it's on and its serial. A short lock.
    pub fn image_looking(&self) -> Option<(u32, PagePoint, u32)> {
        tiles::looking(&lock(&self.engine.shared.reader))
    }

    /// Whether a click on the tab showing is waiting for its image, at whatever stage: its page's text, the worker's boxes, or the app's look.
    pub fn image_finding(&self) -> bool {
        tiles::finding(&lock(&self.engine.shared.reader))
    }

    /// **The image found for the `serial`th click on `tab`:** held, and its box drawn, if that click is still the one looked for (the last thing the
    /// person asked for wins). Says whether it's held. Off the UI thread, as the look's answer lands.
    pub fn image_found(&self, tab: u32, serial: u32, number: u32, rect: Rect) -> bool {
        let mut reader = lock(&self.engine.shared.reader);
        let held = tiles::found(&mut reader, tab, serial, number, rect);
        self.engine.after(reader);
        held
    }

    /// No image for the `serial`th click on `tab`, or none could be looked for: it's no longer waiting.
    pub fn image_not_found(&self, tab: u32, serial: u32) {
        tiles::not_found(&mut lock(&self.engine.shared.reader), tab, serial);
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
