//! The marks that come and go in the real app: the zoom level, the page number, and the window's buttons with the sidebar's. Their rule and timing are `recto-ffi`'s `chrome.rs`, unit-tested there; these check the app draws what it says, driven by the verbs that call what the events call.

use super::{Run, numbered};
use serde_json::Value;
use std::time::{Duration, Instant};

fn marks(run: &Run<'_>) -> Value {
    run.report()["app"]["chrome"].clone()
}

/// Waits, up to 10 s, for the marks to be as asked: longer than a mark's 3 s, which `wait_for`'s 4 s doesn't leave room for on a loaded Mac.
fn wait_marks(run: &Run<'_>, holds: impl Fn(&Value) -> bool) -> Value {
    let began = Instant::now();
    loop {
        let now = marks(run);
        if holds(&now) {
            return now;
        }
        assert!(began.elapsed() < Duration::from_secs(10), "they never came to that: {now}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Failure: the zoom level or the page number stays up for good, never comes back on a change, fades from under the pointer, or Rust says one thing and the window draws another. Owner, 2026-11-08: "Zoom level should auto hid 3s after last zoom change, and similar behaviour for page number". Opening shows both; they go by themselves; the reader's scroll brings the page number back and a zoom the zoom level, each alone; the pointer over the page number holds it with no timer; and what's drawn is what's said.
#[test]
fn the_zoom_level_and_the_page_number_fade_after_their_last_change() {
    let Some(run) = Run::start() else { return };
    // Drawn at once, so what's drawn can be compared with what's said without waiting out a fade.
    run.ok(&["qa", "reduce-motion", "on"]);
    let file = numbered(30);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let opened = marks(&run);
    assert_eq!(
        (opened["zoom_shown"].as_bool(), opened["place_shown"].as_bool()),
        (Some(true), Some(true)),
        "opening a document changes both: {opened}"
    );
    let next = opened["next_change_in_ms"].as_u64().unwrap();
    assert!(next > 0 && next <= 3_000, "they go within 3 s: {opened}");
    let gone = wait_marks(&run, |marks| marks["zoom_shown"] == false && marks["place_shown"] == false);
    assert_eq!(
        (gone["zoom_alpha"].as_f64(), gone["place_alpha"].as_f64()),
        (Some(0.0), Some(0.0)),
        "and are drawn gone: {gone}"
    );
    assert!(gone["next_change_in_ms"].is_null(), "with nothing left to wait for: {gone}");
    // The reader's own scroll brings the page number back, and only that.
    run.ok(&["qa", "scroll", "400"]);
    let scrolled = marks(&run);
    assert_eq!(
        (
            scrolled["place_shown"].as_bool(),
            scrolled["zoom_shown"].as_bool(),
            scrolled["place_alpha"].as_f64()
        ),
        (Some(true), Some(false), Some(1.0)),
        "{scrolled}"
    );
    // A zoom brings the zoom level back, at the change: looked at before the pages at the new zoom are drawn, which on a slow machine can take longer than the mark's 3 s (slow mode, 2026-11-11).
    run.ok(&["qa", "zoom", "1.5"]);
    let zoomed = marks(&run);
    assert_eq!(
        (zoomed["zoom_shown"].as_bool(), zoomed["zoom_alpha"].as_f64()),
        (Some(true), Some(1.0)),
        "{zoomed}"
    );
    run.settled();
    // The pointer over the page number holds it, with no timer for it, and its 3 s start when it leaves.
    wait_marks(&run, |marks| marks["place_shown"] == false && marks["zoom_shown"] == false);
    run.ok(&["qa", "corner-pointer", "lower-right:in"]);
    let over = marks(&run);
    assert_eq!(over["place_shown"], true, "back under the pointer: {over}");
    assert!(over["next_change_in_ms"].is_null(), "held there, with no time running out: {over}");
    run.ok(&["qa", "corner-pointer", "lower-right:out"]);
    let left = marks(&run);
    assert!(
        left["place_shown"] == true && left["next_change_in_ms"].as_u64().is_some_and(|ms| ms <= 3_000),
        "it goes 3 s after: {left}"
    );
    wait_marks(&run, |marks| marks["place_shown"] == false);
}

/// Failure: with the sidebar hidden the window's buttons and the sidebar's stay, or with it showing they go, or the pointer over their corner doesn't bring them back, so a window with the sidebar hidden can't be closed or get its sidebar back. Owner, 2026-11-08: "When sidebar is hidden, I want the traffic light buttons and sidebar toggle to autohide. To show them again you hover above them".
#[test]
fn the_window_buttons_go_with_the_sidebar_and_come_back_under_the_pointer() {
    let Some(run) = Run::start() else { return };
    run.ok(&["qa", "reduce-motion", "on"]);
    let file = numbered(3);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let shown = marks(&run);
    assert_eq!(
        (shown["window_buttons_shown"].as_bool(), shown["window_buttons_alpha"].as_f64()),
        (Some(true), Some(1.0)),
        "{shown}"
    );
    run.ok(&["qa", "button", "sidebar"]);
    run.settled();
    let hidden = marks(&run);
    assert_eq!(run.report()["app"]["sidebar_hidden"], true);
    assert_eq!(
        (hidden["window_buttons_shown"].as_bool(), hidden["window_buttons_alpha"].as_f64()),
        (Some(false), Some(0.0)),
        "{hidden}"
    );
    run.ok(&["qa", "corner-pointer", "top-left:in"]);
    let over = marks(&run);
    assert_eq!(
        (over["window_buttons_shown"].as_bool(), over["window_buttons_alpha"].as_f64()),
        (Some(true), Some(1.0)),
        "{over}"
    );
    run.ok(&["qa", "corner-pointer", "top-left:out"]);
    assert_eq!(marks(&run)["window_buttons_shown"], false);
    // The sidebar's button still works while it's faded, and brings them all back.
    run.ok(&["qa", "button", "sidebar"]);
    run.settled();
    let back = marks(&run);
    assert_eq!(
        (back["window_buttons_shown"].as_bool(), back["window_buttons_alpha"].as_f64()),
        (Some(true), Some(1.0)),
        "{back}"
    );
}
