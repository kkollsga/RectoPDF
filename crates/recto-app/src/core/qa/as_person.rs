//! **`qa as-person`:** a `recto` command run as the person at the window, through the same [`person::submit`] the window's controls use, so a test can do
//! what only a person may (the owner's edits, the window's Find with the person's passwords, an action that doesn't wait for its read) by the action
//! itself and not a stand-in for it. Only in a quiet run, which takes only a temp folder as its state; only at Full; only the commands a person's
//! control sends; and only while every file open, and every file the command names, is a test's own.

use super::values::{bad, is_a_tests_file};
use crate::core::{Inner, person};
use recto_agent::{AgentError, Command};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// **What a person's control sends,** by tag, and nothing else: never a level, an update, the record, a skill, a server or QA itself. `page_delete` is
/// the owner's tools, which aren't built yet, as `page_rotate` with a page is.
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
    "save",
    "tap",
    "undo",
    "view",
];

/// The command's words, as a `.qa` script's line has them: spaces between, and double quotes around a word with spaces in it.
fn words(line: &str) -> Result<Vec<String>, AgentError> {
    let (mut words, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            ' ' | '\t' if !quoted => {
                if any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            other => {
                word.push(other);
                any = true;
            }
        }
    }
    if quoted {
        return Err(bad("A quote in the command isn't closed."));
    }
    if any {
        words.push(word);
    }
    if words.is_empty() {
        return Err(bad("It's a `recto` command's words, as `page rotate 2 --by 90`."));
    }
    Ok(words)
}

/// The command the words are, if a person's control sends it.
pub(super) fn command(line: &str) -> Result<Command, AgentError> {
    let line = recto_agent::table::parse_cli(&words(line)?)?.wire();
    let command = recto_agent::parse(&line)?;
    if !PERSONS_COMMANDS.contains(&command.tag()) {
        return Err(bad(format!(
            "`qa as-person` runs only what a person's control sends, and `{}` isn't.",
            recto_agent::table::display_name(command.tag())
        )));
    }
    Ok(command)
}

/// The files a command names: a file to open, and where a save goes.
fn named_files(command: &Command) -> Vec<std::path::PathBuf> {
    let wire: Value = serde_json::from_str(&command.wire()).unwrap_or(Value::Null);
    ["path", "out"]
        .iter()
        .filter_map(|key| wire[*key].as_str().map(std::path::PathBuf::from))
        .collect()
}

/// Whether a file a command names is a test's: one that's there is, by where it really is; one to be made is, by the folder it goes in.
fn is_a_tests_path(path: &Path, state: Option<&Path>) -> bool {
    if std::fs::symlink_metadata(path).is_ok() {
        return is_a_tests_file(path, state);
    }
    path.parent().is_some_and(|folder| is_a_tests_file(folder, state))
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
    let command = command(line)?;
    let state = inner.serving.paths.as_ref().map(|paths| paths.root.clone());
    let open: Vec<std::path::PathBuf> = inner
        .host
        .session
        .with_session(|session| session.tabs().iter().filter_map(|tab| session.path_of(tab.id)).collect());
    if open
        .iter()
        .chain(named_files(&command).iter())
        .any(|path| !is_a_tests_path(path, state.as_deref()))
    {
        return Err(bad(
            "`qa as-person` acts only on a test's own files, in the quiet run's state folder or a temp folder, and one open or named isn't.",
        ));
    }
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
        ] {
            let refused = super::command(words).map(|_| ()).unwrap_err();
            assert!(matches!(refused.code, "usage" | "bad_command"), "{words}: {refused:?}");
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
        assert_eq!(refused.code, "usage");
        assert!(
            run(app.core(), &format!("save-as --out \"{}\"", elsewhere.with_extension("copy.pdf").display())).is_err(),
            "saving outside"
        );
        app.open(&elsewhere);
        assert!(run(app.core(), "undo").is_err(), "with a file open that isn't a test's");
        let _ = std::fs::remove_file(&elsewhere);
    }
}
