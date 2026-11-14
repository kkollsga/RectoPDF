//! **The command registry** (step R2.2, guide §2.8 and §2.9). Every command is a payload type in a module of its feature, and the module holds everything about it: what it reads from the wire, its rows in the table, its handler, the plain words of its answer, and the properties that guard memory and safety. [`Cmd`] has no defaults for those, so a new command can't compile without saying whether it draws, which tab it holds, what it logs, what level it needs and where it's answered.
//!
//! Commands come in families, one enum each, under the small top-level [`Command`](crate::Command): a family is a group of the help (Read, Move, Edit…), declared below with [`family!`] in the shape of a plain enum, so the ratchet counts its variants as it would any enum's. Adding a command to a family is one line there: its rows go into the table, in its group of `recto help`, at its family's place in [`PARTS`].
//!
//! The wire stays one flat object, `{"cmd": "follow", …}`. [`parse`](crate::parse) reads the tag first, then the line as the family that has it, internally tagged by `cmd`. Each payload refuses unknown fields itself.
//!
//! # The wire and the types
//!
//! A command whose choices are words or a combination of flags (`view`, `compare`, `find`, `edit mode`…) holds them as types, so what it can't mean can't be built (step R2.5). Its wire form is a `line` struct in its module, the fields as the socket has always read them, and serde's reading of it is unchanged: the same nulls, unknown fields and positional arrays. The typed payload is `TryFrom` its line, which refuses what can't be with the words the run gave before, and prints as it (`Serialize` and `Debug`), so a command made in code is a line an agent could send. A command with no `line` is its own.
//!
//! **A malformed request is refused as malformed first** (decided on the review of R2.5, F2). What a typed payload refuses as it's read is refused before
//! anything else is looked at: before the agent's level, before the tab it names, and before whether there's an app to answer it. So a request with two
//! faults is refused for its form: `view` with no such layout and no such tab is `usage`, not `no_such_tab`; `settings --auto-update maybe` with no app is
//! `usage` (exit 2), not `needs_app`; `find --limit 0` is `bad_limit` and nine tabs to compare `too_many_panes` at any level, `off` included. None of these
//! refusals reads the session, so none tells anything of it: each says only constant words and the agent's own, cleaned. Its reply and its log line name
//! its command ([`crate::Refused`]), as a run's refusal does.

pub mod access;
pub mod compare;
pub mod contents;
pub mod edit_mode;
pub mod editing;
pub mod export;
pub mod finding;
pub mod linking;
pub mod measure;
pub mod qa;
pub mod reading;
pub mod record;
pub mod saving;
pub mod session;
pub mod sync;
pub mod tabs;
pub mod text;
pub mod updates;
pub mod view;
pub mod words;

// The scaffold's template (`tools/new-feature.sh`), compiled with the tests so a change that would break what it makes fails here.
#[cfg(test)]
#[path = "../../../../tools/new-feature/command.rs"]
pub mod scaffold_template;

use crate::Code;
use crate::table::{self, Access, Spec};
use crate::{Actor, AgentError, Command, Event, Level, Runs, SessionAccess, TabRef};
use serde_json::{Value, json};
use std::path::Path;

/// One command's payload: what it is, and everything about it. **No defaults**: the properties that guard memory and safety are spelled out by every command.
pub trait Cmd {
    /// Its `cmd` on the wire, the first column of its rows with spaces as underscores. Held to the family's serde name by the wire's golden test.
    const TAG: &'static str;
    /// Its rows in the command table: the CLI's grammar, the help and the level. Rows sharing a tag are faces of one command.
    const ROWS: &'static [Spec];
    /// It draws or reads pages through a worker. These run at most a few at a time, because each can use hundreds of megabytes: one that forgot would skip the server's draw cap.
    const DRAWS: bool;
    /// It reads what's on a tab's pages, which the tab's edits say: refused while they can't be read.
    const READS_PAGES: bool;
    /// It reads nothing of a document, so it may act on one the user opened with its password (closing it, asking where it is).
    const PROTECTED_OK: bool;
    /// **Whose document it reads, for the protected gate** in [`crate::execute`], for a command the session answers (`Runs::Session`; the app's own
    /// commands, `run_app`, have no protected gate today): the tab it names, the document being read whatever it names, or no
    /// document's. One that would read the document being read and answered `Nothing` would give an agent what the person opened with a password, so
    /// it has no default, and the walk in `policy_guards` runs every command with that document being read.
    const ABOUT: About;
    /// Run for the person at the window, it may hold its thread for long: it writes a file, draws, reads pages or a file in place, waits on a worker, a helper,
    /// the network or the clock, or on the window's session thread itself. The window runs such a command on a thread of its own, never on its session thread,
    /// which the person's every tap goes through (R2.3). One that forgot could stop the window answering.
    const WAITS: bool;
    /// The selector of the tab it acts on, which `execute` holds awake for as long as it runs. One that forgot could see its tab go dormant mid-command.
    fn tab(&self) -> Option<&TabRef>;
    /// Who may run it. Most say their row's level, [`row_access`]; some depend on what's asked.
    fn access(&self) -> Access;
    /// Where it's answered.
    fn route(&self) -> Runs;
    /// What goes in the activity log about its arguments. Numbers and cleaned file names, never anything that could hold document text.
    fn log_details(&self) -> Value;
    /// Runs it on the session. A command answered elsewhere says where.
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError>;
    /// Its answer in plain words, for a person at a terminal.
    fn human(data: &Value) -> String;
}

/// Whose document a command reads, for the protected gate: see [`Cmd::ABOUT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum About {
    /// The tab it names ([`Cmd::tab`]); none if it names none.
    Its,
    /// The document being read, the active tab, whatever it names: Find.
    Reading,
    /// No document's: what it answers about tabs (a list, the sidebar, a comparison) leaves a protected one's out itself.
    Nothing,
}

/// The tab a command's [`Cmd::ABOUT`] comes to: the one `execute` asks about before anything of it is read.
pub(crate) fn about_tab<C: Cmd>(command: &C) -> Option<&TabRef> {
    match C::ABOUT {
        About::Its => command.tab(),
        About::Reading => Some(&crate::tab_ref::ACTIVE),
        About::Nothing => None,
    }
}

/// Whether a command only asks about the state, so an update needn't wait for it. Declared by each command the app or a host's record answers, with no default: one that forgot could be refused mid-hand-off, or be run past the hand-off's last look and lose what it did.
pub trait OnlyAsks {
    fn only_asks(&self) -> bool;
}

/// What a command runs with: the session, who asked and the level that goes by ([`Actor::level`]), and the events it makes.
pub struct Cx<'a, A> {
    pub access: &'a A,
    pub actor: Actor,
    pub level: Level,
    pub events: &'a mut Vec<Event>,
}

/// The level of a command's row. A command with no row can't run: the table is the list.
pub fn row_access(tag: &str) -> Access {
    table::by_tag(tag).map_or(Access::Nobody, |spec| spec.access)
}

/// What a command answered by the server's own loop says if it reaches `execute`, as with a session run in one process.
pub fn needs_server() -> AgentError {
    AgentError::new(Code::NEEDS_SERVER, "This needs the long-running session.", crate::exit::NOT_RUNNING)
}

/// A file's name for the log. It comes from outside, so even cleaned, it goes under `untrusted`, here as everywhere.
pub fn log_file(path: &Path) -> Value {
    json!({ "file": crate::file_label(path) })
}

/// A tab's name as typed isn't kept. `with_acted_tab` swaps in the `#id` it turned out to mean, which is what the log is for: this is only what's left if it didn't find one, such as an ambiguous name, or a command refused before it looked.
pub fn log_tab(tab: &TabRef) -> String {
    tab.log()
}

/// The type a command is read from on the wire: its [line](self#the-wire-and-the-types) where it has one, or the payload itself.
macro_rules! line_of {
    ($line:ty ; $payload:ty) => {
        $line
    };
    (; $payload:ty) => {
        $payload
    };
}

/// A command read from its line: checked whole by its `TryFrom`, where it has a line of its own, or the payload as serde read it.
macro_rules! from_line {
    ($line:ty ; $payload:ty ; $read:expr) => {
        <$payload as TryFrom<$line>>::try_from($read)
    };
    (; $payload:ty ; $read:expr) => {
        Ok::<$payload, $crate::AgentError>($read)
    };
}

/// A typed payload's tag, or none for a command that's its own line.
macro_rules! typed_tag {
    ($line:ty ; $payload:ty) => {
        Some(<$payload as $crate::commands::Cmd>::TAG)
    };
    (; $payload:ty) => {
        None
    };
}

/// Declares a family: the enum, and everything about its commands taken from their payloads. Written as a plain enum, so the ratchet counts its variants.
/// A variant marked `#[line(module::line::Name)]` is a typed payload, read from that line and checked whole when it's parsed (step R2.5).
macro_rules! family {
    ($(#[$meta:meta])* pub enum $family:ident { $($(#[line($line:ty)])? $variant:ident($payload:ty),)* }) => {
        $(#[$meta])*
        #[derive(Clone, serde::Serialize)]
        #[serde(tag = "cmd", rename_all = "snake_case")]
        pub enum $family {
            $($variant($payload),)*
        }

        // The payload alone, so a command reads the same in a log or a test whichever family holds it.
        impl std::fmt::Debug for $family {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(Self::$variant(command) => command.fmt(f),)*
                }
            }
        }

        impl $family {
            /// Its commands' rows, in the order of its list: where they stand in their group of `recto help`.
            pub const ROWS: &'static [&'static [$crate::table::Spec]] = &[$(<$payload as $crate::commands::Cmd>::ROWS,)*];

            /// The wire names this family reads.
            pub const TAGS: &'static [&'static str] = &[$(<$payload as $crate::commands::Cmd>::TAG,)*];

            /// Which of them are typed payloads, read from a line of their own and checked whole (step R2.5).
            pub const TYPED: &'static [Option<&'static str>] = &[$(typed_tag!($($line)? ; $payload),)*];

            /// Reads a line known to name one of this family's commands: as serde reads its line, refused as no command if that fails, and then, for a
            /// typed payload, checked whole, refused with its own words if what it holds can't be (step R2.5). A refusal of what a line holds is the
            /// command's, by its tag, as a run's refusal was; only a line that isn't a command is [`UNKNOWN`](crate::UNKNOWN).
            pub(crate) fn parse(line: &str) -> Result<$crate::Command, $crate::Refused> {
                // Named as the family's commands are, which share their words as a family's do.
                #[allow(clippy::enum_variant_names)]
                #[derive(serde::Deserialize)]
                #[serde(tag = "cmd", rename_all = "snake_case")]
                enum Lines {
                    $($variant(line_of!($($line)? ; $payload)),)*
                }
                let read = serde_json::from_str::<Lines>(line).map_err(|_| $crate::Refused::not_a_command())?;
                Ok($crate::Command::$family(match read {
                    $(Lines::$variant(read) => Self::$variant(
                        from_line!($($line)? ; $payload ; read).map_err(|error| $crate::Refused::of(<$payload as $crate::commands::Cmd>::TAG, error))?
                    ),)*
                }))
            }

            pub fn tag(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => <$payload as $crate::commands::Cmd>::TAG,)*
                }
            }

            pub fn access(&self) -> $crate::table::Access {
                match self {
                    $(Self::$variant(command) => $crate::commands::Cmd::access(command),)*
                }
            }

            pub fn draws(&self) -> bool {
                match self {
                    $(Self::$variant(_) => <$payload as $crate::commands::Cmd>::DRAWS,)*
                }
            }

            pub fn reads_pages(&self) -> bool {
                match self {
                    $(Self::$variant(_) => <$payload as $crate::commands::Cmd>::READS_PAGES,)*
                }
            }

            pub fn waits(&self) -> bool {
                match self {
                    $(Self::$variant(_) => <$payload as $crate::commands::Cmd>::WAITS,)*
                }
            }

            pub fn protected_ok(&self) -> bool {
                match self {
                    $(Self::$variant(_) => <$payload as $crate::commands::Cmd>::PROTECTED_OK,)*
                }
            }

            pub fn tab(&self) -> Option<&TabRef> {
                match self {
                    $(Self::$variant(command) => $crate::commands::Cmd::tab(command),)*
                }
            }

            pub fn about(&self) -> Option<&TabRef> {
                match self {
                    $(Self::$variant(command) => $crate::commands::about_tab(command),)*
                }
            }

            pub fn route(&self) -> $crate::Runs {
                match self {
                    $(Self::$variant(command) => $crate::commands::Cmd::route(command),)*
                }
            }

            pub fn log_details(&self) -> serde_json::Value {
                match self {
                    $(Self::$variant(command) => $crate::commands::Cmd::log_details(command),)*
                }
            }

            pub fn run<A: $crate::SessionAccess>(&self, cx: &mut $crate::commands::Cx<'_, A>) -> Result<serde_json::Value, $crate::AgentError> {
                match self {
                    $(Self::$variant(command) => $crate::commands::Cmd::run(command, cx),)*
                }
            }

            /// The plain words for an answer of one of this family's commands.
            pub fn human(tag: &str, data: &serde_json::Value) -> Option<String> {
                $(if tag == <$payload as $crate::commands::Cmd>::TAG {
                    return Some(<$payload as $crate::commands::Cmd>::human(data));
                })*
                None
            }
        }

        $(impl From<$payload> for $crate::Command {
            fn from(command: $payload) -> Self {
                $crate::Command::$family($family::$variant(command))
            }
        })*
    };
}

family! {
    /// Where to begin.
    pub enum Start {
        Status(session::Status),
    }
}

family! {
    /// Reading a document.
    pub enum Read {
        Tabs(reading::Tabs),
        Where(reading::Where),
        Facts(reading::Facts),
        Contents(reading::Contents),
        Search(text::Search),
        Index(text::Index),
        IndexStatus(text::IndexStatus),
        IndexClear(text::IndexClear),
        Text(text::Text),
        Select(text::Select),
        Copy(text::Copy),
        Render(export::Render),
        ExportSvg(export::ExportSvg),
        Images(export::Images),
        Image(export::Image),
        Snapshot(export::Snapshot),
        Print(export::Print),
        Signatures(reading::Signatures),
        Sidebar(reading::Sidebar),
        Links(linking::Links),
    }
}

family! {
    /// Opening and moving around.
    pub enum Move {
        Open(tabs::Open),
        Goto(linking::Goto),
        Back(linking::Back),
        Forward(linking::Forward),
        Follow(linking::Follow),
        Recent(tabs::Recent),
        RecentClear(tabs::RecentClear),
        Close(tabs::Close),
        Tap(tabs::Tap),
        #[line(compare::line::Compare)]
        Compare(compare::Compare),
        ContentsFold(contents::ContentsFold),
        #[line(contents::line::ContentsShow)]
        ContentsShow(contents::ContentsShow),
        ContentsStop(contents::ContentsStop),
        #[line(view::line::View)]
        View(view::View),
        Palette(view::Palette),
        #[line(finding::line::Find)]
        Find(finding::Find),
    }
}

family! {
    /// Changing pages and saving them: rotate, move, delete, insert, the edits, discard, undo, redo, clear-history, save, and the window's edit mode.
    pub enum Edit {
        PageRotate(editing::PageRotate),
        PageMove(editing::PageMove),
        PageDelete(editing::PageDelete),
        PageInsert(editing::PageInsert),
        Edits(editing::Edits),
        Discard(editing::Discard),
        Undo(editing::Undo),
        Redo(editing::Redo),
        ClearHistory(editing::ClearHistory),
        Save(saving::Save),
        #[line(edit_mode::line::EditMode)]
        EditMode(edit_mode::EditMode),
    }
}

family! {
    /// Measuring what things cost: `budget`, `budget render` and `budget viewport`.
    pub enum Measure {
        Budget(measure::Budget),
        BudgetRender(measure::BudgetRender),
        BudgetViewport(measure::BudgetViewport),
    }
}

family! {
    /// The state record, which each host keeps: what it holds, a tab put to sleep, writing it, reading it back, and whether it's a safe moment to hand off.
    pub enum Record {
        SessionShow(record::SessionShow),
        SessionSleep(record::SessionSleep),
        SessionSave(record::SessionSave),
        SessionRestore(record::SessionRestore),
        SessionCheck(record::SessionCheck),
    }
}

family! {
    /// Updates and settings, which only the app has.
    pub enum Updates {
        UpdateStatus(updates::UpdateStatus),
        UpdateCheck(updates::UpdateCheck),
        UpdateInstall(updates::UpdateInstall),
        #[line(updates::line::UpdateDevBuilds)]
        UpdateDevBuilds(updates::UpdateDevBuilds),
        #[line(updates::line::Settings)]
        Settings(updates::Settings),
    }
}

family! {
    /// Testing the app: what an app started in QA mode shows, and making it do what the UI does. Their rows are [`qa::ROWS_QA`].
    pub enum Qa {
        QaRead(qa::QaRead),
        QaDrive(qa::QaDrive),
    }
}

family! {
    /// Waiting instead of polling.
    pub enum Waits {
        Wait(sync::Wait),
        Watch(sync::Watch),
        WatchStop(sync::WatchStop),
    }
}

family! {
    /// Access, and what agents have done.
    pub enum Rights {
        PermissionShow(access::PermissionShow),
        PermissionRequest(access::PermissionRequest),
        Log(access::Log),
        Sign(access::Sign),
        LevelChanged(access::LevelChanged),
    }
}

family! {
    /// The session itself.
    pub enum Session {
        Stop(session::Stop),
    }
}

/// A line's `cmd`, read first so the line can be read as the family that has it. Nothing else in the line is kept, or allocated for: other values are skipped as they're read.
pub(crate) fn tag_of(line: &str) -> Option<std::borrow::Cow<'_, str>> {
    struct Tag<'a>(Option<std::borrow::Cow<'a, str>>);
    impl<'de> serde::Deserialize<'de> for Tag<'de> {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_any(TagVisitor)
        }
    }
    struct TagVisitor;
    impl<'de> serde::de::Visitor<'de> for TagVisitor {
        type Value = Tag<'de>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a command")
        }
        // `{"cmd": "follow", …}`: the first `cmd`. A second is the family's to refuse.
        fn visit_map<M: serde::de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut tag = None;
            while let Some(key) = map.next_key::<std::borrow::Cow<'de, str>>()? {
                if key == "cmd" && tag.is_none() {
                    tag = Some(map.next_value::<std::borrow::Cow<'de, str>>()?);
                } else {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            Ok(Tag(tag))
        }
        // `["follow", …]`, which serde's internally tagged reading also takes.
        fn visit_seq<S: serde::de::SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
            let tag = seq.next_element::<std::borrow::Cow<'de, str>>()?;
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
            Ok(Tag(tag))
        }
    }
    serde_json::from_str::<Tag<'_>>(line).ok()?.0
}

type Reads = fn(&str) -> Result<Command, crate::Refused>;
type Words = fn(&str, &Value) -> Option<String>;
type Typed = &'static [Option<&'static str>];

/// Every family: the wire names it reads, how it reads one, its commands' plain words, and which are typed payloads. One line each.
const FAMILIES: &[(&[&str], Reads, Words, Typed)] = &[
    (Start::TAGS, Start::parse, Start::human, Start::TYPED),
    (Read::TAGS, Read::parse, Read::human, Read::TYPED),
    (Move::TAGS, Move::parse, Move::human, Move::TYPED),
    (Edit::TAGS, Edit::parse, Edit::human, Edit::TYPED),
    (Measure::TAGS, Measure::parse, Measure::human, Measure::TYPED),
    (Waits::TAGS, Waits::parse, Waits::human, Waits::TYPED),
    (Rights::TAGS, Rights::parse, Rights::human, Rights::TYPED),
    (Session::TAGS, Session::parse, Session::human, Session::TYPED),
    (Qa::TAGS, Qa::parse, Qa::human, Qa::TYPED),
    (Record::TAGS, Record::parse, Record::human, Record::TYPED),
    (Updates::TAGS, Updates::parse, Updates::human, Updates::TYPED),
];

/// The family that reads a wire name, if one does. A name no family has is one of the commands not moved here yet, or none.
pub(crate) fn parser_for(tag: &str) -> Option<Reads> {
    FAMILIES.iter().find(|(tags, ..)| tags.contains(&tag)).map(|(_, parse, ..)| *parse)
}

/// The plain words for an answer, from the command whose answer it is.
pub(crate) fn human(tag: &str, data: &Value) -> Option<String> {
    FAMILIES.iter().find_map(|(_, _, words, _)| words(tag, data))
}

/// The refusals of flags that can't go together, by command, which a typed payload makes as it's read (step R2.5).
#[cfg(any(test, feature = "test-words"))]
const COMBINATIONS: &[(&str, &str)] = &[
    ("compare", compare::OFF_ALONE),
    ("compare", compare::TOGGLE_ALONE),
    ("compare", compare::READING_ALONE),
    ("find", finding::ONE_THING),
    ("find", finding::JUMP_WITH_A_STEP),
];

/// For tests (the table's and the fuzz target's): whether a line the CLI built, each of whose words its row takes, is refused as it's read only for what its
/// words come to together. Flags that can't go together, or more tabs to compare than are read; nothing else, so a word the CLI offers and the command's
/// type refuses can't pass as one.
#[cfg(any(test, feature = "test-words"))]
pub fn refused_for_its_combination(tag: &str, error: &AgentError) -> bool {
    let words = error.message.as_str();
    (error.code == Code::USAGE && COMBINATIONS.contains(&(tag, words))) || (tag == "compare" && error.code == Code::TOO_MANY_PANES)
}

/// Whether a wire name is a typed payload's, read from a line of its own and checked whole as it's read (step R2.5).
pub fn is_typed(tag: &str) -> bool {
    FAMILIES.iter().any(|(.., typed)| typed.contains(&Some(tag)))
}

/// **The table.** Each family's rows, and the rows of the commands not in a family yet and of what runs in `recto` itself, which has no command. `recto help` shows them by group in [`GROUP_ORDER`], each group once; within a group, in the order here. A new command needs no line here: its family's list places it.
const PARTS: &[&[&[Spec]]] = &[
    Start::ROWS,
    &[table::CLI_HELP],
    Read::ROWS,
    Move::ROWS,
    Edit::ROWS,
    Waits::ROWS,
    Rights::ROWS,
    &[table::CLI_PERMISSION_SET, table::CLI_SERVE],
    Session::ROWS,
    &[table::CLI_LOCAL],
    Record::ROWS,
    Updates::ROWS,
    Measure::ROWS,
    &[table::CLI_BUDGET_TABS],
    qa::ROWS_QA,
];

const fn rows_in(parts: &[&[&[Spec]]]) -> usize {
    let mut count = 0;
    let mut list = 0;
    while list < parts.len() {
        let mut at = 0;
        while at < parts[list].len() {
            count += parts[list][at].len();
            at += 1;
        }
        list += 1;
    }
    count
}

/// The groups of `recto help`, in the order a newcomer needs them: each is shown once, with its rows in the order of [`PARTS`].
const GROUP_ORDER: &[&str] = &[
    table::GROUP_START,
    table::GROUP_READ,
    table::GROUP_MOVE,
    table::GROUP_EDIT,
    table::GROUP_SYNC,
    table::GROUP_ACCESS,
    table::GROUP_SESSION,
    table::GROUP_APP,
    table::GROUP_SKILL,
    table::GROUP_MEASURE,
    table::GROUP_QA,
];

const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut at = 0;
    while at < a.len() {
        if a[at] != b[at] {
            return false;
        }
        at += 1;
    }
    true
}

/// The parts' rows, group by group in [`GROUP_ORDER`], keeping their order within a group. A row in no listed group fails the build.
const fn grouped<const N: usize>(parts: &[&[&[Spec]]]) -> [Spec; N] {
    let mut out = [table::BASE; N];
    let mut filled = 0;
    let mut group = 0;
    while group < GROUP_ORDER.len() {
        let mut list = 0;
        while list < parts.len() {
            let mut at = 0;
            while at < parts[list].len() {
                let mut row = 0;
                while row < parts[list][at].len() {
                    if same(parts[list][at][row].group, GROUP_ORDER[group]) {
                        out[filled] = parts[list][at][row];
                        filled += 1;
                    }
                    row += 1;
                }
                at += 1;
            }
            list += 1;
        }
        group += 1;
    }
    assert!(filled == N, "a row's group isn't in GROUP_ORDER");
    out
}

/// Every row, in order.
pub(crate) static ROWS: [Spec; rows_in(PARTS)] = grouped(PARTS);
