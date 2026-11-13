//! **The list of tabs and files, as the person uses it** (step R2.3, C5c): a tap on a tab's row, the palette's tab row and Clear Menu are each the command an
//! agent sends, run for the person.

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::Level;
use std::sync::atomic::Ordering;

/// One control: its name, what the person does, and the agent's line.
type Step = (&'static str, Box<dyn Fn(&HeadlessApp)>, String);

/// A tab's row, as the window reports a tap on it.
fn row(app: &HeadlessApp, tab: u32) -> UiAction {
    let row = app.tabs().into_iter().find(|row| row.tab == tab).expect("its row");
    UiAction::Row { row, adding: false }
}

/// Without sleep files, and with this tab put to sleep, so waking it reads its PDF: the tap that wakes it waits for the read.
fn cold(app: &HeadlessApp, tab: u32) {
    app.core().host.session.with_session(|session| session.set_sleep_files(false));
    app.agent(Level::Full, &format!(r##"{{"cmd":"session_sleep","tab":"#{tab}"}}"##))
        .expect("put to sleep");
    app.settle();
}

/// Failure (R2.3, C5c): a tap on a tab's row, the palette's tab row or Clear Menu does something other than the command an agent sends: another tab in front,
/// contents folded or not, another event, or something else kept. Each on one app, and its line at Full on another with the same files: a warm tab, the tab in
/// front (which folds its contents), a cold one that's read first, the palette's row, and the list of files opened lately cleared.
#[test]
fn the_windows_tab_controls_are_the_agents_commands() {
    let twins = Twins::on(&[("a.pdf", 3), ("b.pdf", 3), ("c.pdf", 3)]);
    let tap = |tab: u32| format!(r##"{{"cmd":"tap","tab":"#{tab}"}}"##);
    let steps: Vec<Step> = vec![
        (
            "a warm tab",
            Box::new(|app| {
                app.act(row(app, 2));
            }),
            tap(2),
        ),
        (
            "the tab in front",
            Box::new(|app| {
                app.act(row(app, 2));
            }),
            tap(2),
        ),
        (
            "the palette's tab row",
            Box::new(|app| {
                app.act(UiAction::PaletteRun { id: "tab:3".into() });
            }),
            tap(3),
        ),
        (
            "a cold tab",
            Box::new(|app| {
                app.act(row(app, 1));
            }),
            tap(1),
        ),
        (
            "Clear Menu",
            Box::new(|app| {
                app.act(UiAction::Menu { id: "recent.clear".into() });
            }),
            r#"{"cmd":"recent_clear"}"#.into(),
        ),
    ];
    for (what, person, line) in steps {
        if what == "a cold tab" {
            cold(&twins.person, 1);
            cold(&twins.agent, 1);
            drop((twins.person.heard_events(), twins.agent.heard_events()));
        }
        let before = (
            twins.person.reader().shown_tab,
            twins.person.sidebar(),
            twins.person.core().recent_files().len(),
        );
        let (said, agent_said) = twins.both_with(person, &line);
        assert_eq!(said, agent_said, "{what} and {line}: the same said");
        assert_ne!(
            (
                twins.person.reader().shown_tab,
                twins.person.sidebar(),
                twins.person.core().recent_files().len()
            ),
            before,
            "{what}: something changed"
        );
        twins.assert_same(&format!("{what} and {line}"), &[]);
    }
    assert_eq!(twins.person.reader().shown_tab, 1, "the cold tab's in front");
}

/// Failure (R2.3, C5c): with a tap of a cold tab waiting for its read, a tap of another tab is taken, and then the first read lands and takes the window back
/// to the first, so the tab the person last asked for isn't the one in front; or the second tap waits for the first's read. The later tap wins, at once.
#[test]
fn the_tab_last_tapped_wins_over_one_still_being_read() {
    let app = Disk::new().start();
    let files: Vec<_> = (0..3).map(|at| app.disk().pdf(&format!("f{at}.pdf"), 3)).collect();
    for file in &files {
        app.open(file);
    }
    cold(&app, 1);
    let inner = app.core();
    inner.life.qa_hold_reads.store(true, Ordering::SeqCst);
    app.act(row(&app, 1));
    app.wait_until("the cold tab's read to be under way", || inner.life.reads_running.load(Ordering::SeqCst) > 0);
    app.act(row(&app, 2));
    app.wait_until("the later tap taken", || app.reader().shown_tab == 2);
    inner.life.qa_hold_reads.store(false, Ordering::SeqCst);
    app.settle();
    assert_eq!(app.reader().shown_tab, 2, "the tab last tapped is in front");
    app.hear_events();
    app.act(row(&app, 1));
    app.settle();
    assert_eq!(app.reader().shown_tab, 1, "and the first comes when it's tapped again");
    assert!(names(&app.heard_events()).contains(&"tab_activated"));
}
