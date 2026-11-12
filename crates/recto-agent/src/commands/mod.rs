//! **The command registry** (step R2.2, guide §2.8 and §2.9). Every command is a payload type in a module of its feature, and the module holds everything about it: what it reads from the wire, its rows in the table, its handler, the plain words of its answer, and the properties that guard memory and safety. [`Cmd`] has no defaults for those, so a new command can't compile without saying whether it draws, which tab it holds, what it logs, what level it needs and where it's answered.
//!
//! Commands come in families, one enum each, under the small top-level [`Command`](crate::Command): a family is a group of the help (Read, Move, Edit…), declared below with [`family!`] in the shape of a plain enum, so the ratchet counts its variants as it would any enum's. Adding a command to a family is one line there, and one in [`PARTS`] to say where its rows go in `recto help`.
//!
//! The wire stays one flat object, `{"cmd": "follow", …}`. [`parse`](crate::parse) reads the tag first, then the line as the family that has it, internally tagged by `cmd`. Each payload refuses unknown fields itself.

pub mod access;
pub mod editing;
pub mod export;
pub mod linking;
pub mod measure;
pub mod reading;
pub mod session;
pub mod sync;
pub mod tabs;
pub mod text;

use crate::table::{self, Access, Spec};
use crate::{AgentError, Command, Event, Level, Runs, SessionAccess};
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
    /// The selector of the tab it acts on, which `execute` holds awake for as long as it runs. One that forgot could see its tab go dormant mid-command.
    fn tab(&self) -> Option<&str>;
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

/// What a command runs with: the session, the level it was asked at, and the events it makes.
pub struct Cx<'a, A> {
    pub access: &'a A,
    pub level: Level,
    pub events: &'a mut Vec<Event>,
}

/// The level of a command's row. A command with no row can't run: the table is the list.
pub fn row_access(tag: &str) -> Access {
    table::by_tag(tag).map_or(Access::Nobody, |spec| spec.access)
}

/// What a command answered by the server's own loop says if it reaches `execute`, as with a session run in one process.
pub fn needs_server() -> AgentError {
    AgentError::new("needs_server", "This needs the long-running session.", crate::exit::NOT_RUNNING)
}

/// A file's name for the log. It comes from outside, so even cleaned, it goes under `untrusted`, here as everywhere.
pub fn log_file(path: &Path) -> Value {
    json!({ "file": crate::file_label(path) })
}

/// A tab's name as typed isn't kept. `with_acted_tab` swaps in the `#id` it turned out to mean, which is what the log is for: this is only what's left if it didn't find one, such as an ambiguous name, or a command refused before it looked.
pub fn log_tab(tab: &str) -> String {
    if tab == "active" || tab.strip_prefix('#').is_some_and(|id| id.bytes().all(|b| b.is_ascii_digit()) && id.len() <= 10) {
        tab.to_string()
    } else {
        "by name".to_string()
    }
}

/// Declares a family: the enum, and everything about its commands taken from their payloads. Written as a plain enum, so the ratchet counts its variants.
macro_rules! family {
    ($(#[$meta:meta])* pub enum $family:ident { $($variant:ident($payload:ty),)* }) => {
        $(#[$meta])*
        #[derive(Clone, serde::Deserialize)]
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
            /// The wire names this family reads.
            pub const TAGS: &'static [&'static str] = &[$(<$payload as $crate::commands::Cmd>::TAG,)*];

            /// Reads a line known to name one of this family's commands.
            pub(crate) fn parse(line: &str) -> Result<$crate::Command, serde_json::Error> {
                serde_json::from_str::<Self>(line).map($crate::Command::$family)
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

            pub fn protected_ok(&self) -> bool {
                match self {
                    $(Self::$variant(_) => <$payload as $crate::commands::Cmd>::PROTECTED_OK,)*
                }
            }

            pub fn tab(&self) -> Option<&str> {
                match self {
                    $(Self::$variant(command) => $crate::commands::Cmd::tab(command),)*
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
        Goto(linking::Goto),
        Back(linking::Back),
        Forward(linking::Forward),
        Follow(linking::Follow),
        Recent(tabs::Recent),
        RecentClear(tabs::RecentClear),
        Close(tabs::Close),
    }
}

family! {
    /// Changing pages: `page move`, `page delete`, `page insert`, `edits` and `discard`. Rotate, undo, redo, clear-history and save join when the app's own code stops naming them.
    pub enum Edit {
        PageMove(editing::PageMove),
        PageDelete(editing::PageDelete),
        PageInsert(editing::PageInsert),
        Edits(editing::Edits),
        Discard(editing::Discard),
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

type Reads = fn(&str) -> Result<Command, serde_json::Error>;
type Words = fn(&str, &Value) -> Option<String>;

/// Every family: the wire names it reads, how it reads one, and its commands' plain words. One line each.
const FAMILIES: &[(&[&str], Reads, Words)] = &[
    (Start::TAGS, Start::parse, Start::human),
    (Read::TAGS, Read::parse, Read::human),
    (Move::TAGS, Move::parse, Move::human),
    (Edit::TAGS, Edit::parse, Edit::human),
    (Measure::TAGS, Measure::parse, Measure::human),
    (Waits::TAGS, Waits::parse, Waits::human),
    (Rights::TAGS, Rights::parse, Rights::human),
    (Session::TAGS, Session::parse, Session::human),
];

/// The family that reads a wire name, if one does. A name no family has is one of the commands not moved here yet, or none.
pub(crate) fn parser_for(tag: &str) -> Option<Reads> {
    FAMILIES.iter().find(|(tags, _, _)| tags.contains(&tag)).map(|(_, parse, _)| *parse)
}

/// The plain words for an answer, from the command whose answer it is.
pub(crate) fn human(tag: &str, data: &Value) -> Option<String> {
    FAMILIES.iter().find_map(|(_, _, words)| words(tag, data))
}

/// **The table, in the order `recto help` shows it.** Each command's rows, and the rows that run in `recto` itself, which have no command.
const PARTS: &[&[Spec]] = &[
    <session::Status as Cmd>::ROWS,
    table::CLI_HELP,
    <reading::Tabs as Cmd>::ROWS,
    <reading::Where as Cmd>::ROWS,
    <reading::Facts as Cmd>::ROWS,
    <reading::Contents as Cmd>::ROWS,
    <text::Search as Cmd>::ROWS,
    <text::Index as Cmd>::ROWS,
    <text::IndexStatus as Cmd>::ROWS,
    <text::IndexClear as Cmd>::ROWS,
    <text::Text as Cmd>::ROWS,
    <text::Select as Cmd>::ROWS,
    <text::Copy as Cmd>::ROWS,
    <export::Render as Cmd>::ROWS,
    <export::ExportSvg as Cmd>::ROWS,
    <export::Images as Cmd>::ROWS,
    <export::Image as Cmd>::ROWS,
    <export::Snapshot as Cmd>::ROWS,
    <export::Print as Cmd>::ROWS,
    <reading::Signatures as Cmd>::ROWS,
    <reading::Sidebar as Cmd>::ROWS,
    table::ROWS_OPEN,
    <linking::Goto as Cmd>::ROWS,
    <linking::Back as Cmd>::ROWS,
    <linking::Forward as Cmd>::ROWS,
    <linking::Links as Cmd>::ROWS,
    <linking::Follow as Cmd>::ROWS,
    table::ROWS_TAP,
    <tabs::Recent as Cmd>::ROWS,
    <tabs::RecentClear as Cmd>::ROWS,
    <tabs::Close as Cmd>::ROWS,
    <editing::PageMove as Cmd>::ROWS,
    table::ROWS_PAGE_ROTATE,
    <editing::PageDelete as Cmd>::ROWS,
    <editing::PageInsert as Cmd>::ROWS,
    table::ROWS_UNDO,
    table::ROWS_REDO,
    <editing::Edits as Cmd>::ROWS,
    <editing::Discard as Cmd>::ROWS,
    table::ROWS_CLEAR_HISTORY,
    table::ROWS_SAVE,
    <sync::Wait as Cmd>::ROWS,
    <sync::Watch as Cmd>::ROWS,
    <sync::WatchStop as Cmd>::ROWS,
    <access::PermissionShow as Cmd>::ROWS,
    <access::PermissionRequest as Cmd>::ROWS,
    table::CLI_PERMISSION_SET,
    <access::Log as Cmd>::ROWS,
    <access::Sign as Cmd>::ROWS,
    table::CLI_SERVE,
    <session::Stop as Cmd>::ROWS,
    table::CLI_LOCAL,
    <measure::Budget as Cmd>::ROWS,
    table::CLI_BUDGET_TABS,
    <measure::BudgetRender as Cmd>::ROWS,
    <measure::BudgetViewport as Cmd>::ROWS,
    table::ROWS_COMPARE,
    table::ROWS_VIEW,
    table::ROWS_PALETTE,
    table::ROWS_FIND,
    table::QA_1,
    table::ROWS_SESSION_SHOW,
    table::ROWS_SESSION_SLEEP,
    table::QA_2,
    table::ROWS_SESSION_SAVE,
    table::ROWS_SESSION_RESTORE,
    table::ROWS_SESSION_CHECK,
    table::ROWS_UPDATE_STATUS,
    table::ROWS_UPDATE_CHECK,
    table::ROWS_UPDATE_INSTALL,
    table::ROWS_UPDATE_DEV_BUILDS,
    table::ROWS_SETTINGS,
    <access::LevelChanged as Cmd>::ROWS,
];

const fn rows_in(parts: &[&[Spec]]) -> usize {
    let mut count = 0;
    let mut at = 0;
    while at < parts.len() {
        count += parts[at].len();
        at += 1;
    }
    count
}

/// The parts one after another, at compile time, so the table is one static array as it was.
const fn joined<const N: usize>(parts: &[&[Spec]]) -> [Spec; N] {
    let mut out = [table::BASE; N];
    let mut filled = 0;
    let mut at = 0;
    while at < parts.len() {
        let mut row = 0;
        while row < parts[at].len() {
            out[filled] = parts[at][row];
            filled += 1;
            row += 1;
        }
        at += 1;
    }
    assert!(filled == N);
    out
}

/// Every row, in order.
pub(crate) static ROWS: [Spec; rows_in(PARTS)] = joined(PARTS);
