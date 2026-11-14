//! **The person's edits take effect in the order they asked for them** (the ⌃⌘H-then-⌘Z fault, 2026-11-14; `crate::queue::lane`): a highlight held while
//! its pages are read, with the person's next edits asked meanwhile, then let go; what each order comes to, a save, a close, the cap, a quit, and an agent
//! beside it all.

use crate::core::controls::UiAction;
use crate::core::highlight::{HIGHLIGHT_ID, REMOVE_ID};
use crate::menus::HostAction;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};
use recto_agent::Level;
use serde_json::Value;

/// **A menu item chosen, as a person can:** Undo, Redo, Save and Close only while they're enabled, as AppKit drops a key whose item isn't (the fault as
/// filed: ⌃⌘H then a quick ⌘Z met a disabled Undo, and the highlight stayed). The rest are enabled by the page view's document and selection, which this
/// file doesn't test.
fn menu(app: &HeadlessApp, id: &str) -> HostAction {
    if ["edit.undo", "edit.redo", "file.save", "file.close"].contains(&id) {
        assert!(app.core().menu_item_enabled(id.into()), "{id} can be chosen now");
    }
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
    let (held, go) = app.hold_next(tag);
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

/// **Failure (the fault as filed, review of the lane, D3): on a fresh document, ⌃⌘H then a quick ⌘Z met a disabled Undo,** as the session hadn't
/// published the highlight yet: AppKit dropped the key and the highlight stayed. Undo and Save can be chosen while an edit of the tab showing lands: the
/// ⌘Z undoes the highlight, and a Save writes it.
#[test]
fn undo_and_save_can_be_chosen_while_a_highlight_lands_on_a_fresh_document() {
    let disk = Disk::new();
    let doc = disk.pdf("fresh.pdf", 1);
    let app = disk.start();
    app.open(&doc);
    assert_eq!(undo_redo(&app), (false, false), "nothing done yet");
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        assert!(app.core().menu_item_enabled("file.save".into()), "Save can be chosen");
        menu(&app, "edit.undo");
    });
    assert_eq!(highlights(&app), (0, 0), "the highlight undone");
    assert_eq!(undo_redo(&app), (false, true));

    let disk = Disk::new();
    let doc = disk.pdf("fresh.pdf", 1);
    let app = disk.start();
    app.open(&doc);
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "file.save");
    });
    let read = recto_core::highlights::page_highlights(&doc, 0).expect("the saved file reads");
    assert_eq!(read.found.len(), 1, "the Save chosen while it landed wrote it");
}

/// Failure (D3): the Undo chosen while a highlight landed, with the highlight refused and no step to undo, says something or undoes something. It does
/// nothing and says nothing of its own, as an Undo with nothing to undo always has: the line by the page is the highlight's refusal.
#[test]
fn an_undo_chosen_while_a_refused_highlight_landed_does_nothing() {
    let disk = Disk::new();
    let doc = disk.pdf("fresh.pdf", 1);
    let app = disk.start();
    app.open(&doc);
    selects(&app, (0, 0), (0, 0));
    let (held, go) = app.hold_next("highlight");
    menu(&app, HIGHLIGHT_ID);
    held.recv_timeout(WAIT_LIMIT).expect("held");
    menu(&app, "edit.undo");
    drop(go);
    app.settle();
    assert_eq!(undo_redo(&app), (false, false), "nothing done, nothing undone");
    let notice = app.reader().selection_notice.map(|notice| notice.text);
    assert_eq!(
        notice.as_deref(),
        Some("Couldn't highlight: there's no text selected there."),
        "only the highlight's refusal is said"
    );
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

/// Failure: the person's edits asked while a highlight is read run in another order than asked. Each sequence is one a person can reach, every item
/// enabled as it's chosen: with the turn undone first (so Redo can be chosen), Undo then Redo puts the highlight back; Redo then Undo takes it off (the
/// highlight cleared what there was to redo, so the Redo has nothing); and two highlights then two Undos leave neither and the turn.
#[test]
fn every_order_asked_is_the_order_done() {
    let with_the_turn_undone = || {
        let app = with_a_step_before();
        menu(&app, "edit.undo");
        assert_eq!(undo_redo(&app), (false, true), "the turn undone, to redo");
        app
    };
    let app = with_the_turn_undone();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.undo");
        menu(&app, "edit.redo");
    });
    assert_eq!(highlights(&app), (1, 0), "highlight, undo, redo: it's there");
    assert_eq!(undo_redo(&app), (true, false), "and the turn's redo went with the highlight");

    let app = with_the_turn_undone();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.redo");
        menu(&app, "edit.undo");
    });
    assert_eq!(highlights(&app), (0, 0), "highlight, redo (nothing), undo: it's gone");
    assert_eq!(undo_redo(&app), (false, true), "the turn not redone: only the highlight to redo");

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

    // Two tabs (D2): A's ⌘Z waits for A's highlight; B's, asked after it, goes at once and takes B's own step.
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 2), disk.pdf("b.pdf", 2));
    let app = disk.start();
    app.open(&b);
    menu(&app, "rotate.right");
    app.open(&a);
    menu(&app, "rotate.right");
    app.settle();
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        menu(&app, "edit.undo");
        app.act(UiAction::Open { path: b.display().to_string() });
        menu(&app, "edit.undo");
        assert_eq!(undo_redo(&app), (false, true), "B's undo done, its turn taken off, while A's highlight is read");
        app.act(UiAction::Open { path: a.display().to_string() });
    });
    assert_eq!(highlights(&app), (0, 0), "A: highlight, undo: it's gone");
    assert_eq!(undo_redo(&app), (true, true), "A's turn stands");
}

/// Failure: a highlight that's refused (no text where it was asked) leaves the person's Undo waiting for good, or drops it. The Undo runs when the refusal's
/// in and undoes the last thing that happened, the turn, as the highlight made no step.
#[test]
fn an_undo_after_a_refused_highlight_undoes_the_step_before() {
    let app = with_a_step_before();
    selects(&app, (0, 0), (0, 0));
    let (held, go) = app.hold_next("highlight");
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
    let (held, go) = app.hold_next("save_writing");
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

/// **Failure (review of the lane, D2): an edit in one tab waits for another tab's.** A highlight's read in tab A (up to `selecting`'s 60 s cap, or a hung
/// volume) parked every edit after it, so ⌘Z, ⌘R, Save and ⌘W in tab B did nothing till A's highlight landed. Each is done in B while A's is held, and
/// A's lands after.
#[test]
fn a_held_highlight_in_one_tab_holds_up_nothing_in_another() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 2), disk.pdf("b.pdf", 2));
    let app = disk.start();
    app.open(&b);
    menu(&app, "rotate.right");
    app.settle();
    app.open(&a);
    let in_a = app.reader().shown_tab;
    while_a_highlight_is_read(&app, HIGHLIGHT_ID, "highlight", || {
        app.act(UiAction::Open { path: b.display().to_string() });
        flush(&app);
        assert_ne!(app.reader().shown_tab, in_a, "B showing");
        menu(&app, "edit.undo");
        assert_eq!(undo_redo(&app), (false, true), "⌘Z in B done while A's highlight is held");
        menu(&app, "rotate.right");
        assert_eq!(undo_redo(&app), (true, false), "⌘R in B done");
        menu(&app, "file.save");
        app.wait_until("B saved while A's highlight is held", || {
            flush(&app);
            !app.reader().edited && app.core().tools.save_ui().running.load(std::sync::atomic::Ordering::SeqCst) == 0
        });
        assert_eq!(menu(&app, "file.close"), HostAction::None, "B, saved, closes without asking");
        app.wait_until("B closed while A's highlight is held", || {
            flush(&app);
            app.tabs().len() == 1
        });
        assert!(app.core().life.persons_running() >= 1, "A's highlight still held");
    });
    assert_eq!(app.reader().shown_tab, in_a);
    assert_eq!(highlights(&app), (1, 0), "A's highlight landed after");
    let saved = std::fs::read(&b).expect("B");
    assert!(String::from_utf8_lossy(&saved).contains("/Rotate 90"), "B saved turned");
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
        flush(&app);
        assert!(
            !app.reader().can_undo,
            "done at once: the turn undone (the menu's Undo is on for the highlight landing)"
        );
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
    let (held, go) = app.hold_next("highlight");
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

/// **Failure (review of the lane, D1a): a close asks the person when nothing that changes the document is landing.** Every ordered command's ticket
/// counted as "an edit landing", so Print then ⌘W, and ⌘W ⌘W, on a clean tab raised the close prompt, which `main` never did. Only an edit that lands later
/// (a highlight or its removal) asks: with a print held and a close waiting behind it, ⌘W and ⌘W again close without asking, and the tab goes once the
/// print's pages are read.
#[test]
fn a_close_asks_only_for_an_edit_that_lands_later() {
    let disk = Disk::new();
    let doc = disk.pdf("a.pdf", 2);
    let app = disk.start();
    app.open(&doc);
    let out = app.disk().files().join("printed.pdf");
    let (held, go) = app.hold_next("print");
    app.person(&format!(r#"{{"cmd":"print","to_pdf":{}}}"#, Value::from(out.display().to_string())));
    held.recv_timeout(WAIT_LIMIT).expect("the print held before it reads its pages");
    assert_eq!(menu(&app, "file.close"), HostAction::None, "Print then ⌘W: closed without asking, as on main");
    flush(&app);
    assert_eq!(app.core().core.lane().parked(), 1, "the close waits behind the print");
    assert_eq!(menu(&app, "file.close"), HostAction::None, "⌘W ⌘W: not asked for the first close's own ticket");
    drop(go);
    app.settle();
    assert!(app.tabs().is_empty(), "closed once the print's pages were read");
}

/// What became of the close prompt's choice, made while a highlight of the tab showing landed (or was refused): the tabs left, the highlights in the
/// file, and the save's problem shown, if any.
fn close_prompt_answered(choice: crate::saving::CloseChoice, refused: bool) -> (usize, usize, bool) {
    let disk = Disk::new();
    let doc = disk.pdf("a.pdf", 1);
    let app = disk.start();
    app.open(&doc);
    let tab = app.reader().shown_tab;
    selects(&app, (0, 0), (0, if refused { 0 } else { 5 }));
    let (held, go) = app.hold_next("highlight");
    menu(&app, HIGHLIGHT_ID);
    held.recv_timeout(WAIT_LIMIT).expect("held");
    assert_eq!(menu(&app, "file.close"), HostAction::ConfirmClose, "asked: the highlight is landing");
    let prompt = app.core().close_prompt();
    let titles: Vec<&str> = prompt.buttons.iter().map(|button| button.title.as_str()).collect();
    assert_eq!(
        (prompt.message.as_str(), titles),
        ("Do you want to save the changes you made to “a.pdf”?", vec!["Save", "Cancel", "Don't Save"]),
        "the ordinary unsaved-changes prompt"
    );
    app.core().close_answered(tab, choice);
    drop(go);
    app.settle();
    let in_file = recto_core::highlights::page_highlights(&doc, 0).expect("the file reads").found.len();
    (app.tabs().len(), in_file, app.core().tools.save_ui().problem().is_some())
}

/// **Failure (review of the lane, D1b and c, Serious): ⌃⌘H then ⌘W on a clean tab offered only Close and Cancel,** and Close threw the highlight away:
/// the prompt was built from what the session had published, which didn't have the highlight yet. It's the unsaved-changes prompt, and each choice runs
/// after the highlight lands: Save writes it and closes, Don't Save throws it away and closes, Cancel keeps it. With the highlight refused after all, Save
/// and Don't Save still close, and nothing is said of a save with nothing to save.
#[test]
fn the_close_prompt_while_a_highlight_lands_offers_save_and_each_choice_does_what_it_says() {
    use crate::saving::CloseChoice;
    assert_eq!(close_prompt_answered(CloseChoice::Save, false), (0, 1, false), "Save: written, closed");
    assert_eq!(
        close_prompt_answered(CloseChoice::DontSave, false),
        (0, 0, false),
        "Don't Save: thrown away, closed"
    );
    assert_eq!(
        close_prompt_answered(CloseChoice::Cancel, false),
        (1, 0, false),
        "Cancel: the tab stays, unsaved"
    );
    assert_eq!(
        close_prompt_answered(CloseChoice::Save, true),
        (0, 0, false),
        "refused, Save: closed, no problem said"
    );
    assert_eq!(close_prompt_answered(CloseChoice::DontSave, true), (0, 0, false), "refused, Don't Save: closed");
    assert_eq!(
        close_prompt_answered(CloseChoice::Cancel, true),
        (1, 0, false),
        "refused, Cancel: the tab stays"
    );
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

/// The highlight held as the person's first, with a second highlight (on page 2) and a ⌘Z asked behind it, both parked, and a quit asked: what lets the
/// first go, and the app, quitting.
fn two_highlights_and_an_undo_parked_and_a_quit(app: &HeadlessApp) -> std::sync::mpsc::Sender<()> {
    selects(app, (0, 0), (0, 5));
    let (held, go) = app.hold_next("highlight");
    menu(app, HIGHLIGHT_ID);
    held.recv_timeout(WAIT_LIMIT).expect("the first highlight held");
    selects(app, (1, 0), (1, 5));
    menu(app, HIGHLIGHT_ID);
    menu(app, "edit.undo");
    flush(app);
    assert_eq!(app.core().core.lane().parked(), 2, "the second highlight and the undo wait their turn");
    app.core().quit_requested();
    go
}

fn quit_answers(app: &HeadlessApp) -> usize {
    app.heard().iter().filter(|heard| matches!(heard, crate::testkit::Heard::QuitAnswer(_))).count()
}

/// **Failure (review of the lane, D4): a quit with edits parked ran a later one without an earlier one.** After the freeze the parked highlight was
/// refused as busy, silently, and the ⌘Z parked behind it ran and undid the first highlight: H1, H2, ⌘Z, ⌘Q came back with neither. Everything parked
/// was asked before the quit and passes the freeze in order: at the next launch H1 is there and H2 undone, to redo.
#[test]
fn a_quit_runs_every_edit_parked_in_order() {
    let app = with_a_step_before();
    let go = two_highlights_and_an_undo_parked_and_a_quit(&app);
    drop(go);
    app.wait_until("the quit's answer", || quit_answers(&app) == 1);
    let disk = app.quit_anyway();
    let app = disk.start();
    app.settle();
    assert_eq!(highlights(&app), (1, 0), "the first highlight there, the second undone");
    assert_eq!(undo_redo(&app), (true, true), "the second to redo");
    drop(app.quit());
}

/// Failure (D4): past the quit's patience, some of what's parked runs after the record's written and some doesn't, so a later edit can be had without
/// an earlier one. Everything still parked is dropped together as the patience ends: nothing is undone at the next launch.
#[test]
fn past_the_quits_patience_whats_parked_is_dropped_together() {
    let app = with_a_step_before();
    let go = two_highlights_and_an_undo_parked_and_a_quit(&app);
    app.wait_until("the quit's answer, past its patience", || {
        app.advance(crate::listener::QUIT_PATIENCE_MS);
        quit_answers(&app) == 1
    });
    assert_eq!(app.core().core.lane().parked(), 0, "what was parked is dropped");
    drop(go);
    app.wait_until("the first highlight's thread done", || app.core().life.persons_running() == 0);
    let disk = app.quit_anyway();
    let app = disk.start();
    app.settle();
    assert!(highlights(&app).0 <= 1, "at most the first");
    assert!(!undo_redo(&app).1, "nothing undone: the undo went with the highlight asked before it");
    drop(app.quit());
}

/// **Failure (review of the lane, D9): an ordered command on the session thread that reads its file first passes its turn as it goes to read,** so an
/// edit asked after it acts first. No ordered command reads later today (only a tap, an open and a comparison do), so a turn is never let go at the read;
/// `person::Later` refuses an ordered command there in a debug build, as its turn would have to be carried through the read first. This holds today's:
/// each of the person's ordered commands on the session thread, on a cold tab with reads held, starts no read and is done at once.
#[test]
fn no_ordered_command_on_the_session_thread_reads_its_file_later() {
    let app = HeadlessApp::start();
    let files: Vec<_> = (0..6).map(|at| app.disk().pdf(&format!("f{at}.pdf"), 3)).collect();
    for file in &files {
        app.open(file);
    }
    app.settle();
    // Without sleep files, a cold tab's waking reads its PDF.
    app.core().host.session.with_session(|session| session.dormancy_mut().set_sleep_files(false));
    let inner = app.core();
    inner.life.hold_reads(true);
    for line in [
        r##"{"cmd":"page_rotate","tab":"#1","pages":"all","degrees":90}"##,
        r##"{"cmd":"page_move","tab":"#2","pages":"1","to":2}"##,
        r##"{"cmd":"page_delete","tab":"#3","pages":"1"}"##,
        r##"{"cmd":"undo","tab":"#1"}"##,
        r##"{"cmd":"redo","tab":"#1"}"##,
        r##"{"cmd":"discard","tab":"#2"}"##,
        r##"{"cmd":"clear_history","tab":"#3"}"##,
        r##"{"cmd":"close","tab":"#4"}"##,
    ] {
        assert!(
            super::super::person_lane::ordered(&recto_agent::parse(line).expect("a command")),
            "{line} is ordered"
        );
        app.person(line);
        flush(&app);
        assert_eq!(inner.life.reads_running(), 0, "{line} started no read");
        assert_eq!(inner.core.lane().parked(), 0, "{line} done at once");
    }
    inner.life.hold_reads(false);
    app.settle();
    assert_eq!(app.tabs().len(), 5, "the close was done");
}

/// **Failure (second review of the lane, 3): a command asked as a hand-off froze ran after its last look.** `submit` looked at the freeze, the hand-off
/// froze and saw nothing counted, and the command was then counted and run past the freeze (its turn let it through): what it did was lost, as what's on
/// disk is what comes back. It's counted first and looked at again: frozen between the two, it doesn't run.
#[test]
fn a_command_asked_as_a_hand_off_freezes_doesnt_run_after_its_last_look() {
    let disk = Disk::new();
    let doc = disk.pdf("a.pdf", 1);
    let app = disk.start();
    app.open(&doc);
    selects(&app, (0, 0), (0, 5));
    let (held, go) = app.hold_next("submit_looked");
    std::thread::scope(|scope| {
        let asked = std::thread::Builder::new()
            .name("recto-test-window".into())
            .spawn_scoped(scope, || app.act(UiAction::Menu { id: HIGHLIGHT_ID.into() }))
            .expect("the window's thread");
        held.recv_timeout(WAIT_LIMIT).expect("the highlight held after submit's look");
        app.core().set_frozen(true);
        assert!(app.core().hand_off_check().safe, "the hand-off's last look sees nothing under way");
        drop(go);
        asked.join().expect("the window's thread");
    });
    app.wait_until("nothing of the person's running", || app.core().life.persons_running() == 0);
    flush(&app);
    // Taken again, as a hand-off that didn't go leaves it, to look.
    app.core().set_frozen(false);
    assert_eq!(highlights(&app), (0, 0), "not run after the freeze");
}

/// The state folder's own part that the record goes in, made so it can't be written (so a quit asks, and can be stayed), or writable again.
#[cfg(unix)]
fn record_folder_writable(app: &HeadlessApp, writable: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    let folder = app.core().saver.store.main_folder();
    std::fs::create_dir_all(&folder).expect("the record's folder");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(if writable { 0o700 } else { 0o500 })).expect("the record's folder");
}

/// **Failure (second review of the lane, 4): edits dropped at a quit's patience were dropped in silence,** and a person who then chose Stay saw edits
/// they'd asked for simply not done. Stayed, the page says so once, in Rust's words; the first highlight, in flight, stands, and the undo dropped with the
/// second highlight before it doesn't run.
#[cfg(unix)]
#[test]
fn a_stay_after_the_quits_patience_says_the_edits_dropped() {
    let app = with_a_step_before();
    record_folder_writable(&app, false);
    let go = two_highlights_and_an_undo_parked_and_a_quit(&app);
    app.wait_until("the quit's question, past its patience", || {
        app.advance(crate::listener::QUIT_PATIENCE_MS);
        quit_answers(&app) == 1
    });
    app.stay();
    flush(&app);
    let notice = |app: &HeadlessApp| app.reader().selection_notice.map(|notice| notice.text);
    assert_eq!(notice(&app).as_deref(), Some(crate::core::person_lane::DROPPED_AT_QUIT), "said by the page");
    drop(go);
    app.settle();
    assert_eq!(highlights(&app), (1, 0), "the first highlight stands, the second wasn't done");
    assert_eq!(undo_redo(&app), (true, false), "no undo ran");
    app.core().dismiss_selection_notice();
    app.settle();
    assert_eq!(notice(&app), None, "said once");
    record_folder_writable(&app, true);
    drop(app.quit());
}

/// **Failure (second review of the lane, 5): a Save chosen while a highlight landed took the highlight's refusal off the page.** With the highlight refused,
/// the Save found nothing to save and cleared the line by the page as a save does, so the person never saw why nothing was highlighted. A save that
/// writes nothing leaves the line as it is.
#[test]
fn a_save_with_nothing_to_save_leaves_the_refusal_showing() {
    let disk = Disk::new();
    let doc = disk.pdf("fresh.pdf", 1);
    let app = disk.start();
    app.open(&doc);
    selects(&app, (0, 0), (0, 0));
    let (held, go) = app.hold_next("highlight");
    menu(&app, HIGHLIGHT_ID);
    held.recv_timeout(WAIT_LIMIT).expect("held");
    menu(&app, "file.save");
    drop(go);
    app.settle();
    let notice = app.reader().selection_notice.map(|notice| notice.text);
    assert_eq!(
        notice.as_deref(),
        Some("Couldn't highlight: there's no text selected there."),
        "the refusal still showing"
    );
}
