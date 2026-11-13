//! **The agent's commands and the app's actions tell watchers the same thing** (step R2.1). Both build their events with the functions in [`crate::event`] from what the session says changed, so these run the agent's commands and check the events they make against those functions, which is also what `recto-ffi`'s arms call. Each test names the difference the two paths used to have.

use super::*;
use crate::event;
use recto_core::{DocFacts, OpenError, Toc};
use std::path::Path;

#[derive(Clone)]
struct Fake;

impl DocLoader for Fake {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 10,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// A command run as an agent at Full, with the events it made.
fn events_of(session: &mut Session<Fake>, command: Command) -> Vec<Event> {
    let mut events = Vec::new();
    execute(&Direct(RefCell::new(session), None), Level::Full, &command, &mut events).unwrap();
    events
}

fn two_tabs() -> (Session<Fake>, TabId, TabId) {
    let mut session = Session::new(Fake);
    let a = session.open(Path::new("/d/alpha.pdf")).unwrap();
    let b = session.open(Path::new("/d/beta.pdf")).unwrap();
    (session, a, b)
}

/// Failure: the app's opens, taps and closes said which tab without its name while the agent's `open` said it, so a watcher knew a file's name only if an agent opened it. Opening, tapping to another tab and closing all carry the file's name, on the agent's path and in the functions the app calls.
#[test]
fn opened_and_activated_carry_the_name_on_both_paths() {
    let mut session = Session::new(Fake);
    let events = events_of(
        &mut session,
        Command::from(crate::commands::tabs::Open {
            path: "/d/alpha.pdf".into(),
            page: None,
        }),
    );
    let a = session.tabs()[0].id;
    assert_eq!(events, [event::tab_opened(&session, a)]);
    assert_eq!(events[0].to_json()["untrusted"]["name"], "alpha");
    let b = session.open(Path::new("/d/beta.pdf")).unwrap();
    let events = events_of(&mut session, Command::from(crate::commands::tabs::Tap { tab: format!("#{}", a.0) }));
    assert_eq!(events, [event::tab_activated(&session, a)]);
    assert_eq!(events[0].to_json()["untrusted"]["name"], "alpha");
    // The app's tap: the session says what it switched to, and the same function makes the event.
    let mut app = Vec::new();
    let switched = session.tap(b).unwrap();
    event::switched(&session, switched, &mut app);
    assert_eq!(app, [event::tab_activated(&session, b)]);
    assert_eq!(app[0].to_json()["untrusted"]["name"], "beta");
    // A close says it and the tab that took its place, with its name.
    let events = events_of(&mut session, Command::from(crate::commands::tabs::Close { tab: format!("#{}", b.0) }));
    assert_eq!(events, [Event::TabClosed { tab: b }, event::tab_activated(&session, a)]);
}

/// Failure: a comparison's event said its tabs without `sync` when the app made it and with it when an agent did; the app's sync toggle said `sync` without the tabs, and said it even when nothing changed; and ending a comparison said the one tab left rather than none. Now one function makes it: both keys always, the panes empty when it ends, and nothing at all when a toggle changes nothing.
#[test]
fn comparison_events_say_the_tabs_and_the_sync_and_only_when_they_change() {
    let (mut session, a, b) = two_tabs();
    let both = format!("#{},#{}", a.0, b.0);
    let events = events_of(
        &mut session,
        Command::from(crate::commands::compare::Compare {
            tabs: Some(both.clone()),
            sync: None,
            off: false,
        }),
    );
    assert_eq!(
        events,
        [Event::ComparisonChanged {
            panes: vec![a, b],
            synced: false
        }]
    );
    let line = events[0].to_json();
    assert_eq!((line["comparison"].as_array().unwrap().len(), line["sync"].as_str()), (2, Some("off")));
    // The same again, and `sync off` when it is off: nothing changed, nothing said.
    assert!(
        events_of(
            &mut session,
            Command::from(crate::commands::compare::Compare {
                tabs: Some(both),
                sync: None,
                off: false
            })
        )
        .is_empty()
    );
    assert!(
        events_of(
            &mut session,
            Command::from(crate::commands::compare::Compare {
                tabs: None,
                sync: Some("off".into()),
                off: false
            })
        )
        .is_empty()
    );
    assert_eq!(session.set_compare_synced(false), Some(false), "the app's toggle is told the same: unchanged");
    // On: said, with the tabs; and the app's toggle says it from the same function.
    let events = events_of(
        &mut session,
        Command::from(crate::commands::compare::Compare {
            tabs: None,
            sync: Some("on".into()),
            off: false,
        }),
    );
    assert_eq!(
        events,
        [Event::ComparisonChanged {
            panes: vec![a, b],
            synced: true
        }]
    );
    assert_eq!(session.set_compare_synced(true), Some(false));
    assert_eq!(session.set_compare_synced(false), Some(true));
    assert_eq!(
        event::comparison_changed(&session),
        Event::ComparisonChanged {
            panes: vec![a, b],
            synced: false
        }
    );
    // Ended: no panes, whichever path ended it.
    session.toggle_compare(b).unwrap();
    assert_eq!(event::comparison_changed(&session), Event::ComparisonChanged { panes: vec![], synced: false });
    assert_eq!(event::comparison_changed(&session).to_json()["comparison"], json!([]));
}

/// Failure: an agent's `goto` and `open --page` said the page they were asked for, where the app said the page the tab landed on (the session clamps a jump to the document), and for a protected document an agent's said the page where the app said `null`. Both say the page the tab is on, and nothing for a protected document.
#[test]
fn page_changed_is_the_real_page_and_nothing_for_a_protected_document() {
    let (mut session, a, b) = two_tabs();
    let events = events_of(
        &mut session,
        Command::from(crate::commands::linking::Goto {
            tab: format!("#{}", a.0),
            page: Some(10),
            label: None,
        }),
    );
    assert_eq!(events, [Event::PageChanged { tab: a, page: Some(10) }]);
    assert_eq!(event::page_now(&session, a), events[0], "the app's jump says the same");
    // A jump past the end lands on the last page, and that's what's said, not what was asked.
    session.jump(a, 0, 0.0).unwrap();
    session.jump(a, 99, 0.0).unwrap();
    assert_eq!(event::page_now(&session, a), Event::PageChanged { tab: a, page: Some(10) });
    session.note_password_used(b);
    assert!(session.withheld_from_agents(b));
    assert_eq!(event::page_now(&session, b), Event::PageChanged { tab: b, page: None });
    assert_eq!(event::page_now(&session, b).to_json()["page"], Value::Null);
}
