//! File > Open Recent: its list, its items, and Clear Menu. Twin of `recto-cli`'s app test of the same name.

use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

/// File's Open Recent items, as the menu bar lists them: each item's id and its title, newest first.
fn recent_items(app: &HeadlessApp) -> Vec<(String, String)> {
    let bar = app.core().bar_now();
    let file = bar.iter().find(|menu| menu.title == "File").expect("a File menu");
    file.entries
        .iter()
        .filter(|entry| entry.id.starts_with("recent.") && entry.id != "recent.clear")
        .map(|entry| (entry.id.clone(), entry.title.clone()))
        .collect()
}

/// Failure (review O8): File > Open Recent misses files opened, lists them out of order, opens another file than the item showed once the list has moved, doesn't
/// open one, or Clear Menu leaves any. (The app test `open_recent_lists_opens_and_clears`, which this replaces.)
#[test]
fn open_recent_lists_opens_and_clears() {
    let app = HeadlessApp::start();
    let a = app.disk().pdf("alpha.pdf", 3);
    let b = app.disk().pdf("beta.pdf", 2);
    app.open(&a);
    app.open(&b);
    let bar = app.core().bar_now();
    let ids: Vec<&str> = bar
        .iter()
        .find(|menu| menu.title == "File")
        .unwrap()
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        // Separators, which the window doesn't list as items.
        .filter(|id| !id.is_empty())
        .collect();
    let at = ids.iter().position(|id| *id == "file.open_recent").expect("Open Recent in File");
    assert!(
        ids[at + 1].starts_with("recent.") && ids[at + 2].starts_with("recent.") && ids[at + 3] == "recent.clear",
        "{ids:?}"
    );
    let items = recent_items(&app);
    assert_eq!(
        items.iter().map(|(_, title)| title.as_str()).collect::<Vec<_>>(),
        ["beta", "alpha"],
        "newest first: {items:?}"
    );
    let listed = app.agent(Level::Control, r#"{"cmd":"recent"}"#).expect("recent");
    assert_eq!(listed["count"], 2);
    assert_eq!(listed["files"][0]["untrusted"]["path"], b.to_str().unwrap(), "newest first: {listed}");
    // The item for the older file, kept from before the list moves: it opens that file, not whatever's at its old place in the list.
    let older = items[1].0.clone();
    app.agent(Level::Control, r##"{"cmd":"close","tab":"#1"}"##).expect("close the first");
    app.settle();
    app.open(&a);
    app.agent(Level::Control, r#"{"cmd":"close"}"#).expect("close the one showing");
    app.settle();
    assert_eq!(recent_items(&app)[0].1, "alpha", "the list moved, and only the other file is open");
    assert_eq!(app.tabs().len(), 1);
    app.core().perform(older);
    app.settle();
    let tabs: Vec<String> = app.tabs().into_iter().map(|row| row.text).collect();
    assert_eq!(tabs.len(), 2, "the item opened its file: {tabs:?}");
    assert!(tabs.iter().any(|tab| tab == "alpha"), "{tabs:?}");
    app.core().perform("recent.clear".to_string());
    app.settle();
    assert!(recent_items(&app).is_empty(), "Clear Menu leaves none");
    assert!(app.core().recent_files().is_empty());
}
