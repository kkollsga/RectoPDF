//! What an agent may do after a restart: levels come from their own file, never the record, and the owner's edits stay the owner's. Twin of `recto-cli`'s restore
//! test of the same name.

use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;
use recto_control::levels;

/// Failure: restoring gives an agent more than it had: a look at Find's text below Control, or a way to throw away the owner's edits by restoring over them, or a
/// level from the record. And a session's ownership rule stops holding once it's been restarted: below Full an agent still can't throw away the owner's edits,
/// and it can its own. (The app test `levels_and_ownership_hold_after_a_restart`, which this replaces.)
#[test]
fn levels_and_ownership_hold_after_a_restart() {
    let app = HeadlessApp::start();
    let (a, b) = (app.disk().pdf("owners.pdf", 8), app.disk().pdf("agents.pdf", 8));
    app.open(&a);
    app.person(r#"{"cmd":"page_rotate","pages":"1","degrees":90}"#);
    app.settle();
    app.open(&b);
    app.agent(Level::Control, r#"{"cmd":"page_rotate","pages":"1","degrees":90}"#)
        .expect("the agent's rotation");
    app.person(r#"{"cmd":"find","query":"something private"}"#);
    app.settle();
    let (_, disk) = app.quit();
    let app = disk.start();
    // The level an agent has is what its file says, as the socket's server reads it for every command.
    levels::store(app.disk().paths(), Level::Control).expect("the level's file");
    let level = || levels::current(app.disk().paths());
    assert_eq!(level(), Level::Control);
    assert_eq!(app.agent(level(), r#"{"cmd":"where"}"#).expect("where")["tab"], 2);
    // The agent's own.
    app.agent(level(), r#"{"cmd":"discard"}"#).expect("an agent throws away its own step");
    // The owner's.
    app.agent(level(), r##"{"cmd":"tap","tab":"#1"}"##).expect("tap");
    app.settle();
    let refused = app.agent(level(), r#"{"cmd":"discard"}"#).expect_err("not the owner's edits below Full");
    assert_eq!(refused.exit, recto_agent::exit::PERMISSION, "{refused:?}");
    let refused = app
        .agent(level(), r#"{"cmd":"session_restore"}"#)
        .expect_err("restoring over the owner's edits");
    assert_eq!(refused.exit, recto_agent::exit::PERMISSION, "{refused:?}");
    // A level is only ever in its own file: a record saying otherwise is nothing, as an unknown field.
    assert_eq!(levels::show(app.disk().paths(), None)["level"], "control");
    assert_eq!(
        app.agent(level(), r#"{"cmd":"session_show"}"#).expect("show")["find"]["query"],
        "something private"
    );
    levels::store(app.disk().paths(), Level::ReadOnly).expect("the level's file");
    let show = app.agent(level(), r#"{"cmd":"session_show"}"#).expect("show at Read-only");
    assert!(show["find"].get("query").is_none(), "not below Control: {show}");
    assert_eq!(show["find"]["has_text"], true);
    assert!(app.agent(level(), r#"{"cmd":"session_save"}"#).is_err(), "saving the session is doing");
    levels::store(app.disk().paths(), Level::Off).expect("the level's file");
    assert!(app.agent(level(), r#"{"cmd":"session_show"}"#).is_err(), "nothing at Off");
}
