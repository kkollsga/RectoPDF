//! The real installer, `recto-updater`, on stand-in bundles: a real process at a real path, a real swap, and a real kill at every step. What matters most after any of them: **there's a whole app at the app's path, and the record is as it was or put back.** The whole path with the real app is in `recto-cli`'s `restore` tests.
#![cfg(target_os = "macos")]

use recto_platform::local;
use recto_update::install::DIE_AT_ENV;
use recto_update::plan::{Files, Outcome, OutcomeKind, Plan, TestLaunch, beside_app, new_app_in, previous_app_in};
use recto_update::source::resolve;
use recto_update::stage::app_version;
use recto_update::updater::{AtLaunch, Facts, HandOffRefused, HandedOff, Host, Look, Status, Updater};
use recto_update::{APP_BUNDLE, BUNDLE_ID};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HELPER: &str = env!("CARGO_BIN_EXE_recto-updater");

fn bundle(at: &Path, version: &str) -> PathBuf {
    bundle_at(&at.join(APP_BUNDLE), version)
}

/// The same, at this very path, as a staged or kept copy's name isn't the app's.
fn bundle_at(app: &Path, version: &str) -> PathBuf {
    let app = app.to_path_buf();
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    std::fs::write(app.join("Contents/Info.plist"), format!(r#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string><key>CFBundleExecutable</key><string>RectoPDF</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#)).unwrap();
    // A real program, so the installer finds it by its path.
    std::fs::copy(HELPER, macos.join("RectoPDF")).unwrap();
    std::fs::copy(HELPER, macos.join("recto-updater")).unwrap();
    sign(&app);
    app
}

/// The installer inside an app, which is the one a plan for that app has to be run by.
fn installer_in(app: &Path) -> PathBuf {
    app.join("Contents/MacOS/recto-updater")
}

fn sign(app: &Path) {
    let output = Command::new("/usr/bin/codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(app)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

/// What the app would have put in the plan, having verified this bundle.
fn cdhash_of(app: &Path) -> String {
    let output = Command::new("/usr/bin/codesign").args(["-d", "-vvv"]).arg(app).output().unwrap();
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .find_map(|line| line.strip_prefix("CDHash="))
        .expect("signed")
        .trim()
        .to_owned()
}

/// An installed old version, a staged new one, a record, and a stand-in for the running process.
struct Bed {
    dir: recto_testpdf::TempDir,
    files: Files,
    app: PathBuf,
    plan: Plan,
}

impl Bed {
    fn new(name: &str, new_app_does: &str) -> Self {
        let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), &format!("recto-installer-{name}-"));
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
        bundle_at(&new_app_in(&beside), "0.2.0");
        let cdhash = cdhash_of(&new_app_in(&beside));
        let test = TestLaunch {
            args: vec![
                "test-app".into(),
                "0.2.0".into(),
                new_app_does.into(),
                files.work.display().to_string(),
                files.session().display().to_string(),
            ],
            env: Vec::new(),
            direct: true,
        };
        let plan = Plan {
            id: "first".into(),
            cdhash,
            team_required: false,
            any_version: false,
            development_build: false,
            team_signed: false,
            from: "0.1.0".into(),
            to: "0.2.0".into(),
            pid: 0,
            app: app.clone(),
            beside,
            root,
            quit_at_ms: recto_update::wall_ms(),
            background: true,
            quit_limit_ms: 30_000,
            start_limit_ms: 60_000,
            handed_off_tabs: None,
            open: Vec::new(),
            keep_record: false,
            test: Some(test),
        };
        Self { dir, files, app, plan }
    }

    /// What's running: a process for the installer to wait on.
    fn old_process(&self) -> Child {
        Command::new("/bin/sleep").arg("30").spawn().unwrap()
    }

    fn start(&mut self, old: u32, die_at: Option<&str>) -> Child {
        self.plan.pid = old;
        self.plan.write(&self.files.plan()).unwrap();
        let mut command = Command::new(installer_in(&self.app));
        command.arg("install").arg(self.files.plan());
        if let Some(step) = die_at {
            command.env(DIE_AT_ENV, step);
        }
        command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        command.spawn().unwrap()
    }

    /// With a process that's already gone: the installer doesn't wait.
    /// What's waiting becomes a development build of this version and bundle id, signed ad hoc, as `tools/dev-build.sh` makes them, and the plan says it's one, as the app would write it.
    fn as_a_build(&mut self, version: &str, bundle_id: &str) {
        let new = new_app_in(&self.plan.beside);
        std::fs::remove_dir_all(&new).unwrap();
        bundle_at(&new, version.rsplit_once('.').unwrap().0);
        let plist = new.join("Contents/Info.plist");
        for edit in [format!("Add :RectoDevVersion string {version}"), format!("Set :CFBundleIdentifier {bundle_id}")] {
            assert!(
                Command::new("/usr/libexec/PlistBuddy")
                    .args(["-c", &edit])
                    .arg(&plist)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        sign(&new);
        self.plan.cdhash = cdhash_of(&new);
        self.plan.to = version.to_owned();
        self.plan.any_version = true;
        self.plan.development_build = true;
        // Held to the team, as a release build's installer always is.
        self.plan.team_required = true;
        if let Some(test) = self.plan.test.as_mut() {
            test.args[1] = version.to_owned();
        }
    }

    fn switch(&self, on: Option<bool>) {
        recto_update::saved::Saved {
            dev_builds: on,
            ..recto_update::saved::Saved::default()
        }
        .write(&self.files.saved);
    }

    /// With a process that's already gone, and these set for the installer.
    fn run_with(&mut self, env: &[(&str, &str)]) -> ExitStatus {
        let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
        gone.wait().unwrap();
        self.plan.pid = gone.id();
        self.plan.write(&self.files.plan()).unwrap();
        let mut command = Command::new(installer_in(&self.app));
        command.arg("install").arg(self.files.plan());
        for (name, value) in env {
            command.env(name, value);
        }
        command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        command.status().unwrap()
    }

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
        let updater = Updater::new(
            &self.files.root,
            resolve(false, true, Some("http://127.0.0.1:9".into()), None),
            Arc::new(NoHost),
        );
        let at = updater.launched(Facts {
            version: self.installed(),
            app: self.app.clone(),
            system: "15.0".into(),
        });
        let code = updater.report()["status"].as_str().unwrap().to_owned();
        (status_named(&code, &updater), at)
    }
}

fn status_named(code: &str, updater: &Updater) -> Status {
    match code {
        "updated" => Status::Updated(updater.report()["version"].as_str().unwrap().to_owned()),
        "went_back" => Status::WentBack {
            version: String::new(),
            back_on: String::new(),
        },
        "failed" => Status::Failed {
            version: String::new(),
            why: String::new(),
        },
        _ => Status::Idle,
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        for pid in local::processes_running(&self.plan.executable()) {
            local::signal_process(pid, true);
        }
    }
}

struct NoHost;

impl Host for NoHost {
    fn look(&self) -> Look {
        Look::default()
    }
    fn hand_off(&self, _limit: Duration) -> Result<HandedOff, HandOffRefused> {
        Ok(HandedOff::default())
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

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
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
    assert!(
        !bed.files.plan().exists() && !new_app_in(&bed.plan.beside).exists(),
        "it tidies up after itself"
    );
    assert!(bed.files.before().join(".whole").exists(), "and keeps the record from before, for a while");
    assert!(
        std::fs::read_to_string(&bed.files.log).unwrap().contains("0.2.0 started"),
        "it says how it went"
    );
}

/// Failure: a new version that won't start leaves the user with nothing, or with its damage to the record. It goes back to the previous app with the same state, and says so at the next launch. Each of ending at once and hanging.
#[test]
fn a_new_app_that_wont_start_goes_back_with_the_state_intact() {
    for (name, does) in [("exits", "exits"), ("hangs", "hangs")] {
        let mut bed = Bed::new(name, does);
        // Long enough for it to get going on a loaded machine.
        bed.plan.start_limit_ms = 8_000;
        let ended = bed.run(None);
        assert!(!ended.success());
        assert!(bed.ran("0.2.0"), "it did get to run");
        assert_eq!(bed.installed(), "0.1.0", "the previous app is back at the app's path");
        assert_eq!(bed.kept(), None, "and the one that failed isn't kept");
        assert_eq!(bed.record(), "the state", "what the new version did to the record is undone");
        assert_eq!(
            std::fs::read_to_string(bed.files.session().join("journals/journal-1.log")).unwrap(),
            "the edits"
        );
        assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().kind, OutcomeKind::WentBack);
        wait_until("the old app to be opened", Duration::from_secs(20), || bed.ran("0.1.0"));
        let (status, at) = bed.launch();
        assert!(matches!(status, Status::WentBack { .. }), "{status:?}");
        assert_eq!(at, AtLaunch::GoOn);
    }
}

/// Failure: an installer killed at some step leaves half an app, no app, or a damaged record, or the next launch can't make sense of what it left. **Every step of the way through, and of the way back:** after each, there's a whole app at the app's path, the record is whole, and a launch settles it.
#[test]
fn killed_at_any_step_it_leaves_a_whole_app_and_the_whole_state() {
    for (step, expected, kept) in [
        ("old_app_gone", "0.1.0", None),
        ("copying_record", "0.1.0", None),
        ("record_copied", "0.1.0", None),
        ("still_verified", "0.1.0", None),
        ("swapped", "0.2.0", None),
        ("old_app_kept", "0.2.0", Some("0.1.0")),
        ("opened", "0.2.0", Some("0.1.0")),
    ] {
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
            assert_eq!(
                bed.kept().as_deref(),
                Some("0.1.0"),
                "{step}: the launch keeps the old app, if the installer didn't get to"
            );
            assert!(!new_app_in(&bed.plan.beside).exists(), "{step}");
        } else {
            assert!(matches!(status, Status::Failed { .. }), "{step}: {status:?}");
            assert!(!new_app_in(&bed.plan.beside).exists(), "{step}");
        }
        assert!(!bed.files.plan().exists(), "{step}: nothing's left to confuse the next one");
    }
    // The way back: the new version's failed, and the installer's killed putting things right.
    for (step, expected, record) in [
        ("going_back", "0.2.0", "damaged by the new version"),
        ("new_app_stopped", "0.2.0", "damaged by the new version"),
        ("record_restored", "0.2.0", "the state"),
        ("swapped_back", "0.1.0", "the state"),
        ("outcome_written", "0.1.0", "the state"),
    ] {
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
        } else {
            // That one's still here, not having come up. It's said as the version it is, and going back's still there for the launch guard, from the record's copy too.
            assert_eq!(status, Status::Updated("0.2.0".into()), "{step}");
            assert_eq!(bed.kept().as_deref(), Some("0.1.0"), "{step}");
            assert!(
                bed.files.before().join(".whole").exists(),
                "{step}: not cleared by a launch that hasn't come up"
            );
        }
    }
}

/// Failure: an installer killed in the middle of putting the record back leaves it half one thing and half the other for good. Killed after each file, what's left still has the copy, and the way back finishes the job from it. The review pointed out nothing killed it inside these two.
#[test]
fn killed_while_putting_the_record_back_it_can_still_finish() {
    let mut bed = Bed::new("kill-inside", "exits");
    std::fs::write(bed.files.session().join("record.b.json"), "the other slot").unwrap();
    let ended = bed.run(Some("restoring_record"));
    assert!(!ended.success());
    assert_eq!(bed.installed(), "0.2.0");
    assert!(bed.files.before().join(".whole").exists(), "the copy's still whole");
    let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    let mut plan = bed.plan.clone();
    plan.id = "second".into();
    plan.pid = gone.id();
    plan.write(&bed.files.plan()).unwrap();
    let ended = Command::new(installer_in(&bed.app))
        .arg("back")
        .arg(bed.files.plan())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!ended.success());
    assert_eq!(bed.installed(), "0.1.0");
    assert_eq!(bed.record(), "the state");
    assert_eq!(std::fs::read_to_string(bed.files.session().join("record.b.json")).unwrap(), "the other slot");
    assert_eq!(
        std::fs::read_to_string(bed.files.session().join("journals/journal-1.log")).unwrap(),
        "the edits"
    );
}

/// Failure: **two installers at once destroy the record and an app,** which the review proved in 3 runs of 4: `session/` left empty, and one version deleted. Started together on one plan, a second just waits and finds there's nothing left to do. Run a few times, as what it depends on is timing.
#[test]
fn two_installers_on_one_plan_do_it_once() {
    for attempt in 0..4 {
        let mut bed = Bed::new(&format!("two-{attempt}"), "ok");
        let mut old = bed.old_process();
        let mut first = bed.start(old.id(), None);
        let mut second = Command::new(installer_in(&bed.app))
            .arg("install")
            .arg(bed.files.plan())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        old.kill().unwrap();
        old.wait().unwrap();
        let (one, two) = (first.wait().unwrap(), second.wait().unwrap());
        assert!(
            one.success() != two.success(),
            "{attempt}: one did it, and one stood down: {one:?} {two:?}\n{}",
            std::fs::read_to_string(&bed.files.log).unwrap()
        );
        assert_eq!(bed.installed(), "0.2.0", "{attempt}");
        assert_eq!(bed.kept().as_deref(), Some("0.1.0"), "{attempt}");
        assert_eq!(bed.record(), "the state", "{attempt}");
        assert_eq!(
            std::fs::read_to_string(bed.files.session().join("journals/journal-1.log")).unwrap(),
            "the edits",
            "{attempt}"
        );
    }
}

/// Failure: an installer goes ahead while another's at work. **The lock on its own,** as the plan's name and the last look catch most of what two at once do, and neither closes the moment between a look and a swap. Held by someone else, it waits and touches nothing, and goes when it's let go. If it isn't let go, it stands down and says so.
#[test]
fn an_installer_waits_its_turn() {
    let mut bed = Bed::new("turn", "ok");
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(bed.files.installer_lock())
        .unwrap();
    holder.try_lock().unwrap();
    let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    let mut installer = bed.start(gone.id(), None);
    std::thread::sleep(Duration::from_millis(400));
    assert!(installer.try_wait().unwrap().is_none(), "it's waiting");
    assert_eq!(bed.installed(), "0.1.0");
    assert!(!bed.files.helper_pid().exists(), "and has touched nothing");
    assert!(!bed.files.before().exists());
    holder.unlock().unwrap();
    assert!(installer.wait().unwrap().success(), "{}", std::fs::read_to_string(&bed.files.log).unwrap());
    assert_eq!(bed.installed(), "0.2.0");

    let mut bed = Bed::new("turn-never", "ok");
    bed.plan.quit_limit_ms = 200;
    let holder = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(bed.files.installer_lock())
        .unwrap();
    holder.try_lock().unwrap();
    assert!(!bed.run(None).success());
    assert_eq!(bed.installed(), "0.1.0");
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "another_installer");
    assert!(new_app_in(&bed.plan.beside).exists());
}

/// Failure: an installer whose app took its plan back, and then a retry wrote another, goes ahead when the old process ends, alongside the new one. How the review's case comes about: an app that takes over 15 s to quit, then an agent asking again. The first sees the plan isn't its own, and gets out of the way while the old app's still running; the second goes ahead when it's gone.
#[test]
fn an_installer_whose_plan_was_replaced_stands_down() {
    let mut bed = Bed::new("replaced", "ok");
    let mut old = bed.old_process();
    let mut first = bed.start(old.id(), None);
    wait_until("the first", Duration::from_secs(10), || bed.files.helper_pid().exists());
    let mut later = bed.plan.clone();
    later.id = "later".into();
    later.write(&bed.files.plan()).unwrap();
    wait_until("the first to stand down", Duration::from_secs(10), || first.try_wait().unwrap().is_some());
    assert!(!first.wait().unwrap().success());
    assert_eq!(bed.installed(), "0.1.0", "and it touched nothing");
    assert_eq!(bed.record(), "the state");
    let mut second = Command::new(installer_in(&bed.app))
        .arg("install")
        .arg(bed.files.plan())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(bed.installed(), "0.1.0", "the old app's still running");
    old.kill().unwrap();
    old.wait().unwrap();
    assert!(second.wait().unwrap().success(), "{}", std::fs::read_to_string(&bed.files.log).unwrap());
    assert_eq!(bed.installed(), "0.2.0");
    assert_eq!(bed.record(), "the state");
    assert_eq!(bed.kept().as_deref(), Some("0.1.0"));
}

/// Failure: **the installer swaps in bytes it never checked:** the review put an unsigned 9.9.9 in `new/` and it was swapped in and run. In the ten minutes a by-hand install can spend waiting, anything could have changed. Each is refused before the swap, with the old app reopened and nothing of what was there run. An app that's been changed after it's checked is said so, and not tried by itself again.
#[test]
fn what_it_swaps_in_is_what_was_verified() {
    let swapped_for_another = |bed: &Bed| {
        std::fs::remove_dir_all(new_app_in(&bed.plan.beside)).unwrap();
        // Unsigned, says 9.9.9, and runs.
        let app = bundle_at(&new_app_in(&bed.plan.beside), "9.9.9");
        assert!(
            Command::new("/usr/bin/codesign")
                .arg("--remove-signature")
                .arg(app.join("Contents/MacOS/RectoPDF"))
                .status()
                .unwrap()
                .success()
        );
    };
    let changed_a_file = |bed: &Bed| {
        std::fs::write(new_app_in(&bed.plan.beside).join("Contents/MacOS/recto-updater"), "changed").unwrap();
    };
    let another_good_build = |bed: &Bed| {
        std::fs::remove_dir_all(new_app_in(&bed.plan.beside)).unwrap();
        let app = bundle_at(&new_app_in(&bed.plan.beside), "0.2.0");
        std::fs::write(app.join("Contents/Resources"), "not what was verified").unwrap();
        sign(&app);
    };
    type Change<'a> = &'a dyn Fn(&Bed);
    let cases: [(&str, Change<'_>); 3] = [
        ("another", &swapped_for_another),
        ("changed", &changed_a_file),
        ("rebuilt", &another_good_build),
    ];
    for (name, change) in cases {
        let mut bed = Bed::new(&format!("pin-{name}"), "ok");
        change(&bed);
        let ended = bed.run(None);
        assert!(!ended.success(), "{name}");
        assert_eq!(bed.installed(), "0.1.0", "{name}");
        assert!(!bed.ran("9.9.9") && !bed.ran("0.2.0"), "{name}: none of it was run");
        wait_until("the old app to reopen", Duration::from_secs(20), || bed.ran("0.1.0"));
        assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "changed_since_checked", "{name}");
        assert!(!new_app_in(&bed.plan.beside).exists(), "{name}: and it's gone");
        let (status, _) = bed.launch();
        assert!(matches!(status, Status::Failed { .. }), "{name}: {status:?}");
    }
    // A plan with no hash isn't one anything can be held to, so it's refused the same way.
    let mut bed = Bed::new("pin-none", "ok");
    bed.plan.cdhash = String::new();
    assert!(!bed.run(None).success());
    assert_eq!(bed.installed(), "0.1.0");
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "changed_since_checked");
}

/// Failure: a development build signed ad hoc, which is all a dev build ever is (owner, 2026-11-11: no keychain), is refused at the swap where the team is required, or one is taken with the owner's switch off, as a plan saying so is only a file. The switch is read again here, from the updater's own file.
#[test]
fn an_ad_hoc_build_is_taken_only_with_the_owners_switch() {
    for (switch, taken) in [(None, false), (Some(false), false), (Some(true), true)] {
        let mut bed = Bed::new(&format!("build-{switch:?}"), "ok");
        bed.as_a_build("0.1.0.1", BUNDLE_ID);
        bed.switch(switch);
        let ended = bed.run(None);
        assert_eq!(ended.success(), taken, "{switch:?}");
        if taken {
            assert_eq!(bed.installed(), "0.1.0.1");
            assert_eq!(bed.kept().as_deref(), Some("0.1.0"), "the old app's kept, as for a release");
        } else {
            assert_eq!(bed.installed(), "0.1.0", "{switch:?}");
            assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "changed_since_checked");
        }
    }
}

/// Failure (review, 2026-11-11, reproduced as `installed=0.2.0 id=com.example.Evil`): a forged plan saying "development build" gets the release's installer to swap in any app signed ad hoc, of any bundle id or any version, as an ad-hoc signature is anyone's. Each is refused at the last look, with the switch on: another app's identifier, a plan whose version isn't a build's, and (by the rule, as a test's build can't be a release) a switch that's only on in a folder the plan names.
#[test]
fn a_forged_build_plan_swaps_in_nothing() {
    let mut other = Bed::new("forged-id", "ok");
    other.as_a_build("0.1.0.1", "com.example.Evil");
    other.switch(Some(true));
    assert!(!other.run(None).success());
    assert_eq!(other.installed(), "0.1.0", "another app's identifier");
    assert_eq!(Outcome::read(&other.files.outcome()).unwrap().why, "changed_since_checked");
    let mut release = Bed::new("forged-version", "ok");
    release.as_a_build("0.1.0.1", BUNDLE_ID);
    release.switch(Some(true));
    release.plan.to = "0.2.0".into();
    assert!(!release.run(None).success());
    assert_eq!(release.installed(), "0.1.0", "not a build's version");
    // A release build reads the switch from the user's own folder, never the plan's.
    let plan = &release.plan;
    let real = Path::new("/Users/someone/Library/Application Support/RectoPDF/agent");
    assert_eq!(
        recto_update::install::switch_file(plan, true, Some(real)),
        Some(real.join("updates.json")),
        "a release build"
    );
    assert_eq!(recto_update::install::switch_file(plan, true, None), None, "no home, no switch");
    assert_eq!(
        recto_update::install::switch_file(plan, false, Some(real)),
        Some(plan.root.join("updates.json")),
        "a test's build reads its own"
    );
}

/// Failure (review round 2): an ad-hoc signature can claim any identifier (`codesign -s - -i io.github.kkollsga.RectoPDF`), so a same-user process can make a bundle that passes as "the" dev build, and a copy signed by the team takes it, crossing from a release into a state only as safe as the user account. A copy signed by the team never takes an ad-hoc build, switch or not; the same bundle over an ad-hoc copy goes in.
#[test]
fn a_team_signed_copy_never_takes_an_ad_hoc_build() {
    let mut release = Bed::new("over-a-release", "ok");
    release.as_a_build("0.1.0.1", BUNDLE_ID);
    release.switch(Some(true));
    assert!(!release.run_with(&[(recto_update::install::TEAM_OF_APP_ENV, "6GND8Z2C7Z")]).success());
    assert_eq!(release.installed(), "0.1.0", "the release stays");
    assert!(!release.ran("0.1.0.1"), "none of the build ran");
    let mut adhoc = Bed::new("over-a-build", "ok");
    adhoc.as_a_build("0.1.0.1", BUNDLE_ID);
    adhoc.switch(Some(true));
    assert!(adhoc.run_with(&[]).success());
    assert_eq!(adhoc.installed(), "0.1.0.1", "over an ad-hoc copy, it goes in");
}

/// Failure (review round 2): a plan names an app or a folder beside it that isn't the installer's own, and the installer acts on it: run from another place, or pointing at another folder, nothing's touched.
#[test]
fn an_installer_acts_only_on_its_own_app() {
    let elsewhere = Bed::new("not-its-app", "ok");
    elsewhere.plan.write(&elsewhere.files.plan()).unwrap();
    // The installer as built, in no app at all.
    assert!(!Command::new(HELPER).arg("install").arg(elsewhere.files.plan()).status().unwrap().success());
    assert_eq!(elsewhere.installed(), "0.1.0");
    assert!(Outcome::read(&elsewhere.files.outcome()).is_none(), "nothing written");
    assert!(new_app_in(&elsewhere.plan.beside).exists(), "nothing moved");
    let mut beside = Bed::new("not-its-folder", "ok");
    let other = beside.dir.join("other-beside");
    std::fs::create_dir_all(&other).unwrap();
    beside.plan.beside = other;
    assert!(!beside.run_with(&[]).success());
    assert_eq!(beside.installed(), "0.1.0");
    assert!(new_app_in(&beside_app(&beside.app).unwrap()).exists(), "nothing moved");
}

/// Failure: the first update in place of an app in /Applications is refused by macOS's App Management (EPERM at the swap) and the app's left gone, or says only that it "couldn't be replaced", which tells the owner nothing they can do. It's put back as it was, reopened, and says where to allow it, and isn't held against the version.
#[test]
fn a_swap_macos_refuses_says_where_to_allow_it() {
    let mut bed = Bed::new("eperm", "ok");
    assert!(!bed.run_with(&[(recto_update::install::SWAP_ERROR_ENV, "eperm")]).success());
    assert_eq!(bed.installed(), "0.1.0");
    wait_until("it to reopen", Duration::from_secs(20), || bed.ran("0.1.0"));
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "not_permitted");
    let updater = Updater::new(&bed.files.root, resolve(false, true, Some("http://127.0.0.1:9".into()), None), Arc::new(NoHost));
    updater.launched(Facts {
        version: "0.1.0".into(),
        app: bed.app.clone(),
        system: "15.0".into(),
    });
    let report = updater.report();
    assert!(report["words"].as_str().unwrap().contains("App Management"), "{report}");
    assert_eq!(report["failed_versions"], serde_json::json!([]), "once it's allowed, it can go: {report}");
}

/// Failure: going back by itself puts back a kept copy signed ad hoc that the owner no longer allows, where releases are held to the team. The installer refuses, and opens what's in place again.
#[test]
fn going_back_by_itself_takes_only_a_copy_it_may() {
    for (switch, back) in [(Some(false), false), (Some(true), true)] {
        let mut bed = Bed::new(&format!("back-{switch:?}"), "exits");
        // As a killed installer leaves it: the new one in place, the old one kept.
        assert!(!bed.run(Some("going_back")).success());
        assert_eq!(bed.installed(), "0.2.0");
        bed.switch(switch);
        let _ = std::fs::remove_file(bed.files.work.join("ran-0.2.0"));
        let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
        gone.wait().unwrap();
        let mut plan = bed.plan.clone();
        plan.id = format!("guard-{switch:?}");
        plan.pid = gone.id();
        plan.team_required = true;
        plan.write(&bed.files.plan()).unwrap();
        let _ = Command::new(installer_in(&bed.app))
            .arg("back")
            .arg(bed.files.plan())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        let outcome = Outcome::read(&bed.files.outcome()).unwrap();
        if back {
            assert_eq!((bed.installed().as_str(), outcome.kind), ("0.1.0", OutcomeKind::WentBack));
        } else {
            assert_eq!(bed.installed(), "0.2.0", "what's in place stays");
            assert_eq!(outcome.why, "not_one_to_go_back_to");
            wait_until("it to reopen", Duration::from_secs(20), || bed.ran("0.2.0"));
        }
    }
}

/// Failure: **the last look isn't the last thing:** with the record copied after it, a same-user racer had 316 ms with 3,000 journal files to put something else in `new/`, which a review did, and an unsigned bundle was swapped in, run and kept. Held right after the record's copied, a racer swaps in an unsigned 9.9.9, and it's refused.
#[test]
fn the_last_look_is_the_last_thing_before_the_swap() {
    let mut bed = Bed::new("racer", "ok");
    let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    bed.plan.pid = gone.id();
    bed.plan.write(&bed.files.plan()).unwrap();
    let mut installer = Command::new(installer_in(&bed.app))
        .arg("install")
        .arg(bed.files.plan())
        .env(recto_update::install::PAUSE_AT_ENV, "record_copied")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_until("the installer to reach the copy of the record", Duration::from_secs(20), || {
        bed.files.work.join("paused").exists()
    });
    std::fs::remove_dir_all(new_app_in(&bed.plan.beside)).unwrap();
    let app = bundle_at(&new_app_in(&bed.plan.beside), "9.9.9");
    assert!(
        Command::new("/usr/bin/codesign")
            .arg("--remove-signature")
            .arg(app.join("Contents/MacOS/RectoPDF"))
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(bed.files.work.join("go-on"), "").unwrap();
    assert!(!installer.wait().unwrap().success());
    assert_eq!(bed.installed(), "0.1.0", "{}", std::fs::read_to_string(&bed.files.log).unwrap());
    assert!(!bed.ran("9.9.9"));
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "changed_since_checked");
}

/// Failure: a byte changed inside a nested program passes the last look, as the review found of `recto-updater`, so an app goes in whose installer can't run and that can never update again. The outer seal only names the nested program's signature, and its pages are checked only by going all the way in: a byte a third of the way into it got past `--verify --strict`, and `--deep` refuses it.
#[test]
fn a_changed_nested_program_is_refused_at_the_swap() {
    let mut bed = Bed::new("nested", "ok");
    let nested = new_app_in(&bed.plan.beside).join("Contents/MacOS/recto-updater");
    let mut bytes = std::fs::read(&nested).unwrap();
    let middle = bytes.len() / 3;
    bytes[middle] ^= 0xFF;
    std::fs::write(&nested, bytes).unwrap();
    assert!(!bed.run(None).success());
    assert_eq!(bed.installed(), "0.1.0");
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "changed_since_checked");
}

/// Failure: a plan naming an older genuine build's hash installs a downgrade, as the hash says nothing of whether the build is the version the plan's for. The version the bundle itself says has to be the one the plan's going to.
#[test]
fn an_older_genuine_build_isnt_installed_as_a_newer_one() {
    let mut bed = Bed::new("older", "ok");
    std::fs::remove_dir_all(new_app_in(&bed.plan.beside)).unwrap();
    let older = bundle_at(&new_app_in(&bed.plan.beside), "0.1.5");
    bed.plan.cdhash = cdhash_of(&older);
    assert!(!bed.run(None).success());
    assert_eq!(bed.installed(), "0.1.0");
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "changed_since_checked");
}

/// Failure: a bundle that can't be moved gets as far as the swap, and the swap failing isn't seen for what it is. The check before downloading should have caught this one, and this is for what it can't see: the installer reopens the old app, says why, and the next launch says so to the user and doesn't try again by itself.
#[test]
fn a_swap_that_fails_says_so_and_stops() {
    use std::os::unix::fs::PermissionsExt as _;
    let mut bed = Bed::new("swap-fails", "ok");
    std::fs::set_permissions(&bed.app, std::fs::Permissions::from_mode(0o555)).unwrap();
    let ended = bed.run(None);
    std::fs::set_permissions(&bed.app, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(!ended.success());
    assert_eq!(bed.installed(), "0.1.0");
    wait_until("it to reopen", Duration::from_secs(20), || bed.ran("0.1.0"));
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().why, "swap_failed");
    let updater = Updater::new(&bed.files.root, resolve(false, true, Some("http://127.0.0.1:9".into()), None), Arc::new(NoHost));
    updater.launched(Facts {
        version: "0.1.0".into(),
        app: bed.app.clone(),
        system: "15.0".into(),
    });
    let report = updater.report();
    assert_eq!(report["status"], "failed");
    assert_eq!(report["failed_versions"][0], "0.2.0", "{report}");
    assert!(updater.notice().unwrap().text.contains("couldn't be replaced"), "it's told");
}

/// Failure: with the old app's move to be kept having failed, going back says it went back and leaves the broken version in place, and the next launch deletes the only good copy. The review proved the first half. It's still in the other half of the swap, so that's where it comes back from.
#[test]
fn when_keeping_it_failed_it_comes_back_from_where_it_is() {
    let mut bed = Bed::new("keep-failed", "exits");
    let folder = bed.plan.beside.join("previous");
    std::fs::create_dir_all(&folder).unwrap();
    assert!(Command::new("/usr/bin/chflags").arg("uchg").arg(&folder).status().unwrap().success());
    let ended = bed.run(None);
    assert!(Command::new("/usr/bin/chflags").arg("nouchg").arg(&folder).status().unwrap().success());
    assert!(!ended.success());
    assert_eq!(bed.installed(), "0.1.0", "{}", std::fs::read_to_string(&bed.files.log).unwrap());
    assert_eq!(Outcome::read(&bed.files.outcome()).unwrap().kind, OutcomeKind::WentBack);
    assert_eq!(bed.record(), "the state");

    // And a new version that comes up with the old app stuck in that other half keeps it there, and the launch moves it.
    let mut bed = Bed::new("keep-failed-ok", "ok");
    let folder = bed.plan.beside.join("previous");
    std::fs::create_dir_all(&folder).unwrap();
    assert!(Command::new("/usr/bin/chflags").arg("uchg").arg(&folder).status().unwrap().success());
    let ended = bed.run(None);
    assert!(Command::new("/usr/bin/chflags").arg("nouchg").arg(&folder).status().unwrap().success());
    assert!(ended.success());
    assert_eq!(bed.installed(), "0.2.0");
    assert_eq!(
        app_version(&new_app_in(&bed.plan.beside)).as_deref(),
        Some("0.1.0"),
        "the only way back isn't thrown away"
    );
    bed.launch();
    assert_eq!(bed.kept().as_deref(), Some("0.1.0"), "the next launch keeps it");
}

/// Failure: with nothing to go back to, going back says it did and removes what's there. It says what's so, leaves what's there, and the next launch says that too rather than "0.2.0 came back".
#[test]
fn with_nothing_to_go_back_to_it_says_so() {
    let mut bed = Bed::new("nothing-back", "ok");
    // As the launch guard would find it, only with the kept app the wrong one.
    let ended = bed.run(None);
    assert!(ended.success());
    std::fs::remove_dir_all(bed.plan.beside.join("previous")).unwrap();
    let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    let mut plan = bed.plan.clone();
    plan.id = "back".into();
    plan.pid = gone.id();
    plan.write(&bed.files.plan()).unwrap();
    let ended = Command::new(installer_in(&bed.app))
        .arg("back")
        .arg(bed.files.plan())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!ended.success());
    assert_eq!(bed.installed(), "0.2.0", "what's there is all there is");
    let outcome = Outcome::read(&bed.files.outcome()).unwrap();
    assert_eq!(outcome.kind, OutcomeKind::CouldntGoBack);
    let updater = Updater::new(&bed.files.root, resolve(false, true, Some("http://127.0.0.1:9".into()), None), Arc::new(NoHost));
    updater.launched(Facts {
        version: "0.2.0".into(),
        app: bed.app.clone(),
        system: "15.0".into(),
    });
    let notice = updater.notice().unwrap().text;
    assert!(notice.contains("no earlier version") && !notice.contains("came back"), "{notice}");
}

/// Failure: a new version that comes up having misread the record, starting empty and writing that over the user's, counts as having come up and stays, with their tabs gone. It says it restored fewer tabs than were handed off, and the installer goes back with the record as it was. One whose old app didn't say how many it handed off isn't held to it.
#[test]
fn a_new_app_that_comes_up_empty_goes_back_with_the_record_as_it_was() {
    let mut bed = Bed::new("misreads", "misreads");
    bed.plan.handed_off_tabs = Some(2);
    let ended = bed.run(None);
    assert!(!ended.success(), "{}", std::fs::read_to_string(&bed.files.log).unwrap());
    assert_eq!(bed.installed(), "0.1.0", "the previous app is back");
    assert_eq!(bed.record(), "the state", "and the record from before it, not the empty one");
    let outcome = Outcome::read(&bed.files.outcome()).unwrap();
    assert_eq!((outcome.kind, outcome.why.as_str()), (OutcomeKind::WentBack, "misread"));
    wait_until("the old app to be opened", Duration::from_secs(20), || bed.ran("0.1.0"));
    let updater = Updater::new(&bed.files.root, resolve(false, true, Some("http://127.0.0.1:9".into()), None), Arc::new(NoHost));
    updater.launched(Facts {
        version: "0.1.0".into(),
        app: bed.app.clone(),
        system: "15.0".into(),
    });
    let notice = updater.notice().unwrap().text;
    assert!(notice.contains("didn't read your last session") && notice.contains("came back"), "{notice}");

    let mut bed = Bed::new("misreads-unsaid", "misreads");
    bed.plan.handed_off_tabs = None;
    assert!(bed.run(None).success());
    assert_eq!(bed.installed(), "0.2.0", "nothing to hold it to");
}

/// Failure: a version that came up and was in use, and then kept ending, goes back with the record from before the update, throwing away whatever was done since. Going back for that leaves the record as it is, which the previous version reads, and says why.
#[test]
fn going_back_after_it_came_up_keeps_the_record_as_it_is() {
    let mut bed = Bed::new("kept-ending", "ok");
    assert!(bed.run(None).success());
    assert_eq!(bed.installed(), "0.2.0");
    std::fs::write(bed.files.session().join("record.a.json"), "a week's work on 0.2.0").unwrap();
    let mut gone = Command::new("/usr/bin/true").spawn().unwrap();
    gone.wait().unwrap();
    let plan = Plan {
        id: "back".into(),
        pid: gone.id(),
        keep_record: true,
        ..bed.plan.clone()
    };
    plan.write(&bed.files.plan()).unwrap();
    let ended = Command::new(installer_in(&bed.app))
        .arg("back")
        .arg(bed.files.plan())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!ended.success(), "it went back");
    assert_eq!(bed.installed(), "0.1.0");
    assert_eq!(bed.record(), "a week's work on 0.2.0", "the record as it is");
    let outcome = Outcome::read(&bed.files.outcome()).unwrap();
    assert_eq!((outcome.kind, outcome.why.as_str()), (OutcomeKind::WentBack, "kept_ending"));
}

/// Failure: the old app reopened in the gap, with an installer still at work, sits with its menu disabled and no timer until the next launch. The review found this could be left that way. It goes on as normal.
#[test]
fn an_app_reopened_during_an_install_goes_on_as_normal() {
    let bed = Bed::new("reopened-during", "ok");
    let plan = Plan { pid: 1, ..bed.plan.clone() };
    plan.write(&bed.files.plan()).unwrap();
    // A real process of the installer's name, for the launch to find alive.
    let mut helper = Command::new(HELPER)
        .args([
            "test-app",
            "0.0.0",
            "hangs",
            bed.files.work.to_str().unwrap(),
            bed.dir.join("nowhere").to_str().unwrap(),
        ])
        .spawn()
        .unwrap();
    std::fs::write(bed.files.helper_pid(), helper.id().to_string()).unwrap();
    let updater = Updater::new(&bed.files.root, resolve(false, true, Some("http://127.0.0.1:9".into()), None), Arc::new(NoHost));
    updater.launched(Facts {
        version: "0.1.0".into(),
        app: bed.app.clone(),
        system: "15.0".into(),
    });
    assert!(updater.menu_item().enabled);
    assert!(updater.next_wake().is_some(), "it still has a timer");
    assert_ne!(updater.report()["status"], "installing");
    assert!(bed.files.plan().exists(), "and it's left the plan alone, as there's an installer about");
    helper.kill().unwrap();
    helper.wait().unwrap();
}

/// Failure: a release with a zip the updater's reader won't take gets published, and nobody can update to it. `release-zip.sh` checks with this.
#[test]
fn the_reader_can_be_asked_about_a_zip() {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-installer-checkzip-");
    std::fs::create_dir_all(&dir).unwrap();
    let app = bundle(&dir, "0.2.0");
    let zip = dir.join("r.zip");
    assert!(
        Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--sequesterRsrc", "--keepParent"])
            .arg(&app)
            .arg(&zip)
            .status()
            .unwrap()
            .success()
    );
    let into = dir.join("out");
    std::fs::create_dir_all(&into).unwrap();
    assert!(Command::new(HELPER).arg("check-zip").arg(&zip).arg(&into).status().unwrap().success());
    assert_eq!(app_version(&into.join("RectoPDF.app")).as_deref(), Some("0.2.0"));
    assert!(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(into.join("RectoPDF.app"))
            .status()
            .unwrap()
            .success(),
        "and what comes out still verifies"
    );
    let junk = dir.join("junk.zip");
    std::fs::write(&junk, "not a zip").unwrap();
    assert!(
        !Command::new(HELPER)
            .arg("check-zip")
            .arg(&junk)
            .arg(&into)
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    std::fs::remove_dir_all(&dir).unwrap();
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
    plan.id = "guard".into();
    plan.pid = gone.id();
    plan.write(&bed.files.plan()).unwrap();
    let ended = Command::new(installer_in(&bed.app))
        .arg("back")
        .arg(bed.files.plan())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
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
    let mut second = Command::new(bed.plan.executable())
        .args([
            "test-app",
            "0.1.0",
            "hangs",
            bed.files.work.to_str().unwrap(),
            bed.dir.join("nowhere").to_str().unwrap(),
        ])
        .spawn()
        .unwrap();
    wait_until("the second app", Duration::from_secs(5), || {
        !local::processes_running(&bed.plan.executable()).is_empty()
    });
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
    let ended = Command::new(installer_in(&bed.app))
        .arg("install")
        .arg(bed.files.plan())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(ended.code(), Some(2));
    assert_eq!(bed.installed(), "0.1.0");
    assert!(new_app_in(&bed.plan.beside).exists());
    // One that makes sense and is a downgrade doesn't get to move anything either.
    let mut backwards = Plan {
        id: "backwards".into(),
        from: "0.2.0".into(),
        to: "0.1.0".into(),
        ..bed.plan.clone()
    };
    backwards.pid = 1;
    backwards.write(&bed.files.plan()).unwrap();
    assert!(
        !Command::new(installer_in(&bed.app))
            .arg("install")
            .arg(bed.files.plan())
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(bed.installed(), "0.1.0");
}
