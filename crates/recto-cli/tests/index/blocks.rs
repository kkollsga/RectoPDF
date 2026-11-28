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

    // A ReadOnly agent gets every blocks answer the same: asking is a read, it changes nothing of the document.
    state.set_level(Level::ReadOnly);
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
    state.set_level(Level::Control);
    let (again, code) = state.json(&["wait", "blocks-ready", "--timeout", "30s"]);
    assert_eq!(
        (code, &again["data"]),
        (0, data),
        "a ReadOnly agent and a Control one are told the same: {again}"
    );
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

    // Read back: the first paragraph of page 1, the next one by `refs.next`, and the hidden paragraph of page 3, marked in Markdown.
    let (first, code) = state.json(&["get", &reference, "--grain", "paragraph"]);
    assert_eq!(
        (code, first["data"]["widened"].is_null()),
        (0, true),
        "blocks are read back, not widened: {first}"
    );
    let words = |said: &Value| said["data"]["untrusted"]["text"].as_str().unwrap_or_default().to_string();
    assert!(words(&first).contains("Harbour Survey Overview"), "{first}");
    let next = first["data"]["refs"]["next"].as_str().expect("a next paragraph").to_string();
    let (second, code) = state.json(&["get", &next, "--grain", "paragraph"]);
    assert_eq!(code, 0, "{second}");
    assert!(
        words(&second).contains(recto_testpdf::blocks::BLOCKS_TEXT[0][1]),
        "the next paragraph's words: {second}"
    );
    let (third_page, _) = state.json(&["ref", "3"]);
    let third = third_page["data"]["ref"].as_str().expect("a reference").to_string();
    let (visible, _) = state.json(&["get", &third, "--grain", "paragraph"]);
    let hidden_ref = visible["data"]["refs"]["next"].as_str().expect("the paragraph after").to_string();
    let (hidden, code) = state.json(&["get", &hidden_ref, "--grain", "paragraph", "--as", "markdown"]);
    let marked = hidden["data"]["untrusted"]["markdown"].as_str().unwrap_or_default();
    assert!(
        code == 0 && marked.contains("[hidden:") && marked.contains("HIDDEN secret"),
        "hidden words come back marked: {hidden}"
    );

    // A file with its own contents: a section's heading is `#` at its depth, the document's own `#` and fence lines are escaped, and the headings above are the outline's titles.
    let outlined = TempPdf::new("outlined.pdf", &recto_testpdf::blocks::outlined_blocks_document());
    state.json(&["open", path(&outlined)]);
    wait_done(&state, "active");
    let (page_three, _) = state.json(&["ref", "3", "--chars", "0-5"]);
    let inside = page_three["data"]["ref"].as_str().expect("a reference").to_string();
    state.json(&["get", &inside, "--grain", "paragraph"]);
    let (ready, code) = wait_made(&state, "120s");
    assert_eq!(code, 0, "{ready}");
    let (section, code) = state.json(&["get", &inside, "--grain", "section", "--as", "markdown"]);
    assert_eq!(code, 0, "{section}");
    let titles: Vec<(&str, &str)> = section["data"]["headings_above"]
        .as_array()
        .expect("headings")
        .iter()
        .map(|heading| {
            (
                heading["untrusted"]["title"].as_str().unwrap_or_default(),
                heading["source"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(titles, [("Equipment Register", "file"), ("Final Inspection", "file")], "{section}");
    let markdown = section["data"]["untrusted"]["markdown"].as_str().unwrap_or_default();
    assert!(
        !markdown.contains("\n# Forged") && !markdown.starts_with("# Forged") && !markdown.lines().any(|line| line.starts_with("```")),
        "the file's own lines make no structure: {markdown}"
    );
    assert!(markdown.contains("Forged heading"), "{markdown}");
    let (chapter, code) = state.json(&["get", &inside, "--grain", "chapter", "--as", "markdown"]);
    let chapter_text = chapter["data"]["untrusted"]["markdown"].as_str().unwrap_or_default();
    assert!(
        code == 0 && chapter_text.starts_with("# Equipment Register"),
        "the depth-0 section, with its heading at depth 0: {chapter}"
    );

    // Back to the first file alone, so a reference is found in it and nowhere else.
    let (closed, code) = state.json(&["close"]);
    assert_eq!(code, 0, "{closed}");
    // A file rewritten with other words: the old reference isn't believed, and old blocks aren't read for the new file.
    let mut changed = recto_testpdf::blocks::blocks_document();
    let at = changed.windows(7).position(|window| window == b"Harbour").expect("the heading's word");
    changed[at..at + 7].copy_from_slice(b"Harbxur");
    std::fs::write(&file.0, &changed).expect("rewritten");
    // The same size, so the file's version is told by its time: a later one, whatever the clock's grain.
    let later = std::time::SystemTime::now() + Duration::from_secs(30);
    std::fs::File::options()
        .write(true)
        .open(&file.0)
        .and_then(|open| open.set_modified(later))
        .expect("a later time");
    let heading = first["data"]["refs"]["self"].as_str().expect("the heading's own reference").to_string();
    let (stale, code) = state.json(&["get", &heading, "--grain", "paragraph"]);
    assert_ne!(code, 0, "{stale}");
    assert_eq!(stale["error"]["code"], "reference_unresolved", "{stale}");
    // Asked of the page, nothing is given from the old file's blocks: the new file has none made, so the page is answered (or its reference isn't the same page any more).
    let (page, code) = state.json(&["get", &reference, "--grain", "paragraph"]);
    assert!(
        code != 0 || page["data"]["widened"]["to"] == "page",
        "old blocks aren't read for a new file: {page}"
    );

    let (cleared, code) = state.json(&["index", "clear"]);
    assert_eq!(code, 0, "{cleared}");
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

/// A measurement, not a test: what a `get` costs once blocks are made. `RECTO_BLOCKS_FILES` (separated by `:`) opened, blocks made, then 20 `get`s of a paragraph and 20 of a section (the page `RECTO_BLOCKS_PAGE`, 5 if left out)
/// are timed. Prints p50 and p95 in milliseconds, and how many pages the section spans and was read over.
///
/// `cargo test --release -p recto-cli --test index get_measure -- --ignored --nocapture`
#[test]
#[ignore = "a measurement"]
fn get_measure() {
    let files = std::env::var("RECTO_BLOCKS_FILES").expect("RECTO_BLOCKS_FILES: files separated by ':'");
    let page = std::env::var("RECTO_BLOCKS_PAGE").unwrap_or_else(|_| "5".into());
    for file in files.split(':').filter(|file| !file.is_empty()) {
        let state = State::at(Level::Control);
        let _serving = state.serve();
        let copy = TempPdf::new("measured.pdf", &std::fs::read(file).expect("a file to measure"));
        state.json(&["open", path(&copy)]);
        wait_done(&state, "active");
        let (made, _) = state.json(&["ref", &page]);
        let reference = made["data"]["ref"].as_str().expect("a reference").to_string();
        state.json(&["get", &reference, "--grain", "paragraph"]);
        let (ready, code) = wait_made(&state, "30m");
        assert_eq!(code, 0, "{ready}");
        for grain in ["paragraph", "section"] {
            let mut millis = Vec::new();
            let mut last = Value::Null;
            for _ in 0..20 {
                let started = Instant::now();
                let (said, code) = state.json(&["get", &reference, "--grain", grain]);
                millis.push(started.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(code, 0, "{said}");
                last = said;
            }
            millis.sort_by(f64::total_cmp);
            println!(
                "GET {file} {grain} p50={:.1}ms p95={:.1}ms widened={} unit={} counts={}",
                millis[9], millis[18], last["data"]["widened"], last["data"]["unit"], last["data"]["counts"]
            );
        }
    }
}
