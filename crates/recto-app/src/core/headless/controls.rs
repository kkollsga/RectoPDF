//! **The window's every control, with the session busy** (step R2.3): what the person does is taken at once, whatever the session thread is doing.

use crate::RowKind;
use crate::core::controls::UiAction;
use crate::saving::CloseChoice;
use crate::testkit::headless::{Disk, WAIT_LIMIT};
use std::sync::mpsc;
use std::time::Duration;

/// Each kind of control's place, with no default, so a new one doesn't compile here until it's in the list below: `(place, how many places in all)`.
fn place(action: &UiAction) -> (usize, usize) {
    let place = match action {
        UiAction::Menu { .. } => 0,
        UiAction::Row { adding: false, .. } => 1,
        UiAction::Row { adding: true, .. } => 2,
        UiAction::PaneRead { .. } => 3,
        UiAction::Chevron { .. } => 4,
        UiAction::Open { .. } => 5,
        UiAction::OpenRecent { .. } => 6,
        UiAction::FindTyped { .. } => 7,
        UiAction::FindOpen => 8,
        UiAction::FindClose => 9,
        UiAction::FindReturn { .. } => 10,
        UiAction::FindChoose { .. } => 11,
        UiAction::PageTyped { .. } => 12,
        UiAction::Swiped { .. } => 13,
        UiAction::ZoomTyped { .. } => 14,
        UiAction::ToggleSidebar => 15,
        UiAction::PaletteRun { .. } => 16,
        UiAction::CloseAnswered { .. } => 17,
        UiAction::SaveAs { .. } => 18,
        UiAction::SettingChosen { .. } => 19,
        UiAction::Print { .. } => 20,
        UiAction::LinkClicked { .. } => 21,
        UiAction::SnapshotTaken => 22,
        UiAction::Pasted { .. } => 23,
    };
    (place, 24)
}

/// An app with two files open, and one of every kind of control on it, every menu item among them.
fn every_kind_of_control() -> (crate::testkit::headless::HeadlessApp, Vec<UiAction>) {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 4), disk.pdf("b.pdf", 4));
    let saved_as = disk.files().join("copy.pdf");
    let not_open = disk.pdf("c.pdf", 2);
    let app = disk.start();
    app.open(&a);
    app.open(&b);
    let recent_key = app.core().recent_files()[0].key.clone();
    let rows = app.tabs();
    let (row, tab) = (rows[0].clone(), rows[0].tab);
    let contents = crate::SidebarRow {
        kind: RowKind::Contents,
        page: Some(2),
        ..row.clone()
    };
    let mut actions: Vec<UiAction> = app
        .core()
        .bar_now()
        .into_iter()
        .flat_map(|menu| menu.entries)
        .filter(|entry| !entry.id.is_empty())
        .map(|entry| UiAction::Menu { id: entry.id })
        .collect();
    actions.extend([
        UiAction::Row { row: contents, adding: false },
        // The note over the contents is `contents show` or `contents stop`, and the first on a thread of its own (C5f).
        UiAction::Row {
            row: crate::SidebarRow {
                kind: RowKind::Note,
                action: Some(recto_session::ContentsAction::Generate.id().into()),
                ..row.clone()
            },
            adding: false,
        },
        UiAction::Row {
            row: crate::SidebarRow {
                kind: RowKind::Note,
                action: Some(recto_session::ContentsAction::Stop.id().into()),
                ..row.clone()
            },
            adding: false,
        },
        UiAction::Row {
            row: rows[1].clone(),
            adding: true,
        },
        // A ⌘-click puts the two side by side, and the reader moves into the other pane: `compare --toggle` and `--reading` (C5e).
        UiAction::PaneRead { tab },
        UiAction::Chevron { first: 0 },
        UiAction::Open { path: a.display().to_string() },
        UiAction::OpenRecent { key: "0".repeat(16) },
        // Find's field, its list and its keys are the person's `find` (C5g): a query, opening, a jump each way, a hit's row and a page's, and closing.
        UiAction::FindTyped { text: "page".into() },
        UiAction::FindOpen,
        UiAction::FindReturn { forward: true },
        UiAction::FindReturn { forward: false },
        UiAction::FindChoose { row: 0 },
        UiAction::FindChoose { row: 1 },
        UiAction::FindClose,
        UiAction::PageTyped { text: "2".into() },
        UiAction::Swiped { delta_x: 1.0 },
        // A mouse-up on a page with no link clicked: nothing to follow (a click on one is below, from the caller's thread).
        UiAction::LinkClicked { pane: 0 },
        // The snapshot tool's release with nothing taken, and a paste of an offer there isn't: nothing to do, at once.
        UiAction::SnapshotTaken,
        UiAction::Pasted {
            serial: 1,
            uti: "public.svg-image".into(),
        },
        UiAction::ZoomTyped {
            text: "150".into(),
            decimal: ".".into(),
        },
        UiAction::ToggleSidebar,
        UiAction::PaletteRun { id: "view.zoom_in".into() },
        // The palette's tab row is a person's `tap`, and Clear Menu `recent clear` (C5c).
        UiAction::PaletteRun { id: format!("tab:{tab}") },
        UiAction::Menu { id: "recent.clear".into() },
        // The panel's Rotate is a person's `page rotate`, resolved when it runs, and Undo and Redo `undo` and `redo` (C5d).
        UiAction::Menu { id: "rotate.right".into() },
        UiAction::Menu { id: "edit.undo".into() },
        UiAction::Menu { id: "edit.redo".into() },
        UiAction::Menu { id: "edit_mode.done".into() },
        UiAction::Menu {
            id: "edit.clear_history".into(),
        },
        // Opening a file that isn't open, and Open Recent's file, are a person's `open` (C5c).
        UiAction::Open {
            path: not_open.display().to_string(),
        },
        UiAction::OpenRecent { key: recent_key },
        UiAction::CloseAnswered {
            tab,
            choice: CloseChoice::Cancel,
        },
        // The close prompt's Save is the window's Save, on a thread of its own, and Don't Save `discard` and `close` (C5c).
        UiAction::CloseAnswered {
            tab,
            choice: CloseChoice::Save,
        },
        UiAction::CloseAnswered {
            tab,
            choice: CloseChoice::DontSave,
        },
        UiAction::SaveAs {
            tab,
            path: saved_as.display().to_string(),
        },
        UiAction::SettingChosen {
            id: crate::core::updates::AUTO_UPDATE_ROW.into(),
            on: false,
        },
        // The print panel's answer is the person's `print`, on a thread of its own (S2).
        UiAction::Print {
            tab,
            all: true,
            first: 1,
            last: 1,
            selection_only: false,
            to_file: false,
            serial: 1,
        },
        UiAction::Row { row, adding: false },
    ]);
    (app, actions)
}

/// Failure (rule 3): a control the window has waits for the session's lock on the UI thread, so a command holding it (a save, a long read) freezes the window
/// for as long: the zoom, now `view --zoom` run on the calling thread, would if it ran the session's part of `view`. Every kind of control, and every menu
/// item, and a click on a link, called while another thread holds the session, each returns.
#[test]
fn every_control_returns_while_the_session_is_held() {
    let (app, actions) = every_kind_of_control();
    let mut kinds: Vec<usize> = actions.iter().map(|action| place(action).0).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds, (0..place(&actions[0]).1).collect::<Vec<_>>(), "one of every kind of control");
    let core = std::sync::Arc::clone(app.core());
    let (held_tx, held_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let holder = std::thread::Builder::new()
        .name("test-holder".into())
        .spawn(move || {
            core.host.session.with_session(|_| {
                held_tx.send(()).expect("the test is waiting");
                // Held until every control has returned, or the test has given up on one.
                let _ = done_rx.recv_timeout(WAIT_LIMIT);
            });
        })
        .expect("a thread");
    held_rx.recv_timeout(WAIT_LIMIT).expect("the session held");
    let (returned_tx, returned_rx) = mpsc::channel();
    let caller = {
        let core = std::sync::Arc::clone(app.core());
        std::thread::Builder::new()
            .name("test-caller".into())
            .spawn(move || {
                for action in actions {
                    let what = format!("{action:?}");
                    core.act(action);
                    if returned_tx.send(what).is_err() {
                        return;
                    }
                }
                // A click on a link, which the page view reports: to a page, and Back.
                for target in [
                    recto_core::nav::LinkTarget::Page { page: 2, within: Some(0.5) },
                    recto_core::nav::LinkTarget::History { back: true },
                ] {
                    let what = format!("a link click, {target:?}");
                    let engine = core.panes.reading_engine(&core.engine);
                    engine.links().clicked_for_qa(recto_reader::links::LinkHit {
                        page: 0,
                        rect: [0.0; 4],
                        target,
                    });
                    core.act(UiAction::LinkClicked { pane: 0 });
                    if returned_tx.send(what).is_err() {
                        return;
                    }
                }
            })
            .expect("a thread")
    };
    let (mut returned, mut last) = (0, String::new());
    loop {
        match returned_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(what) => {
                returned += 1;
                last = what;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drop(done_tx);
                panic!("a control didn't return with the session held: the one after {last}, with {returned} returned");
            }
        }
    }
    drop(done_tx);
    holder.join().expect("the holder");
    caller.join().expect("the caller");
    app.settle();
}

/// Failure (the stall record, 2026-11-16): a control has no name for a stall's record to say, or two kinds of control share one so a stall in one reads as the
/// other. Every kind of control, by kind: each name is one word of letters, and no two kinds have the same.
#[test]
fn every_control_has_a_name_of_its_own_for_the_stall_record() {
    let (_app, actions) = every_kind_of_control();
    let mut by_name: std::collections::BTreeMap<&'static str, std::mem::Discriminant<UiAction>> = std::collections::BTreeMap::new();
    for action in &actions {
        let name = action.name();
        assert!(
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic()) && name.len() <= 24,
            "{name:?}"
        );
        let kind = std::mem::discriminant(action);
        assert_eq!(*by_name.entry(name).or_insert(kind), kind, "{name} names two kinds of control");
    }
    // That the list holds every kind is `every_kind_of_control`'s own check (`place`'s match is exhaustive), so no count is repeated here.
}

/// Failure (the stall record, 2026-11-16): `Inner::act` stops saying what it begins, so a stall in a control is kept with no name, or one run off the window's
/// thread (as the QA channel's `SaveAs` is) is named as the window's own pass. A pass of 20 ms on the window's thread that runs a control names it.
#[test]
fn a_control_on_the_window_thread_names_its_pass_and_off_it_does_not() {
    let (app, _) = every_kind_of_control();
    let core = std::sync::Arc::clone(app.core());
    assert!(core.window.main_busy(false));
    core.act(UiAction::FindOpen);
    let other = std::sync::Arc::clone(&core);
    std::thread::Builder::new()
        .name("test-qa-channel".into())
        .spawn(move || other.act(UiAction::ToggleSidebar))
        .expect("a thread")
        .join()
        .expect("the other thread's action");
    // A pass is kept only over 16 ms of wall time: held that long by the clock, not by a sleep.
    let until = std::time::Instant::now() + Duration::from_millis(20);
    while std::time::Instant::now() < until {
        std::hint::spin_loop();
    }
    core.window.main_idle();
    let kept = core.window.kept_stretches();
    assert_eq!(kept.last().map(|stretch| stretch.doing.as_str()), Some("FindOpen"), "{kept:?}");
}

/// Failure (the stall record, 2026-11-16): a bridge call that the window's thread makes (a scroll, a press, a drag or a release on a page, a paste's bytes) stops
/// naming its pass, so a stall in it is kept with no name, or under the word of a call before it. Each of the five functions the bridge reaches names its pass
/// when called on the window's thread, and one called from another thread (the QA channel's) names nothing.
#[test]
fn the_calls_the_bridge_makes_name_their_pass_on_the_window_thread_and_off_it_do_not() {
    let (app, _) = every_kind_of_control();
    let core = std::sync::Arc::clone(app.core());
    let name_of = |call: &dyn Fn()| {
        assert!(core.window.main_busy(false));
        call();
        let until = std::time::Instant::now() + Duration::from_millis(20);
        while std::time::Instant::now() < until {
            std::hint::spin_loop();
        }
        core.window.main_idle();
        core.window
            .kept_stretches()
            .iter()
            .map(|stretch| stretch.doing.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    let reached: [(&str, &dyn Fn()); 5] = [
        ("scroll", &|| {
            drop(core.panes.viewport(&core.engine, 0, false, recto_reader::Viewport::default()))
        }),
        ("select_press", &|| drop(core.panes.pointer_engine("select_press", &core.engine, 0))),
        ("select_drag", &|| drop(core.panes.pointer_engine("select_drag", &core.engine, 0))),
        ("select_release", &|| drop(core.panes.pointer_engine("select_release", &core.engine, 0))),
        ("clipboard_data", &|| drop(core.tools.tool().ready_bytes(1, "public.utf8-plain-text"))),
    ];
    for (word, call) in reached {
        let kept = name_of(call);
        // Each call's pass is its own stretch (the list is longest first, so not in order), and each word is one of its own.
        assert!(kept.iter().any(|kept| kept == word), "{word}: {kept:?}");
    }
    // From another thread: the QA channel's. Its word isn't the window's.
    let kept = name_of(&|| {
        let other = std::sync::Arc::clone(&core);
        std::thread::Builder::new()
            .name("test-qa-channel".into())
            .spawn(move || drop(other.panes.pointer_engine("qa_word", &other.engine, 0)))
            .expect("a thread")
            .join()
            .expect("the other thread");
    });
    assert!(!kept.iter().any(|word| word == "qa_word"), "{kept:?}");
}
