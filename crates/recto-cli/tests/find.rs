//! `recto find`, the app's Find for an agent, through a real session, worker and index on temp state.

mod common;

use common::{Raw, State};
use recto_agent::{Level, exit};
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use serde_json::json;
use std::time::{Duration, Instant};

fn doc(name: &str, count: usize, word: &str) -> TempPdf {
    let bodies: Vec<String> = (1..=count).map(|at| format!("Page {at} of the document\nsays {word} here and {word} again\nthe end of page {at}")).collect();
    let list: Vec<Page> = bodies.iter().map(|body| Page::new(text_lines(&body.lines().collect::<Vec<_>>()))).collect();
    TempPdf::new(name, &pages(&list))
}

fn path(file: &TempPdf) -> &str {
    file.0.to_str().unwrap()
}

/// Failure: Find finds other hits than `search` does, lists them out of reading order, or says where they are in a way a selection can't use; ⌘G's way of stepping doesn't start where the reader is or doesn't go round; closing leaves it open.
#[test]
fn find_lists_what_search_finds_in_order_and_steps_from_the_reader() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("find.pdf", 12, "lantern");
    state.json(&["open", path(&file)]);
    let (found, code) = state.json(&["find", "lantern", "--limit", "200"]);
    assert_eq!(code, 0, "{found}");
    let data = &found["data"];
    assert_eq!((data["state"].as_str(), data["open"].as_bool(), data["hits_total"].as_u64(), data["pages_with_hits"].as_u64()), (Some("done"), Some(true), Some(24), Some(12)), "{found}");
    assert_eq!(data["status"], "24 matches on 12 pages");
    let hits = data["hits"].as_array().unwrap();
    let places: Vec<(u64, u64)> = hits.iter().map(|hit| (hit["page"].as_u64().unwrap(), hit["start"].as_u64().unwrap())).collect();
    let mut sorted = places.clone();
    sorted.sort_unstable();
    assert_eq!(places, sorted, "reading order");
    // What `search` finds, page for page.
    let (searched, _) = state.json(&["search", "lantern", "--limit", "200"]);
    let search_pages: Vec<u64> = searched["data"]["hits"].as_array().unwrap().iter().map(|hit| hit["page"].as_u64().unwrap()).collect();
    assert_eq!(places.iter().map(|place| place.0).collect::<Vec<_>>(), search_pages);
    // A hit's characters are the page's as `text` gives them.
    let (text, _) = state.json(&["text", "--page", "3"]);
    let page_text: Vec<char> = text["data"]["untrusted"]["text"].as_str().unwrap().chars().collect();
    let on_three = hits.iter().find(|hit| hit["page"] == 3).unwrap();
    let words: String = page_text[on_three["start"].as_u64().unwrap() as usize..on_three["end"].as_u64().unwrap() as usize].iter().collect();
    assert_eq!(words, "lantern", "{on_three}");
    // From where the reader is.
    state.json(&["goto", "5"]);
    let (next, _) = state.json(&["find", "--next"]);
    assert_eq!(next["data"]["current"]["page"], 5, "{next}");
    assert_eq!(next["data"]["went_to"]["page"], 4, "from 0, as a selection counts: {next}");
    let (next, _) = state.json(&["find", "--next"]);
    assert_eq!((next["data"]["current"]["page"].as_u64(), next["data"]["current"]["index"].as_u64()), (Some(5), Some(9)), "{next}");
    let (here, _) = state.json(&["where"]);
    assert_eq!(here["data"]["page"], 5);
    state.json(&["goto", "1"]);
    let (back, _) = state.json(&["find", "--previous"]);
    assert_eq!(back["data"]["current"]["index"], 8, "on from the current hit, not the page: {back}");
    for _ in 0..8 {
        state.json(&["find", "--previous"]);
    }
    let (round, _) = state.json(&["find", "--previous"]);
    assert_eq!(round["data"]["current"]["index"], 23, "round to the last: {round}");
    let (shown, _) = state.json(&["session", "show"]);
    assert_eq!(shown["data"]["find"]["query"], "lantern", "{shown}");
    let (closed, _) = state.json(&["find", "--close"]);
    assert_eq!((closed["data"]["open"].as_bool(), closed["data"]["state"].as_str()), (Some(false), Some("idle")), "{closed}");
    let (stepped, code) = state.json(&["find", "--next"]);
    assert_eq!(code, exit::NOT_FOUND, "nothing to go to once it's closed: {stepped}");
}

/// Failure: Find is open to an agent that may only read, or a query that can't find anything is taken, or nothing being read isn't said.
#[test]
fn find_is_refused_below_control_and_says_what_it_cant_do() {
    let state = State::at(Level::ReadOnly);
    let _serving = state.serve();
    let (refused, code) = state.json(&["find", "x"]);
    assert_ne!(code, 0, "{refused}");
    state.set_level(Level::Control);
    let (none, code) = state.json(&["find", "x"]);
    assert_eq!((code, none["error"]["code"].as_str()), (exit::NOT_FOUND, Some("no_active_tab")), "{none}");
    let file = doc("small.pdf", 2, "word");
    state.json(&["open", path(&file)]);
    let (empty, code) = state.json(&["find", "   "]);
    assert_eq!((code, empty["error"]["code"].as_str()), (exit::FAILED, Some("bad_query")), "{empty}");
    let (long, code) = state.json(&["find", &"x".repeat(2000)]);
    assert_eq!(code, exit::FAILED, "{long}");
    let (missing, _) = state.json(&["find", "zzqqxx"]);
    assert_eq!(missing["data"]["status"], "Not found", "{missing}");
}

/// Failure: a new query doesn't stop the old one, so the old one's hits land in the new list, or two run at once; or a session at rest keeps a thread for Find.
#[test]
fn a_new_query_replaces_the_one_running() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("long.pdf", 400, "alpha");
    state.json(&["open", path(&file)]);
    // The first, read page by page with no index yet, from one connection, and the second from another while it runs.
    let mut first = Raw::connect(&state);
    first.send(&json!({ "cmd": "find", "query": "alpha" }).to_string());
    std::thread::sleep(Duration::from_millis(150));
    let (second, code) = state.json(&["find", "page 7 of"]);
    assert_eq!(code, 0, "{second}");
    let first = first.read().expect("an answer");
    assert_eq!(first["ok"], true, "{first}");
    // Whatever the first saw, Find now holds the second's hits only.
    let deadline = Instant::now() + Duration::from_secs(60);
    let now = loop {
        let (now, _) = state.json(&["find", "--limit", "200"]);
        if now["data"]["state"] == "done" || Instant::now() > deadline {
            break now;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(now["data"]["query"], "page 7 of", "{now}");
    let pages: Vec<u64> = now["data"]["hits"].as_array().unwrap().iter().map(|hit| hit["page"].as_u64().unwrap()).collect();
    assert_eq!(pages, vec![7], "only the second query's hits");
    assert!(now["data"]["hits"].as_array().unwrap().iter().all(|hit| hit["untrusted"]["matched"].as_str().unwrap().to_lowercase() == "page 7 of"), "{now}");
}
