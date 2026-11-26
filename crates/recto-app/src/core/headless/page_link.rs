//! `link` through the app core, with no window: the standard link to a page by the file's own name, with the person's unsaved edits, with a protected
//! document, and at the lowest level. The window's Copy Link sends this same command later; everything here is what an agent sees.

use super::changed_under_edits::{look, pdf_bytes, replace};
use super::files::locked_saying;
use super::withheld_events::{PASSWORD, person_unlocks};
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{Level, exit};
use serde_json::json;

/// Failure: with the person's unsaved moves on the tab, the link names the page the agent asked for (the tab's number) and not that page's place in the
/// file, so the link opens another page than the one meant. Page 1 is moved to 3, so the tab's page 3 is the file's page 1: `#page=1`, with `file_page` saying
/// so and a note that the edits aren't in the file. With no edits the two pages are the same and there is no note. The name is the file's own, encoded.
#[test]
fn the_link_names_the_files_page_after_unsaved_moves() {
    let app = HeadlessApp::start();
    let file = app.disk().file("my doc.pdf", &pdf_bytes("Mine", 5));
    app.open(&file);
    let plain = app.recto(Level::ReadOnly, &["link", "3"]).expect("a link");
    assert_eq!(plain["untrusted"]["link"], "my%20doc.pdf#page=3", "{plain}");
    assert!(plain["note"].is_null(), "{plain}");
    app.person(r##"{"cmd":"page_move","tab":"#1","pages":"1","to":3}"##);
    app.settle();
    let moved = app.recto(Level::ReadOnly, &["link", "3"]).expect("a link");
    assert_eq!(
        (moved["page"].as_u64(), moved["file_page"].as_u64(), moved["untrusted"]["link"].as_str()),
        (Some(3), Some(1), Some("my%20doc.pdf#page=1")),
        "{moved}"
    );
    assert!(moved["note"].is_string(), "the edits not being in the file is said: {moved}");
}

/// Failure: an answer at Read gives a path (the person's folders are Control's to see), or a protected document or a tab whose pages are withheld is given a
/// link, which tells an agent the tab's length or that its file changed under the edits. The whole answer lacks the parent folder; a protected tab is
/// refused as `text` is; a tab whose file changed under unsaved edits is `source_changed`, as `text` is.
#[test]
fn below_control_no_answer_holds_a_path() {
    let disk = Disk::new();
    let plain = disk.file("plain.pdf", &pdf_bytes("Mine", 4));
    let locked = disk.file("locked.pdf", &locked_saying(PASSWORD, 2, "Locked", lopdf::encryption::Permissions::all()));
    let folder = disk.files().to_string_lossy().into_owned();
    let app = disk.start();
    app.open(&plain);
    let said = app.recto(Level::ReadOnly, &["link", "2"]).expect("a link");
    assert!(!said.to_string().contains(&folder), "no folder in {said}");
    assert_eq!(said["untrusted"]["link"], "plain.pdf#page=2");
    // Past the end of a tab with no edits is no page, and the refusal names no folder either.
    let past = app.recto(Level::ReadOnly, &["link", "5"]).expect_err("no such page");
    assert_eq!(past.code.as_str(), "no_such_page", "{past:?}");
    assert!(!format!("{past:?}").contains(&folder), "no folder in {past:?}");
    person_unlocks(&app, &locked);
    let tab = app.session_json()["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with("locked.pdf"))))
        .map(|tab| format!("#{}", tab["id"]))
        .expect("the locked tab");
    let text = app
        .agent(Level::Full, &json!({ "cmd": "text", "page": 2, "tab": tab }).to_string())
        .expect_err("text is refused");
    let link = app
        .agent(Level::Full, &json!({ "cmd": "link", "page": 2, "tab": tab }).to_string())
        .expect_err("link is refused");
    assert_eq!((link.code, link.exit, &link.message), (text.code, text.exit, &text.message));
    assert_eq!(link.exit, exit::PROTECTED);
    // The file changes under unsaved edits: its page numbers mean nothing, as for `text`.
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"2","degrees":90}"##);
    app.settle();
    replace(&plain, &pdf_bytes("Theirs", 2));
    look(&app);
    let refused = app.agent(Level::ReadOnly, r##"{"cmd":"link","page":2,"tab":"#1"}"##).expect_err("withheld");
    assert_eq!(refused.code.as_str(), "source_changed", "{refused:?}");
}

/// Failure: a page the person added from another PDF gets a link to a page of the file that isn't that page, or a page past the end is answered as an
/// added one. The added page is `not_referenceable` with no `link`; past the end is `no_such_page`.
#[test]
fn an_added_page_gets_no_link() {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 3));
    app.open(&file);
    let other = app.disk().file("other.pdf", &pdf_bytes("Other", 1));
    app.person(&json!({ "cmd": "page_insert", "tab": "#1", "path": other, "at": 1 }).to_string());
    app.settle();
    let added = app.recto(Level::ReadOnly, &["link", "1"]).expect_err("an added page");
    assert_eq!((added.code.as_str(), added.exit), ("not_referenceable", exit::FAILED), "{added:?}");
    assert!(!added.message.contains("#page"), "{added:?}");
    let own = app.recto(Level::ReadOnly, &["link", "2"]).expect("a page of the file");
    assert_eq!(own["untrusted"]["link"], "doc.pdf#page=1", "{own}");
    let past = app.recto(Level::ReadOnly, &["link", "5"]).expect_err("no such page");
    assert_eq!(past.code.as_str(), "no_such_page", "{past:?}");
}
