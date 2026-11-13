//! **What the window's own state does, step by step, pinned before it moves** (R3.7, S2; AGENTS.md, "guards go in before a move"): a fixed script of what the
//! host tells Rust of its window (opened, made again, closed, in the Dock, hidden, moved, full screen, the sidebar's width, the contents' scroll, focus,
//! appearance, the app in front, the Settings window), and after each step what the host was told, in order, what it was answered, and the host's part of
//! the record as it would be written ([`crate::core::Window::app_record`], with what's been decided for a launch). Held against `golden/window.txt`.
//!
//! The steps the bridge works out itself today are done here by [`bridge`], a copy of the bridge's bodies at `main` on 2026-11-13; as each moves into Rust
//! its copy goes and the step calls what the bridge then calls, and the golden doesn't change.

use super::coverage::heard_kind;
use crate::core::Inner;
use crate::lock;
use crate::records::ScreenInfo;
use crate::testkit::headless::{Disk, HeadlessApp};

/// The bridge's bodies for what it works out itself, copied from `recto-ffi/src/lib.rs` as they were, with `self.inner` for `inner`.
mod bridge {
    use crate::core::{Inner, updates};
    use crate::lock;
    use crate::records::ScreenInfo;
    use recto_session::record::{self, Appearance, FocusView, Rect, ScreenRecord, WindowRecord};

    pub fn window_minimised_changed(inner: &Inner, minimised: bool) {
        if let Some(window) = lock(&inner.window.app).window.as_mut() {
            window.minimised = minimised;
        }
        inner.saver.view_changed();
    }

    pub fn app_hidden_changed(inner: &Inner, hidden: bool) {
        lock(&inner.window.app).app_hidden = hidden;
        if hidden {
            inner.saver.write_soon();
        } else {
            inner.saver.view_changed();
        }
    }

    pub fn app_comes_back_hidden(inner: &Inner) -> bool {
        let mut app = lock(&inner.window.app);
        let hidden = updates::stays_out_of_sight(app.app_hidden, inner.updates.reopened_by_update());
        app.app_hidden = hidden;
        app.hidden_decided = true;
        hidden
    }

    /// The part of `window_placement` that decides what's kept: whether the window comes back in the Dock, and that it's been decided.
    pub fn window_placement_decides(inner: &Inner) -> bool {
        let app = {
            let mut app = lock(&inner.window.app);
            let arrived = inner.updates.reopened_by_update();
            if let Some(window) = app.window.as_mut() {
                window.minimised = updates::stays_out_of_sight(window.minimised, arrived);
            }
            app.minimised_decided = true;
            app.clone()
        };
        app.window.as_ref().is_some_and(|window| window.minimised)
    }

    pub fn window_changed(inner: &Inner, frame: Option<Vec<f64>>, screen: Option<ScreenInfo>, full_screen: bool) {
        let mut app = lock(&inner.window.app);
        let saved = app.window.get_or_insert(WindowRecord {
            frame: Rect {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            screen: None,
            full_screen: false,
            minimised: false,
        });
        if let Some([x, y, width, height]) = frame.as_deref() {
            saved.frame = Rect {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
            };
            saved.screen = screen.map(|screen| ScreenRecord {
                id: screen.id,
                name: recto_core::clean_line(&screen.name),
                frame: Rect {
                    x: screen.x,
                    y: screen.y,
                    width: screen.width,
                    height: screen.height,
                },
            });
        }
        saved.full_screen = full_screen;
        drop(app);
        inner.saver.view_changed();
    }

    pub fn set_sidebar_width(inner: &Inner, width: f64) {
        lock(&inner.window.app).sidebar_width = record::sidebar_width(width);
        inner.core.push(crate::queue::CoreCall::Reconcile);
    }

    pub fn contents_position(inner: &Inner, y: f64) {
        lock(&inner.window.app).contents_y = y.is_finite().then_some(y);
        inner.saver.view_changed();
    }

    pub fn focus_changed(inner: &Inner, view: String) {
        if let Some(view) = FocusView::from_name(&view) {
            lock(&inner.window.app).focus.view = view;
            inner.saver.view_changed();
        }
    }

    pub fn focus_view(inner: &Inner) -> String {
        lock(&inner.window.app).focus.view.name().to_string()
    }

    pub fn appearance_choice(inner: &Inner) -> String {
        match lock(&inner.window.app).appearance {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
        .to_string()
    }

    pub fn set_appearance_choice(inner: &Inner, choice: String) {
        lock(&inner.window.app).appearance = match choice.as_str() {
            "light" => Appearance::Light,
            "dark" => Appearance::Dark,
            _ => Appearance::System,
        };
        inner.saver.view_changed();
    }

    pub fn set_dark_appearance(inner: &Inner, dark: bool) {
        inner.window.dark.store(dark, std::sync::atomic::Ordering::SeqCst);
        inner.core.push(crate::queue::CoreCall::Reconcile);
    }

    pub fn contents_user_scrolled(inner: &Inner) {
        lock(&inner.window.contents_follow).user_scrolled(inner.life.started.elapsed().as_secs_f64());
    }

    pub fn contents_is_following(inner: &Inner) -> bool {
        lock(&inner.window.contents_follow).is_following()
    }

    pub fn opens_in_background(inner: &Inner) -> bool {
        updates::opens_in_background(inner.updates.reopened_by_update(), lock(&inner.window.app).focus.app_frontmost)
    }
}

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
    ("full screen with no frame", |inner| done(bridge::window_changed(inner, None, None, true))),
    ("moved", |inner| {
        done(bridge::window_changed(inner, Some(vec![10.0, 20.0, 800.0, 600.0]), Some(screen()), false))
    }),
    ("a frame of three numbers", |inner| {
        done(bridge::window_changed(inner, Some(vec![1.0, 2.0, 3.0]), Some(screen()), false))
    }),
    ("into the Dock", |inner| done(bridge::window_minimised_changed(inner, true))),
    ("placed at a launch", |inner| said(bridge::window_placement_decides(inner))),
    ("into the Dock again", |inner| done(bridge::window_minimised_changed(inner, true))),
    ("closed", |inner| done(inner.window_closed())),
    ("should open", |inner| said(inner.window.should_open(inner.updates.reopened_by_update()))),
    ("opened", |inner| done(inner.window_opened())),
    ("out of the Dock with no window", |inner| done(bridge::window_minimised_changed(inner, false))),
    ("hidden", |inner| done(bridge::app_hidden_changed(inner, true))),
    ("comes back hidden", |inner| said(bridge::app_comes_back_hidden(inner))),
    ("shown", |inner| done(bridge::app_hidden_changed(inner, false))),
    ("sidebar too narrow", |inner| done(bridge::set_sidebar_width(inner, 10.0))),
    ("sidebar wide", |inner| done(bridge::set_sidebar_width(inner, 300.4))),
    ("contents nowhere", |inner| done(bridge::contents_position(inner, f64::NAN))),
    ("contents scrolled", |inner| done(bridge::contents_position(inner, 120.5))),
    ("focus on the sidebar", |inner| {
        bridge::focus_changed(inner, "sidebar".into());
        Some(bridge::focus_view(inner))
    }),
    ("focus on nothing known", |inner| {
        bridge::focus_changed(inner, "the moon".into());
        Some(bridge::focus_view(inner))
    }),
    ("dark chosen", |inner| {
        bridge::set_appearance_choice(inner, "dark".into());
        Some(bridge::appearance_choice(inner))
    }),
    ("light chosen", |inner| {
        bridge::set_appearance_choice(inner, "light".into());
        Some(bridge::appearance_choice(inner))
    }),
    ("nothing known chosen", |inner| {
        bridge::set_appearance_choice(inner, "plaid".into());
        Some(bridge::appearance_choice(inner))
    }),
    ("the system went dark", |inner| done(bridge::set_dark_appearance(inner, true))),
    ("to the background", |inner| {
        inner.window.frontmost_changed(false, false, &inner.saver, &inner.core);
        said(bridge::opens_in_background(inner))
    }),
    ("to the front", |inner| {
        inner.window.frontmost_changed(true, true, &inner.saver, &inner.core);
        said(bridge::opens_in_background(inner))
    }),
    ("contents scrolled by the person", |inner| {
        bridge::contents_user_scrolled(inner);
        said(bridge::contents_is_following(inner))
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
];

/// What the window's state is after a step, flattened to `path value` pairs: the host's part of the record, what's been decided for a launch, and the
/// system's appearance as the host said it.
fn state(inner: &Inner) -> std::collections::BTreeMap<String, String> {
    let mut state = std::collections::BTreeMap::new();
    flatten(
        "record",
        &serde_json::to_value(inner.window.app_record(lock(&inner.host.notice).clone())).expect("the record as JSON"),
        &mut state,
    );
    let app = lock(&inner.window.app);
    for (name, value) in [
        ("decided.hidden", app.hidden_decided),
        ("decided.minimised", app.minimised_decided),
        ("decided.window", app.window_decided),
        ("dark", inner.window.dark.load(std::sync::atomic::Ordering::SeqCst)),
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
        let answer = step(app.core());
        app.settle();
        let heard: Vec<&str> = app.listener().take().iter().map(heard_kind).collect();
        let heard = if heard.is_empty() { "nothing".to_string() } else { heard.join(" ") };
        let answer = answer.map_or_else(String::new, |answer| format!(" | answer {answer}"));
        let now = state(app.core());
        lines.push(format!("{name} | heard {heard}{answer} | changed {}", changes(&was, &now)));
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
