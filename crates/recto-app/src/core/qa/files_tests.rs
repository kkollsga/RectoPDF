//! **Which QA values hold a file, and the proof that the rest don't** (2026-11-13). Every argument of a QA row is one of three things: a file to the
//! table ([`Kind::Path`]), which the table's check holds to a quiet run's folders before the verb runs; a value holding a file inside it, on
//! [`FILE_IN_A_VALUE`], each held by what reads it and asserted refused here; or not a file at all. The third is proven, not trusted: every verb a client
//! can send is run in a confined app with an outside file for each of its values, and must leave that file as it was, unmoved, unread, and say the same
//! whether it's there or not. So a value the app turns into a path is found whatever its help says (review of the branch, S1: `qa save-from`'s file was
//! text to the table, and the guard before this one went by help wording, so it passed with the gap).

use super::*;
use crate::listener::QuitAnswer;
use crate::testkit::headless::HeadlessApp;
use recto_agent::table::{Kind, Route, Spec, TABLE};
use recto_agent::{Actor, Command, Level};
use recto_control::host::Host;
use recto_platform::fresh::FreshDir;
use std::path::PathBuf;
use std::sync::{OnceLock, Weak};
use std::time::SystemTime;

/// **The arguments whose value holds a file the table can't see as one,** and how a file goes into the value (`{}` is the file). Each is held by what
/// reads it: `qa print-to-pdf`'s by its verb (`check_print_target`), `qa compare-budget`'s list by `recto_control::quiet::check_here`, and `qa
/// as-person`'s command by its own wall and then the session's.
const FILE_IN_A_VALUE: &[(&str, &str, &str)] = &[
    ("qa as-person", "value", "open {}"),
    ("qa compare-budget", "files", "{}"),
    ("qa print-to-pdf", "value", "{}"),
    ("qa print-to-pdf", "value", "{}|1|1"),
];

/// Words in the outside file and nowhere else: one in an answer means the file was read.
const MARKER: &str = "quietrunmarker7f3a";

/// A host that answers every question at once with nothing, so a verb that gets as far as the window comes back, and what it did is Rust's alone.
#[derive(Default)]
struct Answering(OnceLock<Weak<QaBridge>>);

impl crate::Listener for Answering {
    fn model_changed(&self) {}
    fn ops_ready(&self) {}
    fn qa_request(&self, id: u64, _verb: String, _value: String) {
        if let Some(bridge) = self.0.get().and_then(Weak::upgrade) {
            bridge.answer(id, true, "{}".to_owned());
        }
    }
    fn session_ended(&self) {}
    fn copy_ready(&self) {}
    fn restore_ready(&self) {}
    fn quit_answer(&self, _answer: QuitAnswer) {}
    fn hand_off_quit(&self) {}
}

fn answering_bridge(app: &HeadlessApp) -> Arc<QaBridge> {
    let inner = app.core();
    let listener = Arc::new(Answering::default());
    let bridge = Arc::new(QaBridge::new(listener.clone(), inner.engine.clone(), Watchdog::new(), Arc::clone(&inner.core)));
    let _ = listener.0.set(Arc::downgrade(&bridge));
    bridge.attach(Arc::downgrade(inner));
    bridge
}

/// A folder outside every temp folder, in `/var/tmp` (not hidden, and no quiet run's), with a PDF that says [`MARKER`].
fn outside() -> (FreshDir, PathBuf) {
    let dir = FreshDir::new_in(Path::new("/private/var/tmp"), "recto-qa-walk-", "").unwrap();
    assert!(!recto_control::quiet::temp_folders().iter().any(|temp| dir.path().starts_with(temp)));
    let file = dir.path().join("secret.pdf");
    std::fs::write(&file, recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&[MARKER]))])).unwrap();
    (dir, file)
}

/// What the file is, without reading it (a read would move what's being watched): its size, where it is on disk, and when it was last read and written.
fn looks(file: &Path) -> (u64, u64, SystemTime, SystemTime) {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(file).expect("the outside file is still there");
    (meta.len(), meta.ino(), meta.accessed().unwrap(), meta.modified().unwrap())
}

/// An answer as it can be compared: its code and words if refused, its shape (numbers blanked, as a time taken differs run to run) if not.
fn said(answer: &Result<Value, AgentError>) -> String {
    fn blanked(value: &Value) -> Value {
        match value {
            Value::Number(_) => json!(0),
            Value::Array(items) => Value::Array(items.iter().map(blanked).collect()),
            Value::Object(fields) => Value::Object(fields.iter().map(|(key, value)| (key.clone(), blanked(value))).collect()),
            other => other.clone(),
        }
    }
    match answer {
        Ok(value) => blanked(value).to_string(),
        Err(error) => format!("{}: {}", error.code, error.message),
    }
}

/// The verb's wire line with every value that can hold text set to `file`, and a waiting one's wait made short.
fn line_with(spec: &Spec, file: &str) -> Value {
    let mut line = json!({ "cmd": spec.tag });
    for (key, value) in spec.fixed {
        line[*key] = json!(value);
    }
    // A secret read from stdin (`qa password`'s) is a value like any other on the wire.
    if let Some(field) = recto_agent::table::stdin_secret_field(spec) {
        line[field] = json!(file);
    }
    for arg in spec.args {
        line[arg.key] = match arg.kind {
            Kind::Duration => json!(20),
            Kind::Count(_) | Kind::Positive(_) | Kind::Page | Kind::Switch | Kind::Scale | Kind::ScaleOrMax | Kind::Degrees => continue,
            _ => json!(file),
        };
    }
    line
}

/// The verb a QA row sends.
fn verb_of(spec: &Spec) -> &'static str {
    spec.fixed.iter().find(|(key, _)| *key == "verb").map_or(spec.name, |(_, verb)| *verb)
}

/// The app's own QA rows: what a client sends to a running app.
fn app_rows() -> impl Iterator<Item = &'static Spec> {
    TABLE
        .iter()
        .filter(|spec| matches!(spec.tag, "qa_drive" | "qa_read") && spec.route == Route::Session)
}

/// Failure (review of the branch, S1): a QA verb's value is a file to the app but text to the table, so no wall sees it, and a quiet run moves, reads or
/// learns of a file outside its folders: `qa save-from` took any file, said by its errors whether it was there, and moved it into the tab. Every verb a
/// client can send, with an outside file in each value: one whose value is a file to the table is refused by the table's check on the app's own route,
/// one on [`FILE_IN_A_VALUE`] is refused by what reads it, and every other leaves the file as it was, unread, and says the same with it gone.
#[test]
fn no_qa_value_reaches_a_file_outside_the_run() {
    let (dir, file) = outside();
    let path = file.to_string_lossy().into_owned();
    let gone = dir.path().join("kept-aside.pdf");
    let bytes = std::fs::read(&file).unwrap();
    // Read for its bytes, so its last read is set back again after.
    let long_ago = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_accessed(long_ago))
        .unwrap();
    let before = looks(&file);
    let mut app = HeadlessApp::start();
    app.open(&app.disk().pdf("own.pdf", 2));
    let mut walked = std::collections::BTreeSet::new();
    for spec in app_rows() {
        let line = line_with(spec, &path);
        let command = recto_agent::parse(&line.to_string()).unwrap_or_else(|error| panic!("{}: {line}: {}", spec.name, error.message));
        if spec.args.iter().any(|arg| arg.kind == Kind::Path) {
            let refused = app.core().host.run(Actor::Agent(Level::Full), &command, &mut Vec::new()).expect_err(spec.name);
            assert_eq!(refused.code, "outside_quiet_run", "{}: {}", spec.name, refused.message);
            walked.insert(verb_of(spec));
            continue;
        }
        let Command::Qa(asked) = &command else {
            panic!("{} isn't a QA command", spec.name)
        };
        let in_a_value: Vec<_> = FILE_IN_A_VALUE.iter().filter(|(name, _, _)| *name == spec.name).collect();
        for (_, key, form) in &in_a_value {
            let mut line = line.clone();
            line[*key] = json!(form.replace("{}", &path));
            let Command::Qa(asked) = recto_agent::parse(&line.to_string()).unwrap() else {
                unreachable!()
            };
            let refused = answering_bridge(&app).run(&asked).expect_err(spec.name);
            assert!(!refused.message.contains(MARKER), "{}", spec.name);
        }
        let bridge = answering_bridge(&app);
        let there = bridge.run(asked);
        std::fs::rename(&file, &gone).unwrap_or_else(|_| panic!("{} moved or removed the outside file", spec.name));
        let missing = bridge.run(asked);
        std::fs::rename(&gone, &file).unwrap();
        assert_eq!(
            said(&there),
            said(&missing),
            "{} answers differently with the outside file there and gone",
            spec.name
        );
        assert!(!said(&there).contains(MARKER), "{} read the outside file", spec.name);
        assert_eq!(looks(&file), before, "{} touched, moved or read the outside file", spec.name);
        // A verb that ended the app leaves the next a new one, on the same disk.
        if bridge.inner().is_none() || app.core().core.frozen.load(Ordering::SeqCst) {
            let disk = app.kill();
            app = disk.start();
            app.open(&app.disk().pdf("own.pdf", 2));
        }
        walked.insert(verb_of(spec));
    }
    // Every verb a client can send: the QA verb table's, but Rust's own steps, which no client sends.
    let every: std::collections::BTreeSet<&str> = recto_agent::commands::qa::verbs::all()
        .filter(|line| line.kind != recto_agent::commands::qa::verbs::QaKind::Internal)
        .map(|line| line.verb)
        .collect();
    assert_eq!(walked, every, "the walk didn't try every verb");
    assert!(
        app.core().host.session.with_session(|session| session.tab_for_path(&file).is_none()),
        "the outside file became a tab"
    );
    // `qa compare-budget` is `recto`'s own, run in its process: its list is held by `check_here`.
    let spec = TABLE.iter().find(|spec| spec.name == "qa compare-budget").unwrap();
    let mut fields = serde_json::Map::new();
    fields.insert("files".into(), json!(format!("{},{path}", app.disk().pdf("other.pdf", 1).display())));
    let line = recto_agent::table::Invocation {
        spec,
        fields,
        json: true,
        help: false,
        rest: Vec::new(),
    };
    assert_eq!(
        recto_control::quiet::check_here(Ok(app.disk().paths().clone()), &line).unwrap_err().code,
        "outside_quiet_run"
    );
    assert_eq!(looks(&file), before, "the outside file was read");
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "the outside file was changed");
    drop(app.quit());
}

/// Failure: [`FILE_IN_A_VALUE`] names a row or an argument the table hasn't, so a renamed verb's file is held by nothing and the walk above doesn't try
/// it; or a file to the table is listed as a value holding one.
#[test]
fn the_values_holding_files_are_real_text_arguments() {
    for (name, key, _) in FILE_IN_A_VALUE {
        let spec = TABLE.iter().find(|spec| spec.name == *name).unwrap_or_else(|| panic!("no row {name}"));
        let arg = spec.args.iter().find(|arg| arg.key == *key).unwrap_or_else(|| panic!("{name} has no {key}"));
        assert_eq!(arg.kind, Kind::Text, "{name} {key}");
    }
}
