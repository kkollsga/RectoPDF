//! **A comparison with a document the person opened with its password, as an agent sees it** (security review of S1, its S2): `compare`'s note and
//! where synced scroll started, and a plain pane that follows the protected one, give away the protected document's length and place.

use super::withheld_events::{person_unlocks, protected};
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use serde_json::Value;

/// The person has a 30-page plain document beside a protected one of `pages` pages, synced, the protected one read, both laid out in their panes.
/// Gives the app and the two tabs: (plain, protected).
fn synced_beside_protected(pages: usize) -> (HeadlessApp, u32, u32) {
    let disk = Disk::new();
    let plain = disk.pdf("plain.pdf", 30);
    let locked = protected(&disk, pages, "Locked");
    let app = disk.start();
    app.open(&plain);
    person_unlocks(&app, &locked);
    let (plain_tab, locked_tab) = (app.tabs()[0].tab, app.tabs()[1].tab);
    let row = app.tabs().into_iter().find(|row| row.tab == plain_tab).expect("its row");
    app.act(UiAction::Row { row, adding: true });
    app.settle();
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.act(UiAction::PaneRead { tab: locked_tab });
    app.settle();
    (app, plain_tab, locked_tab)
}

/// The person scrolls the protected pane down by `documents` of its whole height from where it is, as their hand does; the plain pane follows.
/// Settled.
fn person_scrolls_protected(app: &HeadlessApp, locked_tab: u32, documents: f64) {
    let inner = std::sync::Arc::clone(app.core());
    let pane = (0..inner.panes.count())
        .find(|pane| inner.panes.tab_of(*pane).map(|id| id.0) == Some(locked_tab))
        .expect("in a pane");
    let engine = inner.panes.engine(&inner.engine, pane).expect("its page view");
    let scroll = |y: f64, magnification: f64| recto_reader::Viewport {
        y,
        magnification,
        backing: 2.0,
        scrolled_to: u64::MAX - 1,
        ..recto_reader::Viewport::default()
    };
    let place = |engine: &recto_reader::Engine| engine.places().1.into_iter().find(|(tab, _)| *tab == locked_tab).expect("its place").1;
    let at = place(&engine);
    drop(inner.panes.viewport(&inner.engine, pane, false, scroll(at.y, at.magnification)));
    let at = place(&engine);
    drop(
        inner
            .panes
            .viewport(&inner.engine, pane, true, scroll(at.y + documents * at.document_height, at.magnification)),
    );
    app.settle();
}

/// What an agent at Read-only reads of the comparison and of the plain tab.
fn agent_reads(app: &HeadlessApp, plain_tab: u32) -> (Value, Value) {
    let compared = app.recto(Level::ReadOnly, &["compare"]).expect("compare");
    let plain = app.recto(Level::ReadOnly, &["where", "--tab", &format!("#{plain_tab}")]).expect("where");
    (compared, plain)
}

/// Failure (security review of phase R, S2): beside a document the person opened with its password, `compare` gives an agent at Read-only its length
/// in the note ("Different lengths: N and 30 pages") and the page synced scroll started it from (`sync_from`), and the plain pane that follows it moves
/// with it and stops where it ends, so `where` on the plain tab, and a watcher's `page_changed` for it, say the protected one's place and length. What
/// an agent reads and hears is the same whatever the protected document's length, before and after the person scrolls it to its end.
#[test]
fn a_comparison_tells_an_agent_nothing_of_a_protected_pane() {
    let told = |pages: usize| {
        let (app, plain_tab, locked_tab) = synced_beside_protected(pages);
        let mut watching = app.agent_watches(Level::ReadOnly);
        let before = agent_reads(&app, plain_tab);
        // To the protected document's end and past it: the plain pane follows as far as it goes.
        person_scrolls_protected(&app, locked_tab, 1.0);
        let after = agent_reads(&app, plain_tab);
        format!("before: {before:?}\nafter: {after:?}\nheard: {:?}", watching.heard())
    };
    let (five, nine) = (told(5), told(9));
    assert_eq!(five, nine, "an agent told the protected document's length or place");
}
