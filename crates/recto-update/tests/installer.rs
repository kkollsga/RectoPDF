//! The real installer, `recto-updater`, on stand-in bundles: a real process at a real path, a real swap, and a real kill at every step. What matters most after any of them: **there's a whole app at the app's path, and the record is as it was or put back.** The whole path with the real app is in `recto-cli`'s `restore` tests.
#![cfg(target_os = "macos")]

use recto_platform::local;
use recto_update::install::DIE_AT_ENV;
use recto_update::plan::{Files, Outcome, OutcomeKind, Plan, TestLaunch, beside_app, new_app_in, previous_app_in};
use recto_update::source::resolve;
use recto_update::stage::app_version;
use recto_update::updater::{AtLaunch, Facts, HandOffRefused, Host, Look, Status, Updater};
use recto_update::{APP_BUNDLE, BUNDLE_ID};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HELPER: &str = env!("CARGO_BIN_EXE_recto-updater");

fn bundle(at: &Path, version: &str) -> PathBuf {
    let app = at.join(APP_BUNDLE);
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::write(app.join("Contents/Info.plist"), format!(r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string><key>CFBundleExecutable</key><string>RectoPDF</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#)).unwrap();
    // A real program, so the installer finds it by its path.
    std::fs::copy(HELPER, macos.join("RectoPDF")).unwrap();
    std::fs::copy(HELPER, macos.join("recto-updater")).unwrap();
    app
}

/// An installed old version, a staged new one, a record, and a stand-in for the running process.
struct Bed {
    dir: PathBuf,
    files: Files,
    app: PathBuf,
    plan: Plan,
}

impl Bed {
    fn new(name: &str, new_app_does: &str) -> Self {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-installer-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("state");
        local::ensure_private_dir(&root).unwrap();
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();
        std::fs::create_dir_all(files.session().join("journals")).unwrap();
        std::fs::write(files.session().join("record.a.json"), "the state").unwrap();
        std::fs::write(files.session().join("journals/journal-1.log"), "the edits").unwrap();
        let install = dir.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let app = bundle(&install, "0.1.0");
        let beside = beside_app(&app).unwrap();
        std::fs::create_dir_all(new_app_in(&beside).parent().unwrap()).unwrap();
        bundle(new_app_in(&beside).parent().unwrap(), "0.2.0");
        let test = TestLaunch { args: vec!["test-app".into(), "0.2.0".into(), new_app_does.into(), files.work.display().to_string(), files.session().display().to_string()], env: Vec::new(), direct: true };
        let plan = Plan { from: "0.1.0".into(), to: "0.2.0".into(), pid: 0, app: app.clone(), beside, root, quit_at_ms: recto_update::wall_ms(), background: true, quit_limit_ms: 5_000, start_limit_ms: 10_000, test: Some(test) };
        Self { dir, files, app, plan }
    }

    /// What's running: a process for the installer to wait on.
    fn old_process(&self) -> Child {
        Command::new("/bin/sleep").arg("30").spawn().unwrap()
    }

    fn start(&mut self, old: u32, die_at: Option<&str>) -> Child {
        self.plan.pid = old;
        self.plan.write(&self.files.plan()).unwrap();
        let mut command = Command::new(HELPER);
        command.arg("install").arg(self.files.plan());
        if let Some(step) = die_at {
            command.env(DIE_AT_ENV, step);
        }
        command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        command.spawn().unwrap()
    }

    /// With a process that's already gone: the installer doesn't wait.
    fn run(&mut self, die_at: Option<&str>) -> ExitStatus {
        let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
        gone.wait().unwrap();
        let old = gone.id();
        self.start(old, die_at).wait().unwrap()
    }

    fn installed(&self) -> String {
        app_version(&self.app).expect("there's a whole app at the app's path")
    }

    fn record(&self) -> String {
        std::fs::read_to_string(self.files.session().join("record.a.json")).unwrap()
    }

    fn kept(&self) -> Option<String> {
        app_version(&previous_app_in(&self.plan.beside))
    }

    fn ran(&self, version: &str) -> bool {
        self.files.work.join(format!("ran-{version}")).exists()
    }

    /// A launch of whichever version's in place, as it would say what became of things.
    fn launch(&self) -> (Status, AtLaunch) {
        let updater = Updater::new(&self.files.root, resolve(false, true, Some("http://127.0.0.1:9".into()), None), Arc::new(NoHost));
        let at = updater.launched(Facts { version: self.installed(), app: self.app.clone(), system: "15.0".into() });
        let code = updater.report()["status"].as_str().unwrap().to_owned();
        (status_named(&code, &updater), at)
    }
}

fn status_named(code: &str, updater: &Updater) -> Status {
    match code {
        "updated" => Status::Updated(updater.report()["version"].as_str().unwrap().to_owned()),
        "went_back" => Status::WentBack { version: String::new(), back_on: String::new() },
        "failed" => Status::Failed { version: String::new(), why: String::new() },
        _ => Status::Idle,
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        for pid in local::processes_running(&self.plan.executable()) {
            local::signal_process(pid, true);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct NoHost;

impl Host for NoHost {
    fn look(&self) -> Look {
        Look::default()
    }
    fn hand_off(&self, _limit: Duration) -> Result<(), HandOffRefused> {
        Ok(())
    }
    fn quit_now(&self) -> bool {
        false
    }
    fn cancelled(&self) {}
    fn changed(&self) {}
    fn frontmost(&self) -> bool {
        false
    }
    fn relaunch(&self) -> TestLaunch {
        TestLaunch::default()
    }
}

fn wait_until(what: &str, limit: Duration, done: impl Fn() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < limit, "waited for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Failure: the new version starts while the old one's still running, which finds the record's lock held and starts empty. Or the swap doesn't leave the new app in place and the old one kept. The heart of it: the installer waits for that one process to end.
#[test]
fn it_waits_for_the_old_process_then_swaps_and_keeps_the_old_app() {
    let mut bed = Bed::new("order", "ok");
    let mut old = bed.old_process();
    let mut installer = bed.start(old.id(), None);
    // As long as the old one's running, nothing moves.
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(bed.installed(), "0.1.0");
    assert!(!bed.ran("0.2.0"), "the new app started before the old one ended");
    old.kill().unwrap();
    old.wait().unwrap();
    let ended = installer.wait().unwrap();
    assert!(ended.success(), "{ended:?}: {}", std::fs::read_to_string(&bed.files.log).unwrap());
    assert_eq!(bed.installed(), "0.2.0");
    assert_eq!(bed.kept().as_deref(), Some("0.1.0"), "the previous app is kept");
    assert!(bed.ran("0.2.0"));
    assert_eq!(bed.record(), "the state");
    assert!(!bed.files.plan().exists() && !bed.files.before().exists() && !new_app_in(&bed.plan.beside).exists(), "it tidies up after itself");
    assert!(std::fs::read_to_string(&bed.files.log).unwrap().contains("first frame"), "it says how it went");
}

/// Failure: a new version that won't start leaves the user with nothing, or with its damage to the record. It goes back to the previous app with the same state, and says so at the next launch. Each of ending at once and hanging.
#[test]
fn a_new_app_that_wont_start_goes_back_with_the_state_intact() {
    for (name, does) in [("exits", "exits"), ("hangs", "hangs")] {
        let mut bed = Bed::new(name, does);
        bed.plan.start_limit_ms = 2_000;
        let ended = bed.run(None);
        assert!(!ended.success());
        assert!(bed.ran("0.2.0"), "it did get to run");
        assert_eq!(bed.installed(), "0.1.0", "the previous app is back at the app's path");
        assert_eq!(bed.kept(), None, "and the one that failed isn't kept");
        assert_eq!(bed.record(), "the state", "what the new version did to the record is undone");
        assert_eq!(std::fs::read_to_string(bed.files.session().join("journals/journal-1.log")).unwrap(), "the edits");
        assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().kind, OutcomeKind::WentBack);
        assert!(bed.ran("0.1.0"), "and it's been opened");
        let (status, at) = bed.launch();
        assert!(matches!(status, Status::WentBack { .. }), "{status:?}");
        assert_eq!(at, AtLaunch::GoOn);
    }
}

/// Failure: an installer killed at some step leaves half an app, no app, or a damaged record, or the next launch can't make sense of what it left. **Every step of the way through, and of the way back:** after each, there's a whole app at the app's path, the record is whole, and a launch settles it.
#[test]
fn killed_at_any_step_it_leaves_a_whole_app_and_the_whole_state() {
    for (step, expected, kept) in [("old_app_gone", "0.1.0", None), ("record_copied", "0.1.0", None), ("swapped", "0.2.0", None), ("old_app_kept", "0.2.0", Some("0.1.0")), ("opened", "0.2.0", Some("0.1.0"))] {
        let mut bed = Bed::new(&format!("kill-{step}"), "ok");
        let ended = bed.run(Some(step));
        assert!(!ended.success(), "{step}: it was to die there");
        assert_eq!(bed.installed(), expected, "{step}: the whole app at the app's path");
        assert_eq!(bed.kept().as_deref(), kept, "{step}");
        assert_eq!(bed.record(), "the state", "{step}");
        let (status, at) = bed.launch();
        assert_eq!(at, AtLaunch::GoOn, "{step}");
        if expected == "0.2.0" {
            assert_eq!(status, Status::Updated("0.2.0".into()), "{step}");
            assert_eq!(bed.kept().as_deref(), Some("0.1.0"), "{step}: the launch keeps the old app, if the installer didn't get to");
            assert!(!new_app_in(&bed.plan.beside).exists(), "{step}");
        } else {
            assert!(matches!(status, Status::Failed { .. }), "{step}: {status:?}");
            assert!(!new_app_in(&bed.plan.beside).exists(), "{step}");
        }
        assert!(!bed.files.plan().exists(), "{step}: nothing's left to confuse the next one");
    }
    // The way back: the new version's failed, and the installer's killed putting things right.
    for (step, expected, record) in [("going_back", "0.2.0", "damaged by the new version"), ("new_app_stopped", "0.2.0", "damaged by the new version"), ("record_restored", "0.2.0", "the state"), ("swapped_back", "0.1.0", "the state"), ("outcome_written", "0.1.0", "the state")] {
        let mut bed = Bed::new(&format!("back-{step}"), "exits");
        let ended = bed.run(Some(step));
        assert!(!ended.success(), "{step}");
        assert_eq!(bed.installed(), expected, "{step}: a whole app");
        assert_eq!(bed.record(), record, "{step}");
        if expected == "0.2.0" {
            assert_eq!(bed.kept().as_deref(), Some("0.1.0"), "{step}: and the old app's there to go back to");
            assert!(bed.files.before().join(".whole").exists(), "{step}: and the record's copy");
        }
        let (status, at) = bed.launch();
        assert_eq!(at, AtLaunch::GoOn, "{step}");
        if expected == "0.1.0" {
            assert_eq!(bed.record(), "the state", "{step}");
            assert!(!matches!(status, Status::Updated(_)), "{step}: {status:?}");
        }
    }
}

/// Failure: a new version that keeps failing with no installer around, after one was killed, has no way back. The guard at launch starts the installer to go back, with what's kept, and the record that was copied is put back.
#[test]
fn going_back_with_no_install_before_it_works() {
    let mut bed = Bed::new("back-alone", "exits");
    // As a killed installer leaves it: the new one in place, the old one kept, the record copied.
    let ended = bed.run(Some("going_back"));
    assert!(!ended.success());
    assert_eq!(bed.installed(), "0.2.0");
    let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    let mut plan = bed.plan.clone();
    plan.pid = gone.id();
    plan.write(&bed.files.plan()).unwrap();
    let ended = Command::new(HELPER).arg("back").arg(bed.files.plan()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap();
    assert!(!ended.success(), "it ends saying it went back");
    assert_eq!(bed.installed(), "0.1.0");
    assert_eq!(bed.record(), "the state");
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().kind, OutcomeKind::WentBack);
}

/// Failure: a process that doesn't end gets replaced anyway, or the installer waits for ever. A hand-off that didn't lead to a quit stands the installer down with nothing changed.
#[test]
fn an_app_that_doesnt_end_is_not_replaced() {
    let mut bed = Bed::new("stays", "ok");
    bed.plan.quit_limit_ms = 300;
    let mut old = bed.old_process();
    let ended = bed.start(old.id(), None).wait().unwrap();
    assert!(!ended.success());
    assert_eq!(bed.installed(), "0.1.0");
    assert!(new_app_in(&bed.plan.beside).exists(), "still staged");
    let outcome = Outcome::read(&bed.files.outcome()).unwrap();
    assert_eq!((outcome.kind, outcome.why.as_str()), (OutcomeKind::NotInstalled, "old_app_stayed"));
    old.kill().unwrap();
    old.wait().unwrap();
}

/// Failure: an app that took its plan back, because a hand-off was cancelled after the installer started, still gets replaced when it quits later on. The plan going is what stands the installer down.
#[test]
fn a_plan_taken_back_stands_the_installer_down() {
    let mut bed = Bed::new("withdrawn", "ok");
    let mut old = bed.old_process();
    let mut installer = bed.start(old.id(), None);
    wait_until("the installer", Duration::from_secs(5), || bed.files.helper_pid().exists());
    std::fs::remove_file(bed.files.plan()).unwrap();
    old.kill().unwrap();
    old.wait().unwrap();
    assert!(!installer.wait().unwrap().success());
    assert_eq!(bed.installed(), "0.1.0");
    assert!(!bed.ran("0.2.0"));
}

/// Failure: someone reopening the app in the moment it was gone has it replaced under them. It isn't the installer's to stop: nothing's replaced and it says so.
#[test]
fn an_app_opened_again_in_the_gap_is_left_alone() {
    let mut bed = Bed::new("reopened", "ok");
    let mut second = Command::new(bed.plan.executable()).args(["test-app", "0.1.0", "hangs", bed.files.work.to_str().unwrap(), bed.dir.join("nowhere").to_str().unwrap()]).spawn().unwrap();
    wait_until("the second app", Duration::from_secs(5), || !local::processes_running(&bed.plan.executable()).is_empty());
    let ended = bed.run(None);
    assert!(!ended.success());
    assert_eq!(bed.installed(), "0.1.0");
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "app_reopened");
    assert!(new_app_in(&bed.plan.beside).exists());
    second.kill().unwrap();
    second.wait().unwrap();
}

/// Failure: a damaged plan moves something. The installer takes nothing from a file that doesn't make sense.
#[test]
fn a_damaged_plan_moves_nothing() {
    let bed = Bed::new("damaged", "ok");
    local::write_private_file(&bed.files.plan(), b"{\"from\":\"nonsense\"").unwrap();
    let ended = Command::new(HELPER).arg("install").arg(bed.files.plan()).stderr(std::process::Stdio::null()).status().unwrap();
    assert_eq!(ended.code(), Some(2));
    assert_eq!(bed.installed(), "0.1.0");
    assert!(new_app_in(&bed.plan.beside).exists());
    // One that makes sense and is a downgrade doesn't get to move anything either.
    let mut backwards = Plan { from: "0.2.0".into(), to: "0.1.0".into(), ..bed.plan.clone() };
    backwards.pid = 1;
    backwards.write(&bed.files.plan()).unwrap();
    assert!(!Command::new(HELPER).arg("install").arg(bed.files.plan()).status().unwrap().success());
    assert_eq!(bed.installed(), "0.1.0");
}
