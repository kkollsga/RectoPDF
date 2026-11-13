//! **A command's choices are types, and the wire is their parse and print** (step R2.5). What a typed payload holds is checked when it's read, so a
//! command that can't mean anything can't be built, by the window or from a line; and what the window builds is a line an agent could send.
//!
//! Failures caught: a value the type holds prints as a line that reads back as another value, or is refused (so the person's control and an agent's
//! command stop being one action); a row's words drift from the type's, so the CLI takes a word the type refuses, or refuses one it takes; a malformed
//! request is refused differently by route or level, or its refusal tells something of the session.

use recto_agent::commands::contents::{ContentsShow, Which};
use recto_agent::commands::edit_mode::EditMode;
use recto_agent::commands::updates::{Settings, UpdateDevBuilds};
use recto_agent::commands::words::OnOff;
use recto_agent::commands::{Edit, Move, Updates};
use recto_agent::{Command, envelope, exit, parse, table};

/// Every value of every typed payload the tests enumerate, as the commands they are.
fn every_typed_command() -> Vec<Command> {
    let mut all: Vec<Command> = Vec::new();
    for set in OnOff::ALL {
        all.push(EditMode { set }.into());
    }
    for which in Which::ALL {
        for tab in ["active", "#3", "report"] {
            all.push(ContentsShow { which, tab: tab.into() }.into());
        }
    }
    for set in [None].into_iter().chain(OnOff::ALL.map(Some)) {
        all.push(UpdateDevBuilds { set }.into());
        all.push(Settings { auto_update: set }.into());
    }
    all
}

/// The typed payload a command holds, compared as the type it is, so two values that printed alike but differ would show.
fn same(a: &Command, b: &Command) -> bool {
    match (a, b) {
        (Command::Edit(Edit::EditMode(a)), Command::Edit(Edit::EditMode(b))) => a == b,
        (Command::Move(Move::ContentsShow(a)), Command::Move(Move::ContentsShow(b))) => a == b,
        (Command::Updates(Updates::UpdateDevBuilds(a)), Command::Updates(Updates::UpdateDevBuilds(b))) => a == b,
        (Command::Updates(Updates::Settings(a)), Command::Updates(Updates::Settings(b))) => a == b,
        _ => false,
    }
}

/// Failure: a typed value the window builds has no line an agent could send for it, or its line reads back as another value.
#[test]
fn every_typed_value_is_the_line_it_writes() {
    let all = every_typed_command();
    for command in &all {
        let line = command.wire();
        let back = parse(&line).unwrap_or_else(|error| panic!("{line} is refused: {}", error.message));
        assert!(same(command, &back), "{line} reads back as {back:?}");
    }
    // Printing tells every value apart, so no two read back as one.
    let mut lines: Vec<String> = all.iter().map(Command::wire).collect();
    lines.sort();
    lines.dedup();
    assert_eq!(lines.len(), all.len(), "two values print as one line");
}

/// The words a row offers for an argument, by its key.
fn row_words(name: &str, key: &str) -> Vec<&'static str> {
    let spec = table::TABLE.iter().find(|spec| spec.name == name).unwrap_or_else(|| panic!("no row {name}"));
    let arg = spec.args.iter().find(|arg| arg.key == key).unwrap_or_else(|| panic!("{name} has no {key}"));
    match arg.kind {
        table::Kind::Choice(words) => words.to_vec(),
        other => panic!("{name} {key} is {other:?}"),
    }
}

/// Failure: the CLI's words for a choice and the type's drift apart, so `recto` offers a word the command refuses, or refuses one it takes.
#[test]
fn a_rows_words_are_its_types_words() {
    let on_off = OnOff::ALL.map(OnOff::word).to_vec();
    assert_eq!(row_words("edit mode", "set"), on_off);
    assert_eq!(row_words("update dev-builds", "set"), on_off);
    assert_eq!(row_words("settings", "auto_update"), on_off);
    assert_eq!(row_words("contents show", "which"), Which::ALL.map(Which::word).to_vec());
}

/// Requests whose words no type takes, sent as JSON, with the word that's wrong in each.
const MALFORMED: &[(&str, &str)] = &[
    (r#"{"cmd":"edit_mode","set":"sideways"}"#, "sideways"),
    (r##"{"cmd":"contents_show","which":"theirs","tab":"#1"}"##, "theirs"),
    (r#"{"cmd":"update_dev_builds","set":"maybe"}"#, ""),
    (r#"{"cmd":"settings","auto_update":"later"}"#, ""),
];

/// Failure (owner's decision on R2.5, D2 b): a malformed request is refused one way at one level and another at the next, or for the person and an agent,
/// or by the CLI and the socket; or its refusal tells something of the session, a tab, a file, a page count or a state. It's `usage` before anything is
/// looked at, so it's the same whoever sends it at whatever level (parsing knows no actor), and says only its own words. That a well-formed command is still
/// checked for its level before anything else is held by the executor's own tests, unchanged.
#[test]
fn a_malformed_request_is_usage_at_every_level_and_says_only_its_own_words() {
    for (line, word) in MALFORMED {
        let refused = parse(line).expect_err(line);
        assert_eq!((refused.code, refused.exit), ("usage", exit::USAGE), "{line}");
        assert!(
            refused.candidates.is_empty() && refused.untrusted.is_none() && refused.ask.is_none(),
            "{line}: {refused:?}"
        );
        assert!(refused.message.contains(word), "{line}: names its own word: {}", refused.message);
        let said = envelope(&Err(refused)).to_string();
        for told in ["\"tab\"", "\"page", "pages", "file", "#1", "untrusted"] {
            assert!(!said.contains(told), "{line}: the refusal tells `{told}`: {said}");
        }
    }
    // The CLI refuses the same words with the same code, before anything is sent.
    for words in [
        &["edit", "mode", "sideways"][..],
        &["contents", "show", "theirs"],
        &["update", "dev-builds", "maybe"],
        &["settings", "--auto-update", "later"],
    ] {
        let words: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
        assert_eq!(table::parse_cli(&words).expect_err("refused").code, "usage", "{words:?}");
    }
}
