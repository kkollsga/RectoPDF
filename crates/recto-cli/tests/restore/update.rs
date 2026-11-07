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
        self.latest(format!(r#"{{"version":"{version}","tag":"v{version}","minimum_macos":"15.0","zip":"RectoPDF-{version}.zip","sha256":"{sha}"}}"#).into_bytes());
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
    for edit in [format!("Set :CFBundleShortVersionString {version}"), "Set :CFBundleIdentifier io.github.kkollsga.RectoPDF.e2e".to_owned(), "Add :LSUIElement bool true".to_owned()] {
        assert!(Command::new("/usr/libexec/PlistBuddy").args(["-c", &edit]).arg(&plist).status().unwrap().success());
    }
    sign(&app);
    app
}

fn sign(app: &Path) {
    let output = Command::new("/usr/bin/codesign").args(["--force", "--deep", "--sign", "-"]).arg(app).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

fn zip_of(app: &Path) -> (Vec<u8>, String) {
    let zip = app.with_extension("zip");
    assert!(Command::new("/usr/bin/ditto").args(["-c", "-k", "--keepParent"]).arg(app).arg(&zip).status().unwrap().success());
    let bytes = std::fs::read(&zip).unwrap();
    let _ = std::fs::remove_file(&zip);
    let sha = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    (bytes, sha)
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
        let folder = std::env::temp_dir().canonicalize().unwrap().join(format!("recto-update-e2e-{}-{}", std::process::id(), wall_ms()));
        std::fs::create_dir_all(&folder).unwrap();
        let app = copy_app(&folder.join("install"), OLD);
        Some(Self { folder, state: State::new(), server: Server::start(), app })
    }

    fn release_of(&self, version: &str, broken: bool) {
        let staging = self.folder.join(format!("release-{version}"));
        let app = copy_app(&staging, version);
        if broken {
            std::fs::write(app.join("Contents/MacOS/RectoPDF"), "#!/bin/sh\nexit 1\n").unwrap();
            sign(&app);
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
        let output = command.args(["qa", "start", "--app", self.app.join("Contents/MacOS/RectoPDF").to_str().unwrap(), "--time-limit", "3m", "--json"]).stderr(std::process::Stdio::inherit()).output().unwrap();
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stdout)));
        assert_eq!(reply["ok"], true, "{reply}");
        let run = Run { state: &self.state, pid: reply["data"]["pid"].as_u64().unwrap() as u32, done: false };
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
        old.done = true;
        let began = Instant::now();
        loop {
            let (reply, code) = self.state.json(&["status"]);
            if code == 0 && reply["ok"] == true {
                let pid = reply["data"]["pid"].as_u64().unwrap() as u32;
                if pid != old_pid && local::process_counters(old_pid).is_none() {
                    return Run { state: &self.state, pid, done: false };
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
        let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister").arg("-u").arg(&self.app).status();
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
    assert!(before["session"]["tabs"][0]["edits"]["undo"].as_u64().unwrap() > 0, "{}", before["session"]["tabs"][0]);
    assert_eq!(before["app"]["settings"]["open"], true);

    bed.release_of(NEW, false);
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
    let shown = &after["record"]["first_shown"];
    assert_eq!(shown["settled"], true, "the first frame was the right one: {shown}");
    assert_eq!(shown["page"], before["reader"]["page"]);
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
    eprintln!("update gap: {gap} ms from quit to the first correct frame, in a quiet run, on a debug build, under the queue's load");
    let log = bed.log();
    for line in log.lines().filter(|line| line.contains("first frame")) {
        eprintln!("{line}");
    }
    assert!(log.contains("in the background"), "`frontmost off` means it reopens without taking the focus: {log}");
    assert!(!log.contains("alpha") && !log.contains("gamma"), "no document's name in the log");
    run.quit();
}

/// Failure: a new version that won't start leaves nothing, or the user's work behind it. **The real app, with a release whose executable ends at once:** it goes back by itself, the whole state comes back, and it says so and doesn't try again by itself.
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
    bed.release_of(NEW, true);
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
    assert_same_but(&before, &run.report(), &[], ACROSS_AN_UPDATE);
    assert!(bed.log().contains("went back to"), "{}", bed.log());
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
    bed.release_of(NEW, false);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let mut run = run;
    let pid = run.pid;
    let began = Instant::now();
    while local::process_counters(pid).is_some() {
        assert!(began.elapsed() < Duration::from_secs(60), "{}", bed.log());
        std::thread::sleep(Duration::from_millis(20));
    }
    run.done = true;
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
    bed.release_of(NEW, false);
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

    bed.server.latest(format!(r#"{{"version":"0.0.9","tag":"v0.0.9","minimum_macos":"15.0","zip":"RectoPDF-0.0.9.zip","sha256":"{}"}}"#, "a".repeat(64)).into_bytes());
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
    bed.release_of(NEW, false);
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
    bed.release_of(NEW, false);
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
    bed.release_of(NEW, false);
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
    assert_eq!(run.ok(&["settings"])["auto_update"], true, "on by default in anything that behaves as a release");
    bed.release_of(NEW, false);
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
    assert_eq!(run.report()["session"]["app"]["focus"]["app_frontmost"], false, "and it's still in the background");
    assert!(bed.log().contains("in the background"), "{}", bed.log());
    run.quit();
}

/// Failure: a build with no test server behaves as a release: it checks by itself, replaces itself or reaches the real server from a quiet run. And an agent below Full can install. Each is refused, and logged.
#[test]
fn a_development_build_only_asks_and_installing_needs_full() {
    let Some(_) = super::app_path() else { return };
    let state = State::new();
    let Some(run) = Run::start(&state) else { return };
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
    assert!(row.as_array().unwrap().iter().any(|item| item["id"] == "app.update" && item["title"] == "Check for Updates…"), "{row}");
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
    assert!(log.contains("update_install") && log.contains("update_check") && log.contains("settings") && log.contains("update_status"), "each is logged: {log}");
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
    bed.server.latest(format!(r#"{{"version":"{OLD}","tag":"v{OLD}","minimum_macos":"15.0","zip":"RectoPDF-{OLD}.zip","sha256":"{}"}}"#, "a".repeat(64)).into_bytes());
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
    bed.release_of(NEW, false);
    run.ok(&["qa", "menu", "app.update"]);
    bed.wait_status(&run, "available");
    assert_eq!(item(&menu(&run), "app.update")["title"], format!("Update to {NEW}…"));
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
