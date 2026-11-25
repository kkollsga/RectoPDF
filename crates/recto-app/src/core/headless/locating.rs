//! `ref` and `resolve` through the app core, with no window: a reference to a page, and where that page is after another program changed the file, with the
//! person's unsaved edits, and with a protected document. Both commands are one executor (`recto_agent::locating`); the window has no control for them, so
//! everything here is what an agent sees and what it may not change.

use super::changed_under_edits::{look, pdf_bytes, replace};
use super::files::locked_saying;
use super::withheld_events::{PASSWORD, person_unlocks};
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{Level, exit};
use serde_json::{Value, json};
use std::path::Path;

/// `pages`' pages, page by page: each says `who` and its number, so `text` tells which page it is.
fn pages_saying(who: &[&str]) -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = who.iter().map(|words| recto_testpdf::Page::new(recto_testpdf::text_lines(&[words]))).collect();
    recto_testpdf::pages(&pages)
}

fn reference(app: &HeadlessApp, page: u32) -> String {
    let made = app
        .agent(Level::ReadOnly, &json!({ "cmd": "ref", "page": page, "tab": "#1" }).to_string())
        .expect("a reference");
    made["ref"].as_str().expect("its token").to_owned()
}

fn resolve(app: &HeadlessApp, level: Level, token: &str) -> Result<Value, recto_agent::AgentError> {
    app.agent(level, &json!({ "cmd": "resolve", "ref": token, "tab": "#1" }).to_string())
}

fn page_text(app: &HeadlessApp, page: u64) -> String {
    let said = app.recto(Level::ReadOnly, &["text", "--page", &page.to_string()]).expect("its text");
    said["untrusted"]["text"].as_str().unwrap_or_default().trim().to_owned()
}

/// Failure: the one thing a reference is for. Page 3 of five, found by a search, is given a reference; another program then puts a cover in front, so the page
/// is page 4 now, and a reader of "page 3" would read the wrong page. `resolve` says moved, with the page's new number, the one `text` reads the same
/// words on, and that it looked at every page to say so; and before the file changed it said found, reading no more than its hint's page and the
/// document's tag pages.
#[test]
fn search_ref_resolve_text_follows_a_page_another_program_moved() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let hits = app.recto(Level::ReadOnly, &["search", "Mine page 3"]).expect("search");
    assert_eq!(hits["hits"][0]["page"], 3, "the page is found: {hits}");
    let token = reference(&app, 3);
    let before = resolve(&app, Level::ReadOnly, &token).expect("resolve");
    assert_eq!(
        (before["status"].as_str(), before["page"].as_u64(), before["looked_at"].as_str()),
        (Some("found"), Some(3), Some("hint")),
        "{before}"
    );
    assert!(before["pages_read"].as_u64().is_some_and(|read| read <= 5), "{before}");
    replace(
        &file,
        &pages_saying(&["Cover", "Mine page 1", "Mine page 2", "Mine page 3", "Mine page 4", "Mine page 5"]),
    );
    look(&app);
    let after = resolve(&app, Level::ReadOnly, &token).expect("resolve");
    assert_eq!(after["status"], "moved", "{after}");
    assert_eq!(
        (after["page"].as_u64(), after["file_page"].as_u64(), after["was_page"].as_u64()),
        (Some(4), Some(4), Some(3)),
        "{after}"
    );
    assert_eq!(
        after["looked_at"], "all",
        "a page that moved is said to have moved only after every page was read: {after}"
    );
    assert_eq!(after["pages_read"], 6, "{after}");
    assert_eq!(after["target"]["kind"], "page");
    assert_eq!(
        page_text(&app, 4),
        "Mine page 3",
        "the page `resolve` names is the one the reference was made of"
    );
}

/// Failure: a page another program removed is answered with the nearest page, or with an error, where it is gone: exit 0, `gone`, every page read, no page
/// and no candidates. Also a page whose content is on two pages is never resolved to either.
#[test]
fn resolve_says_gone_after_another_program_removed_the_page() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let token = reference(&app, 3);
    replace(&file, &pages_saying(&["Mine page 1", "Mine page 2", "Mine page 4", "Mine page 5"]));
    look(&app);
    let gone = resolve(&app, Level::ReadOnly, &token).expect("resolve answers, it doesn't refuse");
    assert_eq!(gone["status"], "gone", "{gone}");
    assert_eq!(gone["looked_at"], "all");
    assert_eq!(gone["pages_read"], 4, "{gone}");
    assert!(
        gone["page"].is_null() && gone["file_page"].is_null() && gone["target"].is_null(),
        "nothing is offered in its place: {gone}"
    );
    assert_eq!(gone["candidates"], json!([]));
    replace(&file, &pages_saying(&["Mine page 1", "Mine page 3", "Mine page 4", "Mine page 3"]));
    look(&app);
    let twice = resolve(&app, Level::ReadOnly, &token).expect("resolve");
    assert_eq!(twice["status"], "ambiguous", "{twice}");
    assert!(twice["page"].is_null(), "neither of the two is chosen: {twice}");
    assert_eq!(twice["candidates"], json!([{ "page": 2, "file_page": 2 }, { "page": 4, "file_page": 4 }]));
}

/// Failure: with the person's unsaved edits on the tab, a reference counts the file's pages and not the tab's, or `resolve` gives the file's number where
/// the person's page is somewhere else, or invents a place for a page their edits removed. File page 2 is deleted by the owner: a reference to the tab's
/// page 2 is of the file's page 3; the page that was deleted, referenced before, is found in the file and null in the tab; a page the owner added from
/// another PDF has no reference until it's saved; and a page past the tab's end isn't there.
#[test]
fn resolve_gives_a_file_pages_place_in_the_unsaved_edits_and_null_once_deleted() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let second = reference(&app, 2);
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"2"}"##);
    app.settle();
    let now_second = app.agent(Level::ReadOnly, r##"{"cmd":"ref","page":2,"tab":"#1"}"##).expect("the tab's page 2");
    assert_eq!(
        (now_second["page"].as_u64(), now_second["file_page"].as_u64()),
        (Some(2), Some(3)),
        "{now_second}"
    );
    let there = resolve(&app, Level::ReadOnly, now_second["ref"].as_str().expect("token")).expect("resolve");
    assert_eq!(
        (there["status"].as_str(), there["page"].as_u64(), there["file_page"].as_u64()),
        (Some("found"), Some(2), Some(3)),
        "{there}"
    );
    let removed = resolve(&app, Level::ReadOnly, &second).expect("resolve");
    assert_eq!(removed["status"], "found", "it's still in the file: {removed}");
    assert_eq!(
        (removed["file_page"].as_u64(), removed["page"].is_null()),
        (Some(2), true),
        "and in no place in the tab: {removed}"
    );
    let past = app
        .agent(Level::ReadOnly, r##"{"cmd":"ref","page":5,"tab":"#1"}"##)
        .expect_err("the tab has four pages");
    assert_eq!(past.code.as_str(), "no_such_page", "{past:?}");
    let other = app.disk().file("other.pdf", &pdf_bytes("Other", 1));
    app.person(&json!({ "cmd": "page_insert", "tab": "#1", "path": other, "at": 1 }).to_string());
    app.settle();
    let added = app.agent(Level::ReadOnly, r##"{"cmd":"ref","page":1,"tab":"#1"}"##).expect_err("an added page");
    assert_eq!((added.code.as_str(), added.exit), ("not_referenceable", exit::FAILED), "{added:?}");
}

/// Failure: a tab whose edits were made on another version of the file gives a reference or an answer as though its page numbers still meant something,
/// where `text` refuses them. Both are `source_changed`, as `text` is.
#[test]
fn an_edited_tab_whose_file_changed_refuses_resolve_as_text_does() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    app.open(&file);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"9","degrees":90}"##);
    app.settle();
    let token = reference(&app, 3);
    replace(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    let as_text = app.recto(Level::Full, &["text", "--page", "3"]).expect_err("text refuses").code;
    assert_eq!(as_text.as_str(), "source_changed");
    let resolved = resolve(&app, Level::ReadOnly, &token).expect_err("resolve refuses");
    let referenced = app.agent(Level::ReadOnly, r##"{"cmd":"ref","page":3,"tab":"#1"}"##).expect_err("ref refuses");
    assert_eq!((resolved.code, referenced.code), (as_text, as_text), "{resolved:?} {referenced:?}");
}

/// Failure: another program replaces the file under a tab with unsaved edits, and an agent asks before the app has looked at the file (so nothing is
/// withheld yet). `ref` maps the tab's page through the old positions onto the new file and hands out a token for the wrong page, or `resolve` answers about
/// it. Both are `source_changed`, as `text` is, with no look in between.
#[test]
fn an_edited_tab_whose_file_changed_before_the_app_looked_refuses_ref_and_resolve_as_text_does() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    app.open(&file);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"9","degrees":90}"##);
    app.settle();
    let token = reference(&app, 3);
    replace(&file, &pdf_bytes("Theirs", 5));
    let as_text = app.recto(Level::Full, &["text", "--page", "3"]).expect_err("text refuses").code;
    assert_eq!(as_text.as_str(), "source_changed");
    let referenced = app.agent(Level::ReadOnly, r##"{"cmd":"ref","page":3,"tab":"#1"}"##).expect_err("ref refuses");
    let resolved = resolve(&app, Level::ReadOnly, &token).expect_err("resolve refuses");
    assert_eq!((referenced.code, resolved.code), (as_text, as_text), "{referenced:?} {resolved:?}");
}

/// Everything of the owner's tab an agent could change: what `edits` says, the session's list, and the file's bytes and time.
fn owners_tab(app: &HeadlessApp, file: &Path) -> (Value, Value, Vec<u8>, std::time::SystemTime) {
    let edits = app.recto(Level::Full, &["edits"]).expect("edits");
    let modified = std::fs::metadata(file).and_then(|meta| meta.modified()).expect("its time");
    (
        edits,
        app.session_json()["tabs"][0]["edits"].clone(),
        std::fs::read(file).expect("the file"),
        modified,
    )
}

/// Failure: reading or resolving a reference, with a good token or a hostile one, undoes, redoes, drops or adds an edit of the owner's, writes the file, or is
/// let through below its level. The owner turns page 9 and deletes page 2; an agent at Read-only and at Control references and resolves (a hostile token
/// too), and the edits, the undo history and the file are as they were. At Off both are refused for the level, and a token that can't be one is refused for
/// its form, before the level is asked.
#[test]
fn an_agent_below_full_changes_nothing_of_the_owners_edits_by_ref_or_resolve() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    app.open(&file);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"9","degrees":90}"##);
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"2"}"##);
    app.settle();
    let before = owners_tab(&app, &file);
    let token = reference(&app, 4);
    for level in [Level::ReadOnly, Level::Control] {
        let made = app.agent(level, r##"{"cmd":"ref","page":4,"tab":"#1"}"##).expect("ref");
        assert_eq!(made["ref"], token.as_str(), "the same page gives the same reference at every level");
        assert_eq!(resolve(&app, level, &token).expect("resolve")["status"], "found");
        let hostile = format!("r1.{}", "z".repeat(300));
        let refused = resolve(&app, level, &hostile).expect_err("a token that is no token");
        assert_eq!((refused.code.as_str(), refused.exit), ("bad_reference", exit::USAGE));
        assert!(!refused.message.contains("zzz"), "the hostile token isn't repeated: {}", refused.message);
    }
    assert_eq!(owners_tab(&app, &file), before, "nothing of the owner's tab or file changed");
    for line in [
        r##"{"cmd":"ref","page":4,"tab":"#1"}"##.to_owned(),
        json!({ "cmd": "resolve", "ref": token, "tab": "#1" }).to_string(),
    ] {
        let refused = app.agent(Level::Off, &line).expect_err("Off");
        assert_eq!(refused.exit, exit::PERMISSION, "{refused:?}");
    }
    let bad = app.agent(Level::Off, r#"{"cmd":"resolve","ref":"é"}"#).expect_err("a bad token at Off");
    assert_eq!(bad.code.as_str(), "bad_reference", "refused for its form, before the level: {bad:?}");
    assert_eq!(owners_tab(&app, &file), before, "and Off changed nothing either");
}

/// Failure: a document the person opened with its password gives an agent a reference or an answer, so its pages, its length or its identity reach an
/// agent that the SPEC keeps them from; or a token made from another file is treated differently from any other valid token, which would tell the agent
/// whether the protected pages match. Both refuse with the protected refusal `text` gives, whatever the token.
#[test]
fn a_protected_document_gives_an_agent_neither_a_reference_nor_an_answer() {
    let disk = Disk::new();
    let plain = disk.file("plain.pdf", &pages_saying(&["Locked page 1", "Locked page 2"]));
    let locked = disk.file("locked.pdf", &locked_saying(PASSWORD, 2, "Locked", lopdf::encryption::Permissions::all()));
    let app = disk.start();
    app.open(&plain);
    let twins = reference(&app, 2);
    person_unlocks(&app, &locked);
    let tab = app.session_json()["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with("locked.pdf"))))
        .map(|tab| format!("#{}", tab["id"]))
        .expect("the locked tab");
    let text = app
        .agent(Level::Full, &json!({ "cmd": "text", "page": 2, "tab": tab }).to_string())
        .expect_err("text is refused");
    assert_eq!(text.exit, exit::PROTECTED);
    let made = app
        .agent(Level::Full, &json!({ "cmd": "ref", "page": 2, "tab": tab }).to_string())
        .expect_err("ref is refused");
    for token in [twins.as_str(), "r1.040g2081040g2.p1-081040g2081040g2081040g208-0000000"] {
        let answered = app
            .agent(Level::Full, &json!({ "cmd": "resolve", "ref": token, "tab": tab }).to_string())
            .expect_err("resolve is refused");
        assert_eq!(
            (answered.code, answered.exit, &answered.message),
            (text.code, text.exit, &text.message),
            "any valid token, the same refusal"
        );
    }
    assert_eq!((made.code, made.exit, &made.message), (text.code, text.exit, &text.message));
}

/// Failure: `recto ref 3` and `recto resolve <ref>` as typed don't reach the commands the wire has (a word the CLI builds into a line the socket refuses),
/// or the token the first prints isn't one the second takes whole.
#[test]
fn recto_cli_words_reach_resolve() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 4));
    app.open(&file);
    let made = app.recto(Level::ReadOnly, &["ref", "3", "--tab", "#1"]).expect("recto ref 3");
    let token = made["ref"].as_str().expect("token").to_owned();
    let answer = app.recto(Level::ReadOnly, &["resolve", &token, "--tab", "#1"]).expect("recto resolve");
    assert_eq!((answer["status"].as_str(), answer["page"].as_u64()), (Some("found"), Some(3)), "{answer}");
    let gone = app.recto(Level::ReadOnly, &["ref", "9"]).expect_err("past the end");
    assert_eq!(gone.code.as_str(), "no_such_page");
    let words = recto_agent::human::render("resolve", &answer);
    assert!(
        words.contains("found") && !words.contains(&token),
        "plain words, and not the token back: {words}"
    );
}
