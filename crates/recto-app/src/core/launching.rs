//! **The launch, as the host steps through it:** the state read on a thread of its own, the patience for it, and the window being up.

use super::*;

impl Inner {
    /// The window is up: files named at launch start opening. The host's own part (a quiet run's line, the print sweep) is the host's.
    pub fn ready(&self, open: &[PathBuf]) {
        for path in open {
            self.core.push(CoreCall::Open(path.clone()));
        }
        self.core.push(CoreCall::Reconcile);
    }

    /// Reads the state, on a thread of its own, and hears `restore_ready` when it's applied. It's what the window waits for, up to `restore_patience_seconds`: its frame and the place on it come from what's read.
    pub fn begin_restore(self: &Arc<Self>) {
        let inner = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("recto-restore".into()).stack_size(1024 * 1024).spawn(move || {
            // Before the record: a version that keeps failing goes back here, and not by reading on.
            let facts = lock(&inner.life.update_facts).take();
            let misreads = updates::misreads_the_record(facts.as_ref());
            if let Some(facts) = facts
                && inner.updates.launched(facts) == recto_update::updater::AtLaunch::Exit
            {
                std::process::exit(0);
            }
            recto_platform::slow::pause(recto_platform::slow::Seam::State);
            let loaded = if misreads { Loaded::Nothing } else { inner.saver.store.load() };
            let loaded = updates::fall_back_if_unreadable(&inner, loaded);
            updates::note_tabs_read(&loaded);
            inner.core.push(CoreCall::Restored(Box::new(loaded)));
        });
        if spawned.is_err() {
            self.core.push(CoreCall::Restored(Box::new(Loaded::Nothing)));
        }
    }

    /// The state hasn't been read in time, or a restored tab's file isn't answering. What's on the screen goes on without it.
    pub fn restore_patience_over(&self) {
        self.core.push(CoreCall::RestorePatienceOver);
    }

    /// Whether the state's been read and taken in. A window that went up without it, past the patience, is made again from it once it has: see the host's `restoreReady`.
    pub fn state_read(&self) -> bool {
        self.life.state_read()
    }

    /// Whether this launch opens a window. Any launch does but an update's arrival, which leaves things as they were: a window closed stays closed, rather than appearing behind whatever the user's doing.
    pub fn window_should_open(&self) -> bool {
        let reopened = self.updates.reopened_by_update();
        let mut app = lock(&self.window.app);
        app.window_decided = true;
        !(app.window_closed && reopened)
    }

    /// The window opened: its page views have somewhere to draw again, what was selected on the window it replaced comes back, and the contents go back to where they were.
    pub fn window_opened(&self) {
        let engines = self.panes.engines(&self.engine);
        for engine in &engines {
            engine.window_opened();
        }
        let held = std::mem::take(&mut lock(&self.window.app).selections_held);
        for (pane, range) in held {
            if let Some(engine) = engines.get(pane) {
                engine.restore_selection((range.start.page, range.start.at), (range.end.page, range.end.at));
            }
        }
        // A new window's contents start at the top: they go back to where they were, following or not, as at a launch. A window made again from a state read late is one (CI, 2026-11-11).
        let contents_y = lock(&self.window.app).contents_y;
        if let Some(y) = contents_y {
            let mut follow = lock(&self.window.contents_follow);
            let following = follow.is_following();
            follow.new_contents();
            follow.resume_at(y, following);
        }
        lock(&self.window.app).window_closed = false;
        self.saver.view_changed();
    }

    /// The window closed, by the person or QA: what it showed goes, and the record says it's closed.
    pub fn window_closed(&self) {
        for engine in self.panes.engines(&self.engine) {
            engine.window_closed();
        }
        let mut app = lock(&self.window.app);
        app.window_closed = true;
        // A window opened again isn't in the Dock.
        if let Some(window) = app.window.as_mut() {
            window.minimised = false;
        }
        drop(app);
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
        }
    }
}
