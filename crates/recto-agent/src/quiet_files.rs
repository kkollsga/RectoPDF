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

/// The folders this session is confined to, if it is. A short look at the session; the files themselves are looked at after, with nothing held.
pub fn confinement<A: SessionAccess>(access: &A) -> Option<Arc<RunFolders>> {
    access.with(|session| session.confinement().folders().cloned())
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

    /// Failure (review of the branch, S1's sweep beyond QA): a command's argument is text to the table but a file to the command, so no wall sees it, and a
    /// confined session reads, writes or learns of a file outside its folders through it. Every session command in the table, from its first example,
    /// with every value that isn't a file to the table, a number or a time set to an outside file (each file it does name kept in the run): the outside
    /// file is left unread, unmoved and unwritten, nothing of it comes back, and every answer is the same with it gone.
    #[test]
    fn no_text_argument_reaches_a_file_outside_the_run() {
        use std::os::unix::fs::MetadataExt;
        let outside = outside();
        let secret = outside.path().join("secret.pdf");
        let gone = outside.path().join("aside.pdf");
        let looks = |file: &Path| {
            let meta = std::fs::metadata(file).expect("the outside file is still there");
            (meta.len(), meta.ino(), meta.accessed().unwrap(), meta.modified().unwrap())
        };
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&secret)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_accessed(long_ago))
            .unwrap();
        let before = looks(&secret);
        let run = recto_testpdf::TempDir::new("recto-quiet-run-");
        std::fs::write(run.join("mine.pdf"), pdf("the run's own")).unwrap();
        // Numbers blanked, as a time taken differs run to run.
        fn blanked(value: &Value) -> Value {
            match value {
                Value::Number(_) => json!(0),
                Value::Array(items) => Value::Array(items.iter().map(blanked).collect()),
                Value::Object(fields) => Value::Object(fields.iter().map(|(key, value)| (key.clone(), blanked(value))).collect()),
                other => other.clone(),
            }
        }
        let said = |answer: Result<Value, AgentError>| match answer {
            Ok(value) => blanked(&value).to_string(),
            Err(error) => format!("{}: {}", error.code, error.message),
        };
        let mut swept = 0;
        for spec in table::TABLE.iter().filter(|spec| spec.route == Route::Session && !spec.tag.starts_with("qa_")) {
            let takes_text = spec.args.iter().any(|arg| matches!(arg.kind, Kind::Text | Kind::Tab | Kind::Rest));
            let Some(example) = spec.examples.first() else {
                assert!(!takes_text, "{} takes text and has no example to try", spec.name);
                continue;
            };
            let words = crate::skill::shell_words(example);
            let line = table::parse_cli(&words[1..]).unwrap_or_else(|error| panic!("{}: {}", spec.name, error.message));
            let example: Value = serde_json::from_str(&line.wire()).unwrap();
            // Every value that can hold text set to the outside file; and if the wire won't take a file for a choice, the text alone.
            let with_file = |choices_too: bool| {
                let mut wire = example.clone();
                let mut set = false;
                for arg in spec.args {
                    match arg.kind {
                        Kind::Path => wire[arg.key] = json!(run.join(format!("{}-{}.out", spec.tag, arg.key))),
                        Kind::Text | Kind::Tab | Kind::Rest => {
                            wire[arg.key] = json!(secret);
                            set = true;
                        }
                        Kind::Choice(_) | Kind::List(_) if choices_too => {
                            wire[arg.key] = json!(secret);
                            set = true;
                        }
                        _ => {}
                    }
                }
                set.then(|| parse(&wire.to_string()).ok()).flatten()
            };
            let Some(command) = with_file(true).or_else(|| with_file(false)) else {
                assert!(!takes_text, "{}: no line with a file in its text could be made", spec.name);
                continue;
            };
            let run_it = || {
                let mut session = Session::new(crate::DrawableLoader);
                session.confinement().confine(RunFolders::quiet_run());
                session.open(&run.join("mine.pdf")).unwrap();
                said(execute(&Mutex::new(session), Actor::Agent(Level::Full), &command, &mut Vec::new()))
            };
            let there = run_it();
            std::fs::rename(&secret, &gone).unwrap();
            let missing = run_it();
            std::fs::rename(&gone, &secret).unwrap();
            assert_eq!(there, missing, "{}: the answer says whether the outside file is there", spec.name);
            assert!(!there.contains("user's own"), "{}: the outside file was read", spec.name);
            assert_eq!(looks(&secret), before, "{}: the outside file was read, moved or written", spec.name);
            swept += 1;
        }
        assert!(swept >= 15, "the sweep found the commands with text: {swept}");
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
