//! Saving a document the person opened with its password: nothing saves into a protected file yet, so the window says so before they try, and an agent is
//! refused as for anything read with the password.

use super::files::locked_allowing;
use crate::testkit::headless::HeadlessApp;
use lopdf::encryption::Permissions;
use recto_agent::Level;

/// A protected file with only printing and copying allowed, opened with its password and edited by the person.
fn opened_and_edited() -> (HeadlessApp, std::path::PathBuf, Vec<u8>) {
    let app = HeadlessApp::start();
    let file = app
        .disk()
        .file("locked.pdf", &locked_allowing("right", 3, Permissions::PRINTABLE | Permissions::COPYABLE));
    let original = std::fs::read(&file).expect("the file");
    app.act(crate::core::controls::UiAction::Open {
        path: file.clone().display().to_string(),
    });
    app.settle();
    let id = app.reader().password_prompt.expect("it asks for its password").id;
    app.core().password_answered(id, Some("right".into()));
    app.settle();
    app.core().perform("rotate.right".into());
    app.settle();
    assert!(app.reader().edited, "the person's edit");
    (app, file, original)
}

/// Failure (found building R2.3): Save is on for a document the person opened with its password, and choosing it fails with the agents' refusal ("nothing
/// read from it is given to an agent"), as the window's Save ran as an agent at Full. The writer can't save into the protected file (it refuses it, and writes
/// nothing), and a copy without the password would take its protection off unasked, so Save and Save As are off, the close prompt offers only Cancel and Don't
/// Save, all in the person's own words, and the file is left as it was.
#[test]
fn a_document_opened_with_its_password_says_it_cant_be_saved_and_isnt_touched() {
    let (app, file, original) = opened_and_edited();
    let reader = app.reader();
    assert!(!reader.save_enabled, "Save is off");
    assert!(!app.core().menu_item_enabled("file.save".into()), "and its menu item");
    assert!(
        !app.core().menu_item_enabled("file.save_as".into()),
        "and Save As's: a copy would be unprotected"
    );
    let prompt = app.core().close_prompt();
    let choices: Vec<_> = prompt.buttons.iter().map(|button| button.choice).collect();
    assert_eq!(
        choices,
        [crate::saving::CloseChoice::Cancel, crate::saving::CloseChoice::DontSave],
        "closing can't offer to save"
    );
    assert!(prompt.informative.contains("protected by a password"), "and says why: {}", prompt.informative);
    assert!(
        !prompt.informative.to_lowercase().contains("agent"),
        "in the person's words: {}",
        prompt.informative
    );
    app.core().perform("file.save".into());
    app.settle();
    assert!(app.core().tools.save_ui().problem().is_none(), "nothing was tried");
    assert_eq!(std::fs::read(&file).expect("the file"), original, "and the file is as it was");
}

/// Failure: an agent's `save` of a document the person opened with its password goes through, or says something other than that it's protected.
#[test]
fn an_agents_save_of_a_document_opened_with_its_password_is_refused_as_protected() {
    let (app, file, original) = opened_and_edited();
    let refused = app.agent(Level::Full, r##"{"cmd":"save","tab":"#1"}"##).expect_err("refused");
    assert_eq!(refused.code, "protected");
    assert_eq!(std::fs::read(&file).expect("the file"), original);
}
