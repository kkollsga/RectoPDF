//! **Find, as the person does it and as an agent does it** (step R2.3, C5g): what each announces, and the person's Find shown equal to the agent's `find`.

use super::twins::names;
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;

/// An app with a 30-page file open, `pdf page 1` on page 1 and pages 10 to 19, hearing what's announced from here on.
fn findable() -> HeadlessApp {
    let disk = Disk::new();
    let doc = disk.pdf("findable.pdf", 30);
    let app = disk.start();
    app.open(&doc);
    app.hear_events();
    app
}

/// Failure (R2.3, C5g, difference 1: an announcement is made when something changed, and only then): ⌘F with Find open already announces `find_changed`,
/// though it only gives the field the keyboard; or an agent's look at Find announces something. The first ⌘F opens Find and is announced, the second isn't,
/// and an agent's `find` with nothing announces nothing.
#[test]
fn opening_find_that_is_open_announces_nothing() {
    let app = findable();
    app.act(UiAction::FindOpen);
    app.settle();
    assert_eq!(names(&app.heard_events()), ["find_changed"], "Find opened");
    app.act(UiAction::FindOpen);
    app.settle();
    assert_eq!(names(&app.heard_events()), Vec::<&str>::new(), "⌘F with Find open");
    let (looked, said) = app.agent_heard(Level::Control, r#"{"cmd":"find"}"#);
    assert!(looked.is_ok(), "{looked:?}");
    app.settle();
    assert!(said.is_empty() && app.heard_events().is_empty(), "an agent's look: {said:?}");
}

/// What an agent's line made and what the session thread announced after it, settled.
fn agent_said(app: &HeadlessApp, line: &str) -> Vec<recto_agent::Event> {
    let (result, mut said) = app.agent_heard(Level::Control, line);
    assert!(result.is_ok(), "{line}: {result:?}");
    app.settle();
    said.extend(app.heard_events());
    said
}

/// Failure (R2.3, C5g, difference 3): an agent's step to a hit is heard with `page_changed` twice, once from the command and again from the window's
/// selecting the hit (`Effect::Reveal`). A hit gone to is heard as `page_changed` and then `find_changed` with its page, once each.
#[test]
fn an_agents_step_is_heard_once() {
    let app = findable();
    agent_said(&app, r#"{"cmd":"find","query":"pdf page 1"}"#);
    let said = agent_said(&app, r#"{"cmd":"find","next":true}"#);
    assert_eq!(names(&said), ["page_changed", "find_changed"], "{said:?}");
}

/// Failure: Return in the Find field is heard with `page_changed` twice, once for the jump to the hit and again when the page view, bringing the hit into
/// view, says it's on the page the session already had. A page is announced when it changed, and only then.
#[test]
fn return_in_find_is_heard_with_one_page_changed() {
    let app = findable();
    app.act(UiAction::FindTyped { text: "pdf page 1".into() });
    app.settle();
    // The first hit is on the page being read, and the second on page 10.
    for to in [1, 10] {
        drop(app.heard_events());
        app.act(UiAction::FindReturn { forward: true });
        app.settle();
        let heard = app.heard_events();
        let pages: Vec<_> = heard
            .iter()
            .filter_map(|event| match event {
                recto_agent::Event::PageChanged { page, .. } => Some(*page),
                _ => None,
            })
            .collect();
        assert_eq!(pages, [Some(to)], "{heard:?}");
    }
}

/// Failure (R2.3, C5g, difference 2): the person's step to a hit (⌘G, Return in the field, a click on a hit's row) is heard as `page_changed` alone, while
/// an agent's is heard with `find_changed` at the hit's page too: a watcher can't tell the hit gone to changed. Each is heard as `page_changed` and then
/// `find_changed` with the hit's page.
#[test]
fn a_persons_step_is_heard_as_an_agents() {
    let app = findable();
    app.act(UiAction::FindTyped { text: "pdf page 1".into() });
    app.settle();
    drop(app.heard_events());
    for (what, action) in [
        ("⌘G", UiAction::Menu { id: "find.next".into() }),
        ("Return", UiAction::FindReturn { forward: true }),
        ("a hit's row", UiAction::FindChoose { row: 1 }),
    ] {
        app.act(action);
        app.settle();
        let heard = app.heard_events();
        assert_eq!(names(&heard), ["page_changed", "find_changed"], "{what}: {heard:?}");
        let recto_agent::Event::PageChanged { page, .. } = &heard[0] else {
            unreachable!()
        };
        assert_eq!(heard[1], recto_agent::Event::FindOpened { page: *page }, "{what}: Find at the hit's page");
    }
}

/// The page being read, from 1.
fn page(app: &HeadlessApp) -> u64 {
    app.recto(Level::Control, &["where"]).expect("where")["page"].as_u64().expect("a page")
}

/// Failure (R2.3, C5g's new surface): an agent can't do what Return in the Find field and a click on a hit's row do: go to a hit marked for `back`, and go
/// to one hit by its index; or `--open` rearranges the person's window (A6) or doesn't find again what was typed last. `--next --jump` goes to page 10 and
/// `back` comes back; `--hit 2` goes to the third hit's page and says so; `--jump` alone is a usage error; `--open` after `--close` finds the query again and
/// leaves a hidden sidebar hidden.
#[test]
fn an_agent_jumps_to_a_hit_chooses_one_and_opens_find() {
    let app = findable();
    let at = |words: &[&str]| app.recto(Level::Control, words).unwrap_or_else(|error| panic!("{words:?}: {error:?}"));
    at(&["find", "pdf page 1"]);
    at(&["find", "--next"]);
    assert_eq!(page(&app), 1, "the first hit is on the page read");
    let went = at(&["find", "--next", "--jump"]);
    assert_eq!((went["went_to"]["page"].as_u64(), page(&app)), (Some(10), 10), "{went}");
    at(&["back"]);
    assert_eq!(page(&app), 1, "a jump is marked for back");
    let went = at(&["find", "--hit", "2"]);
    assert_eq!(
        (went["went_to"]["page"].as_u64(), went["current"]["index"].as_u64(), page(&app)),
        (Some(11), Some(2), 11),
        "{went}"
    );
    let refused = app.recto(Level::Control, &["find", "--hit", "999"]).expect_err("no such hit");
    assert_eq!(refused.code, "no_such_hit");
    let refused = app.recto(Level::Control, &["find", "--jump"]).expect_err("a jump needs a step");
    assert_eq!(refused.code, "usage");
    at(&["find", "--close"]);
    at(&["view", "--sidebar", "off"]);
    app.settle();
    let opened = at(&["find", "--open"]);
    app.settle();
    assert_eq!(
        (opened["open"].as_bool(), opened["hits_total"].as_u64()),
        (Some(true), Some(11)),
        "found again: {opened}"
    );
    assert!(app.reader().sidebar_hidden, "an agent's Find leaves the sidebar hidden");
    assert!(app.recto(Level::ReadOnly, &["find", "--open"]).is_err(), "at Control");
}
