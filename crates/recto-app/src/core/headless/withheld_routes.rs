//! **Which of the person's own moves let a place withheld from agents go** (the fourth security review, items 1 and 2; the coordinator's decisions of
//! 2026-11-14): only a jump to a place chosen by itself, a page by its number, label or contents entry, or a contents row. A link and a Find hit chosen
//! from the list are chosen from where the tab is, and Back, Forward, ⌘G, a scroll and a zoom land where they do from it: each leaves the place withheld.

use super::files::with_sections;
use super::withheld_compare::{deadlines_met, left_by_following_alone_in, shown_of};
use crate::core::controls::UiAction;
use crate::testkit::headless::HeadlessApp;
use recto_session::TabId;

/// The first row of Find's list that is a hit, once the person's query has some: what a click on it chooses.
fn first_hit_row(app: &HeadlessApp) -> u32 {
    let row = || {
        crate::core::find::rows(app.core(), 0, 8)
            .iter()
            .position(|row| row.kind == crate::find::FindRowKind::Hit)
            .and_then(|at| u32::try_from(at).ok())
    };
    app.wait_until("a hit in Find's list", || row().is_some());
    row().expect("a hit's row")
}

/// The person types `query` in Find and clicks its first hit's row, as the window reports both, the host turning as the page view sends it. Settled.
fn person_chooses_a_hit(app: &HeadlessApp, view: &mut recto_reader::Viewport, query: &str) {
    app.act(UiAction::FindTyped { text: query.into() });
    app.settle();
    let row = first_hit_row(app);
    app.act(UiAction::FindChoose { row });
    app.settle();
    crate::testkit::headless::host_turns(app, view, true);
}

/// Whether the plain tab's place is withheld from agents now.
fn withheld(app: &HeadlessApp, plain_tab: u32) -> bool {
    app.core()
        .host
        .session
        .with_session(|s| recto_session::compare::place_withheld(s, TabId(plain_tab)))
}

/// Failure (the fourth security review, item 1): the person's choice of a Find hit by its row let a withheld place go, and the app brings the hit into
/// view only as far as it isn't: when it's in view already the view doesn't move, so the place given after the choice is where following left the tab,
/// to the zoom and how far down. Twins whose protected documents differ in length leave the plain tab on different pages; the person chooses the hit on
/// the page following left the first twin on, in view there and not in the second. What an agent reads of the plain tab after it is the same, and none
/// of its place: it stays withheld.
#[test]
fn a_find_hit_the_person_chooses_by_its_row_keeps_a_withheld_place() {
    deadlines_met();
    let told = |pages: usize, word: &str, to: Option<u32>| {
        let (app, plain_tab, _, mut view, _) = left_by_following_alone_in(|disk| disk.pdf("plain.pdf", 30), pages, word);
        let left_on = app
            .core()
            .host
            .session
            .with_session(|s| s.tabs().tab(TabId(plain_tab)).expect("plain").page + 1);
        let to = to.unwrap_or(left_on);
        person_chooses_a_hit(&app, &mut view, &format!("plain.pdf page {to}"));
        // Looked at again, as every look reads the views.
        app.core().take_places();
        (shown_of(&app, plain_tab), withheld(&app, plain_tab), to, left_on)
    };
    let (five, five_withheld, to, left_on) = told(5, "Locked", None);
    let (nine, nine_withheld, _, left_on_too) = told(9, "Sealed", Some(to));
    assert_ne!(left_on, left_on_too, "following left the twins on different pages");
    assert_eq!(five, nine, "where following left the tab told after the person's choice of a hit");
    assert!(five_withheld && nine_withheld, "the place stays withheld after a hit chosen from the list");
    assert_eq!(five, serde_json::json!([null, null, null]), "nothing of the place");
}

/// Failure (the fourth security review, items 1 and 2): one of the person's own moves lets a withheld place go that leaves it a function of where
/// following left the tab (a link, a Find hit from the list, ⌘G, Back, Forward, a scroll, the page keys, which the host scrolls by as by a scroll, a zoom),
/// or one that sets a place by itself doesn't (a page typed in the go-to field, by its number or label, `goto` by page or contents entry, a contents row).
/// Each route as the window sends it, on a plain tab left by following a protected document, its place withheld again before each.
#[test]
fn only_the_persons_jump_to_a_place_chosen_by_itself_lets_a_withheld_place_go() {
    deadlines_met();
    let (app, plain_tab, _, mut view, engine) = left_by_following_alone_in(|disk| disk.file("plain.pdf", &with_sections(30)), 5, "Locked");
    let inner = std::sync::Arc::clone(app.core());
    let entry = || {
        let contents = crate::lock(&inner.published).contents.clone();
        contents
            .iter()
            .find(|entry| entry.tab == plain_tab && entry.page == Some(4))
            .cloned()
            .unwrap_or_else(|| panic!("the contents entry on page 4: {contents:?}"))
    };
    let mut lets_go = Vec::new();
    let mut route = |name: &str, go: &mut dyn FnMut(&mut recto_reader::Viewport)| {
        inner.host.session.with_session(|s| recto_session::compare::withhold_place(s, TabId(plain_tab)));
        assert!(withheld(&app, plain_tab), "{name}: withheld before");
        go(&mut view);
        app.settle();
        crate::testkit::headless::host_turns(&app, &mut view, true);
        let page = inner.host.session.with_session(|s| s.tabs().tab(TabId(plain_tab)).expect("plain").page + 1);
        lets_go.push((name.to_string(), !withheld(&app, plain_tab), page));
    };
    route("the go-to field, a page's number", &mut |_| {
        assert!(app.act(UiAction::PageTyped { text: "6".into() }).understood);
    });
    route("goto by page", &mut |_| {
        app.person(&format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":8}}"##))
    });
    route("goto by label", &mut |_| {
        app.person(&format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","label":"10"}}"##))
    });
    route("goto by contents entry", &mut |_| {
        let entry = entry();
        app.person(&format!(r##"{{"cmd":"goto","tab":"#{plain_tab}","page":4,"entry":{}}}"##, entry.entry));
    });
    route("a contents row", &mut |_| {
        let entry = entry();
        app.act(UiAction::Row {
            row: crate::SidebarRow {
                kind: crate::RowKind::Contents,
                tab: entry.tab,
                text: entry.text.to_string(),
                page: entry.page,
                depth: entry.depth,
                current: false,
                rail_filled: false,
                rail_first: false,
                rail_last: false,
                notice: None,
                action: None,
                expandable: entry.expandable,
                expanded: entry.expanded,
                in_comparison: false,
            },
            adding: false,
        });
    });
    route("a link", &mut |_| {
        let hit = recto_reader::links::LinkHit {
            page: 0,
            rect: [0.0; 4],
            target: recto_core::nav::LinkTarget::Page { page: 11, within: None },
        };
        crate::core::reconcile::follow_link(&inner, 0, &engine, hit);
    });
    route("a Find hit by its row", &mut |view| person_chooses_a_hit(&app, view, "Page 17"));
    route("⌘G", &mut |_| {
        app.act(UiAction::Menu { id: "find.next".into() });
    });
    route("Back", &mut |_| {
        app.act(UiAction::Menu { id: "go.back".into() });
    });
    route("Forward", &mut |_| {
        app.act(UiAction::Menu { id: "go.forward".into() });
    });
    // The host scrolls the view by the person's hand, as it does for the page keys too.
    route("a scroll, and the page keys", &mut |view| {
        view.y += 300.0;
        crate::testkit::headless::applied_as_the_host(&engine.set_viewport(*view));
    });
    route("a zoom", &mut |_| {
        app.act(UiAction::Menu { id: "view.zoom_in".into() });
    });
    // Each route, whether it let the place go, and the page it left the tab on, so a route that did nothing can't pass as one that kept the place.
    let expected: Vec<(String, bool, u32)> = [
        ("the go-to field, a page's number", true, 6),
        ("goto by page", true, 8),
        ("goto by label", true, 10),
        ("goto by contents entry", true, 4),
        ("a contents row", true, 4),
        ("a link", false, 12),
        ("a Find hit by its row", false, 17),
        ("⌘G", false, 17),
        ("Back", false, 12),
        ("Forward", false, 17),
        ("a scroll, and the page keys", false, 17),
        ("a zoom", false, 17),
    ]
    .map(|(name, go, page)| (name.to_string(), go, page))
    .into();
    assert_eq!(lets_go, expected, "which of the person's moves let the withheld place go");
}
