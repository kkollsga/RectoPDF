//! **The window's rules, each held to what the bridge did before it moved here** (R3.7, S2): every table's expected answers are the bridge's code's, read
//! off `recto-ffi/src/lib.rs` at `main` on 2026-11-13 before its body went. What's kept, what the host is answered, and what the record's writer and the
//! session thread are asked for.

use super::*;
use crate::queue::CoreQueue;
use crate::testkit::FakeClock;
use recto_control::StatePaths;
use recto_control::record_store::{Flavor, RecordStore};
use std::sync::Arc;
use std::time::Duration;

/// A window at launch, with a record's writer and a session thread's queue to see what it asks of them.
struct Rig {
    window: Window,
    saver: Saver,
    core: Arc<CoreQueue>,
    _state: recto_testpdf::TempDir,
}

/// What the record's writer was asked for: to write at once, in its own time, or nothing.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Kept {
    AtOnce,
    InItsTime,
    Not,
}

impl Rig {
    fn new() -> Self {
        let state = recto_testpdf::TempDir::new("win-");
        let core = Arc::new(CoreQueue::default());
        let saver = Saver::new(
            RecordStore::open(&StatePaths::overridden(&state).expect("a state folder"), Flavor::App),
            Arc::clone(&core),
            Arc::new(FakeClock::at(1_000)),
        );
        saver.start();
        let rig = Self {
            window: Window::new(crate::watchdog::Watchdog::new()),
            saver,
            core,
            _state: state,
        };
        // What starting the writer asked of the session thread isn't the window's.
        let _ = rig.queued();
        rig
    }

    /// What the writer was asked for since the window was made: a look once a rig.
    fn kept(&self) -> Kept {
        let kept = match self.saver.wait_for_due() {
            Some(due) if due.is_zero() => Kept::AtOnce,
            Some(_) => Kept::InItsTime,
            None => Kept::Not,
        };
        kept
    }

    /// The calls the session thread was sent since the last look, by name.
    fn queued(&self) -> Vec<String> {
        let mut calls = Vec::new();
        while let Some(call) = self.core.next_or_due(|| Some(Duration::ZERO)) {
            calls.push(format!("{call:?}").split(['(', ' ', '{']).next().unwrap_or_default().to_string());
        }
        calls
    }

    fn app(&self) -> AppState {
        lock(&self.window.app).clone()
    }

    /// A window the host has told of, at a frame of its own.
    fn with_a_window(self) -> Self {
        lock(&self.window.app).window = Some(WindowRecord {
            frame: record::Rect {
                x: 1.0,
                y: 2.0,
                width: 900.0,
                height: 700.0,
            },
            screen: None,
            full_screen: false,
            minimised: false,
        });
        self
    }
}

/// Failure: the window in the Dock isn't kept, or is kept with no window told of (a record with a window nobody made), or isn't written: the bridge's
/// `window_minimised_changed`, which set it on a window there was and asked for a write in its own time either way.
#[test]
fn the_dock_is_kept_as_the_bridge_kept_it() {
    for (window, minimised, kept_minimised) in [(true, true, Some(true)), (true, false, Some(false)), (false, true, None), (false, false, None)] {
        let rig = if window { Rig::new().with_a_window() } else { Rig::new() };
        rig.window.minimised_changed(minimised, &rig.saver);
        assert_eq!(
            rig.app().window.map(|window| window.minimised),
            kept_minimised,
            "window {window}, minimised {minimised}"
        );
        assert_eq!(rig.kept(), Kept::InItsTime, "window {window}, minimised {minimised}");
    }
}

/// Failure: going out of sight with ⌘H waits for the writer's time, so a quit from the Dock's menu loses it, or coming back writes at once for nothing: the
/// bridge's `app_hidden_changed`, at once when hidden and in its own time when shown.
#[test]
fn hidden_is_kept_as_the_bridge_kept_it() {
    for (hidden, kept) in [(true, Kept::AtOnce), (false, Kept::InItsTime)] {
        let rig = Rig::new();
        rig.window.hidden_changed(hidden, &rig.saver);
        assert_eq!((rig.app().app_hidden, rig.kept()), (hidden, kept), "hidden {hidden}");
    }
}

/// Failure: a launch comes back hidden when the person opened it themselves, or shows an app an update brought back that was hidden, or doesn't say it's
/// decided, so a state read late puts back what the host already chose: the bridge's `app_comes_back_hidden`, hidden only when it was and an update reopened
/// it, and decided either way.
#[test]
fn coming_back_hidden_is_decided_as_the_bridge_decided_it() {
    for (was, reopened, answer) in [(false, false, false), (true, false, false), (false, true, false), (true, true, true)] {
        let rig = Rig::new();
        lock(&rig.window.app).app_hidden = was;
        assert_eq!(rig.window.comes_back_hidden(reopened), answer, "was hidden {was}, reopened {reopened}");
        let app = rig.app();
        assert_eq!((app.app_hidden, app.hidden_decided), (answer, true), "was hidden {was}, reopened {reopened}");
        assert_eq!(rig.kept(), Kept::Not, "nothing written by deciding");
    }
}

/// Failure: a launch comes up behind what the person's doing when they opened it, or in front of their work when an update brought it back while another
/// app was in front: the bridge's `opens_in_background`.
#[test]
fn opening_behind_is_decided_as_the_bridge_decided_it() {
    for (frontmost, reopened, answer) in [(true, false, false), (false, false, false), (true, true, false), (false, true, true)] {
        let rig = Rig::new();
        lock(&rig.window.app).focus.app_frontmost = frontmost;
        assert_eq!(rig.window.opens_in_background(reopened), answer, "frontmost {frontmost}, reopened {reopened}");
    }
}
