use super::*;
use crate::core::sheets::NO_BUTTON;
use recto_agent::exit;
use serde_json::json;

const PLAN: &str = "00ab12cd34ef5678";

fn check(tab: u32) -> Family {
    Family::RemovalCheck(RemovalCheck { tab: TabRef::Id(TabId(tab)) })
}

fn clear(tab: u32) -> Family {
    Family::RemovalClear(RemovalClear {
        tab: TabRef::Id(TabId(tab)),
        confirm: PLAN.into(),
    })
}

fn offered(words: &str) -> Result<Value, AgentError> {
    Ok(json!({ "words": words, "plan": PLAN, "offered": true }))
}

fn error(code: Code, message: &str) -> Result<Value, AgentError> {
    Err(AgentError::new(code, message, exit::FAILED))
}

/// A leftovers sheet with an offer for tab 3 showing, and its id.
fn with_an_offer() -> (Leftovers, u64) {
    let sheet = Leftovers::default();
    assert_eq!(sheet.answered(&check(3), &offered("1 unfinished save beside “a.pdf”.")), None);
    let id = sheet.showing().expect("the offer").id;
    (sheet, id)
}

fn is_clear_of(command: &Command, tab: u32) -> bool {
    matches!(command, Command::Removal(Family::RemovalClear(RemovalClear { tab: TabRef::Id(TabId(t)), confirm })) if *t == tab && confirm == PLAN)
}

/// Failure: the plan, which is what lets a clear go, reaches a place an agent or a log can read it: the sheet the host is given (any field), or its debug
/// text. The offer says what it will do, with its destructive button first, and carries nothing a client could replay.
#[test]
fn an_offers_model_has_its_words_and_buttons_and_no_plan_anywhere() {
    let (sheet, _) = with_an_offer();
    let model = sheet.showing().expect("the offer");
    assert_eq!(model.kind, KIND);
    assert_eq!(model.title, words::OFFER_TITLE);
    assert_eq!(model.message, "1 unfinished save beside “a.pdf”.");
    assert_eq!(model.buttons, [words::BUTTON, words::CANCEL]);
    assert_eq!(model.destructive, Some(0));
    for text in [format!("{model:?}"), format!("{sheet:?}")] {
        assert!(!text.contains(PLAN) && !text.contains("00ab"), "{text}");
    }
}

/// Failure: the yes sends nothing, sends a clear of another tab or plan, or sends it twice (a double click clearing twice, or a stale click clearing what a
/// later look changed).
#[test]
fn yes_sends_the_clear_of_that_tab_and_plan_once() {
    let (sheet, id) = with_an_offer();
    let sent = sheet.answer(id, 0).expect("a clear is sent");
    assert!(is_clear_of(&sent, 3), "{sent:?}");
    assert!(sheet.showing().is_none(), "the sheet goes with the yes");
    assert!(sheet.answer(id, 0).is_none(), "a second yes sends nothing");
}

/// Failure: a button that isn't the offer's first sends a clear: Cancel, a window closed with no button, or an index the host got wrong.
#[test]
fn cancel_no_button_and_any_other_number_send_nothing_and_take_the_sheet_away() {
    for button in [1, 7, NO_BUTTON] {
        let (sheet, id) = with_an_offer();
        assert!(sheet.answer(id, button).is_none(), "button {button}");
        assert!(sheet.showing().is_none(), "button {button} closes it");
    }
}

/// Failure: an answer to a sheet that was replaced (a window's late click on the old one) acts on, or closes, the newer sheet.
#[test]
fn a_stale_id_changes_nothing_and_sends_nothing() {
    let (sheet, old) = with_an_offer();
    sheet.answered(&check(4), &offered("another"));
    let newer = sheet.showing().expect("the newer");
    assert_ne!(newer.id, old);
    assert!(sheet.answer(old, 0).is_none());
    assert_eq!(sheet.showing(), Some(newer.clone()), "the newer sheet is as it was");
    assert!(is_clear_of(&sheet.answer(newer.id, 0).expect("its yes"), 4));
}

/// Failure: a look that found nothing offers a button that clears, or hides nothing-to-clear behind a destructive button.
#[test]
fn a_look_with_nothing_to_clear_has_only_ok() {
    let sheet = Leftovers::default();
    sheet.answered(&check(1), &Ok(json!({ "words": "Nothing found to clear.", "offered": false })));
    let model = sheet.showing().expect("said");
    assert_eq!(
        (model.title.as_str(), model.buttons.as_slice(), model.destructive),
        (words::LOOKED_TITLE, &[words::OK.to_string()][..], None)
    );
    assert_eq!(model.message, "Nothing found to clear.");
    assert_eq!(sheet.kind(), "looked");
}

/// Failure: a failure shows an agent's words to a person (a usage error naming `recto`, a level's refusal), or a retry is offered for a tab that's gone,
/// or the stop of a clear is titled as a refusal. The code alone decides.
#[test]
fn each_failure_has_its_title_words_and_buttons_by_its_code() {
    let again = [words::LOOK_AGAIN.to_string(), words::OK.to_string()];
    let ok = [words::OK.to_string()];
    let cases: [(Code, &str, &str, &[String]); 8] = [
        (Code::REMOVAL_BUSY, words::REFUSED_TITLE, "busy words", &again),
        (Code::REMOVAL_PLAN_CHANGED, words::REFUSED_TITLE, "changed words", &again),
        (Code::REMOVAL_PLAN_EXPIRED, words::REFUSED_TITLE, "expired words", &again),
        (Code::REMOVAL_UNKNOWN_PLAN, words::REFUSED_TITLE, "unknown words", &again),
        (Code::REMOVAL_CANT_CLEAR, words::REFUSED_TITLE, "can't words", &again),
        (Code::REMOVAL_STOPPED, words::STOPPED_TITLE, "stopped words", &again),
        (Code::NO_SUCH_TAB, words::REFUSED_TITLE, "gone words", &ok),
        (Code::FILE_MISSING, words::REFUSED_TITLE, "missing words", &ok),
    ];
    for (code, title, message, buttons) in cases {
        let sheet = Leftovers::default();
        sheet.answered(&clear(2), &error(code, message));
        let model = sheet.showing().unwrap_or_else(|| panic!("{code:?} says something"));
        assert_eq!(
            (model.title.as_str(), model.message.as_str(), model.buttons.as_slice(), model.destructive),
            (title, message, buttons, None),
            "{code:?}"
        );
    }
    for code in [Code::USAGE, Code::PERMISSION_DENIED, Code::NOT_FOUND] {
        let sheet = Leftovers::default();
        sheet.answered(&check(2), &error(code, "an agent's words naming `recto` and Full access"));
        let model = sheet.showing().expect("said");
        assert_eq!(
            (model.title.as_str(), model.message.as_str()),
            (words::REFUSED_TITLE, words::COULDNT),
            "{code:?}"
        );
    }
}

/// Failure: Look Again does nothing, or looks at another tab than the one the sheet was about.
#[test]
fn look_again_sends_the_check_of_the_same_tab() {
    let sheet = Leftovers::default();
    sheet.answered(&clear(5), &error(Code::REMOVAL_PLAN_CHANGED, "Something changed since you looked."));
    let id = sheet.showing().expect("told").id;
    let sent = sheet.answer(id, 0).expect("a look");
    assert!(
        matches!(&sent, Command::Removal(Family::RemovalCheck(RemovalCheck { tab: TabRef::Id(TabId(5)) }))),
        "{sent:?}"
    );
    // OK of a refusal with no retry sends nothing.
    sheet.answered(&clear(5), &error(Code::NO_SUCH_TAB, "That tab isn't open."));
    let id = sheet.showing().expect("told").id;
    assert!(sheet.answer(id, 0).is_none());
}

/// Failure: a clear that finished leaves its sheet up, or its words are lost (the person is told nothing happened).
#[test]
fn a_finished_clear_says_its_words_and_leaves_no_sheet() {
    let (sheet, _) = with_an_offer();
    let said = sheet.answered(
        &clear(3),
        &Ok(json!({ "words": "Cleared: 1 unfinished save moved to the Trash.", "outcome": "complete" })),
    );
    assert_eq!(said.as_deref(), Some("Cleared: 1 unfinished save moved to the Trash."));
    assert!(sheet.showing().is_none());
    assert_eq!(sheet.kind(), "none");
}

/// Failure: a sheet's id repeats, so a late click on an old sheet answers a new one.
#[test]
fn ids_grow_and_never_repeat() {
    let sheet = Leftovers::default();
    let mut seen = Vec::new();
    for tab in 1..=5 {
        sheet.answered(&check(tab), &offered("words"));
        let id = sheet.showing().expect("showing").id;
        seen.push(id);
        sheet.answer(id, NO_BUTTON);
    }
    assert_eq!(seen[0], 1);
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]), "{seen:?}");
}
