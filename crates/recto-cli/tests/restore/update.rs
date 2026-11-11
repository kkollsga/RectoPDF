//! **The update, end to end, on the real app.** An ad-hoc-signed "old" app and a fake release with a real zip and `latest.json`, on a server on this machine, with temp state. The app does what it does for a user, by the same calls, in quiet runs. SPEC decision 24: "it should recover state so perfectly that it can update while the user is using it."
//!
//! What can't be tested here is the real signature passing: a Developer ID and Gatekeeper need a real signed release. The refusal is tested here and in `recto-update`'s own tests.

use super::{Run, assert_same_but, long_document, make_rich};
use crate::common::State;
use recto_platform::local;
use recto_update::source::{REPO_ENV, TRUST_ENV};
use recto_update::wall_ms;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const OLD: &str = "0.1.0";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
const NEW: &str = "0.1.99";

/// What differs between an old app and the new one, on purpose. Everything else is compared.
const ACROSS_AN_UPDATE: &[&str] = &["app.settings.status"];

type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// A release server on this machine.
struct Server {
    url: String,
    files: Files,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let files: Files = Arc::default();
        let served = Arc::clone(&files);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let served = Arc::clone(&served);
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 2048];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer) {
                            Ok(0) | Err(_) => return,
                            Ok(count) => request.extend_from_slice(&buffer[..count]),
                        }
                    }
                    let text = String::from_utf8_lossy(&request).to_string();
                    let path = text.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let found = served.lock().unwrap().get(&path).cloned();
                    match found {
                        Some(body) => {
                            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = stream.write_all(&body);
                        }
                        None => {
                            let _ = write!(stream, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                });
            }
        });
        Self { url, files }
    }

    fn put(&self, path: &str, body: Vec<u8>) {
        self.files.lock().unwrap().insert(path.to_owned(), body);
    }

    fn latest(&self, body: Vec<u8>) {
        self.put("/releases/latest/download/latest.json", body);
    }

    /// A release of this version, with this zip, saying this checksum.
    fn release(&self, version: &str, zip: Vec<u8>, sha: &str) {
        self.put(&format!("/releases/download/v{version}/RectoPDF-{version}.zip"), zip);
        self.latest(
            format!(r#"{{"version":"{version}","tag":"v{version}","minimum_macos":"15.0","zip":"RectoPDF-{version}.zip","sha256":"{sha}"}}"#).into_bytes(),
        );
    }
}

/// The built app, cloned to a temp folder as this version, with a test's own signature. It's the same app with another number.
fn copy_app(to_folder: &Path, version: &str) -> PathBuf {
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/build/RectoPDF.app");
    std::fs::create_dir_all(to_folder).unwrap();
    let app = to_folder.join("RectoPDF.app");
    assert!(Command::new("/bin/cp").arg("-cR").arg(&built).arg(&app).status().unwrap().success());
    let plist = app.join("Contents/Info.plist");
    // Its own bundle id, so the system never mistakes this for the real app, as it would on being opened by `open`. And never in the Dock, even for a moment.
    for edit in [
        format!("Set :CFBundleShortVersionString {version}"),
        "Set :CFBundleIdentifier io.github.kkollsga.RectoPDF.e2e".to_owned(),
        "Add :LSUIElement bool true".to_owned(),
    ] {
        assert!(
            Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", &edit])
                .arg(&plist)
                .status()
                .unwrap()
                .success()
        );
    }
    sign(&app);
    app
}

/// The outer app only. `--deep` would sign the renderer helper again without its sandbox, and its worker would then refuse to draw.
fn sign(app: &Path) {
    let output = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(app).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

fn zip_of(app: &Path) -> (Vec<u8>, String) {
    let zip = app.with_extension("zip");
    assert!(
        Command::new("/usr/bin/ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(app)
            .arg(&zip)
            .status()
            .unwrap()
            .success()
    );
    let bytes = std::fs::read(&zip).unwrap();
    let _ = std::fs::remove_file(&zip);
    let sha = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    (bytes, sha)
}

/// What's wrong with a release, for a test.
#[derive(Clone, Copy, PartialEq)]
enum Broken {
    Not,
    /// What should be the app is a program that damages the real record and ends, having shown nothing.
    DamagesTheRecordAndEnds,
    /// The app's fine, and its renderer doesn't work. It'd come up and show a window with no pages.
    NoRenderer,
    /// The real app, which ends a moment after its first frame, every launch: a crash loop that a first frame can't catch.
    EndsAfterFirstFrame,
    /// The real app, which starts empty, as if it couldn't make sense of the record, and comes up.
    MisreadsTheRecord,
}

/// All a real update needs, in temp folders that go with it.
struct Bed {
    folder: PathBuf,
    state: State,
    server: Server,
    app: PathBuf,
}

impl Bed {
    fn new() -> Option<Self> {
        super::app_path()?;
        let folder = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("recto-update-e2e-{}-{}", std::process::id(), wall_ms()));
        std::fs::create_dir_all(&folder).unwrap();
        let app = copy_app(&folder.join("install"), OLD);
        Some(Self {
            folder,
            state: State::new(),
            server: Server::start(),
            app,
        })
    }

    fn release_of(&self, version: &str, broken: Broken) {
        let staging = self.folder.join(format!("release-{version}"));
        let app = copy_app(&staging, version);
        let macos = app.join("Contents/MacOS");
        match broken {
            Broken::Not => {}
            Broken::DamagesTheRecordAndEnds => {
                std::fs::copy(macos.join("recto-updater"), macos.join("RectoPDF")).unwrap();
                sign(&app);
            }
            Broken::NoRenderer => {
                let contents = app.join("Contents");
                std::fs::copy("/usr/bin/true", contents.join("Helpers/RectoRender.app/Contents/MacOS/recto-render-service")).unwrap();
                let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/render-helper.sh");
                // Ad hoc, whatever this shell has set: a test never signs with the owner's identity.
                assert!(
                    Command::new(helper)
                        .arg("--sign")
                        .arg(&contents)
                        .env_remove("RECTO_SIGN_IDENTITY")
                        .status()
                        .unwrap()
                        .success()
                );
                sign(&app);
            }
            Broken::EndsAfterFirstFrame | Broken::MisreadsTheRecord => {
                let what = if broken == Broken::EndsAfterFirstFrame {
                    recto_update::TestBreakage::EndsAfterFirstFrame
                } else {
                    recto_update::TestBreakage::MisreadsTheRecord
                };
                std::fs::write(app.join("Contents/Resources").join(what.file()), "").unwrap();
                sign(&app);
            }
        }
        let (zip, sha) = zip_of(&app);
        self.server.release(version, zip, &sha);
        let _ = std::fs::remove_dir_all(&staging);
    }

    fn start(&self) -> Run<'_> {
        self.start_with(&[])
    }

    /// Starts the old app as `recto qa start` does, told about this bed's server. As a user's would be, but for that.
    fn start_with(&self, env: &[(&str, &str)]) -> Run<'_> {
        let mut command = self.state.command();
        command.env(REPO_ENV, &self.server.url);
        for (name, value) in env {
            command.env(name, value);
        }
        let output = command
            .args([
                "qa",
                "start",
                "--app",
                self.app.join("Contents/MacOS/RectoPDF").to_str().unwrap(),
                "--time-limit",
                "3m",
                "--json",
            ])
            .stderr(std::process::Stdio::inherit())
            .output()
            .unwrap();
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stdout)));
        assert_eq!(reply["ok"], true, "{reply}");
        let run = Run::attach(&self.state, reply["data"]["pid"].as_u64().unwrap() as u32);
        run.ok(&["qa", "scroller-style", "overlay"]);
        run
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.state.root.join("update.log")).unwrap_or_default()
    }

    fn version(&self) -> String {
        recto_update::stage::app_version(&self.app).unwrap()
    }

    fn kept(&self) -> Option<String> {
        recto_update::stage::app_version(&recto_update::plan::previous_app_in(&recto_update::plan::beside_app(&self.app).unwrap()))
    }

    fn status(&self, run: &Run<'_>) -> Value {
        run.ok(&["update", "status"])
    }

    /// Waits for `update status` to say this.
    fn wait_status(&self, run: &Run<'_>, wanted: &str) -> Value {
        let began = Instant::now();
        loop {
            let status = self.status(run);
            if status["status"] == wanted {
                return status;
            }
            assert!(began.elapsed() < Duration::from_secs(60), "waited for {wanted}: {status}\n{}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for the old process to end and a new one to answer, which is what the user sees of an update.
    fn wait_for_new_app(&self, mut old: Run<'_>) -> Run<'_> {
        let old_pid = old.pid;
        old.ended();
        let began = Instant::now();
        loop {
            let (reply, code) = self.state.json(&["status"]);
            if code == 0 && reply["ok"] == true {
                let pid = reply["data"]["pid"].as_u64().unwrap() as u32;
                if pid != old_pid && local::process_counters(old_pid).is_none() {
                    return Run::attach(&self.state, pid);
                }
                // It's still the old one: no point in waiting if it's given up.
                if pid == old_pid {
                    let (status, _) = self.state.json(&["update", "status"]);
                    assert_ne!(status["data"]["status"], "failed", "{status}\n{}", self.log());
                }
            }
            assert!(began.elapsed() < Duration::from_secs(90), "no new app: {reply}\n{}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The same, when what's wanted is one version in particular, as what comes up first may be one that gets sent back.
    fn wait_for_new_app_of<'a>(&'a self, mut old: Run<'a>, version: &str) -> Run<'a> {
        let old_pid = old.pid;
        old.ended();
        let began = Instant::now();
        loop {
            let (status, code) = self.state.json(&["update", "status"]);
            let (pid, _) = self.state.json(&["status"]);
            if code == 0 && status["ok"] == true && status["data"]["version"] == version && pid["data"]["pid"].as_u64().is_some_and(|pid| pid as u32 != old_pid)
            {
                return Run::attach(&self.state, pid["data"]["pid"].as_u64().unwrap() as u32);
            }
            assert!(began.elapsed() < Duration::from_secs(90), "{status}\n{}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Starts whatever's at the app's path as `recto qa start` does, and waits for it to be gone: for a version that ends by itself.
    fn launch_until_it_ends(&self) {
        let mut command = self.state.command();
        command.env(REPO_ENV, &self.server.url);
        let executable = self.app.join("Contents/MacOS/RectoPDF");
        let _ = command
            .args(["qa", "start", "--app", executable.to_str().unwrap(), "--time-limit", "3m", "--json"])
            .stderr(std::process::Stdio::null())
            .output()
            .unwrap();
        let began = Instant::now();
        // Or it went back, and what's running at the app's path now is the version before.
        while !local::processes_running(&executable).is_empty() && self.version() != OLD {
            assert!(began.elapsed() < Duration::from_secs(60), "it didn't end: {}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The run that's answering on this state folder, once one is.
    fn answering(&self) -> Run<'_> {
        let began = Instant::now();
        loop {
            let (status, code) = self.state.json(&["status"]);
            if code == 0 && status["ok"] == true {
                return Run::attach(&self.state, status["data"]["pid"].as_u64().unwrap() as u32);
            }
            assert!(began.elapsed() < Duration::from_secs(60), "{status}\n{}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The paths Launch Services has registered under this bed's folder, from its whole database: `lsregister -dump`, which takes some seconds.
    fn registered_here(&self) -> Vec<String> {
        let dump = Command::new(LSREGISTER).arg("-dump").output().unwrap();
        let folder = self.folder.to_str().unwrap();
        String::from_utf8_lossy(&dump.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("path:"))
            .map(str::trim)
            .filter(|path| path.starts_with(folder))
            .map(str::to_owned)
            .collect()
    }

    /// That it's the same app it was, doing what it did. A frozen one refuses a command.
    fn still_usable(&self, run: &Run<'_>) {
        assert!(local::process_counters(run.pid).is_some(), "it's still running");
        let place = run.ok(&["where"]);
        assert!(place["page"].as_u64().is_some(), "{place}");
        run.ok(&["goto", "2"]);
        run.settled();
    }
}

impl Drop for Bed {
    fn drop(&mut self) {
        for pid in local::processes_running(&self.app.join("Contents/MacOS/RectoPDF")) {
            local::signal_process(pid, true);
        }
        // `open` told the system about these. It forgets them again here, rather than at some later tidying.
        let _ = Command::new(LSREGISTER).arg("-u").arg(&self.app).status();
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// Failure: an update loses or changes anything the user can see or has changed: tabs, focus, pixel scroll, zoom, unsaved edits and their undo history, Find, Settings. **The test that matters most.** A rich state, then a real update: real zip, real server, real installer and a real swap. The whole of `qa state` is compared before and after by restore's own comparison, and the gap from quit to first correct frame is measured.
#[test]
fn an_update_in_the_middle_of_things_changes_nothing() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    assert_eq!(bed.status(&run)["build"], "test", "it's a test's run");
    let (a, b, c) = (long_document("alpha", 60), long_document("beta", 30), super::chaptered("gamma", 20, 8));
    run.ok(&["qa", "appearance", "dark"]);
    make_rich(&run, &[&a, &b, &c]);
    run.settled();
    let before = run.report();
    // Rich, by what it holds.
    assert_eq!(before["session"]["tabs"].as_array().unwrap().len(), 3);
    assert!(
        before["session"]["tabs"][0]["edits"]["undo"].as_u64().unwrap() > 0,
        "{}",
        before["session"]["tabs"][0]
    );
    assert_eq!(before["app"]["settings"]["open"], true);

    bed.release_of(NEW, Broken::Not);
    let checked = run.ok(&["update", "check"]);
    assert_eq!(checked["status"], "available", "{checked}");
    assert_eq!(checked["menu"]["title"], format!("Update to {NEW}…"));
    let answer = run.ok(&["update", "install"]);
    assert_eq!(answer["started"], true);
    let run = bed.wait_for_new_app(run);
    assert!(bed.log().contains("installer"), "{}", bed.log());
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let after = run.report();

    assert_eq!(bed.version(), NEW, "the new app is at the app's path");
    assert_eq!(bed.kept().as_deref(), Some(OLD), "the old one is kept");
    assert_eq!(after["update"]["version"], NEW);
    assert_eq!(after["update"]["kept"], OLD);
    assert_same_but(&before, &after, &[], ACROSS_AN_UPDATE);
    // The exact pixel, at the same zoom, on the first frame, and undo still works, with whose each is.
    for field in ["scroll_y", "scroll_x", "magnification"] {
        let (was, now) = (before["app"]["pages"][field].as_f64().unwrap(), after["app"]["pages"][field].as_f64().unwrap());
        assert!((was - now).abs() < 0.25, "{field}: {was} then {now}");
    }
    // The first frame was the right one: the place, with something on it, which since deep sleep can be the tab's sleep file's picture before a page is drawn. That's the gap the user sees. It isn't what counts as having come up: that's a page of the app's own drawn through the real renderer, which `a_new_app_that_cant_draw_goes_back` holds to with a renderer that can't.
    let shown = &after["record"]["first_shown"];
    assert_eq!(shown["home"], false, "{shown}");
    assert!(shown["tile_layers"].as_u64().unwrap() > 0, "something's on it: {shown}");
    assert_eq!(shown["page"], before["reader"]["page"]);
    assert!(
        (shown["scroll_y"].as_f64().unwrap() - before["reader"]["scroll_y"].as_f64().unwrap()).abs() < 0.25,
        "{shown}"
    );
    assert_eq!(after["session"]["app"]["focus"]["view"], "find");
    run.ok(&["qa", "owner-edit", "redo"]);

    // The gap, as measured from the old app being told to quit to the new one's first correct frame.
    let wait_for_last = Instant::now();
    let mut gap = Value::Null;
    while wait_for_last.elapsed() < Duration::from_secs(5) {
        gap = bed.status(&run)["last_update"]["gap_ms"].clone();
        if gap.is_u64() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(gap.is_u64(), "{}", bed.status(&run));
    eprintln!("update gap: {gap} ms from quit to the first correct frame, in a quiet run, on the app as built, under whatever load this machine has");
    let log = bed.log();
    for line in log.lines().filter(|line| line.contains("first frame") || line.contains(" started.")) {
        eprintln!("{line}");
    }
    assert!(
        log.contains("in the background"),
        "`frontmost off` means it reopens without taking the focus: {log}"
    );
    assert!(!log.contains("alpha") && !log.contains("gamma"), "no document's name in the log");
    run.quit();
}

/// Failure: a new version that won't start leaves nothing, or the user's work behind it. **The real app, with a release that damages the real record in both slots and ends:** it goes back by itself, the whole state comes back as it was, and it says so and doesn't try again by itself.
#[test]
fn a_new_app_that_wont_start_goes_back_with_the_same_state() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 40);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "17"]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:3"]);
    run.ok(&["page", "delete", "5"]);
    run.ok(&["qa", "frontmost", "off"]);
    run.settled();
    // What the journal's called is settled once something's asked to be written.
    run.ok(&["session", "save"]);
    let before = run.report();
    bed.release_of(NEW, Broken::DamagesTheRecordAndEnds);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let run = bed.wait_for_new_app(run);
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    assert_eq!(bed.version(), OLD, "the old app is back at the app's path");
    let status = bed.wait_status(&run, "went_back");
    assert_eq!(status["version"], OLD);
    assert_eq!(status["failed_versions"], json!([NEW]));
    assert!(status["words"].as_str().unwrap().contains("came back"), "{status}");
    // The one thing that's different, and on purpose: it's still on offer, as by hand it can always be tried again.
    let menu = run.ok(&["qa", "state"])["app"]["menu_bar"][0]["items"].clone();
    assert!(
        menu.as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "app.update" && item["title"] == format!("Update to {NEW}…")),
        "{menu}"
    );
    assert_same_but(&before, &run.report(), &[], &["app.settings.status", "app.menu_bar[0].items[1].title"]);
    let log = bed.log();
    assert!(log.contains("ended before it came up") && log.contains("went back to"), "{log}");
    run.quit();
}

/// Failure: a release whose renderer can't work counts as having started, because a window came up, so every copy moves to it and shows no pages. **What "come up" means, on the real app:** the release has its renderer swapped for a program that does nothing, as a library that won't load under the hardened runtime would leave it. It shows a first frame, can't draw its own test page, and goes back.
#[test]
fn a_new_app_that_cant_draw_goes_back() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "9"]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:3"]);
    run.ok(&["qa", "frontmost", "off"]);
    run.settled();
    run.ok(&["session", "save"]);
    let before = run.report();
    bed.release_of(NEW, Broken::NoRenderer);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let run = bed.wait_for_new_app_of(run, OLD);
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    assert_eq!(bed.version(), OLD);
    let status = bed.wait_status(&run, "went_back");
    assert_eq!(status["failed_versions"], json!([NEW]));
    assert_same_but(&before, &run.report(), &[], ACROSS_AN_UPDATE);
    // A first frame was up, from the tab's sleep file's picture if it had one, and that wasn't enough: it was the drawing that said no.
    let log = bed.log();
    assert!(log.contains("can't draw a page") && log.contains("couldnt_draw"), "{log}");
    assert!(!log.contains("0.1.99 started"), "never taken as having come up: {log}");
    run.quit();
}

/// Failure: a new version that keeps failing with no installer around has no way back, or the guard's never run. **The launch guard, on the real app, with what a killed installer leaves:** the new version in place that's never come up, three launches counted, the old one kept. A fourth launch goes back by itself, and the old one comes up.
#[test]
fn the_launch_guard_goes_back_by_itself() {
    let Some(bed) = Bed::new() else { return };
    // The state's there, as the old app left it.
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "9"]);
    run.settled();
    run.ok(&["session", "save"]);
    let page = run.report()["session"]["tabs"][0]["page"].clone();
    run.quit();
    // What's left: the new one in place, unproven, and the old one kept.
    let beside = recto_update::plan::beside_app(&bed.app).unwrap();
    let keep = recto_update::plan::previous_app_in(&beside);
    std::fs::create_dir_all(keep.parent().unwrap()).unwrap();
    std::fs::rename(&bed.app, &keep).unwrap();
    copy_app(bed.app.parent().unwrap(), NEW);
    let files = recto_update::plan::Files::in_state(&bed.state.root);
    let saved = recto_update::saved::Saved {
        unstarted: 3,
        proven: Some(OLD.to_owned()),
        ..recto_update::saved::Saved::default()
    };
    saved.write(&files.saved);

    // It ends by itself, without saying it's ready.
    let mut command = bed.state.command();
    command.env(REPO_ENV, &bed.server.url);
    let reply = command
        .args([
            "qa",
            "start",
            "--app",
            bed.app.join("Contents/MacOS/RectoPDF").to_str().unwrap(),
            "--time-limit",
            "3m",
            "--json",
        ])
        .stderr(std::process::Stdio::null())
        .output()
        .unwrap();
    let began = Instant::now();
    while bed.version() != OLD {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "{}\n{}",
            String::from_utf8_lossy(&reply.stdout),
            bed.log()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let run = loop {
        let (status, code) = bed.state.json(&["status"]);
        if code == 0 && status["ok"] == true {
            break Run::attach(&bed.state, status["data"]["pid"].as_u64().unwrap() as u32);
        }
        assert!(began.elapsed() < Duration::from_secs(60), "{status}\n{}", bed.log());
        std::thread::sleep(Duration::from_millis(20));
    };
    run.settled();
    let status = bed.wait_status(&run, "went_back");
    assert_eq!(status["version"], OLD);
    assert_eq!(status["failed_versions"], json!([NEW]));
    assert!(status["words"].as_str().unwrap().contains("came back"), "{status}");
    assert_eq!(run.report()["session"]["tabs"][0]["page"], page, "with the state as it was");
    assert!(bed.log().contains("going back"), "{}", bed.log());
    run.quit();
}

/// Failure: an app quit and opened again at once finds the old one still letting go of the record's lock, decides it's a second copy, and does no updates for that whole run. It waits for the lock as long as reading the record does. Held for 300 ms by someone else, as a process that's just ended can, and then let go.
#[test]
fn a_quick_reopen_isnt_a_second_copy() {
    let Some(bed) = Bed::new() else { return };
    bed.state.paths.prepare().unwrap();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(bed.state.root.join("run/session.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        drop(lock);
    });
    let run = bed.start();
    releaser.join().unwrap();
    let status = bed.status(&run);
    assert_eq!(status["second_copy"], false, "{status}");
    assert!(status["next_wake_in_ms"].is_u64(), "it has its timer: {status}");
    run.quit();
}

/// Failure: a lost or damaged record turns the switch back on, against what the user chose, and the app replaces itself. It's in the updater's own file.
#[test]
fn a_lost_record_doesnt_turn_automatic_updates_back_on() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    run.ok(&["settings", "--auto-update", "off"]);
    let began = Instant::now();
    while recto_update::saved::Saved::read(&bed.state.root.join("updates.json")).auto_update != Some(false) {
        assert!(began.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
    }
    run.quit();
    let _ = std::fs::remove_dir_all(bed.state.root.join("session"));
    let run = bed.start();
    assert_eq!(run.ok(&["settings"])["auto_update"], false, "still off");
    run.quit();
}

/// Failure: an installer killed partway leaves half an app, or loses the state. Killed right after the swap: the new version's in place and not running. Starting it by hand finds all of it, says it updated, and tidies what the installer left.
#[test]
fn an_installer_killed_after_the_swap_loses_nothing() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start_with(&[(recto_update::install::DIE_AT_ENV, "swapped")]);
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "9"]);
    run.settled();
    run.ok(&["page", "rotate", "2", "--by", "90"]);
    run.settled();
    run.ok(&["session", "save"]);
    let before = run.report();
    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let mut run = run;
    let pid = run.pid;
    let began = Instant::now();
    while local::process_counters(pid).is_some() {
        assert!(began.elapsed() < Duration::from_secs(60), "{}", bed.log());
        std::thread::sleep(Duration::from_millis(20));
    }
    run.ended();
    // The installer's died, with the swap done and nothing started.
    let plan = recto_update::plan::Files::in_state(&bed.state.root).plan();
    let began = Instant::now();
    while !(bed.version() == NEW && !recto_update::plan::helper_alive(&recto_update::plan::Files::in_state(&bed.state.root))) {
        assert!(began.elapsed() < Duration::from_secs(30), "{}", bed.log());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(plan.exists(), "what it left");
    assert!(local::processes_running(&bed.app.join("Contents/MacOS/RectoPDF")).is_empty());
    let run = bed.start();
    run.settled();
    assert_eq!(bed.status(&run)["status"], "updated");
    assert_same_but(&before, &run.report(), &[], ACROSS_AN_UPDATE);
    assert_eq!(bed.kept().as_deref(), Some(OLD), "the launch kept the old app, as the installer didn't get to");
    assert!(!plan.exists());
    run.quit();
}

/// Failure: an app left running with its window closed gets a window put up behind whatever the user's doing, by an update they didn't ask to see. It stays closed across an update, comes back when it's asked for, and with everything in it, and any other launch opens one.
#[test]
fn a_closed_window_stays_closed_across_an_update() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "9"]);
    run.settled();
    run.ok(&["qa", "window", "close"]);
    assert_eq!(run.ok(&["qa", "state"])["app"]["window_open"], false);
    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let run = bed.wait_for_new_app(run);
    assert_eq!(bed.version(), NEW, "the installer heard it had come up, with no frame to show");
    assert_eq!(run.ok(&["qa", "state"])["app"]["window_open"], false, "still closed");
    run.ok(&["qa", "window", "open"]);
    run.settled();
    assert_eq!(run.report()["session"]["tabs"][0]["page"], 8, "with everything in it");
    run.ok(&["qa", "window", "close"]);
    run.quit();
    let run = bed.start();
    assert_eq!(run.ok(&["qa", "state"])["app"]["window_open"], true, "any other launch opens one");
    run.quit();
}

/// Failure (owner, 2026-11-07: "It's crucial that app state is perfectly recovered before and after update"): an app the user had put in the Dock, or hidden, comes back on the screen after an update they didn't watch, as neither was in the record. Both come back exactly so, with everything in them, and any other launch shows them. And neither copy of the app the updater keeps beside it is registered with Launch Services, not even by a scan of the folder (PLAN: "Whether the hidden `previous/` copy gets registered"), which would put a second RectoPDF in Open With.
#[test]
fn a_minimised_window_and_a_hidden_app_stay_so_across_an_update() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "9"]);
    run.settled();
    run.ok(&["qa", "minimise", "on"]);
    run.ok(&["qa", "hide", "on"]);
    run.ok(&["qa", "frontmost", "off"]);
    let before = run.report();
    assert_eq!((&before["app"]["window"]["minimised"], &before["app"]["hidden"]), (&json!(true), &json!(true)));
    assert_eq!(before["session"]["app"]["window"]["minimised"], true);
    assert_eq!(before["session"]["app"]["app_hidden"], true);
    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let run = bed.wait_for_new_app(run);
    assert_eq!(bed.version(), NEW);
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let after = run.report();
    assert_eq!(after["app"]["window"]["minimised"], true, "still in the Dock");
    assert_eq!(after["app"]["hidden"], true, "still hidden");
    assert_same_but(&before, &after, &[], ACROSS_AN_UPDATE);

    // What's kept beside the app isn't an app to the system, even once the folder's been looked through. A scan does find the renderer's helper inside it, which is background-only with no documents, so it's never in Open With, and its name is sealed by the app's signature.
    let install = bed.app.parent().unwrap();
    assert!(Command::new(LSREGISTER).args(["-R", "-f"]).arg(install).status().unwrap().success());
    let registered = bed.registered_here();
    assert!(
        registered
            .iter()
            .all(|path| !path.contains(".RectoPDF-update") || path.contains("/Contents/Helpers/")),
        "only the app at its own path: {registered:?}"
    );
    assert!(bed.kept().is_some(), "and the old one is still kept: {registered:?}");

    run.quit();
    let run = bed.start();
    let shown = run.report();
    assert_eq!(shown["app"]["window"]["minimised"], false, "any other launch shows it");
    assert_eq!(shown["app"]["hidden"], false, "and the app");
    assert_eq!(shown["session"]["app"]["app_hidden"], false, "and the record says so");
    run.quit();
}

/// Failure: a new version that comes up and then crashes, every time, stays, as nothing after the first frame was watched, and the user can't use the app. **The real app, with a release that ends a moment after its first frame:** it comes up, so the installer's done, and then each run ends early; the launch after three in a row goes back by itself, and the state comes back as it was, the record as the new version left it.
#[test]
fn a_new_version_that_keeps_ending_goes_back_by_itself() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 30);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "12"]);
    run.settled();
    run.ok(&["qa", "owner-edit", "rotate:3"]);
    run.ok(&["qa", "frontmost", "off"]);
    run.settled();
    run.ok(&["session", "save"]);
    let before = run.report();
    bed.release_of(NEW, Broken::EndsAfterFirstFrame);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let mut arrived = bed.wait_for_new_app_of(run, NEW);
    // It's come up, for the installer, which has finished. And then it ends.
    let began = Instant::now();
    while local::process_counters(arrived.pid).is_some() {
        assert!(began.elapsed() < Duration::from_secs(60), "{}", bed.log());
        std::thread::sleep(Duration::from_millis(20));
    }
    arrived.ended();
    assert_eq!(bed.version(), NEW, "the installer took it as having come up");
    let files = recto_update::plan::Files::in_state(&bed.state.root);
    let mut launches = 0;
    while bed.version() != OLD {
        launches += 1;
        assert!(launches <= recto_update::updater::EARLY_ENDS_LIMIT + 1, "it never went back: {}", bed.log());
        bed.launch_until_it_ends();
        // A launch that's going back has left the installer its plan, and the version before is in place a moment later.
        let began = Instant::now();
        while (files.plan().exists() || recto_update::plan::helper_alive(&files)) && bed.version() != OLD {
            assert!(began.elapsed() < Duration::from_secs(60), "{}", bed.log());
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert_eq!(launches, recto_update::updater::EARLY_ENDS_LIMIT, "the launch after three in a row goes back");
    let run = bed.answering();
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let status = bed.wait_status(&run, "went_back");
    assert_eq!(status["version"], OLD);
    assert_eq!(status["failed_versions"], json!([NEW]), "not tried by itself again");
    // The one difference, on purpose: it's still on offer, as by hand it can always be tried again.
    assert_same_but(&before, &run.report(), &[], &["app.settings.status", "app.menu_bar[0].items[1].title"]);
    let log = bed.log();
    assert!(log.contains("ended early") && log.contains("went back to"), "{log}");
    run.quit();
}

/// Failure: a new version that misreads the record and starts empty counts as having come up, as its first frame and its own page were fine, and stays, with the user's tabs gone and its empty record written over theirs. **The real app, with a release that starts empty:** it comes up with fewer tabs than were handed off, and the installer goes back with the record as it was, and says why.
#[test]
fn a_new_version_that_misreads_the_record_goes_back() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "9"]);
    run.settled();
    run.ok(&["qa", "frontmost", "off"]);
    run.settled();
    run.ok(&["session", "save"]);
    let before = run.report();
    bed.release_of(NEW, Broken::MisreadsTheRecord);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let run = bed.wait_for_new_app_of(run, OLD);
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    assert_eq!(bed.version(), OLD);
    let status = bed.wait_status(&run, "went_back");
    assert_eq!(status["failed_versions"], json!([NEW]));
    let notice = crate::common::app::wait_for(&run, |state| state["app"]["copy_notice"]["text"].is_string());
    let words = notice["app"]["copy_notice"]["text"].as_str().unwrap();
    assert!(words.contains("didn't read your last session"), "{words}");
    assert_same_but(&before, &run.report(), &[], &["app.settings.status", "app.menu_bar[0].items[1].title"]);
    assert!(bed.log().contains("fewer tabs"), "{}", bed.log());
    run.quit();
}

/// Failure (PLAN, "A check by hand with the window closed says its result only in Settings"): Check for Updates… chosen with the window closed says how it went nowhere the user can see. It's an alert, app-modal, in Rust's words, until it's answered; with the window open it's the margin's, as before.
#[test]
fn a_check_from_the_menu_with_no_window_says_so_in_an_alert() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    run.ok(&["qa", "window", "close"]);
    bed.server.latest(
        format!(
            r#"{{"version":"{OLD}","tag":"v{OLD}","minimum_macos":"15.0","zip":"RectoPDF-{OLD}.zip","sha256":"{}"}}"#,
            "a".repeat(64)
        )
        .into_bytes(),
    );
    run.ok(&["qa", "menu", "app.update"]);
    let shown = crate::common::app::wait_for(&run, |state| state["app"]["sheet"]["kind"] == "update_alert");
    let message = shown["app"]["sheet"]["message"].as_str().unwrap();
    assert!(message.starts_with("You're up to date"), "{message}");
    assert_eq!(shown["app"]["window_open"], false);
    run.ok(&["qa", "sheet-answer", "first"]);
    let answered = run.report();
    assert!(answered["app"]["sheet"].get("kind").is_none(), "{answered}");
    // With the window open, the margin says it, and nothing's put over the page.
    run.ok(&["qa", "window", "open"]);
    run.settled();
    run.ok(&["qa", "menu", "app.update"]);
    let noticed = crate::common::app::wait_for(&run, |state| state["app"]["copy_notice"]["text"].is_string());
    assert!(noticed["app"]["sheet"].get("kind").is_none(), "{noticed}");
    run.quit();
}

/// Failure: a bad release gets anywhere near the app quitting, or a refused update leaves it frozen. Each way a release can be wrong: a garbled description, a downgrade, a wrong checksum and a corrupt zip. The app goes on as it was throughout, as the same process, and the old app's still what's installed.
#[test]
fn a_bad_release_never_touches_the_running_app() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();

    bed.server.latest(b"{ this isn't a release".to_vec());
    let status = run.ok(&["update", "check"]);
    assert_eq!(status["status"], "check_failed", "{status}");
    assert_eq!(status["menu"]["title"], "Check for Updates…");

    bed.server.latest(
        format!(
            r#"{{"version":"0.0.9","tag":"v0.0.9","minimum_macos":"15.0","zip":"RectoPDF-0.0.9.zip","sha256":"{}"}}"#,
            "a".repeat(64)
        )
        .into_bytes(),
    );
    assert_eq!(run.ok(&["update", "check"])["status"], "up_to_date", "a downgrade's no offer");
    let (reply, code) = bed.state.json(&["update", "install"]);
    assert_ne!(code, 0, "{reply}");
    assert_eq!(reply["error"]["code"], "nothing_on_offer");

    // A real zip, and a description that says another checksum.
    let staging = bed.folder.join("r");
    let (zip, sha) = zip_of(&copy_app(&staging, NEW));
    bed.server.release(NEW, zip.clone(), &"0".repeat(64));
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let status = bed.wait_status(&run, "failed");
    assert!(status["words"].as_str().unwrap().contains("isn't the file"), "{status}");
    bed.still_usable(&run);

    // A zip damaged in the middle, with a checksum that matches it.
    let mut damaged = zip.clone();
    let middle = damaged.len() / 2;
    damaged[middle..middle + 4096].fill(0xA5);
    let damaged_sha: String = Sha256::digest(&damaged).iter().map(|byte| format!("{byte:02x}")).collect();
    assert_ne!(damaged_sha, sha);
    bed.server.release(NEW, damaged, &damaged_sha);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    bed.wait_status(&run, "failed");
    bed.still_usable(&run);
    assert_eq!(bed.version(), OLD);
    assert_eq!(bed.kept(), None);
    assert!(run.report()["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 1));
    run.quit();
}

/// Failure: an ad-hoc build passes the real rule, when a run asks for it. This is the refusal, on the real app and a real zip: the Developer ID team and Gatekeeper passing on a good build stay unproven until there's a real signed release.
#[test]
fn the_real_rule_refuses_an_ad_hoc_build() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start_with(&[(TRUST_ENV, "developer-id")]);
    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let status = bed.wait_status(&run, "failed");
    assert!(status["words"].as_str().unwrap().contains("signed by RectoPDF's developer"), "{status}");
    assert_eq!(status["failed_versions"], json!([NEW]), "not tried again by itself");
    assert_eq!(bed.version(), OLD);
    assert!(local::process_counters(run.pid).is_some());
    run.quit();
}

/// Failure: an update goes over an open sheet or a running command, or doesn't say what it's waiting for, or never goes when that's done. **A busy app waits, says so, and then goes,** by the same calls as the sheet opening and closing.
#[test]
fn a_busy_app_waits_and_then_goes() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "7"]);
    run.settled();
    run.ok(&["qa", "sheet", "open"]);
    let before = run.report();
    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let status = bed.wait_status(&run, "waiting");
    assert!(status["words"].as_str().unwrap().contains("sheet"), "{status}");
    assert_eq!(status["menu"]["enabled"], false);
    let began = Instant::now();
    loop {
        let notice = run.report()["app"]["copy_notice"]["text"].as_str().map(str::to_owned);
        if notice.as_deref().is_some_and(|text| text.contains("as soon as this is done")) {
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(10), "the user is told it's waiting");
        std::thread::sleep(Duration::from_millis(20));
    }
    // Waiting isn't frozen: it's still in use.
    run.ok(&["goto", "8"]);
    run.settled();
    assert_eq!(bed.version(), OLD);
    assert!(local::process_counters(run.pid).is_some());
    run.ok(&["qa", "sheet", "closed"]);
    let run = bed.wait_for_new_app(run);
    run.settled();
    assert_eq!(bed.version(), NEW);
    assert_eq!(run.report()["session"]["tabs"][0]["page"], 7, "page 8, counted from 0, as it was when it went");
    let _ = before;
    run.quit();
}

/// Failure: a hand-off that's good and then can't go on leaves the app frozen for good. **The real app with its installer unable to start:** the state's written and read back and the app's frozen, then it has to come back. What the user does and what an agent does both work again, and it says it failed.
#[test]
fn a_cancelled_hand_off_doesnt_leave_the_app_frozen() {
    let Some(bed) = Bed::new() else { return };
    let helper = bed.app.join("Contents/MacOS/recto-updater");
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    // There, as the check before the download looks for it, and it can't be run.
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o644)).unwrap();
    run.ok(&["update", "install"]);
    let status = bed.wait_status(&run, "failed");
    assert!(status["words"].as_str().unwrap().contains("installer"), "{status}");
    bed.still_usable(&run);
    run.ok(&["qa", "click-row", "0"]);
    assert_eq!(bed.version(), OLD);
    run.quit();
}

/// Failure: the 30-minute rule is broken on the real app: it installs while the user's in it, or never does. With the clock moved on, not half an hour's sleep, and the timer's own call. A background app comes back without taking the focus.
#[test]
fn an_automatic_update_waits_for_thirty_quiet_minutes() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "11"]);
    run.settled();
    run.ok(&["qa", "frontmost", "off"]);
    run.settled();
    assert_eq!(
        run.ok(&["settings"])["auto_update"],
        true,
        "on by default in anything that behaves as a release"
    );
    bed.release_of(NEW, Broken::Not);
    run.ok(&["qa", "note-input", "now"]);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["qa", "update-tick", "now"]);
    let status = bed.status(&run);
    assert_eq!(status["status"], "available", "just touched: nothing happens");
    let wait = status["install_look_in_ms"].as_u64().unwrap();
    assert!(wait > 25 * 60_000 && wait <= 30 * 60_000, "and it looks again when it could be time: {status}");
    // Twenty minutes on, and they're back.
    run.ok(&["qa", "update-clock", "1200000"]);
    run.ok(&["qa", "note-input", "now"]);
    run.ok(&["qa", "update-tick", "now"]);
    assert_eq!(bed.status(&run)["status"], "available");
    assert!(local::process_counters(run.pid).is_some());
    // Half an hour and more with nobody there.
    run.ok(&["qa", "update-clock", "1900000"]);
    run.ok(&["qa", "update-tick", "now"]);
    let run = bed.wait_for_new_app(run);
    run.settled();
    assert_eq!(bed.version(), NEW);
    assert_eq!(run.report()["session"]["tabs"][0]["page"], 10);
    assert_eq!(
        run.report()["session"]["app"]["focus"]["app_frontmost"],
        false,
        "and it's still in the background"
    );
    assert!(bed.log().contains("in the background"), "{}", bed.log());
    run.quit();
}

/// Failure: an agent's installing or turning off updates isn't in the log of what changed things, which is where a person looks for what an agent did. That the refusals are logged too is in `a_development_build_only_asks_and_installing_needs_full`.
#[test]
fn what_an_agent_changes_about_updates_goes_in_the_changes_log() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    run.ok(&["settings", "--auto-update", "off"]);
    run.ok(&["settings"]);
    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let run = bed.wait_for_new_app(run);
    run.settled();
    let changes = std::fs::read_to_string(&bed.state.paths.log).unwrap();
    let lines: Vec<Value> = changes.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert!(lines.iter().any(|line| line["cmd"] == "update_install" && line["outcome"] == "ok"), "{changes}");
    assert!(
        lines
            .iter()
            .any(|line| line["cmd"] == "settings" && line["outcome"] == "ok" && line["details"]["auto_update"] == "off"),
        "{changes}"
    );
    assert!(
        !lines.iter().any(|line| line["cmd"] == "settings" && line["details"]["auto_update"].is_null()),
        "asking changes nothing: {changes}"
    );
    run.quit();
}

/// Failure: a build with no test server behaves as a release: it checks by itself, replaces itself or reaches the real server from a quiet run. And an agent below Full can install. Each is refused, and logged.
#[test]
fn a_development_build_only_asks_and_installing_needs_full() {
    let Some(_) = super::app_path() else { return };
    let state = State::new();
    let Some(run) = Run::on(&state) else { return };
    let status = run.ok(&["update", "status"]);
    assert_eq!(status["build"], "development");
    assert_eq!(status["next_wake_in_ms"], Value::Null, "no timer at all");
    assert_eq!(status["auto_update"], false);
    let checked = run.ok(&["update", "check"]);
    assert_eq!(checked["status"], "check_failed", "a quiet run reaches no real server: {checked}");
    let (reply, _) = state.json(&["update", "install"]);
    assert_eq!(reply["error"]["code"], "development_build", "{reply}");
    let shown = run.ok(&["qa", "state"]);
    let row = &shown["app"]["menu_bar"][0]["items"];
    assert!(
        row.as_array()
            .unwrap()
            .iter()
            .any(|item| item["id"] == "app.update" && item["title"] == "Check for Updates…"),
        "{row}"
    );
    assert_eq!(run.ok(&["settings"])["auto_update"], false);

    state.set_level(recto_agent::Level::Control);
    run.ok(&["update", "status"]);
    let (reply, code) = state.json(&["update", "install"]);
    assert_eq!(reply["error"]["code"], "permission_denied", "{reply}");
    assert_ne!(code, 0);
    let (reply, _) = state.json(&["settings", "--auto-update", "on"]);
    assert_eq!(reply["error"]["code"], "permission_denied", "{reply}");
    assert_eq!(run.ok(&["settings"])["auto_update"], false, "refused means unchanged");
    state.set_level(recto_agent::Level::ReadOnly);
    let (reply, _) = state.json(&["update", "check"]);
    assert_eq!(reply["error"]["code"], "permission_denied", "{reply}");
    state.set_level(recto_agent::Level::Full);
    let log = std::fs::read_to_string(&state.paths.other_log).unwrap();
    assert!(
        log.contains("update_install") && log.contains("update_check") && log.contains("settings") && log.contains("update_status"),
        "each is logged: {log}"
    );
    run.quit();
}

/// Failure: Settings isn't there, what it says isn't what's in force, or the choice is lost at a restart. And what's on it draws: real shots, in `RECTO_UPDATE_SHOTS` if it names a folder, of the menu both ways and the window, light and dark.
#[test]
fn settings_and_the_menu_say_how_things_stand() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    let shots = std::env::var_os("RECTO_UPDATE_SHOTS").map(PathBuf::from);
    let shot = |verb: &str, name: &str| {
        if let Some(folder) = &shots {
            let out = folder.join(format!("{name}.png"));
            let _ = std::fs::remove_file(&out);
            run.ok(&["qa", verb, "--out", out.to_str().unwrap()]);
        }
    };
    let menu = |run: &Run<'_>| run.ok(&["qa", "state"])["app"]["menu_bar"][0]["items"].clone();
    let item = |items: &Value, id: &str| items.as_array().unwrap().iter().find(|item| item["id"] == id).unwrap().clone();

    run.ok(&["qa", "menu", "file.settings"]);
    assert_eq!(
        run.ok(&["qa", "state"])["app"]["settings"]["open"],
        true,
        "File > Settings… opens it, by the owner's word"
    );
    run.ok(&["qa", "settings", "close"]);
    run.ok(&["qa", "menu", "app.settings"]);
    assert_eq!(run.ok(&["qa", "state"])["app"]["settings"]["open"], true, "the menu item opens it");
    for look in ["light", "dark"] {
        run.ok(&["qa", "appearance", look]);
        shot("shot-menu", &format!("menu-check-{look}"));
        shot("shot-settings", &format!("settings-{look}"));
    }
    assert_eq!(item(&menu(&run), "app.settings")["key"], ",");
    assert_eq!(item(&menu(&run), "app.update")["title"], "Check for Updates…");
    // A check from the menu, by the item itself, on its own thread: it says how it went, quietly.
    bed.server.latest(
        format!(
            r#"{{"version":"{OLD}","tag":"v{OLD}","minimum_macos":"15.0","zip":"RectoPDF-{OLD}.zip","sha256":"{}"}}"#,
            "a".repeat(64)
        )
        .into_bytes(),
    );
    run.ok(&["qa", "menu", "app.update"]);
    bed.wait_status(&run, "up_to_date");
    let began = Instant::now();
    loop {
        let notice = run.report()["app"]["copy_notice"]["text"].as_str().map(str::to_owned);
        if notice.as_deref().is_some_and(|text| text.starts_with("You're up to date")) {
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(10), "it says so");
        std::thread::sleep(Duration::from_millis(20));
    }
    let threads = run.report()["memory"]["threads"].as_u64().unwrap();
    bed.release_of(NEW, Broken::Not);
    run.ok(&["qa", "menu", "app.update"]);
    bed.wait_status(&run, "available");
    // The status is the updater's at once, and the menu is the window's once it's been told, on its own thread: a moment later, which the menu is waited for.
    let began = Instant::now();
    while item(&menu(&run), "app.update")["title"] != format!("Update to {NEW}…") {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the menu offers the update: {}",
            item(&menu(&run), "app.update")
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(item(&menu(&run), "app.update")["enabled"], true);
    for look in ["light", "dark"] {
        run.ok(&["qa", "appearance", look]);
        shot("shot-menu", &format!("menu-update-{look}"));
        shot("shot-settings", &format!("settings-offer-{look}"));
    }
    // Nothing's left running after a check, whatever it found.
    std::thread::sleep(Duration::from_millis(200));
    assert!(run.report()["memory"]["threads"].as_u64().unwrap() <= threads + 2, "threads gone");

    // The switch: in the window, by the switch, and the command agrees with it. And after a restart.
    let on = |run: &Run<'_>| run.ok(&["qa", "state"])["app"]["settings"]["rows"][0]["on"].clone();
    assert_eq!(on(&run), true);
    run.ok(&["qa", "settings", "update.automatic"]);
    assert_eq!(on(&run), false, "flipped by the switch itself");
    assert_eq!(run.ok(&["settings"])["auto_update"], false);
    run.ok(&["settings", "--auto-update", "on"]);
    assert_eq!(on(&run), true, "and the window follows the command");
    run.ok(&["settings", "--auto-update", "off"]);
    let was = run.report()["app"]["settings"]["x"].clone();
    run.quit();
    let run = bed.start();
    run.settled();
    let now = run.ok(&["qa", "state"]);
    assert_eq!(now["app"]["settings"]["open"], true, "the window comes back");
    assert_eq!(now["app"]["settings"]["x"], was);
    assert_eq!(now["app"]["settings"]["rows"][0]["on"], false, "off stays off");
    run.ok(&["qa", "settings", "close"]);
    assert_eq!(run.ok(&["qa", "state"])["app"]["settings"]["open"], false);
    run.quit();
}

/// **This measures, and asserts only that a timer's armed in one run and not in the other.** Wake-ups, CPU, threads and footprint are printed for a person: they vary too much with load to hold a test to, and a thread count at one moment read 9 and 10 at the same point. That a check leaves no thread of its own is asserted in `recto-update`, in `a_check_leaves_no_thread_behind`. Only with `RECTO_UPDATE_MEASURE` set, as it takes over a minute, and on a release build if the numbers are to mean anything.
#[test]
fn measure_what_the_updater_costs_at_rest() {
    if std::env::var_os("RECTO_UPDATE_MEASURE").is_none() {
        return;
    }
    let watch = |run: &Run<'_>| {
        std::thread::sleep(Duration::from_secs(5));
        let before = local::process_counters(run.pid).unwrap();
        std::thread::sleep(Duration::from_secs(20));
        let after = local::process_counters(run.pid).unwrap();
        let report = run.report();
        (
            after.wakeups - before.wakeups,
            (after.cpu_ns - before.cpu_ns) / 1_000_000,
            report["memory"]["threads"].as_u64().unwrap(),
            report["memory"]["footprint_bytes"].as_u64().unwrap(),
            report["update"]["next_wake_in_ms"].clone(),
        )
    };
    let Some(bed) = Bed::new() else { return };
    let dev_state = State::new();
    let dev = Run::on(&dev_state).unwrap();
    let dev_numbers = watch(&dev);
    dev.quit();
    let run = bed.start();
    bed.release_of(NEW, Broken::Not);
    run.ok(&["settings", "--auto-update", "off"]);
    let armed_before_a_check = watch(&run);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    let armed_numbers = watch(&run);
    eprintln!(
        "over 20 s at rest, wake-ups, CPU ms, threads, footprint bytes, next wake in ms:\n  nothing armed: {dev_numbers:?}\n  timer armed, before any check: {armed_before_a_check:?}\n  an offer waiting and a check done: {armed_numbers:?}"
    );
    assert!(armed_numbers.4.as_u64().is_some(), "and there's a timer armed");
    assert_eq!(dev_numbers.4, Value::Null, "with none for a development build");
    run.quit();
}
