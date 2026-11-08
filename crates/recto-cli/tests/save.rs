//! `recto save` and `recto save-as`, the real binary on a real session: what an agent does, at each level, and what reaches the file. Each test names the failure it catches.

mod common;

use common::State;
use recto_agent::{Level, exit};
use recto_testpdf::{Object, Page, TempPdf, build, pages, text_lines};
use serde_json::Value;

fn marked(markers: &[&str]) -> Vec<u8> {
    pages(&markers.iter().map(|marker| Page::new(text_lines(&[marker]))).collect::<Vec<_>>())
}

fn found(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle.as_bytes())
}

fn ok(state: &State, args: &[&str]) -> Value {
    let (reply, code) = state.json(args);
    assert_eq!(code, 0, "{args:?}: {reply}");
    reply["data"].clone()
}

fn path(file: &TempPdf) -> &str {
    file.0.to_str().unwrap()
}

/// Failure: `recto save` doesn't reach the file, or reaches it and loses the history, so undo stops at the save; or a turn and an added page come out wrong in the file; or the original isn't kept. The whole round at Control: edit, save, read back from the file, undo past the save and come back to it.
#[test]
fn an_agent_saves_turns_and_added_pages_and_undo_goes_past_it() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("doc.pdf", &marked(&["FIRST", "SECOND"]));
    let extra = TempPdf::new("extra.pdf", &marked(&["ADDED"]));
    let original = std::fs::read(&file.0).unwrap();
    ok(&state, &["open", path(&file)]);
    ok(&state, &["page", "rotate", "2", "--by", "90"]);
    ok(&state, &["page", "insert", path(&extra), "--at", "3"]);
    let saved = ok(&state, &["save"]);
    assert_eq!(saved["written"], true, "{saved}");
    assert_eq!(saved["writer"], "append");
    assert_eq!(saved["edited"], false);
    assert_eq!(saved["backup_kept"], true);
    assert!(saved.get("backup").is_none(), "where the backup is, is Full's to see");
    // Undo goes past the save: the file has the added page, the document doesn't now. Redo comes back to the saved point.
    let undone = ok(&state, &["undo"]);
    assert_eq!((undone["edited"].as_bool(), undone["pages"].as_u64()), (Some(true), Some(2)), "{undone}");
    assert_eq!(ok(&state, &["redo"])["edited"], false);
    let bytes = std::fs::read(&file.0).unwrap();
    assert!(bytes.starts_with(&original), "appended");
    assert!(found(&bytes, "ADDED"), "the added page is in the file");
    // The file read fresh says the same: three pages, the added one last.
    ok(&state, &["close"]);
    let opened = ok(&state, &["open", path(&file)]);
    assert_eq!(opened["pages"], 3);
    // Each page where it should be, turned as it should be, read back from the file with the fork's own reader.
    let reader = lopdf::IndexedReader::open(lopdf::FileSource::open(&file.0).unwrap()).unwrap();
    let map = reader.page_map().unwrap();
    let marker_of = |at: usize| {
        let lopdf::Object::Dictionary(page) = reader.resolve_object(map.get(at).unwrap().id()).unwrap() else { panic!() };
        let contents = page.get(b"Contents").and_then(lopdf::Object::as_reference).unwrap();
        let lopdf::Object::Stream(stream) = reader.resolve_object(contents).unwrap() else { panic!() };
        let rotate = page.get(b"Rotate").and_then(lopdf::Object::as_i64).unwrap_or(0);
        (String::from_utf8_lossy(&stream.content).split('(').nth(1).unwrap_or("").split(')').next().unwrap_or("").to_string(), rotate)
    };
    assert_eq!((0..3).map(marker_of).collect::<Vec<_>>(), [("FIRST".to_string(), 0), ("SECOND".to_string(), 90), ("ADDED".to_string(), 0)]);
    let folder = state.paths.root.join("backups.noindex");
    let backups: Vec<_> = std::fs::read_dir(&folder).unwrap().filter_map(Result::ok).collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read(backups[0].path()).unwrap(), original, "the one backup is the original");
}

/// Failure: undoing past a save isn't possible, or doesn't read as unsaved, or coming back doesn't read as saved (SPEC decision 25).
#[test]
fn undo_past_a_save_reads_as_unsaved_and_back_as_saved() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("doc.pdf", &marked(&["ONE", "TWO"]));
    ok(&state, &["open", path(&file)]);
    ok(&state, &["page", "move", "2", "--to", "1"]);
    assert_eq!(ok(&state, &["save"])["edited"], false);
    let undone = ok(&state, &["undo"]);
    assert_eq!(undone["edited"], true, "the file has the move, the document doesn't now");
    let redone = ok(&state, &["redo"]);
    assert_eq!(redone["edited"], false, "and back at the saved point");
    let again = ok(&state, &["save"]);
    assert_eq!(again["written"], false, "nothing unsaved, nothing written");
}

/// Failure: a level that can't edit can save, or Control can replace another file or rewrite one. Read-only can't save; Control saves what's appended and saves as a new file; replacing another file is Full's.
#[test]
fn levels_decide_what_an_agent_can_save() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = TempPdf::new("doc.pdf", &marked(&["A", "B"]));
    let other = TempPdf::new("other.pdf", b"someone else's");
    ok(&state, &["open", path(&file)]);
    ok(&state, &["page", "rotate", "1", "--by", "90"]);
    state.set_level(Level::ReadOnly);
    let (refused, code) = state.json(&["save"]);
    assert_eq!(code, exit::PERMISSION, "{refused}");
    state.set_level(Level::Control);
    let (refused, code) = state.json(&["save-as", "--out", path(&other), "--replace"]);
    assert_eq!(code, exit::PERMISSION, "{refused}");
    let (refused, code) = state.json(&["save-as", "--out", path(&other)]);
    assert_eq!((code, refused["error"]["code"].as_str()), (exit::FAILED, Some("target_exists")), "{refused}");
    assert_eq!(std::fs::read(&other.0).unwrap(), b"someone else's");
    let copy = file.0.with_file_name("copy.pdf");
    let saved = ok(&state, &["save-as", "--out", copy.to_str().unwrap()]);
    assert_eq!(saved["written"], true);
    let (tabs, _) = state.json(&["where"]);
    assert!(tabs.to_string().contains("copy"), "the tab goes over to the copy: {tabs}");
    assert_eq!(std::fs::read(&file.0).unwrap(), marked(&["A", "B"]), "the original is left as it was");
    let _ = std::fs::remove_file(copy);
}

/// Failure: a delete is saved by appending, so the deleted page stays in the file. Until the rewrite is built it's refused, nothing is written, and the edits stay.
#[test]
fn a_delete_isnt_saved_by_appending() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = TempPdf::new("doc.pdf", &marked(&["KEEP", "DELETED"]));
    ok(&state, &["open", path(&file)]);
    ok(&state, &["page", "delete", "2"]);
    let (refused, code) = state.json(&["save"]);
    assert_eq!((code, refused["error"]["code"].as_str()), (exit::FAILED, Some("needs_rewrite")), "{refused}");
    assert!(refused["error"]["message"].as_str().unwrap().contains("Nothing was written"));
    assert_eq!(std::fs::read(&file.0).unwrap(), marked(&["KEEP", "DELETED"]));
    assert_eq!(ok(&state, &["edits"])["edited"], true, "the edits stand");
}

/// Failure (SPEC decision 25, on the real path): marked text that was only ever in the history reaches the saved file: an added PDF that was undone, its name, the journal. The save writes the document and nothing else.
#[test]
fn a_saved_pdf_has_nothing_of_the_history_in_it_on_the_real_path() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let file = TempPdf::new("doc.pdf", &marked(&["DOCUMENT PAGE"]));
    let undone = TempPdf::new("historymarkundonename.pdf", &marked(&["HISTORYMARKUNDONETEXT"]));
    let kept = TempPdf::new("kept.pdf", &marked(&["KEPT PAGE"]));
    ok(&state, &["open", path(&file)]);
    ok(&state, &["page", "insert", path(&undone), "--at", "1"]);
    ok(&state, &["undo"]);
    ok(&state, &["page", "insert", path(&kept), "--at", "2"]);
    ok(&state, &["page", "rotate", "1", "--by", "270"]);
    ok(&state, &["save"]);
    let saved = std::fs::read(&file.0).unwrap();
    for mark in ["HISTORYMARKUNDONETEXT", "historymarkundonename", "journal-", "recto-journal", "kept-"] {
        assert!(!found(&saved, mark), "{mark} reached the saved PDF");
    }
    assert!(found(&saved, "DOCUMENT PAGE") && found(&saved, "KEPT PAGE"), "and it's the document");
    // The history still has the undone PDF: redo isn't possible past a new step, but undo goes back through the save.
    assert_eq!(ok(&state, &["undo"])["edited"], true);
}

/// A signed document: a signature field whose value has a byte range.
fn signed() -> Vec<u8> {
    build(&[
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /SigFlags 3 >> >>".into()),
        Object::Dict("<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into()),
        Object::Dict("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".into()),
        Object::Stream { dict: String::new(), data: text_lines(&["A contract"]) },
        Object::Dict("<< /FT /Sig /T (Tenant) /V 6 0 R >>".into()),
        Object::Dict("<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 5 10 5] /Contents <00> >>".into()),
    ])
}

/// Failure: saving a turn on a signed PDF changes a byte its signature covers. Every one stays, and the answer never calls the signature valid.
#[test]
fn a_signed_pdfs_bytes_never_change() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("signed.pdf", &signed());
    let before = std::fs::read(&file.0).unwrap();
    ok(&state, &["open", path(&file)]);
    ok(&state, &["page", "rotate", "1", "--by", "90"]);
    let saved = ok(&state, &["save"]);
    assert!(std::fs::read(&file.0).unwrap().starts_with(&before));
    assert!(!saved.to_string().to_lowercase().contains("valid"), "{saved}");
}

/// Failure: the file changes on disk under an open tab, and a save writes over the other app's version. Refused, the other version stays, and the edits are still here.
#[test]
fn a_file_replaced_under_a_tab_isnt_saved_over() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("doc.pdf", &marked(&["MINE"]));
    ok(&state, &["open", path(&file)]);
    ok(&state, &["page", "rotate", "1", "--by", "90"]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&file.0, marked(&["SOMEONE ELSE'S"])).unwrap();
    let (refused, code) = state.json(&["save"]);
    assert_eq!((code, refused["error"]["code"].as_str()), (exit::FAILED, Some("file_changed")), "{refused}");
    assert_eq!(std::fs::read(&file.0).unwrap(), marked(&["SOMEONE ELSE'S"]));
    assert_eq!(ok(&state, &["edits"])["edited"], true, "the edits stand");
}

/// Failure (review): a prompt-injected agent at Control saves a hostile PDF as `~/.zshenv`, which the next shell runs, or somewhere else that reads it as other than a PDF. A Save As's name ends in `.pdf`, nothing in its path is hidden, and nothing goes in `~/Library`. Each refusal writes nothing.
#[test]
fn save_as_goes_only_where_a_pdf_belongs() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("doc.pdf", &marked(&["A"]));
    ok(&state, &["open", path(&file)]);
    ok(&state, &["page", "rotate", "1", "--by", "90"]);
    let folder = file.0.parent().unwrap();
    std::fs::create_dir_all(folder.join(".hidden")).unwrap();
    let home = recto_platform::local::real_home().unwrap();
    let refused = [folder.join("notes.txt"), folder.join("doc.PDF.sh"), folder.join(".zshenv.pdf"), folder.join(".hidden").join("doc.pdf"), home.join("Library").join("recto-review-test.pdf")];
    for out in &refused {
        let (reply, code) = state.json(&["save-as", "--out", out.to_str().unwrap()]);
        assert_eq!((code, reply["error"]["code"].as_str()), (exit::USAGE, Some("bad_save_path")), "{}: {reply}", out.display());
        assert!(!out.exists(), "{} was written", out.display());
    }
    let fine = folder.join("Copy.PDF");
    assert_eq!(ok(&state, &["save-as", "--out", fine.to_str().unwrap()])["written"], true, "a .PDF name in an ordinary folder saves");
    let _ = std::fs::remove_file(fine);
    let _ = std::fs::remove_dir(folder.join(".hidden"));
}

/// Failure (review): Save As with replace onto the tab's own file skips the backup and the same-file check, and onto a file open in another tab leaves two tabs on one file (SPEC decision 26). Onto its own file it's a Save; onto another tab's it's refused, with that file untouched.
#[test]
fn save_as_onto_its_own_file_is_a_save_and_onto_another_tabs_is_refused() {
    let state = State::at(Level::Full);
    let _serving = state.serve();
    let mine = TempPdf::new("mine.pdf", &marked(&["MINE"]));
    let theirs = TempPdf::new("theirs.pdf", &marked(&["THEIRS"]));
    ok(&state, &["open", path(&theirs)]);
    ok(&state, &["open", path(&mine)]);
    ok(&state, &["page", "rotate", "1", "--by", "90"]);
    let (refused, code) = state.json(&["save-as", "--out", path(&theirs), "--replace"]);
    assert_eq!((code, refused["error"]["code"].as_str()), (exit::FAILED, Some("open_in_another_tab")), "{refused}");
    assert_eq!(std::fs::read(&theirs.0).unwrap(), marked(&["THEIRS"]));
    let saved = ok(&state, &["save-as", "--out", path(&mine), "--replace"]);
    assert_eq!((saved["written"].as_bool(), saved["backup_kept"].as_bool()), (Some(true), Some(true)), "a Save, with its backup: {saved}");
    assert_eq!(ok(&state, &["tabs"])["count"], 2, "still two tabs, on two files");
}

/// Failure: a Save As of a document with nothing unsaved goes outside the one writer. It's the writer's append, with the original's bytes first, and the tab goes over to the copy.
#[test]
fn an_unedited_save_as_goes_through_the_writer() {
    let state = State::at(Level::Control);
    let _serving = state.serve();
    let file = TempPdf::new("doc.pdf", &marked(&["A"]));
    ok(&state, &["open", path(&file)]);
    let copy = file.0.with_file_name("plain-copy.pdf");
    let saved = ok(&state, &["save-as", "--out", copy.to_str().unwrap()]);
    assert_eq!((saved["written"].as_bool(), saved["writer"].as_str()), (Some(true), Some("append")), "{saved}");
    assert!(std::fs::read(&copy).unwrap().starts_with(&marked(&["A"])));
    let _ = std::fs::remove_file(copy);
}

/// Failure: a kill during a save leaves a broken file, or half of one, under the document's name. At every step, writing, noted and about to move, and just after the move, the file is the original byte for byte or the whole new one, read back page by page.
#[test]
fn a_kill_at_every_step_leaves_the_original_or_the_new_file() {
    for step in ["writing", "copying", "prepared", "committed"] {
        let state = State::at(Level::Control);
        let marker = state.root.join("paused");
        let mut server = state.command().args(["serve", "--json"]).env("RECTO_TEST_SAVE_PAUSE", step).env("RECTO_TEST_SAVE_MARKER", &marker).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let mut ready = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(server.stdout.take().unwrap()), &mut ready).unwrap();
        assert!(ready.contains("\"ok\":true"), "{ready}");
        let file = TempPdf::new("doc.pdf", &marked(&["ONE", "TWO"]));
        let extra = TempPdf::new("extra.pdf", &marked(&["THREE"]));
        let original = std::fs::read(&file.0).unwrap();
        ok(&state, &["open", path(&file)]);
        ok(&state, &["page", "rotate", "1", "--by", "90"]);
        ok(&state, &["page", "insert", path(&extra), "--at", "3"]);
        let mut saving = state.command().args(["save"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let waited = std::time::Instant::now();
        while !marker.exists() {
            assert!(waited.elapsed() < std::time::Duration::from_secs(30), "{step}: the save never got there");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        server.kill().unwrap();
        server.wait().unwrap();
        let _ = saving.kill();
        let _ = saving.wait();
        let now = std::fs::read(&file.0).unwrap();
        // What a kill can leave is the writer's own temporary file, hidden beside the document and swept by a later save, never anything under its name.
        let left: Vec<String> = std::fs::read_dir(file.0.parent().unwrap()).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).filter(|name| name != "doc.pdf" && name != "extra.pdf").collect();
        assert!(left.iter().all(|name| name.starts_with(".append-")), "{step}: left {left:?}");
        if step == "copying" {
            assert!(!left.is_empty(), "the temporary file was there when it was killed mid-write");
        }
        if now == original {
            assert_ne!(step, "committed", "the move had happened");
        } else {
            assert!(now.starts_with(&original), "{step}: appended to");
            let reader = lopdf::IndexedReader::open(lopdf::FileSource::open(&file.0).unwrap()).unwrap();
            assert_eq!(reader.page_count().unwrap(), 3, "{step}: the whole new file");
            assert!(found(&now, "THREE"));
        }
    }
}
