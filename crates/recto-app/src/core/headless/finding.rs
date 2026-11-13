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

/// Failure (R2.3, C5g): the person's Find does something other than the `find` an agent sends: other hits, another hit gone to, another page, another
/// event, or something else kept in the record. Each control on one app, and its line at Full on another with the same file: typing, ⌘G, Return, a click on
/// a hit's row, Escape and ⌘F. Who a jump was by is the one difference, and the caller's (`by_agent`).
#[test]
fn the_windows_find_is_the_agents_find() {
    let twins = super::twins::Twins::on(&[("a.pdf", 12)]);
    // `page 1` is on pages 1, 10, 11 and 12; the list is a page's row and its hit for each, so row 5 is the third hit.
    let steps: Vec<(&str, UiAction, &str)> = vec![
        ("typing", UiAction::FindTyped { text: "page 1".into() }, r#"{"cmd":"find","query":"page 1"}"#),
        ("⌘G", UiAction::Menu { id: "find.next".into() }, r#"{"cmd":"find","next":true}"#),
        ("Return", UiAction::FindReturn { forward: true }, r#"{"cmd":"find","next":true,"jump":true}"#),
        ("a hit's row", UiAction::FindChoose { row: 5 }, r#"{"cmd":"find","hit":2}"#),
        ("Escape", UiAction::FindClose, r#"{"cmd":"find","close":true}"#),
        ("⌘F", UiAction::FindOpen, r#"{"cmd":"find","open":true}"#),
    ];
    for (what, action, line) in steps {
        let before = (twins.person.session_json()["find"].clone(), twins.person.reader());
        let (said, agent_said) = twins.both(action, line);
        assert_eq!(said, agent_said, "{what} and {line}: the same said");
        assert!(!said.is_empty(), "{what}: something announced");
        assert_ne!(
            (twins.person.session_json()["find"].clone(), twins.person.reader()),
            before,
            "{what}: something changed"
        );
        twins.assert_same(&format!("{what} and {line}"), &["by_agent"]);
        assert_eq!(find_view(&twins.person), find_view(&twins.agent), "{what}: Find shows the same");
    }
}

/// Failure (R2.3, A6): the person's ⌘F with the sidebar hidden opens Find where it can't be seen, as Find lives in the sidebar, or isn't heard as a change
/// to the view; or the person's ⌘G with Find closed doesn't open it with what was typed last. ⌘F shows the sidebar and is heard as `view_changed`; Escape,
/// then ⌘G, opens Find with the query found again and goes to its first hit.
#[test]
fn the_persons_find_shows_the_sidebar_and_opens_with_what_was_typed() {
    let app = findable();
    app.act(UiAction::ToggleSidebar);
    app.settle();
    assert!(app.reader().sidebar_hidden);
    drop(app.heard_events());
    app.act(UiAction::FindOpen);
    app.settle();
    assert!(!app.reader().sidebar_hidden, "⌘F shows the sidebar");
    assert_eq!(names(&app.heard_events()), ["view_changed", "find_changed"]);
    app.act(UiAction::FindTyped { text: "pdf page 1".into() });
    app.act(UiAction::FindClose);
    app.act(UiAction::ToggleSidebar);
    app.settle();
    app.act(UiAction::Menu { id: "find.next".into() });
    app.settle();
    let view = find_view(&app);
    assert!(view.open && view.row_count == 22 && view.current_row == Some(1), "{view:?}");
    assert!(!app.reader().sidebar_hidden, "⌘G opening Find shows the sidebar");
}

/// Failure: a query too long to find is searched anyway, or the line under the field doesn't say why nothing's found, or what was typed is lost from the
/// field; or the line keeps saying it once something findable is typed.
#[test]
fn a_query_too_long_is_said_and_not_searched() {
    let app = findable();
    let text = "x".repeat(recto_agent::find::MAX_QUERY_BYTES + 1);
    app.act(UiAction::FindTyped { text: text.clone() });
    app.settle();
    let view = find_view(&app);
    assert!(view.open);
    assert_eq!(view.status, recto_agent::find::too_long().message, "the line says why");
    assert_eq!(view.text, text, "the field keeps what was typed");
    assert!(!view.running && view.row_count == 0, "nothing searched: {view:?}");
    assert_eq!(app.session_json()["find"]["query"], "", "nothing searched for");
    app.act(UiAction::FindTyped { text: "pdf".into() });
    app.settle();
    assert_ne!(find_view(&app).status, recto_agent::find::too_long().message, "said no more");
}

/// Failure (rule 6, R2.3 C5g): an agent's `find`, in any form, on a document the person opened with its password, gets a hit, a page or a count read with
/// the password, at any level, or the person's own Find there finds nothing, as their passwords went with the command. The person's Find finds its hits and
/// goes to one, and what watchers hear of it has no page; every form of an agent's `find` is refused, or says nothing of what's in it.
#[test]
fn an_agents_find_gets_nothing_of_a_protected_tab_and_the_persons_finds_it() {
    let app = HeadlessApp::start();
    let file = app.disk().file("locked.pdf", &super::files::locked_with("right", 3));
    app.act(crate::core::controls::UiAction::Open {
        path: file.display().to_string(),
    });
    app.settle();
    let id = app.reader().password_prompt.expect("it asks for its password").id;
    app.core().password_answered(id, Some("right".into()));
    app.settle();
    app.hear_events();
    app.act(UiAction::FindTyped { text: "locked page".into() });
    app.settle();
    let view = find_view(&app);
    assert_eq!(view.row_count, 6, "the person's Find finds its three hits: {view:?}");
    drop(app.heard_events());
    app.act(UiAction::FindReturn { forward: true });
    app.act(UiAction::FindReturn { forward: true });
    app.settle();
    let heard = app.heard_events();
    assert_eq!(names(&heard), ["page_changed", "find_changed", "page_changed", "find_changed"]);
    let shown = format!("{heard:?}");
    assert!(!shown.contains("Some("), "no page for watchers: {shown}");
    let leaks = |value: &serde_json::Value| {
        let text = value.to_string();
        text.contains("Locked page") || text.contains("\"hits_total\":3") || text.contains("\"page\":2")
    };
    for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
        for line in [
            r#"{"cmd":"find","query":"locked page"}"#,
            r#"{"cmd":"find"}"#,
            r#"{"cmd":"find","next":true}"#,
            r#"{"cmd":"find","previous":true,"jump":true}"#,
            r#"{"cmd":"find","hit":0}"#,
            r#"{"cmd":"find","open":true}"#,
            r#"{"cmd":"find","close":true}"#,
        ] {
            let (result, said) = app.agent_heard(level, line);
            let answer = result.unwrap_or_else(|error| serde_json::json!(format!("{error:?}")));
            assert!(!leaks(&answer), "{level:?} {line}: {answer}");
            assert!(!format!("{said:?}").contains("Some("), "{level:?} {line}: {said:?}");
        }
    }
    app.settle();
    assert_eq!(find_view(&app).row_count, 6, "and the person's Find is as it was");
}

/// What the sidebar draws of Find.
fn find_view(app: &HeadlessApp) -> crate::core::find::FindView {
    crate::core::find::view(app.core())
}

/// Failure (R2.3, C5g's follow-up: an announcement is made when something changed, and only then): Escape, or an agent's `find --close`, with Find closed
/// already announces `find_changed`, though nothing about Find changed. The first close is announced; a second, the person's or an agent's, isn't.
#[test]
fn closing_find_that_is_closed_announces_nothing() {
    let app = findable();
    app.act(UiAction::FindTyped { text: "pdf".into() });
    app.settle();
    drop(app.heard_events());
    app.act(UiAction::FindClose);
    app.settle();
    assert_eq!(names(&app.heard_events()), ["find_changed"], "Find closed");
    app.act(UiAction::FindClose);
    app.settle();
    assert_eq!(names(&app.heard_events()), Vec::<&str>::new(), "the person's Escape with Find closed");
    let said = agent_said(&app, r#"{"cmd":"find","close":true}"#);
    assert!(said.is_empty(), "an agent's close with Find closed: {said:?}");
}
