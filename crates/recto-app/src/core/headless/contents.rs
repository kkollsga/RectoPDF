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

/// Generated contents with a section in them, shown on the active tab, as the helper's look would leave them.
fn generated_with_a_section(app: &HeadlessApp) -> u32 {
    let lines = [
        recto_core::TocLine {
            title: "Part",
            page: 0,
            parent: recto_core::NONE,
            depth: 0,
        },
        recto_core::TocLine {
            title: "Sub",
            page: 1,
            parent: 0,
            depth: 1,
        },
        recto_core::TocLine {
            title: "Next",
            page: 2,
            parent: recto_core::NONE,
            depth: 0,
        },
    ];
    let made = Arc::new(recto_core::Toc::from_lines(&lines, false));
    let id = TabId(app.reader().shown_tab);
    app.core().host.session.with_session(|session| {
        session.set_generated_view(id, GeneratedView::Ready(made));
        session.tabs_mut().choose_generated(id, true);
    });
    app.send(CoreCall::Reconcile);
    app.settle();
    id.0
}

/// A guard, not the proof of a fix (it passes on `main`): the window's chevron folds generated contents as `contents fold` does. Failure: it does nothing
/// on generated contents, so a QA verb that calls the control's method would be silent where the agent's command is not.
#[test]
fn the_chevron_folds_generated_contents() {
    let twins = Twins::with_generating(&[("a.pdf", with_sections(6))]);
    let tab = generated_with_a_section(&twins.person);
    generated_with_a_section(&twins.agent);
    let (first, entry) = chevron(&twins.person);
    let fold = format!(r##"{{"cmd":"contents_fold","entry":{entry},"tab":"#{tab}"}}"##);
    let rows = |app: &HeadlessApp| crate::lock(&app.core().published).contents.len();
    let before = rows(&twins.person);
    twins.both(UiAction::Chevron { first }, &fold);
    assert_ne!(rows(&twins.person), before, "the chevron folded nothing");
    assert_eq!(rows(&twins.person), rows(&twins.agent), "the same rows as the command leaves");
}

/// A file of three pages with a flat outline of six entries, three on page 1, two on page 2 and one on page 3, each with its place down the page as the
/// file's own `/XYZ` says (the last has none).
fn several_to_a_page() -> Vec<u8> {
    let mut objects = vec![
        recto_testpdf::Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 6 0 R >>".into()),
        recto_testpdf::Object::Dict("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 600 800] >>".into()),
        recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R >>".into()),
        recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R >>".into()),
        recto_testpdf::Object::Dict("<< /Type /Page /Parent 2 0 R >>".into()),
        recto_testpdf::Object::Dict("<< /Type /Outlines /First 7 0 R /Last 12 0 R /Count 6 >>".into()),
    ];
    let dests = [
        "[3 0 R /XYZ 0 800 0]",
        "[3 0 R /XYZ 0 520 0]",
        "[3 0 R /XYZ 0 200 0]",
        "[4 0 R /XYZ 0 700 0]",
        "[4 0 R /XYZ 0 300 0]",
        "[5 0 R /Fit]",
    ];
    for (at, dest) in dests.iter().enumerate() {
        let next = if at + 1 < dests.len() {
            format!("/Next {} 0 R", 8 + at)
        } else {
            String::new()
        };
        let prev = if at > 0 { format!("/Prev {} 0 R", 6 + at) } else { String::new() };
        objects.push(recto_testpdf::Object::Dict(format!(
            "<< /Title (Entry {at}) /Parent 6 0 R {next} {prev} /Dest {dest} >>"
        )));
    }
    recto_testpdf::build(&objects)
}

/// The same entries as generated contents: what the helper found, with its places in thousandths.
fn generated_several_to_a_page() -> Arc<recto_core::Toc> {
    let line = |title, page| recto_core::TocLine {
        title,
        page,
        parent: recto_core::NONE,
        depth: 0,
    };
    let lines = [
        line("Entry 0", 0),
        line("Entry 1", 0),
        line("Entry 2", 0),
        line("Entry 3", 1),
        line("Entry 4", 1),
        line("Entry 5", 2),
    ];
    Arc::new(recto_core::Toc::from_lines(&lines, false).with_tops(&[0, 350, 750, 125, 625, 0]))
}

/// The row the window reports for a contents entry, as `core.contentsRows` makes it and the click hands it back.
fn host_row(entry: &crate::ContentsEntry) -> crate::SidebarRow {
    crate::SidebarRow {
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
    }
}

fn rows(app: &HeadlessApp) -> Vec<crate::ContentsEntry> {
    crate::lock(&app.core().published).contents.to_vec()
}

fn shows_generated(app: &HeadlessApp) {
    let id = TabId(app.reader().shown_tab);
    app.core().host.session.with_session(|session| {
        session.set_generated_view(id, GeneratedView::Ready(generated_several_to_a_page()));
        session.tabs_mut().choose_generated(id, true);
    });
    app.send(CoreCall::Reconcile);
    app.settle();
}

/// Failure (finding 3 of the contents fixes, and the rule it holds): after going to a contents entry by the click on its row, a page that holds several
/// entries marks another than the one gone to (the last of the page, or the one above), so the contents scroll to the wrong row and the reader's place isn't
/// the entry's. Every entry of a file's own contents and of generated ones, in turn, from another place each time.
#[test]
fn after_going_to_any_entry_that_entry_is_current_own_and_generated() {
    for generated in [false, true] {
        let disk = crate::testkit::headless::Disk::new();
        let file = disk.file("several.pdf", &several_to_a_page());
        let app = disk.start();
        app.open(&file);
        if generated {
            shows_generated(&app);
        }
        let entries = rows(&app);
        assert_eq!(entries.len(), 6, "generated: {generated}");
        for index in (0..entries.len()).rev().chain(0..entries.len()) {
            app.act(UiAction::Row {
                row: host_row(&entries[index]),
                adding: false,
            });
            app.settle();
            assert_eq!(
                app.sidebar().contents_current,
                Some(index as u32),
                "generated: {generated}, after going to entry {index}"
            );
        }
    }
}

/// Failure: the person's click on an entry lands somewhere than the agent's way to the same entry (`goto` with the page and how far down), or isn't made the
/// same way (a different trail, event or record), or Back after it doesn't return to where the reader was.
#[test]
fn a_click_on_an_entry_is_the_agents_goto_to_its_place_and_back_returns() {
    let twins = Twins::with_generating(&[("several.pdf", several_to_a_page())]);
    let tab = twins.person.reader().shown_tab;
    let entries = rows(&twins.person);
    let line = format!(r##"{{"cmd":"goto","tab":"#{tab}","page":2,"within":0.625}}"##);
    let (said, agent_said) = twins.both(
        UiAction::Row {
            row: host_row(&entries[4]),
            adding: false,
        },
        &line,
    );
    assert_eq!(said, agent_said, "the same said");
    twins.assert_same("a click on entry 4 and goto 2 within 0.625", &["by_agent"]);
    assert_eq!(twins.person.sidebar().contents_current, Some(4));
    let page = |app: &HeadlessApp| {
        app.core()
            .host
            .session
            .with_session(|s| s.tabs().active().and_then(|id| s.tabs().tab(id)).map(|tab| tab.page))
    };
    assert_eq!(page(&twins.person), Some(1));
    twins.person.act(UiAction::Menu { id: "go.back".into() });
    twins.person.settle();
    assert_eq!(page(&twins.person), Some(0), "Back returns to where the reader was");
    assert_eq!(twins.person.sidebar().contents_current, Some(0));
}

/// Failure: a row that the published entries can't name goes to a place by guess: two entries with the same words, page and depth land at the first's or the
/// second's place, or an entry with no place lands anywhere than the page's top.
#[test]
fn a_row_two_entries_could_be_goes_to_the_top_of_its_page() {
    use crate::core::navigation::entry_place;
    let entry = |text: &str, top: u16| crate::ContentsEntry {
        tab: 1,
        text: text.into(),
        page: Some(2),
        depth: 0,
        entry: 0,
        top,
        expandable: false,
        expanded: false,
    };
    let list = [entry("Notes", 200), entry("Notes", 600), entry("Intro", 300), entry("Plain", 0)];
    let row = |text: &str| host_row(&entry(text, 0));
    assert_eq!(entry_place(&list, &row("Notes")), None, "two the same aren't told apart by a guess");
    assert_eq!(entry_place(&list, &row("Intro")), Some(0.3));
    assert_eq!(entry_place(&list, &row("Plain")), None, "no place is the page's top");
    assert_eq!(entry_place(&list, &row("Missing")), None);
}
