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

/// The person scrolls `tab`'s pane down by `documents` of its whole height from where it is (up if less than 0), as their hand does; a synced pane
/// follows. Settled.
fn person_scrolls(app: &HeadlessApp, tab: u32, documents: f64) {
    let inner = std::sync::Arc::clone(app.core());
    let pane = (0..inner.panes.count())
        .find(|pane| inner.panes.tab_of(*pane).map(|id| id.0) == Some(tab))
        .expect("in a pane");
    let engine = inner.panes.engine(&inner.engine, pane).expect("its page view");
    let scroll = |y: f64, magnification: f64| recto_reader::Viewport {
        y,
        magnification,
        backing: 2.0,
        scrolled_to: u64::MAX - 1,
        ..recto_reader::Viewport::default()
    };
    let place = |engine: &recto_reader::Engine| engine.places().1.into_iter().find(|(id, _)| *id == tab).expect("its place").1;
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
        person_scrolls(&app, locked_tab, 1.0);
        let after = agent_reads(&app, plain_tab);
        format!("before: {before:?}\nafter: {after:?}\nheard: {:?}", watching.heard())
    };
    let (five, nine) = (told(5), told(9));
    assert_eq!(five, nine, "an agent told the protected document's length or place");
}

/// A ⌘-click on `tab`'s row, as the person puts a tab side by side or takes it out. Settled.
fn command_click(app: &HeadlessApp, tab: u32) {
    let row = app.tabs().into_iter().find(|row| row.tab == tab).expect("its row");
    app.act(UiAction::Row { row, adding: true });
    app.settle();
}

/// An agent at Control, at `line`: its answer, or its refusal's code.
fn control(app: &HeadlessApp, line: &str) -> Result<Value, String> {
    app.agent(Level::Control, line).map_err(|error| error.code.to_string())
}

/// Failure (security review of phase R, S2): an agent at Control makes for itself what the person's synced comparison gave away: a protected tab synced
/// beside a plain one, then reads `compare`'s note, or makes a plain tab follow a protected one. Each way to a synced comparison with a protected pane is
/// refused as `protected` (put side by side and synced at once; synced afterwards; a tab added to one the person synced); side by side with separate
/// scroll is allowed, as nothing follows.
#[test]
fn an_agent_cant_make_a_synced_comparison_with_a_protected_pane() {
    let disk = Disk::new();
    let (plain, other) = (disk.pdf("plain.pdf", 30), disk.pdf("other.pdf", 30));
    let locked = protected(&disk, 5, "Locked");
    let app = disk.start();
    app.open(&plain);
    app.open(&other);
    person_unlocks(&app, &locked);
    let tab = |name: &str| app.tabs().into_iter().find(|row| row.text == name).expect(name).tab;
    let (plain_tab, other_tab, locked_tab) = (tab("plain"), tab("other"), tab("protected"));
    assert_eq!(
        control(&app, &format!(r##"{{"cmd":"compare","tabs":"#{plain_tab},#{locked_tab}","sync":"on"}}"##))
            .err()
            .as_deref(),
        Some("protected"),
        "side by side and synced at once"
    );
    control(&app, &format!(r##"{{"cmd":"compare","tabs":"#{plain_tab},#{locked_tab}"}}"##)).expect("side by side, separate");
    assert_eq!(
        control(&app, r#"{"cmd":"compare","sync":"on"}"#).err().as_deref(),
        Some("protected"),
        "synced afterwards"
    );
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.settle();
    assert_eq!(
        control(&app, &format!(r##"{{"cmd":"compare","toggle":"#{other_tab}"}}"##)).err().as_deref(),
        Some("protected"),
        "a tab added to the person's synced comparison would follow the protected one"
    );
}

/// Failure (S2, 3b): a plain tab left by following a protected one keeps the page it was left on, which says how far the protected one goes, and an
/// agent reads it once the person turns synced scroll off. Its place stays withheld until it's next set otherwise; an agent's `goto` on it sets it, and
/// says it again. A tab that never moved while it followed was never placed by it, and says its place.
#[test]
fn a_tab_left_by_following_a_protected_one_keeps_its_place_withheld_until_moved() {
    let page_of = |app: &HeadlessApp, tab: u32| app.recto(Level::ReadOnly, &["where", "--tab", &format!("#{tab}")]).expect("where")["page"].clone();
    let (app, plain_tab, _) = synced_beside_protected(5);
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    assert_eq!(page_of(&app, plain_tab), 1, "never moved while it followed: its own place");
    let (app, plain_tab, locked_tab) = synced_beside_protected(5);
    person_scrolls(&app, locked_tab, 1.0);
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    assert_eq!(page_of(&app, plain_tab), Value::Null, "left where following put it");
    command_click(&app, plain_tab);
    assert_eq!(page_of(&app, plain_tab), Value::Null, "and after the comparison ends");
    control(&app, &format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":3}}"##)).expect("goto");
    app.settle();
    assert_eq!(page_of(&app, plain_tab), 3, "an agent's goto set its place");
}

/// **What an agent is told while the person compares a plain document with a protected one of `pages` pages:** side by side; synced, the protected one
/// leading to its end; the plain one leading back up; separate; and an agent at Control asking for synced scroll and setting a separate comparison of
/// its own. At each step, a watch on every group and what `compare`, `where` on the plain tab, `tabs`, `session show` and `sidebar` say.
fn agent_told_of_a_comparison(pages: usize) -> Vec<String> {
    let disk = Disk::new();
    let plain = disk.pdf("plain.pdf", 30);
    let locked = protected(&disk, pages, "Locked");
    let app = disk.start();
    let mut watching = app.agent_watches(Level::Control);
    app.open(&plain);
    person_unlocks(&app, &locked);
    let (plain_tab, locked_tab) = (app.tabs()[0].tab, app.tabs()[1].tab);
    let mut told = Vec::new();
    let mut look = |app: &HeadlessApp, step: &str| {
        let mut told = vec![format!("{step} heard: {:?}", watching.heard())];
        for words in [
            vec!["compare".to_string()],
            vec!["where".into(), "--tab".into(), format!("#{plain_tab}")],
            vec!["tabs".into()],
            vec!["session".into(), "show".into()],
            vec!["sidebar".into()],
        ] {
            let words: Vec<&str> = words.iter().map(String::as_str).collect();
            told.push(format!("{step} {words:?}: {:?}", app.recto(Level::Control, &words)));
        }
        told
    };
    let row = app.tabs().into_iter().find(|row| row.tab == plain_tab).expect("its row");
    app.act(UiAction::Row { row, adding: true });
    app.act(UiAction::PaneRead { tab: locked_tab });
    app.settle();
    told.extend(look(&app, "side by side"));
    app.act(UiAction::Menu { id: "compare.synced".into() });
    app.settle();
    told.extend(look(&app, "synced"));
    person_scrolls(&app, locked_tab, 1.0);
    told.extend(look(&app, "the protected one led to its end"));
    app.act(UiAction::PaneRead { tab: plain_tab });
    person_scrolls(&app, plain_tab, -0.05);
    told.extend(look(&app, "the plain one led back up"));
    app.act(UiAction::Menu { id: "compare.separate".into() });
    app.settle();
    told.extend(look(&app, "separate"));
    told.push(format!("an agent syncs: {:?}", control(&app, r#"{"cmd":"compare","sync":"on"}"#).map(|_| ())));
    command_click(&app, plain_tab);
    told.push(format!(
        "an agent's own: {:?}",
        control(&app, &format!(r##"{{"cmd":"compare","tabs":"#{locked_tab},#{plain_tab}"}}"##)).map(|_| ())
    ));
    app.settle();
    told.extend(look(&app, "an agent's own, separate"));
    // A record really written, with the protected tab's place and length in it.
    app.advance(60_000);
    app.settle();
    told.extend(look(&app, "the record written"));
    told
}

/// Failure (S2; the walk of a comparison beside the events' walk in `withheld_events`): something of a protected document reaches an agent through a
/// comparison: its length in `compare`'s note, where synced scroll started it, the plain pane's place as it follows (where, compare's panes, `tabs`,
/// `session show`, `sidebar`, `page_changed`), the place it was left on, or the record's sizes in `session show`. An agent watching at Control, and
/// looking at each step, is told the same of a five-page and a nine-page protected document.
#[test]
fn an_agent_is_told_the_same_of_two_compared_protected_documents() {
    let (one, two) = (agent_told_of_a_comparison(5), agent_told_of_a_comparison(9));
    let differs: Vec<String> = one.iter().zip(&two).filter(|(a, b)| a != b).map(|(a, b)| format!("  {a}\n  {b}")).collect();
    assert!(
        differs.is_empty(),
        "the agent was told something of the protected document:\n{}",
        differs.join("\n")
    );
    assert_eq!(one.len(), two.len());
}
