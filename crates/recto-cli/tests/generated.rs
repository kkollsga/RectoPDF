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

/// Waits for them. The helper runs at background priority, which a busy Mac can starve: if it never finishes, nextest's limit ends this.
fn wait_made(state: &State) -> (Value, i32) {
    loop {
        let (done, code) = state.json(&["wait", "contents-generated", "--timeout", "120s"]);
        if code != recto_agent::exit::TIMED_OUT {
            return (done, code);
        }
    }
}

fn titles(answer: &Value) -> Vec<String> {
    answer["data"]["entries"].as_array().unwrap().iter().map(|entry| entry["untrusted"]["title"].as_str().unwrap().to_string()).collect()
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
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "asking waited for them: {:?}", started.elapsed());
    assert!(first["data"]["available"] == false || first["data"]["state"] == "ready", "{first}");

    let (made, code) = wait_made(&state);
    assert_eq!(code, 0, "{made}");
    assert_eq!(made["data"]["generated"], true);
    assert!(made["data"]["note"].as_str().unwrap().contains("aren't part of the file"), "{made}");
    let found = titles(&made);
    assert!(found.iter().all(|title| !title.contains("HIDDEN")), "{found:?}");
    assert_eq!(found, ["1 Introduction", "2 Background", "3 Method", "4 Results", "5 Discussion", "6 Conclusion"]);
    let entries = made["data"]["entries"].as_array().unwrap();
    assert_eq!(entries.iter().map(|entry| entry["page"].as_u64().unwrap()).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
    assert!(entries.iter().all(|entry| entry["place"].as_u64().is_some_and(|place| place < 200)), "{made}");
    assert!(made["data"]["hidden_left_out"].as_u64().unwrap() >= 1, "distillPDF 0.1.7 takes the hidden lines for headings: {made}");

    // The same answer from what was kept, at once, and in the shape `contents` has always had.
    let (again, _) = state.json(&["contents", "--generated", "--limit", "2"]);
    assert_eq!(again["data"]["state"], "ready");
    assert_eq!(again["data"]["next_from"], 2);
    let (shown, _, code) = state.text(&["contents", "--generated"]);
    assert_eq!(code, 0);
    assert!(shown.contains("Introduction") && !shown.contains("HIDDEN"), "{shown}");
    // The file's own are still none: nothing was written to it.
    let (own, _) = state.json(&["contents"]);
    assert_eq!(own["data"]["total"], 0, "{own}");
}

/// Failure: a kept file someone has changed is believed: its headings come back for a document they aren't about, or a page past the end sends the reader nowhere. It's thrown away, and they're made again.
#[test]
fn a_forged_kept_file_is_not_believed() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("headings.pdf", &recto_testpdf::headings_document());
    state.json(&["open", path(&file)]);
    state.json(&["contents", "--generated"]);
    wait_made(&state);
    let kept: Vec<_> = std::fs::read_dir(&state.paths.generated).unwrap().flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|ext| ext == "json")).collect();
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
