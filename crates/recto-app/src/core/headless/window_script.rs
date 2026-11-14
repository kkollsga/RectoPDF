//! **What the window's own state does, step by step, pinned before it moves** (R3.7, S2; AGENTS.md, "guards go in before a move"): a fixed script of what the
//! host tells Rust of its window (opened, made again, closed, in the Dock, hidden, moved, full screen, the sidebar's width, the contents' scroll, focus,
//! appearance, the app in front, the Settings window), and after each step what the host was told, in order, what it was answered, what the record's writer
//! was asked for (to write at once, in its own time, or nothing), and the host's part of the record as it would be written
//! ([`crate::core::Window::app_record`], with what's been decided for a launch). Held against `golden/window.txt`. A launch an update reopened, a quiet run's
//! placement and a window made again from a state read late (`from_state`) are steps of their own.
//!
//! The steps the bridge worked out itself were first done here by a copy of the bridge's bodies at `main` on 2026-11-13 (the guard's commit); as each moved
//! into Rust its copy went and the step called what the bridge then calls, with the golden unchanged.

use super::coverage::heard_kind;
use crate::core::Inner;
use crate::lock;
use crate::records::ScreenInfo;
use crate::testkit::headless::{Disk, HeadlessApp};

/// A screen the window is on, with a name that needs cleaning.
fn screen() -> ScreenInfo {
    ScreenInfo {
        id: 7,
        name: "Studio\u{7}  Display".into(),
        x: 0.0,
        y: 0.0,
        width: 2560.0,
        height: 1440.0,
        visible_x: 0.0,
        visible_y: 0.0,
        visible_width: 2560.0,
        visible_height: 1415.0,
    }
}

/// A step of the script: what it does, and what the host is answered, if anything.
type Step = fn(&Inner) -> Option<String>;

/// A step that answers nothing.
fn done((): ()) -> Option<String> {
    None
}

/// A step's yes or no.
#[allow(clippy::unnecessary_wraps)]
fn said(answer: bool) -> Option<String> {
    Some(answer.to_string())
}

/// **The script:** each step's name and what it does. A step that answers nothing is its call passed to [`done`], a line a step.
#[allow(clippy::unit_arg)]
const STEPS: &[(&str, Step)] = &[
    // No window's been told yet: the first change makes one, at the default frame.
    ("full screen with no frame", |inner| done(inner.window.changed(None, None, true, &inner.saver))),
    ("moved", |inner| {
        done(inner.window.changed(Some(vec![10.0, 20.0, 800.0, 600.0]), Some(screen()), false, &inner.saver))
    }),
    ("a frame of three numbers", |inner| {
        done(inner.window.changed(Some(vec![1.0, 2.0, 3.0]), Some(screen()), false, &inner.saver))
    }),
    ("into the Dock", |inner| done(inner.window.minimised_changed(true, &inner.saver))),
    ("placed at a launch", |inner| {
        placed(inner.window.placement(Vec::new(), inner.updates.reopened_by_update(), false))
    }),
    ("placed in a quiet run", |inner| placed(inner.window.placement(vec![screen()], false, true))),
    ("into the Dock again", |inner| done(inner.window.minimised_changed(true, &inner.saver))),
    ("placed at a launch an update reopened", |inner| {
        placed(inner.window.placement(vec![screen()], true, false))
    }),
    ("closed", |inner| done(inner.window_closed())),
    ("should open", |inner| said(inner.window.should_open(inner.updates.reopened_by_update()))),
    ("should open at a launch an update reopened", |inner| said(inner.window.should_open(true))),
    ("opened", |inner| done(inner.window_opened())),
    ("out of the Dock with no window", |inner| {
        done(inner.window.minimised_changed(false, &inner.saver))
    }),
    ("hidden", |inner| done(inner.window.hidden_changed(true, &inner.saver))),
    ("comes back hidden", |inner| {
        said(inner.window.comes_back_hidden(inner.updates.reopened_by_update()))
    }),
    ("shown", |inner| done(inner.window.hidden_changed(false, &inner.saver))),
    ("hidden again", |inner| done(inner.window.hidden_changed(true, &inner.saver))),
    ("comes back hidden at a launch an update reopened", |inner| {
        said(inner.window.comes_back_hidden(true))
    }),
    ("shown again", |inner| done(inner.window.hidden_changed(false, &inner.saver))),
    ("sidebar too narrow", |inner| done(inner.window.set_sidebar_width(10.0, &inner.core))),
    ("sidebar wide", |inner| done(inner.window.set_sidebar_width(300.4, &inner.core))),
    ("contents nowhere", |inner| done(inner.window.contents_position(f64::NAN, &inner.saver))),
    ("contents scrolled", |inner| done(inner.window.contents_position(120.5, &inner.saver))),
    ("focus on the sidebar", |inner| {
        inner.window.focus_changed("sidebar", &inner.saver);
        Some(inner.window.focus_view().to_string())
    }),
    ("focus on nothing known", |inner| {
        inner.window.focus_changed("the moon", &inner.saver);
        Some(inner.window.focus_view().to_string())
    }),
    ("dark chosen", |inner| {
        inner.window.set_appearance_choice("dark", &inner.saver);
        Some(inner.window.appearance_choice().to_string())
    }),
    ("light chosen", |inner| {
        inner.window.set_appearance_choice("light", &inner.saver);
        Some(inner.window.appearance_choice().to_string())
    }),
    ("nothing known chosen", |inner| {
        inner.window.set_appearance_choice("plaid", &inner.saver);
        Some(inner.window.appearance_choice().to_string())
    }),
    ("the system went dark", |inner| done(inner.window.set_dark(true, &inner.core))),
    ("to the background", |inner| {
        inner.window.frontmost_changed(false, false, &inner.saver, &inner.core);
        said(inner.window.opens_in_background(inner.updates.reopened_by_update()))
    }),
    ("opens behind at a launch an update reopened", |inner| {
        said(inner.window.opens_in_background(true))
    }),
    ("to the front", |inner| {
        inner.window.frontmost_changed(true, true, &inner.saver, &inner.core);
        said(inner.window.opens_in_background(inner.updates.reopened_by_update()))
    }),
    ("contents scrolled by the person", |inner| {
        inner.window.contents_user_scrolled(inner.life.seconds_since_start());
        said(inner.window.contents_following())
    }),
    ("settings opened", |inner| {
        inner
            .window
            .settings_window_changed(Some(recto_session::record::SettingsWindow { x: 40.0, y: 50.0 }), &inner.saver);
        Some(format!("{:?}", inner.window.settings_window()))
    }),
    ("settings closed", |inner| {
        inner.window.settings_window_changed(None, &inner.saver);
        Some(format!("{:?}", inner.window.settings_window()))
    }),
    ("made again", |inner| {
        inner.window_replaced();
        done(inner.window_opened())
    }),
    ("made again from a state read late", |inner| {
        {
            // What the state said, taken in after this window went up: its frame, width and look are the new window's; what the host chose of what's out
            // of sight (hidden, closed, the Dock) since is kept.
            let mut app = inner.window.state_for_tests();
            let mut state = app.clone();
            state.sidebar_width = 333.0;
            state.appearance = recto_session::record::Appearance::Dark;
            state.app_hidden = true;
            state.window_closed = true;
            if let Some(window) = state.window.as_mut() {
                window.frame.x = 99.0;
                window.minimised = true;
            }
            app.from_state = Some(Box::new(state));
        }
        inner.window_replaced();
        done(inner.window_opened())
    }),
];

/// A placement's whole answer.
fn placed(placement: crate::core::WindowPlacement) -> Option<String> {
    Some(format!("{placement:?}"))
}

/// What the window's state is after a step, flattened to `path value` pairs: the host's part of the record, what's been decided for a launch, and the
/// system's appearance as the host said it.
fn state(inner: &Inner) -> std::collections::BTreeMap<String, String> {
    let mut state = std::collections::BTreeMap::new();
    flatten(
        "record",
        &serde_json::to_value(inner.window.app_record(|| lock(&inner.host.notice).clone())).expect("the record as JSON"),
        &mut state,
    );
    let app = inner.window.state_for_tests();
    for (name, value) in [
        ("decided.hidden", app.hidden_decided),
        ("decided.minimised", app.minimised_decided),
        ("decided.window", app.window_decided),
        ("dark", inner.window.is_dark()),
    ] {
        state.insert(name.to_string(), value.to_string());
    }
    state
}

fn flatten(path: &str, value: &serde_json::Value, out: &mut std::collections::BTreeMap<String, String>) {
    match value {
        serde_json::Value::Object(map) if !map.is_empty() => {
            for (key, value) in map {
                flatten(&format!("{path}.{key}"), value, out);
            }
        }
        other => {
            out.insert(path.to_string(), other.to_string());
        }
    }
}

/// What changed from one state to the next, `path=value` each, or `nothing`.
fn changes(was: &std::collections::BTreeMap<String, String>, now: &std::collections::BTreeMap<String, String>) -> String {
    let changed: Vec<String> = now
        .iter()
        .filter(|(path, value)| was.get(*path) != Some(*value))
        .map(|(path, value)| format!("{path}={value}"))
        .chain(was.keys().filter(|path| !now.contains_key(*path)).map(|path| format!("{path} gone")))
        .collect();
    if changed.is_empty() { "nothing".to_string() } else { changed.join(" ") }
}

/// The script run on an app with a file open, a line a step: what the host was told, what it was answered, and what of the window's state changed.
fn script_lines() -> Vec<String> {
    let disk = Disk::new();
    let file = disk.pdf("a.pdf", 3);
    let app: HeadlessApp = disk.start();
    app.open(&file);
    app.settle();
    drop(app.listener().take());
    let mut was = state(app.core());
    let mut lines = vec![format!("launched | {}", changes(&std::collections::BTreeMap::new(), &was))];
    for (name, step) in STEPS {
        // The session thread held through the step, so what the writer was asked for is the step's: nothing else asks, and no writer takes it.
        let held = app.hold_session_thread();
        app.core().saver.forget_asked();
        let answer = step(app.core());
        let writes = app.core().saver.asked();
        drop(held);
        app.settle();
        let heard: Vec<&str> = app.listener().take().iter().map(heard_kind).collect();
        let heard = if heard.is_empty() { "nothing".to_string() } else { heard.join(" ") };
        let answer = answer.map_or_else(String::new, |answer| format!(" | answer {answer}"));
        let now = state(app.core());
        lines.push(format!("{name} | heard {heard}{answer} | writes {writes} | changed {}", changes(&was, &now)));
        was = now;
    }
    lines
}

/// Failure (R3.7, S2, a guard before the window's state moves out of `Inner` and its rules out of the bridge): a step the window tells Rust of keeps
/// something else in the record (the default frame, a screen's name uncleaned, a sidebar width past its limits, a scroll that isn't a number, a focus or
/// appearance that isn't one), answers otherwise, or tells the host otherwise or in another order. Each step's line is held against the golden, naming the
/// first that differs.
#[test]
fn the_windows_state_goes_as_it_did_step_by_step() {
    let now = script_lines();
    let golden: Vec<&str> = include_str!("golden/window.txt").lines().collect();
    if golden.iter().copied().eq(now.iter().map(String::as_str)) {
        return;
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/recto-golden");
    let _ = std::fs::create_dir_all(&out);
    let written = out.join("window.txt");
    let _ = std::fs::write(&written, now.join("\n") + "\n");
    let first = golden.iter().zip(&now).find(|(was, is)| **was != is.as_str()).map_or_else(
        || format!("{} lines, the golden {}", now.len(), golden.len()),
        |(was, is)| format!("golden: {was}\nnow:    {is}"),
    );
    panic!("the window's state went otherwise:\n{first}\n(the whole script now: {})", written.display());
}
