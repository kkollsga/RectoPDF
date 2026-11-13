//! **A command's choices are types, and the wire is their parse and print** (step R2.5). What a typed payload holds is checked when it's read, so a
//! command that can't mean anything can't be built, by the window or from a line; and what the window builds is a line an agent could send.
//!
//! Failures caught: a value the type holds prints as a line that reads back as another value, or is refused (so the person's control and an agent's
//! command stop being one action); a row's words drift from the type's, so the CLI takes a word the type refuses, or refuses one it takes; a malformed
//! request is refused differently by route or level, or its refusal tells something of the session.

use recto_agent::commands::compare::Compare;
use recto_agent::commands::contents::{ContentsShow, Which};
use recto_agent::commands::edit_mode::EditMode;
use recto_agent::commands::finding::{Find, FindAsk};
use recto_agent::commands::updates::{Settings, UpdateDevBuilds};
use recto_agent::commands::view::{PaperChoice, Shown, View};
use recto_agent::commands::words::OnOff;
use recto_agent::commands::{Edit, Move, Updates};
use recto_agent::{Command, TabRef, envelope, exit, parse, table};
use recto_session::TabId;

/// Every value of every typed payload the tests enumerate, as the commands they are.
fn every_typed_command() -> Vec<Command> {
    let mut all: Vec<Command> = Vec::new();
    for set in OnOff::ALL {
        all.push(EditMode { set }.into());
    }
    for which in Which::ALL {
        for tab in ["active", "#3", "report"] {
            all.push(ContentsShow { which, tab: TabRef::from(tab) }.into());
        }
    }
    for set in [None].into_iter().chain(OnOff::ALL.map(Some)) {
        all.push(UpdateDevBuilds { set }.into());
        all.push(Settings { auto_update: set }.into());
    }
    let tabs = |list: &[&str]| list.iter().map(|tab| TabRef::from(*tab)).collect::<Vec<TabRef>>();
    all.extend([Compare::Look, Compare::Off].map(Command::from));
    for sync in [None].into_iter().chain(OnOff::ALL.map(Some)) {
        for list in [&["#1", "#2"][..], &["#1", "report", "#3"]] {
            all.push(Compare::Set { tabs: tabs(list), sync }.into());
        }
    }
    for sync in OnOff::ALL {
        all.push(Compare::Sync(sync).into());
    }
    for tab in ["#2", "report", "active"] {
        all.push(Compare::Toggle(TabRef::from(tab)).into());
        all.push(Compare::Reading(TabRef::from(tab)).into());
    }
    let asks = [
        FindAsk::Look,
        FindAsk::Query("words".into()),
        FindAsk::Query(String::new()),
        FindAsk::Step { forward: true, jump: false },
        FindAsk::Step { forward: false, jump: false },
        FindAsk::Step { forward: true, jump: true },
        FindAsk::Step { forward: false, jump: true },
        FindAsk::Hit(0),
        FindAsk::Hit(7),
        FindAsk::Open,
        FindAsk::Close,
    ];
    for ask in asks {
        for limit in [None, std::num::NonZeroU32::new(1), std::num::NonZeroU32::new(200)] {
            all.push(Find { ask: ask.clone(), limit }.into());
        }
    }
    let maybe = |words: &[Option<&str>]| words.iter().map(|word| word.map(str::to_string)).collect::<Vec<_>>();
    let tab_refs = [None, Some(TabRef::Active), Some(TabRef::Id(TabId(2))), Some(TabRef::from("report"))];
    for paper in [None].into_iter().chain(PaperChoice::ALL.map(Some)) {
        for sidebar in [None].into_iter().chain(Shown::ALL.map(Some)) {
            for agent_icon in [None].into_iter().chain(Shown::ALL.map(Some)) {
                for layout in [None].into_iter().chain(recto_session::record::Spread::ALL.map(Some)) {
                    for tab in &tab_refs {
                        for zoom in maybe(&[None, Some("fit"), Some("150%")]) {
                            all.push(
                                View {
                                    paper,
                                    sidebar,
                                    agent_icon,
                                    zoom,
                                    layout,
                                    tab: tab.clone(),
                                }
                                .into(),
                            );
                        }
                    }
                }
            }
        }
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
        (Command::Move(Move::View(a)), Command::Move(Move::View(b))) => a == b,
        (Command::Move(Move::Compare(a)), Command::Move(Move::Compare(b))) => a == b,
        (Command::Move(Move::Find(a)), Command::Move(Move::Find(b))) => a == b,
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
    assert_eq!(row_words("compare", "sync"), on_off);
    assert_eq!(row_words("view", "paper"), PaperChoice::ALL.map(PaperChoice::word).to_vec());
    assert_eq!(row_words("view", "sidebar"), Shown::ALL.map(Shown::word).to_vec());
    assert_eq!(row_words("view", "agent_icon"), Shown::ALL.map(Shown::word).to_vec());
    assert_eq!(
        row_words("view", "layout"),
        recto_session::record::Spread::ALL.map(recto_session::record::Spread::name).to_vec()
    );
}

/// Requests whose words no type takes, sent as JSON, with the word that's wrong in each.
const MALFORMED: &[(&str, &str)] = &[
    (r#"{"cmd":"edit_mode","set":"sideways"}"#, "sideways"),
    (r##"{"cmd":"contents_show","which":"theirs","tab":"#1"}"##, "theirs"),
    (r#"{"cmd":"update_dev_builds","set":"maybe"}"#, ""),
    (r#"{"cmd":"settings","auto_update":"later"}"#, ""),
    (r#"{"cmd":"view","paper":"purple"}"#, "purple"),
    (r#"{"cmd":"view","sidebar":"sideways"}"#, "sideways"),
    (r#"{"cmd":"view","agent_icon":"blink"}"#, "blink"),
    (r##"{"cmd":"view","layout":"three-up","tab":"#1"}"##, "three-up"),
    // A good layout with a bad paper: refused whole, so nothing can be laid out first.
    (r#"{"cmd":"view","layout":"two-up","paper":"purple"}"#, "purple"),
    (r#"{"cmd":"compare","sync":"maybe"}"#, "maybe"),
    (r##"{"cmd":"compare","off":true,"toggle":"#2"}"##, "--off"),
    (r##"{"cmd":"compare","tabs":"#1,#2","reading":"#2"}"##, "--reading"),
    (r##"{"cmd":"compare","toggle":"#2","sync":"on"}"##, "--toggle"),
    (r#"{"cmd":"find","query":"q","next":true}"#, "more than one thing"),
    (r#"{"cmd":"find","next":true,"previous":true}"#, "more than one thing"),
    (r#"{"cmd":"find","open":true,"close":true}"#, "more than one thing"),
    (r#"{"cmd":"find","hit":2,"close":true}"#, "more than one thing"),
    (r#"{"cmd":"find","jump":true}"#, "--jump"),
    (r#"{"cmd":"find","jump":true,"hit":1}"#, "--jump"),
];

/// Failure (owner's decision on R2.5, D2 b): a malformed request is refused one way at one level and another at the next, or for the person and an agent,
/// or by the CLI and the socket; or its refusal tells something of the session, a tab, a file, a page count or a state. It's `usage` before anything is
/// looked at, so it's the same whoever sends it at whatever level (parsing knows no actor), and says only its own words. That a well-formed command is still
/// checked for its level before anything else is held by the executor's own tests, unchanged.
#[test]
fn a_malformed_request_is_usage_at_every_level_and_says_only_its_own_words() {
    for (line, word) in MALFORMED {
        let refused = parse(line).expect_err(line);
        assert_eq!((refused.code.as_str(), refused.exit), ("usage", exit::USAGE), "{line}");
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
    // The CLI refuses the same words with the same code: a word by its row, before anything is sent, and a combination as its line is read.
    for words in [
        &["edit", "mode", "sideways"][..],
        &["contents", "show", "theirs"],
        &["update", "dev-builds", "maybe"],
        &["settings", "--auto-update", "later"],
        &["view", "--paper", "purple"],
        &["view", "--layout", "three-up"],
        &["compare", "--sync", "maybe"],
        &["find", "--next", "--previous"],
        &["find", "--jump"],
    ] {
        let words: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
        let refused = table::parse_cli(&words).and_then(|line| line.command()).expect_err("refused");
        assert_eq!(refused.code, "usage", "{words:?}");
    }
}
