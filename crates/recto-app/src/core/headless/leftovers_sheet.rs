//! **Clear Leftovers as the person has it** (D6): Edit > Clear Leftovers… sends the person's `removal check`, its answer is the sheet, the sheet's yes sends the
//! person's `removal clear`, and what it did is said by the page. Run on the app's own core with `removal_clear.rs`'s bed: one crashed save of a dead process
//! beside `report.pdf`, and the two copies of the session an update and a damaged record leave. An agent's same commands never open a sheet.

use super::removal_clear::{Bed, DEAD, bed};
use crate::core::controls::UiAction;
use crate::core::removal::sheet::{KIND, MENU_ID};
use crate::core::sheets::{NO_BUTTON, SheetModel};
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_agent::{Command, Level, TabRef};
use recto_control::removal_store::leftovers::words;

fn menu(app: &HeadlessApp) {
    app.act(UiAction::Menu { id: MENU_ID.into() });
    app.settle();
}

fn sheet(app: &HeadlessApp) -> Option<SheetModel> {
    app.reader().sheet
}

fn answer(app: &HeadlessApp, sheet: &SheetModel, button: u32) {
    app.act(UiAction::SheetAnswered {
        kind: sheet.kind.clone(),
        id: sheet.id,
        button,
    });
    app.settle();
}

fn notice(app: &HeadlessApp) -> Option<String> {
    app.core().window.notices().notice()
}

/// A second crashed save beside the document, of another dead process.
fn another_crash(bed: &Bed) {
    let temp = format!("{}{}-1.tmp", recto_core::save::writer_temp_prefix(&bed.document), DEAD + 1);
    std::fs::write(bed.document.parent().unwrap().join(temp), b"another crashed save").unwrap();
}

/// Failure: the person's Clear Leftovers does something other than the commands an agent sends (a second path to the same acts), the sheet doesn't offer
/// what is to be done in Rust's words, the yes doesn't move the save and delete the copies, or what was done isn't said by the page. The person's check and
/// then clear are sent as theirs, in that order, and the sheet is gone after.
#[test]
fn the_menu_offers_and_the_yes_clears_by_the_persons_own_commands() {
    let bed = bed();
    bed.app.keep_sent();
    menu(&bed.app);
    let offer = sheet(&bed.app).expect("the check is shown as a sheet");
    assert_eq!((offer.kind.as_str(), offer.title.as_str()), (KIND, words::OFFER_TITLE));
    assert!(
        offer.message.contains("“report.pdf”"),
        "the person's words name the document: {}",
        offer.message
    );
    assert_eq!(offer.buttons, [words::BUTTON, words::CANCEL]);
    assert_eq!(offer.destructive, Some(0));
    answer(&bed.app, &offer, 0);
    assert!(sheet(&bed.app).is_none(), "the yes closes it");
    assert_eq!(std::fs::read_dir(bed.trash()).unwrap().count(), 1, "the save is in the Trash");
    assert!(
        !bed.root().join("update/before").exists() && !bed.root().join("session.damaged").exists(),
        "both copies are gone"
    );
    assert_eq!(
        notice(&bed.app).as_deref(),
        Some("Cleared: 1 unfinished save moved to the Trash, 2 session copies deleted.")
    );
    let sent: Vec<&str> = bed
        .app
        .sent()
        .iter()
        .map(|command| match command {
            Command::Removal(recto_agent::commands::Removal::RemovalCheck(_)) => "check",
            Command::Removal(recto_agent::commands::Removal::RemovalClear(clear)) => {
                assert!(matches!(clear.tab, TabRef::Id(_)), "the tab it was about, by number");
                "clear"
            }
            _ => "other",
        })
        .collect();
    assert_eq!(sent, ["check", "clear"], "the person's own commands, as theirs");
}

/// Failure: Cancel, or a sheet closed with no button, moves something (the destructive button read for any answer).
#[test]
fn cancel_and_a_sheet_closed_with_no_button_change_nothing() {
    for button in [1, NO_BUTTON, 7] {
        let bed = bed();
        let before = bed.listing();
        menu(&bed.app);
        let offer = sheet(&bed.app).expect("an offer");
        answer(&bed.app, &offer, button);
        assert!(sheet(&bed.app).is_none(), "button {button} closes the sheet");
        assert_eq!(bed.listing(), before, "button {button} did nothing");
        assert!(!bed.trash().exists());
    }
}

/// Failure: a save that appeared after the person looked is cleared on a yes they gave to something else (the plan was about what was seen), or a changed
/// plan leaves the person with no way on. It is told, nothing moves, Look Again offers both, and a yes then clears both.
#[test]
fn a_change_after_the_look_is_told_and_look_again_offers_what_is_there() {
    let bed = bed();
    let before = bed.listing();
    menu(&bed.app);
    let offer = sheet(&bed.app).expect("an offer");
    another_crash(&bed);
    let after_the_look = bed.listing();
    answer(&bed.app, &offer, 0);
    let told = sheet(&bed.app).expect("told");
    assert_eq!(told.title, words::REFUSED_TITLE);
    assert!(told.message.starts_with("Something changed since you looked"), "{}", told.message);
    assert_eq!(told.buttons, [words::LOOK_AGAIN, words::OK]);
    assert_ne!(before, after_the_look);
    assert_eq!(bed.listing(), after_the_look, "nothing moved");
    answer(&bed.app, &told, 0);
    let again = sheet(&bed.app).expect("a new offer");
    assert!(again.destructive == Some(0) && again.message.contains("2 unfinished saves"), "{again:?}");
    answer(&bed.app, &again, 0);
    assert!(sheet(&bed.app).is_none());
    assert_eq!(std::fs::read_dir(bed.trash()).unwrap().count(), 2, "both saves are in the Trash");
}

/// Failure (rule 3): the yes waits for the clear, so the window stops while the volume is slow. The yes returns while the clear is held, with nothing moved
/// yet; the menu again, while it runs, is told it is clearing; and once it goes on it finishes and says what it did.
#[test]
fn the_yes_returns_at_once_and_a_clear_in_progress_is_told_clearing() {
    let bed = bed();
    menu(&bed.app);
    let offer = sheet(&bed.app).expect("an offer");
    let removal = bed.app.core().tools.removal();
    removal.hold_at(1);
    bed.app.act(UiAction::SheetAnswered {
        kind: offer.kind.clone(),
        id: offer.id,
        button: 0,
    });
    bed.app
        .wait_for("the clear to be held", || removal.held().then_some(()).ok_or_else(|| "not held yet".to_owned()));
    assert!(!bed.trash().exists(), "the act returned with the clear still to do");
    // Not settled: a settle waits for the held clear, which is what is being looked past.
    bed.app.act(UiAction::Menu { id: MENU_ID.into() });
    bed.app.wait_for("the menu to be told it is clearing", || match sheet(&bed.app) {
        Some(told) if told.message == words::CLEARING => Ok(()),
        other => Err(format!("{other:?}")),
    });
    removal.release();
    bed.app.settle();
    assert!(sheet(&bed.app).is_none());
    assert_eq!(
        notice(&bed.app).as_deref(),
        Some("Cleared: 1 unfinished save moved to the Trash, 2 session copies deleted.")
    );
}

/// Failure: a clear that ran out of time is said as a refusal (so the person thinks nothing was tried), or titled as one that never started.
#[test]
fn a_clear_that_runs_out_of_time_is_a_stop_with_look_again() {
    let bed = bed();
    menu(&bed.app);
    let offer = sheet(&bed.app).expect("an offer");
    let removal = bed.app.core().tools.removal();
    removal.set_limit(std::time::Duration::from_millis(50));
    removal.hold_at(1);
    answer(&bed.app, &offer, 0);
    removal.release();
    bed.app.settle();
    let told = sheet(&bed.app).expect("told");
    assert_eq!(told.title, words::STOPPED_TITLE);
    assert!(told.message.contains("ran out of time"), "{}", told.message);
    assert_eq!(told.buttons, [words::LOOK_AGAIN, words::OK]);
}

/// Failure: a document with nothing beside it offers a button that clears, or says nothing.
#[test]
fn a_document_with_nothing_to_clear_says_so_with_only_ok() {
    let disk = Disk::new();
    let document = disk.pdf("clean.pdf", 2);
    let app = disk.builder().start();
    app.open(&document);
    app.settle();
    menu(&app);
    let looked = sheet(&app).expect("said");
    assert_eq!(
        (looked.title.as_str(), looked.message.as_str()),
        (words::LOOKED_TITLE, "Nothing found to clear.")
    );
    assert_eq!((looked.buttons.as_slice(), looked.destructive), (&[words::OK.to_string()][..], None));
}

/// Failure: an agent's `removal check` (or its palette run of the menu's row) opens a sheet over the person's window, or hands a plan to an agent that may not
/// clear. Every level checks and runs the row with no sheet, only Full's own check is given a plan, and the palette's answer carries none.
#[test]
fn an_agent_never_opens_a_sheet_and_only_full_is_given_a_plan() {
    let bed = bed();
    for level in [Level::ReadOnly, Level::Control, Level::Full] {
        let checked = bed.app.agent(level, r#"{"cmd":"removal_check"}"#).expect("a look");
        bed.app.settle();
        assert_eq!(checked.get("plan").is_some(), level == Level::Full, "{level:?}: {checked}");
        assert!(sheet(&bed.app).is_none(), "{level:?}'s check opened a sheet");
        let ran = bed.app.agent(level, r#"{"cmd":"palette","query":"Clear Leftovers","run":true}"#);
        bed.app.settle();
        if let Ok(ran) = &ran {
            assert_eq!(ran["ran"]["id"], MENU_ID, "{level:?}: {ran}");
            assert!(!ran.to_string().contains("plan"), "the palette's answer carries no plan: {ran}");
        }
        assert!(sheet(&bed.app).is_none(), "{level:?}'s palette run opened a sheet: {ran:?}");
    }
}

/// Failure: a check or clear dropped from the person's lane (answered `CANCELLED`, which no feature says words for) opens a "couldn't clear" sheet over the
/// window, or announces a change when nothing changed.
#[test]
fn a_command_dropped_from_the_lane_opens_no_sheet_and_announces_nothing() {
    let bed = bed();
    let inner = std::sync::Arc::clone(bed.app.core());
    assert!(inner.core.flush(std::time::Duration::from_secs(10)), "at rest");
    let asked = inner.core.reconciles_asked();
    let dropped = recto_agent::parse(r#"{"cmd":"removal_check"}"#).expect("a command");
    crate::core::person::not_run(&crate::core::answer_parts::AnswerParts::of(&inner), &dropped, "dropped");
    assert!(sheet(&bed.app).is_none());
    assert_eq!(inner.core.reconciles_asked(), asked, "nothing changed, so nothing was announced");
}
