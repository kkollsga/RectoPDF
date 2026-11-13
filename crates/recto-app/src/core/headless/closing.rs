//! **Closing a tab, as the person does it** (step R2.3, C5c): File > Close is `close`, the close prompt's Don't Save `discard` and `close`, and its Save the
//! window's Save, as the person's, with `close` after it.

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::queue::{CoreCall, PersonCommand};
use crate::saving::CloseChoice;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{Event, Level};

/// The tab in front, with an edit of the person's on it, made the same way on both apps.
fn edited(twins: &Twins) -> u32 {
    let tab = twins.person.reader().shown_tab;
    let rotate = format!(r##"{{"cmd":"page_rotate","tab":"#{tab}","pages":"all","degrees":90}}"##);
    for app in [&twins.person, &twins.agent] {
        app.person(&rotate);
        app.settle();
        assert!(app.reader().edited);
        drop(app.heard_events());
    }
    tab
}

/// The agent's lines at Full, one after another, each settled: what each said and what the session thread announced after.
fn agent_runs(app: &HeadlessApp, lines: &[String]) -> Vec<Event> {
    let mut said = Vec::new();
    for line in lines {
        let (result, events) = app.agent_heard(Level::Full, line);
        if let Err(error) = result {
            panic!("the agent's `{line}` was refused: {error:?}");
        }
        said.extend(events);
        app.settle();
        said.extend(app.heard_events());
    }
    said
}

/// Failure (R2.3, C5c): File > Close, the close prompt's Don't Save or its Save does something other than the commands an agent sends for them: another tab
/// left in front, edits kept or lost, another event, something else kept, or a Save that runs as an agent's. Each on one app, and its lines at Full on another
/// with the same files: `close`; `discard` and `close`; `save` and `close`.
#[test]
fn the_windows_closes_are_the_agents_commands() {
    let twins = Twins::on(&[("a.pdf", 3), ("b.pdf", 3), ("c.pdf", 3), ("d.pdf", 3)]);
    let close = |tab: u32| format!(r##"{{"cmd":"close","tab":"#{tab}"}}"##);

    let tab = twins.person.reader().shown_tab;
    let (said, agent_said) = twins.both(UiAction::Menu { id: "file.close".into() }, &close(tab));
    assert_eq!(said, agent_said, "File > Close: the same said");
    assert!(names(&said).contains(&"tab_closed"), "{said:?}");
    twins.assert_same("File > Close", &[]);

    let tab = edited(&twins);
    twins.person.act(UiAction::CloseAnswered {
        tab,
        choice: CloseChoice::DontSave,
    });
    twins.person.settle();
    let said = twins.person.heard_events();
    let agent_said = agent_runs(&twins.agent, &[format!(r##"{{"cmd":"discard","tab":"#{tab}"}}"##), close(tab)]);
    assert_eq!(said, agent_said, "Don't Save: the same said");
    assert_eq!(twins.person.tabs().len(), 2, "closed");
    twins.assert_same("Don't Save", &[]);

    let tab = edited(&twins);
    let file = twins
        .person
        .core()
        .host
        .session
        .with_session(|session| session.path_of(recto_session::TabId(tab)))
        .expect("its file");
    let before = std::fs::read(&file).expect("the file");
    twins.person.act(UiAction::CloseAnswered {
        tab,
        choice: CloseChoice::Save,
    });
    twins.person.wait_until("saved and closed", || twins.person.tabs().len() == 1);
    twins.person.settle();
    let said = twins.person.heard_events();
    let agent_said = agent_runs(&twins.agent, &[format!(r##"{{"cmd":"save","tab":"#{tab}"}}"##), close(tab)]);
    assert_eq!(said, agent_said, "Save: the same said");
    assert!(names(&said).contains(&"saved"), "{said:?}");
    assert_ne!(std::fs::read(&file).expect("the file"), before, "saved into the file");
    twins.assert_same("Save", &[]);
}

/// Failure (R2.3, C5c): the close after the prompt's Save is refused, as an edit came in after the save, and the person isn't told the tab stayed, or is told
/// in an agent's words; or a File > Close refused the same way says the prompt's words, which are about a save that never happened. Only the close that
/// finishes a Save says so.
#[test]
fn only_the_close_after_a_save_says_the_tab_stayed() {
    let disk = Disk::new();
    let file = disk.pdf("a.pdf", 3);
    let app = disk.start();
    app.open(&file);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"all","degrees":90}"##);
    app.settle();
    app.person(r##"{"cmd":"close","tab":"#1"}"##);
    app.settle();
    assert_eq!(app.tabs().len(), 1, "it stays, with its edit");
    assert_eq!(crate::lock(&app.core().window.action_notice).clone(), None, "a plain close says nothing");
    app.send(CoreCall::Run(Box::new(PersonCommand::finishing(
        recto_agent::parse(r##"{"cmd":"close","tab":"#1"}"##).expect("a command"),
        None,
    ))));
    app.settle();
    assert_eq!(app.tabs().len(), 1, "it stays");
    assert_eq!(
        crate::lock(&app.core().window.action_notice).clone().as_deref(),
        Some(crate::saving::SAVED_NOT_CLOSED),
        "the close after a save says so, in the prompt's words"
    );
}

/// Failure (found by `menu_twins`, 2026-11-13): the test kit's settle doesn't wait for the window's Save, which runs on a thread of its own, so a test that
/// settles after File > Save and asks what was announced hears nothing, or hears it in the next step's events. Settled, File > Save has been heard.
#[test]
fn file_save_is_heard_once_settled() {
    let twins = Twins::on(&[("a.pdf", 3)]);
    edited(&twins);
    twins.person.act(UiAction::Menu { id: "file.save".into() });
    twins.person.settle();
    let said = twins.person.heard_events();
    assert_eq!(names(&said), ["saved", "edits_changed"], "{said:?}");
}

/// Failure (review of R2.3, S1, and the coordinator's ruling that unknown is "would lose"): an agent's `close` of a tab whose edit list is asleep, with only
/// the owner's undone steps in it and its journal unreadable just now, was refused with the guidance for a tab with nothing of the owner's, and would have
/// gone ahead at Full. With what it would lose not all known, an agent is refused `edits_unreadable` at Control and at Full, in plain words; the person's
/// File > Close asks first, from the list's summary, as for the owner's undone steps.
#[test]
fn an_agents_close_of_an_asleep_tab_says_what_it_would_lose() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 3), disk.pdf("b.pdf", 3));
    let app = disk.start();
    app.open(&a);
    app.open(&b);
    for line in [
        r##"{"cmd":"tap","tab":"#1"}"##,
        r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##,
        r##"{"cmd":"undo","tab":"#1"}"##,
        r##"{"cmd":"tap","tab":"#2"}"##,
    ] {
        app.person(line);
        app.settle();
    }
    let slept = app.agent(Level::Full, r##"{"cmd":"session_sleep","tab":"#1"}"##).expect("put to sleep");
    assert_eq!(slept["list_asleep"], true, "{slept}");
    // Its journal can't be read back just now, so the list stays asleep through the command, with only its summary to go by.
    let journals = app.core().saver.store.main_folder().join("journals");
    for entry in std::fs::read_dir(&journals).expect("the journals").flatten() {
        if entry.file_name().to_string_lossy().starts_with("journal-") {
            std::fs::write(entry.path(), b"not a journal").expect("written over");
        }
    }
    for level in [Level::Control, Level::Full] {
        let error = app
            .agent(level, r##"{"cmd":"close","tab":"#1"}"##)
            .expect_err("not all of what it would lose is known");
        assert_eq!(error.code, "edits_unreadable", "{level:?}: {error:?}");
        assert!(error.message.contains("couldn't be read back"), "{error:?}");
    }
    assert_eq!(app.tabs().len(), 2, "it stays");
    assert!(
        app.core().host.session.with_session(|session| session.list_is_asleep(recto_session::TabId(1))),
        "the list was asleep when the close was asked"
    );
    app.person(r##"{"cmd":"tap","tab":"#1"}"##);
    app.settle();
    let acted = app.act(UiAction::Menu { id: "file.close".into() });
    assert_eq!(acted.host, crate::menus::HostAction::ConfirmClose, "the person is asked first");
}
