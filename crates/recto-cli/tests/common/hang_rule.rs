//! The one judge of "the UI thread never waited", for every test that holds the window to rule 3 by the run's hang record.

use serde_json::Value;

/// The watchdog looks every 250 ms: a stretch seen waiting in this many looks held the window, whatever the machine did besides.
const WAITING_LOOKS_THAT_HOLD: u64 = 3;

/// What the run's hang record says about the window, for the rule "a slow file never holds the window": `Err` with why if a stall reached the fail
/// bound, or the UI thread was seen waiting (blocked on something, not short of a core) for `WAITING_LOOKS_THAT_HOLD` looks. A stretch over the note
/// bound in which it was ready to run in every look is a starved machine's, and passes (CI, 2026-11-15: 1.2 s, ready to run, 84 ms of CPU, load 33 on 3 cores).
pub fn held_the_window(hangs: &Value) -> Result<(), String> {
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
    let hangs = |over_2s: u64, waiting: u64, running: u64| serde_json::json!({ "over_2s": over_2s, "worst": { "waiting_looks": waiting, "running_looks": running, "cpu_ms": 84, "wall_ms": 1238 } });
    assert!(held_the_window(&hangs(0, 0, 1)).is_ok(), "starved: ready to run, little CPU");
    assert!(held_the_window(&hangs(0, 2, 1)).is_ok(), "a short wait");
    assert!(
        held_the_window(&hangs(0, 3, 1)).unwrap_err().contains("waiting"),
        "slept, then waited for a core"
    );
    assert!(held_the_window(&hangs(1, 0, 9)).is_err(), "the fail bound is the fail bound");
    assert!(
        held_the_window(&serde_json::json!({ "over_2s": 0, "worst": null })).is_ok(),
        "no stretch over a second"
    );
}
