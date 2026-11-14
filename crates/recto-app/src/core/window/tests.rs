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
        match self.saver.wait_for_due() {
            Some(due) if due.is_zero() => Kept::AtOnce,
            Some(_) => Kept::InItsTime,
            None => Kept::Not,
        }
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

/// A screen with a name that needs cleaning, as a display's can.
fn screen() -> ScreenInfo {
    ScreenInfo {
        id: 7,
        name: "Studio\u{7}  Display\n".into(),
        x: 0.0,
        y: 0.0,
        width: 2560.0,
        height: 1440.0,
        visible_x: 0.0,
        visible_y: 25.0,
        visible_width: 2560.0,
        visible_height: 1415.0,
    }
}

/// Failure: a move or resize is kept otherwise than the bridge kept it: a window not told of yet isn't made at the default frame, a frame that isn't four
/// numbers is taken anyway, a screen's name goes into the record with what a name can't hold, full screen isn't kept with no frame, or it isn't written:
/// the bridge's `window_changed`.
#[test]
fn a_move_is_kept_as_the_bridge_kept_it() {
    let rect = |x, y, width, height| record::Rect { x, y, width, height };
    // (a window told of already, the frame, a screen, full screen) → (its frame, its screen's name, full screen)
    type Row = (bool, Option<Vec<f64>>, bool, bool, record::Rect, Option<&'static str>, bool);
    let table: [Row; 6] = [
        (false, None, false, true, rect(0.0, 0.0, 1440.0, 900.0), None, true),
        (false, None, true, false, rect(0.0, 0.0, 1440.0, 900.0), None, false),
        (
            false,
            Some(vec![10.0, 20.0, 800.0, 600.0]),
            true,
            false,
            rect(10.0, 20.0, 800.0, 600.0),
            Some("Studio Display"),
            false,
        ),
        (
            true,
            Some(vec![10.0, 20.0, 800.0, 600.0]),
            false,
            true,
            rect(10.0, 20.0, 800.0, 600.0),
            None,
            true,
        ),
        (true, Some(vec![10.0, 20.0, 800.0]), true, false, rect(1.0, 2.0, 900.0, 700.0), None, false),
        (
            true,
            Some(vec![10.0, 20.0, 800.0, 600.0, 5.0]),
            true,
            true,
            rect(1.0, 2.0, 900.0, 700.0),
            None,
            true,
        ),
    ];
    for (told, frame, on_screen, full_screen, kept_frame, kept_screen, kept_full) in table {
        let rig = if told { Rig::new().with_a_window() } else { Rig::new() };
        let asked = format!("told {told}, frame {frame:?}, screen {on_screen}, full screen {full_screen}");
        rig.window.changed(frame, on_screen.then(screen), full_screen, &rig.saver);
        let window = rig.app().window.expect("a window kept");
        assert_eq!(window.frame, kept_frame, "{asked}");
        assert_eq!(window.screen.as_ref().map(|screen| screen.name.as_str()), kept_screen, "{asked}");
        assert_eq!((window.full_screen, window.minimised), (kept_full, false), "{asked}");
        if let Some(screen) = window.screen {
            assert_eq!(
                (screen.id, screen.frame),
                (7, rect(0.0, 0.0, 2560.0, 1440.0)),
                "the screen's whole frame, not its visible part"
            );
        }
        assert_eq!(rig.kept(), Kept::InItsTime, "{asked}");
    }
}

/// Failure: the sidebar's width is kept past its limits or as what isn't a number, so a record makes a sidebar nobody can use, or the window isn't drawn
/// again at it: the bridge's `set_sidebar_width`, held to `record::sidebar_width`'s limits and redrawn by the session thread every time.
#[test]
fn the_sidebars_width_is_kept_as_the_bridge_kept_it() {
    let (narrowest, widest) = record::SIDEBAR_WIDTHS;
    for (asked, kept) in [
        (10.0, narrowest),
        (300.4, 300.0),
        (300.6, 301.0),
        (10_000.0, widest),
        (f64::NAN, record::SIDEBAR_WIDTH),
        (f64::INFINITY, record::SIDEBAR_WIDTH),
    ] {
        let rig = Rig::new();
        rig.window.set_sidebar_width(asked, &rig.core);
        assert_eq!(rig.app().sidebar_width, kept, "asked {asked}");
        assert_eq!(rig.queued(), ["Reconcile"], "asked {asked}");
        assert_eq!(rig.kept(), Kept::Not, "the record goes with the next reconcile, not here");
    }
}

/// Failure: a scroll that isn't a number is kept, so the record can't be read back, or one is lost: the bridge's `contents_position`.
#[test]
fn the_contents_scroll_is_kept_as_the_bridge_kept_it() {
    for (asked, kept) in [
        (120.5, Some(120.5)),
        (0.0, Some(0.0)),
        (-4.0, Some(-4.0)),
        (f64::NAN, None),
        (f64::NEG_INFINITY, None),
    ] {
        let rig = Rig::new();
        lock(&rig.window.app).contents_y = Some(9.0);
        rig.window.contents_position(asked, &rig.saver);
        assert_eq!((rig.app().contents_y, rig.kept()), (kept, Kept::InItsTime), "asked {asked}");
    }
}

/// Failure: a focus the host names that's none of the three is kept as something, or written for nothing, or a real one is lost: the bridge's
/// `focus_changed` and `focus_view`.
#[test]
fn focus_is_kept_as_the_bridge_kept_it() {
    for (asked, answer, kept) in [
        ("page", "page", Kept::InItsTime),
        ("sidebar", "sidebar", Kept::InItsTime),
        ("find", "find", Kept::InItsTime),
        ("the moon", "page", Kept::Not),
        ("Sidebar", "page", Kept::Not),
        ("", "page", Kept::Not),
    ] {
        let rig = Rig::new();
        rig.window.focus_changed(asked, &rig.saver);
        assert_eq!((rig.window.focus_view(), rig.kept()), (answer, kept), "asked {asked:?}");
    }
}

/// Failure: an appearance the host names that's neither light nor dark is kept as one of them, or a choice isn't written: the bridge's
/// `set_appearance_choice` and `appearance_choice`.
#[test]
fn the_appearance_is_kept_as_the_bridge_kept_it() {
    for (asked, kept, answer) in [
        ("light", Appearance::Light, "light"),
        ("dark", Appearance::Dark, "dark"),
        ("system", Appearance::System, "system"),
        ("Dark", Appearance::System, "system"),
        ("plaid", Appearance::System, "system"),
    ] {
        let rig = Rig::new();
        lock(&rig.window.app).appearance = Appearance::Light;
        rig.window.set_appearance_choice(asked, &rig.saver);
        assert_eq!(
            (rig.app().appearance, rig.window.appearance_choice(), rig.kept()),
            (kept, answer, Kept::InItsTime),
            "asked {asked:?}"
        );
    }
}

/// Failure: the system going dark isn't drawn: the bridge's `set_dark_appearance`, kept and the window redrawn either way.
#[test]
fn the_systems_appearance_is_kept_as_the_bridge_kept_it() {
    for dark in [true, false] {
        let rig = Rig::new();
        rig.window.set_dark(dark, &rig.core);
        assert_eq!(rig.window.dark.load(Ordering::SeqCst), dark);
        assert_eq!(rig.queued(), ["Reconcile"], "dark {dark}");
    }
}

/// Failure: going to the back waits for the writer's time, so a quit from the Dock or a logout loses what changed; coming back writes at once for nothing;
/// coming to the front doesn't look at the files showing (another app may have moved, replaced or deleted one), or a change of key window alone looks at
/// them; or the app in front and the window key aren't kept: `Inner::frontmost_changed`, as it was before it moved here.
#[test]
fn the_app_in_front_is_kept_as_the_bridge_kept_it() {
    // (in front before, in front now, the window key) → (what the writer was asked for, what the session thread was sent)
    type Row = ((bool, bool, bool), (Kept, &'static [&'static str]));
    let table: [Row; 5] = [
        ((true, false, false), (Kept::AtOnce, &[])),
        ((false, true, true), (Kept::InItsTime, &["LookAtShown"])),
        ((false, true, false), (Kept::InItsTime, &["LookAtShown"])),
        ((true, true, false), (Kept::InItsTime, &[])),
        ((false, false, false), (Kept::InItsTime, &[])),
    ];
    for ((was, frontmost, key), (kept, sent)) in table {
        let rig = Rig::new();
        lock(&rig.window.app).focus.app_frontmost = was;
        rig.window.frontmost_changed(frontmost, key, &rig.saver, &rig.core);
        let asked = format!("in front {was} → {frontmost}, key {key}");
        let focus = rig.app().focus;
        assert_eq!((focus.app_frontmost, focus.window_key), (frontmost, key), "{asked}");
        assert_eq!(rig.queued(), sent, "{asked}");
        assert_eq!(rig.kept(), kept, "{asked}");
    }
}

/// Failure: where the Settings window is, or that it's closed, isn't kept or isn't written, so it comes back where it was before, or open after it was
/// closed: the bridge's `settings_window_changed`, kept and written in its own time.
#[test]
fn the_settings_window_is_kept_as_the_bridge_kept_it() {
    let place = SettingsWindow { x: 40.0, y: 50.0 };
    for (was, now) in [(None, Some(place)), (Some(place), None), (Some(SettingsWindow { x: 1.0, y: 2.0 }), Some(place))] {
        let rig = Rig::new();
        lock(&rig.window.app).settings_window = was;
        rig.window.settings_window_changed(now, &rig.saver);
        assert_eq!(rig.window.settings_window(), now, "{was:?} → {now:?}");
        assert_eq!(rig.kept(), Kept::InItsTime, "{was:?} → {now:?}");
        assert!(rig.queued().is_empty(), "{was:?} → {now:?}: nothing redrawn for it");
    }
}

/// What a placement's answer is, in one tuple for a table: saved, the frame, full screen, activate, the sidebar focused, in the background, full screen
/// when active, in the Dock.
type Placed = (bool, (f64, f64, f64, f64), bool, bool, bool, bool, bool, bool);

fn placed(placement: WindowPlacement) -> Placed {
    (
        placement.saved,
        (placement.x, placement.y, placement.width, placement.height),
        placement.full_screen,
        placement.activate,
        placement.focus_sidebar,
        placement.in_background,
        placement.full_screen_when_active,
        placement.minimised,
    )
}

/// Failure: a new window is placed otherwise than the bridge placed it: a launch with no window saved isn't a new one, a quiet run's window comes onto a
/// screen (taking the person's), an update arriving behind their work goes into full screen and moves them to another Space, the Dock is kept for a launch
/// the person asked for, or a launch doesn't say the Dock's been decided, so a state read late puts it back: the bridge's `window_placement`.
#[test]
fn a_new_window_is_placed_as_the_bridge_placed_it() {
    let (min_width, min_height) = record::WINDOW_MIN;
    // (a window saved, in the Dock, full screen, in front, the sidebar focused, reopened by an update, quiet) → the placement
    type Asked = (bool, bool, bool, bool, bool, bool, bool);
    let table: [(Asked, Placed); 9] = [
        // No window saved: a new one, wherever the host starts it.
        (
            (false, false, false, true, false, false, false),
            (false, (0.0, 0.0, 0.0, 0.0), false, true, false, false, false, false),
        ),
        (
            (false, false, false, false, true, true, false),
            (false, (0.0, 0.0, 0.0, 0.0), false, false, true, true, false, false),
        ),
        // Where it was, on the screen there is.
        (
            (true, false, false, true, false, false, false),
            (true, (100.0, 100.0, 900.0, 700.0), false, true, false, false, false, false),
        ),
        // Full screen comes back for an app that comes forward, and waits for one that arrives behind the person's work.
        (
            (true, false, true, true, false, false, false),
            (true, (100.0, 100.0, 900.0, 700.0), true, true, false, false, false, false),
        ),
        (
            (true, false, true, false, false, true, false),
            (true, (100.0, 100.0, 900.0, 700.0), false, false, false, true, true, false),
        ),
        // The Dock across an update only.
        (
            (true, true, false, true, false, false, false),
            (true, (100.0, 100.0, 900.0, 700.0), false, true, false, false, false, false),
        ),
        (
            (true, true, false, true, false, true, false),
            (true, (100.0, 100.0, 900.0, 700.0), false, true, false, false, false, true),
        ),
        // A quiet run: off every screen, at its size held to the smallest a window goes, and never behind.
        (
            (true, false, true, false, true, true, true),
            (true, (-40_000.0, -40_000.0, 900.0, 700.0), true, false, true, false, false, false),
        ),
        (
            (true, true, false, true, false, true, true),
            (true, (-40_000.0, -40_000.0, 900.0, 700.0), false, true, false, false, false, true),
        ),
    ];
    let screen = ScreenInfo {
        id: 1,
        name: "Built-in".into(),
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
        visible_x: 0.0,
        visible_y: 0.0,
        visible_width: 1920.0,
        visible_height: 1055.0,
    };
    for ((saved, in_dock, full_screen, frontmost, sidebar, reopened, quiet), expected) in table {
        let rig = Rig::new();
        {
            let mut app = lock(&rig.window.app);
            app.focus.app_frontmost = frontmost;
            app.focus.view = if sidebar { record::FocusView::Sidebar } else { record::FocusView::Page };
            app.window = saved.then_some(WindowRecord {
                frame: record::Rect {
                    x: 100.0,
                    y: 100.0,
                    width: 900.0,
                    height: 700.0,
                },
                screen: None,
                full_screen,
                minimised: in_dock,
            });
        }
        let asked = format!("saved {saved}, in the Dock {in_dock}, full screen {full_screen}, in front {frontmost}, reopened {reopened}, quiet {quiet}");
        assert_eq!(placed(rig.window.placement(vec![screen.clone()], reopened, quiet)), expected, "{asked}");
        let app = rig.app();
        assert!(app.minimised_decided, "{asked}");
        assert_eq!(
            app.window.map(|window| window.minimised),
            saved.then_some(expected.7),
            "{asked}: the Dock as kept"
        );
    }
    // A quiet run's window is never smaller than a window goes.
    let rig = Rig::new().with_a_window();
    lock(&rig.window.app).window.as_mut().expect("a window").frame.width = 10.0;
    let placement = rig.window.placement(Vec::new(), false, true);
    assert_eq!((placement.width, placement.height), (min_width, 700.0_f64.max(min_height)));
}

/// Failure (the review of R3.7's S2): the notice and the window's locks nest, so the UI thread's `notice_read` waits on a writer copying the window: the
/// saver passed the notice as a temporary of its statement, which held the notice's lock through the window's two. The notice is now read by what's passed
/// in, on a line of its own, so its lock ends inside that call; and the window holds neither of its locks while it's read, asserted from inside the read.
/// The notice is in the record as read.
#[test]
fn the_records_notice_is_read_with_the_windows_locks_let_go() {
    let rig = Rig::new().with_a_window();
    let mut read = 0;
    let record = rig.window.app_record(|| {
        read += 1;
        assert!(rig.window.app.try_lock().is_ok(), "the window's state is held while the notice is read");
        assert!(
            rig.window.contents_follow.try_lock().is_ok(),
            "the contents' follow is held while the notice is read"
        );
        Some("other_instance".to_string())
    });
    assert_eq!((read, record.notice.as_deref()), (1, Some("other_instance")));
    assert!(record.window.is_some(), "the window's own part as it is");
}

/// Failure: a menu's update answer put up as an alert over a window that's showing, or said nowhere with the window closed or in the Dock (the rule
/// `take_update_alert` read off the window's state in `core/updates.rs` before it moved here, 2026-11-14). Showing: open and not in the Dock; a window that
/// has no frame yet is open.
#[test]
fn showing_is_open_and_not_in_the_dock() {
    let rig = Rig::new();
    assert!(rig.window.showing(), "at launch, with no frame yet");
    lock(&rig.window.app).window = Some(WindowRecord {
        frame: recto_session::record::Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
        screen: None,
        full_screen: false,
        minimised: false,
    });
    assert!(rig.window.showing());
    rig.window.minimised_changed(true, &rig.saver);
    assert!(!rig.window.showing(), "in the Dock");
    rig.window.minimised_changed(false, &rig.saver);
    rig.window.closed();
    assert!(!rig.window.showing(), "closed");
}
