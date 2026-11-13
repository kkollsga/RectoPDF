//! **A command's choices are types, and the wire is their parse and print** (step R2.5). What a typed payload holds is checked when it's read, so a
//! command that can't mean anything can't be built, by the window or from a line; and what the window builds is a line an agent could send.
//!
//! Failures caught: a value the type holds prints as a line that reads back as another value, or is refused (so the person's control and an agent's
//! command stop being one action); a row's words drift from the type's, so the CLI takes a word the type refuses, or refuses one it takes; a malformed
//! request is refused differently by route or level, or its refusal tells something of the session.

use recto_agent::commands::compare::{Compare, Panes};
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
    let tabs = |list: &[&str]| Panes::new(list.iter().map(|tab| TabRef::from(*tab)).collect()).expect("tabs a list can say");
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

/// Requests whose words no type takes, sent as JSON, each with the word that's wrong in it after a tab. One file, which the fuzz crate's walk of refusals
/// made as a line is read starts from too.
const MALFORMED: &str = include_str!("golden/malformed.txt");

fn malformed() -> impl Iterator<Item = (&'static str, &'static str)> {
    MALFORMED.lines().map(|entry| entry.split_once('\t').unwrap_or((entry, "")))
}

/// Failure (owner's decision on R2.5, D2 b): a malformed request is refused by the socket one way and by the CLI another, or isn't refused until it runs,
/// or its refusal tells something of the session, a tab, a file, a page count or a state. It's refused as `usage` as its line is read, before any actor or
/// level is known (parsing knows neither, so it's the same at every level and for the person and an agent alike), and says only its own words; the CLI
/// refuses the same words as `usage`. That a well-formed command is still checked for its level before anything else is held by the executor's own tests.
#[test]
fn a_malformed_request_is_refused_as_usage_as_its_read_saying_only_its_own_words() {
    for (line, word) in malformed() {
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

/// Failure (review of R2.5, F1): a compare whose tabs code built prints as a list that reads back as other tabs: a name with a comma as two, one with
/// spaces at its ends as another name, an empty one as none, or more than a list is read as refused. Such tabs make no compare; those a list can say read
/// back as themselves.
#[test]
fn a_compare_built_in_code_reads_back_as_itself() {
    for name in ["a,b", " report", "report ", ""] {
        assert!(Panes::new(vec![TabRef::parse(name), TabRef::parse("#2")]).is_none(), "{name:?}");
    }
    assert!(Panes::new(vec![TabRef::Active; 9]).is_none(), "more than are read");
    for list in [&["re port", "#2"][..], &["#+3", "#007", "active", "#x"], &["#1"; 8]] {
        let built = Compare::Set {
            tabs: Panes::new(list.iter().map(|tab| TabRef::parse(tab)).collect()).unwrap_or_else(|| panic!("{list:?}")),
            sync: None,
        };
        let line = Command::from(built.clone()).wire();
        let back = parse(&line);
        assert!(
            back.as_ref().is_ok_and(|back| same(back, &Command::from(built.clone()))),
            "{list:?}: {line} reads back as {back:?}"
        );
    }
}
