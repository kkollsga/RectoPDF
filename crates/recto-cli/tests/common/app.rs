//! The real app in a quiet run, for the tests that drive it: where the app is, a run that ends with its test, waiting for what the app reports, and the documents those tests open. One copy, so a change to how a quiet run starts or ends is made once (review T4).
//!
//! The app has to be built, and these skip if it isn't, so `tools/check.sh --quick` stays fast and works for anyone without it. `--full` builds it and sets `RECTO_REQUIRE_APP`, so there a missing app is a failure.

use super::State;
use recto_platform::local::process_counters;
use recto_testpdf::{Object, Page, TempPdf, build, pages, text_lines};
use serde_json::{Value, json};
use std::io::Read;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// The built app, unless this run said it isn't to be used: a stage of `tools/check.sh` that runs before the app's been built from this tree would otherwise use whatever's there.
pub fn app_path() -> Option<PathBuf> {
    if std::env::var_os("RECTO_SKIP_APP").is_some() {
        return None;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF");
    if root.is_file() {
        return Some(root);
    }
    assert!(std::env::var_os("RECTO_REQUIRE_APP").is_none(), "the app isn't built, and this run needs it");
    eprintln!("skipped: the app isn't built");
    None
}

/// Whether a process is still there to be asked about.
pub fn still_running(pid: u32) -> bool {
    process_counters(pid).is_some()
}

/// A run's state folder: its own, gone with it, or one that outlives it, for what comes back after a restart.
pub enum Held<'a> {
    Own(Box<State>),
    Lent(&'a State),
}

impl Deref for Held<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        match self {
            Self::Own(state) => state,
            Self::Lent(state) => state,
        }
    }
}

/// A quiet run. Dropped without [`Run::quit`], [`Run::hand_off`] or [`Run::kill`], it's stopped, and killed if it's still there after that: a run is never left on the owner's Mac.
pub struct Run<'a> {
    pub state: Held<'a>,
    pub pid: u32,
    done: bool,
}

impl Run<'static> {
    /// A run on a state folder of its own, for at most 2 minutes.
    pub fn start() -> Option<Self> {
        Self::start_with(&[])
    }

    /// The same, with more of `qa start`'s flags.
    pub fn start_with(extra: &[&str]) -> Option<Self> {
        Self::start_lasting(Duration::from_secs(120), extra)
    }

    /// The same, for at most this long: for a test that waits on purpose for longer than a run's usual two minutes. A run's end at its limit is the app gone mid-test, as `session_ended`, so the limit has to outlast whatever the test is prepared to wait for.
    pub fn start_lasting(limit: Duration, extra: &[&str]) -> Option<Self> {
        Some(Self::start_of(&app_path()?, limit, extra))
    }

    /// The same, of this executable: a copy of the app's, such as [`OwnCopy`].
    pub fn start_of(app: &Path, limit: Duration, extra: &[&str]) -> Self {
        let state = State::new();
        let limit = format!("{}s", limit.as_secs());
        let mut args = vec!["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", &limit];
        args.extend_from_slice(extra);
        let pid = launch(&state, &args);
        Self {
            state: Held::Own(Box::new(state)),
            pid,
            done: false,
        }
    }
}

/// A copy of the built app that's one test's alone: an APFS clone beside the build, so it costs no disk and is the build byte for byte. The system sends an `open` of a bundle to a run of that bundle, whichever test started it, and turns a quiet run it reaches into a regular app before the app hears of it, until the run's guard puts it back (2026-11-11: about a millisecond later, and the reopen event some 40 ms after that). So a test that opens the bundle on purpose opens a copy of its own, and reaches no other test's run. The system forgets it, and it's removed, when it's dropped, a panic included.
///
/// **A tile in the Dock, for good:** Launch Services makes a quiet run it reopens a regular app for about a millisecond, and the Dock adds a tile for that which outlives the process and the copy's folder, until the Dock restarts (owner, 2026-11-12: "there is like a billion rectopdf instances"; the two quiet tests left 6 tiles in one run here and about 20 for the coordinator, and every full check ran them). The guard's put-back doesn't change it: going through `accessory` first left 8 tiles from 4 runs, as the plain way did. A copy with `LSUIElement` is made an accessory app instead, and the Dock adds nothing for it (0 tiles from 6 runs, 8 opens each), so every copy is that one, and the tests see `accessory` where the app would see `regular`: the same put-back, a different name. The `regular` case itself isn't tested in a run: one open of the byte-for-byte copy still left a tile (2026-11-12), so no test makes the real bundle regular on a developer's Mac.
#[allow(dead_code)] // Only a test that opens the bundle needs one.
pub struct OwnCopy {
    pub bundle: PathBuf,
}

#[allow(dead_code)]
impl OwnCopy {
    /// `LSUIElement`, and signed again ad hoc on the outside only (as `copy_app` in the restore tests does, the renderer's sandbox kept).
    pub fn new() -> Option<Self> {
        let copy = Self::clone_of_build()?;
        let plist = copy.bundle.join("Contents/Info.plist");
        let edit = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Add :LSUIElement bool true"])
            .arg(&plist)
            .output()
            .unwrap();
        assert!(edit.status.success(), "{}", String::from_utf8_lossy(&edit.stderr));
        let signed = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&copy.bundle)
            .output()
            .unwrap();
        assert!(signed.status.success(), "{}", String::from_utf8_lossy(&signed.stderr));
        Some(copy)
    }

    fn clone_of_build() -> Option<Self> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let built = app_path()?.ancestors().nth(3)?.canonicalize().ok()?;
        let folder = loop {
            let folder = built
                .parent()?
                .join(format!("own-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&folder) {
                Ok(()) => break folder,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("a folder for a copy of the app: {error}"),
            }
        };
        let bundle = folder.join("RectoPDF.app");
        recto_platform::local::clone_file(&built, &bundle).unwrap_or_else(|error| panic!("a clone of {}: {error}", built.display()));
        Some(Self { bundle })
    }

    pub fn executable(&self) -> PathBuf {
        self.bundle.join("Contents/MacOS/RectoPDF")
    }

    /// What `open -g` of this copy does, on this state folder and quiet, should it start a copy rather than reach the one running. The only way a test opens a copy.
    pub fn open_quietly(&self, state: &Path) {
        let opened = Command::new("/usr/bin/open")
            .arg("-g")
            .arg("--env")
            .arg(format!("{}={}", recto_control::state::STATE_ENV, state.display()))
            .arg(&self.bundle)
            .args(["--args", "--qa"])
            .output()
            .unwrap();
        assert!(opened.status.success(), "{opened:?}");
    }
}

impl Drop for OwnCopy {
    /// Every run of this copy ended, as only this test can reach it, and the system made to forget it until its dump says so twice in a row,
    /// both again until they hold, within 20 s; then the folder. A copy launched a moment ago is registered as the launch goes on, so one forgetting before it
    /// had finished, or before its run had quit, was undone (CI run 43101058944: `own-34946-0` left registered).
    fn drop(&mut self) {
        let executable = self.executable();
        let running = || recto_platform::local::processes_running(&executable);
        let began = Instant::now();
        let mut clean = 0;
        loop {
            for signal in ["-TERM", "-KILL"] {
                if running().is_empty() {
                    break;
                }
                for pid in running() {
                    let _ = Command::new("/bin/kill").arg(signal).arg(pid.to_string()).status();
                }
                let asked = Instant::now();
                while !running().is_empty() && asked.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            // Twice in a row clean, as a registration still on its way can land after one clean look: seen once in six, the copy
            // registered again after its first look said it had gone.
            let forgotten = recto_platform::registry::forget_until_gone(&self.bundle, Duration::ZERO);
            clean = if forgotten && running().is_empty() { clean + 1 } else { 0 };
            if clean >= 2 {
                break;
            }
            if began.elapsed() > Duration::from_secs(20) {
                eprintln!(
                    "{} was still registered or running after 20 s of ending and forgetting it",
                    self.bundle.display()
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if let Some(folder) = self.bundle.parent() {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

impl<'a> Run<'a> {
    /// A run on a state folder that outlives it, for at most 3 minutes: quit, hand off or kill it, and start another on the same folder.
    pub fn on(state: &'a State) -> Option<Self> {
        let app = app_path()?;
        let pid = launch(state, &["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "3m"]);
        Some(Self {
            state: Held::Lent(state),
            pid,
            done: false,
        })
    }

    /// [`Run::on`], with these in the app's environment, for a debug build's test seams such as `RECTO_TEST_SAVE_PAUSE`.
    #[allow(dead_code)] // Not every test binary uses a seam.
    pub fn on_with_env(state: &'a State, env: &[(&str, &std::ffi::OsStr)]) -> Option<Self> {
        let app = app_path()?;
        let mut command = state.command();
        command.args(["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "3m", "--json"]);
        for (name, value) in env {
            command.env(name, value);
        }
        let output = command.stderr(std::process::Stdio::inherit()).output().unwrap();
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(0), "{reply}");
        Some(Self {
            state: Held::Lent(state),
            pid: reply["data"]["pid"].as_u64().unwrap() as u32,
            done: false,
        })
    }

    /// A run some other way started on that state folder, such as the app an update put in place, by its process.
    pub fn attach(state: &'a State, pid: u32) -> Self {
        Self {
            state: Held::Lent(state),
            pid,
            done: false,
        }
    }

    /// It ended by itself, as an update ends the app it replaces: nothing's left to stop when it's dropped.
    pub fn ended(&mut self) {
        self.done = true;
    }

    pub fn ok(&self, args: &[&str]) -> Value {
        let (reply, code) = self.state.json(args);
        assert_eq!((code, &reply["ok"]), (0, &json!(true)), "{args:?}: {reply}{}", self.if_gone(&reply));
        reply["data"].clone()
    }

    /// For a failure message: if this answer says the app isn't there, what became of it.
    pub fn if_gone(&self, reply: &Value) -> String {
        if app_gone(reply) {
            format!("\n{}", what_became_of(&self.state, Some(self.pid)))
        } else {
            String::new()
        }
    }

    pub fn settled(&self) {
        self.ok(&["qa", "wait-settled", "--timeout", "30s"]);
    }

    pub fn report(&self) -> Value {
        self.ok(&["qa", "state"])
    }

    /// Waits for the app and its socket to be gone.
    fn wait_gone(&mut self) {
        let began = Instant::now();
        while still_running(self.pid) {
            assert!(began.elapsed() < Duration::from_secs(15), "the app didn't end");
            std::thread::sleep(Duration::from_millis(10));
        }
        // A process that can't be asked about any more can still be closing its sockets. It's gone when nothing answers there.
        while std::os::unix::net::UnixStream::connect(&self.state.paths.socket).is_ok() {
            assert!(began.elapsed() < Duration::from_secs(15), "the socket stayed");
            std::thread::sleep(Duration::from_millis(10));
        }
        self.done = true;
    }

    /// What the Quit menu item does.
    pub fn quit(mut self) {
        let _ = self.state.json(&["qa", "quit", "now"]);
        self.wait_gone();
    }

    /// What an update does.
    pub fn hand_off(mut self) -> Value {
        let (reply, code) = self.state.json(&["qa", "handoff", "now"]);
        assert_eq!(code, 0, "{reply}");
        self.wait_gone();
        reply["data"].clone()
    }

    /// What a crash does.
    pub fn kill(mut self) {
        let _ = Command::new("/bin/kill").arg("-9").arg(self.pid.to_string()).status();
        self.wait_gone();
    }
}

fn launch(state: &State, args: &[&str]) -> u32 {
    let (reply, code) = state.json(args);
    assert_eq!(code, 0, "{reply}");
    reply["data"]["pid"].as_u64().unwrap() as u32
}

impl Drop for Run<'_> {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let _ = self.state.json(&["stop"]);
        let began = Instant::now();
        while still_running(self.pid) && began.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = Command::new("/bin/kill").arg("-9").arg(self.pid.to_string()).status();
        // A run ended by its own quit forgets its copy itself; one that had to be killed couldn't.
        forget_the_built_app();
    }
}

/// Whether an answer says the app it was for isn't there: it ended while asked, or wasn't running.
pub fn app_gone(reply: &Value) -> bool {
    matches!(reply["error"]["code"].as_str(), Some("session_ended" | "not_running"))
}

/// What became of an app that's gone, for a failure message: whether its process is still there, its last words (a panic, or a quiet run's end and why, are said in its messages), and the crash reports the system wrote for it or for the helpers. CI said only `session_ended` or `not_running`, so a crash, a time limit and a test that looked too late couldn't be told apart (run 43054237265, 2026-11-11). Its exit status isn't to be had: the app isn't this test's child, as `recto qa start` or an installer started it.
pub fn what_became_of(state: &State, pid: Option<u32>) -> String {
    let process = match pid {
        Some(pid) if still_running(pid) => format!("process {pid} is still running"),
        Some(pid) => format!("process {pid} has ended"),
        None => "no process known".to_string(),
    };
    format!("{process}\n{}\n{}", state.last_words(), crash_reports(pid))
}

/// The system's crash reports of the last half hour for the app and its helpers, with the exception and termination lines of any for this process.
fn crash_reports(pid: Option<u32>) -> String {
    const RECENT: Duration = Duration::from_secs(30 * 60);
    const MAX_REPORT_BYTES: u64 = 4 * 1024 * 1024;
    let folders = [
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Logs/DiagnosticReports")),
        Some(PathBuf::from("/Library/Logs/DiagnosticReports")),
    ];
    let mut said = Vec::new();
    for folder in folders.into_iter().flatten() {
        let Ok(entries) = std::fs::read_dir(&folder) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let recent = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age < RECENT);
            if !recent || !(name.starts_with("RectoPDF") || name.starts_with("recto")) {
                continue;
            }
            let mut text = String::new();
            let _ = std::fs::File::open(entry.path()).and_then(|file| file.take(MAX_REPORT_BYTES).read_to_string(&mut text));
            let this_one = pid.is_some_and(|pid| text.contains(&format!("\"pid\" : {pid},")) || text.contains(&format!("\"pid\":{pid},")));
            said.push(format!("{}{}", entry.path().display(), if this_one { " (this process)" } else { "" }));
            if this_one {
                said.extend(
                    text.lines()
                        .filter(|line| {
                            ["\"exception\"", "\"termination\"", "\"asi\"", "\"crashed\""]
                                .iter()
                                .any(|key| line.contains(key))
                        })
                        .take(8)
                        .map(|line| format!("    {}", line.chars().take(400).collect::<String>())),
                );
            }
        }
    }
    if said.is_empty() {
        "crash reports: none for the app or its helpers in the last half hour".to_string()
    } else {
        format!("crash reports:\n  {}", said.join("\n  "))
    }
}

/// Tells the system to forget the built app, which starting a quiet run registered: only the one install is to be in Open With (owner, 2026-11-11).
pub fn forget_the_built_app() {
    if let Some(app) = app_path().as_deref().and_then(|program| program.ancestors().nth(3)) {
        recto_platform::registry::forget(app);
    }
}

/// The same, for up to 20 s, for what waits on something slow on purpose.
pub fn wait_for_long(run: &Run<'_>, holds: impl Fn(&Value) -> bool) -> Value {
    wait_for_within(run, Duration::from_secs(20), holds)
}

/// The same, for up to `limit`: for what takes as long as the work it waits on, such as Find through hundreds of pages, each a worker's job.
#[allow(dead_code)] // Not every test binary waits this long.
pub fn wait_for_within(run: &Run<'_>, limit: Duration, holds: impl Fn(&Value) -> bool) -> Value {
    let began = Instant::now();
    loop {
        let last = run.report();
        if holds(&last) {
            return last;
        }
        assert!(began.elapsed() < limit, "it never came to pass in {limit:?}: {last}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Asks for `qa state` until what it says holds, for up to about 4 s, and fails with the last it said.
pub fn wait_for(run: &Run<'_>, holds: impl Fn(&Value) -> bool) -> Value {
    let mut last = Value::Null;
    for _ in 0..400 {
        last = run.report();
        if holds(&last) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("it never came to pass: {last}");
}

/// `long.pdf`: each page says "Page number N", from 0, and a line of words.
pub fn numbered(count: usize) -> TempPdf {
    let list: Vec<Page> = (0..count)
        .map(|index| Page::new(text_lines(&[&format!("Page number {index}"), "Words, so that the page isn't blank"])))
        .collect();
    TempPdf::new("long.pdf", &pages(&list))
}

/// `<name>.pdf`: each page says "<name> page N", from 0, and a line of words, so two documents' pages are told apart.
pub fn long_document(name: &str, count: usize) -> TempPdf {
    let list: Vec<Page> = (0..count)
        .map(|index| Page::new(text_lines(&[&format!("{name} page {index}"), "Words, so that the page isn't blank"])))
        .collect();
    TempPdf::new(&format!("{name}.pdf"), &pages(&list))
}

/// `<name>.pdf`, with chapters and sections, so the sidebar has contents that scroll: a page each, with something on it, wider down the document, so a shot shows which it is.
pub fn chaptered(name: &str, chapters: usize, sections: usize) -> TempPdf {
    let each = sections + 1;
    let total = chapters * each;
    let first_page = 4 + total;
    let item = |chapter: usize, section: usize| 4 + chapter * each + section;
    let mut objects = vec![
        Object::Dict("<< /Type /Catalog /Pages 2 0 R /Outlines 3 0 R >>".into()),
        Object::Dict(format!(
            "<< /Type /Pages /Kids [{}] /Count {total} >>",
            (0..total).map(|index| format!("{} 0 R", first_page + index)).collect::<Vec<_>>().join(" ")
        )),
        Object::Dict(format!(
            "<< /Type /Outlines /First 4 0 R /Last {} 0 R /Count {chapters} >>",
            item(chapters - 1, 0)
        )),
    ];
    for chapter in 0..chapters {
        let prev = if chapter > 0 {
            format!("/Prev {} 0 R", item(chapter - 1, 0))
        } else {
            String::new()
        };
        let next = if chapter + 1 < chapters {
            format!("/Next {} 0 R", item(chapter + 1, 0))
        } else {
            String::new()
        };
        objects.push(Object::Dict(format!(
            "<< /Title (Chapter {chapter}) /Parent 3 0 R {prev} {next} /First {} 0 R /Last {} 0 R /Count -{sections} /Dest [{} 0 R /Fit] >>",
            item(chapter, 1),
            item(chapter, sections),
            first_page + chapter * each
        )));
        for section in 1..=sections {
            let prev = if section > 1 {
                format!("/Prev {} 0 R", item(chapter, section - 1))
            } else {
                String::new()
            };
            let next = if section < sections {
                format!("/Next {} 0 R", item(chapter, section + 1))
            } else {
                String::new()
            };
            objects.push(Object::Dict(format!(
                "<< /Title (Section {chapter}.{section}) /Parent {} 0 R {prev} {next} /Dest [{} 0 R /Fit] >>",
                item(chapter, 0),
                first_page + chapter * each + section
            )));
        }
    }
    let contents = first_page + total;
    for index in 0..total {
        objects.push(Object::Dict(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R >>",
            contents + index
        )));
    }
    for index in 0..total {
        objects.push(Object::Stream {
            dict: String::new(),
            data: format!(
                "0.2 0.3 0.7 rg 72 640 {} 60 re f 0 g 72 560 460 14 re f 72 530 460 14 re f 72 500 300 14 re f",
                40 + index * 2
            )
            .into_bytes(),
        });
    }
    TempPdf::new(&format!("{name}.pdf"), &build(&objects))
}
