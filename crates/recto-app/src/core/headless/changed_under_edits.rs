//! **A tab's file changed on disk by another program while it has unsaved edits,** on the app's own core with real files. The page matcher (owner,
//! 2026-11-13; slice 2, 2026-11-15) follows every edit onto its page in the new version, or holds the whole list: held, nothing is written, the edits are
//! kept, and the words say what's true (they can't be saved, here or as a copy, while the other version is in place); followed, the tab says so in the
//! owner's words, an agent watching hears it, and `edits` says it.

use crate::queue::CoreCall;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;
use serde_json::json;
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
    // The edits are still the tab's (Discard is what lets them go) and nothing's written; the owner's words of 2026-11-15 say it without "kept".
    assert!(
        said.contains("changed on disk") && said.contains("pages aren't shown") && said.contains("Discard them"),
        "{said}"
    );
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
    // File > Save does something only when it can be chosen (CI run 43465773035: a look at the file that read two versions greyed it, and the person's Save
    // did nothing, with nothing said): if it can't, why, as the window has it.
    let reader = app.reader();
    assert!(
        app.core().menu_item_enabled("file.save".into()),
        "Save can't be chosen, so the person's Save would do nothing: edited {}, save on {}, saving {}, readers disagree: {:?}",
        reader.edited,
        reader.save_enabled,
        reader.saving,
        app.core().tools.save_ui().disagreement_now()
    );
    app.core().perform("file.save".into());
    app.settle();
    app.wait_for("the person's Save answered in the window", || {
        app.core().tools.save_ui().problem().map(|_| ()).ok_or_else(|| {
            let reader = app.reader();
            format!("no problem to show; save on {}, saving {}", reader.save_enabled, reader.saving)
        })
    });
    let problem = app.core().tools.save_ui().problem().expect("the person's told");
    assert_eq!(problem.save_as_title, None, "Save As isn't offered, as it would be refused: {problem:?}");
    assert!(!problem.message.contains("Save As"), "{problem:?}");
    assert!(app.reader().edited, "the edits are still here");
    // The version they were made on, put back as it was by a backup tool, under a new number: another file in its place, so the page matcher looks,
    // finds every page, and the edits follow onto it, said (the coordinator's ruling of 2026-11-15: never silent about a replaced file; a version put back
    // under its own number fits silently, `edits_fit_again_when_their_version_is_written_back_over_the_file_in_place`). Then they save.
    put_back(&file, &original, modified);
    look(&app);
    assert_eq!(notice(&app).as_deref(), Some(FOLLOWED), "said, not silent");
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

/// Pinned for the page matcher (2026-11-13), and changed by it as it meant to (slice 2): page 9 of ten turned, and the file becomes five other pages (held:
/// no panic, the edits kept, nothing written, and asking for a page, inside the new file's end or past it, fails in words rather than reading the other version
/// as the edited document's page), then the same ten pages in another order (followed: the turn on "Mine page 9", second now), then fourteen in order
/// (followed again: the turn on page 9, and the four pages another program appended are the document's). Failure caught: an edit on another page than its
/// own, or held where its page is plainly there, or the appended pages out of reach.
#[test]
fn edits_under_a_file_that_shrinks_reorders_or_grows_are_held_or_follow_their_page() {
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
    assert_eq!(notice(&app).as_deref(), Some(FOLLOWED));
    assert_eq!(
        turned_text(&app),
        vec![(2, "Mine page 9".to_owned())],
        "the turn on its own page, second in the new order"
    );
    assert_eq!(std::fs::read(&file).expect("the file"), reordered, "nothing written");
    let fourteen = pdf_bytes("Mine", 14);
    replace(&file, &fourteen);
    look(&app);
    assert_eq!(notice(&app).as_deref(), Some(FOLLOWED));
    assert_eq!(turned_text(&app), vec![(9, "Mine page 9".to_owned())]);
    assert_eq!(
        app.session_json()["tabs"][0]["edits"]["pages"],
        14,
        "the four pages appended are the document's"
    );
    let page_14 = app.recto(Level::Full, &["text", "--page", "14"]).expect("page 14 is there");
    assert!(page_14.to_string().contains("Mine page 14"), "{page_14}");
}

/// The owner's words for a list that followed (2026-11-15).
const FOLLOWED: &str = "This file was changed by another program. Your unsaved edits were moved onto the same pages of the new version.";

/// Each turned page of the edited document, 1-based, with its text as an agent reads it.
fn turned_text(app: &HeadlessApp) -> Vec<(u32, String)> {
    let edits = app.recto(Level::Full, &["edits"]).expect("edits");
    let mut out = Vec::new();
    for run in edits["runs"].as_array().expect("runs") {
        if run["rotation_degrees"] == 0 {
            continue;
        }
        let pages = run["pages"].as_str().expect("pages");
        let page: u32 = pages.split('-').next().and_then(|page| page.parse().ok()).expect("a page");
        let text = app.recto(Level::Full, &["text", "--page", &page.to_string()]).expect("its text");
        let text = text["untrusted"]["text"].as_str().map_or_else(|| text.to_string(), str::to_owned);
        out.push((page, text.trim().to_owned()));
    }
    out
}

/// Failure caught: a page another program inserted before the edited one, and the turn shown on the page that's at its old number now; the person not
/// told, or told in other words; an agent watching not hearing it (an announcement is made when something changed), or `edits` not saying it, or saying a
/// digest. The turn follows "Mine page 9" to page 10, the tab and `edits` say so in counts, the agent hears `edits_followed` once, and leaving the tab lets
/// the news go.
#[test]
fn a_turn_follows_its_page_past_a_cover_another_program_added_and_is_said_and_heard() {
    let (app, file, _original, _modified) = edited();
    let mut watching = app.agent_watches(Level::Control);
    let mut with_cover = vec![recto_testpdf::Page::new(recto_testpdf::text_lines(&["Their cover"]))];
    with_cover.extend((1..=10).map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Mine page {page}")]))));
    replace(&file, &recto_testpdf::pages(&with_cover));
    look(&app);
    assert_eq!(notice(&app).as_deref(), Some(FOLLOWED));
    assert_eq!(turned_text(&app), vec![(10, "Mine page 9".to_owned())]);
    let edits = app.recto(Level::Control, &["edits"]).expect("edits");
    assert_eq!(edits["followed"]["edits"], 1, "{edits}");
    assert_eq!(edits["followed"]["onto"]["bytes"], std::fs::metadata(&file).expect("its size").len(), "{edits}");
    assert_eq!(edits["pages"], 11);
    assert!(!edits["can_undo"].as_bool().expect("can_undo"), "the history ends at the change");
    let heard: Vec<serde_json::Value> = watching.heard().into_iter().filter(|event| event["event"] == "edits_followed").collect();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!((heard[0]["pages"].as_u64(), heard[0]["edits"].as_u64()), (Some(11), Some(1)), "{heard:?}");
    // Counts and the file's own stamp: nothing of what a page is (`qa state`'s counts of named pages are slice 1's, held by its own test).
    let said = format!("{}{}", edits["followed"], heard[0]);
    assert!(!said.contains("digest") && !said.contains("identit"), "{said}");
    assert_eq!(
        edits["followed"].as_object().map(|followed| followed.len()),
        Some(2),
        "how many, and onto which version: {said}"
    );
    // Nothing more is heard for it: a look again finds what's known.
    look(&app);
    assert!(watching.heard().iter().all(|event| event["event"] != "edits_followed"), "said once");
    app.recto(Level::Full, &["save"]).expect("saved onto the new version");
    app.settle();
    let saved = std::fs::read(&file).expect("the file");
    assert!(
        saved.starts_with(&recto_testpdf::pages(&with_cover)),
        "appended to the new version, the other program's cover kept"
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

/// Failure (F8, at its other two call sites): a look's result taken in without asking which stamp it was made against. A save landing between a look at a
/// file (made against the stamp before it) and its result being taken in made the result "changed": the just-saved list's history was cleared as if another
/// program had written the file, and the tab said its file changed on disk. The look before a tab is shown and the look at the tabs not showing each say
/// nothing of a result made against a stamp the tab has moved on from.
#[test]
fn a_look_at_a_shown_or_other_tab_made_before_a_save_landed_changes_nothing() {
    type Take = fn(&crate::core::Inner, recto_session::TabId, Option<recto_platform::FileStamp>, recto_session::record::FileStatus) -> bool;
    let looks: [(&str, Take); 2] = [
        ("before showing", |inner, id, stamp, status| {
            crate::core::sleeper::take_shown_look(&inner.host.session, &inner.core, id, stamp, status)
        }),
        ("at the others", crate::core::session::take_other_look),
    ];
    for (what, take) in looks {
        let (app, file, _original, _modified) = edited();
        let id = recto_session::TabId(tab_of(&app, "doc.pdf")[1..].parse().expect("a number"));
        let before = app.core().host.session.with_session(|s| s.tabs().own_stamp(id));
        app.recto(Level::Full, &["save"]).expect("saved");
        app.settle();
        let late = recto_session::record::look_at_file(&file, before);
        assert!(
            matches!(late, recto_session::record::FileStatus::Changed(_)),
            "{what}: the look finds the save: {late:?}"
        );
        assert!(!take(app.core(), id, before, late), "{what}: about nothing now");
        app.settle();
        assert!(
            app.core().host.session.with_session(|s| s.lists().undo_and_redo(id).0),
            "{what}: the history is still there"
        );
        assert_eq!(notice(&app), None, "{what}: and the tab says nothing changed");
    }
}

/// Another program's save that writes over the file where it is (`cp` onto it), so it keeps its number.
fn rewrite_in_place(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).expect("written over");
}

/// Failure (the first worker's quiet run of 2026-11-13, `06-*-f-original-back`): the version the edits were made on, put back by writing it over the file in
/// place (`cp`, then `touch -r`), is the same number, size and modification time as the stamp the edits keep, so the look the app makes on coming to the front
/// found the file "as it was" against that stamp and told the session nothing: the notice stayed, and Save stayed refused, while the same bytes put back under a
/// new number (the test above) fitted. Rewritten in place both ways, through the app-front look: the notice goes, and the edits save onto that version.
#[test]
fn edits_fit_again_when_their_version_is_written_back_over_the_file_in_place() {
    let (app, file, original, modified) = edited();
    let number = |path: &Path| std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(path).expect("its facts"));
    let before = number(&file);
    rewrite_in_place(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    assert!(notice(&app).is_some_and(|said| said.contains("changed on disk")), "{:?}", notice(&app));
    rewrite_in_place(&file, &original);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .and_then(|open| open.set_modified(modified))
        .expect("its time");
    assert_eq!(number(&file), before, "the same file, as `cp` over it leaves it");
    look(&app);
    assert_eq!(notice(&app), None, "the version the edits were made on is back");
    // Silently, and with its history: under its own number it's the edits' own version, not one they followed onto.
    let edits = app.recto(Level::Full, &["edits"]).expect("edits");
    assert_eq!(
        (edits["can_undo"].as_bool(), &edits["followed"]),
        (Some(true), &serde_json::Value::Null),
        "{edits}"
    );
    app.recto(Level::Full, &["save"]).expect("saved");
    app.settle();
    let saved = std::fs::read(&file).expect("the file");
    assert!(
        saved.len() > original.len() && saved.starts_with(&original),
        "appended to the version they were made on"
    );
}

/// Failure (the first worker's quiet run of 2026-11-13, `06-*-c-find-typed`): Find typed by the person after the file changed under the edits showed no hits
/// and no words, while an agent's `find` was told why. The person's refused query left nothing for the line under the field: only a query typed before the
/// change had its refusal kept against the change (the test above). Typed after: the window's Find says what the agent is told, and finds again once the
/// version the edits were made on is back.
#[test]
fn find_typed_after_the_file_changed_under_the_edits_says_why_it_cant_search() {
    let (app, file, original, modified) = edited();
    replace(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    app.act(crate::core::controls::UiAction::FindTyped { text: "page".into() });
    app.settle();
    let (rows, words, running) = find_shown(&app);
    let refused = app.recto(Level::Full, &["find", "page"]).expect_err("refused");
    assert_eq!((rows, running), (0, false), "{words}");
    assert_eq!(words, refused.message, "the person's Find says what an agent's is told");
    put_back(&file, &original, modified);
    look(&app);
    app.settle();
    let (rows, status, _) = find_shown(&app);
    assert!(rows > 0, "found again: {status}");
}

/// Failure caught (the coordinator's rule, 2026-11-15): a document opened with its password has its pages matched against a new version, so its edits
/// follow, or an agent learns from what it hears or what `edits` says whether they would have (which says something of its pages). Two protected
/// documents, each with page 9 turned by the person, each changed by another program: one as the edits would follow (pages appended), one as they'd be
/// held (other pages). An agent watching at Full hears the same of both and gets the same answer from `edits`, and neither tab says its edits followed.
#[test]
fn two_protected_documents_changed_under_edits_tell_an_agent_the_same() {
    use super::files::locked_saying;
    use super::withheld_events::{PASSWORD, person_unlocks, protected};
    let run = |pages: usize, word: &str| {
        let disk = crate::testkit::headless::Disk::new();
        let file = protected(&disk, 10, "Locked");
        let app = disk.start();
        person_unlocks(&app, &file);
        let mut watching = app.agent_watches(Level::Full);
        app.person(r#"{"cmd":"page_rotate","tab":"active","pages":"9","degrees":90}"#);
        app.settle();
        let before = watching.heard();
        replace(&file, &locked_saying(PASSWORD, pages, word, lopdf::encryption::Permissions::all()));
        look(&app);
        let heard = watching.heard();
        let edits = app
            .recto(Level::Full, &["edits"])
            .map_or_else(|refused| refused.code.to_string(), |answer| answer.to_string());
        assert_ne!(
            notice(&app).as_deref(),
            Some(FOLLOWED),
            "{pages} pages of {word}: a protected tab's edits never follow"
        );
        (before, heard, edits)
    };
    let would_follow = run(12, "Locked");
    let would_be_held = run(10, "Theirs");
    assert_eq!(would_follow, would_be_held, "the same, heard and asked");
}

/// The page the session has the tab on, counted from 1, read off the session (not by a command, which would end with a look of its own).
fn reading_at(app: &HeadlessApp) -> u32 {
    let page = app
        .core()
        .host
        .session
        .with_session(|s| s.tabs().active().and_then(|id| s.tabs().tab(id)).map(|tab| tab.page));
    page.expect("a tab being read") + 1
}

/// Failure caught (review F1): the edits follow, and the person reading page 25 of 30, far from the page they turned (page 4), is taken to the page
/// beside the turned one, as their place was mapped to the nearest edited page; or Back goes there too. With a cover another program put first, the
/// tab is on "Mine page 25" (26 now), and Back is on "Mine page 10" (11 now): each place keeps its page, by the nearest known page and the distance from
/// it.
#[test]
fn the_reading_place_and_the_trail_keep_their_pages_when_the_edits_follow() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 30));
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "4", "--by", "90"]).expect("rotated");
    for page in [10, 25] {
        app.agent(Level::Control, &format!(r#"{{"cmd":"goto","page":{page}}}"#)).expect("went");
    }
    app.settle();
    assert_eq!(reading_at(&app), 25);
    let mut with_cover = vec![recto_testpdf::Page::new(recto_testpdf::text_lines(&["Their cover"]))];
    with_cover.extend((1..=30).map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Mine page {page}")]))));
    replace(&file, &recto_testpdf::pages(&with_cover));
    look(&app);
    assert_eq!(notice(&app).as_deref(), Some(FOLLOWED));
    assert_eq!(turned_text(&app), vec![(5, "Mine page 4".to_owned())]);
    assert_eq!(reading_at(&app), 26, "still on \"Mine page 25\", one on for the cover");
    app.agent(Level::Control, r#"{"cmd":"back"}"#).expect("back");
    app.settle();
    assert_eq!(reading_at(&app), 11, "Back is on \"Mine page 10\"");
}

/// Failure caught (review F3, decided 2026-11-15): the edits follow, and a quit (or a hand-off, or a crash) before the person has left the tab loses the
/// notice, which is never in the record, so the person never learns their edits were moved onto another version. The sidecar's header keeps "followed,
/// not yet told": after a quit straight after the follow the tab says so again, with `edits`'s count; after the person has left the tab (told), a quit
/// brings nothing back.
#[test]
fn the_followed_notice_comes_back_after_a_quit_until_the_person_has_left_the_tab() {
    let (app, file, _original, _modified) = edited();
    let other = app.disk().file("other.pdf", &pdf_bytes("Other", 3));
    app.open(&other);
    app.open(&file);
    app.settle();
    let mut with_cover = vec![recto_testpdf::Page::new(recto_testpdf::text_lines(&["Their cover"]))];
    with_cover.extend((1..=10).map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("Mine page {page}")]))));
    replace(&file, &recto_testpdf::pages(&with_cover));
    look(&app);
    assert_eq!(notice(&app).as_deref(), Some(FOLLOWED));
    let (_, disk) = app.quit();
    let app = disk.start();
    app.settle();
    assert_eq!(notice(&app).as_deref(), Some(FOLLOWED), "not yet told: said again");
    let edits = app.recto(Level::Control, &["edits"]).expect("edits");
    assert_eq!(edits["followed"]["edits"], 1, "{edits}");
    assert_eq!(turned_text(&app), vec![(10, "Mine page 9".to_owned())]);
    // The person goes to the other tab: told. A quit now brings nothing back.
    app.open(&other);
    app.settle();
    let (_, disk) = app.quit();
    let app = disk.start();
    app.settle();
    let first = app.tabs().into_iter().find(|row| row.notice.is_some());
    assert!(first.is_none(), "told, so nothing said again: {first:?}");
    assert_eq!(turned_text_of_tab(&app, &file), vec![(10, "Mine page 9".to_owned())]);
}

/// [`turned_text`] of the tab on `file`, made active first.
fn turned_text_of_tab(app: &HeadlessApp, file: &Path) -> Vec<(u32, String)> {
    app.open(file);
    app.settle();
    turned_text(app)
}

/// Failure caught (review F5): under unsaved edits, another program's version B is put in place and held (its page 9 isn't the edits'), then version C,
/// another file with B's size and time, is renamed in; the look takes C for B, seen already, as it goes by size and time, and C is never looked at, so
/// the edits stay held (and said changed) though C has every page they use. Another inode is another version under unsaved edits: C is read and the
/// edits follow onto it.
#[test]
fn another_file_with_the_held_versions_size_and_time_is_looked_at() {
    let (app, file, original, _modified) = edited();
    let theirs = pdf_bytes("Ours", 10);
    assert_eq!(theirs.len(), original.len(), "the same size");
    replace(&file, &theirs);
    look(&app);
    assert_eq!(app.session_json()["tabs"][0]["notice"], "changed", "B: page 9 isn't the edits', held");
    let b_time = std::fs::metadata(&file).and_then(|meta| meta.modified()).expect("B's time");
    put_back(&file, &original, b_time);
    look(&app);
    assert_eq!(notice(&app).as_deref(), Some(FOLLOWED), "C, with B's size and time, is looked at and followed");
    assert_eq!(turned_text(&app), vec![(9, "Mine page 9".to_owned())]);
}

/// The view the host would have: a Retina window, the page at its fit.
fn window_view() -> recto_reader::Viewport {
    recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

/// Draws what's in the window's view, as the host would with its UI thread, till it's at rest: where the view is now, so a test's place means what it says.
fn drawn(app: &HeadlessApp) {
    let report = app.core().engine.report().0;
    let at = |key: &str| report[key].as_f64().unwrap_or(0.0);
    let mut view = recto_reader::Viewport {
        x: at("scroll_x"),
        y: at("scroll_y"),
        ..window_view()
    };
    crate::testkit::headless::applied_as_the_host(&app.core().engine.set_viewport(view));
    crate::testkit::headless::host_turns(app, &mut view, true);
}

/// The tiles the reading page view has on pages and keeps, and whether it withholds the tab's own pages.
fn tiles(app: &HeadlessApp) -> (u64, u64, bool) {
    let report = app.core().engine.report().0;
    let count = |key: &str| report["tiles"][key].as_u64().unwrap_or(u64::MAX);
    (count("attached"), count("kept"), report["pages_withheld"] == true)
}

/// Failure (the QA pass before 0.1.1, on the real app): a 30-page file with unsaved edits (a turn on page 28, a highlight on page 25) replaced on disk by
/// a 12-page version, by rename or written in place. The edits can't follow, so the list is held, and then the tab kept showing the earlier version's
/// drawn page 25, drew the new file's pages 1 to 12 under the held list's numbers, and left pages 13 to 30 blank with nothing said. Now, while held:
/// the tab reads "28 of 30" (the list's count and place), no tile of either version is on any page or kept, the edits' highlights aren't given to draw,
/// the notice says the pages aren't shown, an agent is told the same (`pages_withheld`, its page reads refused as `source_changed`, a new edit
/// refused); Discard brings the tab to the file as it is ("12 of 12"), drawn, with nothing written to it.
#[test]
fn a_held_list_over_a_shorter_file_shows_nothing_of_either_version_till_discard_brings_the_file() {
    for in_place in [false, true] {
        let how = if in_place { "written in place" } else { "renamed over" };
        let app = HeadlessApp::start();
        app.core().engine.waking().window_up();
        let file = app.disk().file("long.pdf", &pdf_bytes("Mine", 30));
        app.open(&file);
        app.recto(Level::Control, &["page", "rotate", "28", "--by", "90"]).expect("turned");
        app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":25,"chars":"0-4"}"##)
            .expect("highlighted");
        app.recto(Level::Full, &["goto", "25"]).expect("to page 25");
        drawn(&app);
        assert!(tiles(&app).0 >= 1, "{how}: page 25 drawn first: {:?}", tiles(&app));
        // The first twelve pages, as the QA pass's generator left them: pages 25 and 28 aren't in it, so the list is held.
        let theirs = pdf_bytes("Mine", 12);
        if in_place {
            std::fs::write(&file, &theirs).expect("written in place");
        } else {
            replace(&file, &theirs);
        }
        look(&app);
        drawn(&app);
        let said = notice(&app).expect("the tab says what happened");
        assert!(said.contains("pages aren't shown") && said.contains("Discard"), "{how}: {said}");
        // A page inside the new file's length, where its page 2 could be drawn: nothing is.
        app.recto(Level::Full, &["goto", "2"]).expect("page 2 of the held list");
        drawn(&app);
        assert_eq!(app.reader().place, "2 of 30", "{how}");
        assert_eq!(
            tiles(&app),
            (0, 0, true),
            "{how}: no tile of the new file's page 2: {}",
            app.core().engine.report().0
        );
        app.recto(Level::Full, &["goto", "28"]).expect("page 28 of the held list");
        drawn(&app);
        assert_eq!(app.reader().place, "28 of 30", "{how}: the held list's count and place");
        let (attached, kept, withheld) = tiles(&app);
        assert_eq!(
            (attached, kept, withheld),
            (0, 0, true),
            "{how}: no tile of either version: {}",
            app.core().engine.report().0
        );
        let id = recto_session::TabId(app.session_json()["tabs"][0]["id"].as_u64().expect("a tab") as u32);
        let shown = app
            .core()
            .host
            .session
            .with_session(|s| crate::model::pane_snapshot(s, id, true, &[id]).map(|pane| (pane.withheld, pane.marks.is_none())));
        assert_eq!(shown, Some((true, true)), "{how}: withheld, and its highlights aren't given to draw");
        let at = app.recto(Level::ReadOnly, &["where"]).expect("where");
        assert_eq!(
            (&at["pages_withheld"], &at["pages"], &at["page"]),
            (&json!(true), &json!(30), &json!(28)),
            "{how}: {at}"
        );
        for page in ["5", "28"] {
            let refused = app.recto(Level::Full, &["text", "--page", page]).expect_err("refused");
            assert_eq!(refused.code, "source_changed", "{how}, page {page}: {refused:?}");
        }
        assert!(!app.reader().can_rotate, "{how}: Rotate isn't offered, as it would be refused");
        let refused = app.recto(Level::Control, &["page", "rotate", "3", "--by", "90"]).expect_err("no more edits");
        assert_eq!(refused.code, "file_changed", "{how}: {refused:?}");
        assert!(refused.message.contains("No more edits"), "{how}: {refused:?}");
        app.recto(Level::Full, &["discard"]).expect("discarded");
        drawn(&app);
        assert_eq!(app.reader().place, "12 of 12", "{how}: the file as it is now");
        assert!(app.reader().can_rotate, "{how}: Rotate offered again");
        let (attached, _, withheld) = tiles(&app);
        assert!(attached >= 1 && !withheld, "{how}: its pages drawn: {}", app.core().engine.report().0);
        let at = app.recto(Level::ReadOnly, &["where"]).expect("where");
        assert!(at.get("pages_withheld").is_none(), "{how}: {at}");
        assert_eq!(std::fs::read(&file).expect("the file"), theirs, "{how}: nothing written to the file");
    }
}

/// The kinds of the sidebar's rows under the tab, as an agent's `sidebar` gives them: contents entries and the note over them.
fn rows_under_the_tab(app: &HeadlessApp) -> Vec<String> {
    let sidebar = app.recto(Level::ReadOnly, &["sidebar"]).expect("sidebar");
    sidebar["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row["kind"].as_str())
        .filter(|kind| *kind != "tab")
        .map(String::from)
        .collect()
}

/// Failure (review S2 of `build/shorter-file-pages`): a held tab's contents pane listed the new file's outline, mapped through the held list's
/// positions so a row went to the wrong place, while an agent's `contents` on the same tab was refused; and its page field named pages by the new
/// file's labels, through the same positions. While the pages are withheld the pane has no rows of the file's contents and offers none generated,
/// `contents` is refused, the page field says only the number; after Discard, all of it is the file's again.
#[test]
fn a_held_tabs_contents_and_page_labels_arent_the_new_files() {
    // Contents: six pages with an outline, page 5 turned, made four pages with an outline by another program.
    let app = HeadlessApp::start();
    let file = app.disk().file("outlined.pdf", &super::files::with_sections(6));
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "5", "--by", "90"]).expect("turned");
    app.settle();
    assert!(
        rows_under_the_tab(&app).contains(&"contents".to_string()),
        "its contents first: {:?}",
        rows_under_the_tab(&app)
    );
    let theirs = super::files::with_sections(4);
    replace(&file, &theirs);
    look(&app);
    assert!(app.recto(Level::ReadOnly, &["where"]).expect("where")["pages_withheld"] == true, "held");
    assert_eq!(
        rows_under_the_tab(&app),
        Vec::<String>::new(),
        "no rows of the new file's contents, and none offered"
    );
    assert_eq!(app.sidebar().contents_count, 0);
    let refused = app.recto(Level::Full, &["contents"]).expect_err("an agent's contents is refused");
    assert_eq!(refused.code, "source_changed", "{refused:?}");
    app.recto(Level::Full, &["discard"]).expect("discarded");
    app.settle();
    assert!(
        rows_under_the_tab(&app).contains(&"contents".to_string()),
        "the file's contents again: {:?}",
        rows_under_the_tab(&app)
    );
    // Labels: twelve plain pages, page 12 turned, made the twelve labelled pages of a book (`i` to `iv`, `1` to `6`, `A-1`, `A-2`) by another program.
    let app = HeadlessApp::start();
    let file = app.disk().file("plain.pdf", &pdf_bytes("Mine", 12));
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "12", "--by", "90"]).expect("turned");
    app.recto(Level::Full, &["goto", "2"]).expect("to page 2");
    app.settle();
    replace(&file, &recto_testpdf::linked_document());
    look(&app);
    assert!(app.recto(Level::ReadOnly, &["where"]).expect("where")["pages_withheld"] == true, "held");
    assert_eq!(app.reader().place, "2 of 12", "the number, not the new file's label for its page 2");
    // The person types a label of the new file in the page field: not a page of the held list, so nothing moves.
    app.act(crate::core::controls::UiAction::PageTyped { text: "iv".into() });
    app.settle();
    assert_eq!(app.reader().place, "2 of 12", "a label of the new file isn't a page of the held list");
    app.recto(Level::Full, &["discard"]).expect("discarded");
    app.settle();
    assert_eq!(app.reader().place, "ii (2 of 12)", "the file's label again");
    app.act(crate::core::controls::UiAction::PageTyped { text: "iv".into() });
    app.settle();
    assert_eq!(app.reader().place, "iv (4 of 12)");
}

/// Failure (review F1 of `build/shorter-file-pages`): nothing held that a tab's pages come back when the version its edits were made on is written back
/// over the file in place (same number, bytes and time), where the map the page view is given doesn't change: a withhold kept from before passed every
/// test. Held, then put back so: its pages are drawn again, no notice, and no page says it can't be shown.
#[test]
fn a_tabs_pages_come_back_when_its_edits_version_is_written_back_in_place() {
    let (app, file, original, modified) = edited();
    app.core().engine.waking().window_up();
    drawn(&app);
    rewrite_in_place(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    drawn(&app);
    assert_eq!(tiles(&app), (0, 0, true), "held: {}", app.core().engine.report().0);
    rewrite_in_place(&file, &original);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .and_then(|open| open.set_modified(modified))
        .expect("its time");
    look(&app);
    drawn(&app);
    let (attached, _, withheld) = tiles(&app);
    assert!(attached >= 1 && !withheld, "drawn again: {}", app.core().engine.report().0);
    assert_eq!(notice(&app), None);
    assert_eq!(app.core().engine.note(), None, "no page says it can't be shown");
}

/// Failure (review of `build/shorter-file-pages`): only the tab's own file's pages are withheld while its edits are held, but every highlight went,
/// those on pages added from another PDF too, which are still drawn. Held: the added page is drawn and keeps its highlight; the own file's go.
#[test]
fn a_held_tabs_pages_added_from_another_pdf_keep_their_highlights() {
    let app = HeadlessApp::start();
    app.core().engine.waking().window_up();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    let added = app.disk().file("added.pdf", &pdf_bytes("Added", 2));
    app.open(&file);
    app.recto(Level::Full, &["page", "insert", added.to_str().unwrap(), "--at", "11"])
        .expect("added at the end");
    app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":11,"chars":"0-4"}"##)
        .expect("a highlight on the added page");
    app.agent(Level::Control, r##"{"cmd":"highlight","tab":"active","page":3,"chars":"0-4"}"##)
        .expect("one on the own file's page 3");
    app.recto(Level::Control, &["page", "rotate", "9", "--by", "90"]).expect("turned");
    app.settle();
    replace(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    let id = recto_session::TabId(app.session_json()["tabs"][0]["id"].as_u64().expect("a tab") as u32);
    let marks = app.core().host.session.with_session(|s| {
        crate::model::pane_snapshot(s, id, true, &[id]).and_then(|pane| {
            pane.withheld
                .then_some(pane.marks.map(|marks| marks.pages.iter().map(|(page, _)| *page).collect::<Vec<_>>()))
        })
    });
    assert_eq!(marks, Some(Some(vec![10])), "withheld, with only the added page's highlight (position 10)");
    app.recto(Level::Full, &["goto", "11"]).expect("to the added page");
    drawn(&app);
    let (attached, _, withheld) = tiles(&app);
    assert!(withheld && attached >= 1, "the added page is drawn: {}", app.core().engine.report().0);
}

/// Failure (second review of `build/shorter-file-pages`): a follow whose file changed again before it was read (so it's not read, and the list isn't
/// tried against that version again) left the tab saying it was being looked at, as nothing else made its words again. Held as the follow starts, the
/// file replaced again, then let go: the tab says the held words, with no other look.
#[test]
fn a_follow_whose_file_changed_again_says_the_held_words() {
    let (app, file, _original, _modified) = edited();
    let (held, go) = app.hold_next("follow_starting");
    replace(&file, &pdf_bytes("Theirs", 5));
    app.send(CoreCall::LookAtShown);
    held.recv_timeout(crate::testkit::headless::WAIT_LIMIT).expect("the follow held as it starts");
    replace(&file, &pdf_bytes("Others", 6));
    drop(go);
    app.settle();
    let said = notice(&app).expect("the tab says what happened");
    assert!(said.contains("Discard them"), "the held words, not still being looked at: {said}");
}
