//! Opening in the real app, held to rule 3: the UI thread never waits on a file.

use super::Run;
use crate::common::hang_rule::held_the_window;
use recto_testpdf::TempPdf;
use serde_json::Value;
use std::time::{Duration, Instant};

/// The File menu's Open Recent items, as `(id, title)`, newest first.
fn recent_items(state: &Value) -> Vec<String> {
    state["app"]["menu_bar"]
        .as_array()
        .unwrap()
        .iter()
        .find(|menu| menu["title"] == "File")
        .unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["id"].as_str().map(str::to_string))
        .filter(|id| id.starts_with("recent.") && id != "recent.clear")
        .collect()
}

/// Waits up to 60 s for `holds`, longer than the seconds a hostile file takes to read in a debug build under load.
fn wait_long(run: &Run<'_>, holds: impl Fn(&Value) -> bool) -> Value {
    let started = Instant::now();
    loop {
        let state = run.report();
        if holds(&state) {
            return state;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "it never came to pass: {state}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Failure: the native launch never binds its UI watchdog, so a startup stall is invisible even though later file-open work is watched.
fn assert_startup_was_watched(state: &Value) {
    let startup = &state["hangs"]["startup"];
    assert_eq!(startup["clock"].as_str(), Some("watchdog"), "startup uses another clock: {startup}");
    assert_eq!(startup["cap"].as_u64(), Some(16), "startup evidence lost its bound: {startup}");
    assert_eq!(
        startup["complete"].as_bool(),
        Some(true),
        "the launch was not watched through readiness: {startup}"
    );
    assert_eq!(startup["lost"].as_u64(), Some(0), "startup evidence is incomplete: {startup}");
    let points = startup["points"].as_array().unwrap_or_else(|| panic!("startup has no points: {startup}"));
    assert!(points.len() <= 16, "startup evidence exceeded its bound: {startup}");
    let mut last_at = None;
    for point in points {
        let at = point["at_us"].as_u64().unwrap_or_else(|| panic!("startup has an invalid timestamp: {startup}"));
        assert!(last_at.is_none_or(|previous| previous <= at), "startup timestamps went backwards: {startup}");
        last_at = Some(at);
        assert!(point["phase"].is_string(), "startup has an unnamed phase: {startup}");
        assert!(
            point.get("cpu_us").is_some_and(|cpu| cpu.is_null() || cpu.as_u64().is_some()),
            "startup has an invalid optional CPU reading: {startup}"
        );
    }
    let mut previous = None;
    for phase in [
        "launch_begin",
        "restore_queued",
        "restore_begin",
        "window_begin",
        "window_body",
        "window_constructed",
        "window_opened",
        "host_ready",
    ] {
        let index = points
            .iter()
            .position(|point| point["phase"] == phase)
            .unwrap_or_else(|| panic!("startup missed {phase}: {startup}"));
        assert_eq!(
            points.iter().filter(|point| point["phase"] == phase).count(),
            1,
            "startup repeated {phase}: {startup}"
        );
        assert!(
            previous.is_none_or(|earlier| earlier < index),
            "startup delivered {phase} out of order: {startup}"
        );
        previous = Some(index);
    }
}

/// Failure (PLAN, phase 4): opening reads a file's structure with lopdf on the UI thread, so a hostile file that's slow to read (here three
/// sections relisting about a million numbers each, seconds of lopdf's time) holds the window past the 2 s rule. Opened by an agent's `recto open`
/// and by File > Open Recent, the window's own path (`AppCore::open_recent`, as a Finder open's `open_file` goes, onto the core queue), the
/// watchdog sees no stall of even its 1 s note, while the read takes its seconds elsewhere. It also catches an unwatched native launch, before
/// creating the hostile fixture: all startup phases must already have reached the watchdog on its own clock.
#[test]
fn opening_a_slow_hostile_file_never_holds_the_window() {
    let Some(run) = Run::start() else { return };
    assert_startup_was_watched(&run.report());
    let file = TempPdf::new("relisted.pdf", &recto_testpdf::relisted_xref(3));
    let started = Instant::now();
    run.ok(&["open", file.0.to_str().unwrap()]);
    let by_agent = started.elapsed();
    run.settled();
    run.ok(&["close"]);
    let state = wait_long(&run, |state| state["session"]["tabs"].as_array().is_some_and(Vec::is_empty));
    let item = recent_items(&state).into_iter().next().expect("the file is in Open Recent");
    let started = Instant::now();
    run.ok(&["qa", "menu", &item]);
    let state = wait_long(&run, |state| state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 1));
    let by_menu = started.elapsed();
    run.settled();
    let hangs = &run.report()["hangs"];
    eprintln!(
        "opened by `recto open` in {by_agent:?}, by Open Recent in {by_menu:?}; the window's worst wait {}",
        hangs["worst_ms"]
    );
    // Not `over_1s == 0`: a starved runner makes a stretch of a second with the thread ready to run (CI run 43711366303, load 33 on 3 cores).
    if let Err(why) = held_the_window(hangs) {
        panic!("{why}");
    }
    assert_eq!(state["session"]["tabs"][0]["pages"].as_u64().or(Some(1)), Some(1));
}
