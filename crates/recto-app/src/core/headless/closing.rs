//! **Closing a tab, as the person does it** (step R2.3, C5c): File > Close is `close`, the close prompt's Don't Save `discard` and `close`, and its Save the
//! window's Save, as the person's, with `close` after it.

use super::twins::{Twins, names};
use crate::core::controls::UiAction;
use crate::queue::{CoreCall, Origin, PersonCommand};
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
    app.send(CoreCall::Run(Box::new(PersonCommand {
        command: recto_agent::parse(r##"{"cmd":"close","tab":"#1"}"##).expect("a command"),
        origin: Origin::Finishing,
        fetched: None,
    })));
    app.settle();
    assert_eq!(app.tabs().len(), 1, "it stays");
    assert_eq!(
        crate::lock(&app.core().window.action_notice).clone().as_deref(),
        Some(crate::saving::SAVED_NOT_CLOSED),
        "the close after a save says so, in the prompt's words"
    );
}
