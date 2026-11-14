//! **Going to a page, as the person does it** (step R2.3, C5b): a contents row, the go-to field, Back and Forward, the swipe and a link to a page are each
//! the command an agent sends, run for the person.

use super::twins::Twins;
use crate::RowKind;
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;

/// One way of going somewhere: its name, what the person does, the agent's line, and the page it lands on, counted from 1.
type Step = (&'static str, Box<dyn Fn(&HeadlessApp)>, String, u64);

fn menu(id: &str) -> UiAction {
    UiAction::Menu { id: id.into() }
}

/// A contents row of the first tab, naming `page`.
fn contents(app: &HeadlessApp, page: u32) -> UiAction {
    let row = app.tabs()[0].clone();
    UiAction::Row {
        row: crate::SidebarRow {
            kind: RowKind::Contents,
            page: Some(page),
            ..row
        },
        adding: false,
    }
}

/// The page the first tab's on, counted from 1.
fn page(app: &HeadlessApp) -> u64 {
    app.session_json()["tabs"][0]["page"].as_u64().expect("a page") + 1
}

/// A click on page 1's first link (to page 9, a third of the way down), as the page view reports the hit.
fn click_first_link(app: &HeadlessApp, file: &std::path::Path) {
    let read = recto_core::nav::page_links(file, &[0]).expect("the links read");
    let link = read[0].1.links[0].clone();
    let inner = app.core();
    let engine = inner.panes.reading_engine(&inner.engine);
    let hit = recto_reader::links::LinkHit {
        page: 0,
        rect: link.rect,
        target: link.target,
    };
    crate::core::follow_link(inner, 0, &engine, hit);
}

/// Failure (R2.3, C5b): a contents row, the go-to field, Back, Forward, a swipe or a link to a page does something other than the command an agent sends:
/// another page, another event, another trail, or something else kept. Each on one app, and its line at Full on another with the same file. A link's click
/// is compared with `follow` and with `goto --within`, which an agent reaches the same place by.
#[test]
fn the_windows_ways_of_going_places_are_the_agents_commands() {
    let twins = Twins::with(&[("linked.pdf", recto_testpdf::linked_document())]);
    let file = twins.person.disk().files().join("linked.pdf");
    let within = match recto_core::nav::page_links(&file, &[0]).expect("the links read")[0].1.links[0].target {
        recto_core::nav::LinkTarget::Page { within: Some(within), .. } => f64::from(within),
        ref other => panic!("the first link goes to a page a way down: {other:?}"),
    };
    let swipe_back = if crate::linking::swipe_goes_back(1.0) == Some(true) { 1.0 } else { -1.0 };
    let steps: Vec<Step> = vec![
        (
            "contents row",
            Box::new(|app| {
                app.act(contents(app, 7));
            }),
            r##"{"cmd":"goto","tab":"#1","page":7}"##.into(),
            7,
        ),
        (
            "go-to field",
            Box::new(|app| {
                app.act(UiAction::PageTyped { text: " 2 ".into() });
            }),
            r##"{"cmd":"goto","tab":"#1","label":"2"}"##.into(),
            6,
        ),
        (
            "Back",
            Box::new(|app| {
                app.act(menu("go.back"));
            }),
            r#"{"cmd":"back"}"#.into(),
            7,
        ),
        (
            "Forward",
            Box::new(|app| {
                app.act(menu("go.forward"));
            }),
            r#"{"cmd":"forward"}"#.into(),
            6,
        ),
        (
            "swipe",
            Box::new(move |app| {
                app.act(UiAction::Swiped { delta_x: swipe_back });
            }),
            r#"{"cmd":"back"}"#.into(),
            7,
        ),
        (
            "contents row to page 1",
            Box::new(|app| {
                app.act(contents(app, 1));
            }),
            r##"{"cmd":"goto","tab":"#1","page":1}"##.into(),
            1,
        ),
        (
            "link click",
            Box::new(|app| click_first_link(app, &app.disk().files().join("linked.pdf"))),
            r#"{"cmd":"follow","page":1,"link":1}"#.into(),
            9,
        ),
        (
            "Back from the link",
            Box::new(|app| {
                app.act(menu("go.back"));
            }),
            r#"{"cmd":"back"}"#.into(),
            1,
        ),
        (
            "link click again",
            Box::new(|app| click_first_link(app, &app.disk().files().join("linked.pdf"))),
            format!(r##"{{"cmd":"goto","tab":"#1","page":9,"within":{within}}}"##),
            9,
        ),
    ];
    for (what, person, line, landed) in steps {
        let (said, agent_said) = twins.both_with(person, &line);
        assert_eq!(said, agent_said, "{what} and {line}: the same said");
        assert_eq!(page(&twins.person), landed, "{what}: where it went");
        twins.assert_same(&format!("{what} and {line}"), &["by_agent"]);
    }
    // Who each jump was by is what the actor says: the person's are the owner's, an agent's give way to them.
    let marked = |app: &HeadlessApp| app.session_json().to_string().contains(r#""by_agent":true"#);
    assert!(!marked(&twins.person), "the person's jumps are the owner's");
    assert!(marked(&twins.agent), "an agent's are marked as an agent's");
}

/// Failure (accident A5): a contents row naming a page past the last, which a file whose readers count its pages differently can have, does nothing once
/// it's a `goto`, which refuses it, where a tap always landed on the last page; or `goto` takes such a page from an agent. The tap lands on the last page,
/// and the agent's `goto` of it is refused.
#[test]
fn a_contents_row_past_the_end_lands_on_the_last_page_and_goto_refuses_it() {
    let disk = Disk::new();
    let file = disk.pdf("a.pdf", 6);
    let app = disk.start();
    app.open(&file);
    app.act(contents(&app, 99));
    app.settle();
    assert_eq!(page(&app), 6, "the last page");
    let refused = app.agent(Level::Full, r##"{"cmd":"goto","tab":"#1","page":99}"##).expect_err("no page 99");
    assert_eq!(refused.code, "no_such_page");
    assert_eq!(page(&app), 6, "nothing moved");
}

/// Failure: a typed page that's no page says an agent's words to the person (`no_such_label`, naming `recto where`), or nothing, where the go-to field always
/// said it in its own.
#[test]
fn a_typed_page_thats_no_page_says_so_in_the_fields_words() {
    let disk = Disk::new();
    let file = disk.pdf("a.pdf", 6);
    let app = disk.start();
    app.open(&file);
    app.act(UiAction::PageTyped { text: "zz".into() });
    app.settle();
    assert_eq!(
        app.core().window.notices().notice(),
        Some(crate::linking::no_such_page("zz")),
        "the field's words"
    );
}
