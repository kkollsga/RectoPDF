//! **A quiet run only uses files in its own folders** (2026-11-13): a session on a test state folder ([`Session::confine`]) opens and writes a file only
//! in that folder and the system's temp folders, whoever asks and at whatever level. A quiet run starts at Full, which the user never gave, so without
//! this an agent could start one and read or rewrite the user's PDFs through it.
//!
//! Which paths a command names is the table's to say, not each command's: every argument of kind [`Kind::Path`] in the command's rows. So a command added
//! later with a file in it is held by this without a word here, and a guard test fails a path that isn't declared as one. The rule itself is
//! [`RunFolders::admits`]; this is where a command meets it, and the one refusal's words.

use crate::table::{self, Kind};
use crate::{AgentError, Command, SessionAccess};
use recto_platform::run_folders::RunFolders;
use recto_session::SessionError;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The refusal, the same for every route: a command's file, a restored tab's, a file handed to the app.
#[must_use]
pub fn outside_the_run(path: &Path) -> AgentError {
    let mut error = AgentError::from(SessionError::OutsideQuietRun).then("Copy the file into a temp folder first, and use the copy.");
    error.untrusted = Some(Box::new(json!({ "file": crate::file_label(path) })));
    error
}

/// **Every file a command names,** by its row's arguments of kind [`Kind::Path`], read from the command as it goes on the wire.
#[must_use]
pub fn named_paths(command: &Command) -> Vec<PathBuf> {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(&command.wire()) else {
        return Vec::new();
    };
    files_in(command.tag(), &fields)
}

/// The files in a command's fields, by the rows of its wire name that these fields are: a row with fields it always sets (`qa open-file`'s verb) is
/// this command only if it has them, as rows sharing a name differ in what an argument is (`qa zoom`'s value is a number, `qa open-file`'s a file).
#[must_use]
pub fn files_in(tag: &str, fields: &Map<String, Value>) -> Vec<PathBuf> {
    let mut keys: Vec<&str> = table::TABLE
        .iter()
        .filter(|spec| spec.tag == tag && spec.fixed.iter().all(|(key, value)| fields.get(*key).and_then(Value::as_str) == Some(value)))
        .flat_map(|spec| spec.args.iter())
        .filter(|arg| arg.kind == Kind::Path)
        .map(|arg| arg.key)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .filter_map(|key| fields.get(key).and_then(Value::as_str))
        .map(PathBuf::from)
        .collect()
}

/// The folders this session is confined to, if it is: [`SessionAccess::confinement`], with no lock where the host keeps the fact beside the session.
pub fn confinement<A: SessionAccess>(access: &A) -> Option<Arc<RunFolders>> {
    access.confinement()
}

/// **The check:** refused if the session is confined and the command names a file outside its folders. Nothing to do, and nothing read, for a session
/// that isn't.
pub fn check<A: SessionAccess>(access: &A, command: &Command) -> Result<(), AgentError> {
    match confinement(access) {
        Some(folders) => check_paths(&folders, &named_paths(command)),
        None => Ok(()),
    }
}

/// **The session's own wall for a file about to be written** (an export's `--out`, a print's, a Save As's): a whole path, and in a confined session's
/// folders, as it resolves now. Called by what writes, inside the command, so a route that never met [`check`] (the table's wall, in the executor) still
/// can't write outside (review of the branch, F4).
///
/// # Errors
///
/// A relative path's refusal, or [`outside_the_run`].
pub fn check_new_file<A: SessionAccess>(access: &A, out: &Path) -> Result<(), AgentError> {
    crate::require_absolute(out)?;
    match confinement(access) {
        Some(folders) => check_paths(&folders, &[out.to_path_buf()]),
        None => Ok(()),
    }
}

/// Each of these files in the folders, or the refusal for the first that isn't. A relative path is refused as every command refuses one, as it would
/// mean wherever this process happens to be.
pub fn check_paths(folders: &RunFolders, paths: &[PathBuf]) -> Result<(), AgentError> {
    for path in paths {
        crate::require_absolute(path)?;
    }
    match paths.iter().find(|path| !folders.admits(path)) {
        Some(outside) => Err(outside_the_run(outside)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::Route;
    use crate::{Actor, Level, execute, parse};
    use recto_platform::fresh::FreshDir;
    use recto_session::Session;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    fn pdf(text: &str) -> Vec<u8> {
        recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&[text]))])
    }

    /// A private folder outside every temp folder, as `RECTO_STATE_DIR` could name one, holding a real PDF: the checkout's own `target`, which on CI is in
    /// the home and here isn't, and is never a temp folder either way.
    fn outside() -> FreshDir {
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        std::fs::create_dir_all(&target).unwrap();
        let dir = FreshDir::new_in(&target, "recto-quiet-outside-", "").unwrap();
        std::fs::write(dir.path().join("secret.pdf"), pdf("the user's own words")).unwrap();
        dir
    }

    /// One line for each row of the table that names a file, from its first example, with the row's name: every row, so one added later is in it.
    fn commands_naming_files() -> Vec<(&'static str, Value)> {
        table::TABLE
            .iter()
            .filter(|spec| spec.route == Route::Session && spec.args.iter().any(|arg| arg.kind == Kind::Path))
            .map(|spec| {
                let words = crate::skill::shell_words(spec.examples[0]);
                let line = table::parse_cli(&words[1..]).unwrap_or_else(|error| panic!("{}: {}", spec.name, error.message));
                (spec.name, serde_json::from_str(&line.wire()).unwrap())
            })
            .collect()
    }

    /// Failure: a command that names a file reaches a file outside a quiet run's folders, for an agent at Full or for the person: an agent that started
    /// a quiet run (at Full, which the user never gave) opens the user's PDF and reads its text, adds its pages, saves over it, or writes anywhere. Every
    /// command in the table that names a file is tried, each file in it set to one outside, and each is refused with the one wording before anything is
    /// read or written; and the table's every session command with a file is in the list, so one added later is tried too.
    #[test]
    fn every_file_a_command_names_is_refused_outside_a_quiet_run() {
        let outside = outside();
        let secret = outside.path().join("secret.pdf");
        let run = recto_testpdf::TempDir::new("recto-quiet-run-");
        std::fs::write(run.join("mine.pdf"), pdf("the run's own")).unwrap();
        let mut tried = BTreeSet::new();
        for (name, wire) in commands_naming_files() {
            let tag = wire["cmd"].as_str().unwrap().to_owned();
            let mut line = wire.clone();
            let spec = table::TABLE.iter().find(|spec| spec.name == name).unwrap();
            // Every file argument, whether the example names it or not: the PDF itself for what reads one, a new file beside it for what writes one.
            for arg in spec.args.iter().filter(|arg| arg.kind == Kind::Path) {
                line[arg.key] = json!(if matches!(tag.as_str(), "open" | "page_insert" | "update_install") || name == "qa open-file" {
                    secret.clone()
                } else {
                    outside.path().join(format!("{tag}-{}.out", arg.key))
                });
            }
            let command = parse(&line.to_string()).unwrap_or_else(|error| panic!("{line}: {}", error.message));
            for actor in [Actor::Agent(Level::Full), Actor::Person] {
                let mut session = Session::new(crate::DrawableLoader);
                session.confinement().confine(RunFolders::quiet_run());
                session.open(&run.join("mine.pdf")).unwrap();
                let access = Mutex::new(session);
                let error = execute(&access, actor, &command, &mut Vec::new()).expect_err(&format!("{tag} for {actor:?} wasn't refused"));
                assert_eq!(error.code, "outside_quiet_run", "{tag} for {actor:?}: {}", error.message);
                assert_eq!(error.exit, crate::exit::PERMISSION);
                assert!(error.message.contains("A quiet run only uses files in its own folders"), "{}", error.message);
                let session = access.into_inner().unwrap();
                assert_eq!(session.tabs().len(), 1, "{tag} opened something");
            }
            tried.insert(name.to_owned());
        }
        let left: Vec<_> = std::fs::read_dir(outside.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left, ["secret.pdf"], "something was written outside");
        let in_the_table: BTreeSet<String> = table::TABLE
            .iter()
            .filter(|spec| spec.route == Route::Session && spec.args.iter().any(|arg| arg.kind == Kind::Path))
            .map(|spec| spec.name.to_owned())
            .collect();
        assert!(in_the_table.len() >= 12, "the rows that name a file: {in_the_table:?}");
        let untried: Vec<_> = in_the_table.difference(&tried).collect();
        assert!(untried.is_empty(), "commands with a file that weren't tried: {untried:?}");
    }

    /// Failure (review of the branch, F4): what a confined session reads and writes rests only on the table's check in the executor, so a route that runs a
    /// command without it (a host added later, a command run from inside another) writes an export, a print or a Save As outside the run's folders, or
    /// reads another PDF's pages in and says by its refusal whether it was there. Every session command that names a file, run past that check, as
    /// [`execute`] runs it after its checks: each is refused the one way, the same with a file it would read gone, nothing outside is read (its last-read
    /// time), and nothing is written there. The app's own commands (`update install`) are held by the app's route, in `AppHost::run_app`.
    #[test]
    fn past_the_tables_check_the_session_still_uses_nothing_outside() {
        // In `/var/tmp`: not hidden (a worktree's `target` is, under `.worktrees`), so what refuses is the run's rule and not a file name's.
        let outside = FreshDir::new_in(Path::new("/private/var/tmp"), "recto-quiet-outside-", "").unwrap();
        let secret = outside.path().join("secret.pdf");
        std::fs::write(&secret, pdf("the user's own words")).unwrap();
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&secret)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_accessed(long_ago))
            .unwrap();
        let last_read = || std::fs::metadata(&secret).unwrap().accessed().unwrap();
        let before = last_read();
        let run = recto_testpdf::TempDir::new("recto-quiet-run-");
        std::fs::write(run.join("mine.pdf"), pdf("the run's own")).unwrap();
        let mut tried = Vec::new();
        for (name, wire) in commands_naming_files() {
            let spec = table::TABLE.iter().find(|spec| spec.name == name).unwrap();
            // The run's one tab and its one page, so nothing but the file stands in the way; and each file at `read` if it's one to read.
            let line_with = |read: &Path| {
                let mut line = wire.clone();
                for arg in spec.args {
                    match arg.kind {
                        Kind::Path if matches!(spec.tag, "open" | "page_insert") => line[arg.key] = json!(read),
                        // A new name beside it, the example's own, so its ending is what the command takes.
                        Kind::Path => {
                            let named = line[arg.key].as_str().and_then(|path| Path::new(path).file_name().map(|name| name.to_owned()));
                            line[arg.key] = json!(outside.path().join(named.unwrap_or_else(|| format!("{}.out", arg.key).into())));
                        }
                        Kind::Tab => line[arg.key] = json!("#1"),
                        Kind::Page => line[arg.key] = json!(1),
                        Kind::Pages | Kind::PagesOpen if line.get(arg.key).is_some() => line[arg.key] = json!("1"),
                        _ => {}
                    }
                }
                parse(&line.to_string()).unwrap_or_else(|error| panic!("{line}: {}", error.message))
            };
            if line_with(&secret).route() == crate::Runs::App {
                continue;
            }
            let refusal = |read: &Path| {
                let mut session = Session::new(crate::DrawableLoader);
                session.confinement().confine(RunFolders::quiet_run());
                session.open(&run.join("mine.pdf")).unwrap();
                let refused = crate::execute_held(&Mutex::new(session), Actor::Agent(Level::Full), &line_with(read), &mut Vec::new())
                    .expect_err(&format!("{name} ran with a file outside"));
                (refused.code, refused.message)
            };
            let there = refusal(&secret);
            assert_eq!(there.0, "outside_quiet_run", "{name}: {}", there.1);
            assert_eq!(
                there,
                refusal(&outside.path().join("missing.pdf")),
                "{name}: the refusal says whether the file is there"
            );
            assert_eq!(last_read(), before, "{name} read the outside file");
            tried.push(name);
        }
        for each in ["open", "page insert", "save-as", "render", "export svg", "image", "snapshot", "print"] {
            assert!(tried.contains(&each), "{each} wasn't tried: {tried:?}");
        }
        let left: Vec<_> = std::fs::read_dir(outside.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(left, ["secret.pdf"], "something was written outside");
    }

    /// A loader that notes every file a session asks it to read, then reads it as the real one does: the seam every open, wake and inserted PDF's facts go
    /// through, so a read of an outside file this way is seen whatever the volume records.
    #[derive(Clone, Default)]
    struct Noting(Arc<Mutex<Vec<PathBuf>>>);

    impl recto_session::DocLoader for Noting {
        fn load(&self, path: &Path) -> Result<recto_core::DocFacts, recto_core::OpenError> {
            self.0.lock().unwrap().push(path.to_path_buf());
            recto_session::DocLoader::load(&crate::DrawableLoader, path)
        }
    }

    /// **An outside file a walk watches:** a PDF in a folder of its own in `/private/var/tmp`, outside every temp folder a quiet run may use and not
    /// hidden, so what refuses it is the run's rule; and what it is, without reading it.
    struct Watched {
        dir: FreshDir,
        file: PathBuf,
        aside: PathBuf,
        before: (u64, u64, std::time::SystemTime, std::time::SystemTime),
    }

    impl Watched {
        fn new() -> Self {
            let dir = FreshDir::new_in(Path::new("/private/var/tmp"), "recto-quiet-sweep-", "").unwrap();
            assert!(!RunFolders::quiet_run().admits(dir.path()), "{} is a quiet run's", dir.path().display());
            let file = dir.path().join("secret.pdf");
            std::fs::write(&file, pdf("the user's own words")).unwrap();
            reads_are_recorded(dir.path());
            set_unread(&file);
            let aside = dir.path().join("aside.pdf");
            let mut watched = Self {
                dir,
                file,
                aside,
                before: (0, 0, std::time::UNIX_EPOCH, std::time::UNIX_EPOCH),
            };
            watched.before = watched.looks();
            watched
        }

        /// Its size, where it is on disk, and when it was last read and written: a read would move what's watched, so it's never read.
        fn looks(&self) -> (u64, u64, std::time::SystemTime, std::time::SystemTime) {
            use std::os::unix::fs::MetadataExt;
            let meta = std::fs::metadata(&self.file).expect("the outside file is still there");
            (meta.len(), meta.ino(), meta.accessed().unwrap(), meta.modified().unwrap())
        }

        /// A path in its folder that isn't there.
        fn absent(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        /// Runs `step` with the file gone, asserts its path is still free after, and only then puts it back: by a link, which fails if something is
        /// there, so a command that made a file in its place is caught and never written over.
        fn while_gone<R>(&self, name: &str, step: impl FnOnce() -> R) -> R {
            std::fs::rename(&self.file, &self.aside).unwrap_or_else(|_| panic!("{name} moved or removed the outside file"));
            let result = step();
            assert!(std::fs::symlink_metadata(&self.file).is_err(), "{name} made a file where the outside file was");
            std::fs::hard_link(&self.aside, &self.file).unwrap();
            std::fs::remove_file(&self.aside).unwrap();
            result
        }
    }

    /// Its last read set three days back, so a read moves it.
    fn set_unread(file: &Path) {
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
        let times = std::fs::FileTimes::new().set_accessed(long_ago);
        std::fs::File::options().write(true).open(file).unwrap().set_times(times).unwrap();
    }

    /// **A walk sees a read only where the volume records one:** a deliberate read of a file here must move its last-read time, or the walk fails, as on
    /// a volume mounted `noatime` an unread file proves nothing.
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

    /// Numbers blanked, as a time taken differs run to run, and the run's own folder named the same each run.
    fn said(answer: Result<Value, AgentError>, run: &Path) -> String {
        fn blanked(value: &Value) -> Value {
            match value {
                Value::Number(_) => json!(0),
                Value::Array(items) => Value::Array(items.iter().map(blanked).collect()),
                Value::Object(fields) => Value::Object(fields.iter().map(|(key, value)| (key.clone(), blanked(value))).collect()),
                other => other.clone(),
            }
        }
        let words = match answer {
            Ok(value) => blanked(&value).to_string(),
            Err(error) => format!("{}: {}", error.code, error.message),
        };
        words.replace(&run.to_string_lossy().into_owned(), "<run>")
    }

    /// **A session command's line as the sweep tries it:** its tab the open one, a file it reads the run's PDF and one it writes a new name in the run, a
    /// page or range the first, a wait short, and the example's numbers and switches as it has them, so what a value needs beside it is there; every
    /// other value is left out unless it's required, when it keeps the example's.
    fn base_line(spec: &table::Spec, example: &Value, run: &Path) -> Value {
        let mut line = json!({ "cmd": spec.tag });
        for (key, value) in spec.fixed {
            line[*key] = json!(value);
        }
        for arg in spec.args {
            let given = example.get(arg.key);
            let wanted = arg.required || given.is_some();
            line[arg.key] = match arg.kind {
                Kind::Tab => json!("active"),
                _ if !wanted => continue,
                Kind::Path if matches!(spec.tag, "open" | "page_insert") => json!(run.join("mine.pdf")),
                Kind::Path => {
                    let named = given
                        .and_then(Value::as_str)
                        .and_then(|path| Path::new(path).file_name().map(|name| name.to_owned()));
                    json!(run.join(named.unwrap_or_else(|| format!("{}.out", arg.key).into())))
                }
                Kind::Page => json!(1),
                Kind::Pages | Kind::PagesOpen => json!("1"),
                Kind::Duration => json!(20),
                Kind::Count(_) | Kind::Positive(_) | Kind::Switch | Kind::Scale | Kind::ScaleOrMax | Kind::Degrees => match given {
                    Some(value) => value.clone(),
                    None => continue,
                },
                _ if !arg.required => continue,
                _ => match given {
                    Some(value) => value.clone(),
                    None => continue,
                },
            };
        }
        line
    }

    /// Where the run's own PDF goes in a [`forms`] value, as the run's folder is new each run.
    const BESIDE: &str = "<the run's own>";

    /// Each form a value can hold a file in, as the code parses one: the path, a `file://` URL, a JSON string, a list with commas after the run's own
    /// PDF ([`BESIDE`]), `OUT.pdf|FIRST|LAST`.
    fn forms(file: &Path) -> Vec<String> {
        let file = file.to_string_lossy();
        vec![
            file.to_string(),
            format!("file://{file}"),
            json!(file).to_string(),
            format!("{BESIDE},{file}"),
            format!("{file}|1|1"),
        ]
    }

    /// One run of a line, in a confined session of its own on a fresh copy of the run's PDF, at Full: what it said, and which files it asked the loader
    /// for after the run's own open.
    fn run_line(make: &dyn Fn(&Path) -> Value) -> (String, Vec<PathBuf>) {
        let run = recto_testpdf::TempDir::new("recto-quiet-run-");
        let pages = [recto_testpdf::text_lines(&["the run's own"]), recto_testpdf::text_lines(&["page two"])];
        std::fs::write(run.join("mine.pdf"), recto_testpdf::pages(&pages.map(recto_testpdf::Page::new))).unwrap();
        let noting = Noting::default();
        let mut session = Session::new(noting.clone());
        session.confinement().confine(RunFolders::quiet_run());
        session.open(&run.join("mine.pdf")).unwrap();
        noting.0.lock().unwrap().clear();
        let command = parse(&make(run.path()).to_string()).expect("a line that parsed once parses again");
        let answer = execute(&Mutex::new(session), Actor::Agent(Level::Full), &command, &mut Vec::new());
        let loaded = std::mem::take(&mut *noting.0.lock().unwrap());
        (said(answer, run.path()), loaded)
    }

    /// Arguments that are tabs by selector, as [`Kind::Tab`] is, though the table calls them text: a list of them, or one for a pane. Read as
    /// [`crate::TabRef`]s, which only ever match the open tabs' names, so an outside file's path in one is no tab, and is refused at its tab by design.
    const SELECTORS: &[(&str, &str)] = &[("compare", "tabs"), ("compare", "toggle"), ("compare", "reading")];

    /// Failure (review of the branch, S1's sweep beyond QA; and round 2, B1: the sweep set the tab to the outside file too, so nearly every command was
    /// refused at its tab before its value was used, and it could fail for none of them): a command's argument is text to the table but a file to the
    /// command, so no wall sees it, and a confined session reads, writes, makes or learns of a file outside its folders through it. Every session command
    /// in the table runs on the run's open tab with its other optional arguments left out, and each of its values in turn holds an outside file, in each
    /// form the code parses one in: the outside file is left unread (by the loader, exact, and by its last-read time for what reads it another way: text,
    /// Find, a draw by the worker, a plain read), unmoved and unwritten, nothing of it comes back, every answer is the same with it gone, and nothing is
    /// made in its place. Each file argument is refused with `outside_quiet_run`, the file there and not.
    #[test]
    fn no_text_argument_reaches_a_file_outside_the_run() {
        let outside = Watched::new();
        let (mut commands, mut past_the_tab, mut lines) = (0, 0, 0);
        for spec in table::TABLE.iter().filter(|spec| spec.route == Route::Session && !spec.tag.starts_with("qa_")) {
            let takes_values = spec.args.iter().any(|arg| !matches!(arg.kind, Kind::Switch));
            let Some(example) = spec.examples.first() else {
                assert!(!takes_values, "{} takes values and has no example to try", spec.name);
                continue;
            };
            let words = crate::skill::shell_words(example);
            let invocation = table::parse_cli(&words[1..]).unwrap_or_else(|error| panic!("{}: {}", spec.name, error.message));
            let example: Value = serde_json::from_str(&invocation.wire()).unwrap();
            // Whether a value other than the tab was tried, and whether one of those got past the tab to be used.
            let (mut tried, mut got_past_the_tab) = (false, false);
            for arg in spec.args.iter().filter(|arg| arg.kind != Kind::Switch) {
                if arg.kind == Kind::Path {
                    for target in [outside.file.clone(), outside.absent("never.pdf")] {
                        let make = |run: &Path| {
                            let mut line = base_line(spec, &example, run);
                            line[arg.key] = json!(target);
                            line
                        };
                        let (refused, loaded) = run_line(&make);
                        let what = format!("{} --{} {}", spec.name, arg.key, target.display());
                        assert!(refused.starts_with("outside_quiet_run:"), "{what}: {refused}");
                        assert!(loaded.is_empty(), "{what}: the loader read {loaded:?}");
                        assert!(target == outside.file || std::fs::symlink_metadata(&target).is_err(), "{what}: made it");
                    }
                    continue;
                }
                for form in forms(&outside.file) {
                    let make = |run: &Path| {
                        let mut line = base_line(spec, &example, run);
                        line[arg.key] = json!(form.replace(BESIDE, &run.join("mine.pdf").to_string_lossy()));
                        line
                    };
                    // A line that isn't a command reaches nothing. Nor does a choice's: its words are fixed, and a typed payload refuses any other as
                    // it's read (step R2.5); but that is no value used past the tab, which only an argument that ran can be (review of R2.5, F8: a
                    // choice refused as read counted as one, so a command's path-taking argument refused at its tab for every value slipped by).
                    if parse(&make(Path::new("/tmp")).to_string()).is_err() {
                        continue;
                    }
                    let what = format!("{} --{} {form}", spec.name, arg.key);
                    let (there, read_there) = run_line(&make);
                    let (missing, read_missing) = outside.while_gone(&what, || run_line(&make));
                    for loaded in read_there.iter().chain(&read_missing) {
                        assert!(!loaded.starts_with(outside.dir.path()), "{what}: the loader read {}", loaded.display());
                    }
                    assert_eq!(there, missing, "{what}: the answer says whether the outside file is there");
                    assert!(!there.contains("user's own"), "{what}: the outside file was read: {there}");
                    assert_eq!(outside.looks(), outside.before, "{what}: the outside file was read, moved or written");
                    if arg.kind != Kind::Tab && !SELECTORS.contains(&(spec.tag, arg.key)) {
                        tried = true;
                        got_past_the_tab |= !there.starts_with("no_such_tab:");
                    }
                    lines += 1;
                }
            }
            // A command whose every value was refused at its tab was tried for nothing (round 2, B1).
            assert!(
                !tried || got_past_the_tab,
                "{}: refused at its tab for every value, so nothing was tried",
                spec.name
            );
            // A command none of whose values ran takes no free text, which is what could hold a file: a choice's words are fixed and its type
            // refuses any other as it's read, a path is held above, and the rest are numbers and grammars a line can't carry a path in.
            let takes_free_text = spec
                .args
                .iter()
                .any(|arg| matches!(arg.kind, Kind::Text | Kind::Rest) && !SELECTORS.contains(&(spec.tag, arg.key)));
            assert!(tried || !takes_free_text, "{}: free text that could hold a file was never tried", spec.name);
            commands += 1;
            past_the_tab += usize::from(got_past_the_tab);
        }
        eprintln!("the sweep: {commands} commands, {past_the_tab} with a value used past their tab, {lines} lines");
        // 19 until the typed payloads (R2.5): edit mode's, contents show's, update dev-builds', settings' and compare's choices are refused as they're read now, so
        // reach no run; the assertion above holds that such a command takes no free text.
        assert!(
            past_the_tab >= 14,
            "the commands with a value the sweep used past their tab: {past_the_tab} of {commands}"
        );
    }

    /// Failure: the wall is on a session that isn't confined (the user's own app), so the user can't open their own documents; or it's a quiet run's
    /// file and is refused. A free session's `open` of the same outside PDF opens it.
    #[test]
    fn a_free_session_and_a_runs_own_file_are_untouched() {
        let outside = outside();
        let open = |path: PathBuf, confined: bool| {
            let session = Session::new(crate::DrawableLoader);
            if confined {
                session.confinement().confine(RunFolders::quiet_run());
            }
            let access = Mutex::new(session);
            execute(
                &access,
                Actor::Agent(Level::Full),
                &Command::from(crate::commands::tabs::Open { path, page: None }),
                &mut Vec::new(),
            )
        };
        assert!(open(outside.path().join("secret.pdf"), false).is_ok(), "the user's own session opens anywhere");
        let run = recto_testpdf::TempDir::new("recto-quiet-run-");
        std::fs::write(run.join("mine.pdf"), pdf("the run's own")).unwrap();
        assert!(open(run.join("mine.pdf"), true).is_ok());
        // A link in the run's folder to the outside PDF is that PDF.
        std::os::unix::fs::symlink(outside.path().join("secret.pdf"), run.join("link.pdf")).unwrap();
        assert_eq!(open(run.join("link.pdf"), true).unwrap_err().code, "outside_quiet_run");
    }

    /// Failure: a command's file is declared to the table as something other than a file (text, say), so the wall never sees it: every field a command
    /// struct holds as a path is an argument of kind [`Kind::Path`] in some row.
    #[test]
    fn every_path_a_command_holds_is_declared_a_file() {
        let declared: BTreeSet<&str> = table::TABLE
            .iter()
            .flat_map(|spec| spec.args.iter())
            .filter(|arg| arg.kind == Kind::Path)
            .map(|arg| arg.key)
            .collect();
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands");
        let mut files = vec![folder.clone()];
        let mut held = BTreeSet::new();
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(&path).unwrap().filter_map(Result::ok).map(|entry| entry.path()));
                continue;
            }
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("pub ")
                    && let Some((name, kind)) = rest.split_once(": ")
                    && (kind.starts_with("PathBuf") || kind.starts_with("Option<PathBuf>"))
                {
                    held.insert(name.to_owned());
                }
            }
        }
        assert!(held.contains("path") && held.contains("out"), "the scan found the commands' files: {held:?}");
        let undeclared: Vec<_> = held.iter().filter(|name| !declared.contains(name.as_str())).collect();
        assert!(undeclared.is_empty(), "held as paths, not declared as files: {undeclared:?}");
    }
}
