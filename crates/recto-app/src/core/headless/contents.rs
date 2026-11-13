//! **The sidebar's contents, as the person uses them** (step R2.3, C5f): a section's chevron, and the note over a tab's contents, each the command an agent
//! sends, run for the person.

use super::files::with_sections;
use super::twins::Twins;
use crate::core::controls::UiAction;
use crate::queue::CoreCall;
use crate::testkit::headless::HeadlessApp;
use recto_session::{GeneratedView, TabId};
use std::sync::Arc;

/// The chevron of the first contents row that folds, as the window reports it: its place among the rows, and the entry it's on.
fn chevron(app: &HeadlessApp) -> (u32, u32) {
    let published = crate::lock(&app.core().published).contents.clone();
    let (first, entry) = published.iter().enumerate().find(|(_, entry)| entry.expandable).expect("a section that folds");
    (u32::try_from(first).expect("a row"), entry.entry)
}

/// The note over the active tab's contents, tapped, as the window reports it.
fn note(app: &HeadlessApp, action: recto_session::ContentsAction) -> UiAction {
    let tab = app.reader().shown_tab;
    let row = app.tabs().into_iter().find(|row| row.tab == tab).expect("its row");
    UiAction::Row {
        row: crate::SidebarRow {
            kind: crate::RowKind::Note,
            action: Some(action.id().into()),
            page: None,
            ..row
        },
        adding: false,
    }
}

/// Both apps' active tab given generated contents in this state, as a look at the helper's would, so the note offers what follows from it.
fn generated_are(twins: &Twins, view: impl Fn() -> GeneratedView) {
    for app in [&twins.person, &twins.agent] {
        let id = TabId(app.reader().shown_tab);
        app.core().host.session.with_session(|session| session.set_generated_view(id, view()));
        app.send(CoreCall::Reconcile);
        app.settle();
    }
}

/// Failure (R2.3, C5f): a contents section's chevron, or the note's Show generated, Show own or Stop, does something other than the command an agent
/// sends: another section folded, other contents shown, another event, or something else kept in the record. Each on one app, and its line at Full on another
/// with the same files.
#[test]
fn the_windows_contents_controls_are_the_agents_commands() {
    let twins = Twins::with_generating(&[("a.pdf", with_sections(6)), ("b.pdf", with_sections(5))]);
    let tab = twins.person.reader().shown_tab;
    let (first, entry) = chevron(&twins.person);
    let fold = format!(r##"{{"cmd":"contents_fold","entry":{entry},"tab":"#{tab}"}}"##);
    for what in ["fold", "open again"] {
        let before = twins.person.session_json();
        let (said, agent_said) = twins.both(UiAction::Chevron { first }, &fold);
        assert_eq!(said, agent_said, "{what}: the same said");
        assert_ne!(twins.person.session_json(), before, "{what}: the fold is kept");
        twins.assert_same(what, &[]);
    }
    let lines = [
        (
            recto_session::ContentsAction::ShowGenerated,
            format!(r##"{{"cmd":"contents_show","which":"generated","tab":"#{tab}"}}"##),
        ),
        (
            recto_session::ContentsAction::ShowOwn,
            format!(r##"{{"cmd":"contents_show","which":"own","tab":"#{tab}"}}"##),
        ),
        (recto_session::ContentsAction::Stop, format!(r##"{{"cmd":"contents_stop","tab":"#{tab}"}}"##)),
    ];
    let made = recto_core::Toc::from_lines(
        &[recto_core::TocLine {
            title: "Made heading",
            page: 1,
            parent: recto_core::NONE,
            depth: 0,
        }],
        false,
    );
    for (action, line) in lines {
        match action {
            recto_session::ContentsAction::Stop => generated_are(&twins, || GeneratedView::Making),
            recto_session::ContentsAction::ShowGenerated => generated_are(&twins, || GeneratedView::Ready(Arc::new(made.clone()))),
            _ => {}
        }
        let before = (twins.person.sidebar(), twins.person.session_json());
        let (said, agent_said) = twins.both(note(&twins.person, action), &line);
        assert_eq!(said, agent_said, "{action:?}: the same said");
        assert_ne!((twins.person.sidebar(), twins.person.session_json()), before, "{action:?}: something changed");
        twins.assert_same(&format!("{action:?} and {line}"), &[]);
    }
}
