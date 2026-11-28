//! Blocks through the real commands, the way an agent uses them: a session, the real indexer and structure helper on temp state. Made only when an agent first asks for a blocks grain.

use crate::common::{Raw, State};
use crate::{path, wait_done};
use recto_agent::{Level, exit};
use recto_testpdf::TempPdf;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Waits for blocks, up to ten minutes. The helper runs at utility, below the tests' own processes, so a busy Mac slows it.
fn wait_made(state: &State, timeout: &str) -> (Value, i32) {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        assert!(Instant::now() < deadline, "blocks weren't made in ten minutes");
        let (done, code) = state.json(&["wait", "blocks-ready", "--timeout", timeout]);
        if code != exit::TIMED_OUT {
            return (done, code);
        }
    }
}

/// Failure: the first paragraph ask makes nothing (so `blocks_not_made` for ever) or waits for it (an agent stuck on a `get`), a build is made for a file nobody asked blocks of, a finished build isn't announced, or the numbers
/// of it are wrong (nothing stored, no bytes). And `index clear` leaves the blocks. On the blocks document: `get --grain paragraph` says `blocks_building` at once, `wait blocks-ready` gives numbers only, a `watch` hears
/// `blocks_building` then `blocks_built` walled and `ready`, and the clear takes the folder.
#[test]
fn an_agent_gets_blocks_made_on_its_first_paragraph_ask() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("blocks.pdf", &recto_testpdf::blocks::blocks_document());
    state.json(&["open", path(&file)]);
    wait_done(&state, "active");
    // Opening and indexing made no blocks, and a `get` of a page doesn't ask for them.
    let (made, _) = state.json(&["ref", "1"]);
    let reference = made["data"]["ref"].as_str().expect("a reference").to_string();
    let (page, code) = state.json(&["get", &reference, "--grain", "page"]);
    assert_eq!((code, page["data"]["widened"].is_null()), (0, true), "{page}");
    let mut events = Raw::connect(&state);
    events.send(&json!({ "cmd": "watch", "events": ["blocks"], "count": 2, "timeout_ms": 600_000 }).to_string());
    assert_eq!(events.read().unwrap()["data"]["watching"], true);
    let (quiet, _) = state.json(&["get", &reference, "--grain", "page"]);
    assert!(quiet["data"]["widened"].is_null(), "{quiet}");

    let started = Instant::now();
    let (asked, code) = state.json(&["get", &reference, "--grain", "paragraph"]);
    assert_eq!(code, 0, "{asked}");
    assert!(started.elapsed() < Duration::from_secs(5), "asking waited for blocks: {:?}", started.elapsed());
    assert_eq!(asked["data"]["widened"]["why"], "blocks_building", "{asked}");

    let (ready, code) = wait_made(&state, "120s");
    assert_eq!(code, 0, "{ready}");
    let data = &ready["data"];
    assert_eq!(data["state"], "ready", "{ready}");
    assert!(data["stored"].as_u64().is_some_and(|stored| stored > 0), "{ready}");
    assert!(data["bytes"].as_u64().is_some_and(|bytes| bytes > 0) && data["pages"] == 3, "{ready}");
    assert_eq!(data["by_kind"]["table"]["stored"], 0, "{ready}");
    let building = events.read().unwrap();
    assert_eq!(building["data"]["event"], "blocks_building", "{building}");
    let built = events.read().unwrap();
    assert_eq!(
        (built["data"]["event"].as_str(), built["data"]["state"].as_str()),
        (Some("blocks_built"), Some("ready")),
        "{built}"
    );
    assert_eq!(
        built["data"]["walled"], true,
        "the helper wasn't shown walled before it was given the document: {built}"
    );
    assert!(built["data"]["stored"].as_u64().is_some_and(|stored| stored > 0), "{built}");
    assert!(
        built["data"]["map_ms"].is_number() && built["data"]["elapsed_ms"].is_number(),
        "the session's own numbers: {built}"
    );

    let (cleared, code) = state.json(&["index", "clear"]);
    assert_eq!((code, cleared["data"]["cleared"].as_u64()), (0, Some(1)), "{cleared}");
    let key = recto_index::corpus::CorpusKey::of_file(&file.0).unwrap();
    assert!(!state.paths.corpus.join(key.hex()).exists(), "blocks.bin and its header went with the folder");
}

/// A measurement, not a test: `RECTO_BLOCKS_FILES` (separated by `:`) opened, indexed (timed apart), then asked for paragraph blocks and waited for, with the `blocks_built` event read. Prints a line a file.
///
/// `cargo test --release -p recto-cli --test index blocks_measure -- --ignored --nocapture`
#[test]
#[ignore = "a measurement"]
fn blocks_measure() {
    let files = std::env::var("RECTO_BLOCKS_FILES").expect("RECTO_BLOCKS_FILES: files separated by ':'");
    for file in files.split(':').filter(|file| !file.is_empty()) {
        let state = State::at(Level::Control);
        let _serving = state.serve();
        // A session of temp state opens only what is in a temp folder of its own: a copy.
        let copy = TempPdf::new("measured.pdf", &std::fs::read(file).expect("a file to measure"));
        let opened = Instant::now();
        let (said, code) = state.json(&["open", path(&copy)]);
        assert_eq!(code, 0, "{said}");
        wait_done(&state, "active");
        let index_seconds = opened.elapsed().as_secs_f64();
        let mut events = Raw::connect(&state);
        events.send(&json!({ "cmd": "watch", "events": ["blocks"], "count": 2, "timeout_ms": 3_600_000 }).to_string());
        assert_eq!(events.read().unwrap()["data"]["watching"], true);
        let (made, _) = state.json(&["ref", "1"]);
        let reference = made["data"]["ref"].as_str().expect("a reference").to_string();
        let asked = Instant::now();
        state.json(&["get", &reference, "--grain", "paragraph"]);
        let (ready, code) = wait_made(&state, "30m");
        let wall = asked.elapsed().as_secs_f64();
        events.read().unwrap();
        let built = events.read().unwrap();
        println!(
            "BLOCKS {file} code={code} index_s={index_seconds:.1} blocks_wall_s={wall:.1} ready={} built={}",
            ready["data"], built["data"]
        );
    }
}
