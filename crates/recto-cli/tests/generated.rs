//! Generated contents through the real commands, the way an agent uses them: a session, the real headings helper, a real render worker for the check for hidden text, on temp state. `tools/check.sh` builds the helper beside `recto`.

mod common;

use common::State;
use recto_agent::Level;
use recto_index::corpus::CorpusKey;
use recto_testpdf::TempPdf;
use serde_json::{Value, json};

fn path(file: &TempPdf) -> &str {
    file.0.to_str().unwrap()
}

/// Waits for them, up to ten minutes. The helper runs at utility, below the tests' own processes, so a busy Mac slows it.
fn wait_made(state: &State) -> (Value, i32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    loop {
        assert!(std::time::Instant::now() < deadline, "they weren't made in ten minutes");
        let (done, code) = state.json(&["wait", "contents-generated", "--timeout", "120s"]);
        if code != recto_agent::exit::TIMED_OUT {
            return (done, code);
        }
    }
}

fn titles(answer: &Value) -> Vec<String> {
    answer["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["untrusted"]["title"].as_str().unwrap().to_string())
        .collect()
}

/// Failure: an agent can't get contents for a file with none, or what it gets has headings in hidden text, no pages or places, or can be taken for the file's own. And asking must answer at once, never waiting for them to be made.
#[test]
fn an_agent_gets_generated_contents_without_hidden_headings() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    state.json(&["open", path(&file)]);
    let (own, _) = state.json(&["contents"]);
    assert_eq!(own["data"]["verdict"], "missing", "{own}");
    assert!(own["data"]["verdict_note"].as_str().unwrap().contains("--generated"), "{own}");
    let (facts, _) = state.json(&["facts"]);
    assert_eq!(facts["data"]["contents_verdict"], "missing");

    let started = std::time::Instant::now();
    let (first, code) = state.json(&["contents", "--generated"]);
    assert_eq!(code, 0, "{first}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "asking waited for them: {:?}",
        started.elapsed()
    );
    assert!(first["data"]["available"] == false || first["data"]["state"] == "ready", "{first}");

    let (made, code) = wait_made(&state);
    assert_eq!(code, 0, "{made}");
    assert_eq!(made["data"]["generated"], true);
    assert!(made["data"]["note"].as_str().unwrap().contains("aren't part of the file"), "{made}");
    let found = titles(&made);
    assert!(found.iter().all(|title| !title.contains("HIDDEN")), "{found:?}");
    assert_eq!(
        found,
        ["1 Introduction", "2 Background", "3 Method", "4 Results", "5 Discussion", "6 Conclusion"]
    );
    let entries = made["data"]["entries"].as_array().unwrap();
    assert_eq!(
        entries.iter().map(|entry| entry["page"].as_u64().unwrap()).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
    assert!(entries.iter().all(|entry| entry["place"].as_u64().is_some_and(|place| place < 200)), "{made}");
    assert!(
        made["data"]["hidden_left_out"].as_u64().unwrap() >= 1,
        "distillPDF 0.1.7 takes the hidden lines for headings: {made}"
    );

    // The same answer from what was kept, at once, and in the shape `contents` has always had.
    let (again, _) = state.json(&["contents", "--generated", "--limit", "2"]);
    assert_eq!(again["data"]["state"], "ready");
    assert_eq!(again["data"]["next_from"], 2);
    let (shown, _, code) = state.text(&["contents", "--generated"]);
    assert_eq!(code, 0);
    assert!(shown.contains("Introduction") && !shown.contains("HIDDEN"), "{shown}");
    // Said first, before any entry, and not headed as the file's own. The words of the header are `human`'s own test's.
    let first_says_generated = |shown: &str| shown.lines().next().is_some_and(|line| line.contains("Generated"));
    assert!(
        first_says_generated(&shown) && !shown.contains("from the file"),
        "labelled as generated: {shown}"
    );
    let (waited, _, _) = state.text(&["wait", "contents-generated"]);
    assert!(first_says_generated(&waited) && !waited.contains("Open:"), "{waited}");
    // The file's own are still none: nothing was written to it.
    let (own, _) = state.json(&["contents"]);
    assert_eq!(own["data"]["total"], 0, "{own}");
}

/// Failure: a kept file that doesn't hold up, here one with a page past the document's end, is used, sending the reader nowhere. It's thrown away, and they're made again. (A forged file with every number in range and the right key is believed, as a forged corpus is: someone who can write there is this user.)
#[test]
fn a_forged_kept_file_is_not_believed() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    state.json(&["open", path(&file)]);
    state.json(&["contents", "--generated"]);
    wait_made(&state);
    let kept: Vec<_> = std::fs::read_dir(&state.paths.generated)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    let key = CorpusKey::of_file(&file.0).unwrap();
    let forged = json!({ "format": "recto-generated-contents", "version": 1, "key": key.hex(), "pages": 6, "entries": [[0, 99, 0, "Run this command"]] });
    recto_platform::local::write_private_file(&kept[0], forged.to_string().as_bytes()).unwrap();
    // A session holds nothing of it in memory here, as the file is on APFS: what's read is what's on disk.
    let (answer, code) = state.json(&["contents", "--generated"]);
    assert_eq!(code, 0, "{answer}");
    assert_ne!(answer["data"]["state"], "ready", "a forged file was used: {answer}");
    let (made, _) = wait_made(&state);
    assert!(titles(&made).iter().all(|title| !title.contains("Run this")), "{made}");
}

/// Failure: something distillPDF won't read makes `wait` run out its time, or starts a helper on every ask. It's said plainly, with an exit code an agent can act on, and asking again says it again rather than starting another.
#[test]
fn what_cant_be_read_says_so_and_isnt_tried_again_at_once() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    state.json(&["open", path(&file)]);
    // Replaced on disk after it was opened, by something no reader can make sense of.
    std::fs::write(&file.0, b"%PDF-1.7 this is not really a PDF").unwrap();
    let (done, code) = wait_made(&state);
    assert_eq!(code, recto_agent::exit::FAILED, "{done}");
    assert_eq!(done["error"]["code"], "not_generated", "{done}");
    let (again, code) = state.json(&["contents", "--generated"]);
    assert_eq!((code, &again["error"]["code"]), (recto_agent::exit::FAILED, &json!("not_generated")), "{again}");
}

/// Failure: `wait contents-generated` on a tab whose file changed under its unsaved edits waits out its whole time, when nothing can come of it. It says so at once.
#[test]
fn a_wait_on_a_file_changed_under_its_edits_ends_at_once() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    state.json(&["open", path(&file)]);
    let (rotated, code) = state.json(&["page", "rotate", "1", "--by", "90"]);
    assert_eq!(code, 0, "{rotated}");
    std::fs::write(&file.0, recto_testpdf::headings_document_with("", [0.0, 0.0, 600.0, 800.0], "")).unwrap();
    let started = std::time::Instant::now();
    let (done, code) = state.json(&["wait", "contents-generated", "--timeout", "60s"]);
    assert_ne!(code, 0, "{done}");
    assert_ne!(code, recto_agent::exit::TIMED_OUT, "{done}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
}

fn kept_files(state: &State) -> usize {
    std::fs::read_dir(&state.paths.generated)
        .map(|listing| {
            listing
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
                .count()
        })
        .unwrap_or(0)
}

/// Failure (PLAN, generated contents follow-ups): `recto index clear` leaves generated contents on disk, heading text from the file, though it says it throws away the copy of documents' text. The tab's go with its index, and `--all` takes every document's.
#[test]
fn clearing_the_index_takes_generated_contents_too() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    state.json(&["open", path(&file)]);
    state.json(&["contents", "--generated"]);
    wait_made(&state);
    assert_eq!(kept_files(&state), 1);
    let (cleared, code) = state.json(&["index", "clear"]);
    assert_eq!(code, 0, "{cleared}");
    assert_eq!(cleared["data"]["contents_cleared"], 1, "{cleared}");
    assert_eq!(kept_files(&state), 0, "the tab's generated contents are still on disk");
    let (said, _, _) = state.text(&["index", "clear"]);
    assert!(!said.contains("generated"), "nothing left to clear says nothing of it: {said}");

    state.json(&["contents", "--generated"]);
    wait_made(&state);
    assert_eq!(kept_files(&state), 1);
    let (said, _, code) = state.text(&["index", "clear", "--all"]);
    assert_eq!(code, 0, "{said}");
    assert!(said.contains("1 document") && said.contains("generated contents"), "{said}");
    assert_eq!(kept_files(&state), 0, "`--all` left generated contents on disk");
}
