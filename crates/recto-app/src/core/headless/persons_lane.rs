//! **The person's edits take effect in the order they asked for them** (the ⌃⌘H-then-⌘Z fault, 2026-11-14; `crate::queue::lane`): a highlight held while
//! its pages are read, with the person's next edits asked meanwhile, then let go; what each order comes to, a save, a close, the cap, a quit, and an agent
//! beside it all.

use crate::core::controls::UiAction;
use crate::core::highlight::{HIGHLIGHT_ID, REMOVE_ID};
use crate::core::person::hold;
use crate::menus::HostAction;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};
use recto_agent::Level;
use serde_json::Value;

fn menu(app: &HeadlessApp, id: &str) -> HostAction {
    app.act(UiAction::Menu { id: id.into() }).host
}

fn selects(app: &HeadlessApp, from: (u32, u32), to: (u32, u32)) {
    app.core().panes.reading_engine(&app.core().engine).selection().restore(from, to);
}

/// The session thread through what's queued now: what wasn't parked has run.
fn flush(app: &HeadlessApp) {
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took what was queued");
}

/// The highlights made as edits on the tab showing, and those taken out.
fn highlights(app: &HeadlessApp) -> (usize, usize) {
    let listed: Value = app.agent(Level::ReadOnly, r#"{"cmd":"highlights","tab":"active"}"#).expect("highlights");
    let count = |key: &str| listed[key].as_array().map_or(0, Vec::len);
    (count("edits"), count("removed"))
}

/// Whether Undo and Redo can be chosen now, as the menu says, once the session thread has published what's been done.
fn undo_redo(app: &HeadlessApp) -> (bool, bool) {
    flush(app);
    (
        app.core().menu_item_enabled("edit.undo".into()),
        app.core().menu_item_enabled("edit.redo".into()),
    )
}

/// One file of two pages open, turned once by the person, so there's a step before whatever the test does.
fn with_a_step_before() -> HeadlessApp {
    let disk = Disk::new();
    let doc = disk.pdf("a.pdf", 2);
    let app = disk.start();
    app.open(&doc);
    menu(&app, "rotate.right");
    app.settle();
    assert_eq!(undo_redo(&app), (true, false), "the turn, a step to undo");
    app
}

/// **The person's Highlight, held while its pages are read,** with `then` done meanwhile, then let go and settled.
fn while_a_highlight_is_read(app: &HeadlessApp, id: &str, tag: &'static str, then: impl FnOnce()) {
    selects(app, (0, 0), (0, 5));
    let (held, go) = hold(tag);
    menu(app, id);
    held.recv_timeout(WAIT_LIMIT).expect("the highlight held as its pages are read");
    then();
    drop(go);
    app.settle();
}

/// **Failure (release blocker, 2026-11-14): ⌃⌘H then a quick ⌘Z undoes the step before the highlight,** and the highlight then lands: the person asked to
/// highlight and undo it, and got the turn undone and the highlight kept. The ⌘Z waits for the highlight, then undoes it, and the turn stands.
#[test]
fn a_quick_undo_after_a_highlight_undoes_the_highlight() {
    let app = with_a_step_before();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.undo");
        flush(&app);
        assert!(
            app.core().life.persons_running() >= 2,
            "the highlight in flight, the undo waiting its turn, both counted"
        );
    });
    assert_eq!(highlights(&app), (0, 0), "the highlight undone");
    assert_eq!(undo_redo(&app), (true, true), "the turn still there to undo, and the highlight to redo");
}

/// Failure: the same fault for Remove Highlight: ⌘Z after it undoes the step before, and the removal lands. The removal is undone: the highlight's back.
#[test]
fn a_quick_undo_after_a_removal_undoes_the_removal() {
    let app = with_a_step_before();
    selects(&app, (0, 0), (0, 5));
    menu(&app, HIGHLIGHT_ID);
    app.settle();
    assert_eq!(highlights(&app), (1, 0));
    while_a_highlight_is_read(&app, REMOVE_ID, "highlight_remove", || {
        menu(&app, "edit.undo");
    });
    assert_eq!(highlights(&app), (1, 0), "the removal undone: the highlight's back");
    assert_eq!(undo_redo(&app), (true, true), "the highlight and the turn to undo, the removal to redo");
}

/// Failure: the person's edits asked while a highlight is read run in another order than asked: Undo then Redo puts the highlight back, Redo then Undo
/// takes it off (the highlight cleared what there was to redo, so the Redo has nothing), and two highlights then two Undos leave neither and the turn.
#[test]
fn every_order_asked_is_the_order_done() {
    let app = with_a_step_before();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.undo");
        menu(&app, "edit.redo");
    });
    assert_eq!(highlights(&app), (1, 0), "highlight, undo, redo: it's there");
    assert_eq!(undo_redo(&app), (true, false));

    let app = with_a_step_before();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.redo");
        menu(&app, "edit.undo");
    });
    assert_eq!(highlights(&app), (0, 0), "highlight, redo (nothing), undo: it's gone");
    assert_eq!(undo_redo(&app), (true, true), "the turn stands");

    let app = with_a_step_before();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        selects(&app, (1, 0), (1, 5));
        menu(&app, HIGHLIGHT_ID);
        menu(&app, "edit.undo");
        menu(&app, "edit.undo");
    });
    assert_eq!(highlights(&app), (0, 0), "two highlights, two undos: neither");
    assert_eq!(undo_redo(&app), (true, true), "the turn stands");
    app.agent(Level::Full, r#"{"cmd":"undo"}"#).expect("the turn undone");
    assert!(!undo_redo(&app).0, "nothing before the turn: the two undos took exactly the two highlights");
}

/// Failure: a highlight that's refused (no text where it was asked) leaves the person's Undo waiting for good, or drops it. The Undo runs when the refusal's
/// in and undoes the last thing that happened, the turn, as the highlight made no step.
#[test]
fn an_undo_after_a_refused_highlight_undoes_the_step_before() {
    let app = with_a_step_before();
    selects(&app, (0, 0), (0, 0));
    let (held, go) = hold("highlight");
    menu(&app, HIGHLIGHT_ID);
    held.recv_timeout(WAIT_LIMIT).expect("held");
    menu(&app, "edit.undo");
    drop(go);
    app.settle();
    assert_eq!(highlights(&app), (0, 0));
    assert_eq!(undo_redo(&app), (false, true), "the turn undone");
}

/// Failure: a Save asked while a highlight is read writes the file without it, though the person asked for the highlight first. It waits its turn and
/// writes the highlight.
#[test]
fn a_save_asked_during_a_highlight_writes_it() {
    let disk = Disk::new();
    let doc = disk.pdf("saved.pdf", 2);
    let app = disk.start();
    app.open(&doc);
    menu(&app, "rotate.right");
    app.settle();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "file.save");
        flush(&app);
    });
    let read = recto_core::highlights::page_highlights(&doc, 0).expect("the saved file reads");
    assert_eq!(read.found.len(), 1, "the highlight is in the file");
    assert_eq!(highlights(&app), (0, 0), "and no longer an unsaved edit");
}

/// Failure (coordinator, 2026-11-14): a Save holds the lane for its whole write, so an edit in another tab waits seconds for a large file. It passes its
/// turn once it's begun: a turn in another tab runs while the save's write is held.
#[test]
fn a_save_being_written_holds_up_no_edit_in_another_tab() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 2), disk.pdf("b.pdf", 2));
    let app = disk.start();
    app.open(&a);
    menu(&app, "rotate.right");
    app.settle();
    let (held, go) = hold("save_writing");
    menu(&app, "file.save");
    held.recv_timeout(WAIT_LIMIT).expect("the save held as it writes");
    app.act(UiAction::Open { path: b.display().to_string() });
    flush(&app);
    menu(&app, "rotate.right");
    flush(&app);
    assert!(undo_redo(&app).0, "the turn in the other tab is done while the save writes");
    drop(go);
    app.settle();
}

/// Failure: a tab closed by an agent while the person's edits of it wait leaves them waiting for good, or the app unsettled. They run, and are refused as
/// a command about a tab that's gone is.
#[test]
fn edits_waiting_for_a_closed_tab_are_refused_and_nothing_waits() {
    let disk = Disk::new();
    let doc = disk.pdf("a.pdf", 2);
    let app = disk.start();
    app.open(&doc);
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.undo");
        app.agent(Level::Control, r#"{"cmd":"close","tab":"active"}"#)
            .expect("an agent closes the unedited tab");
    });
    assert_eq!(app.core().life.persons_running(), 0, "nothing left waiting");
    assert_eq!(app.core().core.lane().parked(), 0);
}

/// Failure: the person's waiting edits grow without bound, or one past the cap is dropped with nothing said. With the cap at one, the second waiting is
/// refused in Rust's words by the page, and the first runs.
#[test]
fn one_past_the_cap_is_refused_and_said() {
    let app = with_a_step_before();
    app.core().core.lane().set_cap(1);
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.undo");
        flush(&app);
        menu(&app, "edit.undo");
        flush(&app);
        let notice = app.reader().selection_notice.map(|notice| notice.text);
        assert_eq!(notice.as_deref(), Some(crate::core::person_lane::FULL), "said by the page");
    });
    assert_eq!(highlights(&app), (0, 0), "the one waiting ran: the highlight undone");
    assert_eq!(undo_redo(&app), (true, true), "and only it: the turn stands");
}

/// Failure: the lane holds up an agent, or an agent's command waits for the person's: an agent's undo sent while the person's highlight is read runs at
/// once, as it always has, and the highlight lands after it.
#[test]
fn an_agents_command_neither_waits_for_the_persons_nor_holds_them_up() {
    let app = with_a_step_before();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        app.agent(Level::Full, r#"{"cmd":"undo"}"#)
            .expect("the agent's undo, answered while the highlight is held");
        assert_eq!(undo_redo(&app).0, false, "done at once: the turn undone");
    });
    assert_eq!(highlights(&app), (1, 0), "the person's highlight landed");
}

/// Failure (Q4, coordinator, 2026-11-14): ⌃⌘H then ⌘W closes without asking, as the tab isn't edited yet, and the highlight lands on a tab that's gone.
/// The close asks, as ⌘W after the highlight lands does; and ⌘W on another tab, with nothing of its own landing, doesn't.
#[test]
fn a_close_asks_while_an_edit_of_its_tab_is_landing() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 2), disk.pdf("b.pdf", 2));
    let app = disk.start();
    app.open(&b);
    app.open(&a);
    let tab_of = |app: &HeadlessApp| app.reader().shown_tab;
    let first = tab_of(&app);
    selects(&app, (0, 0), (0, 5));
    let (held, go) = hold("highlight");
    menu(&app, HIGHLIGHT_ID);
    held.recv_timeout(WAIT_LIMIT).expect("held");
    assert_eq!(menu(&app, "file.close"), HostAction::ConfirmClose, "asked, its highlight still landing");
    app.act(UiAction::Open { path: b.display().to_string() });
    flush(&app);
    assert_ne!(tab_of(&app), first);
    assert_eq!(menu(&app, "file.close"), HostAction::None, "another tab: closed without asking");
    drop(go);
    app.settle();
}

/// Failure (coordinator, 2026-11-14): a quit with the person's edits waiting drops them as it freezes, or writes the record before they've run. The quit
/// waits for them as for any command running, the waiting Undo runs past the freeze once the highlight lands, and the record has both: at the next launch
/// the highlight is there to redo and the turn to undo.
#[test]
fn a_quit_with_edits_waiting_runs_them_before_it_writes() {
    let app = with_a_step_before();
    let answers = |app: &HeadlessApp| app.heard().iter().filter(|heard| matches!(heard, crate::testkit::Heard::QuitAnswer(_))).count();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.undo");
        flush(&app);
        app.core().quit_requested();
    });
    app.wait_until("the quit's answer", || answers(&app) == 1);
    let disk = app.quit_anyway();
    let app = disk.start();
    app.settle();
    assert_eq!(
        undo_redo(&app),
        (true, true),
        "the undo ran past the freeze, before the write: the highlight to redo, the turn to undo"
    );
    drop(app.quit());
}
