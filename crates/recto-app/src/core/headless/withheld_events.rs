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
pub(super) fn protected(disk: &Disk, pages: usize, word: &str) -> std::path::PathBuf {
    disk.file("protected.pdf", &locked_saying(PASSWORD, pages, word, lopdf::encryption::Permissions::all()))
}

/// The person opens `file` and types its password into the sheet, settled.
pub(super) fn person_unlocks(app: &HeadlessApp, file: &Path) {
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
    // The person prints all of it (PLAN, "the person can't print a document they opened with its password"): what an agent hears and sees while it's
    // being prepared (held after its first page, so its pages are 1 of 3 and 1 of 5) and once its file is the host's and let go.
    let (held, go) = super::printing::hold_at(1);
    super::printing::answer(&app, super::printing::ALL);
    held.recv_timeout(crate::testkit::headless::WAIT_LIMIT)
        .expect("the print held after its first page");
    app.wait_until("the window says it's preparing", || app.core().tools.printing().notice().is_some());
    told.push(format!("printing heard: {:?}", watching.heard()));
    agent_looks(&app, "printing", &mut told);
    assert!(
        told.last().is_some_and(|line| line.contains("\"printing\"")),
        "`session check` says a print is under way: {told:?}"
    );
    drop(go);
    app.settle();
    told.push(format!("printed heard: {:?}", watching.heard()));
    agent_looks(&app, "printed, not yet taken", &mut told);
    assert_ne!(super::printing::printed(&app), "none", "the person's print printed");
    app.settle();
    told.push(format!("printed and let go heard: {:?}", watching.heard()));
    // An edit whose taking depends on the document's length.
    app.person(r##"{"cmd":"page_delete","tab":"#1","pages":"4"}"##);
    app.settle();
    told.push(format!("page 4 deleted: {:?}", watching.heard()));
    for look in [&["tabs"][..], &["session", "show"], &["sidebar"]] {
        told.push(format!("page 4 deleted {look:?}: {:?}", app.recto(Level::Full, look)));
    }
    // The person goes to its last page, and closes the tab while that page's draw is done and its word not yet told, so the word comes after the close.
    // Closing asks about the edit where it took, and the person says not to save it: the tab goes either way.
    let engine = app.core().engine.clone();
    let mut view = window_view();
    app.act(UiAction::PageTyped { text: "5".into() });
    host_turns(&app, &mut view, true);
    // A wider window: the page showing is drawn again, at its new size.
    engine.qa().hold_tells(true);
    view.width *= 2.0;
    view.frame_width *= 2.0;
    applied_as_the_host(&engine.set_viewport(view));
    assert!(engine.qa().wait_for_held_tell(crate::testkit::headless::WAIT_LIMIT), "a draw's word held");
    let tab = app.tabs()[0].tab;
    if app.act(UiAction::Menu { id: "file.close".into() }).host == crate::menus::HostAction::ConfirmClose {
        app.act(UiAction::CloseAnswered {
            tab,
            choice: crate::saving::CloseChoice::DontSave,
        });
    }
    app.wait_until("the tab closed", || app.tabs().is_empty());
    engine.qa().hold_tells(false);
    app.settle();
    told.push(format!("closed: {:?}", watching.heard()));
    // Anything told after that mark (a Finder's or a helper's late word) is read too.
    app.settle();
    told.push(format!("after: {:?}", watching.heard()));
    told
}

/// What an agent at Full is told by a look at the session as a whole while the person prints: `tabs`, `session show`, `sidebar`, what `status` says of
/// the session (its tab count: the rest is the server's own), and of `session check` the busy words alone (`busy: printing` for as long as a print takes
/// is accepted: `docs/engineering/opening.md`). None of it names the print's descriptor (`/dev/fd/N`), the app's own number given only to the window.
fn agent_looks(app: &HeadlessApp, when: &str, told: &mut Vec<String>) {
    for look in [&["tabs"][..], &["session", "show"], &["sidebar"]] {
        told.push(format!("{when} {look:?}: {:?}", app.recto(Level::Full, look)));
    }
    told.push(format!("{when} status tabs: {}", recto_control::host::Host::tab_count(&*app.core().host)));
    let check = app.recto(Level::Full, &["session", "check"]).expect("session check");
    let busy: Vec<&Value> = check["busy"].as_array().into_iter().flatten().map(|item| &item["code"]).collect();
    told.push(format!("{when} session check busy: {busy:?}"));
    assert!(
        told.iter().all(|line| !line.contains("/dev/fd")),
        "an agent was told the print's descriptor: {told:#?}"
    );
}

/// Failure (independent review of phase R, S1; the walk of events beside the commands' walk in `recto-agent`'s `policy_guards`): something of a document
/// the person opened with its password reaches an agent that sends nothing about it, by an event (a page drawn, the place, Find's hits, its news while
/// it runs) or a look at the session as a whole: an agent watching every group at every level, and looking with `tabs`, `session show` and `sidebar`,
/// is told the same while the person opens, draws, turns to page 2 and 4, scrolls, Finds, prints all of it (while it's prepared, once it's ready, once
/// it's let go), deletes page 4 and closes a three-page protected document saying
/// "Locked" as a five-page one saying "Hidden", where page 4 is past the end of one, so the delete takes in only the other (its `edited`, its undo, its
/// `edits_changed` would tell), Find finds three hits in one and none in the other, and the close comes while the draw of the last page (3 in one, 4 in
/// the other) is done and its word not yet told, so a `page_drawn` about a tab that's gone would tell.
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

/// Failure (security review of S1): the helpers' news about a protected document's file goes to an agent: the person asks for generated contents for the
/// protected tab they're reading, the helper is set going on its file (it stops at `needs_password` before anything's decrypted, but how long that took,
/// how much it used and the file's key go out as `contents_generating` and `contents_generated`). It can run: the person's `contents show` reaches the
/// generator for a protected tab. An agent watching hears none of it; the person's sidebar hears it as ever.
#[test]
fn the_helpers_news_of_a_protected_file_doesnt_reach_a_watcher() {
    let disk = Disk::new();
    let file = protected(&disk, 3, "Locked");
    let app = disk.builder().indexing(true).start();
    let mut watching = app.agent_watches(Level::ReadOnly);
    person_unlocks(&app, &file);
    drop(watching.heard());
    app.person(r##"{"cmd":"contents_show","which":"generated","tab":"#1"}"##);
    // Its answer is in when the tab's generated contents stand somewhere other than being made: the helper's last word is said just before.
    let id = recto_session::TabId(app.tabs()[0].tab);
    app.wait_until("the helper's answer", || {
        app.core().host.session.with_session(|session| {
            !matches!(
                session.generated().view(id),
                None | Some(recto_session::GeneratedView::Making | recto_session::GeneratedView::NotKept)
            )
        })
    });
    app.settle();
    let heard = watching.heard();
    assert!(
        !heard.iter().any(|event| event["event"]
            .as_str()
            .is_some_and(|name| name.starts_with("contents_") || name.starts_with("indexing_"))),
        "the watcher heard the helpers' news of the protected file: {heard:?}"
    );
}
