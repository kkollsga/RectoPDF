//! `ref` and `resolve` through the app core, with no window: a reference to a page, and where that page is after another program changed the file, with the
//! person's unsaved edits, and with a protected document. Both commands are one executor (`recto_agent::locating`); the window has no control for them, so
//! everything here is what an agent sees and what it may not change.

use super::changed_under_edits::{look, pdf_bytes, replace};
use super::files::locked_saying;
use super::withheld_events::{PASSWORD, person_unlocks};
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{Level, exit};
use recto_control::host::Host;
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

/// Failure: `ref 0` opens and states the tab's file before it says there is no page 0, so a page that can't exist is answered with the file's state (here
/// `source_changed`) and costs a read of the file. A page that can't be is refused first.
#[test]
fn ref_page_zero_is_refused_before_the_file_is_looked_at() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 4));
    app.open(&file);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"2","degrees":90}"##);
    app.settle();
    replace(&file, &pdf_bytes("Theirs", 2));
    let zero = app.agent(Level::ReadOnly, r##"{"cmd":"ref","page":0,"tab":"#1"}"##).expect_err("no page 0");
    assert_eq!(zero.code.as_str(), "no_such_page", "{zero:?}");
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
    // Reading by reference: `text` reads at Read-only and says where the page was found; `render` writes a file, so it is refused below Full, and
    // neither changes the owner's edits.
    let out = file.with_file_name("by-ref.png");
    for level in [Level::ReadOnly, Level::Control] {
        let read = app.recto(level, &["text", "--ref", &token]).expect("text --ref");
        assert_eq!((read["page"].as_u64(), read["ref"]["status"].as_str()), (Some(4), Some("found")), "{read}");
        let drawn = app
            .agent(level, &json!({ "cmd": "render", "ref": token, "tab": "#1", "out": out }).to_string())
            .expect_err("render is Full");
        assert_eq!(drawn.exit, exit::PERMISSION, "{drawn:?}");
        for line in [
            json!({ "cmd": "text", "ref": format!("r1.{}", "z".repeat(300)) }),
            json!({ "cmd": "render", "ref": format!("r1.{}", "z".repeat(300)), "out": out }),
        ] {
            let hostile = app.agent(level, &line.to_string()).expect_err("a token that is no token");
            assert_eq!((hostile.code.as_str(), hostile.exit), ("bad_reference", exit::USAGE), "{}", line["cmd"]);
        }
    }
    assert!(!out.exists(), "no file was written below Full");
    assert_eq!(
        owners_tab(&app, &file),
        before,
        "reading by reference changed nothing of the owner's tab or file"
    );
    for line in [
        r##"{"cmd":"ref","page":4,"tab":"#1"}"##.to_owned(),
        json!({ "cmd": "resolve", "ref": token, "tab": "#1" }).to_string(),
        json!({ "cmd": "text", "ref": token, "tab": "#1" }).to_string(),
        json!({ "cmd": "render", "ref": token, "tab": "#1", "out": out }).to_string(),
    ] {
        let refused = app.agent(Level::Off, &line).expect_err("Off");
        assert_eq!(refused.exit, exit::PERMISSION, "{refused:?}");
    }
    for line in [
        r#"{"cmd":"resolve","ref":"é"}"#,
        r#"{"cmd":"text","ref":"é"}"#,
        r#"{"cmd":"render","ref":"é","out":"/d/x.png"}"#,
    ] {
        let bad = app.agent(Level::Off, line).expect_err("a bad token at Off");
        assert_eq!(bad.code.as_str(), "bad_reference", "refused for its form, before the level: {bad:?}");
    }
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
    for token in [twins.as_str(), recto_agent::commands::locating::EXAMPLE_REF] {
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
    // Reading by reference is refused as reading by page is, whatever the token, for `text` and for `render` (a new file written is a read too).
    let out = app.disk().files().join("protected.png");
    for token in [twins.as_str(), recto_agent::commands::locating::EXAMPLE_REF] {
        for line in [
            json!({ "cmd": "text", "ref": token, "tab": tab }),
            json!({ "cmd": "render", "ref": token, "tab": tab, "out": out }),
        ] {
            let answered = app.agent(Level::Full, &line.to_string()).expect_err("refused");
            assert_eq!(
                (answered.code, answered.exit, &answered.message),
                (text.code, text.exit, &text.message),
                "{line}"
            );
        }
    }
    assert!(!out.exists(), "and nothing was written");
}

/// What a command left of draws: whether it told the host a page was drawn or failed to draw.
fn drew(events: &[recto_agent::Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, recto_agent::Event::PageDrawn { .. } | recto_agent::Event::PageFailed { .. }))
}

/// `line` as an agent at `level` sends it, with what it told the host.
fn run_saying(app: &HeadlessApp, level: Level, line: &Value) -> (Result<Value, recto_agent::AgentError>, Vec<recto_agent::Event>) {
    let command = recto_agent::parse(&line.to_string()).expect("a command");
    let mut events = Vec::new();
    let answer = app.core().host.run(recto_agent::Actor::Agent(level), &command, &mut events);
    (answer, events)
}

/// Failure: the reason `--ref` exists. A page found by a search and given a reference is read at its new number after another program put a cover in front:
/// `text --ref` says "Mine page 3" where `text --page 3` would say the cover's neighbour, and its answer says the page moved, from which page, and that
/// the document is said, with no token in it.
#[test]
fn search_ref_then_text_ref_reads_the_page_another_program_moved() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let hits = app.recto(Level::ReadOnly, &["search", "Mine page 3"]).expect("search");
    assert_eq!(hits["hits"][0]["page"], 3, "{hits}");
    let token = reference(&app, 3);
    replace(
        &file,
        &pages_saying(&["Cover", "Mine page 1", "Mine page 2", "Mine page 3", "Mine page 4", "Mine page 5"]),
    );
    look(&app);
    assert_eq!(page_text(&app, 3), "Mine page 2", "the page number alone reads the wrong page now");
    let read = app.recto(Level::ReadOnly, &["text", "--ref", &token]).expect("text --ref");
    assert_eq!(read["untrusted"]["text"].as_str().map(str::trim), Some("Mine page 3"), "{read}");
    assert_eq!(read["page"], 4, "{read}");
    // The cover makes it another document by its tag; that is said and doesn't stop the read.
    assert_eq!(
        (
            read["ref"]["status"].as_str(),
            read["ref"]["was_page"].as_u64(),
            read["ref"]["file_page"].as_u64()
        ),
        (Some("moved"), Some(3), Some(4)),
        "{read}"
    );
    assert!(read["ref"]["document"].is_string(), "{read}");
    assert!(!read.to_string().contains(&token), "the token isn't in the answer");
}

/// Failure: a reference to a page that is gone, or that two pages have, reads a page anyway (the nearest, or the first), or says "not found" in the words
/// of a missing page, or starts a draw for a page it didn't choose. Each is `reference_unresolved`, exit not-found, with its own words, and tells the host
/// of no page drawn or failed.
#[test]
fn text_ref_to_a_gone_or_ambiguous_page_is_refused_and_draws_nothing() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let token = reference(&app, 3);
    let ask = json!({ "cmd": "text", "ref": token, "tab": "#1" });
    replace(&file, &pages_saying(&["Mine page 1", "Mine page 2", "Mine page 4", "Mine page 5"]));
    look(&app);
    let (gone, events) = run_saying(&app, Level::ReadOnly, &ask);
    let gone = gone.expect_err("gone");
    assert_eq!((gone.code.as_str(), gone.exit), ("reference_unresolved", exit::NOT_FOUND), "{gone:?}");
    assert!(!drew(&events), "{events:?}");
    replace(&file, &pages_saying(&["Mine page 1", "Mine page 3", "Mine page 4", "Mine page 3"]));
    look(&app);
    let (twice, events) = run_saying(&app, Level::ReadOnly, &ask);
    let twice = twice.expect_err("ambiguous");
    assert_eq!((twice.code.as_str(), twice.exit), ("reference_unresolved", exit::NOT_FOUND), "{twice:?}");
    assert_ne!(twice.message, gone.message, "each case says its own thing");
    assert!(!drew(&events), "{events:?}");
    assert!(!twice.message.contains(&token) && !gone.message.contains(&token));
}

/// Failure: `text --ref` counts the file's pages where the person's unsaved edits have moved them (reads the page the number says, not the one the
/// reference names), or reads a page the person deleted, or says "gone" of it. The owner deletes page 2: the reference to page 3 reads "Mine page 3" at
/// the tab's page 2, with the file's page 3 said; the reference to the deleted page is refused for the edit, not as gone, and draws nothing.
#[test]
fn text_ref_follows_unsaved_edits_and_refuses_a_page_they_removed() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let (second, third) = (reference(&app, 2), reference(&app, 3));
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"2"}"##);
    app.settle();
    let read = app.recto(Level::ReadOnly, &["text", "--ref", &third]).expect("text --ref");
    assert_eq!(read["untrusted"]["text"].as_str().map(str::trim), Some("Mine page 3"), "{read}");
    assert_eq!((read["page"].as_u64(), read["ref"]["file_page"].as_u64()), (Some(2), Some(3)), "{read}");
    let (removed, events) = run_saying(&app, Level::ReadOnly, &json!({ "cmd": "text", "ref": second, "tab": "#1" }));
    let removed = removed.expect_err("removed by the edits");
    assert_eq!((removed.code.as_str(), removed.exit), ("reference_unresolved", exit::NOT_FOUND), "{removed:?}");
    assert!(removed.message.contains("Unsaved edits"), "it says why: {}", removed.message);
    assert!(!drew(&events), "{events:?}");
}

/// Failure: `render --ref` writes a file below Full (the reference is a way round `render`'s level), or at Full draws the page the number would. Below
/// Full it's refused for the level and nothing is written; at Full the file is written and the answer says how the page was found.
#[test]
fn render_ref_below_full_is_refused_as_render_is_and_writes_no_file() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 3));
    app.open(&file);
    let token = reference(&app, 2);
    let out = file.with_file_name("by-ref.png");
    let ask = json!({ "cmd": "render", "ref": token, "tab": "#1", "out": out });
    for level in [Level::Off, Level::ReadOnly, Level::Control] {
        let refused = app.agent(level, &ask.to_string()).expect_err("below Full");
        assert_eq!(refused.exit, exit::PERMISSION, "{level:?} {refused:?}");
        assert!(!out.exists(), "{level:?} wrote a file");
    }
    let drawn = app.agent(Level::Full, &ask.to_string()).expect("at Full");
    assert!(out.exists(), "{drawn}");
    assert_eq!((drawn["page"].as_u64(), drawn["ref"]["status"].as_str()), (Some(2), Some("found")), "{drawn}");
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

/// A reference to characters `[start, end)` of tab `#1`'s page, as an agent at Read-only is given it.
fn chars_reference(app: &HeadlessApp, page: u32, start: u32, end: u32) -> Result<String, recto_agent::AgentError> {
    app.agent(
        Level::ReadOnly,
        &json!({ "cmd": "ref", "page": page, "chars": format!("{start}-{end}"), "tab": "#1" }).to_string(),
    )
    .map(|made| made["ref"].as_str().expect("its token").to_owned())
}

/// The same reference with its text check, or its end, set by the core API as a hand-made token would: a page that is found, and characters that are not
/// the ones the reference says.
fn hostile_chars(token: &str, start: u32, end: u32) -> String {
    use recto_core::locator::{CharacterRange, Locator, Target, decode, encode};
    let made = decode(token).expect("a token");
    let range = CharacterRange::new(start, end, [0xAB; 8]).expect("a range");
    encode(&Locator::new(made.document(), made.page(), Target::Characters(range)))
}

/// Failure: a reference to words of a page, after another program put a cover in front, is found by the page's content but its offsets are taken as
/// vouched for without reading the text (`checked: false`), or the words at its offsets are not the ones it was made of, or `text --ref` reads a page where
/// the characters are not those. The words are `page` of "Mine page 3", the page is `moved` to 4 and says `text: same`, and the offsets read the same words
/// in what `text --ref` gives.
#[test]
fn a_character_reference_reads_the_same_words_after_another_program_moved_the_page() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let whole = app.recto(Level::ReadOnly, &["text", "--page", "3"]).expect("text")["untrusted"]["text"]
        .as_str()
        .expect("its text")
        .to_owned();
    let start = whole.chars().position(|c| c == 'p').expect("the word") as u32;
    let token = chars_reference(&app, 3, start, start + 4).expect("a character reference");
    let before = resolve(&app, Level::ReadOnly, &token).expect("resolve");
    assert_eq!(before["status"], "found", "{before}");
    assert_eq!(
        before["target"],
        json!({ "kind": "characters", "start": start, "end": start + 4, "checked": true, "text": "same" })
    );
    replace(
        &file,
        &pages_saying(&["Cover", "Mine page 1", "Mine page 2", "Mine page 3", "Mine page 4", "Mine page 5"]),
    );
    look(&app);
    let after = resolve(&app, Level::ReadOnly, &token).expect("resolve");
    assert_eq!((after["status"].as_str(), after["page"].as_u64()), (Some("moved"), Some(4)), "{after}");
    assert_eq!(after["target"]["text"], "same", "{after}");
    let read = app.recto(Level::ReadOnly, &["text", "--ref", &token]).expect("text --ref");
    let words: String = read["untrusted"]["text"].as_str().expect("text").chars().skip(start as usize).take(4).collect();
    assert_eq!((words.as_str(), read["page"].as_u64()), ("page", Some(4)), "{read}");
}

/// Failure: a hand-made character token (a check that isn't the text's, or an end at 2^21) on a page that is found is answered `found`, so an agent reads
/// words at offsets that are not the ones named; or `text --ref` goes on to read the page, or draws, or its refusal
/// shares the words of `gone`. The answer is `text_changed`, exit 0, with the page, and `text --ref` is `reference_unresolved` in words of its own.
#[test]
fn a_hostile_character_token_on_a_found_page_is_text_changed_never_found() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let good = chars_reference(&app, 3, 0, 4).expect("a character reference");
    let page = reference(&app, 3);
    for token in [
        hostile_chars(&good, 0, 4),
        hostile_chars(&good, 0, 1 << 21),
        hostile_chars(&good, 4_000_000 >> 2, 1 << 21),
    ] {
        let said = resolve(&app, Level::ReadOnly, &token).expect("an answer, not a refusal");
        assert_eq!(
            (said["status"].as_str(), said["page"].as_u64(), said["file_page"].as_u64()),
            (Some("text_changed"), Some(3), Some(3)),
            "{said}"
        );
        assert_eq!(said["target"]["text"], "changed", "{said}");
        let (read, events) = run_saying(&app, Level::ReadOnly, &json!({ "cmd": "text", "ref": token, "tab": "#1" }));
        let read = read.expect_err("not read");
        assert_eq!((read.code.as_str(), read.exit), ("reference_unresolved", exit::NOT_FOUND), "{read:?}");
        assert!(!drew(&events), "{events:?}");
        assert!(!read.message.contains(&token));
    }
    replace(&file, &pages_saying(&["Mine page 1", "Mine page 2", "Mine page 4", "Mine page 5"]));
    look(&app);
    let gone = app.recto(Level::ReadOnly, &["text", "--ref", &page]).expect_err("gone");
    let changed = app
        .recto(Level::ReadOnly, &["text", "--ref", &hostile_chars(&good, 0, 4)])
        .expect_err("gone too");
    assert_eq!(changed.code.as_str(), "reference_unresolved");
    let again = app.disk().file("again.pdf", &pdf_bytes("Mine", 5));
    app.open(&again);
    let found_hostile = app
        .recto(Level::ReadOnly, &["resolve", &hostile_chars(&good, 0, 4), "--tab", "#2"])
        .expect("resolve");
    assert_eq!(found_hostile["status"], "text_changed", "{found_hostile}");
    let text_changed = app
        .recto(Level::ReadOnly, &["text", "--ref", &hostile_chars(&good, 0, 4), "--tab", "#2"])
        .expect_err("text changed");
    assert_ne!(text_changed.message, gone.message, "each case says its own thing");
}

/// Failure: a range past the end of the page's text is made into a reference (naming characters that are not there), or the last character is refused. The
/// page's text has `limit` characters: `0-limit` is a reference, and `0-limit+1` is `not_referenceable`.
#[test]
fn ref_chars_past_the_pages_text_is_not_referenceable() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 3));
    app.open(&file);
    let limit = app.recto(Level::ReadOnly, &["text", "--page", "2"]).expect("text")["untrusted"]["text"]
        .as_str()
        .expect("its text")
        .chars()
        .count() as u32;
    let made = chars_reference(&app, 2, 0, limit).expect("up to the end of the text");
    assert_eq!(resolve(&app, Level::ReadOnly, &made).expect("resolve")["status"], "found");
    let past = chars_reference(&app, 2, 0, limit + 1).expect_err("one past the text");
    assert_eq!((past.code.as_str(), past.exit), ("not_referenceable", exit::FAILED), "{past:?}");
    let empty = chars_reference(&app, 2, 3, 3).expect_err("an empty range");
    assert_eq!((empty.code.as_str(), empty.exit), ("usage", exit::USAGE), "{empty:?}");
}

/// Failure: a character reference, or checking one, is a way for an agent below Full to change the owner's edits, their undo history or the file, or is
/// let through at Off. At Read-only and Control, `ref --chars`, `resolve` of a good and of a hostile token and `text --ref` leave the tab's edits, the
/// session's list, the file's bytes and time as they were; at Off each is refused for the level.
#[test]
fn an_agent_below_full_changes_nothing_of_the_owners_edits_by_a_character_reference() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    app.open(&file);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"9","degrees":90}"##);
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"2"}"##);
    app.settle();
    let before = owners_tab(&app, &file);
    let good = chars_reference(&app, 4, 0, 4).expect("a character reference");
    let hostile = hostile_chars(&good, 0, 4);
    for level in [Level::ReadOnly, Level::Control] {
        let made = app.agent(level, r##"{"cmd":"ref","page":4,"chars":"0-4","tab":"#1"}"##).expect("ref --chars");
        assert_eq!(made["ref"], good.as_str(), "the same words give the same reference at every level");
        assert_eq!(resolve(&app, level, &good).expect("resolve")["status"], "found");
        assert_eq!(resolve(&app, level, &hostile).expect("resolve")["status"], "text_changed");
        let read = app.recto(level, &["text", "--ref", &good]).expect("text --ref");
        assert_eq!(read["ref"]["text"], "same", "{read}");
        app.recto(level, &["text", "--ref", &hostile]).expect_err("changed text is not read");
    }
    assert_eq!(owners_tab(&app, &file), before, "nothing of the owner's tab or file changed");
    for line in [
        r##"{"cmd":"ref","page":4,"chars":"0-4","tab":"#1"}"##.to_owned(),
        json!({ "cmd": "resolve", "ref": good, "tab": "#1" }).to_string(),
        json!({ "cmd": "text", "ref": hostile, "tab": "#1" }).to_string(),
    ] {
        let refused = app.agent(Level::Off, &line).expect_err("Off");
        assert_eq!(refused.exit, exit::PERMISSION, "{refused:?}");
    }
    assert_eq!(owners_tab(&app, &file), before, "and Off changed nothing either");
}

/// Failure: a document the person opened with its password gives an agent a character reference, or a check of one, so the text of its pages (or whether
/// words are on them) reaches an agent that the SPEC keeps them from. `ref --chars` and `resolve` of a character token give the refusal `text` gives, with
/// nothing drawn.
#[test]
fn a_protected_document_gives_no_character_reference_and_no_check() {
    let disk = Disk::new();
    let plain = disk.file("plain.pdf", &pages_saying(&["Locked page 1", "Locked page 2"]));
    let locked = disk.file("locked.pdf", &locked_saying(PASSWORD, 2, "Locked", lopdf::encryption::Permissions::all()));
    let app = disk.start();
    app.open(&plain);
    let twin = chars_reference(&app, 2, 0, 4).expect("on the plain file");
    person_unlocks(&app, &locked);
    let tab = app.session_json()["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with("locked.pdf"))))
        .map(|tab| format!("#{}", tab["id"]))
        .expect("the locked tab");
    let (text, _) = run_saying(&app, Level::Full, &json!({ "cmd": "text", "page": 2, "tab": tab }));
    let text = text.expect_err("text is refused");
    for line in [
        json!({ "cmd": "ref", "page": 2, "chars": "0-4", "tab": tab }),
        json!({ "cmd": "resolve", "ref": twin, "tab": tab }),
        json!({ "cmd": "resolve", "ref": hostile_chars(&twin, 0, 4), "tab": tab }),
    ] {
        let (answered, events) = run_saying(&app, Level::Full, &line);
        let answered = answered.expect_err("refused");
        assert_eq!(
            (answered.code, answered.exit, &answered.message),
            (text.code, text.exit, &text.message),
            "{line}"
        );
        assert!(!drew(&events), "{events:?}");
    }
}

/// Failure: a character target on a found page whose text can't be read is answered as a status (`found`, or `text_changed`), so an agent is told something
/// about words nobody could read; or `resolve` and `recto text` refuse in different words for the same page. By design (D3) it is `recto text`'s own
/// refusal, with its code and exit, and not a status: the page can't be read, which isn't a page whose text changed.
#[test]
fn a_character_target_on_a_page_whose_text_cannot_be_read_is_texts_own_refusal_not_a_status() {
    let app = HeadlessApp::start();
    let file = app.disk().file("bomb.pdf", &recto_testpdf::form_bomb());
    app.open(&file);
    let page = reference(&app, 1);
    let token = hostile_chars(&page, 0, 1);
    let read = app.recto(Level::ReadOnly, &["text", "--page", "1"]).expect_err("the bomb's text can't be read");
    let said = resolve(&app, Level::ReadOnly, &token).expect_err("a refusal, not a status");
    assert_eq!(
        (said.code, said.exit, said.message.as_str()),
        (read.code, read.exit, read.message.as_str()),
        "{said:?} against {read:?}"
    );
    let by_ref = app.recto(Level::ReadOnly, &["text", "--ref", &token]).expect_err("refused");
    assert_eq!((by_ref.code, by_ref.exit), (read.code, read.exit), "{by_ref:?}");
    let made = app
        .agent(Level::ReadOnly, &json!({ "cmd": "ref", "page": 1, "chars": "0-1", "tab": "#1" }).to_string())
        .expect_err("no characters to point at");
    assert_eq!((made.code, made.exit), (read.code, read.exit), "{made:?}");
}
