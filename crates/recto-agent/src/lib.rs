//! The one control API. The UI, the CLI, QA and agents all go through these typed, versioned commands, and they call the
//! same session methods the UI calls.

mod render;

use recto_session::{DocLoader, RowKind, Session, SessionError, TabId};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

use render::BudgetArgs;
pub use render::DEFAULT_SCALE;

/// The envelope's version. Within a version, fields are only ever added.
pub const VERSION: u32 = 1;

/// Fixed exit codes for the CLI.
pub mod exit {
    pub const OK: i32 = 0;
    pub const FAILED: i32 = 1;
    pub const NOT_FOUND: i32 = 3;
    pub const AMBIGUOUS: i32 = 4;
    pub const PERMISSION: i32 = 5;
    /// The document is protected or locked, and only the user can do something about it. Agent report, §5.
    pub const PROTECTED: i32 = 8;
}

/// What an agent is allowed to do, set by the user. Ordered from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Off,
    ReadOnly,
    Control,
    Full,
}

impl Level {
    fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::ReadOnly => "read_only",
            Self::Control => "control",
            Self::Full => "full",
        }
    }
}

/// Every command. A feature without a command here isn't finished.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Open { path: PathBuf },
    // Empty braces, not unit variants: serde ignores extra fields on a unit variant, even with `deny_unknown_fields`.
    Tabs {},
    Sidebar {},
    Tap { tab: String },
    Goto { tab: String, page: u32 },
    Close { tab: String },
    Budget {},
    /// Draws a page to a PNG file.
    Render {
        tab: String,
        page: u32,
        #[serde(default = "render::default_scale")]
        scale: f32,
        out: PathBuf,
    },
    /// A page's text, normalised for search, with the hidden-text flag.
    Text { tab: String, page: u32 },
    /// What a worker costs: idle, per page, peak, and how often it was retired.
    BudgetRender {
        tab: String,
        pages: u32,
        /// One-based first page. The first page of the file if left out.
        #[serde(default)]
        first: Option<u32>,
        #[serde(default = "render::default_scale")]
        scale: f32,
        /// A fresh worker for every page, to see what one page costs by itself.
        #[serde(default)]
        fresh: bool,
        /// 512 px tiles across each page, rather than whole pages.
        #[serde(default)]
        tiles: bool,
        /// Each page's numbers, as well as the summary.
        #[serde(default)]
        samples: bool,
        /// Overrides the hard memory limit for this measurement, up to 4 GiB.
        #[serde(default)]
        limit_mib: Option<u32>,
        /// Overrides the retirement footprint for this measurement.
        #[serde(default)]
        retire_mib: Option<u32>,
    },
}

impl Command {
    /// The level this command needs. Nothing reads document content at Off.
    pub fn level(&self) -> Level {
        match self {
            // Measuring with the decided limits only reads numbers. Choosing other limits decides what a hostile file may use, so it isn't reading.
            Self::BudgetRender { limit_mib: Some(_), .. } | Self::BudgetRender { retire_mib: Some(_), .. } => Level::Full,
            Self::Tabs {} | Self::Sidebar {} | Self::Budget {} | Self::BudgetRender { .. } => Level::ReadOnly,
            // Reading a page's text is what read-only access is for, and it's what Off exists to refuse.
            Self::Text { .. } => Level::ReadOnly,
            // Writes to disk, so it's with `save` and `export` in the agent report's §5.2.
            Self::Render { .. } => Level::Full,
            Self::Open { .. } | Self::Tap { .. } | Self::Goto { .. } | Self::Close { .. } => Level::Control,
        }
    }
}

/// A failed command, with a stable code and a plain message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentError {
    pub code: &'static str,
    pub message: String,
    pub candidates: Vec<String>,
    pub exit: i32,
}

impl AgentError {
    fn new(code: &'static str, message: impl Into<String>, exit: i32) -> Self {
        Self { code, message: message.into(), candidates: Vec::new(), exit }
    }
}

impl From<SessionError> for AgentError {
    fn from(error: SessionError) -> Self {
        let exit = match error {
            SessionError::NoSuchTab | SessionError::Open(recto_core::OpenError::NotFound) => exit::NOT_FOUND,
            SessionError::Open(recto_core::OpenError::PasswordNeeded) => exit::PROTECTED,
            _ => exit::FAILED,
        };
        Self::new(error.code(), error.message(), exit)
    }
}

/// Wraps a result in the envelope every response uses.
pub fn envelope(result: &Result<Value, AgentError>) -> Value {
    match result {
        Ok(data) => json!({ "v": VERSION, "ok": true, "data": data }),
        Err(error) => json!({
            "v": VERSION,
            "ok": false,
            "error": { "code": error.code, "message": error.message, "candidates": error.candidates },
        }),
    }
}

/// Parses one JSON command. Unknown commands and fields are refused rather than guessed at.
pub fn parse(line: &str) -> Result<Command, AgentError> {
    serde_json::from_str(line).map_err(|_| AgentError::new("bad_command", "That isn't a command RectoPDF knows.", exit::FAILED))
}

/// Runs one command against a session, if the level allows it.
pub fn run<L: DocLoader>(session: &mut Session<L>, level: Level, command: &Command) -> Result<Value, AgentError> {
    let needed = command.level();
    if level < needed {
        let message = format!("This needs agent access level {}. Ask the user to allow it in RectoPDF's settings.", needed.name());
        return Err(AgentError::new("permission_denied", message, exit::PERMISSION));
    }
    match command {
        Command::Open { path } => {
            let id = session.open(path)?;
            Ok(json!({ "tab": id.0 }))
        }
        Command::Tabs {} => Ok(tabs_json(session)),
        Command::Sidebar {} => Ok(sidebar_json(session)),
        Command::Tap { tab } => {
            let id = select(session, tab)?;
            session.tap(id)?;
            Ok(sidebar_json(session))
        }
        Command::Goto { tab, page } => {
            let id = select(session, tab)?;
            // Pages are one-based for people and agents, and zero-based inside.
            session.go_to(id, page.saturating_sub(1))?;
            Ok(sidebar_json(session))
        }
        Command::Close { tab } => {
            let id = select(session, tab)?;
            session.close(id)?;
            Ok(tabs_json(session))
        }
        Command::Budget {} => Ok(json!({
            "tabs": session.tabs().len(),
            "footprint_bytes": recto_platform::footprint_bytes(),
            "open_handles": recto_platform::open_handle_count(),
        })),
        Command::Render { tab, page, scale, out } => {
            let id = select(session, tab)?;
            render::render(&tab_path(session, id)?, *page, *scale, out)
        }
        Command::Text { tab, page } => {
            let id = select(session, tab)?;
            render::text(&tab_path(session, id)?, *page)
        }
        Command::BudgetRender { tab, pages, first, scale, fresh, tiles, samples, limit_mib, retire_mib } => {
            let id = select(session, tab)?;
            let args = BudgetArgs {
                pages: *pages,
                first: *first,
                scale: *scale,
                fresh: *fresh,
                tiles: *tiles,
                samples: *samples,
                limit_mib: *limit_mib,
                retire_mib: *retire_mib,
            };
            render::budget(&tab_path(session, id)?, &args)
        }
    }
}

fn tab_path<L: DocLoader>(session: &Session<L>, id: TabId) -> Result<PathBuf, AgentError> {
    session.path_of(id).ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))
}

/// Finds a tab by `#id` or by a piece of its name. Selectors never guess: an ambiguous name lists the candidates.
fn select<L: DocLoader>(session: &Session<L>, selector: &str) -> Result<TabId, AgentError> {
    let not_found = || AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND);
    if let Some(number) = selector.strip_prefix('#') {
        let id = TabId(number.parse().map_err(|_| not_found())?);
        return session.tab(id).map(|_| id).ok_or_else(not_found);
    }
    let needle = selector.to_lowercase();
    let matches: Vec<_> = session.tabs().iter().filter(|t| t.name.to_lowercase().contains(&needle)).collect();
    match matches.as_slice() {
        [] => Err(not_found()),
        [one] => Ok(one.id),
        many => {
            let mut error = AgentError::new("ambiguous", "More than one tab matches that name.", exit::AMBIGUOUS);
            error.candidates = many.iter().map(|t| format!("#{} {}", t.id.0, t.name)).collect();
            Err(error)
        }
    }
}

fn tabs_json<L: DocLoader>(session: &Session<L>) -> Value {
    let tabs: Vec<Value> = session
        .tabs()
        .iter()
        .map(|t| {
            json!({
                "tab": t.id.0,
                "name": &*t.name,
                "page": t.page + 1,
                "pages": t.page_count,
                "active": session.active() == Some(t.id),
                "problem": t.problem.map(|p| p.message()),
            })
        })
        .collect();
    json!({ "tabs": tabs })
}

fn sidebar_json<L: DocLoader>(session: &Session<L>) -> Value {
    let model = session.sidebar();
    let rows: Vec<Value> = model
        .rows
        .iter()
        .map(|row| {
            json!({
                "kind": match row.kind { RowKind::Tab => "tab", RowKind::Contents => "contents" },
                "tab": row.tab.0,
                "text": row.text,
                "page": row.page,
                "depth": row.depth,
                "current": row.current,
            })
        })
        .collect();
    json!({ "rows": rows, "rail_filled": model.rail_filled, "toc_open": model.toc_open, "place": model.place, "edited": model.edited })
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_core::{DocFacts, OpenError, Toc};
    use std::path::Path;

    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts { page_count: 10, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default() })
        }
    }

    fn with_tabs(names: &[&str]) -> Session<Fake> {
        let mut session = Session::new(Fake);
        for name in names {
            session.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap();
        }
        session
    }

    /// Failure: an agent with too little access changes or reads something it wasn't allowed to.
    #[test]
    fn levels_refuse_what_they_dont_allow() {
        let mut session = with_tabs(&["a"]);
        let off = run(&mut session, Level::Off, &Command::Sidebar {}).unwrap_err();
        assert_eq!(off.code, "permission_denied");
        assert_eq!(off.exit, exit::PERMISSION);
        let read_only = run(&mut session, Level::ReadOnly, &Command::Close { tab: "#1".into() }).unwrap_err();
        assert_eq!(read_only.code, "permission_denied");
        assert_eq!(session.tabs().len(), 1);
        assert!(run(&mut session, Level::ReadOnly, &Command::Tabs {}).is_ok());
    }

    /// Failure: an agent the user turned off reads a page's text, the one thing Off exists to stop. Asserted on the level itself and on the
    /// call, which must refuse before any worker starts.
    #[test]
    fn reading_page_text_needs_more_than_off() {
        let text = Command::Text { tab: "#1".into(), page: 1 };
        assert!(text.level() > Level::Off);
        assert_eq!(text.level(), Level::ReadOnly);
        let mut session = with_tabs(&["a"]);
        let refused = run(&mut session, Level::Off, &text).unwrap_err();
        assert_eq!((refused.code, refused.exit), ("permission_denied", exit::PERMISSION));
        let render = Command::Render { tab: "#1".into(), page: 1, scale: 2.0, out: "/tmp/x.png".into() };
        assert_eq!(run(&mut session, Level::Control, &render).unwrap_err().code, "permission_denied", "it writes to disk, so it needs Full");
    }

    fn budget_command(limit_mib: Option<u32>, retire_mib: Option<u32>) -> Command {
        Command::BudgetRender { tab: "#1".into(), pages: 1, first: None, scale: 2.0, fresh: false, tiles: false, samples: false, limit_mib, retire_mib }
    }

    /// Failure: a read-only agent raises a worker's memory limit to 4 GiB and points it at a hostile file. Choosing limits needs Full, measuring doesn't.
    #[test]
    fn raising_a_workers_limits_needs_full() {
        assert_eq!(budget_command(None, None).level(), Level::ReadOnly);
        assert_eq!(budget_command(Some(4096), None).level(), Level::Full);
        assert_eq!(budget_command(None, Some(4096)).level(), Level::Full);
        let mut session = with_tabs(&["a"]);
        let refused = run(&mut session, Level::Control, &budget_command(Some(4096), None)).unwrap_err();
        assert_eq!(refused.code, "permission_denied");
    }

    /// Failure: a password-protected document reads as a general failure, so an agent retries rather than asking the user. The agent report gives it its own code.
    #[test]
    fn a_protected_document_has_its_own_exit_code() {
        let error = AgentError::from(recto_render_service::RenderFailure::NeedsPassword);
        assert_eq!((error.code, error.exit), ("needs_password", exit::PROTECTED));
        assert_eq!(exit::PROTECTED, 8);
        let opening = AgentError::from(SessionError::Open(OpenError::PasswordNeeded));
        assert_eq!(opening.exit, exit::PROTECTED, "the session refusing it first must say the same");
    }

    /// Failure: a mistyped or extra field on the new commands is silently ignored, and an agent believes it did something.
    #[test]
    fn render_commands_parse_strictly() {
        assert!(matches!(parse(r##"{"cmd":"text","tab":"#1","page":2}"##), Ok(Command::Text { page: 2, .. })));
        assert!(parse(r##"{"cmd":"text","tab":"#1"}"##).is_err(), "the page is required");
        assert!(parse(r##"{"cmd":"text","tab":"#1","page":1,"extra":1}"##).is_err());
        let Ok(Command::Render { scale, .. }) = parse(r##"{"cmd":"render","tab":"#1","page":1,"out":"a.png"}"##) else { panic!() };
        assert_eq!(scale, DEFAULT_SCALE);
        assert!(matches!(parse(r##"{"cmd":"budget_render","tab":"#1","pages":5}"##), Ok(Command::BudgetRender { .. })));
        assert!(parse(r##"{"cmd":"budget_render","tab":"#1","pages":5,"typo":1}"##).is_err());
    }

    /// Failure: an ambiguous name acts on whichever tab came first, rather than listing the candidates.
    #[test]
    fn ambiguous_names_list_candidates_and_never_guess() {
        let mut session = with_tabs(&["Report 2025", "Report 2026", "Invoice"]);
        let error = run(&mut session, Level::Control, &Command::Tap { tab: "report".into() }).unwrap_err();
        assert_eq!(error.code, "ambiguous");
        assert_eq!(error.exit, exit::AMBIGUOUS);
        assert_eq!(error.candidates.len(), 2);
        assert!(run(&mut session, Level::Control, &Command::Tap { tab: "invoice".into() }).is_ok());
    }

    /// Failure: the tap command doesn't reach the session's rule, so agents and QA test something the UI doesn't do.
    #[test]
    fn tap_uses_the_sidebar_rule() {
        let mut session = with_tabs(&["a"]);
        let result = run(&mut session, Level::Control, &Command::Tap { tab: "#1".into() }).unwrap();
        assert_eq!(result["toc_open"], false);
    }

    /// Failure: an unknown command or field is silently ignored, and an agent believes it did something.
    #[test]
    fn unknown_commands_and_fields_are_refused() {
        assert_eq!(parse(r#"{"cmd":"format_disk"}"#).unwrap_err().code, "bad_command");
        assert_eq!(parse(r#"{"cmd":"tabs","extra":1}"#).unwrap_err().code, "bad_command");
        assert!(matches!(parse(r#"{"cmd":"tabs"}"#), Ok(Command::Tabs {})));
    }

    /// Failure: the envelope changes shape, which breaks every agent reading it.
    #[test]
    fn the_envelope_always_says_its_version_and_outcome() {
        let ok = envelope(&Ok(json!({ "x": 1 })));
        assert_eq!((&ok["v"], &ok["ok"]), (&json!(VERSION), &json!(true)));
        let failed = envelope(&Err(AgentError::new("c", "m", exit::FAILED)));
        assert_eq!((&failed["ok"], &failed["error"]["code"]), (&json!(false), &json!("c")));
    }
}
