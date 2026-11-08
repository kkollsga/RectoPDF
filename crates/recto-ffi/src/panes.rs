//! **A comparison's panes, as the host draws them.** Owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view, right click menu can enable separate scroll (default) and synced scroll. Zoom is always synced."
//!
//! Each pane is a page view of its own, an [`Engine`] with its own thread, workers and tiles, so a pane is one live document as a tab on screen is, and nothing new had to be taught to the page view about showing two things. Pane 0 is the app's own page view, which holds the warm tabs too; the others exist only while there's a comparison, at most [`MAX_PANES`] in all, and a pane that goes ends its thread, its workers and its tiles (`Engine::shut_down`). The caps on what's kept off screen are shared out among them, so panes keep no more between them than one view does.
//!
//! The session decides what's side by side and what synced scroll does: [`recto_session::compare`]. This keeps what the UI thread needs to act on that with no lock on the session: which tab each pane shows, the comparison as last laid out, and the one zoom. **Synced scroll** follows the reader's own scroll in a pane, never a pane arriving where it was sent, so panes can't send each other back and forth. **Zoom** is one magnification for every pane, taken from whichever pane it changed in once it's settled, a pinch's end or a pause in ⌘-scroll, so nothing's drawn for every step of a gesture in each pane.

use crate::lock;
use recto_reader::engine::EngineEvents;
use recto_reader::layout::{CORNER_MARGIN, MIN_SIDE_MARGIN, PANE_MARGIN};
use recto_reader::tiles::{Viewport, Zoom};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_session::TabId;
use recto_session::compare::{Comparison, MAX_PANES};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What the UI thread acts on for synced scroll and zoom, as the session thread last laid it out.
#[derive(Default)]
struct Plan {
    comparison: Option<Comparison>,
    /// How many pages each pane's tab has, by the comparison's order.
    pages: Vec<u32>,
    /// The one zoom, once panes are side by side.
    magnification: Option<f64>,
}

pub(crate) struct Panes {
    /// Panes 1 and on. Pane 0 is the app's own page view.
    extra: Mutex<Vec<Engine>>,
    /// Which pane is being read, by its place, which the UI thread asks without the session.
    reading: AtomicUsize,
    /// What each pane shows, left to right.
    shown: Mutex<Vec<TabId>>,
    plan: Mutex<Plan>,
    hooks: Arc<dyn EngineEvents>,
    config: EngineConfig,
}

impl Panes {
    pub(crate) fn new(hooks: Arc<dyn EngineEvents>, config: EngineConfig) -> Self {
        Self { extra: Mutex::new(Vec::new()), reading: AtomicUsize::new(0), shown: Mutex::new(Vec::new()), plan: Mutex::new(Plan::default()), hooks, config }
    }

    /// The page view of a pane, if there's such a pane.
    pub(crate) fn engine(&self, primary: &Engine, pane: usize) -> Option<Engine> {
        if pane == 0 { Some(primary.clone()) } else { lock(&self.extra).get(pane - 1).cloned() }
    }

    /// Every page view there is, pane 0 first.
    pub(crate) fn engines(&self, primary: &Engine) -> Vec<Engine> {
        std::iter::once(primary.clone()).chain(lock(&self.extra).iter().cloned()).collect()
    }

    /// The page view showing a tab, or pane 0's, which holds the warm tabs.
    pub(crate) fn engine_for_tab(&self, primary: &Engine, tab: TabId) -> Engine {
        let at = lock(&self.shown).iter().position(|shown| *shown == tab);
        at.and_then(|pane| self.engine(primary, pane)).unwrap_or_else(|| primary.clone())
    }

    /// The pane being read.
    pub(crate) fn reading(&self) -> usize {
        self.reading.load(Ordering::SeqCst)
    }

    /// The page view of the pane being read: where Copy, Select All and the zoom level go.
    pub(crate) fn reading_engine(&self, primary: &Engine) -> Engine {
        self.engine(primary, self.reading()).unwrap_or_else(|| primary.clone())
    }

    pub(crate) fn count(&self) -> usize {
        lock(&self.shown).len().max(1)
    }

    pub(crate) fn tab_of(&self, pane: usize) -> Option<TabId> {
        lock(&self.shown).get(pane).copied()
    }

    /// **As many page views as there are panes,** on the session thread. A new one starts on the paper showing and with the window as it is; one that's not needed ends now, with everything it held. Each shares the caps, and has a pane's margins.
    pub(crate) fn set_count(&self, primary: &Engine, count: usize, paper: PaperKind, window_open: bool) {
        let count = count.clamp(1, MAX_PANES);
        let mut extra = lock(&self.extra);
        while extra.len() + 1 > count {
            if let Some(gone) = extra.pop() {
                gone.shut_down();
            }
        }
        while extra.len() + 1 < count {
            let engine = Engine::new(self.config.clone(), Arc::clone(&self.hooks));
            engine.set_paper(paper);
            if !window_open {
                engine.window_closed();
            }
            extra.push(engine);
        }
        let all: Vec<Engine> = std::iter::once(primary.clone()).chain(extra.iter().cloned()).collect();
        drop(extra);
        for (pane, engine) in all.iter().enumerate() {
            engine.set_share(count);
            let (left, right) = margins(pane, count);
            engine.set_margins(left, right);
        }
    }

    /// What's side by side now, for the UI thread: which tab each pane shows, which is read, and the comparison with each pane's length.
    pub(crate) fn lay_out(&self, shown: Vec<TabId>, reading: usize, comparison: Option<Comparison>, pages: Vec<u32>) {
        let changed = *lock(&self.shown) != shown;
        *lock(&self.shown) = shown;
        self.reading.store(reading, Ordering::SeqCst);
        let mut plan = lock(&self.plan);
        if comparison.is_none() || changed {
            plan.magnification = None;
        }
        plan.comparison = comparison;
        plan.pages = pages;
    }

    /// **One zoom for every pane,** from the pane being read, when what's side by side has changed. Says what it is.
    pub(crate) fn even_out_zoom(&self, primary: &Engine) -> Option<f64> {
        if self.count() < 2 || lock(&self.plan).magnification.is_some() {
            return None;
        }
        let value = self.reading_engine(primary).magnification()?;
        lock(&self.plan).magnification = Some(value);
        for engine in self.engines(primary) {
            if engine.magnification().is_some_and(|now| (now - value).abs() > 1e-9) {
                engine.zoom(Zoom::To(value));
            }
        }
        Some(value)
    }

    /// A zoom from a key, the menu, the field or an agent: in the pane being read, and then the same in each other.
    pub(crate) fn zoom_all(&self, primary: &Engine, zoom: Zoom) -> Option<f64> {
        let reading = self.reading_engine(primary);
        let value = reading.zoom(zoom)?;
        if self.count() > 1 {
            lock(&self.plan).magnification = Some(value);
            // The others never hold the UI thread up: a busy one takes the zoom from its own thread.
            for engine in self.engines(primary) {
                if !engine.same_as(&reading) {
                    engine.zoom_soon(value);
                }
            }
        }
        Some(value)
    }

    /// **A pane's view moved,** on the UI thread: a short lock on that pane, and for the reader's own scroll in a comparison, a short lock on each other pane to send it along. Says whether the pane being read changed, for the session to be told.
    pub(crate) fn viewport(&self, primary: &Engine, pane: usize, by_reader: bool, viewport: Viewport) -> (Vec<recto_reader::Op>, Option<TabId>) {
        let Some(engine) = self.engine(primary, pane) else { return (Vec::new(), None) };
        let outcome = engine.set_viewport_scrolled(viewport);
        let (ops, scrolled) = (outcome.ops, outcome.scrolled);
        if self.count() < 2 {
            return (ops, None);
        }
        // Zoom: one for all, once a gesture has settled and this pane's taken it.
        if !viewport.live && outcome.magnification.is_some_and(|now| now == viewport.magnification) {
            let mut plan = lock(&self.plan);
            if plan.magnification.is_some_and(|shared| (shared - viewport.magnification).abs() > 1e-9) {
                plan.magnification = Some(viewport.magnification);
                drop(plan);
                for (other, engine) in self.engines(primary).into_iter().enumerate() {
                    if other != pane {
                        engine.zoom_soon(viewport.magnification);
                    }
                }
            }
        }
        // Both have to say so: the host that the reader's hand moved it, and the page view that it was a scroll and not the host arriving somewhere.
        if !(scrolled && by_reader) {
            return (ops, None);
        }
        let focused = (self.reading.swap(pane, Ordering::SeqCst) != pane).then(|| self.tab_of(pane)).flatten();
        if let Some(anchor) = outcome.anchor {
            self.follow(primary, pane, anchor);
        }
        (ops, focused)
    }

    /// The reader's moved into a pane by the pointer: it's the one read. Says its tab if that's a change.
    pub(crate) fn focus(&self, pane: usize) -> Option<TabId> {
        if pane >= self.count() || self.reading.swap(pane, Ordering::SeqCst) == pane {
            return None;
        }
        self.tab_of(pane)
    }

    /// Synced scroll: each other pane to the page the rule says, at the same depth in its page, or at its own end when it's run out.
    fn follow(&self, primary: &Engine, pane: usize, (page, within): (u32, f64)) {
        let shown = lock(&self.shown).clone();
        let Some(leader) = shown.get(pane).copied() else { return };
        let plan = lock(&self.plan);
        let Some(comparison) = plan.comparison.as_ref().filter(|comparison| comparison.synced) else { return };
        let pages = |tab: TabId| comparison.tabs.iter().position(|one| *one == tab).and_then(|at| plan.pages.get(at).copied()).unwrap_or(1);
        let targets = comparison.sync_targets(leader, page, pages);
        let from = comparison.tabs.iter().position(|tab| *tab == leader).and_then(|at| comparison.sync_from.get(at).copied()).unwrap_or(0);
        let wanted: Vec<(TabId, u32, f64)> = targets
            .into_iter()
            .map(|(tab, target)| {
                let start = comparison.tabs.iter().position(|one| *one == tab).and_then(|at| comparison.sync_from.get(at).copied()).unwrap_or(0);
                let meant = i64::from(start) + i64::from(page) - i64::from(from);
                let depth = if meant > i64::from(target) { 1.0 } else if meant < i64::from(target) { 0.0 } else { within };
                (tab, target, depth)
            })
            .collect();
        drop(plan);
        for (tab, target, depth) in wanted {
            if let Some(other) = shown.iter().position(|shown| *shown == tab).and_then(|at| self.engine(primary, at)) {
                other.follow_soon(target, depth);
            }
        }
    }
}

/// Room either side of a pane's pages: a window's alone, a pane's in a comparison, and the corner's for the pane at the right.
pub(crate) fn margins(pane: usize, count: usize) -> (f64, f64) {
    if count <= 1 {
        (MIN_SIDE_MARGIN, CORNER_MARGIN)
    } else if pane + 1 == count {
        (PANE_MARGIN, CORNER_MARGIN)
    } else {
        (PANE_MARGIN, PANE_MARGIN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a pane keeps the window's margins, so three panes leave pages a thumbnail wide, or the pane at the right loses the corner the agent icon and the zoom level sit in, and they're over its page.
    #[test]
    fn only_the_right_pane_keeps_the_corner() {
        assert_eq!(margins(0, 1), (MIN_SIDE_MARGIN, CORNER_MARGIN));
        assert_eq!(margins(0, 3), (PANE_MARGIN, PANE_MARGIN));
        assert_eq!(margins(1, 3), (PANE_MARGIN, PANE_MARGIN));
        assert_eq!(margins(2, 3), (PANE_MARGIN, CORNER_MARGIN));
        assert_eq!(margins(1, 2), (PANE_MARGIN, CORNER_MARGIN));
    }
}
