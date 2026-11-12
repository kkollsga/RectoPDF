//! The search index through the real commands, the way an agent uses them: a session, a real helper process and real PDFium, on temp state.

mod common;

use common::{Raw, State};
use recto_agent::{Level, exit};
use recto_index::corpus::CorpusKey;
use recto_testpdf::{Page, TempPdf, form_bomb, hidden_text_page, pages, text_lines};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn doc(name: &str, count: usize) -> TempPdf {
    let bodies: Vec<String> = (1..=count)
        .map(|at| format!("This is page {at} of the alpha document\nand every page says the word common here\nalso common again on page {at}"))
        .collect();
    let list: Vec<Page> = bodies.iter().map(|body| Page::new(text_lines(&body.lines().collect::<Vec<_>>()))).collect();
    TempPdf::new(name, &pages(&list))
}

fn path(file: &TempPdf) -> &str {
    file.0.to_str().unwrap()
}

/// Everything but what's expected to differ between the two ways of reading a page: how it was read, and how far the count reached.
fn the_same_answer(mut answer: Value) -> Value {
    let data = answer["data"].as_object_mut().unwrap();
    data.remove("route");
    data.remove("index");
    let coverage = data["coverage"].as_object_mut().unwrap();
    coverage.remove("hits_total");
    coverage.remove("hits_at_least");
    coverage.remove("uncounted_pages");
    answer
}

fn wait_done(state: &State, tab: &str) -> Value {
    // The helper is at utility, below the tests' own processes, so a busy Mac slows it. If it never finishes, nextest's limit ends this.
    let (done, code) = loop {
        let (done, code) = state.json(&["wait", "indexing-done", "--tab", tab, "--timeout", "120s"]);
        if code != exit::TIMED_OUT {
            break (done, code);
        }
    };
    assert_eq!(code, 0, "{done}");
    assert_eq!(done["data"]["state"], "ready", "{done}");
    done
}

/// What `the_same_answer` leaves out. Reading page by page can't always know the total, and when it does, the index must say the same. And a total is never given over a page that couldn't be counted.
fn totals_agree(indexed: &Value, scanned: &Value, args: &[&str]) {
    let (index, scan) = (&indexed["data"]["coverage"], &scanned["data"]["coverage"]);
    if scan["hits_total"].is_u64() {
        assert_eq!(index["hits_total"], scan["hits_total"], "{args:?}: {indexed}");
    } else {
        assert!(
            index["hits_at_least"].as_u64() >= scan["hits_at_least"].as_u64(),
            "{args:?}: {indexed} {scanned}"
        );
    }
    for answer in [index, scan] {
        if answer["failed_pages"].as_array().is_some_and(|failed| !failed.is_empty())
            || answer["uncounted_pages"].as_u64().is_some_and(|uncounted| uncounted > 0)
        {
            assert_eq!(answer["hits_total"], Value::Null, "{args:?}: {answer}");
        }
    }
}

/// One command line, read as `recto` reads it, sent over one connection: the two routes are the session's, and a process per query adds nothing to what's
/// compared (58 of them made one test 4 s on CI). `recto` itself is held to the same answer by `the_index_answers_as_reading_the_pages_does`.
fn over_the_socket(state: &State, args: &[&str]) -> Value {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let invocation = recto_agent::table::parse_cli(&args).unwrap_or_else(|error| panic!("{args:?}: {error:?}"));
    Raw::connect(state).call(&serde_json::from_str(&invocation.wire()).unwrap())
}

/// Holds a search answered from the index to the same search read page by page, field by field, and says which route each took.
fn index_matches_scan(state: &State, args: &[&str]) -> Value {
    let indexed = over_the_socket(state, args);
    assert_eq!(indexed["ok"], true, "{args:?}: {indexed}");
    assert_eq!(indexed["data"]["route"], "index", "{args:?} wasn't answered from the index: {indexed}");
    let mut with_scan = args.to_vec();
    with_scan.push("--scan");
    let scanned = over_the_socket(state, &with_scan);
    assert_eq!(scanned["ok"], true, "{args:?}: {scanned}");
    assert_eq!(scanned["data"]["route"], "scan", "{scanned}");
    totals_agree(&indexed, &scanned, args);
    let answer = indexed.clone();
    assert_eq!(the_same_answer(indexed), the_same_answer(scanned), "{args:?}");
    answer
}

/// Failure: the fast route says something the page-by-page route wouldn't: a hit, a line, a snippet, a hidden-text flag, a tag character, a page with no text or where to go on from. **Reading the corpus must make no difference but the time.** Line-order repair and hidden text included, since `hidden_text_page` has every technique the check knows.
#[test]
fn the_index_answers_as_reading_the_pages_does() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let mut list: Vec<Page> = (1..=12)
        .map(|at| Page::new(text_lines(&[&format!("Page {at} says common and common"), "A second line"])))
        .collect();
    list.push(Page::new(hidden_text_page()));
    list.push(Page::new(Vec::new()));
    list.push(Page::new(text_lines(&["word broken at a line", "VISIBLE-SENTINEL again common"])));
    let file = TempPdf::new("matches.pdf", &pages(&list));
    state.json(&["open", path(&file)]);
    state.json(&["index"]);
    wait_done(&state, "active");
    for args in [
        &["search", "common"][..],
        &["search", "COMMON", "--limit", "3"][..],
        &["search", "common", "--limit", "1"][..],
        &["search", "common", "--pages", "2-4"][..],
        &["search", "common", "--pages", "13-"][..],
        &["search", "says common"][..],
        &["search", "second line"][..],
        &["search", "HIDDEN-WHITE"][..],
        &["search", "VISIBLE-SENTINEL"][..],
        &["search", "zzzqqqxx"][..],
    ] {
        index_matches_scan(&state, args);
    }
    let hidden = index_matches_scan(&state, &["search", "HIDDEN-WHITE"]);
    let hit = &hidden["data"]["hits"][0];
    assert_eq!(
        (
            hit["hidden_text"]["checked"].as_bool(),
            hit["hidden_text"]["flagged"].as_bool(),
            hit["overlaps_hidden"].as_bool()
        ),
        (Some(true), Some(true), Some(true)),
        "{hit}"
    );
    let visible = index_matches_scan(&state, &["search", "VISIBLE-SENTINEL normal"]);
    assert_eq!(visible["data"]["hits"][0]["overlaps_hidden"], false, "{visible}");
    let (no_text, _) = state.json(&["search", "zzzqqqxx"]);
    assert_eq!(no_text["data"]["coverage"]["no_text_pages"], serde_json::json!([14]), "{no_text}");
    // `recto` itself, once each way: what it says is the session's answer, and it reads `--scan`.
    for args in [&["search", "common"][..], &["search", "common", "--scan"][..]] {
        let (by_recto, code) = state.json(args);
        assert_eq!(code, 0, "{args:?}: {by_recto}");
        assert_eq!(by_recto, over_the_socket(&state, args), "{args:?}");
    }
}

/// Failure: a blind agent's request: an answer stopped at its limit says nothing about how many there are in all, so it pages through a document to count. It must count past the limit from the index, and page by page must say it can't.
#[test]
fn the_index_gives_a_whole_document_total() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("total.pdf", 30);
    state.json(&["open", path(&file)]);
    let (first, code) = state.json(&["search", "common", "--limit", "3"]);
    assert_eq!(code, 0, "{first}");
    assert_eq!(first["data"]["route"], "scan", "there's no index yet: {first}");
    assert_eq!(
        first["data"]["coverage"]["hits_total"],
        Value::Null,
        "it stopped at the limit, so it can't know: {first}"
    );
    assert!(first["data"]["coverage"]["hits_at_least"].as_u64().unwrap() >= 3);
    assert!(first["data"]["index"]["note"].as_str().unwrap().contains("wait indexing-done"), "{first}");
    wait_done(&state, "active");
    let (limited, _) = state.json(&["search", "common", "--limit", "3"]);
    assert_eq!(limited["data"]["route"], "index", "{limited}");
    assert_eq!(limited["data"]["hits"].as_array().unwrap().len(), 3);
    assert_eq!(limited["data"]["coverage"]["hits_total"], 60, "{limited}");
    assert_eq!(
        limited["data"]["coverage"]["next_page"], 3,
        "two a page, and the limit falls on the second: {limited}"
    );
    let (scoped, _) = state.json(&["search", "common", "--limit", "1", "--pages", "2-3"]);
    assert_eq!(scoped["data"]["coverage"]["hits_total"], 4, "the count is of what was asked about: {scoped}");
    let (whole, _) = state.json(&["search", "common", "--limit", "200"]);
    assert_eq!(whole["data"]["coverage"]["hits_total"], 60);
    let (plain, _, _) = state.text(&["search", "common", "--limit", "3"]);
    assert!(plain.lines().next().unwrap().contains("60 hits in all"), "{plain}");
}

/// Failure: on an edited tab the index answers with the file's page numbers rather than the tab's, misses what's been added, or calls a page from an added file its own. **Page numbers follow the edit order, and added files are marked**, the same as reading the pages.
#[test]
fn an_edited_tab_keeps_its_page_numbers_and_marks_added_files() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("edited.pdf", 6);
    let extra = TempPdf::new(
        "added.pdf",
        &pages(&[
            Page::new(text_lines(&["A page from elsewhere with the word common"])),
            Page::new(text_lines(&["Second added page"])),
        ]),
    );
    state.json(&["open", path(&file)]);
    state.json(&["index"]);
    wait_done(&state, "active");
    let (moved, code) = state.json(&["page", "move", "6", "--to", "1"]);
    assert_eq!(code, 0, "{moved}");
    let (deleted, code) = state.json(&["page", "delete", "3"]);
    assert_eq!(code, 0, "{deleted}");
    let (inserted, code) = state.json(&["page", "insert", path(&extra), "--at", "2"]);
    assert_eq!(code, 0, "{inserted}");
    // The added file has no index yet, so this one's mixed, and it starts one.
    let (mixed, code) = state.json(&["search", "common"]);
    assert_eq!(code, 0, "{mixed}");
    assert_eq!(mixed["data"]["route"], "mixed", "{mixed}");
    wait_done(&state, "active");
    let answer = index_matches_scan(&state, &["search", "elsewhere"]);
    let hit = &answer["data"]["hits"][0];
    assert_eq!(
        (hit["page"].as_u64(), hit["source_page"].as_u64(), hit["from_added_file"].as_bool()),
        (Some(2), Some(1), Some(true)),
        "{hit}"
    );
    assert_eq!(hit["untrusted"]["added_file"], "added.pdf", "{hit}");
    let moved_hit = index_matches_scan(&state, &["search", "page 6 of"]);
    assert_eq!(
        (
            moved_hit["data"]["hits"][0]["page"].as_u64(),
            moved_hit["data"]["hits"][0]["source_page"].as_u64()
        ),
        (Some(1), Some(6)),
        "{moved_hit}"
    );
    let gone = index_matches_scan(&state, &["search", "page 2 of"]);
    assert_eq!(gone["data"]["hits"].as_array().unwrap().len(), 0, "a deleted page isn't searched: {gone}");
    index_matches_scan(&state, &["search", "common", "--limit", "2"]);
    index_matches_scan(&state, &["search", "common", "--pages", "2-4"]);
    let (undone, code) = state.json(&["undo"]);
    assert_eq!(code, 0, "{undone}");
    let back = index_matches_scan(&state, &["search", "common", "--limit", "200"]);
    assert_eq!(back["data"]["coverage"]["hits_total"], 10, "{back}");
}

/// Failure: a file replaced under an open tab answers from its old text, even with its size and time put back. **A replaced file gets a new corpus, and never a stale answer.** Tried both ways a file gets replaced: in place, and moved in.
#[test]
fn a_replaced_file_doesnt_answer_from_old_text() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let one_page = |word: &str| pages(&[Page::new(text_lines(&[&format!("Only this word: {word}")]))]);
    let file = TempPdf::new("replaced.pdf", &one_page("oldword"));
    state.json(&["open", path(&file)]);
    state.json(&["index"]);
    wait_done(&state, "active");
    let (old, _) = state.json(&["search", "oldword"]);
    assert_eq!(
        (old["data"]["route"].as_str(), old["data"]["hits"].as_array().unwrap().len()),
        (Some("index"), 1),
        "{old}"
    );

    // In place, the same length, and the old time put back.
    let when = std::fs::metadata(&file.0).unwrap().modified().unwrap();
    std::fs::write(&file.0, one_page("newword")).unwrap();
    std::fs::File::options().write(true).open(&file.0).unwrap().set_modified(when).unwrap();
    let (stale, code) = state.json(&["search", "oldword"]);
    assert_eq!(code, 0, "{stale}");
    assert_eq!(stale["data"]["route"], "scan", "there's no corpus for this version yet: {stale}");
    assert_eq!(
        stale["data"]["hits"].as_array().unwrap().len(),
        0,
        "old text answered for a replaced file: {stale}"
    );
    wait_done(&state, "active");
    let (fresh, _) = state.json(&["search", "newword"]);
    assert_eq!(
        (fresh["data"]["route"].as_str(), fresh["data"]["hits"].as_array().unwrap().len()),
        (Some("index"), 1),
        "{fresh}"
    );
    let (stale, _) = state.json(&["search", "oldword"]);
    assert_eq!(stale["data"]["hits"].as_array().unwrap().len(), 0, "{stale}");

    // Moved into its place.
    let other = file.0.with_extension("other");
    std::fs::write(&other, one_page("oldword")).unwrap();
    std::fs::File::options().write(true).open(&other).unwrap().set_modified(when).unwrap();
    std::fs::rename(&other, &file.0).unwrap();
    let (back, _) = state.json(&["search", "newword"]);
    assert_eq!(back["data"]["hits"].as_array().unwrap().len(), 0, "{back}");
}

/// Starts the real helper by hand, on a job as the session would give it. Returns once it's written at least `until` pages.
fn start_helper(file: &Path, root: &Path, key: CorpusKey) -> std::process::Child {
    let exe = Path::new(common::bin()).with_file_name("recto-indexer");
    assert!(exe.is_file(), "{} isn't built. `tools/check.sh` builds it.", exe.display());
    let mut child = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let line = serde_json::json!({ "file": file, "root": root, "key": key.hex() }).to_string() + "\n";
    child.stdin.as_mut().unwrap().write_all(line.as_bytes()).unwrap();
    child
}

fn pages_done(root: &Path, key: CorpusKey) -> u32 {
    match recto_index::corpus::disk_state(root, key) {
        recto_index::corpus::DiskState::Partial { pages_done } => pages_done,
        _ => 0,
    }
}

/// Failure: a killed indexer leaves something unusable: a search answers from half a document, the session refuses to go on, or the next build starts over or comes out different. **It's killed with SIGKILL partway, for real,** and then: the search still reads pages and is right, and the next build goes on from there and comes out the same as an unbroken one.
#[test]
fn a_killed_indexer_leaves_a_usable_state() {
    let state = State::at(Level::Control);
    let file = doc("killed.pdf", 400);
    let key = CorpusKey::of_file(&file.0).unwrap();
    let root = state.paths.corpus.clone();
    std::fs::create_dir_all(state.root.as_path()).unwrap();
    state.paths.prepare().unwrap();
    let mut child = start_helper(&file.0, &root, key);
    let deadline = Instant::now() + Duration::from_secs(120);
    while pages_done(&root, key) < 20 {
        assert!(
            Instant::now() < deadline && child.try_wait().unwrap().is_none(),
            "the helper ended before it was killed"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let done = pages_done(&root, key);
    assert!((20..400).contains(&done), "it was meant to be killed partway: {done}");

    let _serving = state.serve();
    state.json(&["open", path(&file)]);
    let (status, code) = state.json(&["index", "status"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(status["data"]["state"], "not_started", "{status}");
    let (found, code) = state.json(&["search", "page 390 of", "--scan"]);
    assert_eq!(code, 0, "{found}");
    assert_eq!(found["data"]["hits"].as_array().unwrap().len(), 1, "{found}");
    let (asked, code) = state.json(&["index"]);
    assert_eq!(code, 0, "{asked}");
    assert!(matches!(asked["data"]["state"].as_str(), Some("queued" | "building" | "ready")), "{asked}");
    wait_done(&state, "active");
    let (found, _) = state.json(&["search", "page 390 of"]);
    assert_eq!(
        (found["data"]["route"].as_str(), found["data"]["hits"].as_array().unwrap().len()),
        (Some("index"), 1),
        "{found}"
    );
    let (found, _) = state.json(&["search", "page 1 of"]);
    assert_eq!(
        found["data"]["hits"].as_array().unwrap().len(),
        1,
        "pages from before the kill are there too: {found}"
    );
    let (all, _) = state.json(&["search", "common", "--limit", "1"]);
    assert_eq!(all["data"]["coverage"]["hits_total"], 800, "nothing doubled and nothing lost: {all}");
}

/// Failure: closing a session leaves its indexer running, on a file nobody's looking at, with nothing to stop it. And a session killed outright does the same. It goes when whoever started it goes, however that happens.
#[test]
fn an_indexer_doesnt_outlive_its_session() {
    let state = State::at(Level::Control);
    state.paths.prepare().unwrap();
    let file = doc("outlive.pdf", 400);
    let key = CorpusKey::of_file(&file.0).unwrap();
    let mut child = start_helper(&file.0, &state.paths.corpus, key);
    let deadline = Instant::now() + Duration::from_secs(120);
    while pages_done(&state.paths.corpus, key) < 2 {
        assert!(Instant::now() < deadline, "it never started");
        std::thread::sleep(Duration::from_millis(2));
    }
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(Instant::now() < deadline, "it's still there after its parent's end of stdin closed");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(pages_done(&state.paths.corpus, key) < 400, "it was meant to stop partway");
}

/// Failure: an adversarial page ends the index, stalls a build for good, or reads as "nothing found", so an agent says a fact isn't in a document it never read. It fails its own page, and a page after it is found, in a run of a few seconds rather than minutes.
#[test]
fn an_adversarial_page_fails_alone_and_says_so() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("bomb.pdf", &form_bomb());
    state.json(&["open", path(&file)]);
    let started = Instant::now();
    state.json(&["index"]);
    let done = wait_done(&state, "active");
    println!("{done}");
    let (found, code) = state.json(&["search", "ordinary page"]);
    assert_eq!(code, 0, "{found}");
    assert_eq!(
        found["data"]["route"], "mixed",
        "the bomb's page is read again, in case it was the moment: {found}"
    );
    assert_eq!(found["data"]["hits"].as_array().unwrap().len(), 1, "the page after the bomb: {found}");
    let hit = &found["data"]["hits"][0];
    assert_eq!(hit["page"], 2);
    // The bomb's page has no text, and it can't be drawn for the check. Which of those shows is the renderer's business: it must show one.
    let said = done["data"]["failed_pages"].as_u64().unwrap()
        + done["data"]["hidden_unchecked_pages"].as_u64().unwrap()
        + done["data"]["no_text_pages"].as_u64().unwrap();
    assert!(said >= 1, "the bomb's page must show up in what's said: {done}");
    assert!(started.elapsed() < Duration::from_secs(120), "{:?}", started.elapsed());
}

/// Failure: opening a file starts an indexer, so 200 tabs start 200 of them, or leave a process, a thread or a folder behind. **Only asking starts one.** And once it's done there's nothing left of it in the session.
#[test]
fn opening_tabs_indexes_nothing_and_asking_leaves_nothing_behind() {
    use recto_platform::local::process_counters;
    let state = State::at(Level::Control);
    let serving = state.serve();
    let file = doc("tabs.pdf", 3);
    let folder = file.0.parent().unwrap().join("links");
    std::fs::create_dir_all(&folder).unwrap();
    for at in 0..200 {
        let link = folder.join(format!("t{at}.pdf"));
        std::fs::copy(&file.0, &link).unwrap();
        let (opened, code) = state.json(&["open", link.to_str().unwrap()]);
        assert_eq!(code, 0, "{opened}");
    }
    assert!(!state.paths.corpus.exists(), "opening 200 tabs wrote to the corpus folder");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(process_counters(serving.pid()).unwrap().threads, 1, "200 open tabs hold a thread");
    let (status, _) = state.json(&["index", "status"]);
    assert_eq!(
        (status["data"]["indexers_running"].as_u64(), status["data"]["indexers_waiting"].as_u64()),
        (Some(0), Some(0)),
        "{status}"
    );
    assert_eq!(status["data"]["state"], "not_started", "{status}");

    // All 200 are one file, so asking for each is one build.
    for tab in [1, 50, 100, 150, 200] {
        let (searched, code) = state.json(&["search", "common", "--tab", &format!("#{tab}")]);
        assert_eq!(code, 0, "{searched}");
    }
    for tab in [1, 50, 100, 150, 200] {
        let done = wait_done(&state, &format!("#{tab}"));
        assert_eq!(done["data"]["pages"], 3, "{done}");
    }
    let corpora = std::fs::read_dir(&state.paths.corpus)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_str().and_then(CorpusKey::from_hex).is_some())
        .count();
    assert_eq!(corpora, 5, "only what was asked for, one for each file");
    let (status, _) = state.json(&["index", "status", "--tab", "#200"]);
    assert_eq!(status["data"]["state"], "ready", "{status}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while process_counters(serving.pid()).unwrap().threads > 1 {
        assert!(Instant::now() < deadline, "a thread stayed behind after indexing");
        std::thread::sleep(Duration::from_millis(10));
    }
    for at in 0..200 {
        let _ = std::fs::remove_file(folder.join(format!("t{at}.pdf")));
    }
    let _ = std::fs::remove_dir(&folder);
}

/// Failure: `wait indexing-done` waits out its time, or never hears the index finish, so an agent polls. It starts the build if nobody has, and a `watch` hears it too.
#[test]
fn waiting_for_the_index_starts_it_and_hears_it_finish() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("waited.pdf", 5);
    state.json(&["open", path(&file)]);
    let (watch_tx, watch_rx) = std::sync::mpsc::channel();
    let mut watch = state.command();
    let output = watch
        .args(["watch", "--events", "indexing", "--count", "2", "--timeout", "60s", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::spawn(move || {
        let out = output.wait_with_output().unwrap();
        let _ = watch_tx.send(String::from_utf8(out.stdout).unwrap());
    });
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    let done = wait_done(&state, "active");
    assert!(started.elapsed() < Duration::from_secs(120));
    assert_eq!(done["data"]["pages"], 5, "{done}");
    assert_eq!(done["data"]["folder"], Value::Null, "a build writes the text once: {done}");
    let (made, code) = state.json(&["index", "--agent"]);
    assert_eq!(code, 0, "{made}");
    let folder = PathBuf::from(made["data"]["folder"].as_str().unwrap());
    assert!(made["data"]["agent_bytes"].as_u64().unwrap() > 0, "{made}");
    assert!(folder.join("manifest.json").is_file() && folder.join("pages/0005.md").is_file(), "{done}");
    assert!(
        folder.starts_with(&state.paths.corpus),
        "QA state must keep it in the temp folder: {}",
        folder.display()
    );
    assert!(
        !folder.to_string_lossy().contains("waited"),
        "a document's name is in the folder's path: {}",
        folder.display()
    );
    let said = watch_rx.recv_timeout(Duration::from_secs(60)).unwrap();
    assert!(said.contains("indexing_started") && said.contains("indexing_finished"), "{said}");
    // Already finished: it's said at once.
    let again = Instant::now();
    wait_done(&state, "active");
    assert!(again.elapsed() < Duration::from_secs(5));
}

/// Failure: a level lets a command touch an index it shouldn't. An agent at Off reads nothing about it, and clearing, which throws away what took work to build, needs Control.
#[test]
fn levels_hold_on_the_index_commands() {
    let state = State::at(Level::Off);
    let _serving = state.serve();
    let file = doc("levels.pdf", 2);
    for args in [
        &["index"][..],
        &["index", "status"][..],
        &["index", "clear"][..],
        &["wait", "indexing-done"][..],
    ] {
        let (refused, code) = state.json(args);
        assert_eq!(code, exit::PERMISSION, "{args:?}: {refused}");
    }
    state.set_level(Level::Control);
    state.json(&["open", path(&file)]);
    state.set_level(Level::ReadOnly);
    let (asked, code) = state.json(&["index"]);
    assert_eq!(code, 0, "{asked}");
    let (refused, code) = state.json(&["index", "clear"]);
    assert_eq!(code, exit::PERMISSION, "{refused}");
    wait_done(&state, "active");
    state.set_level(Level::Control);
    let (cleared, code) = state.json(&["index", "clear"]);
    assert_eq!((code, cleared["data"]["cleared"].as_u64()), (0, Some(1)), "{cleared}");
    let (status, _) = state.json(&["index", "status"]);
    assert_eq!(status["data"]["state"], "not_started", "{status}");
    let (all, code) = state.json(&["index", "clear", "--all"]);
    assert_eq!((code, all["data"]["cleared"].as_u64()), (0, Some(0)), "{all}");
}

/// Failure: a corpus damaged on disk answers with its bad bytes, or takes the search down with it. It isn't believed, the search is right, and a good one replaces it.
#[test]
fn a_damaged_corpus_answers_by_reading_the_pages() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("damaged.pdf", 4);
    state.json(&["open", path(&file)]);
    state.json(&["index"]);
    wait_done(&state, "active");
    let key = CorpusKey::of_file(&file.0).unwrap();
    let paths = recto_index::corpus::CorpusPaths::new(&state.paths.corpus, key);
    let pack = paths.app.join("text.pack");
    let mut bytes = std::fs::read(&pack).unwrap();
    bytes[0] = 0xFF;
    std::fs::write(&pack, bytes).unwrap();
    let (found, code) = state.json(&["search", "page 1 of"]);
    assert_eq!(code, 0, "{found}");
    assert_eq!(found["data"]["hits"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["data"]["route"], "scan", "{found}");
    assert_eq!(found["data"]["index"]["state"], "failed", "{found}");
    wait_done(&state, "active");
    let (found, _) = state.json(&["search", "page 1 of"]);
    assert_eq!(
        (found["data"]["route"].as_str(), found["data"]["hits"].as_array().unwrap().len()),
        (Some("index"), 1),
        "a good one takes its place: {found}"
    );
}

/// Failure: `recto local`, used by QA, starts a build it can't finish, or doesn't use what's built. Its route reads what's there and starts nothing.
#[test]
fn the_local_route_reads_an_index_and_builds_none() {
    let state = State::at(Level::Control);
    let file = doc("local.pdf", 3);
    let (scan, code) = state.json(&["local", path(&file), "search", "common"]);
    assert_eq!(code, 0, "{scan}");
    assert_eq!(scan["data"]["route"], "scan", "{scan}");
    assert!(scan["data"]["index"]["note"].as_str().unwrap().contains("doesn't build"), "{scan}");
    assert!(!state.paths.corpus.exists());
    let (refused, code) = state.json(&["local", path(&file), "index", "clear"]);
    assert_eq!((code, refused["data"]["cleared"].as_u64()), (0, Some(0)), "{refused}");
    {
        let _serving = state.serve();
        state.json(&["open", path(&file)]);
        state.json(&["index"]);
        wait_done(&state, "active");
    }
    let (indexed, code) = state.json(&["local", path(&file), "search", "common"]);
    assert_eq!(code, 0, "{indexed}");
    assert_eq!(indexed["data"]["route"], "index", "{indexed}");
}

/// Failure: `budget index` reports nothing real, or leaves its folder behind. It's what the coordinator measures with.
#[test]
fn the_budget_reports_and_cleans_up() {
    let state = State::at(Level::Control);
    state.paths.prepare().unwrap();
    let file = doc("budget.pdf", 5);
    let (answer, code) = state.json(&[
        "budget",
        "index",
        "--file",
        path(&file),
        "--runs",
        "2",
        "--foreground",
        "--queries",
        "common,zzzqqqxx",
    ]);
    assert_eq!(code, 0, "{answer}");
    let data = &answer["data"];
    assert_eq!(
        (data["build"]["state"].as_str(), data["build"]["pages"].as_u64()),
        (Some("ready"), Some(5)),
        "{answer}"
    );
    assert!(data["build"]["worker_peak_bytes"].as_u64().unwrap() > 0, "{answer}");
    assert!(data["build"]["helper_peak_bytes"].as_u64().unwrap() > 0, "{answer}");
    assert!(data["build"]["corpus_bytes"].as_u64().unwrap() > 0, "{answer}");
    let searches = data["searches"].as_array().unwrap();
    assert_eq!(searches.len(), 2);
    assert_eq!(
        (searches[0]["index"]["route"].as_str(), searches[0]["scan"]["route"].as_str()),
        (Some("index"), Some("scan")),
        "{answer}"
    );
    assert_eq!(searches[0]["index"]["hits_total"], 10, "{answer}");
    let left: Vec<String> = std::fs::read_dir(&state.root)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("budget"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    // Failure: a run killed partway leaves a document's text under Application Support for good, where nothing looks: nothing clears it, and the table says it's temp. It's in the OS's own temp folder, and it's gone after.
    let folder = PathBuf::from(data["corpus_folder"].as_str().unwrap());
    assert!(
        folder.starts_with(recto_platform::local::system_temp_dir().unwrap().canonicalize().unwrap())
            || folder.starts_with(recto_platform::local::system_temp_dir().unwrap()),
        "{}",
        folder.display()
    );
    assert!(!folder.exists(), "{}", folder.display());
    state.set_level(Level::ReadOnly);
    let (refused, code) = state.json(&["budget", "index", "--file", path(&file)]);
    assert_eq!(code, exit::PERMISSION, "{refused}");
}

/// Every kind of search on one tab, from the index and from its pages, each held to the other. Returns how many of each.
fn compare_every_mode(state: &State, tab: &str) -> (u32, u32, u32) {
    let (mut plain, mut patterns, mut ranked) = (0, 0, 0);
    for query in ["the", "of the", "zzzqqqxx", "simulation-based", "data", "must not", "figure", "a"] {
        for limit in ["20", "200"] {
            index_matches_scan(state, &["search", query, "--tab", tab, "--limit", limit]);
            plain += 1;
        }
    }
    for pattern in [
        r"\bthe\b",
        r"must\s+not",
        r"(?-i)[A-Z]{3,}",
        r"\d{4}",
        r"^figure",
        r"zzzqqqxx",
        r"[a-z]+-[a-z]+",
    ] {
        index_matches_scan(state, &["search", pattern, "--regex", "--tab", tab, "--limit", "50"]);
        patterns += 1;
    }
    for words in ["the", "simulation data model", "zzzqqqxx", "must not", "figure results", "数学"] {
        index_matches_scan(state, &["search", words, "--rank", "--tab", tab]);
        ranked += 1;
    }
    (plain, patterns, ranked)
}

/// Failure: reading from the index differs from reading the pages on what real files have that a plain test page doesn't: a word drawn late and put back on its line, a word broken by a hyphen at a line's end, text with no ink, and an empty page. **This one always runs,** so the check holds this with no folder of PDFs. The full run over real files is `tools/check.sh --corpus`.
#[test]
fn the_hard_cases_answer_the_same_every_way() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let late_word = b"BT /F1 12 Tf 72 700 Td (A recipient) Tj ET BT /F1 12 Tf 72 680 Td (ignore the header figure of the data) Tj ET BT /F1 12 Tf 131 700 Td (must not) Tj ET".to_vec();
    let list = [
        Page::new(late_word),
        Page::new(text_lines(&[
            "A simulation-",
            "based data-",
            "driven model of the figure from 2019",
            "THE RESULTS and a word broken at simula-",
            "tion here",
        ])),
        Page::new(hidden_text_page()),
        Page::new(Vec::new()),
        Page::new(text_lines(&["Plain words, the data and the figure, and 数学"])),
    ];
    let file = TempPdf::new("hard-cases.pdf", &pages(&list));
    state.json(&["open", path(&file)]);
    wait_done(&state, "active");
    let (late, _) = state.json(&["text", "--page", "1"]);
    assert!(
        late["data"]["untrusted"]["text"].as_str().unwrap().contains("A recipient must not"),
        "this page is meant to need its line put back: {late}"
    );
    let (plain, patterns, ranked) = compare_every_mode(&state, "#1");
    assert!(plain > 0 && patterns > 0 && ranked > 0);
}

/// Failure: over real pages, the index answers differently from reading the pages: line-order repair, hidden text, the normaliser and hyphen spellings are all in play here, as they aren't in a test file. Set `RECTO_BENCH100` to run; `RECTO_INDEX_ALL` takes in every file there, rather than a handful of kinds.
#[test]
fn real_pages_answer_the_same_from_the_index() {
    let Some(folder) = std::env::var_os("RECTO_BENCH100") else {
        // Asked for by name, with nothing to read: that must not read as a pass. `the_hard_cases_answer_the_same_every_way` is what always runs.
        assert!(
            std::env::var_os("RECTO_INDEX_ALL").is_none(),
            "RECTO_INDEX_ALL is set and RECTO_BENCH100 isn't, so nothing would be compared"
        );
        eprintln!("RECTO_BENCH100 isn't set, so no corpus files were read.");
        return;
    };
    let folder = PathBuf::from(folder);
    let mut files: Vec<PathBuf> = if std::env::var_os("RECTO_INDEX_ALL").is_some() {
        std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "pdf"))
            .collect()
    } else {
        [
            "standards_ietf_rfc9110.pdf",
            "policy_worldbank_wbD34442277.pdf",
            "math_AG_2607.28411.pdf",
            "gov_nasa_ntrs20240002628.pdf",
        ]
        .iter()
        .map(|name| folder.join(name))
        .filter(|path| path.is_file())
        .collect()
    };
    files.sort();
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let (mut documents, mut compared, mut compared_regex, mut compared_ranked) = (0, 0, 0, 0);
    for file in files {
        let text = file.to_str().unwrap().to_string();
        let (opened, code) = state.json(&["open", &text]);
        if code != 0 {
            eprintln!("{text} didn't open: {opened}");
            continue;
        }
        let tab = format!("#{}", opened["data"]["tab"]);
        let (done, code) = state.json(&["wait", "indexing-done", "--tab", &tab, "--timeout", "1h"]);
        if code != 0 {
            eprintln!("{text} wasn't indexed: {done}");
            continue;
        }
        eprintln!(
            "{text}: {} pages, {} failed, {} flagged, {} bytes",
            done["data"]["pages"], done["data"]["failed_pages"], done["data"]["hidden_flagged_pages"], done["data"]["corpus_bytes"]
        );
        documents += 1;
        let (plain, patterns, ranked) = compare_every_mode(&state, &tab);
        compared += plain;
        compared_regex += patterns;
        compared_ranked += ranked;
        state.json(&["close", &tab]);
    }
    eprintln!("{documents} documents, {compared} plain searches, {compared_regex} patterns and {compared_ranked} ranked searches compared");
}

/// Failure: a password-protected file's text ends up on disk, or it reads as "still building" for ever. It's refused, with its own exit code, and nothing's there but the reason.
#[test]
fn a_password_protected_file_leaves_no_text() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("was-open.pdf", 2);
    state.json(&["open", path(&file)]);
    // A tab can't open such a file, so this is the way one gets here: replaced under an open tab.
    std::fs::write(&file.0, common::password_protected()).unwrap();
    let (refused, code) = state.json(&["wait", "indexing-done", "--timeout", "60s"]);
    assert_eq!((code, refused["error"]["code"].as_str()), (exit::PROTECTED, Some("not_indexed")), "{refused}");
    let key = CorpusKey::of_file(&file.0).unwrap();
    let paths = recto_index::corpus::CorpusPaths::new(&state.paths.corpus, key);
    assert!(!paths.app.join("text.pack").exists(), "there's text on disk");
    assert!(
        !paths.pages.exists() || std::fs::read_dir(&paths.pages).is_ok_and(|mut pages| pages.next().is_none()),
        "there are page files on disk"
    );
    let (searched, code) = state.json(&["search", "anything"]);
    assert_eq!(code, exit::PROTECTED, "{searched}");
}

/// Reads a real file, except that one page fails, as a worker lost at the wrong moment would make it.
struct OnePageFails {
    doc: recto_render_service::DocRenderer,
    file: PathBuf,
    key: CorpusKey,
    fails: u32,
}

impl recto_index::corpus::PageFeed for OnePageFails {
    fn page_count(&mut self) -> Result<u32, recto_index::corpus::FeedError> {
        self.doc.page_count().map_err(|_| recto_index::corpus::FeedError::Cant("couldnt_open"))
    }
    fn read(&mut self, page: u32) -> Result<recto_index::corpus::FedPage, recto_index::corpus::FeedError> {
        if page == self.fails {
            return Err(recto_index::corpus::FeedError::Page(recto_index::corpus::FailReason::Stopped));
        }
        let read = self.doc.page_characters(page, true, true).unwrap();
        Ok(recto_index::corpus::FedPage {
            chars: read.chars,
            boxes: read.boxes,
            hidden_checked: read.hidden_checked,
        })
    }
    fn still_the_same_file(&mut self) -> bool {
        CorpusKey::of_file(&self.file).is_ok_and(|now| now == self.key)
    }
}

/// Builds a file's corpus here, with one page recorded as failed.
fn corpus_with_a_failed_page(state: &State, file: &TempPdf, page: u32) {
    let key = CorpusKey::of_file(&file.0).unwrap();
    let doc = recto_render_service::DocRenderer::new(
        &file.0,
        None,
        recto_render_service::ServiceConfig::for_worker(Path::new(common::bin()).with_file_name("recto-render-service")).unwrap(),
        recto_render_service::Policy::default(),
    )
    .unwrap();
    let mut feed = OnePageFails {
        doc,
        file: file.0.clone(),
        key,
        fails: page,
    };
    state.paths.prepare().unwrap();
    let built = recto_index::corpus::build(
        &state.paths.corpus,
        key,
        &mut feed,
        &recto_index::corpus::DocMeta::default(),
        &recto_index::corpus::BuildLimits::default(),
    )
    .unwrap();
    assert!(matches!(built, recto_index::corpus::BuildOutcome::Ready { .. }), "{built:?}");
}

/// Failure: a page that failed only because of the moment it was read, such as a worker lost to memory pressure or a deadline under load, reads as failed for as long as the file stays the same, though `--scan` reads it perfectly well, and it's listed in `failed_pages`, so its words can't be found. A failed page in a corpus is only what happened then, so a search reads it now.
#[test]
fn a_search_reads_a_page_the_build_couldnt() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("failed.pdf", 5);
    corpus_with_a_failed_page(&state, &file, 2);
    state.json(&["open", path(&file)]);
    let (found, code) = state.json(&["search", "page 3 of"]);
    assert_eq!(code, 0, "{found}");
    assert_eq!(found["data"]["coverage"]["failed_pages"], serde_json::json!([]), "{found}");
    assert_eq!(found["data"]["hits"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["data"]["hits"][0]["page"], 3, "{found}");
    assert_eq!(found["data"]["route"], "mixed", "{found}");
    assert!(
        found["data"]["index"]["note"].as_str().unwrap().contains("read again just now"),
        "a page read live isn't said to have come from the index: {found}"
    );
    let (scanned, code) = state.json(&["search", "page 3 of", "--scan"]);
    assert_eq!(code, 0, "{scanned}");
    assert_eq!(the_same_answer(found), the_same_answer(scanned));
    index_or_mixed_matches_scan(&state, &["search", "common"]);
    index_or_mixed_matches_scan(&state, &["search", "common", "--limit", "200"]);
    index_or_mixed_matches_scan(&state, &["search", "zzzqqqxx"]);
}

/// The same as [`index_matches_scan`], for a corpus some of whose pages have to be read live.
fn index_or_mixed_matches_scan(state: &State, args: &[&str]) {
    let (indexed, code) = state.json(args);
    assert_eq!(code, 0, "{args:?}: {indexed}");
    let mut with_scan = args.to_vec();
    with_scan.push("--scan");
    let (scanned, code) = state.json(&with_scan);
    assert_eq!(code, 0, "{args:?}: {scanned}");
    totals_agree(&indexed, &scanned, args);
    assert_eq!(the_same_answer(indexed), the_same_answer(scanned), "{args:?}");
}

/// Every byte under a folder, read as text, for looking for a document's words.
fn all_text_under(folder: &Path) -> String {
    let mut found = String::new();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else { continue };
        for entry in entries.flatten() {
            if entry.file_type().unwrap().is_dir() {
                stack.push(entry.path());
            } else if let Ok(bytes) = std::fs::read(entry.path()) {
                found.push_str(&String::from_utf8_lossy(&bytes));
            }
        }
    }
    found
}

/// Failure: a deleted PDF's text stays on disk for good, and the rule of 90 days never fires, because only a build that succeeds ever makes room. A session that only reads what's already built, or whose builds all fail, must still clear what's past its age, the first time anyone asks.
#[test]
fn a_deleted_pdfs_text_goes_when_its_time_is_up_with_nothing_being_built() {
    let state = State::at(Level::Control);
    let deleted = TempPdf::new(
        "deleted.pdf",
        &pages(&[Page::new(text_lines(&["Words only the deleted file has: vanishingword"]))]),
    );
    let kept = doc("kept.pdf", 2);
    let (deleted_key, kept_key);
    {
        let _serving = state.serve();
        state.json(&["open", path(&deleted)]);
        state.json(&["index"]);
        wait_done(&state, "active");
        state.json(&["open", path(&kept)]);
        state.json(&["index"]);
        wait_done(&state, "active");
        deleted_key = CorpusKey::of_file(&deleted.0).unwrap();
        kept_key = CorpusKey::of_file(&kept.0).unwrap();
    }
    let root = state.paths.corpus.clone();
    assert!(all_text_under(&root).contains("vanishingword"));
    std::fs::remove_file(&deleted.0).unwrap();
    // Its last use was 100 days ago, and the file's gone.
    let used = recto_index::corpus::CorpusPaths::new(&root, deleted_key).app.join("used");
    let long_ago = std::time::SystemTime::now() - Duration::from_secs(100 * 24 * 3600);
    std::fs::File::options().write(true).open(&used).unwrap().set_modified(long_ago).unwrap();

    let _serving = state.serve();
    state.json(&["open", path(&kept)]);
    let (found, code) = state.json(&["search", "common"]);
    assert_eq!(code, 0, "{found}");
    assert_eq!(found["data"]["route"], "index", "nothing here needs building: {found}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while all_text_under(&root).contains("vanishingword") {
        assert!(Instant::now() < deadline, "a deleted file's text is still there, 100 days after its last use");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(root.join(kept_key.hex()).exists(), "what's in use stays");
}

/// Failure: a saved edit that removes something, a redaction, or a file that's since been encrypted, gets a new key, and nothing connects it to the old one. So the old version's plain text stays, for as long as the cache keeps it, after a new version has been indexed and after `index clear`. That matters most for just the edits meant to take something out.
#[test]
fn an_old_versions_text_goes_when_a_new_version_is_indexed_or_the_index_is_cleared() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let one_page = |word: &str| pages(&[Page::new(text_lines(&[&format!("This page says {word} once")]))]);
    let file = TempPdf::new("versions.pdf", &one_page("redactedword"));
    let root = state.paths.corpus.clone();
    state.json(&["open", path(&file)]);
    state.json(&["index"]);
    wait_done(&state, "active");
    assert!(all_text_under(&root).contains("redactedword"));

    // A new version, saved in place: indexing it takes the old one's text away.
    std::fs::write(&file.0, one_page("secondword")).unwrap();
    wait_done(&state, "active");
    let text = all_text_under(&root);
    assert!(text.contains("secondword"), "{text}");
    assert!(
        !text.contains("redactedword"),
        "the old version's text is still on disk after the new one was indexed"
    );

    // A new version moved into place, not indexed: clearing takes what's there.
    let other = file.0.with_extension("new");
    std::fs::write(&other, one_page("thirdword")).unwrap();
    std::fs::rename(&other, &file.0).unwrap();
    let (cleared, code) = state.json(&["index", "clear"]);
    assert_eq!(code, 0, "{cleared}");
    assert!(
        !all_text_under(&root).contains("secondword"),
        "clearing left the text of the version before this one"
    );

    // Renamed, which the path can't connect but the file itself can.
    wait_done(&state, "active");
    assert!(all_text_under(&root).contains("thirdword"));
    let renamed = file.0.with_file_name("versions-renamed.pdf");
    std::fs::rename(&file.0, &renamed).unwrap();
    state.json(&["close"]);
    state.json(&["open", renamed.to_str().unwrap()]);
    std::fs::write(&renamed, one_page("fourthword")).unwrap();
    wait_done(&state, "active");
    let text = all_text_under(&root);
    assert!(text.contains("fourthword") && !text.contains("thirdword"), "{text}");

    // Gone altogether: it can't be looked at, and clearing still finds its text through its path.
    std::fs::remove_file(&renamed).unwrap();
    let (cleared, code) = state.json(&["index", "clear"]);
    assert_eq!(code, 0, "{cleared}");
    assert!(!all_text_under(&root).contains("fourthword"), "clearing left a deleted file's text");
    let text = all_text_under(&root);
    assert!(!text.contains("versions"), "a sidecar holds a file's name: {text}");
    std::fs::write(&file.0, b"").unwrap();
}

/// Failure: `index clear` says it cleared a file that's waiting its turn, then the queue writes its text anyway, and one being built reports `being_built` and finishes. When someone asks for text to be gone, what's on its way must stop too.
#[test]
fn clearing_stops_what_is_on_its_way() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let running = doc("running.pdf", 1200);
    let waiting = TempPdf::new(
        "waiting.pdf",
        &pages(&[Page::new(text_lines(&["Words only the waiting file has: queuedword"]))]),
    );
    let root = state.paths.corpus.clone();
    let running_key = CorpusKey::of_file(&running.0).unwrap();
    state.json(&["open", path(&running)]);
    state.json(&["index"]);
    let deadline = Instant::now() + Duration::from_secs(120);
    while pages_done(&root, running_key) < 5 {
        assert!(Instant::now() < deadline, "it never started");
        std::thread::sleep(Duration::from_millis(2));
    }
    state.json(&["open", path(&waiting)]);
    let (asked, _) = state.json(&["index"]);
    assert_eq!(asked["data"]["state"], "queued", "{asked}");
    let (cleared, code) = state.json(&["index", "clear", "--tab", "waiting"]);
    assert_eq!(code, 0, "{cleared}");
    let (status, _) = state.json(&["index", "status", "--tab", "waiting"]);
    assert_eq!(status["data"]["state"], "not_started", "it's still on its way after being cleared: {status}");

    let (cleared, code) = state.json(&["index", "clear", "--tab", "running"]);
    assert_eq!(code, 0, "{cleared}");
    assert_eq!(
        (cleared["data"]["cleared"].as_u64(), cleared["data"]["being_built"].as_u64()),
        (Some(1), Some(0)),
        "{cleared}"
    );
    let (status, _) = state.json(&["index", "status", "--tab", "running"]);
    assert_eq!(
        (status["data"]["state"].as_str(), status["data"]["indexers_running"].as_u64()),
        (Some("not_started"), Some(0)),
        "{status}"
    );
    std::thread::sleep(Duration::from_millis(500));
    let text = all_text_under(&root);
    assert!(!text.contains("alpha document"), "the build that was cleared carried on writing");
    assert!(!text.contains("queuedword"), "the file that was cleared got written anyway");
}

/// Failure: lowering agent access to Off leaves up to 16 waiting files and the running one writing documents' text, after the user has said no. It stops, and what's waiting goes.
#[test]
fn lowering_to_off_stops_indexing() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let running = doc("off-running.pdf", 1200);
    let waiting = TempPdf::new(
        "off-waiting.pdf",
        &pages(&[Page::new(text_lines(&["Words only the waiting file has: offqueuedword"]))]),
    );
    let root = state.paths.corpus.clone();
    let running_key = CorpusKey::of_file(&running.0).unwrap();
    state.json(&["open", path(&running)]);
    state.json(&["index"]);
    let deadline = Instant::now() + Duration::from_secs(120);
    while pages_done(&root, running_key) < 5 {
        assert!(Instant::now() < deadline, "it never started");
        std::thread::sleep(Duration::from_millis(2));
    }
    state.json(&["open", path(&waiting)]);
    state.json(&["index"]);
    let (lowered, code) = state.json(&["permission", "set", "off"]);
    assert_eq!(code, 0, "{lowered}");
    std::thread::sleep(Duration::from_millis(500));
    let at_first = pages_done(&root, running_key);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(pages_done(&root, running_key), at_first, "it kept writing at Off");
    assert!(at_first < 1200, "it was meant to stop partway");
    assert!(!all_text_under(&root).contains("offqueuedword"), "a waiting file got written at Off");
    state.set_level(Level::Control);
    let (status, code) = state.json(&["index", "status", "--tab", "off-waiting"]);
    assert_eq!(code, 0, "{status}");
    assert_eq!(
        (
            status["data"]["state"].as_str(),
            status["data"]["indexers_running"].as_u64(),
            status["data"]["indexers_waiting"].as_u64()
        ),
        (Some("not_started"), Some(0), Some(0)),
        "{status}"
    );
}

/// Failure: a page couldn't be read, and the answer still says `hits_total` and "N hits in all", a claim about a page nobody read. `the_same_answer` left these fields out, so nothing saw it. It says how many it counted, and not that that's all, both for a failure in scope and for one reached only to count.
#[test]
fn no_total_is_given_over_a_page_that_couldnt_be_counted() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let bomb = TempPdf::new("no-total.pdf", &form_bomb());
    state.json(&["open", path(&bomb)]);
    wait_done(&state, "active");
    let (found, code) = state.json(&["search", "ordinary"]);
    assert_eq!(code, 0, "{found}");
    assert_eq!(found["data"]["coverage"]["failed_pages"], serde_json::json!([1]), "{found}");
    assert_eq!(
        (
            found["data"]["coverage"]["hits_total"].clone(),
            found["data"]["coverage"]["hits_at_least"].as_u64()
        ),
        (Value::Null, Some(1)),
        "{found}"
    );
    let (plain, _, _) = state.text(&["search", "ordinary"]);
    assert!(!plain.contains("in all"), "{plain}");

    // Past the limit, a corpus's failed page is only counted, and it can't be.
    let file = doc("uncounted.pdf", 5);
    corpus_with_a_failed_page(&state, &file, 4);
    state.json(&["open", path(&file)]);
    let (limited, code) = state.json(&["search", "common", "--limit", "1"]);
    assert_eq!(code, 0, "{limited}");
    let coverage = &limited["data"]["coverage"];
    assert_eq!(
        (
            coverage["hits_total"].clone(),
            coverage["uncounted_pages"].as_u64(),
            coverage["hits_at_least"].as_u64()
        ),
        (Value::Null, Some(1), Some(8)),
        "{limited}"
    );
    let (plain, _, _) = state.text(&["search", "common", "--limit", "1"]);
    assert!(plain.contains("At least 8") && !plain.contains("8 hits in all"), "{plain}");
}

/// Failure: on an edited tab whose file has changed, every page fails and nothing is read, and the answer says `index.state: ready` and "Answered from the index", a false account of what happened to a reader deciding what to trust.
#[test]
fn a_changed_file_isnt_said_to_have_been_answered_from_the_index() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("changed.pdf", 3);
    state.json(&["open", path(&file)]);
    wait_done(&state, "active");
    let (rotated, code) = state.json(&["page", "rotate", "1", "--by", "90"]);
    assert_eq!(code, 0, "{rotated}");
    std::fs::write(&file.0, pages(&[Page::new(text_lines(&["Another file now"]))])).unwrap();
    let (found, code) = state.json(&["search", "common"]);
    assert_eq!(code, 0, "{found}");
    assert_eq!(found["data"]["coverage"]["failed_pages"], serde_json::json!([1, 2, 3]), "{found}");
    assert_eq!(found["data"]["route"], "scan", "{found}");
    assert_eq!(found["data"]["index"]["state"], "unavailable", "{found}");
    assert!(!found["data"]["index"]["note"].as_str().unwrap().contains("Answered from the index"), "{found}");
    let (plain, _, _) = state.text(&["search", "common"]);
    assert!(!plain.contains("Answered"), "{plain}");
}

/// Failure: a corpus's folders or files can be read by another user on this Mac, so plain text of a user's documents is open to anyone who logs in. Every folder is 0700 and every file 0600, the lock and the notes between versions included.
#[test]
fn a_corpus_is_private_to_this_user() {
    use std::os::unix::fs::PermissionsExt;
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("private.pdf", 2);
    state.json(&["open", path(&file)]);
    wait_done(&state, "active");
    let mut stack = vec![state.paths.corpus.clone()];
    let mut seen = (0, 0);
    while let Some(next) = stack.pop() {
        let meta = std::fs::symlink_metadata(&next).unwrap();
        if meta.is_dir() {
            seen.0 += 1;
            assert_eq!(meta.permissions().mode() & 0o777, 0o700, "{}", next.display());
            stack.extend(std::fs::read_dir(&next).unwrap().flatten().map(|entry| entry.path()));
        } else {
            seen.1 += 1;
            assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{}", next.display());
        }
    }
    assert!(seen.0 >= 5 && seen.1 >= 5, "{seen:?}");
}

/// Failure: the folder a build makes isn't marked to stay out of backups, in the way the OS itself reads, through the session rather than on a library's say.
#[test]
fn a_session_marks_its_corpus_folder() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("marked.pdf", 1);
    state.json(&["open", path(&file)]);
    wait_done(&state, "active");
    assert!(state.paths.corpus.ends_with("corpus.noindex"), "{}", state.paths.corpus.display());
    assert!(recto_platform::local::is_excluded_from_backups(&state.paths.corpus));
    // As the OS itself reads it.
    let said = Command::new("/usr/bin/tmutil").arg("isexcluded").arg(&state.paths.corpus).output().unwrap();
    assert!(String::from_utf8_lossy(&said.stdout).contains("[Excluded]"), "{said:?}");
}

/// Failure: asking for the agent's folder before there's an index fails, or writes half of one. Or a search makes it, so everyone pays for what only an agent's tools will read, or a clear leaves it behind.
#[test]
fn the_agent_folder_comes_only_when_asked_for_and_goes_with_a_clear() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = doc("agent-folder.pdf", 40);
    let root = state.paths.corpus.clone();
    state.json(&["open", path(&file)]);
    let (early, code) = state.json(&["index", "--agent"]);
    assert_eq!(code, 0, "{early}");
    assert_eq!(early["data"]["folder"], Value::Null, "it isn't finished, so there's nothing to give: {early}");
    wait_done(&state, "active");
    let (searched, _) = state.json(&["search", "common"]);
    assert_eq!(searched["data"]["route"], "index", "{searched}");
    assert!(!all_text_under(&root).contains("|"), "nothing's written with marks yet");
    let key = CorpusKey::of_file(&file.0).unwrap();
    let paths = recto_index::corpus::CorpusPaths::new(&root, key);
    assert!(!paths.agent.exists(), "a search wrote the second copy");
    let (status, _) = state.json(&["index", "status"]);
    assert_eq!(
        (status["data"]["folder"].clone(), status["data"]["agent_bytes"].as_u64()),
        (Value::Null, Some(0)),
        "{status}"
    );

    // With the PDF gone: a folder comes from the pack alone.
    let (made, code) = state.json(&["index", "--agent"]);
    assert_eq!(code, 0, "{made}");
    assert!(paths.manifest.is_file() && paths.page_file(39, 4).is_file(), "{made}");
    let text = std::fs::read_to_string(paths.page_file(2, 4)).unwrap();
    assert!(text.contains("This is page 3 of the alpha document"), "{text}");
    let (status, _) = state.json(&["index", "status"]);
    assert_eq!(status["data"]["folder"].as_str(), Some(paths.agent.to_str().unwrap()), "{status}");

    let (cleared, code) = state.json(&["index", "clear"]);
    assert_eq!((code, cleared["data"]["cleared"].as_u64()), (0, Some(1)), "{cleared}");
    assert!(!all_text_under(&root).contains("alpha document"), "a clear left a form of the text");
}

/// A document for telling searches apart: words on some pages and not others, one page about them far more than the rest, a hidden run, and a page with nothing on it.
fn searchable(name: &str) -> TempPdf {
    let mut list: Vec<Page> = (1..=8)
        .map(|at| {
            Page::new(text_lines(&[
                &format!("Page {at} is about nothing much at all"),
                "with an aside on liability",
                "and a date 2019-03 here",
            ]))
        })
        .collect();
    list.push(Page::new(text_lines(&[
        "Liability and insurance, liability and caps",
        "liability insurance covers liability for insurance",
    ])));
    list.push(Page::new(hidden_text_page()));
    list.push(Page::new(Vec::new()));
    TempPdf::new(name, &pages(&list))
}

/// Failure: a pattern or a ranking says something different read from the index than read from the pages: a hit, a score, an order, a snippet, a hidden-text flag or what was covered. **Which way a page was read makes no difference, in every mode.**
#[test]
fn regex_and_ranked_searches_answer_the_same_either_way() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = searchable("modes.pdf");
    state.json(&["open", path(&file)]);
    wait_done(&state, "active");
    for args in [
        &["search", r"\d{4}-\d{2}", "--regex"][..],
        &["search", r"liability\s+and", "--regex", "--limit", "3"][..],
        &["search", r"(?-i)Liability", "--regex"][..],
        &["search", r"^with", "--regex", "--pages", "2-4"][..],
        &["search", r"HIDDEN-\w+", "--regex", "--context", "1"][..],
        &["search", r"zzzqqqxx", "--regex"][..],
        &["search", "liability insurance", "--rank"][..],
        &["search", "liability", "--rank", "--limit", "2"][..],
        &["search", "liability", "--rank", "--pages", "1-5", "-C", "1"][..],
        &["search", "HIDDEN-WHITE", "--rank"][..],
        &["search", "zzzqqqxx nothing", "--rank"][..],
        &["search", "common words", "-C", "2"][..],
    ] {
        index_matches_scan(&state, args);
    }
}

/// Failure: a pattern's hit lacks what a plain hit has: a page number from the edit order, the mark of an added file, the hidden-text flags, or coverage, so someone switching to `--regex` loses what stops them being misled.
#[test]
fn a_pattern_keeps_all_that_a_hit_says() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = searchable("shape.pdf");
    let extra = TempPdf::new("shape-added.pdf", &pages(&[Page::new(text_lines(&["From elsewhere with a date 2024-11"]))]));
    state.json(&["open", path(&file)]);
    wait_done(&state, "active");
    let (hidden, _) = state.json(&["search", r"hidden-white\s+\w+", "--regex"]);
    let hit = &hidden["data"]["hits"][0];
    assert_eq!(
        (
            hit["hidden_text"]["checked"].as_bool(),
            hit["hidden_text"]["flagged"].as_bool(),
            hit["overlaps_hidden"].as_bool(),
            hit["page"].as_u64()
        ),
        (Some(true), Some(true), Some(true), Some(10)),
        "{hit}"
    );
    assert_eq!(hidden["data"]["mode"], "regex");
    let (no_text, _) = state.json(&["search", r"zzz", "--regex"]);
    assert_eq!(no_text["data"]["coverage"]["no_text_pages"], serde_json::json!([11]), "{no_text}");
    assert_eq!(no_text["data"]["coverage"]["hits_total"], 0, "{no_text}");
    state.json(&["page", "move", "9", "--to", "1"]);
    state.json(&["page", "insert", path(&extra), "--at", "2"]);
    wait_done(&state, "active");
    let (found, code) = state.json(&["search", r"2024-\d+", "--regex"]);
    assert_eq!(code, 0, "{found}");
    let hit = &found["data"]["hits"][0];
    assert_eq!(
        (
            hit["page"].as_u64(),
            hit["source_page"].as_u64(),
            hit["from_added_file"].as_bool(),
            hit["untrusted"]["added_file"].as_str()
        ),
        (Some(2), Some(1), Some(true), Some("shape-added.pdf")),
        "{hit}"
    );
    let (moved, _) = state.json(&["search", r"caps$", "--regex"]);
    assert_eq!(
        (moved["data"]["hits"][0]["page"].as_u64(), moved["data"]["hits"][0]["source_page"].as_u64()),
        (Some(1), Some(9)),
        "{moved}"
    );
    let (context, _) = state.json(&["search", r"an aside", "--regex", "-C", "1", "--limit", "1"]);
    let around = &context["data"]["hits"][0]["untrusted"]["context"];
    assert!(around["before"][0].as_str().unwrap().contains("about nothing"), "{context}");
    assert!(around["after"][0].as_str().unwrap().contains("a date"), "{context}");
    let (plain, _, _) = state.text(&["search", r"an aside", "--regex", "-C", "1", "--limit", "1"]);
    assert!(plain.contains("- ") && plain.contains("+ "), "{plain}");
}

/// Failure: a mistyped or hostile pattern reads as "nothing found", or takes the session down, or two search modes at once picks one silently. Each is refused plainly, and the session's fine after.
#[test]
fn a_bad_pattern_is_refused_and_the_session_carries_on() {
    let state = State::at(Level::Control);
    let serving = state.serve();
    let file = doc("patterns.pdf", 1);
    state.json(&["open", path(&file)]);
    for (pattern, code) in [
        ("(unclosed", "bad_pattern"),
        (r"(a)\1", "bad_pattern"),
        (r"(?:\w{1000}){1000}", "pattern_too_large"),
    ] {
        let (refused, exit_code) = state.json(&["search", pattern, "--regex"]);
        assert_eq!((refused["error"]["code"].as_str(), exit_code), (Some(code), exit::FAILED), "{refused}");
        assert!(!refused["error"]["next"].as_str().unwrap().is_empty(), "{refused}");
    }
    let (both, code) = state.json(&["search", "x", "--regex", "--rank"]);
    assert_eq!((both["error"]["code"].as_str(), code), (Some("bad_query"), exit::FAILED), "{both}");
    let (nothing, code) = state.json(&["search", "!!! ...", "--rank"]);
    assert_eq!((nothing["error"]["code"].as_str(), code), (Some("bad_query"), exit::FAILED), "{nothing}");
    let (status, code) = state.json(&["status"]);
    assert_eq!((status["data"]["pid"].as_u64(), code), (Some(u64::from(serving.pid())), 0));
}

/// Failure: a ranked answer reads as every hit, or as a verdict on the pages: nothing says it's ordered by a count of words, a total is claimed, the page with most of the words isn't first, or a failed page drops out of sight. Text stays under `untrusted`, and the flags stay on.
#[test]
fn a_ranked_answer_says_what_it_is() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = searchable("ranked.pdf");
    corpus_with_a_failed_page(&state, &file, 3);
    state.json(&["open", path(&file)]);
    let (ranked, code) = state.json(&["search", "Liability, insurance", "--rank"]);
    assert_eq!(code, 0, "{ranked}");
    let data = &ranked["data"];
    assert_eq!(data["mode"], "ranked");
    assert_eq!(data["terms"], serde_json::json!(["liability", "insurance"]), "{ranked}");
    let hits = data["hits"].as_array().unwrap();
    assert_eq!(
        (hits[0]["page"].as_u64(), hits[0]["terms"].clone()),
        (Some(9), serde_json::json!(["liability", "insurance"])),
        "the page with the most of them is first: {ranked}"
    );
    assert_eq!(hits[0]["term_counts"], serde_json::json!([4, 3]), "{ranked}");
    assert!(hits.windows(2).all(|pair| pair[0]["score"].as_f64() >= pair[1]["score"].as_f64()), "{ranked}");
    assert!(hits[0]["untrusted"]["snippet"].as_str().unwrap().contains("Liability"), "{ranked}");
    assert!(hits[0]["hidden_text"]["checked"].is_boolean(), "{ranked}");
    assert_eq!(hits[0]["line"], 1);
    assert_eq!(data["coverage"]["pages_with_these_words"], 9, "{ranked}");
    assert_eq!(
        data["coverage"]["hits_total"],
        Value::Null,
        "a ranked answer isn't every hit, so it gives no total: {ranked}"
    );
    assert!(data["coverage"].get("hits_at_least").is_none(), "a count of 0 beside real hits: {ranked}");
    assert_eq!(data["coverage"]["no_text_pages"], serde_json::json!([11]), "{ranked}");
    let note = data["ranked_note"].as_str().unwrap();
    assert!(
        note.contains("isn't every hit") && note.contains("count") && !note.contains("relevan"),
        "{note}"
    );
    let (plain, _, _) = state.text(&["search", "Liability, insurance", "--rank"]);
    assert!(plain.contains("isn't every hit") && plain.contains("score "), "{plain}");
    let (limited, _) = state.json(&["search", "liability", "--rank", "--limit", "1"]);
    assert_eq!(limited["data"]["hits"].as_array().unwrap().len(), 1, "{limited}");
    let (hidden, _) = state.json(&["search", "HIDDEN-WHITE", "--rank"]);
    assert_eq!(
        (
            hidden["data"]["hits"][0]["hidden_text"]["flagged"].as_bool(),
            hidden["data"]["hits"][0]["overlaps_hidden"].as_bool()
        ),
        (Some(true), Some(true)),
        "{hidden}"
    );
}

/// Failure: a hostile page that repeats a word reaches the top, which can't be stopped, and comes out reading as trustworthy rather than marked as the document's words. Or one word makes a page score without limit, so nothing else shows.
#[test]
fn stuffing_a_page_gets_it_first_and_no_further() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let stuffed = "keyword ".repeat(60);
    let list = [
        Page::new(text_lines(&["a page that mentions keyword once and other words beside"])),
        Page::new(text_lines(&[&stuffed])),
    ];
    let file = TempPdf::new("stuffed.pdf", &pages(&list));
    state.json(&["open", path(&file)]);
    let (ranked, code) = state.json(&["search", "keyword", "--rank"]);
    assert_eq!(code, 0, "{ranked}");
    let hits = ranked["data"]["hits"].as_array().unwrap();
    assert_eq!(hits[0]["page"], 2, "{ranked}");
    assert!(hits[0].get("snippet").is_none() && hits[0]["untrusted"]["snippet"].is_string(), "{ranked}");
    assert_eq!(hits.len(), 2, "the honest page is still there: {ranked}");
    assert!(hits[0]["score"].as_f64().unwrap() < 4.0 * hits[1]["score"].as_f64().unwrap(), "{ranked}");
}

/// Failure: a page too long for a pattern reads as having nothing on it, or takes one route and not the other. It's listed as not read, so "nothing found" doesn't cover it, no total is given over it, and a plain search still finds what's there.
#[test]
fn a_page_too_long_for_a_pattern_is_listed() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    // Many short lines of the same words, past what a pattern is run over.
    let line = "long page words here that repeat and repeat and repeat and repeat and repeat";
    let lines: Vec<&str> = std::iter::repeat_n(line, 2000).collect();
    let list = [
        Page::new(text_lines(&["A short page with a word like needleword"])),
        Page::new(text_lines(&lines)),
    ];
    let file = TempPdf::new("long-page.pdf", &pages(&list));
    state.json(&["open", path(&file)]);
    wait_done(&state, "active");
    let (found, code) = state.json(&["search", "needleword|repeat", "--regex", "--limit", "5"]);
    assert_eq!(code, 0, "{found}");
    let coverage = &found["data"]["coverage"];
    assert_eq!(
        (
            coverage["failed_pages"].clone(),
            coverage["pages_too_long_for_a_pattern"].as_u64(),
            coverage["hits_total"].clone()
        ),
        (serde_json::json!([2]), Some(1), Value::Null),
        "{found}"
    );
    assert_eq!(found["data"]["hits"].as_array().unwrap().len(), 1, "the short page is still searched: {found}");
    let (scanned, _) = state.json(&["search", "needleword|repeat", "--regex", "--limit", "5", "--scan"]);
    assert_eq!(the_same_answer(found), the_same_answer(scanned));
    let (plain, _) = state.json(&["search", "repeat and", "--limit", "1"]);
    assert_eq!(plain["data"]["coverage"]["failed_pages"], serde_json::json!([]), "{plain}");
}

/// Failure: an earlier build's `corpus` folder keeps plain text outside `.noindex`, eviction and `clear --all` for good, with nothing to take it away. It goes when a session starts, and nothing else does: not a folder named nearly the same, and not what a link there points at.
#[test]
fn the_old_corpus_folder_goes_at_startup() {
    let state = State::at(Level::Control);
    state.paths.prepare().unwrap();
    let old = state.root.join("corpus");
    std::fs::create_dir_all(old.join("abc").join("v1")).unwrap();
    std::fs::write(old.join("abc").join("v1").join("page.md"), "old plain text").unwrap();
    let nearly = state.root.join("corpus.keep");
    std::fs::create_dir_all(&nearly).unwrap();
    std::fs::write(nearly.join("mine"), "someone's").unwrap();
    {
        let _serving = state.serve();
        let deadline = Instant::now() + Duration::from_secs(30);
        while old.exists() {
            assert!(Instant::now() < deadline, "the old folder's still there");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(std::fs::read_to_string(nearly.join("mine")).unwrap(), "someone's");
    }
    // A link there isn't the old folder, and what it points at stays.
    std::os::unix::fs::symlink(&nearly, &old).unwrap();
    let serving = state.serve();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        recto_platform::local::process_counters(serving.pid()).unwrap().threads,
        1,
        "a start with nothing to remove holds a thread"
    );
    assert!(std::fs::symlink_metadata(&old).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(nearly.join("mine")).unwrap(), "someone's");
}
