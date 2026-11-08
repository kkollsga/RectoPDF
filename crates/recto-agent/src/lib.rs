//! The one control API. The UI, the CLI, QA and agents all go through these typed, versioned commands, and they call the
//! same session methods the UI calls.
//!
//! This crate holds no state of its own. A caller brings a session through [`SessionAccess`], and [`execute`] takes the session only for as long as each step needs it: reading a file or drawing
//! a page happens outside, so one slow PDF never stalls another client. [`table`] is the one list of commands, and the CLI grammar, `recto help`, `SKILL.md` and the level checks all come from it.

pub mod generated;
pub mod human;
pub mod index;
mod loader;
mod reading;
mod render;
pub use render::{output_exists, publish_picture};
mod search;
mod selecting;
pub mod skill;
pub mod table;

use recto_session::{Author, DocLoader, DrawNote, PageSource, RowKind, Session, SessionError, TabId};
use serde::Deserialize;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use loader::DrawableLoader;
use render::BudgetArgs;
pub use render::DEFAULT_SCALE;
pub use table::{Access, Spec};

/// The envelope's version. Within a version, fields are only ever added.
pub const VERSION: u32 = 1;

/// Fixed exit codes for the CLI.
pub mod exit {
    pub const OK: i32 = 0;
    pub const FAILED: i32 = 1;
    /// The command line itself was wrong.
    pub const USAGE: i32 = 2;
    pub const NOT_FOUND: i32 = 3;
    pub const AMBIGUOUS: i32 = 4;
    pub const PERMISSION: i32 = 5;
    /// No RectoPDF session is running.
    pub const NOT_RUNNING: i32 = 6;
    /// Something ran out of time. Agent report, §5.
    pub const TIMED_OUT: i32 = 7;
    /// The document is protected or locked, and only the user can do something about it. Agent report, §5.
    pub const PROTECTED: i32 = 8;
    /// The session is too busy for this right now. Try again.
    pub const BUSY: i32 = 10;
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
    /// The name used on the command line and in JSON.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::ReadOnly => "read_only",
            Self::Control => "control",
            Self::Full => "full",
        }
    }

    /// The name shown to people.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::ReadOnly => "Read-only",
            Self::Control => "Control",
            Self::Full => "Full",
        }
    }

    /// Reads a level's name. `read-only` and `read_only` are the same word.
    pub fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().replace('-', "_").as_str() {
            "off" => Some(Self::Off),
            "read_only" | "readonly" => Some(Self::ReadOnly),
            "control" => Some(Self::Control),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

fn active_tab() -> String {
    "active".into()
}

fn default_wait_ms() -> u64 {
    DEFAULT_WAIT_MS
}

/// How long `wait` waits if not told.
pub const DEFAULT_WAIT_MS: u64 = 30_000;
/// The longest any wait or watch may run, so a forgotten one can't stay for ever.
pub const MAX_WAIT_MS: u64 = 24 * 60 * 60 * 1000;
/// How long `watch` runs if not told.
pub const DEFAULT_WATCH_MS: u64 = 10 * 60 * 1000;

/// What `wait` can wait for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// A tab matching the selector is open.
    TabOpen,
    /// No tab matches the selector any more.
    TabClosed,
    /// The page has finished drawing, or failed to.
    PageDrawn,
    /// The search index for every file behind this tab is finished. Asking counts as wanting it built.
    IndexingDone,
    /// The tab's generated contents are ready. Asking counts as wanting them made.
    ContentsGenerated,
}

/// Every command. A feature without a command here isn't finished.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Open {
        path: PathBuf,
        /// One-based page to land on.
        #[serde(default)]
        page: Option<u32>,
    },
    Tabs {
        /// Tabs to skip, from `next_from` in the last answer.
        #[serde(default)]
        from: Option<u32>,
        #[serde(default)]
        limit: Option<u32>,
    },
    // Empty braces, not unit variants: serde ignores extra fields on a unit variant, even with `deny_unknown_fields`.
    Sidebar {},
    Tap {
        #[serde(default = "active_tab")]
        tab: String,
    },
    Goto {
        #[serde(default = "active_tab")]
        tab: String,
        page: u32,
    },
    Close {
        #[serde(default = "active_tab")]
        tab: String,
    },
    Budget {},
    /// How the app shows pages.
    View {
        #[serde(default)]
        paper: Option<String>,
        #[serde(default)]
        sidebar: Option<String>,
        #[serde(default)]
        agent_icon: Option<String>,
        #[serde(default)]
        zoom: Option<String>,
        /// The active tab's layout: `single`, `two-up` or `two-up-cover`.
        #[serde(default)]
        layout: Option<String>,
    },
    /// Tabs side by side: which, and separate or synced scroll. With nothing, only says.
    Compare {
        /// Tabs by selector, with commas between: `#1,#3`.
        #[serde(default)]
        tabs: Option<String>,
        /// `on` for synced scroll, `off` for separate.
        #[serde(default)]
        sync: Option<String>,
        /// Back to one tab, the one being read.
        #[serde(default)]
        off: bool,
    },
    /// Asks the running app what it shows. Only an app started in QA mode answers.
    QaRead {
        verb: String,
        #[serde(default)]
        out: Option<PathBuf>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Has the running app do what the UI does. Only an app started in QA mode answers.
    QaDrive {
        verb: String,
        value: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Draws a page to a PNG file.
    Render {
        #[serde(default = "active_tab")]
        tab: String,
        page: u32,
        /// A number, or `"max"` for the largest that fits, which reads as [`render::LARGEST_THAT_FITS`].
        #[serde(default = "render::default_scale", deserialize_with = "render::scale_or_max")]
        scale: f32,
        out: PathBuf,
    },
    /// Writes a page, or a box on it, to a new SVG file.
    ExportSvg {
        #[serde(default = "active_tab")]
        tab: String,
        page: u32,
        out: PathBuf,
        /// `x, y, width, height`, in points from the page's top left.
        #[serde(default)]
        rect: Option<[f32; 4]>,
        /// `both`, `paths` or `selectable`.
        #[serde(default)]
        text: Option<String>,
        /// The scale of anything that has to go as a picture.
        #[serde(default = "render::default_scale")]
        scale: f32,
        /// Takes out of JPEGs what isn't the picture, and the document's title.
        #[serde(default)]
        strip_metadata: bool,
    },
    /// A page's text, normalised for search, with the hidden-text flag.
    Text {
        #[serde(default = "active_tab")]
        tab: String,
        page: u32,
    },
    /// What a click, a drag or a range of characters takes: the text, and where it shows on each page.
    Select {
        #[serde(default = "active_tab")]
        tab: String,
        #[serde(default)]
        page: Option<u32>,
        #[serde(default)]
        to_page: Option<u32>,
        #[serde(default)]
        chars: Option<String>,
        #[serde(default)]
        at: Option<String>,
        #[serde(default)]
        to: Option<String>,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        anyway: bool,
    },
    /// The text ⌘C would put on the clipboard for the same pick. It comes back here, and no clipboard is touched.
    Copy {
        #[serde(default = "active_tab")]
        tab: String,
        #[serde(default)]
        page: Option<u32>,
        #[serde(default)]
        to_page: Option<u32>,
        #[serde(default)]
        chars: Option<String>,
        #[serde(default)]
        at: Option<String>,
        #[serde(default)]
        to: Option<String>,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        anyway: bool,
    },
    /// What a worker costs: idle, per page, peak, and how often it was retired.
    BudgetRender {
        #[serde(default = "active_tab")]
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
    /// What a reader's window costs: first view, one scroll step, one zoom step, with peak footprint.
    BudgetViewport {
        #[serde(default = "active_tab")]
        tab: String,
        /// `WIDTHxHEIGHT` in device pixels, such as `3024x1964`.
        viewport: String,
        /// One-based first page. The first page of the file if left out.
        #[serde(default)]
        first: Option<u32>,
        #[serde(default = "render::default_scale")]
        scale: f32,
        /// `per_tile` or `region`, for comparing with the default, which picks by page.
        #[serde(default)]
        strategy: Option<String>,
    },
    /// Where a tab is: its page, its length and whether it has unsaved edits.
    Where {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// A document's facts: pages, size, version, what could be read of its structure.
    Facts {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// A document's contents, in pieces.
    Contents {
        #[serde(default = "active_tab")]
        tab: String,
        /// Entries to skip, from `next_from` in the last answer.
        #[serde(default)]
        from: Option<u32>,
        #[serde(default)]
        limit: Option<u32>,
        /// Deepest nesting to include. 0 is the top level.
        #[serde(default)]
        depth: Option<u8>,
        /// The ones made from the document's headings, rather than the file's own.
        #[serde(default)]
        generated: bool,
    },
    /// Finds words across a document, from its index if that's finished, and otherwise by reading its pages.
    Search {
        #[serde(default = "active_tab")]
        tab: String,
        query: String,
        /// `4` or `4-6`. All pages if left out.
        #[serde(default)]
        pages: Option<String>,
        #[serde(default)]
        limit: Option<u32>,
        /// Reads the pages even if there's an index, to check one against the other.
        #[serde(default)]
        scan: bool,
        /// The query is a regular expression, run by ripgrep's engine in this process.
        #[serde(default)]
        regex: bool,
        /// Best pages first, by a count of the words, and not every hit.
        #[serde(default)]
        rank: bool,
        /// Lines around each hit.
        #[serde(default)]
        context: Option<u32>,
    },
    /// Asks for a document's search index to be built if it isn't, and says where it stands. Never waits.
    Index {
        #[serde(default = "active_tab")]
        tab: String,
        /// Also writes the folder an agent reads, one file per page, once the index is finished.
        #[serde(default)]
        agent: bool,
    },
    /// Where a document's search index stands. Starts nothing.
    IndexStatus {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// Throws away a document's search index, or all of them. It's extracted text, so someone may want it gone.
    IndexClear {
        #[serde(default = "active_tab")]
        tab: String,
        #[serde(default)]
        all: bool,
    },
    /// What the file says about signatures. Facts only, never a verdict.
    Signatures {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// The unsaved page edits on a tab.
    Edits {
        #[serde(default = "active_tab")]
        tab: String,
    },
    PageMove {
        #[serde(default = "active_tab")]
        tab: String,
        pages: String,
        to: u32,
    },
    PageRotate {
        #[serde(default = "active_tab")]
        tab: String,
        pages: String,
        degrees: i32,
    },
    PageDelete {
        #[serde(default = "active_tab")]
        tab: String,
        pages: String,
    },
    PageInsert {
        #[serde(default = "active_tab")]
        tab: String,
        path: PathBuf,
        at: u32,
    },
    Undo {
        #[serde(default = "active_tab")]
        tab: String,
    },
    Redo {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// Throws away a tab's edits. They can't be got back, so below Full it refuses if any are the user's.
    Discard {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// Puts a tab to sleep now, as being left alone for five minutes would.
    SessionSleep {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// Clears a tab's edit history: every step undo or redo could take, and its file. Unsaved edits stay. Below Full it refuses if any are the user's.
    ClearHistory {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// Doesn't exist yet, and says so.
    Save {
        #[serde(default = "active_tab")]
        tab: String,
    },
    /// Never reachable by an agent. It exists to say so.
    Sign {},
    /// The session: is it there, which version, what level.
    Status {},
    PermissionShow {},
    /// Asks the user for more access. Grants nothing by itself.
    PermissionRequest {
        level: Level,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// The level's file changed: read it again. Carries no level, so it can't raise one.
    LevelChanged {},
    /// Blocks until a condition holds, or the time runs out.
    Wait {
        condition: Condition,
        #[serde(default = "active_tab")]
        tab: String,
        #[serde(default)]
        page: Option<u32>,
        #[serde(default = "default_wait_ms")]
        timeout_ms: u64,
    },
    /// Streams events, one line each.
    Watch {
        #[serde(default)]
        events: Option<Vec<String>>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        count: Option<u32>,
    },
    /// Ends every watch running on this session.
    WatchStop {},
    /// What agents have done, newest last.
    Log {
        #[serde(default)]
        last: Option<u32>,
    },
    /// Ends the session.
    Stop {},
    /// What the state record holds and what's become of it.
    SessionShow {},
    /// Writes the state record now and reads it back.
    SessionSave {},
    /// Puts the session back as the state record has it.
    SessionRestore {},
    /// Whether this is a safe moment to hand off to a new version.
    SessionCheck {},
    /// What's running, what's on offer, and how the last update went.
    UpdateStatus {},
    /// Looks for a new version now, and says what it found.
    UpdateCheck {},
    /// Starts installing what's on offer, with the app's state carried across.
    UpdateInstall {},
    /// The app's settings. With a value, changes it.
    Settings {
        #[serde(default)]
        auto_update: Option<String>,
    },
}

impl Command {
    /// The name on the wire, which is the first column of the command table with spaces as underscores. Spelled out so a new command can't compile without one.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Tabs { .. } => "tabs",
            Self::Sidebar {} => "sidebar",
            Self::Tap { .. } => "tap",
            Self::Goto { .. } => "goto",
            Self::Close { .. } => "close",
            Self::Budget {} => "budget",
            Self::View { .. } => "view",
            Self::Compare { .. } => "compare",
            Self::QaRead { .. } => "qa_read",
            Self::QaDrive { .. } => "qa_drive",
            Self::Render { .. } => "render",
            Self::ExportSvg { .. } => "export_svg",
            Self::Text { .. } => "text",
            Self::Select { .. } => "select",
            Self::Copy { .. } => "copy",
            Self::BudgetRender { .. } => "budget_render",
            Self::BudgetViewport { .. } => "budget_viewport",
            Self::Where { .. } => "where",
            Self::Facts { .. } => "facts",
            Self::Contents { .. } => "contents",
            Self::Search { .. } => "search",
            Self::Index { .. } => "index",
            Self::IndexStatus { .. } => "index_status",
            Self::IndexClear { .. } => "index_clear",
            Self::Signatures { .. } => "signatures",
            Self::Edits { .. } => "edits",
            Self::PageMove { .. } => "page_move",
            Self::PageRotate { .. } => "page_rotate",
            Self::PageDelete { .. } => "page_delete",
            Self::PageInsert { .. } => "page_insert",
            Self::Undo { .. } => "undo",
            Self::Redo { .. } => "redo",
            Self::Discard { .. } => "discard",
            Self::ClearHistory { .. } => "clear_history",
            Self::SessionSleep { .. } => "session_sleep",
            Self::Save { .. } => "save",
            Self::Sign {} => "sign",
            Self::Status {} => "status",
            Self::PermissionShow {} => "permission_show",
            Self::PermissionRequest { .. } => "permission_request",
            Self::LevelChanged {} => "level_changed",
            Self::Wait { .. } => "wait",
            Self::Watch { .. } => "watch",
            Self::WatchStop {} => "watch_stop",
            Self::Log { .. } => "log",
            Self::Stop {} => "stop",
            Self::SessionShow {} => "session_show",
            Self::SessionSave {} => "session_save",
            Self::SessionRestore {} => "session_restore",
            Self::SessionCheck {} => "session_check",
            Self::UpdateStatus {} => "update_status",
            Self::UpdateCheck {} => "update_check",
            Self::UpdateInstall {} => "update_install",
            Self::Settings { .. } => "settings",
        }
    }

    /// Who may run this. From the command table, with the one case the table can't express.
    pub fn access(&self) -> Access {
        match self {
            // Measuring with the decided limits only reads numbers. Choosing other limits decides what a hostile file may use, so it isn't reading.
            Self::BudgetRender { limit_mib: Some(_), .. } | Self::BudgetRender { retire_mib: Some(_), .. } => Access::At(Level::Full),
            // Asking how things are shown changes nothing. Changing it is moving around.
            Self::View { paper: None, sidebar: None, agent_icon: None, zoom: None, layout: None } => table::by_tag(self.tag()).map_or(Access::Nobody, |spec| spec.access),
            Self::View { .. } => Access::At(Level::Control),
            Self::Compare { tabs: None, sync: None, off: false } => table::by_tag(self.tag()).map_or(Access::Nobody, |spec| spec.access),
            Self::Compare { .. } => Access::At(Level::Control),
            // Asking is reading. Whether this app replaces itself is as far-reaching as installing, so it's the same level.
            Self::Settings { auto_update: Some(_) } => Access::At(Level::Full),
            // A command with no row can't run: the table is the list.
            _ => table::by_tag(self.tag()).map_or(Access::Nobody, |spec| spec.access),
        }
    }

    /// Whether this draws or reads pages through a worker. These run at most a few at a time, because each can use hundreds of megabytes.
    pub fn draws(&self) -> bool {
        matches!(self, Self::Render { .. } | Self::ExportSvg { .. } | Self::Text { .. } | Self::Select { .. } | Self::Copy { .. } | Self::Search { .. } | Self::BudgetRender { .. } | Self::BudgetViewport { .. })
    }

    /// What goes in the activity log about this command's arguments. Numbers and cleaned file names, and never anything that could hold document text: a tab's name as typed or a search's words aren't kept.
    pub fn log_details(&self) -> Value {
        // A file's name comes from outside, so even cleaned, it goes under `untrusted`, here as everywhere.
        let file = |path: &Path| json!({ "file": file_label(path) });
        // A tab's name as typed isn't kept. `with_acted_tab` swaps in the `#id` it turned out to mean, which is what the log is for: this is only what's left if it didn't find one, such as an ambiguous name, or a command refused before it looked.
        let tab_of = |tab: &str| if tab == "active" || tab.strip_prefix('#').is_some_and(|id| id.bytes().all(|b| b.is_ascii_digit()) && id.len() <= 10) { tab.to_string() } else { "by name".to_string() };
        match self {
            Self::Open { path, page } => json!({ "page": page, "untrusted": file(path) }),
            Self::PageInsert { tab, path, at } => json!({ "tab": tab_of(tab), "at": at, "untrusted": file(path) }),
            Self::Render { tab, page, out, .. } | Self::ExportSvg { tab, page, out, .. } => json!({ "tab": tab_of(tab), "page": page, "untrusted": file(out) }),
            Self::Text { tab, page } | Self::Goto { tab, page } => json!({ "tab": tab_of(tab), "page": page }),
            // Which pages, and not where on them or what's there.
            Self::Select { tab, page, to_page, all, anyway, .. } | Self::Copy { tab, page, to_page, all, anyway, .. } => json!({ "tab": tab_of(tab), "page": page, "to_page": to_page, "all": all, "anyway": anyway }),
            Self::PageMove { tab, pages, to } => json!({ "tab": tab_of(tab), "pages": clean_range(pages), "to": to }),
            Self::PageRotate { tab, pages, degrees } => json!({ "tab": tab_of(tab), "pages": clean_range(pages), "degrees": degrees }),
            Self::PageDelete { tab, pages } => json!({ "tab": tab_of(tab), "pages": clean_range(pages) }),
            // Making generated contents starts a helper, which is heavier work than reading the file's own, so the log can tell them apart.
            Self::Contents { tab, generated: true, .. } => json!({ "tab": tab_of(tab), "generated": true }),
            Self::Tap { tab } | Self::Close { tab } | Self::Where { tab } | Self::Facts { tab } | Self::Contents { tab, .. } | Self::Search { tab, .. } | Self::Index { tab, .. } | Self::IndexStatus { tab } | Self::IndexClear { tab, .. } | Self::Signatures { tab } | Self::Edits { tab } | Self::Undo { tab } | Self::Redo { tab } | Self::Discard { tab } | Self::ClearHistory { tab } | Self::SessionSleep { tab } | Self::Save { tab } | Self::BudgetRender { tab, .. } | Self::BudgetViewport { tab, .. } => json!({ "tab": tab_of(tab) }),
            Self::PermissionRequest { level, .. } => json!({ "level": level.name() }),
            Self::QaRead { verb, .. } | Self::QaDrive { verb, .. } => json!({ "verb": recto_core::clean_line(verb) }),
            Self::Settings { auto_update } => json!({ "auto_update": auto_update.as_deref().map(recto_core::clean_line) }),
            Self::Compare { sync, off, .. } => json!({ "sync": sync.as_deref().map(recto_core::clean_line), "off": off }),
            Self::View { .. } | Self::Wait { .. } | Self::Watch { .. } | Self::WatchStop {} | Self::Log { .. } | Self::Tabs { .. } | Self::Sidebar {} | Self::Budget {} | Self::Sign {} | Self::Status {} | Self::PermissionShow {} | Self::LevelChanged {} | Self::Stop {} | Self::SessionShow {} | Self::SessionSave {} | Self::SessionRestore {} | Self::SessionCheck {} | Self::UpdateStatus {} | Self::UpdateCheck {} | Self::UpdateInstall {} => Value::Null,
        }
    }
}

impl Command {
    /// The selector this command acts on, if it has one.
    /// Whether it reads what's on a tab's pages, which a tab's edits say.
    fn reads_pages(&self) -> bool {
        matches!(self, Self::Text { .. } | Self::Render { .. } | Self::ExportSvg { .. } | Self::Select { .. } | Self::Copy { .. } | Self::Search { .. } | Self::Index { .. } | Self::BudgetRender { .. } | Self::BudgetViewport { .. })
    }

    fn tab_selector(&self) -> Option<&str> {
        match self {
            Self::Tap { tab } | Self::Goto { tab, .. } | Self::Close { tab } | Self::Render { tab, .. } | Self::ExportSvg { tab, .. } | Self::Text { tab, .. } | Self::Select { tab, .. } | Self::Copy { tab, .. } | Self::BudgetRender { tab, .. } | Self::BudgetViewport { tab, .. } | Self::Where { tab } | Self::Facts { tab } | Self::Contents { tab, .. } | Self::Search { tab, .. } | Self::Index { tab, .. } | Self::IndexStatus { tab } | Self::IndexClear { tab, .. } | Self::Signatures { tab } | Self::Edits { tab } | Self::PageMove { tab, .. } | Self::PageRotate { tab, .. } | Self::PageDelete { tab, .. } | Self::PageInsert { tab, .. } | Self::Undo { tab } | Self::Redo { tab } | Self::Discard { tab } | Self::ClearHistory { tab } | Self::SessionSleep { tab } | Self::Save { tab } | Self::Wait { tab, .. } => Some(tab),
            _ => None,
        }
    }
}

/// Which tab a command is about to act on, as an id for the log. Never a name, and none if it means no tab or several. Asked before the command runs, because `close` leaves nothing to ask afterwards.
pub fn acted_tab<L: DocLoader>(session: &Session<L>, command: &Command) -> Option<TabId> {
    select(session, command.tab_selector()?).ok()
}

/// Puts the tab a command really acted on in its log entry. `open` learns its tab from its answer.
pub fn with_acted_tab(mut details: Value, acted: Option<TabId>, result: &Result<Value, AgentError>, command: &Command) -> Value {
    let id = match (command, result) {
        (Command::Open { .. }, Ok(data)) => data["tab"].as_u64(),
        _ => acted.map(|id| u64::from(id.0)),
    };
    if let Some(id) = id {
        details["tab"] = json!(format!("#{id}"));
    }
    details
}

/// Keeps a page range in the log only if it's what a range should be.
fn clean_range(text: &str) -> &str {
    if text.len() <= 24 && text.bytes().all(|b| b.is_ascii_digit() || b == b'-') { text } else { "?" }
}

/// A failed command, with a stable code, a plain message and the next step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentError {
    pub code: &'static str,
    pub message: String,
    pub candidates: Vec<String>,
    pub exit: i32,
    /// What to do about it, in plain words.
    pub next: String,
    /// The one command that asks the user for what's missing, on a refusal.
    pub ask: Option<Box<str>>,
    /// Anything from outside this error needs to name, which only ever goes under `untrusted`.
    pub untrusted: Option<Box<Value>>,
}

impl AgentError {
    pub fn new(code: &'static str, message: impl Into<String>, exit: i32) -> Self {
        Self { code, message: message.into(), candidates: Vec::new(), exit, next: next_step(code, exit).to_string(), ask: None, untrusted: None }
    }

    /// Replaces the next step with one that knows more.
    #[must_use]
    pub fn then(mut self, next: impl Into<String>) -> Self {
        self.next = next.into();
        self
    }
}

/// The next step for an error, by its code, or else by its kind. Every error says one.
fn next_step(code: &str, exit: i32) -> &'static str {
    match code {
        "no_such_tab" => "`recto tabs` lists what's open. `recto open <file.pdf>` opens a file.",
        "no_such_page" | "page_out_of_range" => "`recto where` gives the tab's length. Pages count from 1.",
        "not_found" => "Check the path. It's read by the session, and relative paths are taken from where you ran `recto`.",
        "not_allowed" => "Ask the user to give this user account access to the file.",
        "not_pdf" => "This file can't be opened. Try another file.",
        "password_needed" | "needs_password" => "Ask the user to open it in the app and allow it. Passwords are never taken here.",
        "ambiguous" => "Use one of the candidates, written as `#id`.",
        "unsaved_changes" => "`recto discard` throws them away, then `recto close` or `recto stop` can go on. It needs Full access only if some are the user's. Saving isn't built yet.",
        "last_page" => "Keep at least one page, or delete fewer.",
        "too_many_pages" | "too_many_sources" => "Add less to this document.",
        "too_many_tabs" => "`recto close` closes a tab.",
        "renderer_only" => "This file can still be read and drawn. To change it, it first needs repairing in another tool.",
        "source_changed" => "Tell the user this file changed. `recto edits` shows what's unsaved. `recto discard` throws the edits away, which needs Full access if some are the user's, and opening the file again starts from what's there now.",
        "nothing_to_undo" | "nothing_to_redo" => "`recto edits` shows where this tab is.",
        "not_built" => "Tell the user this isn't in this version yet.",
        "too_large_at_this_scale" => "Try a smaller `--scale`. 2 is twice the page's size, and 1 is its own.",
        "page_too_large_to_read" => "Tell the user this page can't be read. The rest of the document may be fine.",
        "render_worker_missing" => "Tell the user RectoPDF's install is incomplete: `recto-render-service` must be in the same folder as `recto`. In a source tree, `cargo build --workspace` builds both, which `cargo build -p recto-cli` doesn't.",
        "render_library_missing" => "Tell the user RectoPDF's install is incomplete. In a source tree, `tools/pdfium.sh` fetches the library.",
        "bad_command" => "`recto help` lists every command.",
        "bad_path" => "Give the whole path. `recto` does this for you.",
        "output_exists" => "Choose a new file name. An existing file is never replaced.",
        "bad_output" => "Choose a file name ending in .png.",
        "bad_rect" => "Write it as `--rect 10,20,300,200`, in points from the page's top left.",
        "bad_text_mode" => "Use `--text both`, `paths` or `selectable`.",
        "needs_server" => "Start the session with `recto serve --background`, then use `recto` without `local`.",
        "not_running" => "Start it with `recto serve --background`.",
        "needs_terminal" => "Ask the user to run this command themselves, in their own terminal.",
        "sign_refused" => "Tell the user. Only they can sign, in the app.",
        "busy" => "Try again in a moment.",
        "wrong_version" => "Stop the old session with `recto stop` and start this version with `recto serve --background`.",
        _ => match exit {
            exit::NOT_FOUND => "`recto tabs` lists what's open.",
            exit::AMBIGUOUS => "Use one of the candidates.",
            exit::PERMISSION => "Ask the user for more access.",
            exit::NOT_RUNNING => "Start it with `recto serve --background`.",
            exit::TIMED_OUT => "Try again, or with a longer `--timeout`.",
            exit::PROTECTED => "Ask the user: only they can unlock this.",
            exit::BUSY => "Try again in a moment.",
            exit::USAGE => "`recto help` lists every command.",
            _ => "`recto status` shows whether the session is healthy. If this keeps happening, tell the user what you tried.",
        },
    }
}

impl From<SessionError> for AgentError {
    fn from(error: SessionError) -> Self {
        let exit = match error {
            SessionError::NoSuchTab | SessionError::Open(recto_core::OpenError::NotFound) | SessionError::Edit(recto_session::edit::EditError::OutOfRange) => exit::NOT_FOUND,
            SessionError::Open(recto_core::OpenError::PasswordNeeded) => exit::PROTECTED,
            _ => exit::FAILED,
        };
        Self::new(error.code(), error.message(), exit)
    }
}

impl From<recto_core::OpenError> for AgentError {
    fn from(error: recto_core::OpenError) -> Self {
        Self::from(SessionError::Open(error))
    }
}

/// An error as JSON, the same everywhere.
fn error_json(error: &AgentError) -> Value {
    let mut out = json!({ "code": error.code, "message": error.message, "candidates": error.candidates, "exit": error.exit, "next": error.next });
    if let Some(ask) = &error.ask {
        out["ask"] = json!(ask);
    }
    if let Some(untrusted) = &error.untrusted {
        out["untrusted"] = (**untrusted).clone();
    }
    out
}

/// Wraps a result in the envelope every response uses.
pub fn envelope(result: &Result<Value, AgentError>) -> Value {
    match result {
        Ok(data) => json!({ "v": VERSION, "ok": true, "data": data }),
        Err(error) => json!({ "v": VERSION, "ok": false, "error": error_json(error) }),
    }
}

/// The envelope with the command's name, which is what the socket and `--json` print.
pub fn reply(cmd: &str, result: &Result<Value, AgentError>) -> Value {
    let mut out = envelope(result);
    out["cmd"] = json!(cmd);
    out
}

/// Longest tab selector looked at. A name is capped well under this, so a longer one matches nothing, and can't make a wait hold 64 KiB.
pub const MAX_SELECTOR_BYTES: usize = 1024;

/// Longest line a client may send: a command and a path or two. No command needs more, so a longer one is refused before anything reads it.
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;

/// Parses one JSON command. Unknown commands and fields are refused rather than guessed at.
pub fn parse(line: &str) -> Result<Command, AgentError> {
    if line.len() > MAX_COMMAND_BYTES {
        return Err(AgentError::new("bad_command", "That command is too long.", exit::FAILED));
    }
    serde_json::from_str(line).map_err(|_| AgentError::new("bad_command", "That isn't a command RectoPDF knows.", exit::FAILED))
}

/// The refusal for a level that's too low: what's missing, and the one command that asks for it.
pub fn refusal(command_name: &str, have: Level, needed: Access) -> AgentError {
    match needed {
        Access::At(level) => {
            let mut error = AgentError::new("permission_denied", format!("`{command_name}` needs {} access, and agent access is {} now.", level.label(), have.label()), exit::PERMISSION);
            let ask = format!("recto permission request {}", level.name());
            error.next = format!("Run `{ask}`, which tells the user what to allow. Nothing else raises this.");
            error.ask = Some(ask.into());
            error
        }
        Access::UserOnly => AgentError::new("needs_terminal", format!("`{command_name}` is for the user, at their own terminal."), exit::PERMISSION),
        Access::Nobody => AgentError::new("sign_refused", "Only the user can sign a document, in the app. No agent command signs, at any level.", exit::PERMISSION),
    }
}

/// Whether a level may run a command.
pub fn allowed(have: Level, needed: Access) -> bool {
    matches!(needed, Access::At(level) if have >= level)
}

/// Something that happened in the session, for `wait` and `watch`. Holds no document text beyond a cleaned name, marked untrusted.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub name: &'static str,
    pub data: Value,
}

impl Event {
    pub fn new(name: &'static str, data: Value) -> Self {
        Self { name, data }
    }

    /// The group `watch --events` selects it by.
    pub fn group(&self) -> &'static str {
        table::event_group(self.name)
    }

    /// The line a watcher receives.
    pub fn to_json(&self) -> Value {
        let mut out = self.data.clone();
        out["event"] = json!(self.name);
        out
    }
}

/// A way to reach the session. In one process that's a plain reference; on the socket it's a lock taken only as long as each step needs.
pub trait SessionAccess {
    type Loader: DocLoader + Clone;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Self::Loader>) -> R) -> R;
    /// How documents get indexed here, if they do.
    fn indexing(&self) -> Option<&dyn index::Indexing> {
        None
    }
    /// How generated contents get made here, if they do.
    fn generating(&self) -> Option<&dyn generated::Generating> {
        None
    }
}

impl<L: DocLoader + Clone> SessionAccess for Mutex<Session<L>> {
    type Loader = L;
    fn with<R>(&self, step: impl FnOnce(&mut Session<L>) -> R) -> R {
        // A panic on another thread doesn't make the tabs wrong, and every session method leaves them whole.
        step(&mut self.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
}

struct Direct<'a, L>(RefCell<&'a mut Session<L>>, Option<&'a dyn index::Indexing>);

impl<L: DocLoader + Clone> SessionAccess for Direct<'_, L> {
    type Loader = L;
    fn with<R>(&self, step: impl FnOnce(&mut Session<L>) -> R) -> R {
        step(&mut self.0.borrow_mut())
    }
    fn indexing(&self) -> Option<&dyn index::Indexing> {
        self.1
    }
}

/// Runs one command against a session, if the level allows it.
pub fn run<L: DocLoader + Clone>(session: &mut Session<L>, level: Level, command: &Command) -> Result<Value, AgentError> {
    run_indexed(session, level, command, None)
}

/// The same, with a way to read or build indexes.
pub fn run_indexed<L: DocLoader + Clone>(session: &mut Session<L>, level: Level, command: &Command, indexing: Option<&dyn index::Indexing>) -> Result<Value, AgentError> {
    execute(&Direct(RefCell::new(session), indexing), level, command, &mut Vec::new())
}

/// Runs one command, noting what happened in `events`. Commands that only a long-running session can answer say so.
/// What's chosen, in a word: `auto` goes with the window.
pub fn paper_name(view: recto_session::ViewSettings) -> &'static str {
    view.paper.map_or("auto", recto_session::Paper::name)
}

/// What watchers are told when how pages are shown changes: the same words wherever it was changed from.
pub fn view_changed(view: recto_session::ViewSettings) -> Event {
    Event::new("view_changed", json!({ "paper": paper_name(view), "sidebar": !view.sidebar_hidden }))
}

/// QA commands reach an app started in QA mode, and nothing else. Other sessions, a shipped app's included, say this.
/// `recto stop` against the owner's own app. Only they quit it: the default, by the coordinator's decision of 2026-11-07, until the owner says otherwise.
pub fn only_the_user_quits() -> AgentError {
    AgentError::new("only_the_user_quits", "This session is the app someone is using, and only they can quit it.", exit::FAILED).then("Ask them to, if it needs to stop. Everything else works with it running.")
}

/// `session restore` would lose the owner's edits, which is theirs to say below Full: SPEC decision 20.
pub fn restore_would_lose_owners_edits() -> AgentError {
    AgentError::new("owner_edits_at_stake", "Restoring would replace this session, which holds the user's own unsaved edits, so it wasn't done.", exit::PERMISSION).then("Ask them to save or discard those first.")
}

/// What `session show` says, from the record and the session. No more of a document than `tabs` gives, and what the owner typed in Find only at Control, as it's not about any document. Called with the session held.
pub fn session_show_json<L: DocLoader>(session: &Session<L>, level: Level, kept: Value) -> Value {
    let tabs: Vec<Value> = session
        .tabs()
        .iter()
        .take(MAX_TABS_LISTED as usize)
        .map(|tab| {
            let notice = session.tab_notice(tab.id).map(|notice| json!({ "code": notice.code(), "message": session.tab_notice_message(tab.id) }));
            let place = session.place_to_show(tab.id);
            json!({ "tab": tab.id.0, "page": tab.page + 1, "pages": session.pages_in(tab.id), "zoom": place.map(|place| place.zoom), "within_page": place.filter(|place| place.anchor_page == place.page).map(|place| place.within), "layout": tab.spread.name(), "edited": session.is_edited(tab.id), "can_undo": session.undo_and_redo(tab.id).0, "can_redo": session.undo_and_redo(tab.id).1, "notice": notice })
        })
        .collect();
    let find = session.find();
    let mut find_json = json!({ "open": find.open, "has_text": !find.query.is_empty() || !find.draft.is_empty() });
    if level >= Level::Control {
        find_json["query"] = json!(find.query);
        find_json["draft"] = json!(find.draft);
    }
    json!({ "record": kept, "count": session.tabs().len(), "active": session.active().map(|id| id.0), "tabs": tabs, "find": find_json, "edit_mode": session.edit_mode(), "paper": paper_name(session.view()), "sidebar": !session.view().sidebar_hidden })
}

/// Updates and settings are the app's. A session with no window has nothing to replace, and nothing to set.
pub fn update_needs_app() -> AgentError {
    AgentError::new("needs_app", "Updates and settings belong to the app, and this session has no window.", exit::FAILED).then("Open RectoPDF, and `recto` will talk to that.")
}

pub fn qa_needs_app() -> AgentError {
    AgentError::new("qa_needs_app", "QA commands only work on an app started with `recto qa start`.", exit::FAILED).then("Set RECTO_STATE_DIR to a temp folder and run `recto qa start`.")
}

pub fn execute<A: SessionAccess>(access: &A, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let needed = command.access();
    if !allowed(level, needed) {
        return Err(refusal(table::display_name(command.tag()), level, needed));
    }
    // A command about a tab is someone working on it, so it doesn't go dormant from under them, and what it reads is there to read. Every route's through here.
    let held = command.tab_selector().and_then(|selector| {
        access.with(|s| {
            let id = select(s, selector).ok()?;
            let awake = s.wake_list(id);
            s.touch(id);
            s.hold(id);
            Some((id, awake))
        })
    });
    // Its edits say what's on its pages, and they can't be read just now: anything that reads a page says so, rather than that there's no such page (review).
    let result = match held {
        Some((_, false)) if command.reads_pages() => Err(AgentError::new(recto_session::SessionError::EditsUnreadable.code(), recto_session::SessionError::EditsUnreadable.message(), exit::FAILED).then("`recto discard` gives up on them, and `recto session show` says what's known of them.")),
        _ => execute_held(access, level, command, events),
    };
    let held = held.map(|(id, _)| id);
    if let Some(id) = held {
        access.with(|s| s.let_go(id));
    }
    result
}

fn execute_held<A: SessionAccess>(access: &A, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    match command {
        Command::Open { path, page } => open(access, path, *page, events),
        Command::Tabs { from, limit } => access.with(|s| tabs_json(s, *from, *limit)),
        Command::Sidebar {} => Ok(access.with(|s| sidebar_json(s))),
        Command::Tap { tab } => {
            let id = access.with(|s| select(s, tab))?;
            read_ahead(access, |s| s.wake_for_tap(id));
            access.with(|s| {
            let id = select(s, tab)?;
            let was_active = s.active();
            s.tap(id)?;
            if was_active != Some(id) {
                events.push(Event::new("tab_activated", json!({ "tab": id.0 })));
            }
            Ok(sidebar_json(s))
            })
        }
        Command::Goto { tab, page } => access.with(|s| {
            let id = select(s, tab)?;
            go_to(s, id, *page)?;
            events.push(Event::new("page_changed", json!({ "tab": id.0, "page": page })));
            Ok(where_json(s, id))
        }),
        Command::Close { tab } => {
            // What would refuse it comes first: no reading a file for a close that won't happen.
            let id = access.with(|s| {
                let id = select(s, tab)?;
                refuse_close_that_loses_something(s, id, level)?;
                Ok::<_, AgentError>(id)
            })?;
            read_ahead(access, |s| s.wake_for_close(id));
            access.with(|s| {
            let id = select(s, tab)?;
            refuse_close_that_loses_something(s, id, level)?;
            let before = s.active();
            // At Full, what's lost is the holder's call, and `refuse_close_that_loses_something` has let it through. The session itself won't lose the owner's undone steps unasked.
            if level >= Level::Full && s.has_undone_owner_steps(id) {
                s.discard_edits(id);
            }
            s.close(id)?;
            events.push(Event::new("tab_closed", json!({ "tab": id.0 })));
            if s.active() != before
                && let Some(active) = s.active()
            {
                events.push(Event::new("tab_activated", json!({ "tab": active.0 })));
            }
            // Not the whole list: closing a thousand tabs shouldn't build a thousand lists.
            Ok(json!({ "closed": id.0, "count": s.tabs().len(), "active": s.active().map(|active| where_json(s, active)) }))
            })
        }
        Command::View { zoom: Some(_), .. } => Err(AgentError::new("zoom_needs_app", "Zoom is about what's on a screen, and this session has none.", exit::FAILED).then("It works when `recto` is talking to the app.")),
        Command::View { paper, sidebar, agent_icon, zoom: None, layout } => access.with(|s| {
            let was = s.view();
            if let Some(name) = layout.as_deref() {
                let spread = recto_session::record::Spread::from_name(name).ok_or_else(|| AgentError::new("usage", format!("`{}` isn't a layout. It's one of {}.", recto_core::clean_line(name), recto_session::record::Spread::ALL.map(recto_session::record::Spread::name).join(", ")), exit::USAGE))?;
                let id = s.active().ok_or_else(|| AgentError::new("no_active_tab", "There's no tab showing to lay out.", exit::NOT_FOUND).then("`recto open` or `recto tap` one first."))?;
                if s.set_spread(id, spread) {
                    events.push(Event::new("view_changed", json!({ "tab": id.0, "layout": spread.name() })));
                }
            }
            let mut view = was;
            match paper.as_deref() {
                Some("auto") => view.paper = None,
                Some(name) => view.paper = Some(recto_session::Paper::from_name(name).ok_or_else(|| AgentError::new("usage", format!("`{}` isn't a paper. It's one of {}, or `auto`.", recto_core::clean_line(name), recto_session::Paper::ALL.map(recto_session::Paper::name).join(", ")), exit::USAGE))?),
                None => {}
            }
            match sidebar.as_deref() {
                Some("on") => view.sidebar_hidden = false,
                Some("off") => view.sidebar_hidden = true,
                Some(other) => return Err(AgentError::new("usage", format!("`{}` isn't for the sidebar. It's `on` or `off`.", recto_core::clean_line(other)), exit::USAGE)),
                None => {}
            }
            match agent_icon.as_deref() {
                Some("on") => view.agent_icon_hidden = false,
                Some("off") => view.agent_icon_hidden = true,
                Some(other) => return Err(AgentError::new("usage", format!("`{}` isn't for the agent icon. It's `on` or `off`.", recto_core::clean_line(other)), exit::USAGE)),
                None => {}
            }
            s.set_view(view);
            if view != was {
                events.push(view_changed(view));
            }
            Ok(json!({ "paper": paper_name(view), "papers": recto_session::Paper::ALL.map(recto_session::Paper::name), "sidebar": !view.sidebar_hidden, "agent_icon": !view.agent_icon_hidden, "layout": s.active().and_then(|id| s.spread_of(id)).map(recto_session::record::Spread::name) }))
        }),
        Command::Compare { tabs, sync, off } => compare(access, tabs.as_deref(), sync.as_deref(), *off, events),
        Command::QaRead { .. } | Command::QaDrive { .. } => Err(qa_needs_app()),
        Command::UpdateStatus {} | Command::UpdateCheck {} | Command::UpdateInstall {} | Command::Settings { .. } => Err(update_needs_app()),
        Command::Budget {} => Ok(json!({
            "tabs": access.with(|s| s.tabs().len()),
            "footprint_bytes": recto_platform::footprint_bytes(),
            "open_handles": recto_platform::open_handle_count(),
        })),
        Command::Where { tab } => access.with(|s| Ok(where_json(s, select(s, tab)?))),
        Command::Facts { tab } => reading::facts(access, tab),
        Command::Contents { tab, from, limit, depth, generated } => reading::contents(access, tab, *from, *limit, *depth, *generated),
        Command::Signatures { tab } => reading::signatures(access, tab),
        Command::Search { tab, query, pages, limit, scan, regex, rank, context } => search::search(access, &search::Asked { tab, query, pages: pages.as_deref(), limit: *limit, scan: *scan, regex: *regex, rank: *rank, context: *context }),
        Command::Index { tab, agent } => index::index(access, tab, true, *agent),
        Command::IndexStatus { tab } => index::index(access, tab, false, false),
        Command::IndexClear { tab, all } => index::clear(access, tab, *all),
        Command::Edits { tab } => access.with(|s| {
            let id = select(s, tab)?;
            Ok(edits_json(s, id))
        }),
        Command::Render { tab, page, scale, out } => {
            require_absolute(out)?;
            let (id, position, source) = locate(access, tab, *page)?;
            let drawn = render::render(&source.path, source.page + 1, *scale, out);
            if !is_about_the_output(drawn.as_ref().err().map(|error| error.code)) {
                note_draw(access, id, position, drawn.as_ref().err().map(|error| error.code), events);
            }
            let mut data = drawn?;
            describe_source(&mut data, *page, &source);
            Ok(data)
        }
        Command::ExportSvg { tab, page, out, rect, text, scale, strip_metadata } => {
            require_absolute(out)?;
            let (id, position, source) = locate(access, tab, *page)?;
            let args = render::SvgArgs { path: &source.path, page: source.page + 1, out, rect: *rect, text: text.as_deref(), scale: *scale, strip_metadata: *strip_metadata };
            let drawn = render::export_svg(&args);
            if !is_about_the_output(drawn.as_ref().err().map(|error| error.code)) {
                note_draw(access, id, position, drawn.as_ref().err().map(|error| error.code), events);
            }
            let mut data = drawn?;
            describe_source(&mut data, *page, &source);
            Ok(data)
        }
        Command::Text { tab, page } => {
            let (id, position, source) = locate(access, tab, *page)?;
            let read = render::text(&source.path, source.page + 1);
            let drew = read.as_ref().is_ok_and(|data| data["hidden_text"]["checked"] == true);
            if drew || read.is_err() {
                note_draw(access, id, position, read.as_ref().err().map(|error| error.code), events);
            }
            let mut data = read?;
            describe_source(&mut data, *page, &source);
            Ok(data)
        }
        Command::Select { tab, page, to_page, chars, at, to, unit, all, anyway } => selecting::run(access, &selecting::Pick { tab, page: *page, to_page: *to_page, chars: chars.as_deref(), at: at.as_deref(), to: to.as_deref(), unit: unit.as_deref(), all: *all, anyway: *anyway }, true),
        Command::Copy { tab, page, to_page, chars, at, to, unit, all, anyway } => selecting::run(access, &selecting::Pick { tab, page: *page, to_page: *to_page, chars: chars.as_deref(), at: at.as_deref(), to: to.as_deref(), unit: unit.as_deref(), all: *all, anyway: *anyway }, false),
        Command::BudgetViewport { tab, viewport, first, scale, strategy } => {
            let path = access.with(|s| tab_path(s, select(s, tab)?))?;
            render::budget_viewport(&path, viewport, *first, *scale, strategy.as_deref())
        }
        Command::BudgetRender { tab, pages, first, scale, fresh, tiles, samples, limit_mib, retire_mib } => {
            let path = access.with(|s| tab_path(s, select(s, tab)?))?;
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
            render::budget(&path, &args)
        }
        Command::PageMove { tab, pages, to } => {
            let (start, len) = parse_pages(pages, false)?;
            let destination = one_based(*to)?;
            let did = if start == destination { format!("{} already at {to}, so nothing changed.", which_pages(pages, "is", "are")) } else { format!("Moved {} to {to}.", which_pages(pages, "", "")) };
            edit(access, level, tab, events, did, |e| e.move_pages(start, len, destination))
        }
        Command::PageRotate { tab, pages, degrees } => {
            let turns = quarter_turns(*degrees)?;
            // `all` turns every page as one step, as the panel's Rotate does outside edit mode.
            if pages == "all" {
                return edit(access, level, tab, events, format!("Rotated every page by {degrees} degrees."), |e| e.rotate(0, e.len(), turns));
            }
            let (start, len) = parse_pages(pages, false)?;
            edit(access, level, tab, events, format!("Rotated {} by {degrees} degrees.", which_pages(pages, "", "")), |e| e.rotate(start, len, turns))
        }
        Command::PageDelete { tab, pages } => {
            let (start, len) = parse_pages(pages, false)?;
            edit(access, level, tab, events, format!("Deleted {}.", which_pages(pages, "", "")), |e| e.delete(start, len))
        }
        Command::PageInsert { tab, path, at } => {
            require_absolute(path)?;
            let position = one_based(*at)?;
            // The file is read first and outside the session, so a slow one holds nobody else up.
            let loader = access.with(|s| s.loader().clone());
            let stamp = recto_platform::file_stamp(path).ok();
            let facts = loader.load(path)?;
            access.with(|s| {
                let id = select(s, tab)?;
                keep_owners_undone_steps(s, id, level)?;
                s.set_agent_at_full(level >= Level::Full);
                s.insert_pdf_with_facts(id, Author::Agent, position, path, &facts, stamp)?;
                events.push(edits_event(s, id));
                let mut data = edit_state_json(s, id);
                data["did"] = json!(format!("Added {} at page {at}.", if facts.page_count == 1 { "1 page".to_string() } else { format!("{} pages", facts.page_count) }));
                Ok(with_place_note(s, id, data))
            })
        }
        Command::Undo { tab } => access.with(|s| {
            let id = select(s, tab)?;
            s.set_agent_at_full(level >= Level::Full);
            if !s.undo_edit_as(id, Author::Agent)? {
                return Err(AgentError::new("nothing_to_undo", "There's nothing to undo on this tab.", exit::FAILED));
            }
            events.push(edits_event(s, id));
            let mut data = edit_state_json(s, id);
            data["did"] = json!("Undid the last edit.");
            Ok(with_place_note(s, id, data))
        }),
        Command::Redo { tab } => access.with(|s| {
            let id = select(s, tab)?;
            s.set_agent_at_full(level >= Level::Full);
            if !s.redo_edit_as(id, Author::Agent)? {
                return Err(AgentError::new("nothing_to_redo", "There's nothing to redo on this tab.", exit::FAILED));
            }
            events.push(edits_event(s, id));
            let mut data = edit_state_json(s, id);
            data["did"] = json!("Redid the last undone edit.");
            Ok(with_place_note(s, id, data))
        }),
        Command::Discard { tab } => access.with(|s| {
            let id = select(s, tab)?;
            refuse_discard_of_owners_edits(s, id, level)?;
            let had_any = s.page_edits(id).is_some();
            s.discard_edits(id);
            events.push(edits_event(s, id));
            let mut data = edit_state_json(s, id);
            data["did"] = json!(if had_any { "Threw away this tab's edits." } else { "There were no edits to throw away." });
            Ok(with_place_note(s, id, data))
        }),
        Command::ClearHistory { tab } => access.with(|s| {
            let id = select(s, tab)?;
            refuse_discard_of_owners_edits(s, id, level)?;
            let had_any = s.page_edits(id).is_some_and(|list| list.can_undo() || list.can_redo());
            s.clear_history(id, recto_session::ClearReason::UserAsked);
            events.push(edits_event(s, id));
            let mut data = edit_state_json(s, id);
            data["did"] = json!(if had_any { "Cleared this tab's edit history. Unsaved edits are still here, and can't be undone now." } else { "There was no edit history to clear." });
            Ok(with_place_note(s, id, data))
        }),
        Command::Save { .. } => Err(AgentError::new(
            "not_built",
            "Saving isn't built yet, so nothing was written. Page edits stay in this session only, the file on disk is unchanged, and they're lost when the session ends.",
            exit::FAILED,
        )),
        Command::Sign {} => Err(refusal("sign", level, Access::Nobody)),
        Command::SessionSleep { .. } | Command::Status {} | Command::PermissionShow {} | Command::PermissionRequest { .. } | Command::LevelChanged {} | Command::Wait { .. } | Command::Watch { .. } | Command::WatchStop {} | Command::Log { .. } | Command::Stop {} | Command::SessionShow {} | Command::SessionSave {} | Command::SessionRestore {} | Command::SessionCheck {} => {
            Err(AgentError::new("needs_server", "This needs the long-running session.", exit::NOT_RUNNING))
        }
    }
}

/// What a page's draw teaches the session and anyone waiting.
fn note_draw<A: SessionAccess>(access: &A, tab: TabId, position: u32, failure: Option<&'static str>, events: &mut Vec<Event>) {
    access.with(|s| s.note_drawn(DrawNote { tab, page: position, failure }));
    let page = position + 1;
    events.push(match failure {
        None => Event::new("page_drawn", json!({ "tab": tab.0, "page": page })),
        Some(code) => Event::new("page_failed", json!({ "tab": tab.0, "page": page, "code": code })),
    });
}

/// A refused file name says nothing about whether a page can be drawn, so a wait on that page mustn't hear of it.
fn is_about_the_output(failure: Option<&str>) -> bool {
    matches!(failure, Some("output_exists" | "bad_output" | "bad_scale" | "bad_rect" | "bad_text_mode" | "write_failed" | "output_too_large"))
}

/// Says when a page isn't the file's page of that number, when it comes from another PDF altogether, and that a drawing doesn't show an unsaved turn.
fn describe_source(data: &mut Value, page: u32, source: &PageSource) {
    data["page"] = json!(page);
    if source.page + 1 != page || source.added {
        data["source_page"] = json!(source.page + 1);
    }
    if source.added {
        data["from_added_file"] = json!(true);
        data["untrusted"]["added_file"] = json!(file_label(&source.path));
    }
    if source.quarter_turns != 0 {
        data["unsaved_rotation_degrees"] = json!(u32::from(source.quarter_turns) * 90);
    }
}

/// A file's name, made safe to pass on. It's still only what the file is called.
fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(String::new, |name| recto_core::clean_line(&name.to_string_lossy()))
}

/// The refusal when a file has changed under the page numbers that were recorded from it.
fn source_changed(path: &Path) -> AgentError {
    let mut error = AgentError::new("source_changed", "The file this page comes from has changed or gone since its page numbers were recorded, so the page can't be trusted to be the same one.", exit::FAILED);
    error.untrusted = Some(Box::new(json!({ "file": file_label(path) })));
    error
}

/// Finds a tab's page as things stand now, edits included. Refuses if its file isn't the one the page's number is about.
fn locate<A: SessionAccess>(access: &A, tab: &str, page: u32) -> Result<(TabId, u32, PageSource), AgentError> {
    let (id, position, source) = access.with(|s| {
        let id = select(s, tab)?;
        let position = page.checked_sub(1).ok_or_else(no_such_page)?;
        let source = s.page_source(id, position).ok_or_else(no_such_page)?;
        Ok::<_, AgentError>((id, position, source))
    })?;
    if !source.is_current() {
        return Err(source_changed(&source.path));
    }
    Ok((id, position, source))
}

fn no_such_page() -> AgentError {
    AgentError::new("no_such_page", "That page isn't in this document.", exit::NOT_FOUND)
}

fn require_absolute(path: &Path) -> Result<(), AgentError> {
    if path.is_absolute() { Ok(()) } else { Err(AgentError::new("bad_path", "That path isn't whole. A session has no folder of its own to start from.", exit::FAILED)) }
}

/// Reads a cold tab's file, if what's about to happen would wake one, with the session free. A file on a dead volume holds up this command and no other.
/// The most selectors `compare --tabs` reads, so a long list can't be made to look up thousands of names. More than there can be panes, so too many is said in words.
const MAX_COMPARE_SELECTORS: usize = 8;

/// `recto compare`: what the sidebar's ⌘-click and the right-click menu's scroll items do, for an agent. Owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view, right click menu can enable separate scroll (default) and synced scroll. Zoom is always synced."
fn compare<A: SessionAccess>(access: &A, tabs: Option<&str>, sync: Option<&str>, off: bool, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let usage = |words: String| AgentError::new("usage", words, exit::USAGE);
    let refused = |error: recto_session::compare::CompareError| AgentError::new(error.code(), error.message(), exit::FAILED);
    if off && (tabs.is_some() || sync.is_some()) {
        return Err(usage("`--off` goes alone: it ends the comparison.".into()));
    }
    let sync = match sync {
        None => None,
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(other) => return Err(usage(format!("`{}` isn't for `--sync`. It's `on` or `off`.", recto_core::clean_line(other)))),
    };
    let before = access.with(|s| compare_json(s));
    if off {
        access.with(|s| s.end_compare());
    }
    if let Some(list) = tabs {
        let selectors: Vec<&str> = list.split(',').map(str::trim).filter(|word| !word.is_empty()).take(MAX_COMPARE_SELECTORS + 1).collect();
        if selectors.len() > MAX_COMPARE_SELECTORS {
            return Err(refused(recto_session::compare::CompareError::TooMany));
        }
        let ids = access.with(|s| selectors.iter().map(|selector| select(s, selector)).collect::<Result<Vec<TabId>, AgentError>>())?;
        recto_session::compare::Comparison::check(&ids).map_err(refused)?;
        // The pane being read becomes the active tab, and a cold one's read here with the session free, as a tap's is.
        let reading = access.with(|s| s.active().filter(|active| ids.contains(active)).unwrap_or(ids[0]));
        read_ahead(access, |s| s.wake_for_tap(reading));
        access.with(|s| s.compare(&ids, Some(reading))).map_err(refused)?;
    }
    if let Some(on) = sync {
        access.with(|s| s.set_compare_synced(on)).then_some(()).ok_or_else(|| AgentError::new("not_comparing", "There's no comparison to sync.", exit::NOT_FOUND).then("`recto compare --tabs \"#1,#2\"` puts two side by side first."))?;
    }
    let after = access.with(|s| compare_json(s));
    if after != before {
        events.push(Event::new("view_changed", json!({ "comparison": after["panes"].as_array().map(|panes| panes.iter().map(|pane| pane["tab"].clone()).collect::<Vec<_>>()), "sync": after["sync"] })));
    }
    Ok(after)
}

/// What's side by side: each pane's tab, its page and length, the one being read, synced or separate, and what's said about lengths.
pub fn compare_json<L: DocLoader>(session: &Session<L>) -> Value {
    let panes: Vec<Value> = session
        .comparison()
        .map(|comparison| comparison.tabs.iter().filter_map(|id| session.tab(*id).map(|tab| json!({ "tab": id.0, "page": tab.page + 1, "pages": session.pages_in(*id), "reading": session.active() == Some(*id), "untrusted": { "name": &*tab.name } }))).collect())
        .unwrap_or_default();
    let comparison = session.comparison();
    json!({
        "comparing": comparison.is_some(),
        "panes": panes,
        "sync": comparison.map_or("off", |comparison| if comparison.synced { "on" } else { "off" }),
        "sync_from": comparison.filter(|comparison| comparison.synced).map(|comparison| comparison.sync_from.iter().map(|page| page + 1).collect::<Vec<_>>()),
        "note": session.compare_note(),
        "max_panes": recto_session::compare::MAX_PANES,
    })
}

fn read_ahead<A: SessionAccess>(access: &A, plan: impl FnOnce(&mut Session<A::Loader>) -> Option<(TabId, PathBuf)>) {
    let Some((id, path)) = access.with(|s| plan(s)) else { return };
    let loader = access.with(|s| s.loader().clone());
    let stamp = recto_platform::file_stamp(&path).ok();
    let loaded = loader.load(&path);
    access.with(|s| s.give_woken(id, loaded, stamp));
}

fn open<A: SessionAccess>(access: &A, path: &Path, page: Option<u32>, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    require_absolute(path)?;
    let wrong_page = |pages: u32| AgentError::new("no_such_page", format!("Nothing was opened: it has {pages} pages, and page {} isn't one of them.", page.unwrap_or(0)), exit::NOT_FOUND).then("Open it with a page it has, or with no `--page`.");
    let fits = |pages: u32| page.is_none_or(|page| page >= 1 && page <= pages);
    // Looked at first, so a change during a read shows as a change, and what's the same file can be told. A slow file holds up nobody: the session isn't held.
    let stamp = recto_platform::file_stamp(path).ok();
    // A file already open, in a tab gone cold, is read with the session free too.
    read_ahead(access, |s| s.tab_for_file(path, stamp).and_then(|id| s.wake_for_tap(id)));
    // Already open, by this name or another: SPEC decision 26. Check the page before anything moves.
    let existing = access.with(|s| {
        let id = s.tab_for_file(path, stamp)?;
        let pages = s.pages_in(id);
        if !fits(pages) {
            return Some(Err(wrong_page(pages)));
        }
        s.go_to_file(path, stamp);
        Some(Ok(id))
    });
    let (id, is_new) = match existing {
        Some(found) => (found?, false),
        None => {
            // The file is read outside the session, so a slow or hostile one holds nobody else up.
            let loader = access.with(|s| s.loader().clone());
            let facts = loader.load(path)?;
            // No tab for a page that isn't there: failing here leaves nothing to explain.
            if !fits(facts.page_count) {
                return Err(wrong_page(facts.page_count));
            }
            let before = access.with(|s| s.tabs().len());
            // A closed PDF's history comes back on this route as on any, read with the session free too.
            let source = access.with(|s| if s.tab_for_file(path, stamp).is_some() { None } else { s.history_source() });
            let history = source.and_then(|source| source.history_of(stamp, path));
            let id = access.with(|s| s.open_with_facts(path, facts, stamp, history))?;
            (id, access.with(|s| s.tabs().len()) > before)
        }
    };
    access.with(|s| {
        if let Some(page) = page {
            go_to(s, id, page)?;
        }
        let name = s.tab(id).map(|t| t.name.to_string());
        events.push(Event::new(if is_new { "tab_opened" } else { "tab_activated" }, json!({ "tab": id.0, "untrusted": { "name": name } })));
        if page.is_some() {
            events.push(Event::new("page_changed", json!({ "tab": id.0, "page": page })));
        }
        let mut data = where_json(s, id);
        // A PDF is open once, so what's asked for by any name is this tab, and an agent is told.
        if !is_new {
            data["already_open"] = json!(true);
            data["open_note"] = json!("That PDF was already open, so this is its tab. A link to a file is the same file.");
        }
        Ok(data)
    })
}

/// Goes to a one-based page. A page that isn't there is refused here rather than clamped, so an agent can't believe it's somewhere it isn't.
fn go_to<L: DocLoader>(session: &mut Session<L>, id: TabId, page: u32) -> Result<(), AgentError> {
    let position = page.checked_sub(1).filter(|position| *position < session.pages_in(id)).ok_or_else(no_such_page)?;
    session.go_to(id, position)?;
    Ok(())
}

fn one_based(page: u32) -> Result<u32, AgentError> {
    page.checked_sub(1).ok_or_else(|| AgentError::new("no_such_page", "Pages are numbered from 1.", exit::NOT_FOUND))
}

/// How many a command should give: what was asked, up to the most it gives, or its default. Asking for none is refused: it can't be what was meant, and answering with one would be a guess.
pub(crate) fn positive_limit(limit: Option<u32>, default: u32, max: u32) -> Result<usize, AgentError> {
    match limit {
        Some(0) => Err(AgentError::new("bad_limit", "A limit has to be at least 1.", exit::FAILED).then("Leave it out to get the default, or ask for 1 or more.")),
        Some(limit) => Ok(limit.min(max) as usize),
        None => Ok(default.min(max) as usize),
    }
}

/// Parses `4` or `4-6` into a zero-based start and a length. `4-` means to the end only where `open_ended` says that makes sense, and only that exact form: `4-x-` isn't it.
pub(crate) fn parse_pages(pages: &str, open_ended: bool) -> Result<(u32, u32), AgentError> {
    let bad = || AgentError::new("bad_pages", "Pages are one number or a range, such as 4 or 4-6.", exit::FAILED).then("Write them as 4 or 4-6, counting from 1.");
    let (first, last) = pages.split_once('-').unwrap_or((pages, pages));
    let number = |text: &str| if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) { Err(bad()) } else { text.parse::<u32>().map_err(|_| bad()) };
    let first = number(first)?;
    // `5-` means to the end. The length is then as long as can be, and whoever knows the document's length cuts it down.
    let last = if open_ended && last.is_empty() { u32::MAX } else { number(last)? };
    if first == 0 || last < first {
        return Err(bad());
    }
    Ok((first - 1, last - first + 1))
}

fn quarter_turns(degrees: i32) -> Result<i32, AgentError> {
    if degrees % 90 != 0 || degrees % 360 == 0 {
        return Err(AgentError::new("bad_rotation", "A rotation is a quarter turn or more, such as 90, 180, 270 or -90 degrees.", exit::FAILED).then("Use 90, 180, 270 or -90."));
    }
    Ok(degrees / 90)
}

/// `page 3` or `pages 4-6`, as it was asked for, which `parse_pages` has already held to digits, with a verb that agrees if one's wanted.
fn which_pages(pages: &str, one: &str, many: &str) -> String {
    let (noun, verb) = if pages.contains('-') { ("Pages", many) } else { ("Page", one) };
    let noun = if verb.is_empty() { noun.to_lowercase() } else { noun.to_string() };
    if verb.is_empty() { format!("{noun} {pages}") } else { format!("{noun} {pages} {verb}") }
}

fn edit<A: SessionAccess>(access: &A, level: Level, tab: &str, events: &mut Vec<Event>, did: String, change: impl FnOnce(&mut recto_session::edit::PageEdits) -> Result<(), recto_session::edit::EditError>) -> Result<Value, AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        keep_owners_undone_steps(s, id, level)?;
        s.set_agent_at_full(level >= Level::Full);
        s.edit_pages_as(id, Author::Agent, change)?;
        events.push(edits_event(s, id));
        let mut data = edit_state_json(s, id);
        data["did"] = json!(did);
        Ok(with_place_note(s, id, data))
    })
}

/// What asking for Full is for, where only that would do. The one command an agent can run to get there.
const ASK_FULL: &str = "recto permission request full";

/// Undone steps can still be got back, and a new edit clears them for good, which is how an agent could erase the user's edits: undo them all, make one edit. So below Full, an edit that would lose an undone step of the user's is refused. Undone steps of agents' own are theirs to lose. SPEC decision log, 2026-11-07: below Full, an agent can throw away only agents' edits, never the owner's.
fn keep_owners_undone_steps<L: DocLoader>(session: &Session<L>, id: TabId, level: Level) -> Result<(), AgentError> {
    if level >= Level::Full || !session.page_edits(id).is_some_and(recto_session::edit::PageEdits::owner_steps_undone) {
        return Ok(());
    }
    let mut error = AgentError::new("would_clear_redo", "A new edit here would throw away the user's edits, undone but still there to redo, and that needs Full access.", exit::PERMISSION);
    error.ask = Some(ASK_FULL.into());
    error.next = format!("Run `recto redo` until there's nothing left to redo, which keeps everything, and then go on. Or run `{ASK_FULL}` to ask for Full access.");
    Err(error)
}

/// `discard` throws away everything on the tab's list, undone or not, and whatever fell off its capped undo list too, since that's still in the page order. Below Full, none of that may be the user's.
fn refuse_discard_of_owners_edits<L: DocLoader>(session: &Session<L>, id: TabId, level: Level) -> Result<(), AgentError> {
    if level >= Level::Full || !session.page_edits(id).is_some_and(recto_session::edit::PageEdits::holds_owner_edits) {
        return Ok(());
    }
    let mut error = AgentError::new("would_discard_user_edits", "Nothing was discarded: some of this tab's edits are the user's, and throwing those away needs Full access.", exit::PERMISSION);
    error.ask = Some(ASK_FULL.into());
    error.next = format!("`recto undo` steps back through them, and keeps them. Run `{ASK_FULL}` to ask for Full access, or tell the user.");
    Err(error)
}

/// Stops a close that would lose edits, and says all of what it would lose and every way on that's real. Unsaved edits stop a close at every level: `discard` first, then `close`, so closing never loses them silently. Undone steps stop it below Full only if they're the user's, as they stop an edit.
fn refuse_close_that_loses_something<L: DocLoader>(session: &Session<L>, id: TabId, level: Level) -> Result<(), AgentError> {
    let edits = session.page_edits(id);
    let (unsaved, undone) = (session.is_edited(id), edits.is_some_and(recto_session::edit::PageEdits::can_redo));
    let protected = level < Level::Full;
    let owners_undone = protected && edits.is_some_and(recto_session::edit::PageEdits::owner_steps_undone);
    let owners_anywhere = protected && edits.is_some_and(recto_session::edit::PageEdits::holds_owner_edits);
    if !unsaved && !owners_undone {
        return Ok(());
    }
    let what = match (unsaved, undone) {
        (true, true) => "unsaved edits and undone steps",
        (true, false) => "unsaved edits",
        _ => "undone steps",
    };
    let whose = if owners_anywhere { " Some of them are the user's." } else { "" };
    let mut ways = vec!["Leave the tab open: nothing is lost until the session ends.".to_string()];
    if undone {
        ways.push("`recto redo` puts the undone steps back, and that includes anything you undid on purpose.".to_string());
    }
    if unsaved {
        if edits.is_some_and(recto_session::edit::PageEdits::history_dropped) {
            ways.push("`recto undo` can't take all of it off, because some of it is past the undo limit.".to_string());
        } else {
            ways.push("`recto undo` until there's nothing left to undo takes the unsaved edits off, and keeps them as undone steps.".to_string());
        }
    }
    let discard = if undone { "`recto discard` throws all of it away for good, and then the tab can close" } else { "`recto discard` throws them away for good, and then the tab can close" };
    if owners_anywhere {
        ways.push(format!("{discard}, which needs Full access, because it would lose the user's edits: `{ASK_FULL}` asks for it."));
    } else {
        ways.push(format!("{discard}."));
    }
    ways.push("Saving isn't built yet, so they can't be kept on disk.".to_string());
    let message = format!("This tab wasn't closed: closing it would lose its {what}.{whose}");
    // The user's undone steps are what the level is there to protect. Otherwise it's the edits themselves that stop it, at any level, and `discard` is the way on.
    let mut error = if owners_undone { AgentError::new("would_clear_redo", message, exit::PERMISSION) } else { AgentError::new("unsaved_changes", message, exit::FAILED) };
    if owners_anywhere {
        error.ask = Some(ASK_FULL.into());
    }
    error.next = ways.join(" ");
    Err(error)
}

/// Says when the page being read went away, so the tab is on another one.
fn with_place_note<L: DocLoader>(session: &mut Session<L>, id: TabId, mut data: Value) -> Value {
    if session.take_place_lost()
        && let Some(tab) = session.tab(id)
    {
        data["page_was_removed"] = json!(true);
        data["place_note"] = json!(format!("The page this tab was on was removed, so it's now on page {}.", tab.page + 1));
    }
    data
}

/// What watchers are told when a tab's edits change, wherever they were changed from: an agent, the panel or the Edit menu.
pub fn edits_changed<L: DocLoader>(session: &Session<L>, id: TabId) -> Event {
    edits_event(session, id)
}

fn edits_event<L: DocLoader>(session: &Session<L>, id: TabId) -> Event {
    Event::new("edits_changed", json!({ "tab": id.0, "pages": session.pages_in(id), "edited": session.is_edited(id) }))
}

/// The plain truth about unsaved edits, wherever a tab reports them.
pub const UNSAVED_NOTE: &str = "These edits are in this session only. Saving isn't built yet, so the file on disk is unchanged and they're lost when the session ends.";

/// What's true when an agent has undone the user's edits: the page matches the file, and they're not gone yet.
pub const USER_EDITS_UNDONE_NOTE: &str = "The user's edits on this tab are undone, not gone: `recto redo` brings them back, and closing would lose them. Nothing has been saved.";

fn edit_state_json<L: DocLoader>(session: &Session<L>, id: TabId) -> Value {
    let edited = session.is_edited(id);
    let user_edits_undone = session.has_undone_owner_steps(id);
    json!({
        "tab": id.0,
        "pages": session.pages_in(id),
        "file_pages": session.tab(id).map(|t| t.page_count),
        "page": session.tab(id).map(|t| t.page + 1),
        "edited": edited,
        // The user's undone edits aren't in the file, and closing would lose them, so nothing about this says "saved".
        "saved": !edited && !user_edits_undone,
        "user_edits_undone": user_edits_undone,
        "can_undo": session.undo_and_redo(id).0,
        "can_redo": session.undo_and_redo(id).1,
        "note": if edited { Some(UNSAVED_NOTE) } else { user_edits_undone.then_some(USER_EDITS_UNDONE_NOTE) },
    })
}

/// Most runs `edits` lists, so a hostile edit history can't make an enormous answer.
const MAX_EDIT_RUNS: usize = 1000;

/// The edited page order, as runs of pages from one file with one rotation.
fn edits_json<L: DocLoader>(session: &Session<L>, id: TabId) -> Value {
    let mut data = edit_state_json(session, id);
    let Some(edits) = session.page_edits(id) else {
        data["runs"] = json!([]);
        data["runs_truncated"] = json!(false);
        return data;
    };
    let mut runs: Vec<Value> = Vec::new();
    let mut slots = edits.slots().iter().copied().peekable();
    let mut position = 0u32;
    let mut truncated = false;
    while let Some(first) = slots.next() {
        if runs.len() >= MAX_EDIT_RUNS {
            truncated = true;
            break;
        }
        let mut len = 1u32;
        while slots.peek().is_some_and(|next| next.source == first.source && next.quarter_turns == first.quarter_turns && next.page == first.page + len) {
            slots.next();
            len += 1;
        }
        let file = match first.source {
            0 => None,
            source => edits.added_sources().get(usize::from(source) - 1).map(|added| file_label(&added.path)),
        };
        runs.push(json!({
            "pages": page_span(position + 1, position + len),
            "from_file_pages": page_span(first.page + 1, first.page + len),
            "rotation_degrees": u32::from(first.quarter_turns) * 90,
            "added": first.source != 0,
            "untrusted": { "added_file": file },
        }));
        position += len;
    }
    data["runs"] = json!(runs);
    data["runs_truncated"] = json!(truncated);
    data
}

fn page_span(first: u32, last: u32) -> String {
    if first == last { first.to_string() } else { format!("{first}-{last}") }
}

fn tab_path<L: DocLoader>(session: &Session<L>, id: TabId) -> Result<PathBuf, AgentError> {
    session.path_of(id).ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))
}

/// Finds a tab by `#id`, by `active`, or by a piece of its name. Selectors never guess: an ambiguous name lists the candidates.
/// Which tab a selector means, for a host that handles a command itself.
pub fn select_tab<L: DocLoader>(session: &Session<L>, selector: &str) -> Result<TabId, AgentError> {
    select(session, selector)
}

pub(crate) fn select<L: DocLoader>(session: &Session<L>, selector: &str) -> Result<TabId, AgentError> {
    let not_found = || AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND);
    if selector.len() > MAX_SELECTOR_BYTES {
        return Err(not_found());
    }
    if selector == "active" {
        return session.active().ok_or_else(|| AgentError::new("no_such_tab", "No PDF is open.", exit::NOT_FOUND).then("Open one with `recto open <file.pdf>`."));
    }
    if let Some(number) = selector.strip_prefix('#') {
        let id = TabId(number.parse().map_err(|_| not_found())?);
        return session.tab(id).map(|_| id).ok_or_else(not_found);
    }
    let needle = selector.to_lowercase();
    let matches: Vec<_> = session.tabs().iter().filter(|t| t.name.to_lowercase().contains(&needle)).take(MAX_CANDIDATES + 1).collect();
    match matches.as_slice() {
        [] => Err(not_found()),
        [one] => Ok(one.id),
        many => {
            let mut error = AgentError::new("ambiguous", "More than one tab matches that name.", exit::AMBIGUOUS);
            with_tab_candidates(&mut error, many.iter().take(MAX_CANDIDATES).map(|t| (t.id, &*t.name)));
            Err(error)
        }
    }
}

/// Lists tabs in an error: their ids to use, and their names, which come from files, only under `untrusted`.
pub fn with_tab_candidates<'a>(error: &mut AgentError, tabs: impl Iterator<Item = (TabId, &'a str)>) {
    let mut names = Vec::new();
    error.candidates.clear();
    for (id, name) in tabs {
        let sel = format!("#{}", id.0);
        names.push(json!({ "sel": sel, "name": name }));
        error.candidates.push(sel);
    }
    error.untrusted = Some(Box::new(json!({ "candidates": names })));
}

/// Most candidates listed for an ambiguous name. Enough to choose from, and the rest of a thousand tabs isn't help.
const MAX_CANDIDATES: usize = 20;

/// What `wait` finds when it looks.
#[derive(Clone, Debug, PartialEq)]
pub enum ConditionState {
    Waiting,
    Met(Value),
    Failed(AgentError),
}

/// Looks at a condition once. Blocks for nothing but the session's lock.
pub fn check_condition<A: SessionAccess>(access: &A, condition: Condition, tab: &str, page: Option<u32>) -> ConditionState {
    // Looks at files, so it goes outside the lock.
    if condition == Condition::IndexingDone {
        return index::look(access, tab);
    }
    if condition == Condition::ContentsGenerated {
        return generated::look(access, tab);
    }
    access.with(|s| match condition {
        Condition::IndexingDone | Condition::ContentsGenerated => ConditionState::Waiting,
        Condition::TabOpen => match select(s, tab) {
            Ok(id) => ConditionState::Met(where_json(s, id)),
            Err(error) if error.exit == exit::AMBIGUOUS => ConditionState::Failed(error),
            Err(_) => ConditionState::Waiting,
        },
        Condition::TabClosed => match select(s, tab) {
            Ok(_) => ConditionState::Waiting,
            Err(error) if error.exit == exit::AMBIGUOUS => ConditionState::Failed(error),
            Err(_) => ConditionState::Met(json!({ "closed": true })),
        },
        Condition::PageDrawn => {
            let Some(page) = page.filter(|page| *page >= 1) else {
                return ConditionState::Failed(AgentError::new("no_such_page", "`wait page-drawn` needs `--page`, counting from 1.", exit::NOT_FOUND));
            };
            match select(s, tab) {
                Err(error) if error.exit == exit::AMBIGUOUS => ConditionState::Failed(error),
                Err(_) => ConditionState::Waiting,
                Ok(id) => match s.draw_note(id, page - 1) {
                    None => ConditionState::Waiting,
                    Some(DrawNote { failure: None, .. }) => ConditionState::Met(json!({ "tab": id.0, "page": page, "drawn": true })),
                    Some(DrawNote { failure: Some(code), .. }) => ConditionState::Failed(AgentError::new("page_failed", format!("Page {page} couldn't be drawn ({code})."), exit::FAILED).then("Try another page, or `recto text --page N` for its text.")),
                },
            }
        }
    })
}

/// Which event groups could change what a condition sees. A waiter is only looked at again when one of these happens.
pub fn condition_topics(condition: Condition) -> &'static [&'static str] {
    match condition {
        Condition::TabOpen | Condition::TabClosed => &["tabs"],
        Condition::PageDrawn => &["tabs", "draws"],
        Condition::IndexingDone => &["tabs", "indexing"],
        Condition::ContentsGenerated => &["tabs", "contents"],
    }
}

fn where_json<L: DocLoader>(session: &Session<L>, id: TabId) -> Value {
    let Some(tab) = session.tab(id) else { return Value::Null };
    let edited = session.is_edited(id);
    json!({
        "tab": id.0,
        "sel": format!("#{}", id.0),
        "page": tab.page + 1,
        "pages": session.pages_in(id),
        "file_pages": tab.page_count,
        "active": session.active() == Some(id),
        "edited": edited,
        "user_edits_undone": session.has_undone_owner_steps(id),
        "structure": if tab.renderer_only { "renderer_only" } else { "readable" },
        "problem": tab.problem.map(recto_core::OpenError::message),
        "note": if edited { Some(UNSAVED_NOTE) } else { session.has_undone_owner_steps(id).then_some(USER_EDITS_UNDONE_NOTE) },
        "file_notice": session.tab_notice(id).map(|notice| json!({ "code": notice.code(), "message": session.tab_notice_message(id) })),
        "untrusted": { "name": &*tab.name },
    })
}

/// Tabs in one `tabs` answer if not told otherwise, because a thousand of them is more than an agent can read. The rest are a `--from` away.
const DEFAULT_TABS_LISTED: u32 = 200;
const MAX_TABS_LISTED: u32 = 1000;

fn tabs_json<L: DocLoader>(session: &Session<L>, from: Option<u32>, limit: Option<u32>) -> Result<Value, AgentError> {
    let from = from.unwrap_or(0) as usize;
    let limit = positive_limit(limit, DEFAULT_TABS_LISTED, MAX_TABS_LISTED)?;
    let tabs: Vec<Value> = session.tabs().iter().skip(from).take(limit).map(|t| where_json(session, t.id)).collect();
    let end = from + tabs.len();
    Ok(json!({ "tabs": tabs, "count": session.tabs().len(), "next_from": (end < session.tabs().len()).then_some(end) }))
}

fn sidebar_json<L: DocLoader>(session: &Session<L>) -> Value {
    let model = session.sidebar();
    let rows: Vec<Value> = model
        .rows
        .iter()
        .map(|row| {
            // A note's words are Rust's, so they're not marked as the file's.
            if row.kind == RowKind::Note {
                return json!({ "kind": "note", "tab": row.tab.0, "text": row.text, "hover": row.notice, "action": row.action.map(recto_session::ContentsAction::id) });
            }
            json!({
                "kind": match row.kind { RowKind::Tab => "tab", _ => "contents" },
                "tab": row.tab.0,
                "page": row.page,
                "depth": row.depth,
                "current": row.current,
                "expandable": row.expandable,
                "expanded": row.expanded,
                "untrusted": { "text": row.text },
            })
        })
        .collect();
    json!({ "rows": rows, "rail_filled": model.rail_filled, "toc_open": model.toc_open, "place": model.place, "edited": model.edited, "undone_owner_steps": model.undone_owner_steps })
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_core::{DocFacts, OpenError, Toc};
    use std::path::Path;

    #[derive(Clone)]
    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts { page_count: 10, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false })
        }
    }

    /// Failure: waking a cold tab reads its file with the session's lock held, so a file on a dead volume hangs `status`, `tabs` and the UI thread's every pull with it. The file is read with the lock free, for a tap, for a close that wakes what's next, and for an open of a file whose tab is cold, and it isn't read twice.
    #[test]
    fn waking_a_cold_tab_reads_its_file_with_the_session_free() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        #[derive(Clone, Default)]
        struct Gated {
            armed: Arc<AtomicBool>,
            inside: Arc<AtomicBool>,
            release: Arc<AtomicBool>,
            reads: Arc<AtomicU32>,
        }
        impl DocLoader for Gated {
            fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
                if self.armed.load(Ordering::SeqCst) {
                    self.reads.fetch_add(1, Ordering::SeqCst);
                    self.inside.store(true, Ordering::SeqCst);
                    while !self.release.load(Ordering::SeqCst) {
                        std::thread::yield_now();
                    }
                }
                Ok(DocFacts { page_count: 10, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false })
            }
        }
        let gate = Gated::default();
        let session = Arc::new(Mutex::new(Session::new(gate.clone())));
        for name in ["a", "b", "c", "d", "e", "f"] {
            session.lock().unwrap().open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap();
        }
        let wake = |command: Command| {
            gate.reads.store(0, Ordering::SeqCst);
            gate.inside.store(false, Ordering::SeqCst);
            gate.release.store(false, Ordering::SeqCst);
            gate.armed.store(true, Ordering::SeqCst);
            let shared = Arc::clone(&session);
            let running = std::thread::spawn(move || execute(&*shared, Level::Control, &command, &mut Vec::new()).map(|_| ()));
            while !gate.inside.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            assert!(session.try_lock().is_ok(), "the session's lock is held across the read");
            gate.release.store(true, Ordering::SeqCst);
            running.join().unwrap().unwrap();
            gate.armed.store(false, Ordering::SeqCst);
            assert_eq!(gate.reads.load(Ordering::SeqCst), 1, "read once");
        };
        wake(Command::Tap { tab: "#1".into() });
        assert_eq!(session.lock().unwrap().active().map(|id| id.0), Some(1));
        wake(Command::Close { tab: "#1".into() });
        assert_eq!(session.lock().unwrap().active().map(|id| id.0), Some(2));
        // Opening a file whose tab has gone cold.
        wake(Command::Open { path: PathBuf::from("/d/c.pdf"), page: None });
        assert_eq!(session.lock().unwrap().active().map(|id| id.0), Some(3));
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
        assert!(run(&mut session, Level::ReadOnly, &Command::Tabs { from: None, limit: None }).is_ok());
    }

    /// Failure: an agent the user turned off reads a page's text, the one thing Off exists to stop. Asserted on the level itself and on the
    /// call, which must refuse before any worker starts.
    #[test]
    fn reading_page_text_needs_more_than_off() {
        let text = Command::Text { tab: "#1".into(), page: 1 };
        assert_eq!(text.access(), Access::At(Level::ReadOnly));
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
        assert_eq!(budget_command(None, None).access(), Access::At(Level::ReadOnly));
        assert_eq!(budget_command(Some(4096), None).access(), Access::At(Level::Full));
        assert_eq!(budget_command(None, Some(4096)).access(), Access::At(Level::Full));
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
        assert_eq!(error.candidates, ["#1", "#2"], "ids to use, and no names outside `untrusted`");
        assert_eq!(error.untrusted.unwrap()["candidates"][0]["name"], "Report 2025");
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
        assert!(matches!(parse(r#"{"cmd":"tabs"}"#), Ok(Command::Tabs { .. })));
    }

    fn every_command() -> Vec<Command> {
        let tab = || "#1".to_string();
        vec![
            Command::Open { path: "/d/a.pdf".into(), page: None },
            Command::Tabs { from: None, limit: None },
            Command::Sidebar {},
            Command::Tap { tab: tab() },
            Command::Goto { tab: tab(), page: 1 },
            Command::Close { tab: tab() },
            Command::Budget {},
            Command::Compare { tabs: None, sync: None, off: false },
            Command::Compare { tabs: None, sync: None, off: true },
            Command::Render { tab: tab(), page: 1, scale: 2.0, out: "/d/x.png".into() },
            Command::ExportSvg { tab: tab(), page: 1, out: "/d/x.svg".into(), rect: None, text: None, scale: 2.0, strip_metadata: false },
            Command::Text { tab: tab(), page: 1 },
            Command::Select { tab: tab(), page: Some(1), to_page: None, chars: None, at: None, to: None, unit: None, all: false, anyway: false },
            Command::Copy { tab: tab(), page: Some(1), to_page: None, chars: None, at: None, to: None, unit: None, all: false, anyway: false },
            budget_command(None, None),
            Command::BudgetViewport { tab: tab(), viewport: "10x10".into(), first: None, scale: 2.0, strategy: None },
            Command::Where { tab: tab() },
            Command::Facts { tab: tab() },
            Command::Contents { tab: tab(), from: None, limit: None, depth: None, generated: false },
            Command::Search { tab: tab(), query: "x".into(), pages: None, limit: None, scan: false, regex: false, rank: false, context: None },
            Command::Index { tab: tab(), agent: false },
            Command::IndexStatus { tab: tab() },
            Command::IndexClear { tab: tab(), all: false },
            Command::Signatures { tab: tab() },
            Command::Edits { tab: tab() },
            Command::PageMove { tab: tab(), pages: "1".into(), to: 1 },
            Command::PageRotate { tab: tab(), pages: "1".into(), degrees: 90 },
            Command::PageDelete { tab: tab(), pages: "1".into() },
            Command::PageInsert { tab: tab(), path: "/d/b.pdf".into(), at: 1 },
            Command::Undo { tab: tab() },
            Command::Redo { tab: tab() },
            Command::Discard { tab: tab() },
            Command::ClearHistory { tab: tab() },
            Command::SessionSleep { tab: tab() },
            Command::Save { tab: tab() },
            Command::Sign {},
            Command::Status {},
            Command::PermissionShow {},
            Command::PermissionRequest { level: Level::Full, timeout_ms: None },
            Command::LevelChanged {},
            Command::Wait { condition: Condition::TabOpen, tab: tab(), page: None, timeout_ms: 1 },
            Command::Watch { events: None, timeout_ms: None, count: None },
            Command::WatchStop {},
            Command::Log { last: None },
            Command::View { paper: None, sidebar: None, agent_icon: None, zoom: None, layout: None },
            Command::QaRead { verb: "state".into(), out: None, timeout_ms: None },
            Command::QaDrive { verb: "zoom".into(), value: "1".into(), timeout_ms: None },
            Command::Stop {},
            Command::SessionShow {},
            Command::SessionSave {},
            Command::SessionRestore {},
            Command::SessionCheck {},
            Command::UpdateStatus {},
            Command::UpdateCheck {},
            Command::UpdateInstall {},
            Command::Settings { auto_update: None },
        ]
    }

    /// Failure: an agent below Full can install an update, or turn on what would install one for it, or can't read how things stand at Read-only. The owner's bar: installing needs Full. A session with no app says it's not a matter of level.
    #[test]
    fn installing_needs_full_and_reading_needs_only_read() {
        assert_eq!(Command::UpdateInstall {}.access(), Access::At(Level::Full));
        assert_eq!(Command::Settings { auto_update: Some("on".into()) }.access(), Access::At(Level::Full));
        assert_eq!(Command::Settings { auto_update: Some("off".into()) }.access(), Access::At(Level::Full));
        assert_eq!(Command::Settings { auto_update: None }.access(), Access::At(Level::ReadOnly));
        assert_eq!(Command::UpdateStatus {}.access(), Access::At(Level::ReadOnly));
        assert_eq!(Command::UpdateCheck {}.access(), Access::At(Level::Control));
        let mut s = with_tabs(&["a"]);
        for command in [Command::UpdateStatus {}, Command::UpdateCheck {}, Command::UpdateInstall {}, Command::Settings { auto_update: None }] {
            let error = run(&mut s, Level::Full, &command).unwrap_err();
            assert_eq!(error.code, "needs_app");
            assert_ne!(error.exit, exit::PERMISSION);
        }
        assert!(run(&mut s, Level::Control, &Command::UpdateInstall {}).unwrap_err().exit == exit::PERMISSION, "below the level, it's the level that's said");
    }

    /// Failure: changing how pages are shown works at Read-only, or asking needs more than asking should, because the row can state only one level.
    #[test]
    fn view_asks_at_read_only_and_changes_at_control() {
        assert_eq!(Command::View { paper: None, sidebar: None, agent_icon: None, zoom: None, layout: None }.access(), Access::At(Level::ReadOnly));
        assert_eq!(Command::View { paper: Some("white".into()), sidebar: None, agent_icon: None, zoom: None, layout: None }.access(), Access::At(Level::Control));
        assert_eq!(Command::View { paper: None, sidebar: Some("off".into()), agent_icon: None, zoom: None, layout: None }.access(), Access::At(Level::Control));
    }

    /// Failure: a session with no app takes a QA command and does nothing, or says something that reads as though it worked. It's refused plainly, and not as a matter of level, since no level would help.
    #[test]
    fn qa_commands_need_an_app() {
        let mut s = with_tabs(&["a"]);
        for command in [Command::QaRead { verb: "state".into(), out: None, timeout_ms: None }, Command::QaDrive { verb: "zoom".into(), value: "1".into(), timeout_ms: None }] {
            let error = run(&mut s, Level::Full, &command).unwrap_err();
            assert_eq!(error.code, "qa_needs_app");
            assert_ne!(error.exit, exit::PERMISSION);
        }
    }

    /// Failure: a command is added with no row in the table, so it has no level, no docs and no CLI. `Command::tag` is exhaustive, so a new variant can't compile without a name, and this holds the name to a row.
    #[test]
    fn every_command_has_a_row_and_every_session_row_has_a_command() {
        let commands = every_command();
        for command in &commands {
            let spec = table::by_tag(command.tag()).unwrap_or_else(|| panic!("{} has no row", command.tag()));
            assert_eq!(spec.route, table::Route::Session, "{}", spec.name);
        }
        for spec in table::TABLE.iter().filter(|spec| spec.route == table::Route::Session) {
            assert!(commands.iter().any(|command| command.tag() == spec.tag), "row {} has no command", spec.name);
        }
    }

    /// Failure: an agent can't do what ⌘-click and the right-click menu do: put tabs side by side, sync them, and go back to one; or asking changes something, or what's wrong isn't said in words with a way on.
    #[test]
    fn compare_puts_tabs_side_by_side_and_says_so() {
        let mut session = with_tabs(&["a", "b", "c", "d"]);
        let compare = |tabs: Option<&str>, sync: Option<&str>, off: bool| Command::Compare { tabs: tabs.map(str::to_string), sync: sync.map(str::to_string), off };
        let asked = run(&mut session, Level::ReadOnly, &compare(None, None, false)).unwrap();
        assert_eq!(asked["comparing"], false);
        assert_eq!(run(&mut session, Level::ReadOnly, &compare(Some("#1,#2"), None, false)).unwrap_err().exit, exit::PERMISSION, "changing it is Control");
        let two = run(&mut session, Level::Control, &compare(Some("#1, #3"), None, false)).unwrap();
        assert_eq!(two["comparing"], true);
        assert_eq!(two["panes"].as_array().unwrap().iter().map(|pane| pane["tab"].as_u64().unwrap()).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(two["sync"], "off", "separate is the default");
        let synced = run(&mut session, Level::Control, &compare(None, Some("on"), false)).unwrap();
        assert_eq!(synced["sync"], "on");
        assert_eq!(synced["sync_from"], json!([1, 1]));
        assert_eq!(run(&mut session, Level::Control, &compare(Some("#1,#2,#3,#4"), None, false)).unwrap_err().code, "too_many_panes");
        assert_eq!(run(&mut session, Level::Control, &compare(Some("#1"), None, false)).unwrap_err().code, "too_few_panes");
        assert_eq!(run(&mut session, Level::Control, &compare(Some("#1,#1"), None, false)).unwrap_err().code, "same_tab_twice");
        assert_eq!(run(&mut session, Level::Control, &compare(Some("#1,#9"), None, false)).unwrap_err().code, "no_such_tab");
        assert_eq!(run(&mut session, Level::Control, &compare(None, Some("maybe"), false)).unwrap_err().code, "usage");
        assert_eq!(run(&mut session, Level::Control, &compare(Some("#1,#2"), None, true)).unwrap_err().code, "usage");
        let ended = run(&mut session, Level::Control, &compare(None, None, true)).unwrap();
        assert_eq!(ended["comparing"], false);
        assert_eq!(run(&mut session, Level::Control, &compare(None, Some("on"), false)).unwrap_err().code, "not_comparing");
        run(&mut session, Level::Control, &compare(Some("#2,#4"), None, false)).unwrap();
        run(&mut session, Level::Control, &Command::Tap { tab: "#4".into() }).unwrap();
        assert_eq!(run(&mut session, Level::ReadOnly, &compare(None, None, false)).unwrap()["comparing"], false, "a tap goes back to one");
    }

    /// Failure: two faces of one command disagree on what they need, so which row is found first decides who gets in.
    #[test]
    fn rows_sharing_a_command_share_their_access() {
        for spec in table::TABLE {
            let first = table::by_tag(spec.tag).unwrap();
            assert_eq!(first.access, spec.access, "{}", spec.name);
        }
    }

    /// Failure: something reaches a document at Off, or a level above doesn't include what's below, or signing or setting the level is open to any level. Walks every command at every level.
    #[test]
    fn walking_every_command_at_every_level() {
        let mut off_allowed = Vec::new();
        for command in every_command() {
            let access = command.access();
            let mut session = with_tabs(&["a"]);
            let mut was_allowed = false;
            for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
                let result = run(&mut session, level, &command);
                let is_allowed = allowed(level, access);
                let refused = result.as_ref().err().is_some_and(|error| error.exit == exit::PERMISSION);
                assert_eq!(refused, !is_allowed, "{} at {level:?}: {result:?}", command.tag());
                assert!(!was_allowed || is_allowed, "{} is allowed at a lower level but not at {level:?}", command.tag());
                was_allowed = is_allowed;
                if level == Level::Off && is_allowed {
                    off_allowed.push(command.tag());
                }
            }
        }
        off_allowed.sort_unstable();
        assert_eq!(off_allowed, ["level_changed", "permission_request", "permission_show", "save", "status"], "what's open at Off must stay this short");
        assert_eq!(Command::Sign {}.access(), Access::Nobody);
        for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
            assert!(!allowed(level, Access::Nobody) && !allowed(level, Access::UserOnly));
        }
        assert!(table::TABLE.iter().filter(|spec| spec.access == Access::UserOnly).all(|spec| spec.route == table::Route::Here), "what's for the user alone must not go over the socket");
        assert!(table::TABLE.iter().filter(|spec| spec.tag == "permission_set").all(|spec| spec.access == Access::UserOnly));
        assert!(parse(r#"{"cmd":"permission_set","level":"full"}"#).is_err(), "no command on the wire sets a level");
    }

    /// Failure: a refusal doesn't say what's missing or gives a command that doesn't exist, so an agent is stuck or guesses.
    #[test]
    fn a_refusal_gives_one_command_that_works_at_off() {
        for level in [Level::ReadOnly, Level::Control, Level::Full] {
            let error = refusal("text", Level::Off, Access::At(level));
            assert_eq!((error.code, error.exit), ("permission_denied", exit::PERMISSION));
            assert!(error.message.contains(level.label()) && error.message.contains("Off"), "{}", error.message);
            let ask = error.ask.unwrap();
            let words = skill::shell_words(&ask);
            assert_eq!(words[0], "recto");
            let line = table::parse_cli(&words[1..]).unwrap();
            assert_eq!(line.spec.tag, "permission_request");
            assert_eq!(line.spec.access, Access::At(Level::Off), "the asking must work at the level it's asked from");
        }
    }

    /// Failure: an error comes with no next step, so an agent is left guessing. Every code the session, core and renderer can give must say one, whichever kind it is.
    #[test]
    fn every_error_says_the_next_step() {
        use recto_session::edit::EditError;
        let session_errors = [
            SessionError::NoSuchTab,
            SessionError::TooManyTabs,
            SessionError::UnsavedChanges,
            SessionError::RendererOnly,
            SessionError::Edit(EditError::OutOfRange),
            SessionError::Edit(EditError::LastPage),
            SessionError::Edit(EditError::TooManyPages),
            SessionError::Edit(EditError::TooManySources),
            SessionError::Edit(EditError::SourceChanged),
            SessionError::Open(OpenError::NotFound),
            SessionError::Open(OpenError::NotAllowed),
            SessionError::Open(OpenError::PasswordNeeded),
            SessionError::Open(OpenError::NotPdf),
        ];
        for error in session_errors {
            let error = AgentError::from(error);
            assert!(!error.next.is_empty(), "{}", error.code);
        }
        for exit_code in [exit::FAILED, exit::USAGE, exit::NOT_FOUND, exit::AMBIGUOUS, exit::PERMISSION, exit::NOT_RUNNING, exit::TIMED_OUT, exit::PROTECTED, exit::BUSY] {
            assert!(!AgentError::new("some_new_code", "m", exit_code).next.is_empty(), "exit {exit_code}");
        }
    }

    /// Failure: the exit codes move, which breaks every script that reads them. Fixed by the agent report.
    #[test]
    fn exit_codes_are_fixed() {
        assert_eq!((exit::OK, exit::FAILED, exit::USAGE, exit::NOT_FOUND, exit::AMBIGUOUS, exit::PERMISSION, exit::NOT_RUNNING, exit::TIMED_OUT, exit::PROTECTED, exit::BUSY), (0, 1, 2, 3, 4, 5, 6, 7, 8, 10));
    }

    fn edit_session() -> Session<Fake> {
        with_tabs(&["a"])
    }

    /// Failure: page edits reach the session wrongly: off by one on a one-based number, or the tab doesn't report it has unsaved edits.
    #[test]
    fn page_commands_use_one_based_numbers_and_report_unsaved_edits() {
        let mut s = edit_session();
        let moved = run(&mut s, Level::Control, &Command::PageMove { tab: "#1".into(), pages: "3".into(), to: 1 }).unwrap();
        assert_eq!(moved["edited"], true);
        assert_eq!(moved["saved"], false);
        assert!(moved["note"].as_str().unwrap().contains("Saving isn't built"), "it must say so plainly: {moved}");
        let id = TabId(1);
        assert_eq!(s.page_edits(id).unwrap().slots()[0].page, 2);
        let tabs = run(&mut s, Level::ReadOnly, &Command::Tabs { from: None, limit: None }).unwrap();
        assert_eq!(tabs["tabs"][0]["edited"], true);
        run(&mut s, Level::Control, &Command::PageRotate { tab: "active".into(), pages: "2-3".into(), degrees: -90 }).unwrap();
        assert_eq!(s.page_edits(id).unwrap().slots()[1].quarter_turns, 3);
        let turns = |s: &Session<_>| s.page_edits(id).unwrap().slots().iter().map(|slot| slot.quarter_turns).collect::<Vec<u8>>();
        let before = turns(&s);
        run(&mut s, Level::Control, &Command::PageRotate { tab: "active".into(), pages: "all".into(), degrees: 90 }).unwrap();
        assert!(turns(&s).iter().zip(&before).all(|(now, was)| *now == (was + 1) % 4), "every page a quarter more: {:?} from {before:?}", turns(&s));
        assert!(s.undo_edit(id));
        assert_eq!(turns(&s), before, "one undo takes every page back, as it was one step");
        run(&mut s, Level::Control, &Command::PageDelete { tab: "active".into(), pages: "1".into() }).unwrap();
        assert_eq!(s.pages_in(id), 9);
        run(&mut s, Level::Control, &Command::PageInsert { tab: "active".into(), path: "/d/b.pdf".into(), at: 10 }).unwrap();
        assert_eq!(s.pages_in(id), 19);
        let shown = run(&mut s, Level::ReadOnly, &Command::Edits { tab: "active".into() }).unwrap();
        assert_eq!(shown["runs"].as_array().unwrap().last().unwrap()["added"], true);
        assert_eq!(shown["runs"].as_array().unwrap().last().unwrap()["untrusted"]["added_file"], "b.pdf");
        run(&mut s, Level::Control, &Command::Undo { tab: "active".into() }).unwrap();
        assert_eq!(s.pages_in(id), 9);
        let redone = run(&mut s, Level::Control, &Command::Redo { tab: "active".into() }).unwrap();
        assert_eq!((&redone["pages"], &redone["can_redo"]), (&json!(19), &json!(false)));
    }

    /// Failure: a bad request reads as success, or changes the document. Each must be refused with its own code and leave things as they were.
    #[test]
    fn bad_page_commands_change_nothing() {
        let mut s = edit_session();
        let id = TabId(1);
        let tab = || "#1".to_string();
        assert_eq!(run(&mut s, Level::Control, &Command::PageDelete { tab: tab(), pages: "1-10".into() }).unwrap_err().code, "last_page");
        let far = run(&mut s, Level::Control, &Command::PageRotate { tab: tab(), pages: "11".into(), degrees: 90 }).unwrap_err();
        assert_eq!((far.code, far.exit), ("page_out_of_range", exit::NOT_FOUND));
        assert_eq!(run(&mut s, Level::Control, &Command::PageRotate { tab: tab(), pages: "1".into(), degrees: 45 }).unwrap_err().code, "bad_rotation");
        assert_eq!(run(&mut s, Level::Control, &Command::PageRotate { tab: tab(), pages: "1".into(), degrees: 720 }).unwrap_err().code, "bad_rotation", "a turn that does nothing mustn't count as an edit");
        assert_eq!(run(&mut s, Level::Control, &Command::PageMove { tab: tab(), pages: "0".into(), to: 1 }).unwrap_err().code, "bad_pages");
        assert_eq!(run(&mut s, Level::Control, &Command::PageMove { tab: tab(), pages: "9-3".into(), to: 1 }).unwrap_err().code, "bad_pages");
        assert_eq!(run(&mut s, Level::Control, &Command::PageRotate { tab: tab(), pages: "4-".into(), degrees: 90 }).unwrap_err().code, "bad_pages", "to the end isn't for edits");
        assert_eq!(run(&mut s, Level::Control, &Command::PageRotate { tab: tab(), pages: "4-x-".into(), degrees: 90 }).unwrap_err().code, "bad_pages");
        assert_eq!(run(&mut s, Level::Control, &Command::PageMove { tab: tab(), pages: "1".into(), to: 0 }).unwrap_err().code, "no_such_page");
        assert_eq!(run(&mut s, Level::Control, &Command::PageInsert { tab: tab(), path: "relative.pdf".into(), at: 1 }).unwrap_err().code, "bad_path");
        assert_eq!(run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap_err().code, "nothing_to_undo");
        assert_eq!(run(&mut s, Level::Control, &Command::Redo { tab: tab() }).unwrap_err().code, "nothing_to_redo");
        assert!(!s.is_edited(id));
        assert_eq!(s.pages_in(id), 10);
    }

    /// Failure: `save` pretends to work, so someone believes their edits are on disk. And closing must not lose them without saying.
    #[test]
    fn saving_says_plainly_that_it_isnt_built_and_closing_keeps_edits() {
        let mut s = edit_session();
        run(&mut s, Level::Control, &Command::PageRotate { tab: "#1".into(), pages: "1".into(), degrees: 90 }).unwrap();
        let save = run(&mut s, Level::Full, &Command::Save { tab: "#1".into() }).unwrap_err();
        assert_eq!(save.code, "not_built");
        assert!(save.message.contains("nothing was written"), "{}", save.message);
        for level in [Level::Control, Level::Full] {
            let close = run(&mut s, level, &Command::Close { tab: "#1".into() }).unwrap_err();
            assert_eq!(close.code, "unsaved_changes", "{level:?}");
            assert!(close.next.contains("discard"), "{}", close.next);
        }
        assert_eq!(s.tabs().len(), 1);
        assert_eq!(run(&mut s, Level::ReadOnly, &Command::Discard { tab: "#1".into() }).unwrap_err().code, "permission_denied", "throwing edits away asks for what making them does");
        run(&mut s, Level::Control, &Command::Discard { tab: "#1".into() }).unwrap();
        assert!(!s.is_edited(TabId(1)));
        assert!(run(&mut s, Level::Control, &Command::Close { tab: "#1".into() }).is_ok());
    }

    fn owners_rotation(s: &mut Session<Fake>, page: u32) {
        s.edit_pages(TabId(1), |edits| edits.rotate(page, 1, 1)).unwrap();
    }

    /// A rotation of page 1, which tells apart "the agent's edit is still here" from "it's gone".
    fn turned_pages(s: &Session<Fake>) -> Vec<u8> {
        s.page_edits(TabId(1)).map_or_else(Vec::new, |edits| edits.slots().iter().map(|slot| slot.quarter_turns).collect())
    }

    fn agent_attempts() -> Vec<Command> {
        let tab = || "#1".to_string();
        vec![Command::PageRotate { tab: tab(), pages: "9".into(), degrees: 90 }, Command::PageDelete { tab: tab(), pages: "10".into() }, Command::PageMove { tab: tab(), pages: "9".into(), to: 10 }, Command::PageInsert { tab: tab(), path: "/d/b.pdf".into(), at: 1 }, Command::Close { tab: tab() }]
    }

    /// Failure: the attack the rule is for. The user edits; an agent at Control undoes it all, makes an edit, undoes that, and closes or discards. Each step must keep the user's edits, whatever order it's tried in, and bringing them back must get every one.
    #[test]
    fn an_agent_cant_erase_the_users_edits_at_any_step() {
        let mut s = edit_session();
        let id = TabId(1);
        let tab = || "#1".to_string();
        owners_rotation(&mut s, 0);
        owners_rotation(&mut s, 1);
        let users = turned_pages(&s);
        let check_still_there = |s: &Session<Fake>, when: &str| {
            assert!(s.page_edits(id).is_some_and(recto_session::edit::PageEdits::holds_owner_edits), "{when}");
            assert_eq!(s.tabs().len(), 1, "{when}");
        };
        // Directly: discard and close while they're still applied.
        assert_eq!(run(&mut s, Level::Control, &Command::Discard { tab: tab() }).unwrap_err().code, "would_discard_user_edits");
        assert_eq!(run(&mut s, Level::Control, &Command::Close { tab: tab() }).unwrap_err().code, "unsaved_changes");
        check_still_there(&s, "before any undo");
        // Undone, one at a time. Undoing is allowed, since it can be redone. Nothing that clears them is, at any point.
        for left in [1, 0] {
            run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();
            for attempt in agent_attempts() {
                let refused = run(&mut s, Level::Control, &attempt).unwrap_err();
                assert_eq!((refused.code, refused.exit), ("would_clear_redo", exit::PERMISSION), "{} with {left} left", attempt.tag());
                assert!(refused.next.contains("recto redo") && refused.next.contains(ASK_FULL), "{}", refused.next);
                assert_eq!(refused.ask.as_deref(), Some(ASK_FULL));
                assert!(refused.message.contains("user's"), "the message must say whose they are: {}", refused.message);
            }
            let discard = run(&mut s, Level::Control, &Command::Discard { tab: tab() }).unwrap_err();
            assert_eq!((discard.code, discard.exit), ("would_discard_user_edits", exit::PERMISSION));
            assert!(discard.message.contains("user's"), "{}", discard.message);
            check_still_there(&s, "after an undo");
        }
        assert!(!s.is_edited(id), "with both undone there's nothing unsaved, and that's when a close would have got through");
        // And redo gets back all of it, exactly.
        run(&mut s, Level::Control, &Command::Redo { tab: tab() }).unwrap();
        run(&mut s, Level::Control, &Command::Redo { tab: tab() }).unwrap();
        assert_eq!(turned_pages(&s), users);
        // The agent's own work on top: edit, undo, edit again, all fine, and the user's stay under it.
        run(&mut s, Level::Control, &Command::PageRotate { tab: tab(), pages: "9".into(), degrees: 90 }).unwrap();
        run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();
        run(&mut s, Level::Control, &Command::PageDelete { tab: tab(), pages: "10".into() }).expect("only the agent's own undone step goes");
        run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();
        // Closing with the agent's own step undone still can't happen, because the user's edits are unsaved.
        let close = run(&mut s, Level::Control, &Command::Close { tab: tab() }).unwrap_err();
        assert_eq!(close.code, "unsaved_changes");
        assert!(close.message.contains("user's") && close.next.contains(ASK_FULL), "{} {}", close.message, close.next);
        assert_eq!(turned_pages(&s), users, "nothing of the user's has gone");
        // At Full, it's the holder's call, as it was.
        run(&mut s, Level::Full, &Command::Discard { tab: tab() }).unwrap();
        assert!(run(&mut s, Level::Full, &Command::Close { tab: tab() }).is_ok());
    }

    /// Failure: an agent's `undo` hides the user's edit. It's off the page, so every answer read `saved: true` and nothing said it was there to lose, and the session's own close, the app's way out, would have gone through. At Full it's still the holder's call, and goes through this time because it's said on purpose.
    #[test]
    fn undoing_the_users_edit_doesnt_read_as_nothing_there() {
        let mut s = edit_session();
        let id = TabId(1);
        let tab = || "#1".to_string();
        owners_rotation(&mut s, 0);
        let undone = run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();
        assert_eq!((&undone["edited"], &undone["saved"], &undone["user_edits_undone"], &undone["can_redo"]), (&json!(false), &json!(false), &json!(true), &json!(true)), "{undone}");
        assert!(human::render("undo", &undone).contains("recto redo"), "{undone}");
        let where_now = run(&mut s, Level::ReadOnly, &Command::Where { tab: tab() }).unwrap();
        assert_eq!(where_now["user_edits_undone"], true, "{where_now}");
        assert!(human::render("where", &where_now).contains("recto redo"), "{where_now}");
        assert_eq!(run(&mut s, Level::ReadOnly, &Command::Sidebar {}).unwrap()["undone_owner_steps"], true);
        assert_eq!(s.close(id), Err(SessionError::UnsavedChanges), "the app's route refuses too");
        assert_eq!(run(&mut s, Level::Control, &Command::Close { tab: tab() }).unwrap_err().code, "would_clear_redo");
        let closed = run(&mut s, Level::Full, &Command::Close { tab: tab() }).unwrap();
        assert_eq!(closed["count"], 0, "at Full it closes, as it did: {closed}");
    }

    /// Failure: an agent that does `page move`, `undo`, then anything else is refused and has to ask for Full to clear up after itself. A blind agent got stuck exactly here.
    #[test]
    fn an_agent_can_throw_away_its_own_edits() {
        let mut s = edit_session();
        let id = TabId(1);
        let tab = || "#1".to_string();
        run(&mut s, Level::Control, &Command::PageMove { tab: tab(), pages: "3".into(), to: 1 }).unwrap();
        run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();
        run(&mut s, Level::Control, &Command::PageDelete { tab: tab(), pages: "2".into() }).expect("a new edit may clear its own undone step");
        assert!(!s.page_edits(id).unwrap().can_redo());
        // A close with unsaved edits stops, and says the way on, and that way works at Control.
        let close = run(&mut s, Level::Control, &Command::Close { tab: tab() }).unwrap_err();
        assert_eq!((close.code, close.exit), ("unsaved_changes", exit::FAILED));
        assert!(close.next.contains("recto discard") && !close.next.contains("Full") && close.ask.is_none(), "{}", close.next);
        run(&mut s, Level::Control, &Command::Discard { tab: tab() }).expect("its own edits are its own to throw away");
        assert!(!s.is_edited(id));
        // Undone, with nothing unsaved: it can close, and lose only that.
        run(&mut s, Level::Control, &Command::PageRotate { tab: tab(), pages: "1".into(), degrees: 90 }).unwrap();
        run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();
        assert!(s.page_edits(id).unwrap().can_redo());
        run(&mut s, Level::Control, &Command::Close { tab: tab() }).expect("nothing of the user's is at stake");
        assert!(s.tabs().is_empty());
    }

    /// Failure: a plain answer to an edit says only how long the document is now, so an agent can't tell from it what was done, and `page move 3 --to 3` reads as a move. And `tabs` can't show how far a tab's length has come from its file's.
    #[test]
    fn an_edit_says_what_it_did() {
        let mut s = edit_session();
        let tab = || "#1".to_string();
        let said = |s: &mut Session<Fake>, command: Command| {
            let data = run(s, Level::Control, &command).unwrap();
            let shown = human::render(command.tag(), &data);
            assert!(shown.starts_with(data["did"].as_str().unwrap()), "first, before the state: {shown}");
            data
        };
        assert_eq!(said(&mut s, Command::PageRotate { tab: tab(), pages: "4".into(), degrees: 270 })["did"], "Rotated page 4 by 270 degrees.");
        assert_eq!(said(&mut s, Command::PageRotate { tab: tab(), pages: "1-2".into(), degrees: -90 })["did"], "Rotated pages 1-2 by -90 degrees.");
        assert_eq!(said(&mut s, Command::PageMove { tab: tab(), pages: "3".into(), to: 1 })["did"], "Moved page 3 to 1.");
        assert_eq!(said(&mut s, Command::PageMove { tab: tab(), pages: "4-5".into(), to: 7 })["did"], "Moved pages 4-5 to 7.");
        assert_eq!(said(&mut s, Command::PageDelete { tab: tab(), pages: "5".into() })["did"], "Deleted page 5.");
        assert_eq!(said(&mut s, Command::PageInsert { tab: tab(), path: "/d/b.pdf".into(), at: 2 })["did"], "Added 10 pages at page 2.");
        assert_eq!(said(&mut s, Command::Undo { tab: tab() })["did"], "Undid the last edit.");
        assert_eq!(said(&mut s, Command::Redo { tab: tab() })["did"], "Redid the last undone edit.");
        let shown = human::render("page_delete", &run(&mut s, Level::Control, &Command::PageDelete { tab: tab(), pages: "2-3".into() }).unwrap());
        assert!(shown.starts_with("Deleted pages 2-3. 17 pages now, 10 in the file."), "{shown}");
        let tabs = run(&mut s, Level::ReadOnly, &Command::Tabs { from: None, limit: None }).unwrap();
        assert_eq!((&tabs["tabs"][0]["pages"], &tabs["tabs"][0]["file_pages"]), (&json!(17), &json!(10)), "{tabs}");
        assert_eq!(said(&mut s, Command::Discard { tab: tab() })["did"], "Threw away this tab's edits.");
        assert_eq!(said(&mut s, Command::Discard { tab: tab() })["did"], "There were no edits to throw away.");
        let nothing = said(&mut s, Command::PageMove { tab: tab(), pages: "3".into(), to: 3 });
        assert_eq!((&nothing["did"], &nothing["edited"], &nothing["saved"]), (&json!("Page 3 is already at 3, so nothing changed."), &json!(false), &json!(true)));
        assert_eq!(said(&mut s, Command::PageMove { tab: tab(), pages: "3-4".into(), to: 3 })["did"], "Pages 3-4 are already at 3, so nothing changed.");
        assert!(run(&mut s, Level::Control, &Command::Close { tab: tab() }).is_ok(), "and so it closes");
    }

    /// Failure: an edit through the agent interface is recorded as the user's, so it's protected from the agent that made it, or recorded as nobody's. Every edit command must be an agent's, however it gets to the session.
    #[test]
    fn every_edit_through_the_agent_interface_is_an_agents() {
        let mut s = edit_session();
        let tab = || "#1".to_string();
        for command in [
            Command::PageRotate { tab: tab(), pages: "1".into(), degrees: 90 },
            Command::PageMove { tab: tab(), pages: "1".into(), to: 2 },
            Command::PageDelete { tab: tab(), pages: "1".into() },
            Command::PageInsert { tab: tab(), path: "/d/b.pdf".into(), at: 1 },
        ] {
            run(&mut s, Level::Control, &command).unwrap();
            assert!(!s.page_edits(TabId(1)).unwrap().holds_owner_edits(), "{}", command.tag());
        }
        run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();
        run(&mut s, Level::Control, &Command::Redo { tab: tab() }).unwrap();
        assert!(!s.page_edits(TabId(1)).unwrap().holds_owner_edits(), "undo and redo don't change that");
    }

    /// Failure: an agent makes enough edits that the user's step falls off the capped list, and then its discard counts only what's left on the list, and throws away the user's edit, still in the page order, along with its own. And the reverse: an agent's step falling off must not stop it clearing up.
    #[test]
    fn a_step_past_the_undo_cap_still_counts_for_discard() {
        let mut s = edit_session();
        let tab = || "#1".to_string();
        owners_rotation(&mut s, 0);
        for _ in 0..recto_session::edit::MAX_UNDO - 1 {
            run(&mut s, Level::Control, &Command::PageMove { tab: tab(), pages: "2".into(), to: 3 }).unwrap();
        }
        // Below Full, the owner's step at the bottom can't be pushed out, and it says why. That's the gap in the plan: an agent editing past the cap so what was done can't be undone.
        let refused = run(&mut s, Level::Control, &Command::PageMove { tab: tab(), pages: "2".into(), to: 3 }).unwrap_err();
        assert_eq!(refused.code, "history_full");
        assert!(!s.page_edits(TabId(1)).unwrap().history_dropped(), "and it's still there");
        // At Full it's the holder's call.
        run(&mut s, Level::Full, &Command::PageMove { tab: tab(), pages: "2".into(), to: 3 }).unwrap();
        assert_eq!(run(&mut s, Level::Control, &Command::Discard { tab: tab() }).unwrap_err().code, "would_discard_user_edits");
        assert_eq!(turned_pages(&s)[0], 1, "the user's edit is still there");
        // The cap isn't an agent erasing anything: all the agent's steps can come off, and undo isn't refused.
        while run(&mut s, Level::Control, &Command::Undo { tab: tab() }).is_ok() {}
        assert!(s.is_edited(TabId(1)), "it still differs from the file, and says so");
        let close = run(&mut s, Level::Control, &Command::Close { tab: tab() }).unwrap_err();
        assert!(close.next.contains("past the undo limit"), "what it can't do must not be offered: {}", close.next);
        let mut own = edit_session();
        for _ in 0..=recto_session::edit::MAX_UNDO {
            run(&mut own, Level::Control, &Command::PageMove { tab: tab(), pages: "2".into(), to: 3 }).unwrap();
        }
        assert!(own.page_edits(TabId(1)).unwrap().history_dropped());
        run(&mut own, Level::Control, &Command::Discard { tab: tab() }).expect("all of it is its own");
        assert!(run(&mut own, Level::Control, &Command::Close { tab: tab() }).is_ok());
    }

    /// Failure: the refusal mentions only undone steps, so an agent with unsaved edits runs `redo` as told and brings back a delete it had backed out of, thinking that clears the way. It must name all that's at stake and each honest way on, including that saving doesn't exist. Nothing here may loosen the rule.
    #[test]
    fn a_refused_close_says_all_it_would_lose_and_every_way_on() {
        let mut s = edit_session();
        let tab = || "#1".to_string();
        owners_rotation(&mut s, 0);
        owners_rotation(&mut s, 1);
        run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();

        let both = run(&mut s, Level::Control, &Command::Close { tab: tab() }).unwrap_err();
        assert_eq!((both.code, both.exit), ("would_clear_redo", exit::PERMISSION), "below Full, this is still that refusal");
        assert_eq!(both.ask.as_deref(), Some(ASK_FULL));
        assert!(both.message.contains("unsaved edits and undone steps") && both.message.contains("user's"), "{}", both.message);
        for must in ["recto redo", "anything you undid on purpose", "recto undo", "recto discard", "needs Full", "Saving isn't built", "Leave the tab open"] {
            assert!(both.next.contains(must), "{must}: {}", both.next);
        }

        let at_full = run(&mut s, Level::Full, &Command::Close { tab: tab() }).unwrap_err();
        assert_eq!((at_full.code, at_full.exit), ("unsaved_changes", exit::FAILED), "unsaved edits stop a close at any level");
        assert!(!at_full.next.contains("needs Full"), "{}", at_full.next);

        run(&mut s, Level::Control, &Command::Undo { tab: tab() }).unwrap();
        let only_undone = run(&mut s, Level::Control, &Command::Close { tab: tab() }).unwrap_err();
        assert_eq!(only_undone.code, "would_clear_redo");
        assert!(!only_undone.message.contains("unsaved"), "nothing's unsaved now: {}", only_undone.message);

        run(&mut s, Level::Control, &Command::Redo { tab: tab() }).unwrap();
        run(&mut s, Level::Control, &Command::Redo { tab: tab() }).unwrap();
        let only_unsaved = run(&mut s, Level::Control, &Command::Close { tab: tab() }).unwrap_err();
        assert_eq!(only_unsaved.code, "unsaved_changes");
        assert!(!only_unsaved.next.contains("recto redo"), "there's nothing to redo: {}", only_unsaved.next);
        assert_eq!(s.tabs().len(), 1, "none of these closed it");
    }

    /// Failure: `facts` says "encrypted" with nothing more, so a reader takes it the contents are protected, when anyone could open this file without a password.
    #[test]
    fn facts_say_when_an_encrypted_file_opened_with_no_password() {
        #[derive(Clone)]
        struct Encrypted;
        impl DocLoader for Encrypted {
            fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
                Ok(DocFacts { page_count: 1, byte_len: 1, encrypted: true, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false })
            }
        }
        let mut s = Session::new(Encrypted);
        s.open(&PathBuf::from("/d/locked.pdf")).unwrap();
        let facts = run(&mut s, Level::ReadOnly, &Command::Facts { tab: "active".into() }).unwrap();
        assert!(facts["encryption_note"].as_str().unwrap().contains("no password"), "{facts}");
        assert!(human::render("facts", &facts).contains("no password"), "{facts}");
        let mut plain = with_tabs(&["a"]);
        assert!(run(&mut plain, Level::ReadOnly, &Command::Facts { tab: "active".into() }).unwrap()["encryption_note"].is_null());
    }

    /// Failure: the log says "by name", so nobody can tell afterwards which tab an agent acted on. It keeps the `#id`, never the name, and `open` gives its own.
    #[test]
    fn the_log_says_which_tab_by_id_and_never_by_name() {
        let mut s = with_tabs(&["secret report", "other"]);
        let by_name = Command::Facts { tab: "secret".into() };
        let acted = acted_tab(&s, &by_name);
        assert_eq!(acted, Some(TabId(1)));
        let details = with_acted_tab(by_name.log_details(), acted, &Ok(json!({})), &by_name);
        assert_eq!(details["tab"], "#1");
        assert!(!details.to_string().contains("secret"), "{details}");
        let active = Command::Text { tab: "active".into(), page: 1 };
        assert_eq!(acted_tab(&s, &active), s.active());
        let nothing = Command::Facts { tab: "nothing like it".into() };
        let details = with_acted_tab(nothing.log_details(), acted_tab(&s, &nothing), &Ok(json!({})), &nothing);
        assert_eq!(details["tab"], "by name", "{details}");
        assert!(!details.to_string().contains("nothing like it"), "{details}");
        let mut alike = with_tabs(&["report one", "report two"]);
        let ambiguous = Command::Facts { tab: "report".into() };
        let acted = acted_tab(&alike, &ambiguous);
        assert_eq!(acted, None);
        let refused = run(&mut alike, Level::ReadOnly, &ambiguous);
        assert_eq!(with_acted_tab(ambiguous.log_details(), acted, &refused, &ambiguous)["tab"], "by name", "two tabs matched, and saying there's no such tab would be false");
        let open = Command::Open { path: "/d/new.pdf".into(), page: None };
        let opened = run(&mut s, Level::Control, &open);
        assert_eq!(with_acted_tab(open.log_details(), None, &opened, &open)["tab"], "#3");
    }

    /// Failure: a missing renderer reads as a vague install problem, so an agent can't say what to fix. The file's name and how to get it must be there.
    #[test]
    fn a_missing_renderer_names_what_is_missing() {
        let worker = AgentError::from(recto_render_service::RenderFailure::WorkerMissing);
        assert!(worker.message.contains("recto-render-service"), "{}", worker.message);
        assert!(worker.next.contains("cargo build --workspace"), "{}", worker.next);
        let library = AgentError::from(recto_render_service::RenderFailure::LibraryMissing);
        assert!(library.message.contains("libpdfium.dylib"), "{}", library.message);
        assert!(library.next.contains("tools/pdfium.sh"), "{}", library.next);
    }

    /// Failure: `4-x-` reads as `4-`, or a limit of none is quietly answered with one.
    #[test]
    fn pages_and_limits_mean_what_they_say() {
        assert_eq!(parse_pages("4-", true).unwrap().0, 3);
        assert_eq!(parse_pages("4-6", true).unwrap(), (3, 3));
        for bad in ["4-x-", "4--", "-4", "-", "", "x", "4-x"] {
            assert!(parse_pages(bad, true).is_err(), "{bad:?}");
        }
        assert!(parse_pages("4-", false).is_err());
        assert_eq!(positive_limit(Some(0), 20, 200).unwrap_err().code, "bad_limit");
        assert_eq!(positive_limit(Some(999), 20, 200).unwrap(), 200);
        assert_eq!(positive_limit(None, 20, 200).unwrap(), 20);
    }

    /// Failure: an agent goes to a page that isn't there, the session clamps it, and it believes it's on that page.
    #[test]
    fn going_past_the_end_is_refused() {
        let mut s = edit_session();
        let error = run(&mut s, Level::Control, &Command::Goto { tab: "#1".into(), page: 11 }).unwrap_err();
        assert_eq!((error.code, error.exit), ("no_such_page", exit::NOT_FOUND));
        assert_eq!(run(&mut s, Level::Control, &Command::Goto { tab: "#1".into(), page: 0 }).unwrap_err().code, "no_such_page");
        assert_eq!(run(&mut s, Level::Control, &Command::Goto { tab: "#1".into(), page: 10 }).unwrap()["page"], 10);
    }

    /// Failure: `active` with nothing open reads as an internal error, rather than saying to open something.
    #[test]
    fn no_active_tab_says_to_open_one() {
        let mut s = Session::new(Fake);
        let error = run(&mut s, Level::ReadOnly, &Command::Where { tab: "active".into() }).unwrap_err();
        assert_eq!((error.code, error.exit), ("no_such_tab", exit::NOT_FOUND));
        assert!(error.next.contains("recto open"), "{}", error.next);
    }

    /// Failure: every tab named the same floods an ambiguous error with a thousand candidates.
    #[test]
    fn candidates_are_capped() {
        let names: Vec<String> = (0..100).map(|i| format!("same {i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut s = with_tabs(&names);
        let error = run(&mut s, Level::Control, &Command::Tap { tab: "same".into() }).unwrap_err();
        assert_eq!(error.code, "ambiguous");
        assert!(error.candidates.len() <= MAX_CANDIDATES);
    }

    /// Failure: the log gets what an agent typed or a search's words, which can hold document text, or a file name with hidden characters in it.
    #[test]
    fn log_details_hold_no_text_and_clean_file_names() {
        // Failure: the log can't tell asking for generated contents, which starts a helper, from reading the file's own.
        assert_eq!(Command::Contents { tab: "#1".into(), from: None, limit: None, depth: None, generated: true }.log_details()["generated"], true);
        let hostile = format!("/d/a{}\nb.pdf", char::from_u32(0xE0041).unwrap());
        let open = Command::Open { path: hostile.into(), page: Some(3) }.log_details();
        assert_eq!(open["untrusted"]["file"], "a b.pdf");
        assert_eq!(open["page"], 3);
        let search = Command::Search { tab: "secret report".into(), query: "the secret words".into(), pages: None, limit: None, scan: false, regex: false, rank: false, context: None }.log_details().to_string();
        assert!(!search.contains("secret"), "{search}");
        assert_eq!(Command::Text { tab: "#4".into(), page: 2 }.log_details()["tab"], "#4");
        assert_eq!(Command::PageDelete { tab: "active".into(), pages: "ignore all".into() }.log_details()["pages"], "?");
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
