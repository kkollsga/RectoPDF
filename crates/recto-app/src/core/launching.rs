//! **The launch, as the host steps through it:** the state read on a thread of its own, the patience for it, and the window being up.

use super::*;

impl Inner {
    /// The window is up: files named at launch start opening. The host's own part (a quiet run's line, the print sweep) is the host's.
    pub fn ready(&self, open: &[PathBuf]) {
        recto_platform::slow::window_up();
        for path in open {
            super::opening::open(&self.core, path.clone());
        }
        self.core.push(CoreCall::Reconcile);
    }

    /// Reads the state, on a thread of its own, and hears `restore_ready` when it's applied. It's what the window waits for, up to `restore_patience_seconds`: its frame and the place on it come from what's read.
    /// The record's keeper reads it ([`keeper::Keeper::begin_restore`]); the bridge's entry point.
    pub fn begin_restore(self: &Arc<Self>) {
        self.on_disk.keeper().begin_restore();
    }

    /// The state hasn't been read in time, or a restored tab's file isn't answering. What's on the screen goes on without it.
    pub fn restore_patience_over(&self) {
        self.core.push(CoreCall::RestorePatienceOver);
    }

    /// The window opened: its page views have somewhere to draw again, what was selected on the window it replaced comes back, and the contents go back to where they were.
    pub fn window_opened(&self) {
        self.life.window_opened();
        let engines = self.panes.engines(&self.engine);
        for engine in &engines {
            engine.window_opened();
        }
        for (pane, range) in self.window.take_selections_held() {
            if let Some(engine) = engines.get(pane) {
                engine.selection().restore((range.start.page, range.start.at), (range.end.page, range.end.at));
            }
        }
        self.window.opened();
        self.saver.view_changed();
    }

    /// The window's going, for another made at once in its place, from a state that came in late: what it showed goes, as for a closed window, and nothing's recorded as closed.
    pub fn window_replaced(&self) {
        // The host's word that it made the window again: what `record.remade` says.
        let _ = self.life.step(crate::lifecycle::Event::WindowRemade);
        let engines = self.panes.engines(&self.engine);
        // What's selected goes with the window's layers, and comes back on the new one (a selection and Find's hit, under a late state).
        let held: Vec<(usize, recto_index::select::Range)> = engines
            .iter()
            .enumerate()
            .filter_map(|(pane, engine)| Some((pane, engine.selection().hot()?.1?)))
            .collect();
        for engine in &engines {
            engine.window_closed();
        }
        self.window.replaced(held);
    }

    /// The window closed, by the person or QA: what it showed goes, and the record says it's closed.
    pub fn window_closed(&self) {
        for engine in self.panes.engines(&self.engine) {
            engine.window_closed();
        }
        self.window.closed();
        self.saver.view_changed();
    }

    /// What the host shows has been made from the state that was read.
    pub fn shown_with_state(&self) {
        self.carry_out(self.life.step(crate::lifecycle::Event::ShownWithState));
    }

    /// **What the app's life came to, carried out** once its lock is let go of ([`crate::lifecycle::Out`]).
    pub fn carry_out(&self, out: Option<crate::lifecycle::Out>) {
        match out {
            None => {}
            // A test's slow window: the window that went up without the state is made again from it this much later, so what's done in between can be seen to wait for it.
            Some(crate::lifecycle::Out::RestoreReady { late: true, .. }) => {
                let listener = Arc::clone(&self.listener);
                recto_platform::slow::later(recto_platform::slow::Seam::Remake, move || listener.restore_ready());
            }
            Some(crate::lifecycle::Out::RestoreReady { .. }) => self.listener.restore_ready(),
            Some(crate::lifecycle::Out::Freeze(frozen)) => self.set_frozen(frozen),
            // Begun only from the host's call, which has the core's own handle: see `carry_out_quit`.
            Some(crate::lifecycle::Out::QuitBegins) => {}
            Some(out @ (crate::lifecycle::Out::QuitAnswer { .. } | crate::lifecycle::Out::QuitStayed)) => self.carry_out_quit_answer(out),
            Some(crate::lifecycle::Out::HandOffQuit) => self.listener.hand_off_quit(),
            Some(crate::lifecycle::Out::SessionEnded) => self.listener.session_ended(),
            // Carried out on the session thread, which made the step: see `keeping`.
            Some(crate::lifecycle::Out::Keeping(_)) => {}
        }
    }
}
