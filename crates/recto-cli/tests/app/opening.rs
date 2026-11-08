//! Opening in the real app, held to rule 3: the UI thread never waits on a file.

use super::Run;
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

/// Failure (PLAN, phase 4): opening reads a file's structure with lopdf on the UI thread, so a hostile file that's slow to read (here three
/// sections relisting about a million numbers each, seconds of lopdf's time) holds the window past the 2 s rule. Opened by an agent's `recto open`
/// and by File > Open Recent, the window's own path (`AppCore::open_recent`, as a Finder open's `open_file` goes, onto the core queue), the
/// watchdog sees no stall of even its 1 s note, while the read takes its seconds elsewhere.
#[test]
fn opening_a_slow_hostile_file_never_holds_the_window() {
    let Some(run) = Run::start() else { return };
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
    assert_eq!(hangs["over_2s"], 0, "{hangs}");
    assert_eq!(hangs["over_1s"], 0, "not even a 1 s note: {hangs}");
    assert_eq!(state["session"]["tabs"][0]["pages"].as_u64().or(Some(1)), Some(1));
}
