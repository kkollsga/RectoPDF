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
    app.core().host.session.with_session(|session| session.dormancy_mut().set_sleep_files(false));
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

/// Failure (R2.3, by A2's rule): opening a file that's open already is announced other than by whether it changed what's in front: the window's open said
/// nothing when it brought the tab to the front, and an agent's `open` said `tab_activated` when that tab was in front already. For the person and an agent,
/// a tab brought to the front is announced, and one that was there already isn't.
#[test]
fn opening_a_file_thats_open_announces_its_tab_only_when_it_comes_to_the_front() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 3), disk.pdf("b.pdf", 3));
    let app = disk.start();
    app.open(&a);
    app.open(&b);
    app.hear_events();
    let person = |path: &std::path::Path| {
        app.act(UiAction::Open {
            path: path.display().to_string(),
        });
        app.settle();
        names(&app.heard_events())
    };
    let agent = |path: &std::path::Path| {
        let (result, mut said) = app.agent_heard(Level::Full, &format!(r#"{{"cmd":"open","path":{:?}}}"#, path.display().to_string()));
        result.expect("opened");
        app.settle();
        said.extend(app.heard_events());
        names(&said)
    };
    assert_eq!(person(&a), ["tab_activated"], "the person's open brought a.pdf to the front");
    assert!(person(&a).is_empty(), "and again, with it in front already");
    assert_eq!(agent(&b), ["tab_activated"], "an agent's open brought b.pdf to the front");
    assert!(agent(&b).is_empty(), "and again, with it in front already");
}

/// Failure (R2.3, C5c): opening a file as the window does (the Open panel, Open Recent, a file named at launch) does something other than `open`: another tab
/// made or brought forward, another event, or something else kept; or a file that doesn't open says nothing, or says it in an agent's words. Each on one app,
/// and `open` at Full on the other: a file that isn't open, one that is, Open Recent's, and one named at launch.
#[test]
fn the_windows_opens_are_the_agents_open() {
    let twins = Twins::on(&[("a.pdf", 3), ("b.pdf", 3)]);
    for app in [&twins.person, &twins.agent] {
        app.disk().pdf("c.pdf", 4);
        app.disk().pdf("d.pdf", 5);
    }
    let open = |name: &str| {
        let path = twins.agent.disk().files().join(name);
        format!(r#"{{"cmd":"open","path":{:?}}}"#, path.display().to_string())
    };
    let steps: Vec<Step> = vec![
        (
            "the Open panel's new file",
            Box::new(|app| {
                app.act(UiAction::Open {
                    path: app.disk().files().join("c.pdf").display().to_string(),
                });
            }),
            open("c.pdf"),
        ),
        (
            "a file that's open",
            Box::new(|app| {
                app.act(UiAction::Open {
                    path: app.disk().files().join("a.pdf").display().to_string(),
                });
            }),
            open("a.pdf"),
        ),
        (
            "Open Recent",
            Box::new(|app| {
                let key = app
                    .core()
                    .recent_files()
                    .into_iter()
                    .find(|file| file.path.ends_with("b.pdf"))
                    .expect("b.pdf's in the list")
                    .key;
                app.act(UiAction::OpenRecent { key });
            }),
            open("b.pdf"),
        ),
        (
            "a file named at launch",
            Box::new(|app| app.core().ready(&[app.disk().files().join("d.pdf")])),
            open("d.pdf"),
        ),
    ];
    for (what, person, line) in steps {
        let (said, agent_said) = twins.both_with(person, &line);
        assert_eq!(said, agent_said, "{what} and {line}: the same said");
        assert!(!said.is_empty(), "{what}: something said");
        twins.assert_same(&format!("{what} and {line}"), &[]);
    }
    assert_eq!(twins.person.tabs().len(), 4);
    // A file that isn't a PDF says so by the window, in the session's words.
    let not_pdf = twins.person.disk().file("notes.pdf", b"not a pdf at all");
    twins.person.act(UiAction::Open {
        path: not_pdf.display().to_string(),
    });
    twins.person.settle();
    assert_eq!(
        crate::lock(&twins.person.core().window.open_failure).clone().as_deref(),
        Some(recto_core::OpenError::NotPdf.message()),
        "the person's words"
    );
}

/// Failure (R2.3, C5c): a file opened before the state's been read, as a launch by a double-click in the Finder asks, is opened at once on an empty session,
/// and the state coming in after puts what was left over it, so the file asked for isn't open; or it waits for good. It's held until the state's in, then
/// opened beside what came back.
#[test]
fn a_file_opened_before_the_state_is_in_opens_once_it_is() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 3), disk.pdf("b.pdf", 3));
    let app = disk.start();
    app.open(&a);
    let (_, disk) = app.quit();
    let app = disk.builder().hold(&[recto_platform::slow::Seam::State]).start();
    app.act(UiAction::Open { path: b.display().to_string() });
    assert!(app.core().core.flush(crate::testkit::headless::WAIT_LIMIT), "taken, and any read for it done");
    assert!(!app.core().life.state_read(), "the state's still held");
    assert_eq!(
        app.core().host.session.with_session(|session| session.tabs().len()),
        0,
        "nothing's opened before the state's in"
    );
    app.let_state_in();
    app.settle();
    let names: Vec<String> = app.tabs().into_iter().map(|row| row.text).collect();
    assert_eq!(names.len(), 2, "what came back, and the file asked for: {names:?}");
    assert_eq!(app.reader().shown_tab, app.tabs()[1].tab, "the file asked for is in front");
}
