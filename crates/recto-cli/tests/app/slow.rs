//! The slow mode itself (`recto_platform::slow`): a quiet run with `RECTO_QA_SLOW` set is late at every waiting point it passes, says so in `qa state`, and never on the UI thread. What the slow mode finds is in the tests the `slow` stage of `tools/check.sh --full` runs with it on.

use super::common::State;
use super::common::app::{Run, numbered, wait_for_long};
use recto_agent::Level;
use serde_json::Value;

fn times(report: &Value, seam: &str) -> u64 {
    report["slow"]["seams"][seam]["times"].as_u64().unwrap_or(0)
}

/// Failure: the slow mode is on but waits nowhere, or at some seams only, so a stage run with it passes for a slow machine's when nothing was slow; or a seam waits on the UI thread, which would hang the app (AGENTS rule 3). Each seam a launch, two tabs, an index, going dormant and waking pass through is counted as waited at, and none was asked on the UI thread.
#[test]
fn a_slow_run_waits_at_every_seam_it_passes() {
    let state = State::at(Level::Full);
    let Some(run) = Run::on_with_env(&state, &[("RECTO_QA_SLOW", std::ffi::OsStr::new("ci"))]) else {
        return;
    };
    let (left, read) = (numbered(6), numbered(3));
    run.ok(&["open", left.0.to_str().unwrap()]);
    run.settled();
    let done = run.ok(&["wait", "indexing-done", "--tab", "active", "--timeout", "60s"]);
    assert_eq!(done["state"], "ready", "{done}");
    run.ok(&["open", read.0.to_str().unwrap()]);
    run.settled();
    // Left alone, a second counting as five minutes: it goes dormant, with a sleep file, and wakes from it when tapped.
    run.ok(&["qa", "dormant-after", "1"]);
    // Its sleep file's written by the queue's writer, which ends once it's done.
    wait_for_long(&run, |report| {
        report["record"]["sleep"]["gone_dormant"]["left_alone"].as_u64().unwrap_or(0) >= 1 && report["record"]["sleep"]["leaving"]["writer"] == false
    });
    run.ok(&["qa", "dormant-after", "300"]);
    let tabs = run.report()["session"]["tabs"].as_array().unwrap().clone();
    let real = |path: &str| std::fs::canonicalize(path).ok();
    let left_tab = tabs
        .iter()
        .find(|tab| real(tab["path"].as_str().unwrap_or("")) == real(left.0.to_str().unwrap()))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    run.ok(&["tap", &format!("#{left_tab}")]);
    run.settled();
    let report = run.report();
    let slow = &report["slow"];
    for seam in ["state", "socket", "worker", "draw", "index", "sleep_write", "sleep_read"] {
        assert!(times(&report, seam) >= 1, "{seam} was never waited at: {slow}");
        assert!(slow["seams"][seam]["ms"].as_u64().unwrap() > 0, "{seam}: {slow}");
    }
    assert_eq!(slow["ui_refusals"], 0, "a seam was asked to wait on the UI thread: {slow}");
    run.quit();
}

/// Failure: a quiet run with no switch is slow anyway, or says it is, so every app test pays for delays nobody asked for. Started with the switches taken out of its environment, as the `slow` stage has them in its own.
#[test]
fn a_quiet_run_with_no_switch_is_never_slow() {
    let Some(app) = super::app_path() else { return };
    let state = State::new();
    // Whatever this test was run with: the `slow` stage runs it with the switch on.
    let mut command = state.command();
    command.args(["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "1m", "--json"]);
    for name in ["RECTO_QA_SLOW", "RECTO_QA_RESTORE_DELAY_MS", "RECTO_QA_REMAKE_DELAY_MS"] {
        command.env_remove(name);
    }
    let output = command.output().unwrap();
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(0), "{reply}");
    let run = Run::attach(&state, reply["data"]["pid"].as_u64().unwrap() as u32);
    assert!(run.report()["slow"].is_null(), "{}", run.report()["slow"]);
    run.quit();
}

/// Failure: a switch with a typo in it is taken as nothing, and the slow stage passes as a fast run. The launch is refused, with the switch's words.
#[test]
fn a_switch_that_cant_be_read_refuses_the_launch() {
    let Some(app) = super::app_path() else { return };
    let state = State::new();
    let (reply, code) = {
        let output = state
            .command()
            .args(["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "1m", "--json"])
            .env("RECTO_QA_SLOW", "wrker=5")
            .output()
            .unwrap();
        (String::from_utf8_lossy(&output.stdout).into_owned(), output.status.code())
    };
    assert_ne!(code, Some(0), "{reply}");
    assert!(reply.contains("RECTO_QA_SLOW"), "{reply}");
}

/// Failure: a test the `slow` stage of `tools/check.sh` names is renamed or moved, and its filter matches nothing, so the stage quietly stops running it with nobody told. Every name in `SLOW_TESTS` is a test in the app or restore binaries, in the module it says. Needs no app.
#[test]
fn every_test_the_slow_stage_names_is_there() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let check = std::fs::read_to_string(tests.join("../../../tools/check.sh")).unwrap();
    let line = check
        .lines()
        .find(|line| line.starts_with("SLOW_TESTS='"))
        .expect("check.sh says what the slow stage runs");
    let names: Vec<&str> = line.split("test(=").skip(1).filter_map(|part| part.split(')').next()).collect();
    assert!(names.len() >= 20, "{names:?}");
    for name in names {
        let (files, function) = match name.split_once("::") {
            Some((module, function)) => (
                vec![
                    tests.join("app").join(format!("{module}.rs")),
                    tests.join("restore").join(format!("{module}.rs")),
                ],
                function,
            ),
            None => (vec![tests.join("app.rs"), tests.join("restore.rs")], name),
        };
        let found = files.iter().filter_map(|file| std::fs::read_to_string(file).ok()).any(|source| {
            let lines: Vec<&str> = source.lines().collect();
            lines
                .windows(2)
                .any(|pair| pair[0].trim() == "#[test]" && pair[1].starts_with(&format!("fn {function}()")))
        });
        assert!(found, "the slow stage names `{name}`, which isn't a test where it says");
    }
}
