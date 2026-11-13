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
        UiAction::Chevron { .. } => 3,
        UiAction::Open { .. } => 4,
        UiAction::OpenRecent { .. } => 5,
        UiAction::FindTyped { .. } => 6,
        UiAction::FindOpen => 7,
        UiAction::FindClose => 8,
        UiAction::FindReturn { .. } => 9,
        UiAction::FindChoose { .. } => 10,
        UiAction::PageTyped { .. } => 11,
        UiAction::Swiped { .. } => 12,
        UiAction::ZoomTyped { .. } => 13,
        UiAction::ToggleSidebar => 14,
        UiAction::PaletteRun { .. } => 15,
        UiAction::CloseAnswered { .. } => 16,
        UiAction::SaveAs { .. } => 17,
        UiAction::SettingChosen { .. } => 18,
    };
    (place, 19)
}

/// Failure (rule 3): a control the window has waits for the session's lock on the UI thread, so a command holding it (a save, a long read) freezes the window
/// for as long: the zoom, now `view --zoom` run on the calling thread, would if it ran the session's part of `view`. Every kind of control, and every menu
/// item, and a click on a link, called while another thread holds the session, each returns.
#[test]
fn every_control_returns_while_the_session_is_held() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 4), disk.pdf("b.pdf", 4));
    let saved_as = disk.files().join("copy.pdf");
    let app = disk.start();
    app.open(&a);
    app.open(&b);
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
        UiAction::Row {
            row: row.clone(),
            adding: true,
        },
        UiAction::Chevron { first: 0 },
        UiAction::Open { path: a.clone() },
        UiAction::OpenRecent { key: "0".repeat(16) },
        UiAction::FindTyped { text: "page".into() },
        UiAction::FindOpen,
        UiAction::FindReturn { forward: true },
        UiAction::FindChoose { row: 0 },
        UiAction::FindClose,
        UiAction::PageTyped { text: "2".into() },
        UiAction::Swiped { delta_x: 1.0 },
        UiAction::ZoomTyped {
            text: "150".into(),
            decimal: '.',
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
        UiAction::Row { row, adding: false },
    ]);
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
                    crate::core::follow_link(
                        &core,
                        0,
                        &engine,
                        recto_reader::links::LinkHit {
                            page: 0,
                            rect: [0.0; 4],
                            target,
                        },
                    );
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
