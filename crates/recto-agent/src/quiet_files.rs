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
                session.confinement().confine(RunFolders::for_state(outside.path()));
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

    /// Failure: the wall is on a session that isn't confined (the user's own app), so the user can't open their own documents; or it's a quiet run's
    /// file and is refused. A free session's `open` of the same outside PDF opens it.
    #[test]
    fn a_free_session_and_a_runs_own_file_are_untouched() {
        let outside = outside();
        let open = |path: PathBuf, confined: bool| {
            let session = Session::new(crate::DrawableLoader);
            if confined {
                session.confinement().confine(RunFolders::for_state(outside.path()));
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
