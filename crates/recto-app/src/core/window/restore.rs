//! **A saved session's window, put back** as the state's taken in (`session::apply_restored`): what the record says of the window, with what the host has
//! decided since the launch kept (hidden, the Dock, closed), a window already taking input keeping what it shows, and a late keep's window kept as it is until
//! it's made again. Moved from `apply_restored` (R3.7, S2b), its body unchanged but for the receiver and what it's passed.

use super::{AppState, Window};
use crate::core::updates;
use crate::lock;
use recto_session::TabId;
use recto_session::record::{self, AppRecord, Appearance, SessionRecord};
use std::sync::atomic::Ordering;

/// **What a restore puts back of the window,** borrowed from what was read: the window's part of the record, the tab that was active with what was selected
/// on it, and for a late keep's restore the window as it was before the restore began with the appearance it had as the try was kept.
pub struct Restoring<'a> {
    saved: &'a AppRecord,
    active: Option<TabId>,
    selection: Option<record::Selection>,
    real: Option<&'a AppState>,
    appearance_then: Option<Appearance>,
}

impl<'a> Restoring<'a> {
    /// From the saved session; with `real`, the window as [`Window::as_it_is`] had it before the restore began, for a late keep's restore, whose try was kept
    /// with `appearance_then`.
    pub(in crate::core) fn new(record: &'a SessionRecord, real: Option<&'a AppState>, appearance_then: Option<Appearance>) -> Self {
        let active = record.active.map(TabId);
        let selection = active.and_then(|id| record.tabs.iter().find(|tab| tab.id == id.0)?.selection);
        Self {
            saved: &record.app,
            active,
            selection,
            real,
            appearance_then,
        }
    }
}

impl Window {
    /// **The window as it is now,** for a late keep's restore to keep until the window's made again from what was saved ([`Restoring::new`]).
    pub(in crate::core) fn as_it_is(&self) -> AppState {
        lock(&self.app).clone()
    }

    /// **Puts the window back as the saved session had it.** `interactive` says whether the window takes input now: asked under the window's lock (the life's
    /// lock after the window's), and only for a window already decided and not a late keep's.
    pub(in crate::core) fn restore(&self, restoring: &Restoring<'_>, interactive: impl FnOnce() -> bool) {
        let saved = restoring.saved;
        if let Some(paper_set) = saved.sidebar_width {
            lock(&self.app).sidebar_width = record::sidebar_width(paper_set);
        }
        self.panel_open.store(saved.panel_open, Ordering::SeqCst);
        let mut app = lock(&self.app);
        app.settings_window = saved.settings_window;
        app.window_closed = updates::out_of_sight_when_restored(saved.window_closed, app.window_decided.then_some(app.window_closed));
        // Once the host's shown or hidden things without the state, what it did is how things are, and the record says that, not what was saved.
        app.app_hidden = updates::out_of_sight_when_restored(saved.app_hidden, app.hidden_decided.then_some(app.app_hidden));
        let minimised_now = app.minimised_decided.then(|| app.window.as_ref().is_some_and(|window| window.minimised));
        app.window = saved.window.clone();
        if let Some(window) = app.window.as_mut() {
            window.minimised = updates::out_of_sight_when_restored(window.minimised, minimised_now);
        }
        // A window that's up and taking input isn't made again from a state that comes now, so what it shows stays, its appearance with it: the host
        // applies the state's appearance as it hears it (`restoredLate`), and the record says what the window shows. A late keep's window goes by
        // `as_it_was`.
        let shown_as_it_is = app.window_decided && restoring.real.is_none() && interactive();
        if !shown_as_it_is {
            app.appearance = saved.appearance;
        }
        app.focus = saved.focus;
        app.contents_resume = restoring.active.zip(saved.contents_scroll);
        app.selection_resume = restoring.active.zip(restoring.selection);
        app.contents_y = saved.contents_scroll.map(|scroll| scroll.y);
        // Taken in with a window already up without it: the window made again from it goes by what it said.
        if app.window_decided {
            let state = AppState {
                from_state: None,
                ..app.clone()
            };
            app.from_state = Some(Box::new(state));
        }
        if let Some(real) = restoring.real {
            as_it_was(&mut app, real, restoring.appearance_then);
        }
    }
}

/// **The window as it is, kept until it's made again** from a late keep's restore: its frame, focus, sidebar width and Settings, which the restore put in
/// `from_state` for the window made again ([`crate::core::Inner::window_replaced`]), so if the person touches it first and it isn't made again, the record
/// says what the real window is. And the appearance, if the person chose one while the saved session was read (it isn't `appearance_then`, what it was as the
/// try was kept): what they chose last wins over what was saved, in the record and in the window made again.
fn as_it_was(app: &mut AppState, real: &AppState, appearance_then: Option<Appearance>) {
    if appearance_then.is_some_and(|then| then != real.appearance) {
        app.appearance = real.appearance;
        if let Some(state) = app.from_state.as_mut() {
            state.appearance = real.appearance;
        }
    }
    app.window.clone_from(&real.window);
    app.focus = real.focus;
    app.sidebar_width = real.sidebar_width;
    app.settings_window = real.settings_window;
}
