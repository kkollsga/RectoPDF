//! **Which QA values hold a file, and the proof that the rest don't** (2026-11-13). Every argument of a QA row is one of three things: a file to the
//! table ([`Kind::Path`]), which the table's check holds to a quiet run's folders before the verb runs; a value holding a file inside it, on
//! [`FILE_IN_A_VALUE`], each held by what reads it and asserted refused here; or not a file at all. The third is proven, not trusted: every verb a client
//! can send is run in a confined app with an outside file for each of its values, and must leave that file as it was, unmoved, unread, and say the same
//! whether it's there or not. So a value the app turns into a path is found whatever its help says (review of the branch, S1: `qa save-from`'s file was
//! text to the table, and the guard before this one went by help wording, so it passed with the gap).

use super::*;
use crate::listener::QuitAnswer;
use crate::testkit::headless::HeadlessApp;
use recto_agent::Code;
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
    reads_are_recorded(dir.path());
    set_unread(&file);
    (dir, file)
}

/// Its last read set three days back, so a read moves it.
fn set_unread(file: &Path) {
    let long_ago = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
    let times = std::fs::FileTimes::new().set_accessed(long_ago);
    std::fs::File::options().write(true).open(file).unwrap().set_times(times).unwrap();
}

/// **The walk sees a read only where the volume records one:** a deliberate read of a file here must move its last-read time, or the walk fails, as on a
/// volume mounted `noatime` an unread file proves nothing. The headless app reads with the real loader, on its own threads, so there's no seam to count
/// its reads by: the last-read time is what sees them all.
fn reads_are_recorded(dir: &Path) {
    use std::os::unix::fs::MetadataExt;
    let probe = dir.join("probe");
    std::fs::write(&probe, b"probe").unwrap();
    set_unread(&probe);
    let before = std::fs::metadata(&probe).unwrap().accessed().unwrap();
    std::fs::read(&probe).unwrap();
    let after = std::fs::metadata(&probe).unwrap().accessed().unwrap();
    std::fs::remove_file(&probe).unwrap();
    let device = std::fs::metadata(dir).unwrap().dev();
    let volume = dir
        .ancestors()
        .take_while(|up| std::fs::metadata(up).is_ok_and(|meta| meta.dev() == device))
        .last()
        .unwrap_or(dir);
    assert!(
        after > before,
        "a read on the volume at {} doesn't move a file's last-read time (mounted noatime?), so this walk can't see a read and proves nothing",
        volume.display()
    );
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

/// **The verb's wire line as the walk tries it:** its tab the open one, a wait short, and every value left out: each is tried in turn, by [`with_value`].
fn base_line(spec: &Spec) -> Value {
    let mut line = json!({ "cmd": spec.tag });
    for (key, value) in spec.fixed {
        line[*key] = json!(value);
    }
    for arg in spec.args {
        match arg.kind {
            Kind::Duration => line[arg.key] = json!(20),
            Kind::Tab => line[arg.key] = json!("active"),
            _ => {}
        }
    }
    line
}

/// The verb's line with one value set.
fn with_value(spec: &Spec, key: &str, value: &str) -> Value {
    let mut line = base_line(spec);
    line[key] = json!(value);
    line
}

/// The values a client can send a verb, one at a time: each argument that isn't a number, a switch, a wait or the tab, and a secret read from stdin
/// (`qa password`'s), which is a value like any other on the wire.
fn values_of(spec: &Spec) -> Vec<(&'static str, Kind)> {
    let mut values: Vec<_> = spec
        .args
        .iter()
        .filter(|arg| {
            !matches!(
                arg.kind,
                Kind::Duration | Kind::Tab | Kind::Count(_) | Kind::Positive(_) | Kind::Page | Kind::Switch | Kind::Scale | Kind::ScaleOrMax | Kind::Degrees
            )
        })
        .map(|arg| (arg.key, arg.kind))
        .collect();
    if let Some(field) = recto_agent::table::stdin_secret_field(spec)
        && !values.iter().any(|(key, _)| *key == field)
    {
        values.push((field, Kind::Text));
    }
    values
}

/// Each form a value can hold a file in, as the code parses one: the path, a `file://` URL, a JSON string, a list with commas, `OUT.pdf|FIRST|LAST`.
fn forms(file: &str, beside: &str) -> Vec<String> {
    vec![
        file.to_owned(),
        format!("file://{file}"),
        json!(file).to_string(),
        format!("{beside},{file}"),
        format!("{file}|1|1"),
    ]
}

/// Runs `step` with the outside file gone, asserts its path is still free after, and only then puts it back: by a link, which fails if something is there,
/// so a verb that made a file in its place is caught and never written over.
fn while_gone<R>(file: &Path, aside: &Path, name: &str, step: impl FnOnce() -> R) -> R {
    std::fs::rename(file, aside).unwrap_or_else(|_| panic!("{name} moved or removed the outside file"));
    let result = step();
    assert!(std::fs::symlink_metadata(file).is_err(), "{name} made a file where the outside file was");
    std::fs::hard_link(aside, file).unwrap();
    std::fs::remove_file(aside).unwrap();
    result
}

/// Whether a refusal is the tab's: the value wasn't used.
fn at_the_tab(answer: &Result<Value, AgentError>) -> bool {
    answer
        .as_ref()
        .is_err_and(|error| matches!(error.code, Code::NO_SUCH_TAB | Code::NOT_THE_SHOWN_TAB))
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

/// Failure (review of the branch, S1; and round 2, B1: the walk set `--tab` to the outside file too, so `qa menu`, `button`, `key-action` and
/// `print-to-pdf` were refused at their tab before their value was used, and `print-to-pdf`'s check was held only to "some error", which an outside file
/// that's there gets anyway): a QA verb's value is a file to the app but text to the table, so no wall sees it, and a quiet run moves, reads, makes or
/// learns of a file outside its folders: `qa save-from` took any file, said by its errors whether it was there, and moved it into the tab. Every verb a
/// client can send runs on the open tab, each value in turn holding an outside file, in each form the code parses one in. A file to the table, and a value
/// on [`FILE_IN_A_VALUE`], is refused with `outside_quiet_run` for an outside file that's there and one that isn't, which isn't made; every other value
/// leaves the file as it was, unread (by its last-read time), says the same with it gone, and nothing is made in its place.
#[test]
fn no_qa_value_reaches_a_file_outside_the_run() {
    let (dir, file) = outside();
    let path = file.to_string_lossy().into_owned();
    let absent = dir.path().join("never.pdf");
    let aside = dir.path().join("kept-aside.pdf");
    let bytes = std::fs::read(&file).unwrap();
    // Read for its bytes, so its last read is set back again after.
    set_unread(&file);
    let before = looks(&file);
    let mut app = HeadlessApp::start();
    app.open(&app.disk().pdf("own.pdf", 2));
    let beside = app.disk().files().join("own.pdf").to_string_lossy().into_owned();
    let mut walked = std::collections::BTreeSet::new();
    let (mut past_the_tab, mut with_a_tab) = (0, 0);
    for spec in app_rows() {
        let parsed = |line: &Value| recto_agent::parse(&line.to_string()).unwrap_or_else(|error| panic!("{}: {line}: {}", spec.name, error.message));
        let (mut tried, mut got_past_the_tab) = (false, false);
        for (key, kind) in values_of(spec) {
            // A file to the table, or one in a value: refused the one way, the file there and not, and one that isn't there isn't made.
            let in_a_value: Vec<&str> = FILE_IN_A_VALUE
                .iter()
                .filter(|(name, held, _)| *name == spec.name && *held == key)
                .map(|(_, _, form)| *form)
                .collect();
            let refused_forms = if kind == Kind::Path { vec!["{}"] } else { in_a_value };
            for form in &refused_forms {
                for target in [path.as_str(), &absent.to_string_lossy()] {
                    let command = parsed(&with_value(spec, key, &form.replace("{}", target)));
                    let what = format!("{} {key}={}", spec.name, form.replace("{}", target));
                    let refused = match (&command, kind) {
                        (_, Kind::Path) => app.core().host.run(Actor::Agent(Level::Full), &command, &mut Vec::new()),
                        (Command::Qa(asked), _) => answering_bridge(&app).run(asked),
                        _ => panic!("{} isn't a QA command", spec.name),
                    }
                    .expect_err(&what);
                    assert_eq!(refused.code, "outside_quiet_run", "{what}: {}", refused.message);
                    assert!(!refused.message.contains(MARKER), "{what}");
                    assert!(std::fs::symlink_metadata(&absent).is_err(), "{what}: made the file");
                    tried = true;
                    got_past_the_tab = true;
                }
            }
            if kind == Kind::Path {
                continue;
            }
            for form in forms(&path, &beside) {
                let what = format!("{} {key}={form}", spec.name);
                let Command::Qa(asked) = parsed(&with_value(spec, key, &form)) else {
                    panic!("{} isn't a QA command", spec.name)
                };
                let bridge = answering_bridge(&app);
                let there = bridge.run(&asked);
                let missing = while_gone(&file, &aside, &what, || bridge.run(&asked));
                assert_eq!(said(&there), said(&missing), "{what}: answers differently with the outside file there and gone");
                assert!(!said(&there).contains(MARKER), "{what}: read the outside file");
                assert_eq!(looks(&file), before, "{what}: touched, moved or read the outside file");
                tried = true;
                got_past_the_tab |= !at_the_tab(&there);
                // A verb that ended the app leaves the next a new one, on the same disk.
                if bridge.inner().is_none() || app.core().core.frozen.load(Ordering::SeqCst) {
                    let disk = app.kill();
                    app = disk.start();
                    app.open(&app.disk().pdf("own.pdf", 2));
                }
            }
        }
        if spec.args.iter().any(|arg| arg.kind == Kind::Tab) {
            assert!(
                !tried || got_past_the_tab,
                "{}: refused at its tab for every value, so nothing was tried",
                spec.name
            );
            with_a_tab += 1;
            past_the_tab += usize::from(got_past_the_tab);
        }
        walked.insert(verb_of(spec));
    }
    eprintln!(
        "the walk: {} verbs; of the {with_a_tab} with a tab, {past_the_tab} used a value past it",
        walked.len()
    );
    // Every verb a client can send: the QA verb table's, but Rust's own steps, which no client sends.
    let every: std::collections::BTreeSet<&str> = recto_agent::commands::qa::verbs::all()
        .filter(|line| line.kind != recto_agent::commands::qa::verbs::QaKind::Internal)
        .map(|line| line.verb)
        .collect();
    assert_eq!(walked, every, "the walk didn't try every verb");
    assert!(
        app.core().host.session.with_session(|session| session.tabs().for_path(&file).is_none()),
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
