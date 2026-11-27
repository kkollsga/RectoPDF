//! **Sheets over the window** (a save's problem today, Clear Leftovers next): what a sheet carries and what each answer does, pinned on the app's own core
//! before the sheets moved onto one Rust-worded model (`core/sheets.rs`).

use super::changed_under_edits::{look, pdf_bytes, replace};
use crate::core::HostAction;
use crate::core::controls::UiAction;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

/// A save that was refused because another program replaced the file under the edits: the person's Save leaves a problem to show.
fn with_a_problem() -> HeadlessApp {
    let app = HeadlessApp::start();
    let file = app.disk().file("doc.pdf", &pdf_bytes("Mine", 10));
    app.open(&file);
    app.recto(Level::Control, &["page", "rotate", "9", "--by", "90"]).expect("rotated");
    app.settle();
    replace(&file, &pdf_bytes("Theirs", 5));
    look(&app);
    app.core().perform("file.save".into());
    app.settle();
    app.wait_for("the person's Save answered in the window", || {
        app.reader().sheet.map(|_| ()).ok_or_else(|| "no problem to show yet".to_string())
    });
    app
}

/// Failure: a sheet that lost a word, a button, or the way an answer acts (Save As opening, or the problem staying after it's answered) when the sheets
/// moved onto one model. The person's Save problem has Rust's title, message and OK; a problem where a copy helps also has Save As; either answer clears
/// it; only Save As asks the host to open its panel.
#[test]
fn a_save_problem_carries_its_words_and_each_answer_acts_as_before() {
    let app = with_a_problem();
    let problem = app.reader().sheet.expect("the problem is showing");
    let error = recto_agent::AgentError::new(recto_agent::Code::READ_ONLY, "The file is read-only.", recto_agent::exit::FAILED);
    let words = crate::saving::problem_for(&error);
    assert_eq!(problem.title, words.title, "Rust's title, as the save's own words have it");
    assert!(!problem.message.is_empty(), "{problem:?}");
    assert_eq!((problem.kind.as_str(), problem.destructive), ("save_problem", None), "{problem:?}");
    assert_eq!(problem.buttons.first().map(String::as_str), Some("OK"));
    // A file changed under the edits can't be saved as a copy either, so no Save As is offered here.
    assert_eq!(problem.buttons.len(), 1, "{problem:?}");
    assert_eq!(answer(&app, 0), HostAction::None);
    app.settle();
    assert!(app.reader().sheet.is_none(), "answered, so gone");
    // A problem where a copy helps offers Save As, and choosing it opens the panel.
    app.core().tools.save_ui().set_problem(Some(words.clone()));
    app.send(crate::queue::CoreCall::Reconcile);
    app.settle();
    let problem = app.reader().sheet.expect("the second problem is showing");
    assert_eq!(problem.buttons, [words.ok_title.clone(), "Save As…".to_owned()], "{problem:?}");
    assert_eq!(
        (problem.title.as_str(), problem.message.as_str()),
        (words.title.as_str(), words.message.as_str())
    );
    assert_eq!((problem.kind.as_str(), problem.destructive), ("save_problem", None));
    // OK is button 0 even where Save As is offered: it opens nothing, and the problem goes.
    assert_eq!(answer(&app, 0), HostAction::None);
    app.settle();
    assert!(app.reader().sheet.is_none());
    app.core().tools.save_ui().set_problem(Some(words));
    app.send(crate::queue::CoreCall::Reconcile);
    app.settle();
    assert_eq!(answer(&app, 1), HostAction::ShowSavePanel);
    app.settle();
    assert!(app.reader().sheet.is_none(), "answered, so gone");
}

/// The person's answer to the sheet showing: its button, as the window sends it.
fn answer(app: &HeadlessApp, button: u32) -> HostAction {
    app.act(UiAction::SheetAnswered {
        kind: crate::core::saving::SHEET_KIND.into(),
        id: 0,
        button,
    })
    .host
}
