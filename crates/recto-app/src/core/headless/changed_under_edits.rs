//! **A tab's file changed on disk by another program while it has unsaved edits,** on the app's own core with real files. Until the page matcher exists
//! (owner, 2026-11-13), an edit names its page by its number in the version it was made on, so what holds here is that nothing is written, the edits are kept,
//! and the words say what's true: they can't be saved, here or as a copy, while the other version is in place.

use crate::queue::CoreCall;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;
use std::path::{Path, PathBuf};

/// A PDF of `pages` pages whose text says `who` and the page.
fn pdf_bytes(who: &str, pages: usize) -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = (1..=pages)
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{who} page {page}")])))
        .collect();
    recto_testpdf::pages(&pages)
}

/// Another program's save: a new file renamed over the old one.
fn replace(path: &Path, bytes: &[u8]) {
    let new = path.with_extension("incoming");
    std::fs::write(&new, bytes).expect("the new version");
    std::fs::rename(&new, path).expect("put in place");
}

/// The earlier version put back as it was, bytes and modification time, as a backup tool restores it: a new file, under a new number.
fn put_back(path: &Path, bytes: &[u8], modified: std::time::SystemTime) {
    let new = path.with_extension("restored");
    std::fs::write(&new, bytes).expect("the old version");
    std::fs::File::options()
        .write(true)
        .open(&new)
        .and_then(|file| file.set_modified(modified))
        .expect("its time");
    std::fs::rename(&new, path).expect("put back");
}

/// The app comes back to the front, so each file showing is looked at.
fn look(app: &HeadlessApp) {
    app.send(CoreCall::LookAtShown);
    app.settle();
}

fn notice(app: &HeadlessApp) -> Option<String> {
    app.tabs().into_iter().next().and_then(|row| row.notice)
}

/// `doc.pdf`, ten pages, open, with page 9 turned by the person.
fn edited() -> (HeadlessApp, PathBuf, Vec<u8>, std::time::SystemTime) {
    let app = HeadlessApp::start();
    let original = pdf_bytes("Mine", 10);
    let file = app.disk().file("doc.pdf", &original);
    let modified = std::fs::metadata(&file).and_then(|meta| meta.modified()).expect("its time");
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "9", "--by", "90"]).expect("rotated");
    app.settle();
    (app, file, original, modified)
}

/// Failure (B2): the edits made on a file another program then replaced could be saved nowhere, while every word said otherwise: Save's refusal said
/// "Save As keeps both", the agent was told "`recto save-as` keeps both versions", and the person's sheet offered Save As, all of which were refused too. Save
/// As stays refused (the edits would land on the new file's pages, in a new file), and the refusal, the agent's next step, the sheet and the tab's notice say
/// what is so: the edits were made on a version that isn't there, they're kept, nothing's written. Putting that version back, bytes and time, makes them fit
/// again, and then they save.
#[test]
fn edits_on_a_replaced_file_save_nowhere_say_so_and_fit_again_when_its_put_back() {
    let (app, file, original, modified) = edited();
    let theirs = pdf_bytes("Theirs", 5);
    replace(&file, &theirs);
    look(&app);
    let said = notice(&app).expect("the tab says what happened");
    assert!(said.contains("changed on disk") && said.contains("kept"), "{said}");
    assert!(!said.to_lowercase().contains("save as"), "{said}");
    let copy = file.with_file_name("copy.pdf");
    for words in [vec!["save"], vec!["save-as", "--out", copy.to_str().unwrap()]] {
        let refused = app.recto(Level::Full, &words).expect_err("refused");
        assert_eq!(refused.code, "file_changed", "{words:?}: {refused:?}");
        for text in [&refused.message, &refused.next] {
            let text = text.to_lowercase();
            assert!(
                !text.contains("keeps both") && !text.contains("save-as") && !text.contains("save as keeps"),
                "{words:?}: {text}"
            );
        }
        assert!(refused.message.contains("Nothing was written"), "{refused:?}");
    }
    assert!(!copy.exists(), "no copy was made");
    assert_eq!(std::fs::read(&file).expect("the file"), theirs, "and the other program's version is as it was");
    // The person's Save: the same refusal, with no Save As offered.
    app.core().perform("file.save".into());
    app.settle();
    let problem = app.core().tools.save_ui.problem().expect("the person's told");
    assert_eq!(problem.save_as_title, None, "Save As isn't offered, as it would be refused: {problem:?}");
    assert!(!problem.message.contains("Save As"), "{problem:?}");
    assert!(app.reader().edited, "the edits are still here");
    // The version they were made on, put back as it was: they fit it, the tab says nothing, and they save.
    put_back(&file, &original, modified);
    look(&app);
    assert_eq!(notice(&app), None);
    app.recto(Level::Full, &["save"]).expect("saved");
    app.settle();
    let saved = std::fs::read(&file).expect("the file");
    assert_ne!(saved, original, "the turn went in");
    assert!(saved.starts_with(&original), "appended to the version they were made on");
}

/// The tab with this file name's selector, `#n`.
fn tab_of(app: &HeadlessApp, name: &str) -> String {
    let tabs = app.session_json()["tabs"].clone();
    let tab = tabs
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(name))))
        .unwrap_or_else(|| panic!("a tab of {name}: {tabs}"));
    format!("#{}", tab["id"].as_u64().expect("its number"))
}

/// **The ordering that leaves an edited tab with no stamp** (the branch's own `an_edit_made_before_a_changed_file_is_read_again_is_kept`, on the app): `doc.pdf`,
/// ten pages, open behind `other.pdf`; another program makes it five pages and the tab hears of it, its stamp moving to the new file, its length still the old
/// one's as it hasn't been read; an agent turns its page 9 before the read; the read lands at five pages, so the list (ten long) isn't about it and the stamp
/// goes. Gives the app, the file, the five pages put in, and the tab's selector.
fn dropped_stamp() -> (HeadlessApp, PathBuf, Vec<u8>, String) {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    app.open(&file);
    app.open(&app.disk().pdf("other.pdf", 2));
    let tab = tab_of(&app, "doc.pdf");
    let id = recto_session::TabId(tab[1..].parse().expect("a number"));
    let before = app.core().host.session.with_session(|s| s.tabs().own_stamp(id));
    let theirs = pdf_bytes("Theirs", 5);
    replace(&file, &theirs);
    let now = recto_platform::file_stamp(&file).expect("its stamp");
    app.send(CoreCall::WokenFileIsOther(id, recto_session::record::FileStatus::Changed(now), before));
    app.settle();
    app.recto(Level::Control, &["page", "rotate", "9", "--by", "90", "--tab", &tab])
        .expect("an agent's turn on the tab behind");
    app.settle();
    app.recto(Level::Full, &["tap", &tab]).expect("to it");
    app.settle();
    assert_eq!(
        app.core().host.session.with_session(|s| s.tabs().own_stamp(id)),
        None,
        "the ordering drops the stamp"
    );
    (app, file, theirs, tab)
}

/// Failure (S1, review round 1): with the stamp dropped, the tab's pages were still taken to be current, so `text`, `render`, `select`, `export` and print to PDF
/// read the five-page file by the ten-page list's numbers, and print to PDF wrote the edited composition over the other version's pages into a new file (what
/// Save As is refused for), with no refusal; `search` returned hits on the other version's pages. Each is refused, nothing is written, and no word of the other
/// version comes back as the edited document's. So did `contents`, `signatures` and `facts`, which took no claim for a current one. And (F4a) the agent was told the user could put the earlier version back, which nothing names here.
#[test]
fn with_no_stamp_nothing_reads_the_other_version_as_the_edited_document() {
    let (app, file, theirs, tab) = dropped_stamp();
    let out = |name: &str| file.with_file_name(name);
    let refused = |words: &[&str]| -> recto_agent::AgentError {
        let mut words = words.to_vec();
        words.extend(["--tab", &tab]);
        let error = app.recto(Level::Full, &words).expect_err(&format!("{words:?} refused"));
        assert!(!format!("{error:?}").contains("Theirs"), "{words:?}: nothing of the other version: {error:?}");
        error
    };
    // Contents, signatures and facts read the file by its pages too, against the edited list's numbers.
    assert_eq!(refused(&["contents"]).code, "source_changed", "contents");
    assert_eq!(refused(&["signatures"]).code, "source_changed", "signatures");
    let facts = app.recto(Level::Full, &["facts", "--tab", &tab]).expect("facts");
    assert_eq!(facts["file_changed_since_edits"], true, "{facts}");
    for page in ["3", "9"] {
        assert_eq!(refused(&["text", "--page", page]).code, "source_changed", "text of page {page}");
        assert_eq!(refused(&["select", page]).code, "source_changed", "select on page {page}");
    }
    let png = out("p3.png");
    assert_eq!(refused(&["render", "3", "--out", png.to_str().unwrap()]).code, "source_changed");
    let svg = out("p3.svg");
    assert_eq!(refused(&["export", "svg", "3", "--out", svg.to_str().unwrap()]).code, "source_changed");
    let printed = out("printed.pdf");
    assert_eq!(refused(&["print", "--to-pdf", printed.to_str().unwrap()]).code, "source_changed");
    for written in [&png, &svg, &printed] {
        assert!(!written.exists(), "nothing written: {written:?}");
    }
    // Search, Find and Save are refused as the file changed under the edits, with nothing to put back: no version is known to be theirs (F4a).
    for words in [vec!["search", "Theirs"], vec!["find", "Theirs"], vec!["save"]] {
        // `find` is of the tab being read, which this one is.
        let refused = match words[0] {
            "find" => app.recto(Level::Full, &words).expect_err("find refused"),
            _ => refused(&words),
        };
        assert_eq!(refused.code, "file_changed", "{words:?}: {refused:?}");
        assert!(refused.message.contains("changed on disk"), "{words:?}: the reason is said: {refused:?}");
        assert!(
            !refused.next.contains("earlier version back") && refused.next.contains("discard"),
            "{words:?}: no version to put back: {refused:?}"
        );
    }
    assert_eq!(std::fs::read(&file).expect("the file"), theirs, "and the file is as the other program left it");
}

/// What Find shows: its rows, its status, and whether it's still looking.
fn find_shown(app: &HeadlessApp) -> (u32, String, bool) {
    let view = crate::core::find::view(app.core());
    (view.row_count, view.status, view.running)
}

/// Failure (B5): Find's hits are about a version of the file, but on an edited tab what they were about was the edits' stamp, which never moves, so hits found
/// on the earlier version stayed listed over the new one, never found again. Find can't say which page of this file a match is on until the edits fit it (the
/// page matcher's work), so it says so, plainly, for the person and an agent alike: the hits go when the change is taken in, the field says why, and an
/// agent's `find` is refused in the same words with the same next step as its save. And (F5) an agent's `search` was let through, returning hits on the other
/// version's pages: it's refused for the same reason.
#[test]
fn find_on_an_edited_tab_whose_file_changed_says_it_cant_search() {
    let (app, file, original, modified) = edited();
    app.act(crate::core::controls::UiAction::FindTyped { text: "Mine page".into() });
    app.settle();
    assert!(find_shown(&app).0 > 0, "hits on the version the edits were made on: {:?}", find_shown(&app));
    replace(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    let (rows, words, running) = find_shown(&app);
    assert_eq!((rows, running), (0, false), "the earlier version's hits are gone: {words}");
    let refused = app.recto(Level::Full, &["find", "Theirs"]).expect_err("refused");
    assert_eq!(refused.code, "file_changed", "{refused:?}");
    assert!(refused.message.contains("changed on disk"), "the reason is said: {refused:?}");
    assert_eq!(words, refused.message, "the person's Find says what an agent's is told");
    assert!(refused.next.contains("earlier version back"), "{refused:?}");
    // F5: an agent's search is refused for the same reason, with the same way on, rather than searching the other version's pages.
    let searched = app.recto(Level::Full, &["search", "Theirs"]).expect_err("search refused too");
    assert_eq!((searched.code, &searched.next), (refused.code, &refused.next), "{searched:?}");
    assert!(searched.message.contains("changed on disk"), "{searched:?}");
    // Looking again isn't news, and Find goes on saying the same.
    look(&app);
    assert_eq!(find_shown(&app).1, words);
    // The version the edits were made on, back: Find finds in it again.
    put_back(&file, &original, modified);
    look(&app);
    app.settle();
    let (rows, status, _) = find_shown(&app);
    assert!(rows > 0, "found again: {status}");
}

/// Failure (B5): every return to the front read an edited tab's changed file again and reset its contents' folds, as each look took the change in anew. A look
/// that finds what's already known does nothing: a fold made after the change is taken in stays across looks.
#[test]
fn looking_again_at_an_edited_tab_whose_file_changed_changes_nothing() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &super::files::with_sections(6));
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "2", "--by", "90"]).expect("rotated");
    app.settle();
    replace(&file, &super::files::with_sections(8));
    look(&app);
    let tab = app.session_json()["tabs"][0]["id"].as_u64().expect("a tab");
    app.agent(Level::Control, &format!(r##"{{"cmd":"contents_fold","entry":0,"tab":"#{tab}"}}"##))
        .expect("folded");
    app.settle();
    let folded = app.session_json()["tabs"][0].clone();
    for _ in 0..3 {
        look(&app);
    }
    assert_eq!(app.session_json()["tabs"][0], folded, "nothing about the tab changed");
}

/// What every change under the edits must leave: the edits as they were, the tab saying its file changed, Save and Save As refused with nothing written, and
/// the file as the other program left it.
fn assert_kept_and_nothing_written(app: &HeadlessApp, file: &Path, theirs: &[u8], edits_before: &serde_json::Value, what: &str) {
    assert_eq!(&app.session_json()["tabs"][0]["edits"], edits_before, "{what}: the edits as they were");
    assert_eq!(app.session_json()["tabs"][0]["notice"], "changed", "{what}");
    let copy = file.with_file_name("copy.pdf");
    for words in [vec!["save"], vec!["save-as", "--out", copy.to_str().unwrap()]] {
        let refused = app.recto(Level::Full, &words).expect_err("refused");
        assert_eq!(refused.code, "file_changed", "{what}, {words:?}: {refused:?}");
    }
    assert!(!copy.exists(), "{what}");
    assert_eq!(std::fs::read(file).expect("the file"), theirs, "{what}: nothing written");
}

/// Pinned for the page matcher (2026-11-13): page 9 of ten turned, and the file becomes five pages, then the same ten pages in another order, then fourteen.
/// No panic anywhere, the edits are kept, nothing is written, and asking for a page, inside the new file's end or past it, fails in words rather than reading
/// the other version as the edited document's page (the test once took any answer).
/// (The edited document stays the edit list's ten positions whatever the file's length: the four pages appended aren't reachable until the matcher.)
#[test]
fn edits_under_a_file_that_shrinks_reorders_or_grows_are_kept_and_nothing_is_written() {
    let (app, file, _original, _modified) = edited();
    let edits_before = app.session_json()["tabs"][0]["edits"].clone();
    let five = pdf_bytes("Theirs", 5);
    replace(&file, &five);
    look(&app);
    assert_kept_and_nothing_written(&app, &file, &five, &edits_before, "five pages");
    // Inside the five pages and past them alike: the edited document's page isn't read from a version its edits weren't made on, so it's refused, in words.
    for page in ["3", "6", "9", "10"] {
        let refused = app.recto(Level::Full, &["text", "--page", page]).expect_err(&format!("page {page} refused"));
        assert_eq!(refused.code, "source_changed", "page {page}: {refused:?}");
        assert!(!refused.message.is_empty(), "page {page}: {refused:?}");
    }
    let reordered: Vec<recto_testpdf::Page> = (1..=10)
        .rev()
        .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Mine page {page}")])))
        .collect();
    let reordered = recto_testpdf::pages(&reordered);
    replace(&file, &reordered);
    look(&app);
    assert_kept_and_nothing_written(&app, &file, &reordered, &edits_before, "the same pages reordered");
    let fourteen = pdf_bytes("Mine", 14);
    replace(&file, &fourteen);
    look(&app);
    assert_kept_and_nothing_written(&app, &file, &fourteen, &edits_before, "four pages appended");
    // Marks what the page matcher changes: until it, the edited document is the list's ten positions whatever the file's length, so the four pages appended
    // aren't in it. Nothing a person relies on is pinned here; when the matcher lands, this is the line it means to change.
    assert_eq!(
        app.session_json()["tabs"][0]["edits"]["pages"],
        10,
        "the appended pages aren't in the edited document, until the matcher"
    );
}

/// Pinned for the page matcher: the file under an edited tab is caught half-written (cut off in the middle of an object), as a save in progress or a copy
/// from a disk that went away leaves it. No panic in the loader or the session, the edits are kept, and nothing is written.
#[test]
fn a_half_written_file_under_edits_panics_nothing_and_keeps_them() {
    let (app, file, _original, _modified) = edited();
    let edits_before = app.session_json()["tabs"][0]["edits"].clone();
    let whole = pdf_bytes("Theirs", 10);
    let cut = whole[..whole.len() / 2].to_vec();
    replace(&file, &cut);
    look(&app);
    assert_kept_and_nothing_written(&app, &file, &cut, &edits_before, "half-written");
    let other = app.disk().pdf("other.pdf", 2);
    app.open(&other);
    let tab = app.session_json()["tabs"][0]["id"].as_u64().expect("a tab");
    app.recto(Level::Full, &["tap", &format!("#{tab}")]).expect("back to it");
    app.settle();
    assert_eq!(app.session_json()["tabs"][0]["edits"], edits_before, "read again, still kept");
}

/// Failure (F8): a look's result was taken in without asking which stamp it was made against. A save landing between a look at the file (made against the stamp
/// before it) and the look's result being taken in made the result "changed": the just-saved list's undo history was cleared as if another program had
/// written the file, and the tab said its file changed on disk. A result made against a stamp the tab has moved on from says nothing.
#[test]
fn a_look_made_before_a_save_landed_changes_nothing() {
    let (app, file, _original, _modified) = edited();
    let id = recto_session::TabId(tab_of(&app, "doc.pdf")[1..].parse().expect("a number"));
    let before = app.core().host.session.with_session(|s| s.tabs().own_stamp(id));
    let status = || recto_session::record::look_at_file(&file, before);
    app.recto(Level::Full, &["save"]).expect("saved");
    app.settle();
    assert!(
        app.core().host.session.with_session(|s| s.lists().undo_and_redo(id).0),
        "the save keeps its history"
    );
    let late = status();
    assert!(
        matches!(late, recto_session::record::FileStatus::Changed(_)),
        "a look against the old stamp finds the save: {late:?}"
    );
    app.send(CoreCall::WokenFileIsOther(id, late, before));
    app.settle();
    assert!(
        app.core().host.session.with_session(|s| s.lists().undo_and_redo(id).0),
        "the history is still there"
    );
    assert_eq!(notice(&app), None, "and the tab says nothing changed");
}
