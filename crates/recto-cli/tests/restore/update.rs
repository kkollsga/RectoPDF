//! **The update, end to end, on the real app.** An ad-hoc-signed "old" app and a fake release with a real zip and `latest.json`, on a server on this machine, with temp state. The app does what it does for a user, by the same calls, in quiet runs. SPEC decision 24: "it should recover state so perfectly that it can update while the user is using it."
//!
//! What can't be tested here is the real signature passing: a Developer ID and Gatekeeper need a real signed release. The refusal is tested here and in `recto-update`'s own tests.

use super::{Run, assert_same_but, long_document, make_rich};
use crate::common::State;
use recto_platform::local;
use recto_update::source::REPO_ENV;
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
/// A development build of the old version.
const DEV: &str = "0.1.0.1";

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
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/build.noindex/RectoPDF.app");
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
    /// As [`Broken::EndsAfterFirstFrame`], and as it ends it leaves the record so the version before can't read it.
    EndsAndDamagesTheRecord,
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
            .join(format!("recto-update-e2e-{}-{}.noindex", std::process::id(), wall_ms()));
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
            Broken::EndsAfterFirstFrame | Broken::EndsAndDamagesTheRecord => {
                use recto_update::TestBreakage as B;
                let what: &[B] = match broken {
                    Broken::EndsAfterFirstFrame => &[B::EndsAfterFirstFrame],
                    _ => &[B::EndsAfterFirstFrame, B::DamagesTheRecordAsItEnds],
                };
                for what in what {
                    std::fs::write(app.join("Contents/Resources").join(what.file()), "").unwrap();
                }
                sign(&app);
            }
        }
        let (zip, sha) = zip_of(&app);
        self.server.release(version, zip, &sha);
        let _ = std::fs::remove_dir_all(&staging);
    }

    /// A development build of this version, made by `tools/dev-build.sh --package` from the test's copy, as the owner's are made: ad hoc, with its identifier as its designated requirement, and never the keychain, which a `security` that fails and says it was called stands guard over.
    fn dev_build(&self, version: &str, broken: Broken) -> PathBuf {
        let release = version.rsplit_once('.').unwrap().0;
        let staging = self.folder.join(format!("dev-{version}"));
        let app = copy_app(&staging, release);
        if broken == Broken::DamagesTheRecordAndEnds {
            let macos = app.join("Contents/MacOS");
            std::fs::copy(macos.join("recto-updater"), macos.join("RectoPDF")).unwrap();
        }
        let guard = self.folder.join("no-keychain");
        std::fs::create_dir_all(&guard).unwrap();
        let called = guard.join("called");
        std::fs::write(guard.join("security"), format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 1\n", called.display())).unwrap();
        assert!(Command::new("/bin/chmod").arg("+x").arg(guard.join("security")).status().unwrap().success());
        let tool = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/dev-build.sh");
        let path = format!("{}:{}", guard.display(), std::env::var("PATH").unwrap_or_default());
        let output = Command::new(tool)
            .args(["--package"])
            .arg(&app)
            .arg("--out")
            .arg(&staging)
            .args(["--version", version])
            .env("PATH", path)
            .env_remove("RECTO_SIGN_IDENTITY")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(
            !called.exists(),
            "a dev build touched the keychain: {}",
            std::fs::read_to_string(&called).unwrap_or_default()
        );
        let zip = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().lines().last().unwrap());
        // Ad hoc, the app and its helper, with no team: nothing of the keychain's was used.
        let look = staging.join("signed");
        assert!(
            Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(&zip)
                .arg(&look)
                .status()
                .unwrap()
                .success()
        );
        for signed in [look.join("RectoPDF.app"), look.join("RectoPDF.app/Contents/Helpers/RectoRender.app")] {
            let said = Command::new("/usr/bin/codesign").args(["-dv"]).arg(&signed).output().unwrap();
            let said = String::from_utf8_lossy(&said.stderr).to_string();
            assert!(said.lines().any(|line| line == "Signature=adhoc"), "{}: {said}", signed.display());
            assert!(said.lines().any(|line| line == "TeamIdentifier=not set"), "no team: {said}");
        }
        let _ = std::fs::remove_dir_all(&look);
        assert_eq!(zip.file_name().unwrap().to_str().unwrap(), format!("RectoPDF-{version}.zip"));
        zip
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

    /// What the state folder's run said, the update log among it, and what the system recorded, for a failure message where nothing answered.
    fn said(&self) -> String {
        crate::common::app::what_became_of(&self.state, None)
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
            assert!(began.elapsed() < Duration::from_secs(60), "waited for {wanted}: {status}\n{}", self.said());
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
            assert!(began.elapsed() < Duration::from_secs(90), "no new app: {reply}\n{}", self.said());
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
            assert!(began.elapsed() < Duration::from_secs(90), "{status}\n{}", self.said());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for the old process to end, for the installer to see this version come up, and for it to end by itself: for a release that ends a moment after its first frame. From the update log and the processes at the app's path, and not by catching it answering: it answers for well under a second, which a poll on a slow machine can miss, and then nothing's running to answer until the test starts one. CI's run 43054237265 saw `not_running` for the whole 90 s after it had come and gone (2026-11-11).
    fn wait_for_new_version_to_come_and_go(&self, mut old: Run<'_>, version: &str) {
        old.ended();
        let came_up = format!("installer: {version} started");
        let began = Instant::now();
        while !self.log().contains(&came_up) {
            assert!(
                began.elapsed() < Duration::from_secs(90),
                "the installer never saw {version} come up:\n{}",
                self.said()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // The old app was gone before the swap, so whatever runs from the app's path now is the new version.
        let executable = self.app.join("Contents/MacOS/RectoPDF");
        while !local::processes_running(&executable).is_empty() {
            assert!(began.elapsed() < Duration::from_secs(120), "{version} didn't end by itself:\n{}", self.said());
            std::thread::sleep(Duration::from_millis(20));
        }
        self.wait_until_nothing_answers();
    }

    /// Starts whatever's at the app's path as `recto qa start` does, and waits for it to be gone: for a version that ends by itself. Says what `qa start` answered, for a failure to show. A start refused as "already running" isn't a launch: what's answering is the last one still closing its socket, and it's waited for and tried again.
    fn launch_until_it_ends(&self) -> String {
        let executable = self.app.join("Contents/MacOS/RectoPDF");
        let began = Instant::now();
        loop {
            let mut command = self.state.command();
            command.env(REPO_ENV, &self.server.url);
            let output = command
                .args(["qa", "start", "--app", executable.to_str().unwrap(), "--time-limit", "3m", "--json"])
                .output()
                .unwrap();
            let answered = format!(
                "{} {}",
                String::from_utf8_lossy(&output.stdout).trim(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            let reply: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
            if reply["error"]["code"] == "already_running" {
                assert!(
                    began.elapsed() < Duration::from_secs(60),
                    "the last run never let go: {answered}\n{}",
                    self.log()
                );
                self.wait_until_nothing_answers();
                continue;
            }
            if let Some(pid) = reply["data"]["pid"].as_u64() {
                self.wait_gone(pid as u32);
            }
            // Or it went back, and what's running at the app's path now is the version before.
            while !local::processes_running(&executable).is_empty() && self.version() != OLD {
                assert!(began.elapsed() < Duration::from_secs(60), "it didn't end: {}", self.log());
                std::thread::sleep(Duration::from_millis(20));
            }
            if self.version() != OLD {
                self.wait_until_nothing_answers();
            }
            return answered;
        }
    }

    /// Waits for a run to be gone: its process, and then its socket, which a process that can't be asked about any more can still be closing, and which still takes a connection meanwhile, so the next `qa start` would be told something's already running.
    fn wait_gone(&self, pid: u32) {
        let began = Instant::now();
        while local::process_counters(pid).is_some() {
            assert!(began.elapsed() < Duration::from_secs(60), "process {pid} didn't end: {}", self.log());
            std::thread::sleep(Duration::from_millis(20));
        }
        self.wait_until_nothing_answers();
    }

    fn wait_until_nothing_answers(&self) {
        let began = Instant::now();
        while std::os::unix::net::UnixStream::connect(&self.state.paths.socket).is_ok() {
            assert!(began.elapsed() < Duration::from_secs(60), "the socket stayed: {}", self.log());
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
            assert!(began.elapsed() < Duration::from_secs(60), "{status}\n{}", self.said());
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
}

impl Drop for Bed {
    fn drop(&mut self) {
        for pid in local::processes_running(&self.app.join("Contents/MacOS/RectoPDF")) {
            local::signal_process(pid, true);
        }
        // `open` told the system about these, and a scan may have found the kept copy and its helper. It forgets every one of them here, before they go, rather than at some later tidying.
        recto_platform::registry::forget_all_under(&self.folder);
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

#[path = "update/slow.rs"]
mod slow;

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

/// Failure: a development build doesn't go in as a release does, changes anything the user can see or has changed, or is taken from an agent below Full. **The owner's dev cycle, end to end:** `tools/dev-build.sh` makes the zip with no keychain, ad hoc with its identifier as its designated requirement; *Allow development builds* is hidden in Settings on a release and turned on by the command; the build's refused at agent access Off and Control, as anyone can sign ad hoc, and goes in at Full, by the same hand-off, swap and first frame as a release, with the whole of `qa state` compared before and after and the old app kept. On the build, Settings shows the switch.
#[test]
fn a_development_build_goes_in_by_the_same_hand_off() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let (a, b, c) = (long_document("alpha", 60), long_document("beta", 30), super::chaptered("gamma", 20, 8));
    make_rich(&run, &[&a, &b, &c]);
    run.settled();
    let zip = bed.dev_build(DEV, Broken::Not);
    // What it's signed with, from the zip itself.
    let look = bed.folder.join("look");
    assert!(
        Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(&look)
            .status()
            .unwrap()
            .success()
    );
    let requirement = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-"])
        .arg(look.join("RectoPDF.app"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&requirement.stderr).contains("designated => identifier \"io.github.kkollsga.RectoPDF.e2e\"")
            || String::from_utf8_lossy(&requirement.stdout).contains("designated => identifier \"io.github.kkollsga.RectoPDF.e2e\""),
        "every dev build is the same app to macOS: {requirement:?}"
    );
    // Off until the owner turns it on, and on a release, not in Settings at all.
    let rows = |run: &Run<'_>| -> Vec<String> {
        run.ok(&["qa", "state"])["app"]["settings"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap().to_owned())
            .collect()
    };
    run.ok(&["qa", "menu", "app.settings"]);
    assert_eq!(rows(&run), ["update.automatic"], "a release shows no switch for builds");
    let (reply, _) = bed.state.json(&["update", "install", zip.to_str().unwrap(), "--now"]);
    assert_eq!(reply["error"]["code"], "dev_builds_off", "{reply}");
    assert_eq!(run.ok(&["update", "dev-builds", "on"])["dev_builds"], true);
    assert!(rows(&run).contains(&"update.dev_builds".to_owned()), "on, it's there to turn off");
    run.ok(&["qa", "settings", "close"]);
    let before = run.report();
    // The switch isn't consent below Full: anyone can sign a build ad hoc.
    for level in [recto_agent::Level::Off, recto_agent::Level::Control] {
        bed.state.set_level(level);
        let (refused, _) = bed.state.json(&["update", "install", zip.to_str().unwrap(), "--now"]);
        assert_eq!(refused["error"]["code"], "permission_denied", "{level:?}: {refused}");
    }
    bed.state.set_level(recto_agent::Level::Full);
    let (answer, code) = bed.state.json(&["update", "install", zip.to_str().unwrap(), "--now"]);
    assert_eq!((code, &answer["ok"]), (0, &json!(true)), "{answer}");
    assert_eq!(answer["data"]["version"], DEV);
    assert!(
        answer["data"]["words"].as_str().unwrap().contains("folders"),
        "signed ad hoc, and it says what that means: {answer}"
    );
    let run = bed.wait_for_new_app_of(run, DEV);
    run.ok(&["qa", "scroller-style", "overlay"]);
    run.settled();
    let after = run.report();
    assert_eq!(bed.version(), DEV, "the build is at the app's path");
    assert_eq!(bed.kept().as_deref(), Some(OLD), "the old one is kept");
    assert_eq!(after["update"]["version"], DEV);
    assert_same_but(&before, &after, &[], ACROSS_AN_UPDATE);
    let log = bed.log();
    assert!(log.contains(&format!("development build {DEV} checked, signed ad hoc")), "{log}");
    run.ok(&["update", "dev-builds", "off"]);
    run.ok(&["qa", "menu", "app.settings"]);
    assert!(rows(&run).contains(&"update.dev_builds".to_owned()), "a development build shows it, on or off");
    run.quit();
}

/// Failure: with a development build waiting for its five idle minutes, Settings shows no way to install it (owner, 2026-11-12: "I dont see the update now button in settings"), the plain command installs it against the owner's rule, or the button does something the menu and `--now` don't. **The real app, through the real hand-over:** `recto update install <zip>` copies the zip into the app's folder and the app opens only that; Settings shows the build and an enabled Update now; the plain command only says what it waits for; the button goes at once and the new app comes up. With `RECTO_SHOTS_DIR` set, light and dark screenshots of Settings with and without the waiting build are written there.
#[test]
fn settings_offers_update_now_for_a_waiting_build() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start();
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["goto", "4"]);
    run.ok(&["qa", "frontmost", "off"]);
    run.settled();
    run.ok(&["update", "dev-builds", "on"]);
    run.ok(&["qa", "settings", "open"]);
    let shot = |name: &str| {
        let Some(dir) = std::env::var_os("RECTO_SHOTS_DIR") else { return };
        std::fs::create_dir_all(&dir).unwrap();
        for look in ["light", "dark"] {
            run.ok(&["qa", "appearance", look]);
            let out = PathBuf::from(&dir).join(format!("settings-{name}-{look}.png"));
            run.ok(&["qa", "shot-settings", "--out", out.to_str().unwrap()]);
        }
        run.ok(&["qa", "appearance", "light"]);
    };
    let action = || run.ok(&["qa", "state"])["app"]["settings"]["action"].clone();
    assert!(action().is_null(), "nothing waits, so no button: {}", action());
    shot("without");
    let zip = bed.dev_build(DEV, Broken::Not);
    let answer = run.ok(&["update", "install", zip.to_str().unwrap()]);
    assert!(answer["waits_ms"].as_u64().unwrap() > 4 * 60_000, "{answer}");
    let folder = PathBuf::from(bed.status(&run)["handover_dir"].as_str().unwrap());
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0, "the copy is gone once it was read");
    // The plain command starts nothing: the five idle minutes are the owner's rule.
    let plain = run.ok(&["update", "install"]);
    assert_eq!(plain["started"], false, "{plain}");
    assert!(plain["words"].as_str().unwrap().contains("Nothing was started"), "{plain}");
    assert_eq!(bed.status(&run)["status"], "build_ready");
    // Settings and the menu, from Rust's words.
    let wanted = format!("Development build {DEV} is ready");
    let began = Instant::now();
    while action()["title"] != json!(wanted) {
        assert!(began.elapsed() < Duration::from_secs(10), "Settings never showed the build: {}", action());
        std::thread::sleep(Duration::from_millis(50)); // ratchet: poll
    }
    let shown = action();
    assert_eq!((&shown["button"], &shown["enabled"]), (&json!("Update now"), &json!(true)), "{shown}");
    let status = bed.status(&run);
    assert_eq!(status["menu"], json!({ "title": format!("Update to {DEV}…"), "enabled": true }), "{status}");
    shot("with");
    // The button is the menu's call and `--now`'s: it goes at once.
    run.ok(&["qa", "settings", "update.now"]);
    let run = bed.wait_for_new_app_of(run, DEV);
    run.settled();
    assert_eq!(bed.version(), DEV);
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
    assert_eq!(run.ok(&["qa", "state"])["app"]["window_open"], false, "still closed: DIAG {}", bed.log());
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
    assert_eq!(after["app"]["window"]["minimised"], true, "still in the Dock: DIAG {}", bed.log());
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
    let (run, before) = keeps_ending(&bed, Broken::EndsAfterFirstFrame);
    // The one difference, on purpose: it's still on offer, as by hand it can always be tried again.
    assert_same_but(&before, &run.report(), &[], &["app.settings.status", "app.menu_bar[0].items[1].title"]);
    let log = bed.log();
    assert!(log.contains("ended early") && log.contains("went back to"), "{log}");
    run.quit();
}

/// Failure (review 1): going back from a version that kept ending deleted the record from before the update, the only good copy when what that version left can't be read by the one before. **The real app, with a release that kept ending and left the record damaged:** the version before can't read it, puts back the record from before, and says so.
#[test]
fn coming_back_to_a_record_it_cant_read_takes_the_one_from_before() {
    let Some(bed) = Bed::new() else { return };
    let (run, before) = keeps_ending(&bed, Broken::EndsAndDamagesTheRecord);
    let noticed = crate::common::app::wait_for(&run, |state| {
        state["app"]["copy_notice"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("from before the update"))
    });
    assert_eq!(noticed["session"]["tabs"].as_array().unwrap().len(), 1, "{}", noticed["session"]);
    assert_same_but(
        &before,
        &run.report(),
        &[],
        &["app.settings.status", "app.menu_bar[0].items[1].title", "app.note", "session.app.notice"],
    );
    assert!(bed.log().contains("record from before the update was put back"), "{}", bed.log());
    run.quit();
}

/// An update to a release that ends after its first frame, then launches until it's gone back. Gives back the version before, answering, and the state as it was before the update.
fn keeps_ending(bed: &Bed, broken: Broken) -> (Run<'_>, Value) {
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
    bed.release_of(NEW, broken);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    // It comes up, for the installer, which finishes. And then it ends, socket and all.
    bed.wait_for_new_version_to_come_and_go(run, NEW);
    assert_eq!(bed.version(), NEW, "the installer took it as having come up");
    let files = recto_update::plan::Files::in_state(&bed.state.root);
    let mut launches = 0;
    let mut answers = Vec::new();
    while bed.version() != OLD {
        launches += 1;
        assert!(
            launches <= recto_update::updater::EARLY_ENDS_LIMIT + 1,
            "it never went back: {}\nqa start answered:\n{}",
            bed.log(),
            answers.join("\n")
        );
        answers.push(bed.launch_until_it_ends());
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
    (run, before)
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
    // In the Dock, the window can't say it either (review 1).
    run.ok(&["qa", "window", "open"]);
    run.settled();
    run.ok(&["qa", "minimise", "on"]);
    run.ok(&["qa", "menu", "app.update"]);
    let shown = crate::common::app::wait_for(&run, |state| state["app"]["sheet"]["kind"] == "update_alert");
    assert_eq!(shown["app"]["window"]["minimised"], true);
    run.ok(&["qa", "sheet-answer", "first"]);
    // With the window open, the margin says it, and nothing's put over the page.
    run.ok(&["qa", "minimise", "off"]);
    run.ok(&["qa", "menu", "app.update"]);
    let noticed = crate::common::app::wait_for(&run, |state| state["app"]["copy_notice"]["text"].is_string());
    assert!(noticed["app"]["sheet"].get("kind").is_none(), "{noticed}");
    run.quit();
}

/// Failure (review 1): a version being watched after an update counts a quit someone asked for, or a logout's or `killall`'s SIGTERM, as a run that ended early, so three ordinary quits in its first minutes send a good version back. A real `quit_requested`, from the menu or a SIGTERM, isn't counted; a kill is.
#[test]
fn a_quit_isnt_an_early_end_and_a_kill_is() {
    let Some(bed) = Bed::new() else { return };
    let files = recto_update::plan::Files::in_state(&bed.state.root);
    bed.state.paths.prepare().unwrap();
    recto_update::saved::Saved {
        proven: Some(OLD.to_owned()),
        probation: Some(recto_update::saved::Probation {
            version: OLD.to_owned(),
            since_ms: wall_ms(),
            running_since_ms: None,
            early_ends: 0,
        }),
        ..recto_update::saved::Saved::default()
    }
    .write(&files.saved);
    let probation = || recto_update::saved::Saved::read(&files.saved).probation.unwrap();
    let run = bed.start();
    assert!(probation().running_since_ms.is_some(), "this run's watched");
    run.quit();
    assert_eq!(probation().running_since_ms, None, "the menu's quit isn't an early end");
    let mut run = bed.start();
    assert!(Command::new("/bin/kill").args(["-TERM", &run.pid.to_string()]).status().unwrap().success());
    let began = Instant::now();
    while local::process_counters(run.pid).is_some() {
        assert!(began.elapsed() < Duration::from_secs(20), "a SIGTERM quits");
        std::thread::sleep(Duration::from_millis(20));
    }
    run.ended();
    assert_eq!(probation().running_since_ms, None, "nor is a SIGTERM's");
    let run = bed.start();
    assert_eq!(probation().early_ends, 0);
    run.kill();
    let run = bed.start();
    assert_eq!(probation().early_ends, 1, "a kill is");
    run.quit();
}

/// Failure (full check under load, 2026-11-11: "still closed", and "still in the Dock", each once): the new version read its state after the window's second of patience, so the window went up without it, a closed one opened and one in the Dock shown, and the app hidden came back on the screen. **A slow disk, on purpose:** the new version reads its state 2.5 s after its launch, and a launch an update opened waits for it, so all of it comes back as it was.
#[test]
fn a_slow_disk_doesnt_bring_back_what_was_out_of_sight() {
    let Some(bed) = Bed::new() else { return };
    let run = bed.start_with(&[("RECTO_QA_RESTORE_DELAY_MS", "2500")]);
    let file = long_document("alpha", 20);
    run.ok(&["open", file.0.to_str().unwrap()]);
    run.settled();
    run.ok(&["qa", "hide", "on"]);
    run.ok(&["qa", "window", "close"]);
    bed.release_of(NEW, Broken::Not);
    assert_eq!(run.ok(&["update", "check"])["status"], "available");
    run.ok(&["update", "install"]);
    let run = bed.wait_for_new_app(run);
    assert_eq!(bed.version(), NEW);
    // Once the state's been read, as before then there's no window to see either way.
    let state = crate::common::app::wait_for_long(&run, |state| state["session"]["tabs"].as_array().is_some_and(|tabs| tabs.len() == 1));
    assert_eq!(state["app"]["window_open"], false, "still closed: {}", bed.log());
    assert_eq!(state["app"]["hidden"], true, "still hidden");
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
    let threads = crate::common::threads::own(run.pid);
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
    // Nothing's left running after a check, whatever it found: the app's own threads, by name, as the count of all took in the system's libdispatch pool, and a fixed 200 ms could catch the check's thread on its way out.
    if let Some(wrong) = crate::common::threads::back_to(run.pid, &threads, Duration::from_secs(30)) {
        panic!("a check's thread stayed: {wrong}");
    }

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
