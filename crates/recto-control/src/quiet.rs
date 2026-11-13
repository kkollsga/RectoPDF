//! **Which sessions are confined to their own folders** (2026-11-13): every session on a state folder set with [`crate::state::STATE_ENV`], a quiet run's
//! and any other (`recto serve`, `recto session`, `recto local`, the app launched with it), since the level in such a folder is never the user's: `recto qa
//! start` writes Full there, and whatever runs on that folder next would read it. The user's own state folder can't be named through the variable, so the
//! user's own level and a free session go together, and nothing can turn one into the other.
//!
//! The rule is [`RunFolders::admits`] (`recto-platform`), and the refusal `recto_agent::quiet_files::outside_the_run`.

use crate::StatePaths;
use recto_agent::AgentError;
use recto_agent::quiet_files;
use recto_agent::table::Invocation;
use recto_platform::run_folders::{self, RunFolders};
use recto_session::{DocLoader, Session};
use std::path::{Path, PathBuf};

/// The system's temp folders, each where it really is: this user's own (from `confstr`, never `$TMPDIR`) and `/tmp`.
#[must_use]
pub fn temp_folders() -> Vec<PathBuf> {
    run_folders::temp_folders()
}

/// Whether a file is a quiet run's: in the [`temp_folders`], wherever its links really lead. A state folder adds nothing of its own unless it is in them
/// too, so naming a private folder of the user's as one gives a run none of its files.
#[must_use]
pub fn is_a_quiet_runs_file(path: &Path) -> bool {
    RunFolders::quiet_run().admits(path)
}

/// **The folders a session on these paths is confined to,** or `None` for the user's own state folder, whose sessions aren't.
#[must_use]
pub fn folders_for(paths: &StatePaths) -> Option<RunFolders> {
    paths.overridden.then(RunFolders::quiet_run)
}

/// **Confines a session made on these paths,** if they're a test state folder's: [`session_on`] and [`crate::host::SessionHost::on_state`], and
/// `local::run_here` for a session it's handed.
pub fn confine<L: DocLoader>(session: &mut Session<L>, paths: &StatePaths) {
    if let Some(folders) = folders_for(paths) {
        session.confinement().confine(folders);
    }
}

/// **A session on a state folder, confined if the folder is a test one:** the one way shipping code makes a [`Session`] from a state folder, so there's no
/// session to forget to confine (review of the branch, F7). A guard test fails any other.
#[must_use]
pub fn session_on<L: DocLoader>(paths: &StatePaths, loader: L) -> Session<L> {
    let mut session = Session::new(loader);
    confine(&mut session, paths);
    session
}

/// The one file argument of `recto`'s own commands that isn't a document or an output: the app a QA command starts, an executable, held instead to being a
/// RectoPDF bundle's (`recto-cli`'s `qa::find_app`).
pub const THE_APP_TO_START: &str = "app";

/// Arguments that hold several files in one value, with commas between: each is held to the rule.
pub const FILES_WITH_COMMAS: &[(&str, &str)] = &[("qa_compare_budget", "files")];

/// **A list of files with commas between**, split the one way: what's checked is what's opened (`qa compare-budget`'s list).
#[must_use]
pub fn split_files(list: &str) -> Vec<PathBuf> {
    list.split(',').map(str::trim).filter(|file| !file.is_empty()).map(PathBuf::from).collect()
}

/// **`recto`'s own commands that start a quiet run of their own** (`StatePaths::for_quiet_run`, whatever state folder this process was given): what they
/// name is a quiet run's, so it's held to a quiet run's folders always, the script `qa run` reads in this process included.
pub const STARTS_A_QUIET_RUN: &[&str] = &["qa_run", "qa_budget", "qa_compare_budget", "qa_select_budget"];

/// Every file one of `recto`'s own commands names: its row's files ([`quiet_files::files_in`]), and each in a list with commas, but not the app to start.
#[must_use]
pub fn named_here(invocation: &Invocation) -> Vec<PathBuf> {
    let tag = invocation.spec.tag;
    let mut fields = invocation.fields.clone();
    fields.remove(THE_APP_TO_START);
    let mut named = quiet_files::files_in(tag, &fields);
    for (with_commas, key) in FILES_WITH_COMMAS {
        if *with_commas == tag
            && let Some(list) = fields.get(*key).and_then(serde_json::Value::as_str)
        {
            named.extend(split_files(list));
        }
    }
    named
}

/// **`recto`'s own commands, run in its process** (`local`, `budget`, `skill install`, and QA's `run`, `budget`, `compare-budget` and `select-budget`):
/// every file one names, by the table, in the folders, if the state folder is a test one, or always for what starts a quiet run of its own. The same rule
/// and refusal as a session's. Asked before anything is read, and when the state folder can't be worked out a command that names a file is refused with
/// why, so neither route says whether a file outside is there (review of the branch, F2).
///
/// # Errors
///
/// [`recto_agent::quiet_files::outside_the_run`] for the first file outside, or the state folder's own refusal.
pub fn check_here(paths: Result<StatePaths, AgentError>, invocation: &Invocation) -> Result<(), AgentError> {
    let named = named_here(invocation);
    if named.is_empty() {
        return Ok(());
    }
    let folders = if STARTS_A_QUIET_RUN.contains(&invocation.spec.tag) {
        Some(RunFolders::quiet_run())
    } else {
        folders_for(&paths?)
    };
    match folders {
        Some(folders) => quiet_files::check_paths(&folders, &named),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::SessionHost;

    /// Failure: a test state folder's session isn't confined, so the Full that `recto qa start` leaves there reaches the user's files through `recto
    /// local` or `recto serve`; or the files of the run's own state folder are refused.
    #[test]
    fn a_test_state_folder_is_confined_to_the_temp_folders() {
        let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-quiet-");
        let paths = StatePaths::overridden(&dir.join("state")).unwrap();
        let folders = folders_for(&paths).expect("a test state folder is confined");
        assert!(folders.admits(&dir.join("state/out.pdf")));
        assert!(folders.admits(&dir.join("beside.pdf")), "the temp folder it's in");
        assert!(!folders.admits(Path::new("/Users/Shared/report.pdf")));
        assert!(is_a_quiet_runs_file(&dir.join("x.pdf")));
        assert!(!is_a_quiet_runs_file(Path::new("/Applications/RectoPDF.app/Contents/Info.plist")));
        assert_eq!(temp_folders(), run_folders::temp_folders());
    }

    /// Failure: the user's own state folder's sessions are confined, so the app the user runs can't open their own documents.
    #[test]
    fn the_users_own_state_folder_isnt_confined() {
        let real = StatePaths {
            overridden: false,
            ..StatePaths::overridden(&std::env::temp_dir().join("recto-quiet-real")).unwrap()
        };
        assert!(folders_for(&real).is_none());
    }

    fn pdf() -> Vec<u8> {
        recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["words"]))])
    }

    /// A test state folder and a PDF outside every temp folder, in the checkout's `target`.
    fn state_and_outside() -> (recto_testpdf::TempDir, StatePaths, recto_platform::fresh::FreshDir) {
        let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-quiet-");
        let paths = StatePaths::overridden(&dir.join("state")).unwrap();
        paths.prepare().unwrap();
        crate::levels::store(&paths, recto_agent::Level::Full).unwrap();
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
        std::fs::create_dir_all(&target).unwrap();
        let outside = recto_platform::fresh::FreshDir::new_in(&target, "recto-quiet-outside-", "").unwrap();
        std::fs::write(outside.path().join("secret.pdf"), pdf()).unwrap();
        (dir, paths, outside)
    }

    fn open(path: &Path) -> recto_agent::Command {
        recto_agent::Command::from(recto_agent::commands::tabs::Open {
            path: path.to_path_buf(),
            page: None,
        })
    }

    /// Failure (2026-11-13): a host on a test state folder isn't confined, so the Full that `recto qa start` leaves in that folder reaches the user's
    /// files with no app at all: `recto session` and `recto local` (both [`crate::local::run_here`]), `recto serve` ([`SessionHost::serving`]), and
    /// `budget tabs` in this process. Each, at Full from the folder's level file, refuses the outside PDF and opens one in temp.
    #[test]
    fn a_state_folder_that_isnt_the_users_is_confined_on_every_host() {
        let (dir, paths, outside) = state_and_outside();
        let secret = outside.path().join("secret.pdf");
        std::fs::write(dir.join("mine.pdf"), pdf()).unwrap();
        let mut session = Session::new(recto_agent::DrawableLoader);
        let mut log = crate::log::ActivityLog::new(&paths);
        let refused = crate::local::run_here(&mut session, &paths, &mut log, &open(&secret)).unwrap_err();
        assert_eq!(refused.code, "outside_quiet_run", "session and local: {}", refused.message);
        assert!(crate::local::run_here(&mut session, &paths, &mut log, &open(&dir.join("mine.pdf"))).is_ok());
        let served = SessionHost::serving(&paths);
        let refused = crate::host::Host::run(&served, recto_agent::Actor::Agent(recto_agent::Level::Full), &open(&secret), &mut Vec::new()).unwrap_err();
        assert_eq!(refused.code, "outside_quiet_run", "serve: {}", refused.message);
        assert!(served.with_session(|session| session.tabs().is_empty()));
    }

    /// Failure: `recto`'s own commands that open a file in its process (`local`, `budget tabs`, `budget index`, `skill install`) take one outside a test
    /// state folder's run, as `budget index` would build the user's PDF's index and answer how often a word is in it. Every such row of the table is
    /// tried, so one added later is too, and each refuses with the one wording; QA's own are left to QA's rules, by name.
    #[test]
    fn recto_s_own_commands_with_a_file_are_held_to_the_run() {
        let (dir, paths, outside) = state_and_outside();
        let secret = outside.path().join("secret.pdf").to_string_lossy().into_owned();
        let mut tried = Vec::new();
        for spec in recto_agent::table::TABLE.iter().filter(|spec| spec.route == recto_agent::table::Route::Here) {
            let keys: Vec<&str> = spec
                .args
                .iter()
                .filter(|arg| arg.kind == recto_agent::table::Kind::Path)
                .map(|arg| arg.key)
                .collect();
            let keys: Vec<&str> = keys
                .into_iter()
                .filter(|key| *key != THE_APP_TO_START)
                .chain(FILES_WITH_COMMAS.iter().filter(|(tag, _)| *tag == spec.tag).map(|(_, key)| *key))
                .collect();
            if keys.is_empty() {
                continue;
            }
            let mut fields = serde_json::Map::new();
            for key in &keys {
                fields.insert((*key).to_owned(), serde_json::json!(secret));
            }
            let outside_line = Invocation {
                spec,
                fields: fields.clone(),
                json: true,
                help: false,
                rest: Vec::new(),
            };
            let refused = check_here(Ok(paths.clone()), &outside_line).expect_err(spec.tag);
            assert_eq!(refused.code, "outside_quiet_run", "{}", spec.tag);
            for value in fields.values_mut() {
                *value = serde_json::json!(dir.join("mine.pdf"));
            }
            assert!(
                check_here(
                    Ok(paths.clone()),
                    &Invocation {
                        spec,
                        fields,
                        json: true,
                        help: false,
                        rest: Vec::new()
                    }
                )
                .is_ok(),
                "{}",
                spec.tag
            );
            tried.push(spec.tag);
        }
        tried.sort_unstable();
        assert_eq!(
            tried,
            [
                "budget_index",
                "budget_tabs",
                "local",
                "qa_budget",
                "qa_compare_budget",
                "qa_run",
                "qa_select_budget",
                "skill_install"
            ]
        );
        // The user's own state folder: nothing here is held.
        let real = StatePaths {
            overridden: false,
            ..paths.clone()
        };
        let spec = recto_agent::table::by_tag("local").unwrap();
        let mut fields = serde_json::Map::new();
        fields.insert("file".into(), serde_json::json!(secret));
        assert!(
            check_here(
                Ok(real),
                &Invocation {
                    spec,
                    fields,
                    json: true,
                    help: false,
                    rest: Vec::new()
                }
            )
            .is_ok()
        );
    }

    /// Failure: a place in the shipping code makes a session or a host itself instead of through [`session_on`] or `SessionHost::on_state`, which confine
    /// one on a test state folder by construction, so a host added later on such a folder reaches the user's files at the Full a quiet run leaves. In the
    /// crates that make them, no shipping line outside those two makes one at all (review of the branch, F7: the guard before this one went by what came
    /// within four lines after, and skipped folders). Test code, after its `#[cfg(test)]` or in a test file, makes its own.
    #[test]
    fn no_shipping_code_makes_a_session_but_the_confining_constructors() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files: Vec<PathBuf> = ["recto-cli/src", "recto-app/src", "recto-ffi/src", "recto-control/src"]
            .iter()
            .map(|dir| root.join(dir))
            .collect();
        let mut makers = Vec::new();
        let mut scanned = 0;
        while let Some(path) = files.pop() {
            if path.is_dir() {
                files.extend(std::fs::read_dir(&path).unwrap().filter_map(Result::ok).map(|entry| entry.path()));
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let test_file = name.ends_with("_tests.rs")
                || name == "tests.rs"
                || path.components().any(|part| matches!(part.as_os_str().to_str(), Some("tests" | "testkit")));
            if !name.ends_with(".rs") || test_file {
                continue;
            }
            scanned += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            let shipping = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (at, line) in shipping.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                if ["Session::new(", "SessionHost::new(", "SessionHost::default()"]
                    .iter()
                    .any(|made| code.contains(made))
                {
                    makers.push(format!("{}:{}", path.strip_prefix(&root).unwrap().display(), at + 1));
                }
            }
        }
        assert!(scanned > 50, "the scan found the shipping files: {scanned}");
        // The constructors themselves: `SessionHost::new`, `on_state` calling it, `Default` for the host, and `session_on`.
        let own = |place: &String| place.starts_with("recto-control/src/host.rs:") || place.starts_with("recto-control/src/quiet.rs:");
        let others: Vec<&String> = makers.iter().filter(|place| !own(place)).collect();
        assert!(others.is_empty(), "shipping code that makes a session or host itself: {others:?}");
        assert!(makers.iter().any(own), "the scan found the constructors: {makers:?}");
    }
}
