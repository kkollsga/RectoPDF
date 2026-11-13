//! ⌘K's palette, as an agent runs it: `recto palette --run` keeps to each command's level. Twin of `recto-cli`'s app test of the same name.

use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;

/// Failure: `recto palette --run` runs a row at the palette's own level rather than the command's, so one at Read-only changes something, or the row's step is
/// the owner's, not the agent's. The update's row is held by [`the_palettes_update_is_the_updaters_command_at_the_agents_level`]. (The app test `the_palettes_run_keeps_to_each_commands_level`, which this
/// replaces.)
#[test]
fn the_palettes_run_keeps_to_each_commands_level() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("numbered.pdf", 4);
    app.open(&file);
    let zoomed = app
        .agent(Level::Control, r#"{"cmd":"palette","query":"zoom in","run":true}"#)
        .expect("zoom in at Control");
    assert_eq!(zoomed["ran"]["id"], "view.zoom_in", "{zoomed}");
    let rotated = app
        .agent(Level::Control, r#"{"cmd":"palette","query":"rotate all pages right","run":true}"#)
        .expect("rotate at Control");
    assert_eq!(rotated["ran"]["id"], "rotate.right", "{rotated}");
    app.settle();
    let edits = app.agent(Level::Control, r#"{"cmd":"edits"}"#).expect("edits");
    assert_eq!(edits["edited"], true, "{edits}");
    // Below Full, `discard` throws away only agents' steps: it going through says the step is the agent's, as `page rotate` makes it, and not the owner's.
    app.agent(Level::Control, r#"{"cmd":"discard"}"#).expect("the step is the agent's");
    let listed = app.agent(Level::ReadOnly, r#"{"cmd":"palette","query":"zoom"}"#).expect("listing is reading");
    assert!(listed["items"].as_array().is_some_and(|items| !items.is_empty()), "{listed}");
    app.agent(Level::ReadOnly, r#"{"cmd":"palette","query":"zoom in","run":true}"#)
        .expect_err("running is doing");
}

/// Failure (R2.3, the coordinator's ruling of 2026-11-13): the palette's update item runs as something other than the command the updater's state gives it, at the
/// agent's level: an agent at Control installs an update through it, as `update install --now` needs Full, or is refused the check `update check` lets it
/// make, or one at Read-only does either. *Check for Updates…* is `update check` (Control), *Update to X…* `update install --now` (Full).
#[test]
fn the_palettes_update_is_the_updaters_command_at_the_agents_level() {
    use crate::testkit::update_bed::{NEW, OLD, UpdateBed};
    let disk = crate::testkit::headless::Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let file = disk.pdf("alpha.pdf", 3);
    let app = disk.builder().update(&bed).start();
    app.open(&file);
    let run = |level: Level, query: &str| app.agent(level, &format!(r#"{{"cmd":"palette","query":"{query}","run":true}}"#));
    let refused = |result: Result<serde_json::Value, recto_agent::AgentError>| result.map(drop).map_err(|error| error.code.to_string());
    assert_eq!(
        refused(run(Level::ReadOnly, "check for updates")),
        Err("permission_denied".into()),
        "below Control"
    );
    let checked = run(Level::Control, "check for updates").expect("the check at Control");
    assert_eq!(checked["ran"]["id"], "app.update", "{checked}");
    bed.releases.offer(NEW);
    app.agent(Level::Full, r#"{"cmd":"update_check"}"#).expect("checked");
    app.settle();
    let direct = refused(app.agent(Level::Control, r#"{"cmd":"update_install","zip":null,"now":true}"#));
    assert_eq!(direct, Err("permission_denied".into()), "`update install --now` needs Full");
    assert_eq!(
        refused(run(Level::Control, "update to")),
        direct,
        "the palette's Update to X… is refused as that command is"
    );
    assert_eq!(refused(run(Level::ReadOnly, "update to")), Err("permission_denied".into()), "below Control");
}

/// Failure (rule 6 and the levels, review of 2026-11-13): an agent's `palette --run` of a row with no command of its own ran as the person, through the
/// window's own control, so an agent at Full skipped the refusal a document opened with its password gives every agent's command, and made the owner's edit:
/// Rotate in edit mode rotated a protected page, and Find opened on it with the person's passwords. An agent's action is an agent's whatever route it takes:
/// a row is the agent's own command, refused as that command is (edit mode's Rotate is `page rotate --pages N`, refused on a protected tab with its own
/// code), and nothing about the document changes; nor does File > Close by the palette close over the owner's edits.
#[test]
fn an_agents_palette_run_never_runs_as_the_person() {
    let app = HeadlessApp::start();
    let file = app.disk().file("locked.pdf", &super::files::locked_with("right", 3));
    app.core().open_file(file);
    app.settle();
    let id = app.reader().password_prompt.expect("it asks for its password").id;
    app.core().password_answered(id, Some("right".into()));
    app.settle();
    app.person(r#"{"cmd":"edit_mode","set":"on"}"#);
    app.settle();
    assert!(app.reader().edit_mode, "the person's edit mode");
    let before = app.session_json();
    // Edit mode's Rotate by the palette is `page rotate --pages N` as the agent, refused as that command is on a protected tab, with its own code.
    let direct = app.agent(Level::Full, r#"{"cmd":"page_rotate","tab":"active","pages":"1","degrees":90}"#);
    let by_palette = app.agent(Level::Full, r#"{"cmd":"palette","query":"rotate this page right","run":true}"#);
    assert_eq!(
        (
            direct.map(drop).map_err(|error| error.code.to_string()),
            by_palette.map(drop).map_err(|error| error.code.to_string())
        ),
        (Err("protected".to_string()), Err("protected".to_string())),
        "refused the same way"
    );
    for query in ["rotate this page right", "find…"] {
        let line = format!(r#"{{"cmd":"palette","query":"{query}","run":true}}"#);
        let result = app.agent(Level::Full, &line);
        app.settle();
        assert!(result.is_err(), "{query}: an agent's palette run of the person's control: {result:?}");
    }
    assert!(!app.reader().edited, "nothing rotated");
    assert_eq!(app.session_json()["find"], before["find"], "Find not opened on it");
    // With the owner's edit there, File > Close by the palette is refused as an agent's `close` over the owner's edits is, and the tab stays.
    app.act(crate::core::controls::UiAction::Menu { id: "rotate.right".into() });
    app.settle();
    assert!(app.reader().edited, "the owner's edit");
    let closed = app.agent(Level::Full, r#"{"cmd":"palette","query":"close","run":true}"#);
    app.settle();
    assert!(closed.is_err(), "{closed:?}");
    assert_eq!(app.tabs().len(), 1, "the tab and its edit stay");
}
