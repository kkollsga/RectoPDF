//! The real app in a quiet run, for the tests that drive it: where the app is, a run that ends with its test, waiting for what the app reports, and the documents those tests open. One copy, so a change to how a quiet run starts or ends is made once (review T4).
//!
//! The app has to be built, and these skip if it isn't, so `tools/check.sh --quick` stays fast and works for anyone without it. `--full` builds it and sets `RECTO_REQUIRE_APP`, so there a missing app is a failure.

use super::State;
use recto_platform::local::process_counters;
use recto_testpdf::{Object, Page, TempPdf, build, pages, text_lines};
use serde_json::{Value, json};
use std::ops::Deref;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// The built app, unless this run said it isn't to be used: a stage of `tools/check.sh` that runs before the app's been built from this tree would otherwise use whatever's there.
pub fn app_path() -> Option<PathBuf> {
    if std::env::var_os("RECTO_SKIP_APP").is_some() {
        return None;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/build/RectoPDF.app/Contents/MacOS/RectoPDF");
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
        let app = app_path()?;
        let state = State::new();
        let mut args = vec!["qa", "start", "--app", app.to_str().unwrap(), "--time-limit", "2m"];
        args.extend_from_slice(extra);
        let pid = launch(&state, &args);
        Some(Self {
            state: Held::Own(Box::new(state)),
            pid,
            done: false,
        })
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
        assert_eq!((code, &reply["ok"]), (0, &json!(true)), "{args:?}: {reply}");
        reply["data"].clone()
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
