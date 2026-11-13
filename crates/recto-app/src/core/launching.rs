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
    pub fn begin_restore(self: &Arc<Self>) {
        let inner = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("recto-restore".into())
            .stack_size(1024 * 1024)
            .spawn(move || read_state(&inner));
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

    /// The page checks as they are now, gathered for [`showing::decide`]. The UI thread asks, and doesn't wait for the session.
    fn page_checks(&self) -> showing::PageChecks {
        let active = self.host.session.try_with_session(|session| session.tabs().active().is_some());
        showing::PageChecks {
            state_read: self.state_read(),
            holding_tab: lock(&self.life.holding_tab).is_some(),
            session_free: active.is_some(),
            has_tab: active.unwrap_or(false),
            on_place: self.engine.waking().is_on_place(),
            showable: self.engine.waking().is_showable(),
        }
    }

    /// Whether the window may be shown now, by [`showing::decide`]; what it said is kept for the record.
    pub fn ready_to_show(&self) -> bool {
        let opened = lock(&self.life.window_opened_at).map(|at| at.elapsed());
        let decision = showing::decide(opened, self.life.started.elapsed(), self.page_checks());
        self.life.shown_by_cap.store(decision == showing::Show::Cap, Ordering::Relaxed);
        decision != showing::Show::Wait
    }

    /// **What the window's first showing had on it, and why it was shown.** `why` is `ready` (the page checks passed), `cap` (Rust's cap ran out: the host asked and was told yes) or `patience` (the host's timer showed it). The rest is how the page checks were at the show, written once per window.
    pub fn record_window_shown(&self, home: bool, tile_layers: u32, why: &str) {
        let report = self.engine.report().0;
        let checks = self.page_checks();
        let why = if why == "ready" && self.life.shown_by_cap.load(Ordering::Relaxed) {
            "cap"
        } else {
            why
        };
        let opened = lock(&self.life.window_opened_at).map(|at| at.elapsed());
        *lock(&self.life.first_shown) = Some(json!({
            "home": home, "page": report["page"], "scroll_y": report["scroll_y"], "scroll_x": report["scroll_x"], "magnification": report["magnification"], "settled": report["settled"], "tab": report["tab"],
            "tile_layers": tile_layers, "ms_from_start": self.life.started.elapsed().as_secs_f64() * 1000.0,
            "why": why,
            "ms_since_window_opened": opened.map(|opened| opened.as_secs_f64() * 1000.0),
            "patience_ms": SHOW_PATIENCE.as_millis() as u64,
            "when_shown": { "state_read": checks.state_read, "holding_tab": checks.holding_tab, "session_free": checks.session_free, "has_tab": checks.has_tab, "on_place": checks.on_place, "showable": checks.showable },
        }));
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
        *lock(&self.life.window_opened_at) = Some(Instant::now());
        let engines = self.panes.engines(&self.engine);
        for engine in &engines {
            engine.window_opened();
        }
        let held = std::mem::take(&mut lock(&self.window.app).selections_held);
        for (pane, range) in held {
            if let Some(engine) = engines.get(pane) {
                engine.selection().restore((range.start.page, range.start.at), (range.end.page, range.end.at));
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
        // What the going window said about itself since the state came in isn't kept: the new one comes from the state. What the host chose about what's out of sight is.
        let mut app = lock(&self.window.app);
        if let Some(state) = app.from_state.take() {
            let minimised = app.window.as_ref().is_some_and(|window| window.minimised);
            let (hidden, closed) = (app.app_hidden, app.window_closed);
            let decided = (app.hidden_decided, app.minimised_decided, app.window_decided);
            *app = *state;
            app.app_hidden = hidden;
            app.window_closed = closed;
            (app.hidden_decided, app.minimised_decided, app.window_decided) = decided;
            if let Some(window) = app.window.as_mut() {
                window.minimised = minimised;
            }
        }
        app.selections_held = held;
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
            Some(crate::lifecycle::Out::HandOffQuit) => self.listener.hand_off_quit(),
            Some(crate::lifecycle::Out::SessionEnded) => self.listener.session_ended(),
            // Carried out on the session thread, which made the step: see `keeping`.
            Some(crate::lifecycle::Out::Keeping(_)) => {}
        }
    }
}

/// **The state read and handed to the session thread,** on a thread that can wait: at the launch ([`Inner::begin_restore`]), and once a late try has got the
/// record's lock ([`super::keeping`]), the one way the state is read for a launch. **The lock is asked for once:** one wait of the store's patience, whose
/// answer the updater is told ([`updates::hold_or_tell`]) and the read goes by, so a lock that comes after the wait can't be got by the read with the updater
/// still held back (the review's blocker of 2026-11-13); it's the late try's to take. The updater's told of a copy that keeps the record before anything's read,
/// so a version that keeps failing goes back here and not by reading on.
pub(super) fn read_state(inner: &Arc<Inner>) {
    let loaded = read(inner);
    inner.core.push(CoreCall::Restored(Box::new(loaded)));
}

/// The launch's state, read as [`read_state`] says. A late try's read is its own ([`super::keeping`]): it tells the updater after it reads, once it has claimed
/// the record, so a read that runs past its patience is a second copy's.
pub(super) fn read(inner: &Arc<Inner>) -> Loaded {
    let misreads = updates::misreads_the_record(lock(&inner.life.update_facts).facts.as_ref());
    let keeps = inner.saver.store.wait_for_lock();
    if updates::hold_or_tell(inner, keeps) == recto_update::updater::AtLaunch::Exit {
        std::process::exit(0);
    }
    recto_platform::slow::pause(recto_platform::slow::Seam::State);
    let loaded = if !keeps {
        Loaded::OtherInstance
    } else if misreads {
        Loaded::Nothing
    } else {
        inner.saver.store.load()
    };
    let loaded = updates::fall_back_if_unreadable(&inner.updates, &inner.saver.store, loaded);
    updates::note_tabs_read(&loaded);
    loaded
}
