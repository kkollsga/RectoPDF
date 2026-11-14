//! **Find tells the host the model changed when something changed, and only then** (rule: "an announcement is made when something changed, and only then,
//! for a person and an agent alike"), for every Find command from every state it can be in, with what the window pulls compared before and after.

use crate::core::controls::UiAction;
use crate::testkit::Heard;
use crate::testkit::headless::{Disk, HeadlessApp};

/// What the window pulls after a `model_changed`: Find's view and its rows, the sidebar's model and the reader's.
fn pulled(app: &HeadlessApp) -> String {
    let core = app.core();
    let find = crate::core::find::view(core);
    let rows = crate::core::find::rows(core, 0, 256);
    format!("{find:?} {rows:?} {:?} {:?}", app.sidebar(), app.reader())
}

/// A file of 20 pages: `pdf page 1` is on page 1 and pages 10 to 19, eleven hits.
fn findable() -> HeadlessApp {
    let disk = Disk::new();
    let doc = disk.pdf("findable.pdf", 20);
    let app = disk.start();
    app.open(&doc);
    app
}

fn run(app: &HeadlessApp, line: &str) {
    app.person(line);
    app.settle();
}

const QUERY: &str = r#"{"cmd":"find","query":"pdf page 1"}"#;
const NEXT: &str = r#"{"cmd":"find","next":true}"#;

/// Every state Find can be in that a command is run from, as the commands that bring it about.
fn states() -> Vec<(&'static str, Vec<String>)> {
    let next = |times: usize| vec![NEXT.to_string(); times];
    let typed = |query: &str| vec![format!(r#"{{"cmd":"find","query":"{query}"}}"#)];
    vec![
        ("closed, never used", vec![]),
        ("open, nothing typed", vec![r#"{"cmd":"find","open":true}"#.into()]),
        ("a query with hits, none gone to", typed("pdf page 1")),
        ("on the first hit", [typed("pdf page 1"), next(1)].concat()),
        ("on a middle hit", [typed("pdf page 1"), next(5)].concat()),
        ("on the last hit", [typed("pdf page 1"), next(11)].concat()),
        ("a query with no hits", typed("zzqx")),
        (
            "closed with a query kept",
            [typed("pdf page 1"), vec![r#"{"cmd":"find","close":true}"#.into()]].concat(),
        ),
    ]
}

/// The commands, as the window sends them for the person (⌘F, Escape, ⌘G, ⇧⌘G, Return, a click on a hit's row, typing).
fn actions() -> Vec<(&'static str, &'static str)> {
    vec![
        ("open", r#"{"cmd":"find","open":true}"#),
        ("close", r#"{"cmd":"find","close":true}"#),
        ("next", r#"{"cmd":"find","next":true}"#),
        ("previous", r#"{"cmd":"find","previous":true}"#),
        ("next --jump", r#"{"cmd":"find","next":true,"jump":true}"#),
        ("hit 0", r#"{"cmd":"find","hit":0}"#),
        ("hit 3", r#"{"cmd":"find","hit":3}"#),
        ("the same query again", QUERY),
        ("another query", r#"{"cmd":"find","query":"zzqx"}"#),
        ("an empty query", r#"{"cmd":"find","query":""}"#),
    ]
}

/// Failure: Find tells the host the model changed after a command that changed nothing it would pull (⌘F with Find open, ⌘G with nothing to find, a
/// click on the hit already gone to, the same query again), a redraw for nothing; or it doesn't tell after one that did, and the window shows Find as it was;
/// or it tells twice for one command that has no search behind it. The table has each command from each state, the host told or not against what the window
/// pulls before and after. A command that starts a search or moves the page has more than one change (Find's, then the hits' or the page's) and is told for each; any other, at most once.
#[test]
fn find_tells_the_host_when_what_it_pulls_changed_and_only_then() {
    let mut wrong = Vec::new();
    let mut table = String::new();
    for (state, setup) in states() {
        for (name, line) in actions() {
            let app = findable();
            for step in &setup {
                run(&app, step);
            }
            let before = pulled(&app);
            let page = app.reader();
            app.listener().take();
            run(&app, line);
            let told = app.listener().take().iter().filter(|heard| **heard == Heard::ModelChanged).count();
            let changed = pulled(&app) != before;
            // A command that moves the page, or starts a search (a typed query, or Find opened again over a query kept), has more than one change to announce: Find's, and the page's or the hits'.
            let more_than_one = line.contains("query") || state.contains("kept") || app.reader() != page;
            table += &format!("{state:34} {name:22} changed {changed:5} told {told}\n");
            if (told > 0) != changed || (!more_than_one && told > 1) {
                wrong.push(format!("{state} / {name}: changed {changed}, told {told}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}\n{table}");
    // A last look at the typed controls the table doesn't run: Return in the field with nothing found is no change either.
    let app = findable();
    app.act(UiAction::FindOpen);
    app.settle();
    let before = pulled(&app);
    app.listener().take();
    app.act(UiAction::FindReturn { forward: true });
    app.settle();
    assert_eq!(app.listener().take().len(), usize::from(pulled(&app) != before), "Return with nothing to find");
}
