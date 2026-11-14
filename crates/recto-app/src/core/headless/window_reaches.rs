//! **What core files did with the window's state from outside it, pinned before it moves into `Window`** (R3.7, S2b; AGENTS.md, "guards go in before a
//! move"): the restore of the window's part of a saved session at each way it can come in, the notice and the open failure (said, shown, and when each goes),
//! and the panel's toggle. The window's own steps are `window_script.rs`'s.

use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_platform::slow::Seam;

/// The window's whole state as Rust keeps it, and what goes beside it: the system's appearance, the panel, the contents' follow, and what's selected on the page
/// being read (the selection put back on the tab it was on).
fn window_state(app: &HeadlessApp) -> String {
    let inner = app.core();
    let state = format!("{:?}", *inner.window.state_for_tests());
    let selection = inner.panes.reading_engine(&inner.engine).selection().hot();
    format!(
        "{state} | dark={} panel={} following={} moving={} selection={selection:?} notice={:?}",
        inner.window.is_dark(),
        inner.window.panel_open(),
        inner.window.contents_following(),
        inner.window.contents_moving(),
        crate::lock(&inner.host.notice).clone(),
    )
}

/// A session of two tabs whose window was told everything the record keeps of it, each by the call the host makes: moved, a sidebar width, the panel open,
/// Settings open, dark chosen, the sidebar focused, the contents scrolled by the person, in the Dock, hidden, and the page's text selected. Quit, as the copy
/// before.
fn a_saved_window() -> Disk {
    let app = HeadlessApp::start();
    let (a, b) = (app.disk().pdf("a.pdf", 3), app.disk().pdf("b.pdf", 3));
    app.open(&a);
    app.open(&b);
    let inner = app.core();
    let (window, saver) = (&inner.window, &inner.saver);
    window.changed(Some(vec![10.0, 20.0, 800.0, 600.0]), None, false, saver);
    window.set_sidebar_width(300.0, &inner.core);
    app.act(UiAction::Menu { id: "panel.toggle".into() });
    window.settings_window_changed(Some(recto_session::record::SettingsWindow { x: 40.0, y: 50.0 }), saver);
    window.set_appearance_choice("dark", saver);
    window.focus_changed("sidebar", saver);
    window.contents_position(123.0, saver);
    window.contents_user_scrolled(inner.life.seconds_since_start());
    window.minimised_changed(true, saver);
    window.hidden_changed(true, saver);
    inner.panes.reading_engine(&inner.engine).selection().select_all();
    app.settle();
    let (answer, disk) = app.quit();
    assert!(answer.go, "{answer:?}");
    disk
}

/// What the host says of its own window before a state read late comes in, as a launch without the state does: the window moved, and asked what comes back
/// hidden, in the Dock, and open.
fn the_host_decides(app: &HeadlessApp) {
    let inner = app.core();
    inner.window.changed(Some(vec![500.0, 60.0, 700.0, 500.0]), None, false, &inner.saver);
    let _ = inner.window.comes_back_hidden(false);
    let _ = inner.window.placement(Vec::new(), false, false);
    app.settle();
}

/// The window's state after each way a saved window comes back, a line each.
fn restore_lines() -> Vec<String> {
    let mut lines = Vec::new();
    // A launch that reads the state in time.
    let app = a_saved_window().start();
    app.settle();
    lines.push(format!("in time | {}", window_state(&app)));
    drop(app.quit());
    // Read late, with the window up and decided first, and taking no input yet: made again from what was saved.
    let app = a_saved_window().builder().hold(&[Seam::State]).start();
    the_host_decides(&app);
    app.let_state_in();
    app.settle();
    lines.push(format!("late, before its patience | {}", window_state(&app)));
    drop(app.quit());
    // Read late, past the window's patience, the person having chosen light meanwhile: the window that takes input keeps what it shows.
    let app = a_saved_window().builder().hold(&[Seam::State]).start();
    the_host_decides(&app);
    app.advance(u64::try_from(crate::core::updates::INPUT_PATIENCE.as_millis()).expect("ms"));
    app.core().window.set_appearance_choice("light", &app.core().saver);
    app.let_state_in();
    app.settle();
    lines.push(format!("late, past its patience | {}", window_state(&app)));
    drop(app.quit());
    // A late keep's restore, the person having moved the window, chosen light, focused Find, set the sidebar's width and opened Settings while it was read.
    let app = super::late_lock::restoring_held(a_saved_window());
    let inner = app.core();
    inner.window.changed(Some(vec![222.0, 30.0, 640.0, 480.0]), None, false, &inner.saver);
    inner.window.set_appearance_choice("light", &inner.saver);
    inner.window.focus_changed("find", &inner.saver);
    inner.window.set_sidebar_width(250.0, &inner.core);
    inner
        .window
        .settings_window_changed(Some(recto_session::record::SettingsWindow { x: 7.0, y: 8.0 }), &inner.saver);
    super::late_lock::let_the_restore_in(&app);
    lines.push(format!("a late keep | {}", window_state(&app)));
    drop(app.quit());
    lines
}

/// Failure (R3.7, S2b, a guard before the restore moves from `session::apply_restored` into `Window`): a saved window comes back otherwise: a field of the
/// window's state not put back, or put back over what the host decided (hidden, the Dock, closed) or over a window taking input (its appearance), the window
/// made again from something other than what was saved, a late keep's restore putting the saved frame, focus, width, Settings or appearance over what the
/// person did meanwhile, or the contents' scroll or the selection not taken by the tab they were on. Each way's line is held against the golden.
#[test]
fn a_saved_window_comes_back_as_it_did_each_way() {
    let now = restore_lines();
    let golden: Vec<&str> = include_str!("golden/window_restore.txt").lines().collect();
    if golden.iter().copied().eq(now.iter().map(String::as_str)) {
        return;
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/recto-golden");
    let _ = std::fs::create_dir_all(&out);
    let written = out.join("window_restore.txt");
    let _ = std::fs::write(&written, now.join("\n") + "\n");
    let first = golden.iter().zip(&now).find(|(was, is)| **was != is.as_str()).map_or_else(
        || format!("{} lines, the golden {}", now.len(), golden.len()),
        |(was, is)| format!("golden: {was}\nnow:    {is}"),
    );
    panic!("a saved window came back otherwise:\n{first}\n(all of it now: {})", written.display());
}

/// The notice that goes by itself, as the window keeps it.
fn notice(app: &HeadlessApp) -> Option<String> {
    app.core().window.notices().notice()
}

/// Why an open failed, as the window keeps it.
fn open_failure(app: &HeadlessApp) -> Option<String> {
    app.core().window.notices().open_failure()
}

/// The notice as the window draws it.
fn shown(app: &HeadlessApp) -> Option<String> {
    app.reader().selection_notice.map(|notice| notice.text)
}

fn person(app: &HeadlessApp, line: &str) {
    app.person(line);
    app.settle();
}

/// Failure (R3.7, S2b, a guard before the notice and the open failure move into `Window`): a notice that isn't drawn, or that stays where it always went: on a
/// change of tab (a save's note read as about the next document), on the reader dismissing it, or on a rotate that worked; or an open's failure that isn't
/// drawn, or stays after the next open worked.
#[test]
fn a_notice_and_an_open_failure_are_said_and_go_as_they_did() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 3), disk.pdf("b.pdf", 3));
    let broken = disk.file("broken.pdf", b"not a pdf at all");
    let app = disk.start();
    app.open(&a);
    app.open(&b);
    let no_such_page = Some(crate::linking::no_such_page("zz"));
    app.act(UiAction::PageTyped { text: "zz".into() });
    app.settle();
    assert_eq!(notice(&app), no_such_page, "said");
    assert_eq!(shown(&app), no_such_page, "and drawn");
    person(&app, r##"{"cmd":"tap","tab":"#1"}"##);
    assert_eq!((notice(&app), shown(&app)), (None, None), "gone with a change of tab");
    person(&app, r##"{"cmd":"page_rotate","tab":"#1","pages":"99","degrees":90}"##);
    let refused = notice(&app).expect("a rotate that couldn't be done says why");
    assert!(refused.starts_with("Couldn't rotate: "), "{refused}");
    assert_eq!(shown(&app).as_ref(), Some(&refused));
    person(&app, r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##);
    assert_eq!((notice(&app), shown(&app)), (None, None), "gone once a rotate worked");
    app.act(UiAction::PageTyped { text: "zz".into() });
    app.settle();
    assert_eq!(notice(&app), no_such_page);
    app.core().dismiss_selection_notice();
    app.settle();
    assert_eq!((notice(&app), shown(&app)), (None, None), "gone when dismissed");
    app.open(&broken);
    app.settle();
    let failed = open_failure(&app).expect("an open that failed says why");
    assert_eq!(app.reader().note.as_ref(), Some(&failed), "drawn by the window");
    app.open(&a);
    app.settle();
    assert_eq!(open_failure(&app), None, "gone with the next open");
    assert_eq!(app.reader().note, None);
}

/// Failure (R3.7, S2b, a guard before the panel's toggle moves into `Window`: the controls' golden holds only that the model was said to change): the panel's
/// row says it changed but doesn't toggle it, or the record doesn't keep it.
#[test]
fn the_panels_row_toggles_it_and_the_record_keeps_it() {
    let app = HeadlessApp::start();
    for open in [true, false, true] {
        app.act(UiAction::Menu { id: "panel.toggle".into() });
        app.settle();
        assert_eq!(app.core().window.panel_open(), open);
        assert_eq!(app.core().window.app_record(|| None).panel_open, open, "as the record has it");
    }
}
