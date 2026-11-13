//! `recto find`, the app's Find for an agent, through a real session, worker and index on temp state.

mod common;

use common::State;
use recto_agent::find;
use recto_agent::{Command, DrawableLoader, Event};
use recto_agent::{Level, exit};
use recto_control::host::{Host, SessionHost};
use recto_testpdf::{Page, TempPdf, pages, text_lines};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn doc(name: &str, count: usize, word: &str) -> TempPdf {
    let bodies: Vec<String> = (1..=count)
        .map(|at| format!("Page {at} of the document\nsays {word} here and {word} again\nthe end of page {at}"))
        .collect();
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
    assert_eq!(
        (
            data["state"].as_str(),
            data["open"].as_bool(),
            data["hits_total"].as_u64(),
            data["pages_with_hits"].as_u64()
        ),
        (Some("done"), Some(true), Some(24), Some(12)),
        "{found}"
    );
    assert_eq!(data["status"], "24 matches on 12 pages");
    let hits = data["hits"].as_array().unwrap();
    let places: Vec<(u64, u64)> = hits.iter().map(|hit| (hit["page"].as_u64().unwrap(), hit["start"].as_u64().unwrap())).collect();
    let mut sorted = places.clone();
    sorted.sort_unstable();
    assert_eq!(places, sorted, "reading order");
    // What `search` finds, page for page.
    let (searched, _) = state.json(&["search", "lantern", "--limit", "200"]);
    let search_pages: Vec<u64> = searched["data"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["page"].as_u64().unwrap())
        .collect();
    assert_eq!(places.iter().map(|place| place.0).collect::<Vec<_>>(), search_pages);
    // A hit's characters are the page's as `text` gives them.
    let (text, _) = state.json(&["text", "--page", "3"]);
    let page_text: Vec<char> = text["data"]["untrusted"]["text"].as_str().unwrap().chars().collect();
    let on_three = hits.iter().find(|hit| hit["page"] == 3).unwrap();
    let words: String = page_text[on_three["start"].as_u64().unwrap() as usize..on_three["end"].as_u64().unwrap() as usize]
        .iter()
        .collect();
    assert_eq!(words, "lantern", "{on_three}");
    // From where the reader is.
    state.json(&["goto", "5"]);
    let (next, _) = state.json(&["find", "--next"]);
    assert_eq!(next["data"]["current"]["page"], 5, "{next}");
    assert_eq!(next["data"]["went_to"]["page"], 5, "counted from 1, as every page in the answer is: {next}");
    let (next, _) = state.json(&["find", "--next"]);
    assert_eq!(
        (next["data"]["current"]["page"].as_u64(), next["data"]["current"]["index"].as_u64()),
        (Some(5), Some(9)),
        "{next}"
    );
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
    assert_eq!(
        (closed["data"]["open"].as_bool(), closed["data"]["state"].as_str()),
        (Some(false), Some("idle")),
        "{closed}"
    );
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

/// Failure: a page deleted, moved or added after a query leaves Find's hits on the old arrangement, so `--next` (and ⌘G) selects the wrong words. The hits are found again for the pages as they are.
#[test]
fn hits_follow_the_pages_after_an_edit() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("edited.pdf", 12, "lantern");
    state.json(&["open", path(&file)]);
    state.json(&["find", "lantern"]);
    // Pages 1 to 9 go, so what's left is "Page 10" onwards, one digit longer: every hit's place on its page moves.
    let (deleted, code) = state.json(&["page", "delete", "1-9"]);
    assert_eq!(code, 0, "{deleted}");
    state.json(&["goto", "1"]);
    let (next, code) = state.json(&["find", "--next"]);
    assert_eq!(code, 0, "{next}");
    assert_eq!(
        (next["data"]["hits_total"].as_u64(), next["data"]["current"]["page"].as_u64()),
        (Some(6), Some(1)),
        "found again on the three pages left: {next}"
    );
    let (text, _) = state.json(&["text", "--page", "1"]);
    let chars: Vec<char> = text["data"]["untrusted"]["text"].as_str().unwrap().chars().collect();
    let (start, end) = (
        next["data"]["current"]["start"].as_u64().unwrap() as usize,
        next["data"]["current"]["end"].as_u64().unwrap() as usize,
    );
    assert_eq!(chars[start..end].iter().collect::<String>(), "lantern", "{next}");
}

/// Failure: a query asked for while another runs doesn't stop it, so two run, the first's hits land in the second's list, or each says it's finished; or the thread stays once nothing runs. The second is started from the first's own news, so they surely overlap.
#[test]
fn a_query_started_mid_run_stops_the_first() {
    let state = State::new();
    let host = Arc::new(SessionHost::new(DrawableLoader).with_indexing(&state.paths));
    let file = doc("overlap.pdf", 400, "alpha");
    host.run(
        recto_agent::Actor::Agent(Level::Full),
        &Command::from(recto_agent::commands::tabs::Open {
            path: file.0.clone(),
            page: None,
        }),
        &mut Vec::new(),
    )
    .unwrap();
    let finished = Arc::new(AtomicU32::new(0));
    let started_second = Arc::new(AtomicBool::new(false));
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let (host_for_news, finished, started_second, seen) = (Arc::clone(&host), Arc::clone(&finished), Arc::clone(&started_second), Arc::clone(&seen));
        host.attach(Arc::new(move |events: Vec<Event>| {
            for event in events {
                seen.lock().unwrap().push(event.name().to_string());
                if event.name() == "find_finished" {
                    finished.fetch_add(1, Ordering::SeqCst);
                }
                if matches!(event, recto_agent::Event::FindRunning) && !started_second.swap(true, Ordering::SeqCst) {
                    host_for_news.find_begin("page 7 of").unwrap();
                }
            }
        }));
    }
    host.find_begin("alpha").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while (host.finder().is_running() || finished.load(Ordering::SeqCst) == 0 || host.finder().has_thread()) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        started_second.load(Ordering::SeqCst),
        "the second was started mid-run: {:?}",
        seen.lock().unwrap()
    );
    assert_eq!(finished.load(Ordering::SeqCst), 1, "one query finished, the second: {:?}", seen.lock().unwrap());
    assert!(!host.finder().has_thread(), "no thread once nothing runs");
    let mut pages = Vec::new();
    let mut query = String::new();
    find::Finding::with_results(host.finder(), &mut |results| {
        pages = results.hits.iter().map(|hit| hit.page).collect();
        query.clone_from(&results.query);
    });
    assert_eq!((query.as_str(), pages), ("page 7 of", vec![6]), "only the second's hits");
}

/// Failure: a query told to stop goes on reading pages, so a new one waits behind the old one's whole document.
#[test]
fn a_stopped_query_stops_at_the_next_page() {
    let file = doc("stop.pdf", 50, "alpha");
    let mut session = recto_session::Session::new(DrawableLoader);
    recto_agent::run(
        &mut session,
        Level::Full,
        &Command::from(recto_agent::commands::tabs::Open {
            path: file.0.clone(),
            page: None,
        }),
    )
    .unwrap();
    let id = session.active().unwrap();
    let plan = find::plan(&session, id).unwrap();
    let stop = AtomicBool::new(false);
    let (end, progress) = find::run(&plan, &find::needle("alpha").unwrap(), None, &stop, &mut |_, _| {
        stop.store(true, Ordering::SeqCst)
    });
    assert_eq!((end, progress.pages_done), (find::End::Stopped, 1));
}
