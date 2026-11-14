//! **What an agent watching hears while the person reads a document they opened with its password** (SPEC: nothing read from such a file goes to an
//! agent, not its length, place, or find hits): heard over the app's socket, by every way an event goes to watchers (the session thread's announcements,
//! the page views' draws, the Finder's news), as `recto watch` hears it.

use super::files::locked_saying;
use crate::core::controls::UiAction;
use crate::testkit::headless::{Disk, HeadlessApp, applied_as_the_host, host_turns};
use crate::testkit::watching::Watching;
use recto_agent::Level;
use serde_json::Value;
use std::path::Path;

const PASSWORD: &str = "open sesame";

/// A small window at one pixel a point, as the kit's drawing tests use: what's decided here doesn't depend on a picture's size.
fn window_view() -> recto_reader::Viewport {
    recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 292.0,
        height: 225.0,
        frame_width: 292.0,
        magnification: 1.0,
        backing: 1.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

/// A protected file of `pages` pages, each saying "`word` page N".
fn protected(disk: &Disk, pages: usize, word: &str) -> std::path::PathBuf {
    disk.file("protected.pdf", &locked_saying(PASSWORD, pages, word, lopdf::encryption::Permissions::all()))
}

/// The person opens `file` and types its password into the sheet, settled.
fn person_unlocks(app: &HeadlessApp, file: &Path) {
    app.open(file);
    let prompt = app.reader().password_prompt.expect("a protected file asks for its password");
    app.core()
        .tools
        .prompts()
        .password_answered(prompt.id, Some(PASSWORD.to_owned()), &app.core().core, &app.core().window);
    app.settle();
    assert!(app.reader().password_prompt.is_none(), "the password opened it");
}

/// **The person reads the protected document showing:** it's drawn in their window, they go to page 2 and to page 4 by typing it, scroll, and Find
/// "Locked" and go to its first hit, each step settled with its drawing done. Find is left open.
fn person_reads(app: &HeadlessApp) {
    let engine = app.core().engine.clone();
    let mut view = window_view();
    applied_as_the_host(&engine.set_viewport(view));
    host_turns(app, &mut view, true);
    for page in ["2", "4"] {
        app.act(UiAction::PageTyped { text: page.into() });
        host_turns(app, &mut view, true);
    }
    view.y = (view.y - 100.0).max(0.0);
    applied_as_the_host(&engine.set_viewport(view));
    host_turns(app, &mut view, true);
    app.act(UiAction::FindTyped { text: "Locked".into() });
    app.settle();
    app.act(UiAction::FindReturn { forward: true });
    host_turns(app, &mut view, true);
}

/// What `heard` says of the tab `tab` or of Find: every line that names the tab, but its being opened, shown or closed and whether it's edited, which
/// `tabs` says of a protected document too, and every line of Find.
fn told_of(heard: &[Value], tab: u64) -> Vec<Value> {
    heard
        .iter()
        .filter(|event| {
            let name = event["event"].as_str().unwrap_or_default();
            let about_tab = event["tab"].as_u64() == Some(tab) && !matches!(name, "tab_opened" | "tab_activated" | "tab_closed" | "edits_changed");
            about_tab || name.starts_with("find_")
        })
        .cloned()
        .collect()
}

/// Failure (independent review of phase R, S1): while the person reads a document they opened with its password, an agent watching is told which pages
/// they're drawing (`page_drawn`), which page they're on, and how many times Find found what they typed (`find_finished`, `find_changed`), and at
/// Control `session show` gives what they typed: together, whether a word is in the document and how often. An agent watching at Read-only, then at
/// Control, hears nothing of the document beyond its tab, and `session show` gives nothing of what was typed in Find.
#[test]
fn a_watcher_hears_nothing_of_a_protected_documents_place_or_find() {
    let disk = Disk::new();
    let file = protected(&disk, 5, "Locked");
    let app = disk.start();
    let mut watching: Watching = app.agent_watches(Level::ReadOnly);
    person_unlocks(&app, &file);
    let tab = app.recto(Level::ReadOnly, &["tabs"]).expect("tabs")["tabs"][0]["tab"]
        .as_u64()
        .expect("the tab");
    for level in [Level::ReadOnly, Level::Control] {
        watching.set_level(level);
        person_reads(&app);
        let heard = watching.heard();
        let told = told_of(&heard, tab);
        assert!(
            told.is_empty(),
            "at {level:?} the watcher was told of the protected document: {told:#?}\n(all it heard: {heard:#?})"
        );
        let shown = app.recto(level, &["session", "show"]).expect("session show");
        assert!(
            !shown["find"].to_string().contains("Locked"),
            "at {level:?} `session show` gave what the person typed in Find over a protected document: {}",
            shown["find"]
        );
        app.act(UiAction::FindClose);
        app.settle();
    }
}

/// **Everything an agent is told while the person reads a protected file of `pages` pages saying `word`:** a watch on every group, at Read-only, then
/// Control, then Full, each round the person reading and closing Find; at each level what `tabs`, `session show` and `sidebar` say; the person deleting
/// page 4, which takes only in a document that has one, and what's heard and said then; and the person closing the tab at the end.
fn agent_told(pages: usize, word: &str) -> Vec<String> {
    let disk = Disk::new();
    let file = protected(&disk, pages, word);
    let app = disk.start();
    let mut watching = app.agent_watches(Level::ReadOnly);
    person_unlocks(&app, &file);
    let mut told = Vec::new();
    for level in [Level::ReadOnly, Level::Control, Level::Full] {
        watching.set_level(level);
        person_reads(&app);
        app.act(UiAction::FindClose);
        app.settle();
        told.push(format!("{level:?} heard: {:?}", watching.heard()));
        for look in [&["tabs"][..], &["session", "show"], &["sidebar"]] {
            told.push(format!("{level:?} {look:?}: {:?}", app.recto(level, look)));
        }
    }
    // An edit whose taking depends on the document's length.
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"4"}"##);
    app.settle();
    told.push(format!("page 4 deleted: {:?}", watching.heard()));
    for look in [&["tabs"][..], &["session", "show"], &["sidebar"]] {
        told.push(format!("page 4 deleted {look:?}: {:?}", app.recto(Level::Full, look)));
    }
    // Closing asks about the edit where it took, and the person says not to save it: the tab goes either way.
    let tab = app.tabs()[0].tab;
    if app.act(UiAction::Menu { id: "file.close".into() }).host == crate::menus::HostAction::ConfirmClose {
        app.act(UiAction::CloseAnswered {
            tab,
            choice: crate::saving::CloseChoice::DontSave,
        });
    }
    app.settle();
    assert!(app.tabs().is_empty(), "the tab closed");
    told.push(format!("closed: {:?}", watching.heard()));
    told
}

/// Failure (independent review of phase R, S1; the walk of events beside the commands' walk in `recto-agent`'s `policy_guards`): something of a document
/// the person opened with its password reaches an agent that sends nothing about it, by an event (a page drawn, the place, Find's hits, its news while
/// it runs) or a look at the session as a whole: an agent watching every group at every level, and looking with `tabs`, `session show` and `sidebar`,
/// is told the same while the person opens, draws, turns to page 2 and 4, scrolls, Finds, deletes page 4 and closes a three-page protected document saying
/// "Locked" as a five-page one saying "Hidden", where page 4 is past the end of one, so the delete takes in only the other (its `edited`, its undo, its
/// `edits_changed` would tell), and Find finds three hits in one and none in the other.
#[test]
fn an_agent_is_told_the_same_of_two_protected_documents() {
    let (one, two) = (agent_told(3, "Locked"), agent_told(5, "Hidden"));
    let differs: Vec<String> = one.iter().zip(&two).filter(|(a, b)| a != b).map(|(a, b)| format!("  {a}\n  {b}")).collect();
    assert!(
        differs.is_empty(),
        "the agent was told something of the protected document:\n{}",
        differs.join("\n")
    );
    assert_eq!(one.len(), two.len());
}
