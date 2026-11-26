//! A search hit's `untrusted.link` through the app core, with no window: the file's own name and the FILE's page, with the person's unsaved edits, on a page
//! added from another PDF, and below Control. The one `recto_core::locator::page_link` makes it, as `recto link` does.

use super::changed_under_edits::pdf_bytes;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use serde_json::{Value, json};

fn search(app: &HeadlessApp, words: &[&str]) -> Value {
    let mut line = vec!["search"];
    line.extend_from_slice(words);
    app.recto(Level::ReadOnly, &line).expect("a search")
}

/// Failure: a hit's link names the page's place in the tab (where an unsaved move put it) and not its page of the file on disk, so the link opens another
/// page than the one found; or the raw name goes in, so a `#`, `:` or space in it adds a fragment or a scheme; or a page added from another PDF is given a
/// link to a page of the file that isn't it; or the parent folder is in an answer below Control. Page 1 is moved to 3, so the hit for "Mine page 1" is the
/// tab's page 3 and the file's page 1; a phrase, a pattern and a ranked search all say `#page=1`.
#[test]
fn a_hits_link_names_the_files_page_and_the_encoded_name_and_no_folder() {
    let disk = Disk::new();
    let file = disk.file("a#b: c.pdf", &pdf_bytes("Mine", 5));
    let folder = disk.files().to_string_lossy().into_owned();
    let app = disk.builder().indexing(true).start();
    app.open(&file);
    app.recto(Level::Full, &["index"]).expect("asked for");
    app.wait_until("the index is built", || {
        app.recto(Level::Full, &["index", "status"]).is_ok_and(|status| status["state"] == "ready")
    });
    app.person(r##"{"cmd":"page_move","tab":"#1","pages":"1","to":3}"##);
    app.settle();
    let expected = "a%23b%3A%20c.pdf#page=1";
    // The first is read from the index with the file never opened for it; the others, and the one after, from the file or the same index: all say the same.
    let indexed = search(&app, &["Mine page 1"]);
    assert_eq!(indexed["route"], "index", "{indexed}");
    for words in [
        &["Mine page 1"][..],
        &["Mine page 1", "--regex"],
        &["Mine page 1", "--rank"],
        &["Mine page 1", "--scan"],
    ] {
        let said = search(&app, words);
        let hit = &said["hits"][0];
        assert_eq!(hit["page"], 3, "the hit is at the tab's page 3: {said}");
        assert_eq!(hit["untrusted"]["link"], expected, "{words:?}: {said}");
        assert!(!said.to_string().contains(&folder), "no folder in {said}");
    }
    let other = app.disk().file("other.pdf", &pdf_bytes("Other", 1));
    app.person(&json!({ "cmd": "page_insert", "tab": "#1", "path": other, "at": 1 }).to_string());
    app.settle();
    let said = search(&app, &["Other page 1"]);
    let hit = &said["hits"][0];
    assert_eq!(hit["from_added_file"], true, "{said}");
    assert!(hit["untrusted"]["link"].is_null(), "an added page has no link: {said}");
    let own = search(&app, &["Mine page 1"]);
    assert_eq!(own["hits"][0]["untrusted"]["link"], expected, "{own}");
    assert!(!own.to_string().contains(&folder), "no folder in {own}");
}
