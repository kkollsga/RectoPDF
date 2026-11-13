//! **A tab's wake and its sleep file, and the window going up, from the engine's callers:** what a sleep file gives a tab's first frame and gets from what's held, when a wake began, and whether a window can go up with what's on the layers. The sleeper, the launch and the session's tab changes take it alone.
//!
//! A borrow of the [`Engine`], built inline by [`Engine::waking`]: each method takes the page view's lock exactly as the `Engine` method it was until R3.6
//! did, in the same order with the same locks after it, so the engine's lock map holds as it was.

use super::{Engine, lock};
use crate::tiles::PageMap;
use std::time::Instant;

/// A tab's wake and sleep file, and the first frame, on this page view.
#[derive(Clone, Copy)]
pub struct Wake<'a> {
    engine: &'a Engine,
}

// size: one reference, so taking it costs what calling a method of `Engine` did.
const _: () = assert!(size_of::<Wake<'static>>() <= size_of::<usize>(), "a facade of the engine grew");

impl Engine {
    /// A tab's wake and sleep file, and the first frame, on this page view.
    #[inline]
    pub fn waking(&self) -> Wake<'_> {
        Wake { engine: self }
    }
}

impl Wake<'_> {
    /// What a tab's first frame shows, for its next [`Engine::activate`].
    pub fn give_sleep(&self, tab: u32, view: crate::tiles::SleepView) {
        lock(&self.engine.shared.reader).wake.give_sleep(tab, view);
    }

    /// The window's gone up.
    pub fn window_up(&self) {
        lock(&self.engine.shared.reader).wake.window_up();
    }

    /// A tab's being woken, as of now.
    pub fn note_wake(&self, tab: u32, began: Instant) {
        let mut reader = lock(&self.engine.shared.reader);
        let hot = reader.docs.hot_tab();
        reader.wake.note_wake(hot, tab, began);
    }

    /// Whether a view of this tab's held.
    pub fn holds(&self, tab: u32) -> bool {
        lock(&self.engine.shared.reader).docs.holds(tab)
    }

    /// What a tab's sleep file gets from here.
    pub fn sleep_material(&self, tab: u32, now: &PageMap) -> Option<crate::tiles::SleepMaterial> {
        lock(&self.engine.shared.reader).sleep_material(tab, now)
    }

    /// Whether a window can go up with what's on the layers.
    pub fn is_showable(&self) -> bool {
        lock(&self.engine.shared.reader).is_showable()
    }

    /// Whether the place is on the layers, and nothing's on its way. A short lock, asked only until the window is showing.
    pub fn is_on_place(&self) -> bool {
        lock(&self.engine.shared.reader).is_on_place()
    }
}
