//! **A quiet run only uses files in its own folders** (2026-11-13), in the app: the headless kit's state folder is a test one, as a quiet run's is, so
//! its session is confined whatever the launch says, and each route a file reaches the app by is refused outside the temp folders.

use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;
use recto_platform::fresh::FreshDir;
use std::path::Path;

/// A private folder outside every temp folder, holding a real PDF: in `/var/tmp`, which no quiet run's rule counts as a temp folder (only the system's own
/// and `/tmp` are), and whose name isn't hidden, so what refuses a file there is the quiet run's rule and not a file name's.
fn outside(app: &HeadlessApp) -> FreshDir {
    let dir = FreshDir::new_in(Path::new("/private/var/tmp"), "recto-quiet-outside-", "").unwrap();
    assert!(
        !recto_control::quiet::temp_folders().iter().any(|temp| dir.path().starts_with(temp)),
        "/var/tmp is outside the temp folders"
    );
    let inside = app.disk().pdf("secret.pdf", 2);
    std::fs::copy(&inside, dir.path().join("secret.pdf")).unwrap();
    dir
}

/// Failure: an app on a test state folder opens a file outside the temp folders, by an agent's `open` at Full, by the person's Open (a drop, the
/// Finder, Open Recent are that open), or as a file named at launch, so a quiet run started at Full reads the user's PDF. None makes a tab, the person is
/// told why in the session's words, and a file in the run's own folder still opens.
#[test]
fn every_route_to_an_open_is_held_to_the_run() {
    let app = HeadlessApp::start();
    assert!(
        app.core().host.session.with_session(|session| session.confinement().folders().is_some()),
        "the kit's state folder is a test one"
    );
    let outside = outside(&app);
    let secret = outside.path().join("secret.pdf");
    let refused = app.recto(Level::Full, &["open", secret.to_str().unwrap()]).unwrap_err();
    assert_eq!(refused.code, "outside_quiet_run", "{}", refused.message);
    app.open(&secret);
    assert!(app.tabs().is_empty(), "the person's open made a tab");
    assert_eq!(
        crate::lock(&app.core().window.open_failure).as_deref(),
        Some(recto_session::confined::OUTSIDE_WORDS),
        "the person is told why"
    );
    let mine = app.disk().pdf("mine.pdf", 2);
    app.open(&mine);
    assert_eq!(app.tabs().len(), 1);
    drop(app.quit());

    let launched = HeadlessApp::builder().open(&secret).start();
    assert!(launched.tabs().is_empty(), "a file named at launch was opened");
    drop(launched.quit());
}

/// Failure: the app's own commands (a build's zip for `update install`, a QA verb's `--out`) reach a file outside the run's folders, as the session's are
/// held not to: a development build at Full in a quiet run reads any zip, or a shot is written over a file of the user's.
#[test]
fn the_apps_own_commands_are_held_to_the_run() {
    let app = HeadlessApp::start();
    let outside = outside(&app);
    for words in [
        vec![
            "update".to_owned(),
            "install".to_owned(),
            outside.path().join("build.zip").to_str().unwrap().to_owned(),
        ],
        vec![
            "qa".to_owned(),
            "shot".to_owned(),
            "--out".to_owned(),
            outside.path().join("shot.png").to_str().unwrap().to_owned(),
        ],
    ] {
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let refused = app.recto(Level::Full, &words).unwrap_err();
        assert_eq!(refused.code, "outside_quiet_run", "{words:?}: {}", refused.message);
    }
    let left: Vec<_> = std::fs::read_dir(outside.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(left, ["secret.pdf"]);
    drop(app.quit());
}

/// Failure: the save Save As's sheet makes (`qa save-as` drives it, behind QA's own wall) writes outside the run's folders when it's given a path there,
/// the session's wall being the line that holds it whatever route the sheet's answer came by. Sent as the window's action, past QA's wall: nothing is
/// written outside.
#[test]
fn save_as_outside_the_run_writes_nothing() {
    let app = HeadlessApp::start();
    app.open(&app.disk().pdf("own.pdf", 2));
    let outside = outside(&app);
    let tab = app.session_json()["active"].as_u64().expect("a tab") as u32;
    app.act(crate::core::controls::UiAction::SaveAs {
        tab,
        path: outside.path().join("copy.pdf").to_string_lossy().into_owned(),
    });
    app.wait_until("the save to be over", || {
        app.core().tools.save_ui.running.load(std::sync::atomic::Ordering::SeqCst) == 0
    });
    app.settle();
    assert!(!outside.path().join("copy.pdf").exists(), "a save was written outside the run's folders");
    drop(app.quit());
}

/// Failure: `qa save-as` writes outside the run's folders through a link in a temp folder that leads out, as the path's name is what's checked and not
/// where it leads. Refused, and nothing is written where the link leads.
#[test]
fn qa_save_as_through_a_link_out_is_refused() {
    let app = HeadlessApp::start();
    app.open(&app.disk().pdf("own.pdf", 2));
    let outside = outside(&app);
    let link = app.disk().files().join("leads-out");
    std::os::unix::fs::symlink(outside.path(), &link).unwrap();
    let refused = app.recto(Level::Full, &["qa", "save-as", link.join("copy.pdf").to_str().unwrap()]).unwrap_err();
    assert_eq!(refused.code, "outside_quiet_run", "{refused:?}");
    app.settle();
    assert!(!outside.path().join("copy.pdf").exists(), "a save was written where the link leads");
    drop(app.quit());
}
