//! **`qa as-person`:** a `recto` command run as the person at the window, through the same [`person::submit`] the window's controls use, so a test can do
//! what only a person may (the owner's edits, the window's Find with the person's passwords, an action that doesn't wait for its read) by the action
//! itself and not a stand-in for it. Only in a quiet run, which takes only a temp folder as its state; only at Full; only the commands a person's
//! control sends; and only while every file open, and every file the command names, is a test's own.

use super::values::{bad, is_a_tests_file};
use crate::core::{Inner, person};
use recto_agent::{AgentError, Command};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// **What a person's control sends,** by tag, and nothing else: never a level, an update, the record, a skill, a server or QA itself. `page_delete` is
/// the owner's tools, which aren't built yet, as `page_rotate` with a page is. Not `save`: the window's Save and Save As don't go through
/// [`person::submit`] yet (`core/saving.rs` runs them itself), so a test's save is `qa save-as`, the sheet's own answer, behind the same wall.
pub(super) const PERSONS_COMMANDS: &[&str] = &[
    "back",
    "clear_history",
    "close",
    "compare",
    "contents_fold",
    "contents_show",
    "contents_stop",
    "discard",
    "edit_mode",
    "find",
    "forward",
    "goto",
    "open",
    "page_delete",
    "page_rotate",
    "recent_clear",
    "redo",
    "tap",
    "undo",
    "view",
];

/// The fields of a command's line that name a file, as [`named_files`] reads them. Every path argument of every command in [`PERSONS_COMMANDS`] is one
/// (`each_path_argument_is_a_named_file`).
const NAMED_FILE_KEYS: &[&str] = &["path"];

/// The command's words, as a `.qa` script's line has them, with a `$` taken as itself ([`recto_agent::commands::qa::line_words`]).
fn words(line: &str) -> Result<Vec<String>, AgentError> {
    let words = recto_agent::commands::qa::line_words(line, None).map_err(bad)?;
    if words.is_empty() {
        return Err(bad("It's a `recto` command's words, as `page rotate 2 --by 90`."));
    }
    Ok(words)
}

/// The command the words are, if a person's control sends it: which command it is is asked first, so anything else is refused by this list and not by
/// whatever its own parse says.
pub(super) fn command(line: &str) -> Result<Command, AgentError> {
    let invocation = recto_agent::table::parse_cli(&words(line)?)?;
    if !PERSONS_COMMANDS.contains(&invocation.spec.tag) {
        return Err(not_a_persons(invocation.spec.name));
    }
    recto_agent::parse(&invocation.wire())
}

fn not_a_persons(name: &str) -> AgentError {
    bad(format!("`qa as-person` runs only what a person's control sends, and `{name}` isn't."))
}

/// The files a command names.
fn named_files(command: &Command) -> Vec<PathBuf> {
    let wire: Value = serde_json::from_str(&command.wire()).unwrap_or(Value::Null);
    NAMED_FILE_KEYS.iter().filter_map(|key| wire[*key].as_str().map(PathBuf::from)).collect()
}

/// Whether a file a command names is a test's: one that's there is, by where it really is; one to be made is, by the folder it goes in.
fn is_a_tests_path(path: &Path) -> bool {
    if std::fs::symlink_metadata(path).is_ok() {
        return is_a_tests_file(path);
    }
    path.parent().is_some_and(is_a_tests_file)
}

/// **The QA verbs' file wall, stated once:** what acts as the person (`qa as-person`, its `owner-edit` alias, `qa save-as`, `qa password`) does so only
/// while every file open in the session, and every file it names, is a test's own: in the quiet run's state folder or one of the temp folders a quiet run
/// may use.
///
/// # Errors
///
/// `usage`, saying so, when one isn't.
pub(super) fn check_tests_files(inner: &Inner, named: &[&Path]) -> Result<(), AgentError> {
    let open: Vec<PathBuf> = inner
        .host
        .session
        .with_session(|session| session.tabs().iter().filter_map(|tab| session.tabs().path_of(tab.id)).collect());
    if open
        .iter()
        .map(PathBuf::as_path)
        .chain(named.iter().copied())
        .any(|path| !is_a_tests_path(path))
    {
        return Err(bad(
            "This QA verb acts as the person only on a test's own file, in the quiet run's state folder or a temp folder, and one open or named isn't.",
        ));
    }
    Ok(())
}

/// The command as it will run: its tab as the `#id` it means now, and the files it names as the files they really are, so what was checked is what runs.
fn pinned(inner: &Inner, command: &Command) -> Result<Command, AgentError> {
    let mut wire: Value = serde_json::from_str(&command.wire()).map_err(|_| bad("That command can't be read back."))?;
    if let Some(tab) = wire.get("tab").and_then(Value::as_str).map(str::to_owned) {
        let id = inner.host.session.with_session(|session| recto_agent::select_tab(session, &tab))?;
        wire["tab"] = Value::from(format!("#{}", id.0));
    }
    for key in NAMED_FILE_KEYS {
        if let Some(real) = wire.get(*key).and_then(Value::as_str).and_then(|path| Path::new(path).canonicalize().ok()) {
            wire[*key] = Value::from(real.to_string_lossy().into_owned());
        }
    }
    recto_agent::parse(&wire.to_string())
}

/// **`qa owner-edit`'s words as `qa as-person`'s:** `rotate:PAGE` a quarter turn, `delete:PAGE`, `undo`, `redo`. Kept, as an alias on the person's own
/// commands, only until the worker in `recto-cli/tests/long_session.rs` has merged and that test moves to `qa as-person` (2026-11-13); then it goes.
pub(super) fn owner_edit_words(value: &str) -> Result<String, AgentError> {
    let page = |word: &str| word.parse::<u32>().ok().filter(|page| *page > 0);
    match value.split_once(':') {
        Some(("rotate", word)) if page(word).is_some() => Ok(format!("page rotate {word} --by 90")),
        Some(("delete", word)) if page(word).is_some() => Ok(format!("page delete {word}")),
        None if matches!(value, "undo" | "redo") => Ok(value.to_owned()),
        _ => Err(bad("It's `rotate:PAGE`, `delete:PAGE`, `undo` or `redo`.")),
    }
}

/// Runs the words as the person's command, after the checks. Returns as the window's control does: at once, the command on its way.
pub(super) fn run(inner: &Arc<Inner>, line: &str) -> Result<(), AgentError> {
    let asked = command(line)?;
    // A confined session's one rule first, so a file outside its folders is refused as every route refuses one; the person's command would meet it
    // only later, on the session thread, with nobody to hear. On the files as named, before anything resolves them, so the refusal doesn't say
    // whether a file outside exists; and again on the files they resolve to, which is what runs.
    let confinement = inner.host.session.confinement();
    if let Some(folders) = &confinement {
        recto_agent::quiet_files::check_paths(folders, &named_files(&asked))?;
    }
    let command = pinned(inner, &asked)?;
    let named = named_files(&command);
    if let Some(folders) = &confinement {
        recto_agent::quiet_files::check_paths(folders, &named)?;
    }
    check_tests_files(inner, &named.iter().map(PathBuf::as_path).collect::<Vec<_>>())?;
    person::submit(inner, command);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::headless::HeadlessApp;
    use recto_agent::Level;

    /// Failure: the alias for `long_session.rs` turns an owner's edit into something else than what it said: a page other than the one named, a turn
    /// other than a quarter, or a word it doesn't know let through.
    #[test]
    fn owner_edit_is_the_persons_command() {
        assert_eq!(owner_edit_words("rotate:3").unwrap(), "page rotate 3 --by 90");
        assert_eq!(owner_edit_words("delete:9").unwrap(), "page delete 9");
        assert_eq!(owner_edit_words("undo").unwrap(), "undo");
        for wrong in ["rotate:0", "rotate:x", "delete", "save", ""] {
            assert!(owner_edit_words(wrong).is_err(), "{wrong}");
        }
    }

    /// Failure: the command's words are split other than as a `.qa` line splits them, so a path with a space in it is two words, or an open quote runs on.
    #[test]
    fn the_words_split_as_a_script_line() {
        assert_eq!(words("page rotate 2").unwrap(), ["page", "rotate", "2"]);
        assert_eq!(words("save-as --out \"/tmp/a b.pdf\"").unwrap(), ["save-as", "--out", "/tmp/a b.pdf"]);
        assert!(words("open \"/tmp/a").is_err());
        assert!(words("  ").is_err());
    }

    /// Failure: `qa as-person` becomes a way for an agent to act as the person beyond a test of what a person does. Below Full it's refused; what no
    /// person's control sends is refused (raising the level, installing an update, QA itself); and outside a QA run there's no such verb at all, through
    /// the app's own `Host::run` as through the socket.
    #[test]
    fn as_person_is_only_a_tests_and_only_what_a_person_does() {
        let line = |words: &str| format!(r#"{{"cmd":"qa_drive","verb":"as_person","value":{}}}"#, Value::from(words));
        let command = recto_agent::parse(&line("undo")).unwrap();
        assert_eq!(
            command.access(),
            recto_agent::Access::At(Level::Full),
            "its own row's level, not qa_drive's first"
        );
        let app = HeadlessApp::start();
        let refused = app.agent(Level::Control, &line("undo")).expect_err("below Full");
        assert_eq!(refused.exit, recto_agent::exit::PERMISSION, "{refused:?}");
        let none = app.agent(Level::Full, &line("undo")).expect_err("no QA run");
        assert_eq!(none.code, "qa_needs_app", "outside a QA run: {none:?}");
        for words in [
            "permission set full",
            "update install --now",
            "qa state",
            "session save",
            "skill install --path /tmp/x",
            // The window's save doesn't go through the person's path yet: `qa save-as` is a test's save.
            "save",
            "save-as --out /tmp/x.pdf",
        ] {
            let refused = super::command(words).map(|_| ()).unwrap_err();
            assert!(
                refused.message.contains("runs only what a person's control sends"),
                "{words}: the allow-list's own refusal: {refused:?}"
            );
        }
        assert!(super::command("page rotate 1 --by 90").is_ok());
        assert!(super::command("find \"two words\"").is_ok());
    }

    /// Failure: `qa as-person` acts on a file that isn't a test's: one the person had open, or one it opens or saves to. With such a file open,
    /// nothing it's given runs; a test's own file opened then runs as the owner's.
    #[test]
    fn as_person_acts_only_on_a_tests_own_files() {
        let app = HeadlessApp::start();
        let own = app.disk().pdf("own.pdf", 3);
        app.open(&own);
        run(app.core(), "page rotate 1 --by 90").expect("a test's own file");
        app.settle();
        let edits = app.session_json();
        assert_eq!(
            edits["tabs"][0]["edits"]["holds_owner_edits"], true,
            "the person's edit is the owner's: {edits}"
        );
        let Some(elsewhere) = crate::testkit::headless::outside_the_tests_folders("recto-as-person.pdf") else {
            return;
        };
        std::fs::copy(&own, &elsewhere).unwrap();
        let refused = run(app.core(), &format!("open \"{}\"", elsewhere.display())).expect_err("opening a file that isn't a test's");
        // Refused by the confined session's one rule first, as every route refuses a file outside a quiet run's folders.
        assert_eq!(refused.code, "outside_quiet_run");
        // A file that isn't a test's doesn't open in a quiet run at all (the session's wall, 2026-11-13), so the wall's "with one open" is a second line
        // nothing reaches now; it stays, for a session that was never confined.
        app.open(&elsewhere);
        assert_eq!(app.tabs().len(), 1, "a file that isn't a test's opened");
        let _ = std::fs::remove_file(&elsewhere);
    }

    /// **Failure (R3.5b, untested since R3.5a): `qa as-person` in a confined run opens a relative path** from a folder the rule never looked at. A path
    /// is made whole as `recto` makes it, from the process's folder (`table::parse_cli`), before anything else sees it, so what the rule is put to is
    /// that whole path: outside the run's folders here, as a test's folder is, it's refused as every route refuses a file outside, and nothing opens.
    #[test]
    fn a_relative_path_is_held_to_the_rule_from_the_processs_folder() {
        let app = HeadlessApp::start();
        let folders = app.core().host.session.confinement().expect("a test's session is confined");
        let whole = std::path::absolute("a.pdf").unwrap();
        assert!(!folders.admits(&whole), "the test runs in a folder outside the run's: {}", whole.display());
        let refused = run(app.core(), "open a.pdf").expect_err("a relative path, outside once whole");
        assert_eq!(refused.code, "outside_quiet_run", "{}", refused.message);
        app.settle();
        assert!(app.tabs().is_empty(), "nothing opened");
    }

    /// **Failure (R3.5a): a refusal says whether a file outside the run exists.** The named file was resolved before the rule ran, so a link in a test's
    /// folder to a file outside was refused under the outside file's own name when that file was there, and under the link's when it wasn't. The rule
    /// goes first, on the files as named, so the refusal is the same either way.
    #[cfg(unix)]
    #[test]
    fn a_refusal_is_the_same_whether_the_outside_file_exists() {
        let app = HeadlessApp::start();
        let Some(elsewhere) = crate::testkit::headless::outside_the_tests_folders("recto-as-person-refusal.pdf") else {
            return;
        };
        let alias = app.disk().pdf("a.pdf", 1).with_file_name("alias.pdf");
        std::os::unix::fs::symlink(&elsewhere, &alias).unwrap();
        let open = format!("open \"{}\"", alias.display());
        let _ = std::fs::remove_file(&elsewhere);
        let missing = run(app.core(), &open).expect_err("a link to nowhere outside");
        std::fs::write(&elsewhere, b"%PDF-1.7").unwrap();
        let there = run(app.core(), &open).expect_err("a link to a file outside");
        let _ = std::fs::remove_file(&elsewhere);
        assert_eq!(missing.code, "outside_quiet_run");
        assert_eq!(
            (there.code, &there.message, &there.untrusted),
            (missing.code, &missing.message, &missing.untrusted),
            "the refusal is the same whether the outside file is there or not"
        );
    }

    /// Failure (review of R2.4, item 3): what the wall checked isn't what runs: a tab named by `active` or by a name that means another tab by the time the
    /// command runs, or a file named through a link that's pointed elsewhere after the check. The command that's submitted names the tab by its `#id` and
    /// the file as the file it really is.
    #[test]
    fn what_runs_is_what_was_checked() {
        let app = HeadlessApp::start();
        let (a, b) = (app.disk().pdf("a.pdf", 2), app.disk().pdf("b.pdf", 2));
        app.open(&a);
        app.open(&b);
        let active = app.session_json()["active"].as_u64().expect("an active tab");
        let wire = |line: &str| -> Value { serde_json::from_str(&pinned(app.core(), &command(line).unwrap()).unwrap().wire()).unwrap() };
        assert_eq!(wire("page rotate 1 --by 90")["tab"], format!("#{active}"), "`active` as the tab it is now");
        assert_eq!(wire("page rotate 1 --by 90 --tab a")["tab"], "#1", "a name as the tab it is now");
        let link = a.with_file_name("link.pdf");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&b, &link).unwrap();
        #[cfg(unix)]
        assert_eq!(
            wire(&format!("open \"{}\"", link.display()))["path"],
            b.canonicalize().unwrap().to_string_lossy().as_ref(),
            "the file a link is to"
        );
    }

    /// Failure (review of R2.4, item 4): a command on the allow-list names a file in a field the wall doesn't read, so the file goes unchecked. Every path
    /// argument of every allowed command is one of the fields [`named_files`] reads.
    #[test]
    fn each_path_argument_is_a_named_file() {
        let mut seen = 0;
        for spec in recto_agent::table::TABLE.iter().filter(|spec| PERSONS_COMMANDS.contains(&spec.tag)) {
            for arg in spec.args.iter().filter(|arg| matches!(arg.kind, recto_agent::table::Kind::Path)) {
                seen += 1;
                assert!(
                    NAMED_FILE_KEYS.contains(&arg.key),
                    "`{}` names a file in `{}`, which the wall doesn't read",
                    spec.name,
                    arg.key
                );
            }
        }
        assert!(seen > 0, "no allowed command names a file, so this holds nothing");
    }
}
