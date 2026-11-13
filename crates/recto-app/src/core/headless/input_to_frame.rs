//! **Input to frame, measured** (step R2.3): how long a person's action takes from the window's call to what the frame is made from being published, on the
//! headless app, for the actions on the hot path. Not a check: it asserts nothing about time, which is one machine's, and runs only when asked, on a release build:
//!
//! `tools/sysguard/heavy.sh cargo nextest run --release --locked -p recto-app --run-ignored only --no-capture -E 'test(=core::headless::input_to_frame::input_to_frame)'`
//!
//! Each case times the window's own call (the UI thread's part: it must never wait) and then a flush of the session thread's queue, which returns once the call
//! and the look after it are done: what the frame waits for. The window's calls as it makes them, `UiAction`s (which before C4 were the bridge's own methods, the
//! same calls), before and after R2.3 moves a person's actions onto commands, so the runs compare.
//!
//! **What two builds' numbers can't tell apart:** this runs in `recto-app`'s test binary with every other test of the crate, so a build whose app code is the
//! same but whose tests aren't lays the code out differently, and that alone moves the 1,000-tab medians by up to 10 µs (2026-11-13: the first slice's
//! "unexplained 5 to 10 µs" on contents jump, typed page and Find typing was this, bisected to a commit and then reproduced with only its test files added
//! to the commit before: 46.0 to 55.0 µs typed page, the app's code byte for byte the same). A difference that size between builds is the layout, not the app.

use crate::RowKind;
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};
use std::time::{Duration, Instant};

/// How many times each case is timed.
const RUNS: usize = 500;

/// The window's call, and then the wait for what it made to be published.
fn timed(app: &HeadlessApp, call: impl FnOnce(&HeadlessApp)) -> (Duration, Duration) {
    let began = Instant::now();
    call(app);
    let called = began.elapsed();
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took the call");
    (called, began.elapsed())
}

fn micros(sorted: &[Duration], at: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * at).round() as usize;
    sorted[index].as_secs_f64() * 1e6
}

/// One line of the table: the case, the tabs open, the window's call at p99, and call-to-published at p50, p95 and p99, in microseconds.
fn report(case: &str, tabs: usize, mut called: Vec<Duration>, mut published: Vec<Duration>) {
    called.sort_unstable();
    published.sort_unstable();
    eprintln!(
        "{case:<14} {tabs:>5} tabs  ui p99 {:>8.1}  published p50 {:>8.1}  p95 {:>8.1}  p99 {:>8.1}",
        micros(&called, 0.99),
        micros(&published, 0.5),
        micros(&published, 0.95),
        micros(&published, 0.99)
    );
}

/// Times one case, alternating between two forms of it so each run changes something, with `between` run untimed before each.
fn case(app: &HeadlessApp, name: &str, tabs: usize, between: impl Fn(&HeadlessApp), call: impl Fn(&HeadlessApp, bool)) {
    let (mut called, mut published) = (Vec::with_capacity(RUNS), Vec::with_capacity(RUNS));
    for run in 0..RUNS {
        between(app);
        let (ui, done) = timed(app, |app| call(app, run % 2 == 0));
        called.push(ui);
        published.push(done);
    }
    app.settle();
    report(name, tabs, called, published);
}

fn measure(tabs: usize) {
    let disk = Disk::new();
    let files: Vec<_> = (0..tabs).map(|at| disk.pdf(&format!("f{at:04}.pdf"), 10)).collect();
    let app = disk.start();
    // One at a time: a few files are read at once, and past that one doesn't open.
    for file in &files {
        app.open(file);
    }
    let rows = app.tabs();
    assert_eq!(rows.len(), tabs, "every file has its tab");
    // The last two opened, which are warm.
    let (a, b) = (rows[rows.len() - 1].clone(), rows[rows.len().saturating_sub(2)].clone());
    let nothing = |_: &HeadlessApp| {};
    case(&app, "tap row", tabs, nothing, |app, first| {
        app.act(UiAction::Row {
            row: if first { b.clone() } else { a.clone() },
            adding: false,
        });
    });
    // A file that's open already, opened again, as the Open panel, a drop or the Finder does: its tab comes to the front, with nothing read.
    let (file_a, file_b) = (files[files.len() - 1].clone(), files[files.len().saturating_sub(2)].clone());
    case(&app, "open its tab", tabs, nothing, |app, first| {
        app.act(UiAction::Open {
            path: if first { file_b.clone() } else { file_a.clone() },
        });
    });
    app.act(UiAction::Row { row: a.clone(), adding: false });
    app.settle();
    let contents = |page: u32| crate::SidebarRow {
        kind: RowKind::Contents,
        page: Some(page),
        ..a.clone()
    };
    case(&app, "contents jump", tabs, nothing, |app, first| {
        app.act(UiAction::Row {
            row: contents(if first { 7 } else { 3 }),
            adding: false,
        });
    });
    // A click on a link to a page of this document, as the page view reports the hit: here, a link on the first page.
    case(&app, "link click", tabs, nothing, |app, first| {
        let inner = app.core();
        let engine = inner.panes.reading_engine(&inner.engine);
        let hit = recto_reader::links::LinkHit {
            page: 0,
            rect: [72.0, 112.0, 300.0, 128.0],
            target: recto_core::nav::LinkTarget::Page {
                page: if first { 8 } else { 4 },
                within: Some(0.3),
            },
        };
        crate::core::follow_link(inner, 0, &engine, hit);
    });
    case(&app, "typed page", tabs, nothing, |app, first| {
        app.act(UiAction::PageTyped {
            text: (if first { "5" } else { "2" }).into(),
        });
    });
    case(&app, "find typed", tabs, nothing, |app, first| {
        app.act(UiAction::FindTyped {
            text: (if first { "page 1" } else { "page" }).into(),
        });
    });
    // ⌘G and ⇧⌘G through what was typed last, its hits all in.
    app.act(UiAction::FindTyped { text: "page".into() });
    app.settle();
    case(&app, "find step", tabs, nothing, |app, first| {
        app.act(UiAction::Menu {
            id: (if first { "find.next" } else { "find.previous" }).into(),
        });
    });
    app.act(UiAction::FindClose);
    app.settle();
    // ⌘-click on the row of a warm tab, b, with a in front: b joins, side by side, and leaves again, a alone.
    case(&app, "compare click", tabs, nothing, |app, _| {
        app.act(UiAction::Row { row: b.clone(), adding: true });
    });
    // With a and b side by side, the reader moving from one pane into the other, as the pointer or a scroll reports it.
    app.act(UiAction::Row { row: b.clone(), adding: true });
    app.settle();
    case(&app, "pane focus", tabs, nothing, |app, first| {
        let tab = recto_session::TabId(if first { a.tab } else { b.tab });
        app.core().core.push(crate::queue::CoreCall::FocusPane(tab));
    });
    app.act(UiAction::Row { row: a.clone(), adding: false });
    app.settle();
    case(&app, "zoom", tabs, nothing, |app, first| {
        app.act(UiAction::Menu {
            id: (if first { "view.zoom_in" } else { "view.zoom_out" }).into(),
        });
    });
    // The panel's Rotate, every page as one step, as outside edit mode.
    case(&app, "rotate", tabs, nothing, |app, first| {
        app.act(UiAction::Menu {
            id: (if first { "rotate.right" } else { "rotate.left" }).into(),
        });
    });
    let rotate = |app: &HeadlessApp| {
        app.act(UiAction::Menu { id: "rotate.right".into() });
        assert!(app.core().core.flush(WAIT_LIMIT));
    };
    case(&app, "undo", tabs, rotate, |app, _| {
        app.act(UiAction::Menu { id: "edit.undo".into() });
    });
    // A file that isn't open, opened as the Open panel does: read off the session thread, and its tab made. A flush waits for the reads before it, so this
    // is the read too. Closed again, untimed, before each; the first close is of the edited tab in front, and is refused.
    let new = app.disk().pdf("new.pdf", 10);
    let close_it = |app: &HeadlessApp| {
        let _ = app.agent(recto_agent::Level::Full, r#"{"cmd":"close"}"#);
        app.settle();
    };
    case(&app, "open new file", tabs, close_it, |app, _| {
        app.act(UiAction::Open { path: new.clone() });
    });
    drop(app.quit());
}

/// Measures, and asserts nothing about time: see the module's words.
#[test]
#[ignore = "a measurement, run on a release build when asked"]
fn input_to_frame() {
    for tabs in [1, 100, 1000] {
        measure(tabs);
    }
}
