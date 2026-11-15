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

/// The watchdog looks every 250 ms: a stretch seen waiting in this many looks held the window, whatever the machine did besides.
const WAITING_LOOKS_THAT_HOLD: u64 = 3;

/// What the run's hang record says about the window, for the rule "a slow file never holds the window": `Err` with why if a stall reached the fail
/// bound, or the UI thread was seen waiting (blocked on something, not short of a core) for `WAITING_LOOKS_THAT_HOLD` looks. A stretch over the note
/// bound in which it was ready to run in every look is a starved machine's, and passes (CI, 2026-11-15: 1.2 s, ready to run, 84 ms of CPU, load 33 on 3 cores).
fn held_the_window(hangs: &Value) -> Result<(), String> {
    if hangs["over_2s"] != 0 {
        return Err(format!("a stall reached the fail bound: {hangs}"));
    }
    let worst = &hangs["worst"];
    if worst["waiting_looks"].as_u64().unwrap_or(0) >= WAITING_LOOKS_THAT_HOLD {
        return Err(format!("the UI thread was waiting, not short of a core: {hangs}"));
    }
    Ok(())
}

/// Failure: the rule's judge lets a waiting thread pass because the machine was slow, or fails a starved one.
#[test]
fn a_waiting_window_is_told_from_a_starved_one() {
    let hangs = |over_2s: u64, waiting: u64, running: u64| {
        serde_json::json!({ "over_2s": over_2s, "worst": { "waiting_looks": waiting, "running_looks": running, "cpu_ms": 84, "wall_ms": 1238 } })
    };
    assert!(held_the_window(&hangs(0, 0, 1)).is_ok(), "starved: ready to run, little CPU");
    assert!(held_the_window(&hangs(0, 2, 1)).is_ok(), "a short wait");
    assert!(held_the_window(&hangs(0, 3, 1)).unwrap_err().contains("waiting"), "slept, then waited for a core");
    assert!(held_the_window(&hangs(1, 0, 9)).is_err(), "the fail bound is the fail bound");
    assert!(held_the_window(&serde_json::json!({ "over_2s": 0, "worst": null })).is_ok(), "no stretch over a second");
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
    // Not `over_1s == 0`: a starved runner makes a stretch of a second with the thread ready to run (CI run 43711366303, load 33 on 3 cores).
    if let Err(why) = held_the_window(hangs) {
        panic!("{why}");
    }
    assert_eq!(state["session"]["tabs"][0]["pages"].as_u64().or(Some(1)), Some(1));
}
