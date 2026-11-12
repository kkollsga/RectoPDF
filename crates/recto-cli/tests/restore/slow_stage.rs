//! **The slow stage's own state tests** (`tools/check.sh`'s `slow` stage, docs/engineering/slow-mode.md). The whole-state tests of the normal run set up a very rich state with a settle after almost every step; with every worker answer late, each settle costs seconds, and two such tests were 117 s and 70 s of a 148 s stage. What the stage is there for is the late paths at a launch (the state read late, the window made again from it, the session thread late) and at a hand-off, and those are crossed once per launch, however the state was set up. So the stage runs this lean setup: the same state, every part of it, but the window-level parts set on the empty window (nothing to draw yet) and a settle only where a later step needs what the earlier one drew.

use super::{Run, assert_same, chaptered, known_paths, long_document};
use crate::common::State;
use crate::common::app::wait_for_long;
use recto_testpdf::TempPdf;

/// The state of `make_rich` (restore.rs), set in the order that costs the least under late workers: size, full screen, sidebar, panel, paper, Settings, Find, focus and edit mode on the empty window; then the three tabs with their places, zoom, layouts, folds, edits with undo history by both the owner and the agent, and the comparison; the rotation last. Every field `make_rich` sets is set here, and the comparison covers all of `qa state`.
pub(crate) fn make_rich_lean(run: &Run<'_>, files: &[&TempPdf]) {
    run.ok(&["view", "--paper", "warm", "--agent-icon", "off"]);
    run.ok(&["settings", "--auto-update", "off"]);
    run.ok(&["qa", "settings", "open"]);
    run.ok(&["qa", "frontmost", "off"]);
    for file in files {
        run.ok(&["open", file.0.to_str().unwrap()]);
    }
    run.settled();
    // The first tab: far along, zoomed in, scrolled, with edits by both and a way back.
    run.ok(&["tap", "#1"]);
    run.ok(&["goto", "40"]);
    run.ok(&["goto", "20"]);
    run.ok(&["back"]);
    run.ok(&["qa", "zoom", "1.7"]);
    run.ok(&["qa", "scroll", "137"]);
    run.ok(&["qa", "owner-edit", "rotate:2"]);
    run.ok(&["page", "delete", "5"]);
    run.ok(&["page", "move", "6", "--to", "2"]);
    run.ok(&["page", "insert", files[1].0.to_str().unwrap(), "--at", "3"]);
    run.ok(&["qa", "owner-edit", "delete:9"]);
    run.ok(&["page", "rotate", "3", "--by", "90"]);
    run.ok(&["undo"]);
    run.ok(&["qa", "owner-edit", "undo"]);
    // The second: another zoom, a layout and folded contents.
    run.ok(&["tap", "#2"]);
    run.ok(&["view", "--layout", "two-up-cover"]);
    run.ok(&["goto", "11"]);
    run.ok(&["qa", "zoom", "0.8"]);
    run.ok(&["qa", "scroll", "60"]);
    run.ok(&["qa", "click-row", "1"]);
    // The third, which stays active, side by side, then compared with the second, synced, each pane in its place.
    run.ok(&["tap", "#3"]);
    run.ok(&["view", "--layout", "two-up"]);
    run.ok(&["goto", "120"]);
    // The contents' scroll by the reader needs the contents laid out, which is drawn: settled before it, as it is in `make_rich`.
    run.settled();
    run.ok(&["qa", "contents-scroll", "60"]);
    run.ok(&["qa", "scroll", "42"]);
    run.settled();
    run.ok(&["compare", "--tabs", "#2,#3", "--sync", "on"]);
    run.settled();
    run.ok(&["qa", "pane-scroll", "0:120"]);
    run.settled();
    run.ok(&["qa", "pane-focus", "1"]);
    run.ok(&["qa", "resize", "1300x820"]);
    run.ok(&["qa", "sidebar-width", "318"]);
    run.ok(&["qa", "panel", "toggle"]);
    run.ok(&["qa", "full-screen", "on"]);
    run.ok(&["qa", "find", "open"]);
    run.ok(&["qa", "find-query", "ran this"]);
    run.ok(&["qa", "find-draft", "typed but not run"]);
    run.ok(&["qa", "focus", "find"]);
    run.ok(&["qa", "edit-mode", "on"]);
    run.ok(&["qa", "view-rotation", "1"]);
    run.settled();
}

/// Failure (the stage's version of `everything_comes_back_after_a_quit`; CI, main at f4b4d22 and 2026-11-11's late-state fixes): anything the user can see or has changed doesn't survive a quit and a launch when **every waiting point is late**: the state read 1.5 s late so the window goes up without it and is made again from it, the session thread late after it, the socket's first answer late. The whole of `qa state` compared across the quit, as the normal test does, from a state set up the lean way and in dark.
#[test]
fn everything_comes_back_after_a_quit_when_every_wait_is_late() {
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let (a, b, c) = (long_document("alpha", 60), long_document("beta", 30), chaptered("gamma", 20, 8));
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.ok(&["qa", "appearance", "dark"]);
    make_rich_lean(&run, &[&a, &b, &c]);
    run.ok(&["view", "--paper", "dark"]);
    run.settled();
    // The first tab's edits are journalled a little after they are made, not at once (fewer writes to the disk); the long setup of `make_rich` gives them that time, this one waits for it.
    wait_for_long(&run, |report| report["session"]["tabs"][0]["journal"].is_string());
    let before = run.report();
    run.quit();
    let run = Run::on_with_env(&state, &[]).unwrap();
    let tabs = before["session"]["tabs"].as_array().unwrap().len();
    wait_for_long(&run, |report| report["session"]["tabs"].as_array().is_some_and(|now| now.len() == tabs));
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let after = run.report();
    assert_same(&before, &after, &known_paths());
    for field in ["scroll_y", "scroll_x", "magnification"] {
        let (was, now) = (before["app"]["pages"][field].as_f64().unwrap(), after["app"]["pages"][field].as_f64().unwrap());
        assert!((was - now).abs() < 0.25, "{field}: {was} then {now}");
    }
    assert_eq!(after["session"]["find"]["draft"], "typed but not run");
    assert_eq!(after["session"]["app"]["focus"]["view"], "find");
    run.ok(&["qa", "owner-edit", "redo"]);
    run.quit();
}
