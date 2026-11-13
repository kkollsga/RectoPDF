//! The one control API. The UI, the CLI, QA and agents all go through these typed, versioned commands, and they call the
//! same session methods the UI calls.
//!
//! This crate holds no state of its own. A caller brings a session through [`SessionAccess`], and [`execute`] takes the session only for as long as each step needs it: reading a file or drawing
//! a page happens outside, so one slow PDF never stalls another client. [`table`] is the one list of commands, and the CLI grammar, `recto help`, `SKILL.md` and the level checks all come from it.

pub mod actor;
pub mod close_rule;
pub use actor::Actor;
pub mod agreement;
pub mod effect;
pub mod later;
pub use effect::Effect;
pub mod commands;
pub mod event;
#[cfg(test)]
mod event_agreement;
pub use event::{Event, edits_changed, page_changed, page_now, view_changed};
pub mod extract;
pub mod find;
pub mod generated;
pub mod human;
pub mod index;
pub mod linking;
mod loader;
pub mod print;
pub mod quiet_files;
mod reading;
mod render;
pub mod saving;
pub use render::{output_exists, publish_picture, svg_in_memory};
mod command;
mod route;
mod search;
mod selecting;
pub mod skill;
pub mod table;
pub mod unfit;
pub use commands::updates::update_needs_app;
pub use route::{Runs, qa_needs_app};
#[cfg(test)]
mod registry_golden;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, serde::Serialize)]
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

    /// What this level allows, in words, in the one place: `recto permission show`, the skill's reference and `PRIVACY.md` all say this. Held to the command table by `each_level_says_what_its_commands_do`.
    pub fn allows(self) -> &'static str {
        match self {
            Self::Off => {
                "Status, help, and asking for more. It can see how many PDFs are open, and nothing from any of them: no names, no page text and no drawing."
            }
            Self::ReadOnly => "Read and search documents that are open, and watch what happens. Changes no document.",
            Self::Control => {
                "Open and close PDFs, move around, edit pages in memory, save edits by adding them to the end of a file or as a new file, check for updates, and throw away agents' own edits. Never the user's edits."
            }
            Self::Full => {
                "Everything Control can, and write exports and pictures to disk, save over another file or rewrite one, throw away the user's edits too, install updates, change the update settings, and choose a worker's limits. Signing is never included."
            }
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

pub(crate) fn active_tab() -> String {
    "active".into()
}

pub(crate) fn default_wait_ms() -> u64 {
    DEFAULT_WAIT_MS
}

/// How long `wait` waits if not told.
pub const DEFAULT_WAIT_MS: u64 = 30_000;
/// The longest any wait or watch may run, so a forgotten one can't stay for ever.
pub const MAX_WAIT_MS: u64 = 24 * 60 * 60 * 1000;
/// How long `watch` runs if not told.
pub const DEFAULT_WATCH_MS: u64 = 10 * 60 * 1000;

/// What `wait` can wait for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, serde::Serialize)]
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
    // The families: each reads its own commands, by `parse`.
    #[serde(skip)]
    Start(commands::Start),
    #[serde(skip)]
    Waits(commands::Waits),
    #[serde(skip)]
    Rights(commands::Rights),
    #[serde(skip)]
    Session(commands::Session),
    #[serde(skip)]
    Measure(commands::Measure),
    #[serde(skip)]
    Edit(commands::Edit),
    #[serde(skip)]
    Move(commands::Move),
    #[serde(skip)]
    Read(commands::Read),
    #[serde(skip)]
    Qa(commands::Qa),
    #[serde(skip)]
    Record(commands::Record),
    #[serde(skip)]
    Updates(commands::Updates),
}

/// Which tab a command is about to act on, as an id for the log. Never a name, and none if it means no tab or several. Asked before the command runs, because `close` leaves nothing to ask afterwards.
pub fn acted_tab<L: DocLoader>(session: &Session<L>, command: &Command) -> Option<TabId> {
    select(session, command.tab_selector()?).ok()
}

/// Puts the tab a command really acted on in its log entry. `open` learns its tab from its answer.
pub fn with_acted_tab(mut details: Value, acted: Option<TabId>, result: &Result<Value, AgentError>, command: &Command) -> Value {
    let id = match (command, result) {
        (Command::Move(commands::Move::Open(_)), Ok(data)) => data["tab"].as_u64(),
        _ => acted.map(|id| u64::from(id.0)),
    };
    if let Some(id) = id {
        details["tab"] = json!(format!("#{id}"));
    }
    details
}

/// Keeps a page range in the log only if it's what a range should be.
pub(crate) fn clean_range(text: &str) -> &str {
    if text.len() <= 24 && text.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        text
    } else {
        "?"
    }
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
        Self {
            code,
            message: message.into(),
            candidates: Vec::new(),
            exit,
            next: next_step(code, exit).to_string(),
            ask: None,
            untrusted: None,
        }
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
        "protected" => "Tell the user. Only they can allow an agent to read a protected document, and this version can't ask them, so work on another file.",
        "ambiguous" => "Use one of the candidates, written as `#id`.",
        "unsaved_changes" => {
            "`recto discard` throws them away, then `recto close` or `recto stop` can go on. It needs Full access only if some are the user's. `recto save` puts them in the file instead, if the user wants that."
        }
        "last_page" => "Keep at least one page, or delete fewer.",
        "too_many_pages" | "too_many_sources" => "Add less to this document.",
        "too_many_tabs" => "`recto close` closes a tab.",
        "renderer_only" => "This file can still be read and drawn. To change it, it first needs repairing in another tool.",
        "source_changed" => {
            "Tell the user this file changed. `recto edits` shows what's unsaved. `recto discard` throws the edits away, which needs Full access if some are the user's, and opening the file again starts from what's there now."
        }
        "nothing_to_undo" | "nothing_to_redo" => "`recto edits` shows where this tab is.",
        "not_built" => "Tell the user this isn't in this version yet.",
        "too_large_at_this_scale" => "Try a smaller `--scale`. 2 is twice the page's size, and 1 is its own.",
        "page_too_large_to_read" => "Tell the user this page can't be read. The rest of the document may be fine.",
        "render_worker_missing" => {
            "Tell the user RectoPDF's install is incomplete: `recto-render-service` must be in the app's `Contents/Helpers/RectoRender.app`, or in a source tree in the same folder as `recto`. There, `cargo build --workspace` builds both, which `cargo build -p recto-cli` doesn't."
        }
        "renderer_not_sandboxed" => {
            "Tell the user this copy of RectoPDF is signed wrong, so its renderer refuses to read any PDF. Installing a release again fixes it. In a source tree, `app/build.sh` signs the helper as it should be."
        }
        "renderer_inside_another_sandbox" => {
            "Run `recto` outside the sandbox it's in, or ask the user to: the renderer can't start inside another sandbox. Nothing needs reinstalling."
        }
        "renderer_mach_drop_failed" | "renderer_launchd_reachable" => {
            "Tell the user RectoPDF's renderer can't close itself off on this version of macOS, so it won't read PDFs here, and give them this code and their macOS version to report. Nothing needs reinstalling."
        }
        "renderer_start_refused" => {
            "Tell the user macOS refused to start RectoPDF's renderer. A security tool, or a sandbox `recto` runs in, may be blocking it."
        }
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
            SessionError::NoSuchTab | SessionError::Open(recto_core::OpenError::NotFound) | SessionError::Edit(recto_session::edit::EditError::OutOfRange) => {
                exit::NOT_FOUND
            }
            SessionError::Open(recto_core::OpenError::PasswordNeeded) => exit::PROTECTED,
            SessionError::OutsideQuietRun => exit::PERMISSION,
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
    // The tag first, then the line as the family that reads it. A tag no family has is a command not moved into one yet, or none.
    match commands::tag_of(line).as_deref().and_then(commands::parser_for) {
        Some(family) => family(line),
        None => Err(bad_command()),
    }
}

/// The refusal of a line that isn't a command: no tag a family has, or a line serde can't read as its command.
pub(crate) fn bad_command() -> AgentError {
    AgentError::new("bad_command", "That isn't a command RectoPDF knows.", exit::FAILED)
}

/// The refusal for a level that's too low: what's missing, and the one command that asks for it.
pub fn refusal(command_name: &str, have: Level, needed: Access) -> AgentError {
    match needed {
        Access::At(level) => {
            let mut error = AgentError::new(
                "permission_denied",
                format!("`{command_name}` needs {} access, and agent access is {} now.", level.label(), have.label()),
                exit::PERMISSION,
            );
            let ask = format!("recto permission request {}", level.name());
            error.next = format!("Run `{ask}`, which tells the user what to allow. Nothing else raises this.");
            error.ask = Some(ask.into());
            error
        }
        Access::UserOnly => AgentError::new(
            "needs_terminal",
            format!("`{command_name}` is for the user, at their own terminal."),
            exit::PERMISSION,
        ),
        Access::Nobody => AgentError::new(
            "sign_refused",
            "Only the user can sign a document, in the app. No agent command signs, at any level.",
            exit::PERMISSION,
        ),
    }
}

/// Whether a level may run a command.
pub fn allowed(have: Level, needed: Access) -> bool {
    matches!(needed, Access::At(level) if have >= level)
}

/// A way to reach the session. In one process that's a plain reference; on the socket it's a lock taken only as long as each step needs.
pub trait SessionAccess {
    type Loader: DocLoader + Clone;
    fn with<R>(&self, step: impl FnOnce(&mut Session<Self::Loader>) -> R) -> R;
    /// How documents get indexed here, if they do.
    fn indexing(&self) -> Option<&dyn index::Indexing> {
        None
    }
    /// What a save needs of the host beyond the session, if anything.
    fn saving(&self) -> Option<&dyn saving::SaveSupport> {
        None
    }
    /// How generated contents get made here, if they do.
    fn generating(&self) -> Option<&dyn generated::Generating> {
        None
    }
    /// How Find runs in the background here, if it does. Without it, `recto find` finds in the command itself.
    fn finding(&self) -> Option<&dyn find::Finding> {
        None
    }
    /// The state record this host keeps, for `session show`, `sleep`, `save`, `restore` and `check`. A session in one process has none: they need the long-running one.
    fn records(&self) -> Result<&dyn commands::record::Records, AgentError> {
        Err(commands::needs_server())
    }
    /// What the window's to do after this command, if there's a window: see [`Effect`]. A session with no window has nothing to show.
    fn effect(&self, _effect: Effect) {}
    /// **The person's passwords, for their own Find** in a document they opened with one ([`find::FindPlan::with_keys`]): only for an actor who may see
    /// protected documents ([`Actor::sees_protected`]), so an agent's `find` never gets them. A session with no window has none.
    fn find_keys(&self, _actor: Actor) -> Option<find::KeyLookup> {
        None
    }
    /// How this host reads what a command needs, if it mustn't wait on a disk: see [`later`]. Every host but the window's session thread reads in place.
    fn reads_later(&self) -> Option<&dyn later::ReadLater> {
        None
    }
    /// **Whether anyone reads the answer,** or only what the command does: not the window's own commands for the person, which it draws from what the
    /// session publishes. A command whose answer is costly to build makes none when it isn't read: a tap's is the sidebar's every row, 650 µs at 1,000 tabs,
    /// five times the tap (R2.3). Every other host's answers are read.
    fn answer_read(&self) -> bool {
        true
    }
    /// **Takes in where the window's page views are now,** before a jump or a step in the trail marks where it leaves: what the reader has scrolled to that
    /// the session hasn't heard yet, so Back comes back to the place that was showing (R2.3, accident A1). Called without the session held, as it takes it
    /// itself. A session with no window has nothing to take in.
    fn take_places(&self) {}
    /// **The folders this session is confined to, if it is** ([`quiet_files`]): set when the session was made and never changed, so a host that keeps
    /// the session's [`recto_session::confined::Confinement::shared`] answers without the session's lock, and a free session's command pays only a branch for
    /// it. Asked of the session here otherwise.
    fn confinement(&self) -> Option<std::sync::Arc<recto_platform::run_folders::RunFolders>> {
        self.with(|session| session.confinement().folders().cloned())
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
pub fn run_indexed<L: DocLoader + Clone>(
    session: &mut Session<L>,
    level: Level,
    command: &Command,
    indexing: Option<&dyn index::Indexing>,
) -> Result<Value, AgentError> {
    execute(&Direct(RefCell::new(session), indexing), Actor::Agent(level), command, &mut Vec::new())
}

/// What's chosen, in a word: `auto` goes with the window.
pub fn paper_name(view: recto_session::ViewSettings) -> &'static str {
    view.paper.map_or("auto", recto_session::Paper::name)
}

/// What's said about a protected document wherever an agent would otherwise get something read from it.
pub const PROTECTED_NOTE: &str = "The user opened this PDF with its password, so nothing read from it is given to an agent: not its text, contents, pages, length or facts. Only they can allow that.";

/// The refusal for anything that would read a protected document the person opened with its password. Exit 8, as a file that needs a password is.
pub fn protected_refusal() -> AgentError {
    AgentError::new("protected", PROTECTED_NOTE, exit::PROTECTED)
}

/// `recto stop` against the owner's own app. Only they quit it: the default, by the coordinator's decision of 2026-11-07, until the owner says otherwise.
pub fn only_the_user_quits() -> AgentError {
    AgentError::new(
        "only_the_user_quits",
        "This session is the app someone is using, and only they can quit it.",
        exit::FAILED,
    )
    .then("Ask them to, if it needs to stop. Everything else works with it running.")
}

/// `session restore` would lose the owner's edits, which is theirs to say below Full: SPEC decision 20.
pub fn restore_would_lose_owners_edits() -> AgentError {
    AgentError::new(
        "owner_edits_at_stake",
        "Restoring would replace this session, which holds the user's own unsaved edits, so it wasn't done.",
        exit::PERMISSION,
    )
    .then("Ask them to save or discard those first.")
}

/// What `session show` says, from the record and the session. No more of a document than `tabs` gives, and what the owner typed in Find only at Control, as it's not about any document. Called with the session held.
pub fn session_show_json<L: DocLoader>(session: &Session<L>, level: Level, kept: Value) -> Value {
    let tabs: Vec<Value> = session
        .tabs()
        .iter()
        .take(MAX_TABS_LISTED as usize)
        .map(|tab| {
            let notice = session.notices().said(tab.id).map(|said| json!({ "code": said.code, "message": said.message }));
            let place = session.tabs().place_to_show(tab.id);
            // A protected document's place and length were read with its password.
            if session.tabs().withheld_from_agents(tab.id) {
                return json!({ "tab": tab.id.0, "protected": true, "page": null, "pages": null, "zoom": place.map(|place| place.zoom), "within_page": null, "layout": tab.spread.name(), "edited": session.lists().is_edited(tab.id), "can_undo": session.lists().undo_and_redo(tab.id).0, "can_redo": session.lists().undo_and_redo(tab.id).1, "notice": notice });
            }
            json!({ "tab": tab.id.0, "page": tab.page + 1, "pages": session.pages().count(tab.id), "zoom": place.map(|place| place.zoom), "within_page": place.filter(|place| place.anchor_page == place.page).map(|place| place.within), "layout": tab.spread.name(), "edited": session.lists().is_edited(tab.id), "can_undo": session.lists().undo_and_redo(tab.id).0, "can_redo": session.lists().undo_and_redo(tab.id).1, "notice": notice })
        })
        .collect();
    let find = session.viewing().find();
    let mut find_json = json!({ "open": find.open, "has_text": !find.query.is_empty() || !find.draft.is_empty() });
    if level >= Level::Control {
        find_json["query"] = json!(find.query);
        find_json["draft"] = json!(find.draft);
    }
    json!({ "record": kept, "count": session.tabs().len(), "active": session.tabs().active().map(|id| id.0), "tabs": tabs, "find": find_json, "edit_mode": session.viewing().edit_mode(), "paper": paper_name(session.viewing().view()), "sidebar": !session.viewing().view().sidebar_hidden })
}

/// **The one executor:** runs one command for whoever asked, `actor`, noting what happened in `events`. The person's actions at the window and an agent's commands both come here; what tells them apart is [`Actor`]'s. Commands that only a long-running session can answer say so.
pub fn execute<A: SessionAccess>(access: &A, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let needed = command.access();
    if !actor.may(needed) {
        return Err(refusal(table::display_name(command.tag()), actor.level(), needed));
    }
    // A quiet run's session uses only files in its own folders, whoever asks and at whatever level: every file this command names, before it runs.
    quiet_files::check(access, command)?;
    // The record's commands read nothing of a document, and `session sleep`'s tab mustn't be held awake: each host's record answers them.
    if command.route() == Runs::Record {
        return execute_held(access, actor, command, events);
    }
    // A protected document the person opened with its password gives an agent nothing read from it: agent report §5.5. Closing it, tapping its row or asking where it is reads nothing of it, and what they answer leaves it out and says why. Asked before the tab is touched, woken or held.
    // Find has no tab of its own: it's in the document being read, so that's the one asked about.
    let about = command
        .tab_selector()
        .or_else(|| matches!(command, Command::Move(commands::Move::Find(_))).then_some("active"));
    if let Some(selector) = about
        && !command.protected_ok()
        && !actor.sees_protected()
        && access.with(|s| select(s, selector).is_ok_and(|id| s.tabs().withheld_from_agents(id)))
    {
        return Err(protected_refusal());
    }
    // A command about a tab is someone working on it, so it doesn't go dormant from under them, and what it reads is there to read. Every route's through here.
    let held = command.tab_selector().and_then(|selector| {
        access.with(|s| {
            let id = select(s, selector).ok()?;
            // Its edits' list, if it's asleep, is read back from its journal: a bounded private file in the app's own state folder, never the PDF or its
            // volume, so it's read here, on whatever thread runs this, the window's session thread included, as that thread's own taps, opens and edits
            // always have (coordinator, 2026-11-13).
            let awake = s.wake_list(id);
            s.touch(id);
            s.dormancy_mut().hold(id);
            Some((id, awake))
        })
    });
    // Its edits say what's on its pages, and they can't be read just now: anything that reads a page says so, rather than that there's no such page (review).
    let result = match held {
        Some((_, false)) if command.reads_pages() => Err(AgentError::new(
            recto_session::SessionError::EditsUnreadable.code(),
            recto_session::SessionError::EditsUnreadable.message(),
            exit::FAILED,
        )
        .then("`recto discard` gives up on them, and `recto session show` says what's known of them.")),
        _ => execute_held(access, actor, command, events),
    };
    let held = held.map(|(id, _)| id);
    if let Some(id) = held {
        access.with(|s| s.let_go(id));
    }
    result
}

fn execute_held<A: SessionAccess>(access: &A, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let level = actor.level();
    match command {
        Command::Measure(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Edit(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Move(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Read(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Updates(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Start(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Waits(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Rights(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Session(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Qa(command) => command.run(&mut commands::Cx { access, actor, level, events }),
        Command::Record(command) => command.run(&mut commands::Cx { access, actor, level, events }),
    }
}

/// What a page's draw teaches the session and anyone waiting.
fn note_draw<A: SessionAccess>(access: &A, tab: TabId, position: u32, failure: Option<&'static str>, events: &mut Vec<Event>) {
    access.with(|s| s.mail_mut().note_drawn(DrawNote { tab, page: position, failure }));
    let page = position + 1;
    events.push(match failure {
        None => Event::PageDrawn { tab, page },
        Some(code) => Event::PageFailed { tab, page, code },
    });
}

/// A refused file name says nothing about whether a page can be drawn, so a wait on that page mustn't hear of it.
fn is_about_the_output(failure: Option<&str>) -> bool {
    matches!(
        failure,
        Some("output_exists" | "bad_output" | "bad_scale" | "bad_rect" | "bad_text_mode" | "write_failed" | "output_too_large")
    )
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
pub(crate) fn file_label(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |name| recto_core::clean_line(&name.to_string_lossy()))
}

/// The refusal when a file has changed under the page numbers that were recorded from it.
fn source_changed(path: &Path) -> AgentError {
    let mut error = AgentError::new(
        "source_changed",
        "The file this page comes from has changed or gone since its page numbers were recorded, so the page can't be trusted to be the same one.",
        exit::FAILED,
    );
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
    if path.is_absolute() {
        Ok(())
    } else {
        Err(AgentError::new(
            "bad_path",
            "That path isn't whole. A session has no folder of its own to start from.",
            exit::FAILED,
        ))
    }
}

/// What's side by side: each pane's tab, its page and length, the one being read, synced or separate, and what's said about lengths.
pub fn compare_json<L: DocLoader>(session: &Session<L>) -> Value {
    let comparison = session.comparison();
    let (tabs, pages) = (session.tabs(), session.pages());
    let active = tabs.active();
    let pane = |id: TabId| {
        let tab = tabs.tab(id)?;
        let reading = active == Some(id);
        Some(if tabs.withheld_from_agents(id) {
            json!({ "tab": id.0, "protected": true, "page": null, "pages": null, "reading": reading, "untrusted": { "name": &*tab.name } })
        } else {
            json!({ "tab": id.0, "page": tab.page + 1, "pages": pages.count(id), "reading": reading, "untrusted": { "name": &*tab.name } })
        })
    };
    let panes: Vec<Value> = comparison
        .map(|comparison| comparison.tabs.iter().filter_map(|id| pane(*id)).collect())
        .unwrap_or_default();
    json!({
        "comparing": comparison.is_some(),
        "panes": panes,
        "sync": comparison.map_or("off", |comparison| if comparison.synced { "on" } else { "off" }),
        "sync_from": comparison.filter(|comparison| comparison.synced).map(|comparison| comparison.sync_from.iter().map(|page| page + 1).collect::<Vec<_>>()),
        "note": session.compare_note(),
        "max_panes": recto_session::compare::MAX_PANES,
    })
}

/// Reads a cold tab's file, if what's about to happen needs it awake, with the session free. A file on a dead volume holds up this command and no other. A host
/// that mustn't wait has it read off its thread and runs the command again: [`later`].
fn read_ahead<A: SessionAccess>(access: &A, plan: impl FnOnce(&mut Session<A::Loader>) -> Option<(TabId, PathBuf)>) -> Result<(), AgentError> {
    // On a host that reads later, what's held already was read for this command, which is running again now it's in.
    let reads_later = access.reads_later().is_some();
    let Some((id, path)) = access.with(|s| plan(s).filter(|(id, _)| !(reads_later && s.warm().holds_read_of(*id)))) else {
        return Ok(());
    };
    if let Some(reads) = access.reads_later() {
        reads.wake(id, path);
        return Err(later::later());
    }
    let loader = access.with(|s| s.loader().clone());
    let stamp = recto_platform::file_stamp(&path).ok();
    let loaded = loader.load(&path);
    access.with(|s| s.give_woken(id, loaded, stamp));
    Ok(())
}

/// [`read_ahead`] where it only spares the session's lock a read it would otherwise make, as for the tab a close shows next: a host that reads later lets the
/// session wake it as that host always has, and the command doesn't wait for it.
fn read_soon<A: SessionAccess>(access: &A, plan: impl FnOnce(&mut Session<A::Loader>) -> Option<(TabId, PathBuf)>) {
    if access.reads_later().is_none() {
        let _ = read_ahead(access, plan);
    }
}

/// Goes to a one-based page. A page that isn't there is refused here rather than clamped, so an agent can't believe it's somewhere it isn't.
/// `recto goto`: a jump, so Back comes here again, as following a link does.
fn go_to<L: DocLoader>(session: &mut Session<L>, id: TabId, page: u32, within: f64, by: Author) -> Result<(), AgentError> {
    let position = page
        .checked_sub(1)
        .filter(|position| *position < session.pages().count(id))
        .ok_or_else(no_such_page)?;
    // An agent's mark gives way to the person's in the trail. How far down is clamped to the page there, as a link's is.
    session.jump_as(id, position, within, by)?;
    Ok(())
}

fn one_based(page: u32) -> Result<u32, AgentError> {
    page.checked_sub(1)
        .ok_or_else(|| AgentError::new("no_such_page", "Pages are numbered from 1.", exit::NOT_FOUND))
}

/// How many a command should give: what was asked, up to the most it gives, or its default. Asking for none is refused: it can't be what was meant, and answering with one would be a guess.
pub(crate) fn positive_limit(limit: Option<u32>, default: u32, max: u32) -> Result<usize, AgentError> {
    match limit {
        Some(0) => {
            Err(AgentError::new("bad_limit", "A limit has to be at least 1.", exit::FAILED).then("Leave it out to get the default, or ask for 1 or more."))
        }
        Some(limit) => Ok(limit.min(max) as usize),
        None => Ok(default.min(max) as usize),
    }
}

/// Parses `4` or `4-6` into a zero-based start and a length. `4-` means to the end only where `open_ended` says that makes sense, and only that exact form: `4-x-` isn't it.
pub(crate) fn parse_pages(pages: &str, open_ended: bool) -> Result<(u32, u32), AgentError> {
    let bad =
        || AgentError::new("bad_pages", "Pages are one number or a range, such as 4 or 4-6.", exit::FAILED).then("Write them as 4 or 4-6, counting from 1.");
    let (first, last) = pages.split_once('-').unwrap_or((pages, pages));
    let number = |text: &str| {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            Err(bad())
        } else {
            text.parse::<u32>().map_err(|_| bad())
        }
    };
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
        return Err(AgentError::new(
            "bad_rotation",
            "A rotation is a quarter turn or more, such as 90, 180, 270 or -90 degrees.",
            exit::FAILED,
        )
        .then("Use 90, 180, 270 or -90."));
    }
    Ok(degrees / 90)
}

/// `page 3` or `pages 4-6`, as it was asked for, which `parse_pages` has already held to digits, with a verb that agrees if one's wanted.
fn which_pages(pages: &str, one: &str, many: &str) -> String {
    let (noun, verb) = if pages.contains('-') { ("Pages", many) } else { ("Page", one) };
    let noun = if verb.is_empty() { noun.to_lowercase() } else { noun.to_string() };
    if verb.is_empty() {
        format!("{noun} {pages}")
    } else {
        format!("{noun} {pages} {verb}")
    }
}

fn edit<A: SessionAccess>(
    access: &A,
    actor: Actor,
    tab: &str,
    events: &mut Vec<Event>,
    did: String,
    change: impl FnOnce(&mut recto_session::PartMut<'_, recto_session::edit::PageEdits>) -> Result<(), recto_session::edit::EditError>,
) -> Result<Value, AgentError> {
    access.with(|s| {
        let id = select(s, tab)?;
        keep_owners_undone_steps(s, id, actor.level())?;
        note_agent_at_full(s, actor);
        s.edit_pages_as(id, actor.author(), change)?;
        events.push(edits_changed(s, id));
        let mut data = edit_state_json(s, id);
        data["did"] = json!(did);
        Ok(with_place_note(s, id, data))
    })
}

/// An agent's edit, undo or redo says whether it's at Full, which may push past the owner's steps. The person's edits are the owner's, and say nothing of it.
fn note_agent_at_full<L: DocLoader>(session: &mut Session<L>, actor: Actor) {
    if let Actor::Agent(level) = actor {
        session.lists_mut().set_agent_at_full(level >= Level::Full);
    }
}

/// What asking for Full is for, where only that would do. The one command an agent can run to get there.
pub(crate) const ASK_FULL: &str = "recto permission request full";

/// Undone steps can still be got back, and a new edit clears them for good, which is how an agent could erase the user's edits: undo them all, make one edit. So below Full, an edit that would lose an undone step of the user's is refused. Undone steps of agents' own are theirs to lose. SPEC decision log, 2026-11-07: below Full, an agent can throw away only agents' edits, never the owner's.
fn keep_owners_undone_steps<L: DocLoader>(session: &Session<L>, id: TabId, level: Level) -> Result<(), AgentError> {
    if level >= Level::Full || !session.lists().has_undone_owner_steps(id) {
        return Ok(());
    }
    let mut error = AgentError::new(
        "would_clear_redo",
        "A new edit here would throw away the user's edits, undone but still there to redo, and that needs Full access.",
        exit::PERMISSION,
    );
    error.ask = Some(ASK_FULL.into());
    error.next =
        format!("Run `recto redo` until there's nothing left to redo, which keeps everything, and then go on. Or run `{ASK_FULL}` to ask for Full access.");
    Err(error)
}

/// **An agent throws away nothing it can't read** (coordinator, 2026-11-13: unknown is "would lose", not "nothing to lose"). Every command about a tab reads
/// its list back first, so one still asleep is one whose journal couldn't be read: its summary says some of what's there, not all of it. An agent's
/// `discard` or `clear-history` of it is refused at every level; the person, whose tab says so, decides.
fn refuse_unreadable<L: DocLoader>(session: &Session<L>, id: TabId, actor: Actor, what: &str) -> Result<(), AgentError> {
    if actor == Actor::Person || !session.lists().is_asleep(id) {
        return Ok(());
    }
    Err(AgentError::new(
        "edits_unreadable",
        format!("Nothing was {what}: this tab's edits couldn't be read back just now, so what that would throw away can't all be known."),
        exit::FAILED,
    )
    .then("Leave them: the tab tells the user its edits couldn't be read, and what to do with them is theirs. `recto session show` says what's known of them."))
}

/// `discard` throws away everything on the tab's list, undone or not, and whatever fell off its capped undo list too, since that's still in the page order. Below Full, none of that may be the user's.
fn refuse_discard_of_owners_edits<L: DocLoader>(session: &Session<L>, id: TabId, level: Level) -> Result<(), AgentError> {
    if level >= Level::Full || !session.lists().summary(id).is_some_and(|summary| summary.holds_owner_edits) {
        return Ok(());
    }
    let mut error = AgentError::new(
        "would_discard_user_edits",
        "Nothing was discarded: some of this tab's edits are the user's, and throwing those away needs Full access.",
        exit::PERMISSION,
    );
    error.ask = Some(ASK_FULL.into());
    error.next = format!("`recto undo` steps back through them, and keeps them. Run `{ASK_FULL}` to ask for Full access, or tell the user.");
    Err(error)
}

/// Says when the page being read went away, so the tab is on another one.
fn with_place_note<L: DocLoader>(session: &mut Session<L>, id: TabId, mut data: Value) -> Value {
    if session.mail_mut().take_place_lost()
        && let Some(tab) = session.tabs().tab(id)
    {
        data["page_was_removed"] = json!(true);
        data["place_note"] = json!(format!("The page this tab was on was removed, so it's now on page {}.", tab.page + 1));
    }
    data
}

/// The plain truth about unsaved edits, wherever a tab reports them.
pub const UNSAVED_NOTE: &str = "These edits aren't saved: the file on disk is unchanged until `recto save`. Save only when the user asks.";

/// What's true when an agent has undone the user's edits: the page matches the file, and they're not gone yet.
pub const USER_EDITS_UNDONE_NOTE: &str =
    "The user's edits on this tab are undone, not gone: `recto redo` brings them back, and closing would lose them. Nothing has been saved.";

fn edit_state_json<L: DocLoader>(session: &Session<L>, id: TabId) -> Value {
    let edited = session.lists().is_edited(id);
    let user_edits_undone = session.lists().has_undone_owner_steps(id);
    json!({
        "tab": id.0,
        "pages": session.pages().count(id),
        "file_pages": session.tabs().tab(id).map(|t| t.page_count),
        "page": session.tabs().tab(id).map(|t| t.page + 1),
        "edited": edited,
        // The user's undone edits aren't in the file, and closing would lose them, so nothing about this says "saved".
        "saved": !edited && !user_edits_undone,
        "user_edits_undone": user_edits_undone,
        "can_undo": session.lists().undo_and_redo(id).0,
        "can_redo": session.lists().undo_and_redo(id).1,
        "note": if edited { Some(UNSAVED_NOTE) } else { user_edits_undone.then_some(USER_EDITS_UNDONE_NOTE) },
    })
}

/// What `edits` calls pages from a copy the history keeps of the file as it was before a save.
pub const KEPT_LABEL: &str = "this file, as it was before a save";

/// Most runs `edits` lists, so a hostile edit history can't make an enormous answer.
const MAX_EDIT_RUNS: usize = 1000;

/// The edited page order, as runs of pages from one file with one rotation.
fn edits_json<L: DocLoader>(session: &Session<L>, id: TabId) -> Value {
    let mut data = edit_state_json(session, id);
    let Some(edits) = session.lists().get(id) else {
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
        while slots
            .peek()
            .is_some_and(|next| next.source == first.source && next.quarter_turns == first.quarter_turns && next.page == first.page + len)
        {
            slots.next();
            len += 1;
        }
        let file = match first.source {
            0 => None,
            // A copy the history keeps is in the app's own folder: no level sees where, only what it is.
            source => edits
                .added_sources()
                .get(usize::from(source) - 1)
                .map(|added| if added.kept { KEPT_LABEL.to_string() } else { file_label(&added.path) }),
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
    session
        .tabs()
        .path_of(id)
        .ok_or_else(|| AgentError::new("no_such_tab", "That tab isn't open.", exit::NOT_FOUND))
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
        return session
            .tabs()
            .active()
            .ok_or_else(|| AgentError::new("no_such_tab", "No PDF is open.", exit::NOT_FOUND).then("Open one with `recto open <file.pdf>`."));
    }
    if let Some(number) = selector.strip_prefix('#') {
        let id = TabId(number.parse().map_err(|_| not_found())?);
        return session.tabs().tab(id).map(|_| id).ok_or_else(not_found);
    }
    let needle = selector.to_lowercase();
    let matches: Vec<_> = session
        .tabs()
        .iter()
        .filter(|t| t.name.to_lowercase().contains(&needle))
        .take(MAX_CANDIDATES + 1)
        .collect();
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
    // Whether a protected document's page is drawn, indexed or has contents is something read from it. Only its being open or closed is said.
    if !matches!(condition, Condition::TabOpen | Condition::TabClosed) && access.with(|s| select(s, tab).is_ok_and(|id| s.tabs().withheld_from_agents(id))) {
        return ConditionState::Failed(protected_refusal());
    }
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
                return ConditionState::Failed(AgentError::new(
                    "no_such_page",
                    "`wait page-drawn` needs `--page`, counting from 1.",
                    exit::NOT_FOUND,
                ));
            };
            match select(s, tab) {
                Err(error) if error.exit == exit::AMBIGUOUS => ConditionState::Failed(error),
                Err(_) => ConditionState::Waiting,
                Ok(id) => match s.mail().draw_note(id, page - 1) {
                    None => ConditionState::Waiting,
                    Some(DrawNote { failure: None, .. }) => ConditionState::Met(json!({ "tab": id.0, "page": page, "drawn": true })),
                    Some(DrawNote { failure: Some(code), .. }) => ConditionState::Failed(
                        AgentError::new("page_failed", format!("Page {page} couldn't be drawn ({code})."), exit::FAILED)
                            .then("Try another page, or `recto text --page N` for its text."),
                    ),
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

/// Files opened lately, newest first. A name comes from outside, so it's under `untrusted`. The command needs Control, the coordinator's decision (review O7): names of files someone's closed say what they've been reading. Paths are held to Control here too, whatever the row says.
fn recent_json<L: DocLoader>(session: &Session<L>, level: Level) -> Value {
    let files: Vec<Value> = session
        .recent_files()
        .iter()
        .enumerate()
        .map(|(at, file)| {
            let mut out =
                json!({ "file": at + 1, "open_tab": file.open.map(|id| format!("#{}", id.0)), "untrusted": { "name": recto_core::clean_line(&file.name) } });
            if level >= Level::Control {
                out["untrusted"]["path"] = json!(file.path.to_string_lossy());
            }
            out
        })
        .collect();
    json!({ "count": files.len(), "files": files, "paths_shown": level >= Level::Control })
}

/// Puts each hit's page label on it, under `untrusted` as it's the file's words, where the file calls the page something other than its number, as Find shows it.
fn label_hits<A: SessionAccess>(access: &A, tab: &str, answer: &mut Value) {
    access.with(|s| {
        let Ok(id) = select(s, tab) else { return };
        if let Some(hits) = answer.get_mut("hits").and_then(Value::as_array_mut) {
            for hit in hits {
                let label = hit["page"]
                    .as_u64()
                    .and_then(|page| u32::try_from(page).ok())
                    .and_then(|page| page.checked_sub(1))
                    .and_then(|position| s.pages().label_differs(id, position));
                if let Some(label) = label {
                    hit["untrusted"]["label"] = json!(label);
                }
            }
        }
    });
}

fn where_json<L: DocLoader>(session: &Session<L>, id: TabId) -> Value {
    let (tabs, lists, pages) = (session.tabs(), session.lists(), session.pages());
    let Some(tab) = tabs.tab(id) else { return Value::Null };
    let edited = lists.is_edited(id);
    let active = tabs.active() == Some(id);
    // Its file's name is the person's, and not read from the file. Its place, length and the rest were read with its password.
    if tabs.withheld_from_agents(id) {
        return json!({ "tab": id.0, "sel": format!("#{}", id.0), "protected": true, "protected_note": PROTECTED_NOTE, "page": null, "pages": null, "file_pages": null, "active": active, "edited": edited, "untrusted": { "name": &*tab.name } });
    }
    let label = pages.label_differs(id, tab.page);
    let (can_go_back, can_go_forward) = tabs.can_go_back_and_forward(id);
    let undone = lists.has_undone_owner_steps(id);
    json!({
        "tab": id.0,
        "sel": format!("#{}", id.0),
        "page": tab.page + 1,
        "has_label": label.is_some(),
        "pages": pages.count(id),
        "file_pages": tab.page_count,
        "can_go_back": can_go_back,
        "can_go_forward": can_go_forward,
        "active": active,
        "edited": edited,
        "user_edits_undone": undone,
        "structure": if tab.renderer_only { "renderer_only" } else { "readable" },
        "problem": tab.problem.map(recto_core::OpenError::message),
        "note": if edited { Some(UNSAVED_NOTE) } else { undone.then_some(USER_EDITS_UNDONE_NOTE) },
        "file_notice": session.notices().said(id).map(|said| json!({ "code": said.code, "message": said.message })),
        "untrusted": { "name": &*tab.name, "label": label },
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
    // A protected document's contents rows and notes, its place and how far into its contents the rail is were read with its password: its tab row stays, with no page.
    let withheld = |id: TabId| session.tabs().withheld_from_agents(id);
    let active_withheld = session.tabs().active().is_some_and(withheld);
    let rows: Vec<Value> = model
        .rows
        .iter()
        .filter(|row| row.kind == RowKind::Tab || !withheld(row.tab))
        .map(|row| {
            if row.kind == RowKind::Tab && withheld(row.tab) {
                return json!({ "kind": "tab", "tab": row.tab.0, "protected": true, "page": null, "depth": row.depth, "current": row.current, "expandable": false, "expanded": false, "untrusted": { "text": row.text } });
            }
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
    if active_withheld {
        return json!({ "rows": rows, "rail_filled": 0, "toc_open": false, "place": "", "protected": true, "protected_note": PROTECTED_NOTE, "edited": model.edited, "undone_owner_steps": model.undone_owner_steps });
    }
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
            Ok(DocFacts {
                page_count: 10,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
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
                Ok(DocFacts {
                    page_count: 10,
                    byte_len: 1,
                    encrypted: false,
                    version: "1.7".into(),
                    title: None,
                    toc: Toc::default(),
                    renderer_only: false,
                })
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
            let running = std::thread::spawn(move || execute(&*shared, Actor::Agent(Level::Control), &command, &mut Vec::new()).map(|_| ()));
            while !gate.inside.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
            assert!(session.try_lock().is_ok(), "the session's lock is held across the read");
            gate.release.store(true, Ordering::SeqCst);
            running.join().unwrap().unwrap();
            gate.armed.store(false, Ordering::SeqCst);
            assert_eq!(gate.reads.load(Ordering::SeqCst), 1, "read once");
        };
        wake(Command::from(crate::commands::tabs::Tap { tab: "#1".into() }));
        assert_eq!(session.lock().unwrap().tabs().active().map(|id| id.0), Some(1));
        wake(Command::from(crate::commands::tabs::Close { tab: "#1".into() }));
        assert_eq!(session.lock().unwrap().tabs().active().map(|id| id.0), Some(2));
        // Opening a file whose tab has gone cold.
        wake(Command::from(crate::commands::tabs::Open {
            path: PathBuf::from("/d/c.pdf"),
            page: None,
        }));
        assert_eq!(session.lock().unwrap().tabs().active().map(|id| id.0), Some(3));
    }

    /// A session on a host that mustn't wait on a disk, as the window's session thread is: what a command asks to read is noted, and read only when the test
    /// says, as the window's own read would be.
    struct Deferring<'a> {
        session: RefCell<&'a mut Session<Fake>>,
        woke: RefCell<Vec<(TabId, PathBuf)>>,
        loads: RefCell<Vec<PathBuf>>,
        ready: RefCell<Option<later::Fetched>>,
    }

    impl SessionAccess for Deferring<'_> {
        type Loader = Fake;
        fn with<R>(&self, step: impl FnOnce(&mut Session<Fake>) -> R) -> R {
            step(&mut self.session.borrow_mut())
        }
        fn reads_later(&self) -> Option<&dyn later::ReadLater> {
            Some(self)
        }
    }

    impl later::ReadLater for Deferring<'_> {
        fn wake(&self, tab: TabId, path: PathBuf) {
            self.woke.borrow_mut().push((tab, path));
        }
        fn load(&self, path: &Path) -> Option<later::Fetched> {
            let ready = self.ready.borrow_mut().take();
            if ready.is_none() {
                self.loads.borrow_mut().push(path.to_path_buf());
            }
            ready
        }
    }

    /// What a command could have changed: the tabs, which is active, each one's page, and what's side by side.
    type Looks = (Option<TabId>, Vec<(TabId, u32)>, Option<Vec<TabId>>);

    fn looks(session: &Session<Fake>) -> Looks {
        (
            session.tabs().active(),
            session.tabs().iter().map(|tab| (tab.id, tab.page)).collect(),
            session.comparison().map(|comparison| comparison.tabs.clone()),
        )
    }

    /// Failure (R2.3's riskiest part): a person's command on the window's session thread, waiting for a file to be read, changes something before its read, so
    /// running it again once the read's in does it twice or half (a tab opened twice, a tap that leaves a comparison and then lands elsewhere); or it reads the
    /// file in place and holds the thread up; or a close waits on the next tab's read, which a dead volume would make for ever. For a tap of a cold tab, an open of
    /// a new file and of a cold tab already open, and a close: nothing changes and nothing's said until the read's in, asking again changes nothing either, and
    /// once it's in the command answers as it would have in one go.
    #[test]
    fn a_command_waiting_for_a_read_does_nothing_until_it_runs_again() {
        let names = ["a", "b", "c", "d", "e", "f"];
        let tap = Command::from(crate::commands::tabs::Tap { tab: "#1".into() });
        let open_new = Command::from(crate::commands::tabs::Open {
            path: PathBuf::from("/d/new.pdf"),
            page: None,
        });
        let open_cold = Command::from(crate::commands::tabs::Open {
            path: PathBuf::from("/d/a.pdf"),
            page: Some(2),
        });
        // A ⌘-click on a cold tab's row, and two cold tabs put side by side, each read in turn (R2.3, C5e).
        let toggle_cold = Command::from(crate::commands::compare::Compare {
            toggle: Some("#1".into()),
            ..Default::default()
        });
        let compare_cold = Command::from(crate::commands::compare::Compare {
            tabs: Some("#1,#2".into()),
            ..Default::default()
        });
        for command in [&tap, &open_new, &open_cold, &toggle_cold, &compare_cold] {
            let mut session = with_tabs(&names);
            let before = looks(&session);
            let deferring = Deferring {
                session: RefCell::new(&mut session),
                woke: RefCell::default(),
                loads: RefCell::default(),
                ready: RefCell::default(),
            };
            let mut events = Vec::new();
            for asked in 0..2 {
                let first = execute(&deferring, Actor::Person, command, &mut events).expect_err("it waits for its read");
                assert!(later::is_later(&first), "{}: {first:?}", command.tag());
                assert_eq!(
                    deferring.with(|s| looks(s)),
                    before,
                    "{}: nothing changed before the read, asked {} times",
                    command.tag(),
                    asked + 1
                );
                assert!(events.is_empty(), "{}: and nothing said", command.tag());
            }
            // The read, as the window's thread makes it, and the next one the command asks for as it runs again, if it needs two.
            let mut answer = Err(later::later());
            for _ in 0..3 {
                let Err(waiting) = &answer else { break };
                assert!(later::is_later(waiting), "{}: {waiting:?}", command.tag());
                give_what_was_asked(&deferring);
                answer = execute(&deferring, Actor::Person, command, &mut events);
            }
            let answer = answer.expect("once it's in");
            let after = deferring.with(|s| looks(s));
            let mut twin = with_tabs(&names);
            let mut twin_events = Vec::new();
            let direct = execute(&Direct(RefCell::new(&mut twin), None), Actor::Person, command, &mut twin_events).expect("in one go");
            assert_eq!(answer, direct, "{}: the same answer", command.tag());
            assert_eq!(after, looks(&twin), "{}: the same session", command.tag());
            assert_eq!(format!("{events:?}"), format!("{twin_events:?}"), "{}: the same news", command.tag());
        }
        // A close doesn't wait for the tab it shows next: the session wakes that as it always has for the window.
        let mut session = with_tabs(&names);
        let deferring = Deferring {
            session: RefCell::new(&mut session),
            woke: RefCell::default(),
            loads: RefCell::default(),
            ready: RefCell::default(),
        };
        let close = Command::from(crate::commands::tabs::Close { tab: "#6".into() });
        execute(&deferring, Actor::Person, &close, &mut Vec::new()).expect("closed at once");
        assert!(deferring.woke.borrow().is_empty(), "nothing read for it");
        assert_eq!(deferring.with(|s| s.tabs().len()), 5);
    }

    /// The read a command waiting on [`Deferring`] asked for last, made as the window's thread makes it: a cold tab's, or a new file's.
    fn give_what_was_asked(deferring: &Deferring<'_>) {
        let woke = deferring.woke.borrow_mut().drain(..).next_back();
        if let Some((tab, path)) = woke {
            deferring.with(|s| s.give_woken(tab, Fake.load(&path), None));
            return;
        }
        assert_eq!(
            deferring.loads.borrow().last().map(PathBuf::as_path),
            Some(Path::new("/d/new.pdf")),
            "a file asked for"
        );
        *deferring.ready.borrow_mut() = Some(later::Fetched {
            stamp: None,
            facts: Fake.load(Path::new("/d/new.pdf")),
            history: None,
            unlocked: false,
        });
    }

    /// Failure (R2.3, input to frame): the window's tap, run for the person, builds the sidebar's every row for an answer nobody reads, 650 µs of a tap's
    /// 780 at 1,000 tabs, past the bound on a person's input. Where the answer isn't read, a tap does all it does (the tab in front, its news) and answers
    /// nothing; where it is, it answers the sidebar, as ever.
    #[test]
    fn a_tap_whose_answer_isnt_read_builds_none() {
        struct Unread<'a>(RefCell<&'a mut Session<Fake>>);
        impl SessionAccess for Unread<'_> {
            type Loader = Fake;
            fn with<R>(&self, step: impl FnOnce(&mut Session<Fake>) -> R) -> R {
                step(&mut self.0.borrow_mut())
            }
            fn answer_read(&self) -> bool {
                false
            }
        }
        let tap = Command::from(crate::commands::tabs::Tap { tab: "#1".into() });
        let (mut session, mut twin) = (with_tabs(&["a", "b"]), with_tabs(&["a", "b"]));
        let (mut events, mut twin_events) = (Vec::new(), Vec::new());
        let unread = execute(&Unread(RefCell::new(&mut session)), Actor::Person, &tap, &mut events).expect("tapped");
        let read = execute(&Direct(RefCell::new(&mut twin), None), Actor::Person, &tap, &mut twin_events).expect("tapped");
        assert_eq!(unread, Value::Null, "nothing built");
        assert!(read["rows"].as_array().is_some_and(|rows| rows.len() >= 2), "the sidebar: {read}");
        assert_eq!(looks(&session), looks(&twin), "the same done");
        assert_eq!(format!("{events:?}"), format!("{twin_events:?}"), "the same news");
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
        let off = run(&mut session, Level::Off, &Command::from(crate::commands::reading::Sidebar {})).unwrap_err();
        assert_eq!(off.code, "permission_denied");
        assert_eq!(off.exit, exit::PERMISSION);
        let read_only = run(&mut session, Level::ReadOnly, &Command::from(crate::commands::tabs::Close { tab: "#1".into() })).unwrap_err();
        assert_eq!(read_only.code, "permission_denied");
        assert_eq!(session.tabs().len(), 1);
        assert!(
            run(
                &mut session,
                Level::ReadOnly,
                &Command::from(crate::commands::reading::Tabs { from: None, limit: None })
            )
            .is_ok()
        );
    }

    /// Failure: an agent the user turned off reads a page's text, the one thing Off exists to stop. Asserted on the level itself and on the
    /// call, which must refuse before any worker starts.
    #[test]
    fn reading_page_text_needs_more_than_off() {
        let text = Command::from(crate::commands::text::Text { tab: "#1".into(), page: 1 });
        assert_eq!(text.access(), Access::At(Level::ReadOnly));
        let mut session = with_tabs(&["a"]);
        let refused = run(&mut session, Level::Off, &text).unwrap_err();
        assert_eq!((refused.code, refused.exit), ("permission_denied", exit::PERMISSION));
        let render = Command::from(crate::commands::export::Render {
            tab: "#1".into(),
            page: 1,
            scale: 2.0,
            out: "/tmp/x.png".into(),
            anyway: false,
        });
        assert_eq!(
            run(&mut session, Level::Control, &render).unwrap_err().code,
            "permission_denied",
            "it writes to disk, so it needs Full"
        );
    }

    fn budget_command(limit_mib: Option<u32>, retire_mib: Option<u32>) -> Command {
        Command::from(crate::commands::measure::BudgetRender {
            tab: "#1".into(),
            pages: 1,
            first: None,
            scale: 2.0,
            fresh: false,
            tiles: false,
            samples: false,
            limit_mib,
            retire_mib,
        })
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
        let opening = AgentError::from(SessionError::Open(OpenError::PasswordNeeded));
        assert_eq!(opening.exit, exit::PROTECTED, "the session refusing it first must say the same");
    }

    /// Failure: a mistyped or extra field on the new commands is silently ignored, and an agent believes it did something.
    #[test]
    fn render_commands_parse_strictly() {
        assert!(matches!(
            parse(r##"{"cmd":"text","tab":"#1","page":2}"##),
            Ok(Command::Read(crate::commands::Read::Text(crate::commands::text::Text { page: 2, .. })))
        ));
        assert!(parse(r##"{"cmd":"text","tab":"#1"}"##).is_err(), "the page is required");
        assert!(parse(r##"{"cmd":"text","tab":"#1","page":1,"extra":1}"##).is_err());
        let Ok(Command::Read(crate::commands::Read::Render(crate::commands::export::Render { scale, .. }))) =
            parse(r##"{"cmd":"render","tab":"#1","page":1,"out":"a.png"}"##)
        else {
            panic!()
        };
        assert_eq!(scale, DEFAULT_SCALE);
        assert!(matches!(
            parse(r##"{"cmd":"budget_render","tab":"#1","pages":5}"##),
            Ok(Command::Measure(crate::commands::Measure::BudgetRender(
                crate::commands::measure::BudgetRender { .. }
            )))
        ));
        assert!(parse(r##"{"cmd":"budget_render","tab":"#1","pages":5,"typo":1}"##).is_err());
    }

    /// Failure: an ambiguous name acts on whichever tab came first, rather than listing the candidates.
    #[test]
    fn ambiguous_names_list_candidates_and_never_guess() {
        let mut session = with_tabs(&["Report 2025", "Report 2026", "Invoice"]);
        let error = run(
            &mut session,
            Level::Control,
            &Command::from(crate::commands::tabs::Tap { tab: "report".into() }),
        )
        .unwrap_err();
        assert_eq!(error.code, "ambiguous");
        assert_eq!(error.exit, exit::AMBIGUOUS);
        assert_eq!(error.candidates, ["#1", "#2"], "ids to use, and no names outside `untrusted`");
        assert_eq!(error.untrusted.unwrap()["candidates"][0]["name"], "Report 2025");
        assert!(
            run(
                &mut session,
                Level::Control,
                &Command::from(crate::commands::tabs::Tap { tab: "invoice".into() })
            )
            .is_ok()
        );
    }

    /// Failure: the tap command doesn't reach the session's rule, so agents and QA test something the UI doesn't do.
    #[test]
    fn tap_uses_the_sidebar_rule() {
        let mut session = with_tabs(&["a"]);
        let result = run(&mut session, Level::Control, &Command::from(crate::commands::tabs::Tap { tab: "#1".into() })).unwrap();
        assert_eq!(result["toc_open"], false);
    }

    /// Failure: an unknown command or field is silently ignored, and an agent believes it did something.
    #[test]
    fn unknown_commands_and_fields_are_refused() {
        assert_eq!(parse(r#"{"cmd":"format_disk"}"#).unwrap_err().code, "bad_command");
        assert_eq!(parse(r#"{"cmd":"tabs","extra":1}"#).unwrap_err().code, "bad_command");
        assert!(matches!(
            parse(r#"{"cmd":"tabs"}"#),
            Ok(Command::Read(crate::commands::Read::Tabs(crate::commands::reading::Tabs { .. })))
        ));
    }

    fn every_command() -> Vec<Command> {
        let tab = || "#1".to_string();
        vec![
            Command::from(crate::commands::tabs::Open {
                path: "/d/a.pdf".into(),
                page: None,
            }),
            Command::from(crate::commands::reading::Tabs { from: None, limit: None }),
            Command::from(crate::commands::reading::Sidebar {}),
            Command::from(crate::commands::tabs::Tap { tab: tab() }),
            Command::from(crate::commands::linking::Goto {
                tab: tab(),
                page: Some(1),
                label: None,
                within: None,
            }),
            Command::from(crate::commands::linking::Links { tab: tab(), page: 1 }),
            Command::from(crate::commands::linking::Follow { tab: tab(), page: 1, link: 1 }),
            Command::from(crate::commands::linking::Back { tab: tab() }),
            Command::from(crate::commands::linking::Forward { tab: tab() }),
            Command::from(crate::commands::tabs::Recent {}),
            Command::from(crate::commands::tabs::RecentClear {}),
            Command::from(crate::commands::tabs::Close { tab: tab() }),
            Command::from(crate::commands::measure::Budget {}),
            Command::from(crate::commands::compare::Compare {
                tabs: None,
                sync: None,
                off: false,
                toggle: None,
                reading: None,
            }),
            Command::from(crate::commands::compare::Compare {
                tabs: None,
                sync: None,
                off: true,
                toggle: None,
                reading: None,
            }),
            Command::from(crate::commands::export::Render {
                tab: tab(),
                page: 1,
                scale: 2.0,
                out: "/d/x.png".into(),
                anyway: false,
            }),
            Command::from(crate::commands::export::ExportSvg {
                tab: tab(),
                page: 1,
                out: "/d/x.svg".into(),
                rect: None,
                text: None,
                scale: 2.0,
                strip_metadata: false,
                anyway: false,
            }),
            Command::from(crate::commands::export::Images { tab: tab(), page: 1 }),
            Command::from(crate::commands::export::Image {
                tab: tab(),
                page: 1,
                number: 1,
                out: "/d/x.png".into(),
                anyway: false,
            }),
            Command::from(crate::commands::export::Snapshot {
                tab: tab(),
                page: 1,
                rect: [0.0, 0.0, 10.0, 10.0],
                format: "png".into(),
                out: Some("/d/x.png".into()),
                scale: 2.0,
                anyway: false,
            }),
            Command::from(crate::commands::export::Print {
                tab: tab(),
                pages: None,
                dpi: 300,
                to_pdf: "/d/x.pdf".into(),
                anyway: false,
            }),
            Command::from(crate::commands::text::Text { tab: tab(), page: 1 }),
            Command::from(crate::commands::text::Select {
                tab: tab(),
                page: Some(1),
                to_page: None,
                chars: None,
                at: None,
                to: None,
                unit: None,
                all: false,
                anyway: false,
            }),
            Command::from(crate::commands::text::Copy {
                tab: tab(),
                page: Some(1),
                to_page: None,
                chars: None,
                at: None,
                to: None,
                unit: None,
                all: false,
                anyway: false,
            }),
            budget_command(None, None),
            Command::from(crate::commands::measure::BudgetViewport {
                tab: tab(),
                viewport: "10x10".into(),
                first: None,
                scale: 2.0,
                strategy: None,
            }),
            Command::from(crate::commands::reading::Where { tab: tab() }),
            Command::from(crate::commands::reading::Facts { tab: tab() }),
            Command::from(crate::commands::reading::Contents {
                tab: tab(),
                from: None,
                limit: None,
                depth: None,
                generated: false,
            }),
            Command::from(crate::commands::text::Search {
                tab: tab(),
                query: "x".into(),
                pages: None,
                limit: None,
                scan: false,
                regex: false,
                rank: false,
                context: None,
            }),
            Command::from(crate::commands::finding::Find {
                query: Some("x".into()),
                ..Default::default()
            }),
            Command::from(crate::commands::text::Index { tab: tab(), agent: false }),
            Command::from(crate::commands::view::Palette {
                query: Some("zoom".into()),
                run: false,
                limit: None,
            }),
            Command::from(crate::commands::text::IndexStatus { tab: tab() }),
            Command::from(crate::commands::text::IndexClear { tab: tab(), all: false }),
            Command::from(crate::commands::reading::Signatures { tab: tab() }),
            Command::from(crate::commands::editing::Edits { tab: tab() }),
            Command::from(crate::commands::editing::PageMove {
                tab: tab(),
                pages: "1".into(),
                to: 1,
            }),
            Command::from(crate::commands::editing::PageRotate {
                tab: tab(),
                pages: "1".into(),
                degrees: 90,
            }),
            Command::from(crate::commands::editing::PageDelete { tab: tab(), pages: "1".into() }),
            Command::from(crate::commands::editing::PageInsert {
                tab: tab(),
                path: "/d/b.pdf".into(),
                at: 1,
            }),
            Command::from(crate::commands::editing::Undo { tab: tab() }),
            Command::from(crate::commands::editing::Redo { tab: tab() }),
            Command::from(crate::commands::editing::Discard { tab: tab() }),
            Command::from(crate::commands::editing::ClearHistory { tab: tab() }),
            Command::from(crate::commands::edit_mode::EditMode {
                set: commands::words::OnOff::On,
            }),
            Command::from(crate::commands::contents::ContentsFold { entry: 0, tab: tab() }),
            Command::from(crate::commands::contents::ContentsShow {
                which: commands::contents::Which::Generated,
                tab: tab(),
            }),
            Command::from(crate::commands::contents::ContentsStop { tab: tab() }),
            Command::from(crate::commands::record::SessionSleep { tab: tab() }),
            Command::from(crate::commands::saving::Save {
                tab: tab(),
                out: None,
                replace: false,
            }),
            Command::from(commands::access::Sign {}),
            Command::from(commands::session::Status {}),
            Command::from(commands::access::PermissionShow {}),
            Command::from(commands::access::PermissionRequest {
                level: Level::Full,
                timeout_ms: None,
            }),
            Command::from(commands::access::LevelChanged {}),
            Command::from(commands::sync::Wait {
                condition: Condition::TabOpen,
                tab: tab(),
                page: None,
                timeout_ms: 1,
            }),
            Command::from(commands::sync::Watch {
                events: None,
                timeout_ms: None,
                count: None,
            }),
            Command::from(commands::sync::WatchStop {}),
            Command::from(commands::access::Log { last: None }),
            Command::from(crate::commands::view::View {
                paper: None,
                sidebar: None,
                agent_icon: None,
                zoom: None,
                layout: None,
                tab: None,
            }),
            Command::from(crate::commands::qa::QaRead {
                verb: "state".into(),
                out: None,
                timeout_ms: None,
            }),
            Command::from(crate::commands::qa::QaDrive {
                verb: "zoom".into(),
                value: "1".into(),
                timeout_ms: None,
                tab: None,
            }),
            Command::from(commands::session::Stop {}),
            Command::from(crate::commands::record::SessionShow {}),
            Command::from(crate::commands::record::SessionSave {}),
            Command::from(crate::commands::record::SessionRestore {}),
            Command::from(crate::commands::record::SessionCheck {}),
            Command::from(commands::updates::UpdateStatus {}),
            Command::from(commands::updates::UpdateCheck {}),
            Command::from(commands::updates::UpdateInstall { zip: None, now: false }),
            Command::from(commands::updates::UpdateDevBuilds { set: None }),
            Command::from(commands::updates::Settings { auto_update: None }),
        ]
    }

    /// Failure: an agent below Full can install an update, or turn on what would install one for it, or can't read how things stand at Read-only. The owner's bar: installing needs Full. A session with no app says it's not a matter of level.
    #[test]
    fn installing_needs_full_and_reading_needs_only_read() {
        assert_eq!(
            Command::from(commands::updates::UpdateInstall { zip: None, now: false }).access(),
            Access::At(Level::Full)
        );
        assert_eq!(
            Command::from(commands::updates::UpdateInstall {
                zip: Some("/b/RectoPDF-0.1.0.1.zip".into()),
                now: true
            })
            .access(),
            Access::At(Level::Full),
            "a build too: only the owner's switch lets one in below that, in the app"
        );
        assert_eq!(
            Command::from(commands::updates::UpdateDevBuilds {
                set: Some(commands::words::OnOff::On)
            })
            .access(),
            Access::At(Level::Full)
        );
        assert_eq!(
            Command::from(commands::updates::UpdateDevBuilds { set: None }).access(),
            Access::At(Level::ReadOnly)
        );
        assert_eq!(
            Command::from(commands::updates::Settings {
                auto_update: Some(commands::words::OnOff::On)
            })
            .access(),
            Access::At(Level::Full)
        );
        assert_eq!(
            Command::from(commands::updates::Settings {
                auto_update: Some(commands::words::OnOff::Off)
            })
            .access(),
            Access::At(Level::Full)
        );
        assert_eq!(
            Command::from(commands::updates::Settings { auto_update: None }).access(),
            Access::At(Level::ReadOnly)
        );
        assert_eq!(Command::from(commands::updates::UpdateStatus {}).access(), Access::At(Level::ReadOnly));
        assert_eq!(Command::from(commands::updates::UpdateCheck {}).access(), Access::At(Level::Control));
        let mut s = with_tabs(&["a"]);
        for command in [
            Command::from(commands::updates::UpdateStatus {}),
            Command::from(commands::updates::UpdateCheck {}),
            Command::from(commands::updates::UpdateInstall { zip: None, now: false }),
            Command::from(commands::updates::UpdateDevBuilds { set: None }),
            Command::from(commands::updates::Settings { auto_update: None }),
        ] {
            let error = run(&mut s, Level::Full, &command).unwrap_err();
            assert_eq!(error.code, "needs_app");
            assert_ne!(error.exit, exit::PERMISSION);
        }
        assert!(
            run(
                &mut s,
                Level::Control,
                &Command::from(commands::updates::UpdateInstall { zip: None, now: false })
            )
            .unwrap_err()
            .exit
                == exit::PERMISSION,
            "below the level, it's the level that's said"
        );
    }

    /// Failure: changing how pages are shown works at Read-only, or asking needs more than asking should, because the row can state only one level.
    #[test]
    fn view_asks_at_read_only_and_changes_at_control() {
        assert_eq!(
            Command::from(crate::commands::view::View {
                paper: None,
                sidebar: None,
                agent_icon: None,
                zoom: None,
                layout: None,
                tab: None
            })
            .access(),
            Access::At(Level::ReadOnly)
        );
        assert_eq!(
            Command::from(crate::commands::view::View {
                paper: Some(crate::commands::view::PaperChoice::Is(recto_session::Paper::White)),
                sidebar: None,
                agent_icon: None,
                zoom: None,
                layout: None,
                tab: None
            })
            .access(),
            Access::At(Level::Control)
        );
        assert_eq!(
            Command::from(crate::commands::view::View {
                paper: None,
                sidebar: Some(crate::commands::view::Shown::Off),
                agent_icon: None,
                zoom: None,
                layout: None,
                tab: None
            })
            .access(),
            Access::At(Level::Control)
        );
    }

    /// Failure: a session with no app takes a QA command and does nothing, or says something that reads as though it worked. It's refused plainly, and not as a matter of level, since no level would help.
    #[test]
    fn qa_commands_need_an_app() {
        let mut s = with_tabs(&["a"]);
        for command in [
            Command::from(crate::commands::qa::QaRead {
                verb: "state".into(),
                out: None,
                timeout_ms: None,
            }),
            Command::from(crate::commands::qa::QaDrive {
                verb: "zoom".into(),
                value: "1".into(),
                timeout_ms: None,
                tab: None,
            }),
        ] {
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
        let compare = |tabs: Option<&str>, sync: Option<&str>, off: bool| {
            Command::from(crate::commands::compare::Compare {
                tabs: tabs.map(str::to_string),
                sync: sync.map(str::to_string),
                off,
                toggle: None,
                reading: None,
            })
        };
        let asked = run(&mut session, Level::ReadOnly, &compare(None, None, false)).unwrap();
        assert_eq!(asked["comparing"], false);
        assert_eq!(
            run(&mut session, Level::ReadOnly, &compare(Some("#1,#2"), None, false)).unwrap_err().exit,
            exit::PERMISSION,
            "changing it is Control"
        );
        let two = run(&mut session, Level::Control, &compare(Some("#1, #3"), None, false)).unwrap();
        assert_eq!(two["comparing"], true);
        assert_eq!(
            two["panes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pane| pane["tab"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert_eq!(two["sync"], "off", "separate is the default");
        let synced = run(&mut session, Level::Control, &compare(None, Some("on"), false)).unwrap();
        assert_eq!(synced["sync"], "on");
        assert_eq!(synced["sync_from"], json!([1, 1]));
        assert_eq!(
            run(&mut session, Level::Control, &compare(Some("#1,#2,#3,#4"), None, false)).unwrap_err().code,
            "too_many_panes"
        );
        assert_eq!(
            run(&mut session, Level::Control, &compare(Some("#1"), None, false)).unwrap_err().code,
            "too_few_panes"
        );
        assert_eq!(
            run(&mut session, Level::Control, &compare(Some("#1,#1"), None, false)).unwrap_err().code,
            "same_tab_twice"
        );
        assert_eq!(
            run(&mut session, Level::Control, &compare(Some("#1,#9"), None, false)).unwrap_err().code,
            "no_such_tab"
        );
        assert_eq!(
            run(&mut session, Level::Control, &compare(None, Some("maybe"), false)).unwrap_err().code,
            "usage"
        );
        assert_eq!(
            run(&mut session, Level::Control, &compare(Some("#1,#2"), None, true)).unwrap_err().code,
            "usage"
        );
        let ended = run(&mut session, Level::Control, &compare(None, None, true)).unwrap();
        assert_eq!(ended["comparing"], false);
        assert_eq!(
            run(&mut session, Level::Control, &compare(None, Some("on"), false)).unwrap_err().code,
            "not_comparing"
        );
        run(&mut session, Level::Control, &compare(Some("#2,#4"), None, false)).unwrap();
        run(&mut session, Level::Control, &Command::from(crate::commands::tabs::Tap { tab: "#4".into() })).unwrap();
        assert_eq!(
            run(&mut session, Level::ReadOnly, &compare(None, None, false)).unwrap()["comparing"],
            false,
            "a tap goes back to one"
        );
    }

    /// Failure: two faces of one command disagree on what they need, so which row is found first decides who gets in. QA's two commands are the
    /// exception, held in their own module: each verb's level is its own row's (`commands::qa::verbs`).
    #[test]
    fn rows_sharing_a_command_share_their_access() {
        for spec in table::TABLE.iter().filter(|spec| !matches!(spec.tag, "qa_read" | "qa_drive")) {
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
        assert_eq!(
            off_allowed,
            ["level_changed", "permission_request", "permission_show", "status"],
            "what's open at Off must stay this short"
        );
        assert_eq!(Command::from(commands::access::Sign {}).access(), Access::Nobody);
        for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
            assert!(!allowed(level, Access::Nobody) && !allowed(level, Access::UserOnly));
        }
        assert!(
            table::TABLE
                .iter()
                .filter(|spec| spec.access == Access::UserOnly)
                .all(|spec| spec.route == table::Route::Here),
            "what's for the user alone must not go over the socket"
        );
        assert!(
            table::TABLE
                .iter()
                .filter(|spec| spec.tag == "permission_set")
                .all(|spec| spec.access == Access::UserOnly)
        );
        assert!(
            parse(r#"{"cmd":"permission_set","level":"full"}"#).is_err(),
            "no command on the wire sets a level"
        );
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
        for exit_code in [
            exit::FAILED,
            exit::USAGE,
            exit::NOT_FOUND,
            exit::AMBIGUOUS,
            exit::PERMISSION,
            exit::NOT_RUNNING,
            exit::TIMED_OUT,
            exit::PROTECTED,
            exit::BUSY,
        ] {
            assert!(!AgentError::new("some_new_code", "m", exit_code).next.is_empty(), "exit {exit_code}");
        }
    }

    /// Failure: the exit codes move, which breaks every script that reads them. Fixed by the agent report.
    #[test]
    fn exit_codes_are_fixed() {
        assert_eq!(
            (
                exit::OK,
                exit::FAILED,
                exit::USAGE,
                exit::NOT_FOUND,
                exit::AMBIGUOUS,
                exit::PERMISSION,
                exit::NOT_RUNNING,
                exit::TIMED_OUT,
                exit::PROTECTED,
                exit::BUSY
            ),
            (0, 1, 2, 3, 4, 5, 6, 7, 8, 10)
        );
    }

    fn edit_session() -> Session<Fake> {
        with_tabs(&["a"])
    }

    /// Failure: page edits reach the session wrongly: off by one on a one-based number, or the tab doesn't report it has unsaved edits.
    #[test]
    fn page_commands_use_one_based_numbers_and_report_unsaved_edits() {
        let mut s = edit_session();
        let moved = run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageMove {
                tab: "#1".into(),
                pages: "3".into(),
                to: 1,
            }),
        )
        .unwrap();
        assert_eq!(moved["edited"], true);
        assert_eq!(moved["saved"], false);
        assert!(moved["note"].as_str().unwrap().contains("aren't saved"), "it must say so plainly: {moved}");
        let id = TabId(1);
        assert_eq!(s.lists().get(id).unwrap().slots()[0].page, 2);
        let tabs = run(
            &mut s,
            Level::ReadOnly,
            &Command::from(crate::commands::reading::Tabs { from: None, limit: None }),
        )
        .unwrap();
        assert_eq!(tabs["tabs"][0]["edited"], true);
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageRotate {
                tab: "active".into(),
                pages: "2-3".into(),
                degrees: -90,
            }),
        )
        .unwrap();
        assert_eq!(s.lists().get(id).unwrap().slots()[1].quarter_turns, 3);
        let turns = |s: &Session<_>| s.lists().get(id).unwrap().slots().iter().map(|slot| slot.quarter_turns).collect::<Vec<u8>>();
        let before = turns(&s);
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageRotate {
                tab: "active".into(),
                pages: "all".into(),
                degrees: 90,
            }),
        )
        .unwrap();
        assert!(
            turns(&s).iter().zip(&before).all(|(now, was)| *now == (was + 1) % 4),
            "every page a quarter more: {:?} from {before:?}",
            turns(&s)
        );
        assert!(s.undo_edit(id));
        assert_eq!(turns(&s), before, "one undo takes every page back, as it was one step");
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageDelete {
                tab: "active".into(),
                pages: "1".into(),
            }),
        )
        .unwrap();
        assert_eq!(s.pages().count(id), 9);
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageInsert {
                tab: "active".into(),
                path: "/d/b.pdf".into(),
                at: 10,
            }),
        )
        .unwrap();
        assert_eq!(s.pages().count(id), 19);
        let shown = run(
            &mut s,
            Level::ReadOnly,
            &Command::from(crate::commands::editing::Edits { tab: "active".into() }),
        )
        .unwrap();
        assert_eq!(shown["runs"].as_array().unwrap().last().unwrap()["added"], true);
        assert_eq!(shown["runs"].as_array().unwrap().last().unwrap()["untrusted"]["added_file"], "b.pdf");
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: "active".into() })).unwrap();
        assert_eq!(s.pages().count(id), 9);
        let redone = run(&mut s, Level::Control, &Command::from(crate::commands::editing::Redo { tab: "active".into() })).unwrap();
        assert_eq!((&redone["pages"], &redone["can_redo"]), (&json!(19), &json!(false)));
    }

    /// Failure: a bad request reads as success, or changes the document. Each must be refused with its own code and leave things as they were.
    #[test]
    fn bad_page_commands_change_nothing() {
        let mut s = edit_session();
        let id = TabId(1);
        let tab = || "#1".to_string();
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageDelete {
                    tab: tab(),
                    pages: "1-10".into()
                })
            )
            .unwrap_err()
            .code,
            "last_page"
        );
        let far = run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageRotate {
                tab: tab(),
                pages: "11".into(),
                degrees: 90,
            }),
        )
        .unwrap_err();
        assert_eq!((far.code, far.exit), ("page_out_of_range", exit::NOT_FOUND));
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageRotate {
                    tab: tab(),
                    pages: "1".into(),
                    degrees: 45
                })
            )
            .unwrap_err()
            .code,
            "bad_rotation"
        );
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageRotate {
                    tab: tab(),
                    pages: "1".into(),
                    degrees: 720
                })
            )
            .unwrap_err()
            .code,
            "bad_rotation",
            "a turn that does nothing mustn't count as an edit"
        );
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageMove {
                    tab: tab(),
                    pages: "0".into(),
                    to: 1
                })
            )
            .unwrap_err()
            .code,
            "bad_pages"
        );
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageMove {
                    tab: tab(),
                    pages: "9-3".into(),
                    to: 1
                })
            )
            .unwrap_err()
            .code,
            "bad_pages"
        );
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageRotate {
                    tab: tab(),
                    pages: "4-".into(),
                    degrees: 90
                })
            )
            .unwrap_err()
            .code,
            "bad_pages",
            "to the end isn't for edits"
        );
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageRotate {
                    tab: tab(),
                    pages: "4-x-".into(),
                    degrees: 90
                })
            )
            .unwrap_err()
            .code,
            "bad_pages"
        );
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageMove {
                    tab: tab(),
                    pages: "1".into(),
                    to: 0
                })
            )
            .unwrap_err()
            .code,
            "no_such_page"
        );
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageInsert {
                    tab: tab(),
                    path: "relative.pdf".into(),
                    at: 1
                })
            )
            .unwrap_err()
            .code,
            "bad_path"
        );
        assert_eq!(
            run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() }))
                .unwrap_err()
                .code,
            "nothing_to_undo"
        );
        assert_eq!(
            run(&mut s, Level::Control, &Command::from(crate::commands::editing::Redo { tab: tab() }))
                .unwrap_err()
                .code,
            "nothing_to_redo"
        );
        assert!(!s.lists().is_edited(id));
        assert_eq!(s.pages().count(id), 10);
    }

    /// Failure: a save that can't write pretends it did, so someone believes their edits are on disk; and a save is open to a level that can't edit. Here the file isn't there, so it fails, says nothing was written, and the edits stay. And closing must not lose them without saying.
    #[test]
    fn a_save_that_cant_write_says_so_and_closing_keeps_edits() {
        let mut s = edit_session();
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageRotate {
                tab: "#1".into(),
                pages: "1".into(),
                degrees: 90,
            }),
        )
        .unwrap();
        assert_eq!(
            run(
                &mut s,
                Level::ReadOnly,
                &Command::from(crate::commands::saving::Save {
                    tab: "#1".into(),
                    out: None,
                    replace: false
                })
            )
            .unwrap_err()
            .code,
            "permission_denied"
        );
        let save = run(
            &mut s,
            Level::Full,
            &Command::from(crate::commands::saving::Save {
                tab: "#1".into(),
                out: None,
                replace: false,
            }),
        )
        .unwrap_err();
        assert!(
            save.message.contains("Nothing was written") || save.message.contains("nothing was changed"),
            "{}",
            save.message
        );
        assert!(
            s.lists().is_edited(TabId(1)) && !s.saves().is_saving(TabId(1)),
            "the edits stand, and the tab isn't held"
        );
        let replace = run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::saving::Save {
                tab: "#1".into(),
                out: Some("/nowhere/x.pdf".into()),
                replace: true,
            }),
        )
        .unwrap_err();
        assert_eq!(replace.code, "permission_denied", "replacing another file is Full's");
        for level in [Level::Control, Level::Full] {
            let close = run(&mut s, level, &Command::from(crate::commands::tabs::Close { tab: "#1".into() })).unwrap_err();
            assert_eq!(close.code, "unsaved_changes", "{level:?}");
            assert!(close.next.contains("discard"), "{}", close.next);
        }
        assert_eq!(s.tabs().len(), 1);
        assert_eq!(
            run(&mut s, Level::ReadOnly, &Command::from(crate::commands::editing::Discard { tab: "#1".into() }))
                .unwrap_err()
                .code,
            "permission_denied",
            "throwing edits away asks for what making them does"
        );
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Discard { tab: "#1".into() })).unwrap();
        assert!(!s.lists().is_edited(TabId(1)));
        assert!(run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: "#1".into() })).is_ok());
    }

    fn owners_rotation(s: &mut Session<Fake>, page: u32) {
        s.edit_pages(TabId(1), |edits| edits.rotate(page, 1, 1)).unwrap();
    }

    /// A rotation of page 1, which tells apart "the agent's edit is still here" from "it's gone".
    fn turned_pages(s: &Session<Fake>) -> Vec<u8> {
        s.lists()
            .get(TabId(1))
            .map_or_else(Vec::new, |edits| edits.slots().iter().map(|slot| slot.quarter_turns).collect())
    }

    fn agent_attempts() -> Vec<Command> {
        let tab = || "#1".to_string();
        vec![
            Command::from(crate::commands::editing::PageRotate {
                tab: tab(),
                pages: "9".into(),
                degrees: 90,
            }),
            Command::from(crate::commands::editing::PageDelete {
                tab: tab(),
                pages: "10".into(),
            }),
            Command::from(crate::commands::editing::PageMove {
                tab: tab(),
                pages: "9".into(),
                to: 10,
            }),
            Command::from(crate::commands::editing::PageInsert {
                tab: tab(),
                path: "/d/b.pdf".into(),
                at: 1,
            }),
            Command::from(crate::commands::tabs::Close { tab: tab() }),
        ]
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
            assert!(s.lists().get(id).is_some_and(recto_session::edit::PageEdits::holds_owner_edits), "{when}");
            assert_eq!(s.tabs().len(), 1, "{when}");
        };
        // Directly: discard and close while they're still applied.
        assert_eq!(
            run(&mut s, Level::Control, &Command::from(crate::commands::editing::Discard { tab: tab() }))
                .unwrap_err()
                .code,
            "would_discard_user_edits"
        );
        assert_eq!(
            run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() }))
                .unwrap_err()
                .code,
            "unsaved_changes"
        );
        check_still_there(&s, "before any undo");
        // Undone, one at a time. Undoing is allowed, since it can be redone. Nothing that clears them is, at any point.
        for left in [1, 0] {
            run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();
            for attempt in agent_attempts() {
                let refused = run(&mut s, Level::Control, &attempt).unwrap_err();
                assert_eq!(
                    (refused.code, refused.exit),
                    ("would_clear_redo", exit::PERMISSION),
                    "{} with {left} left",
                    attempt.tag()
                );
                assert!(refused.next.contains("recto redo") && refused.next.contains(ASK_FULL), "{}", refused.next);
                assert_eq!(refused.ask.as_deref(), Some(ASK_FULL));
                assert!(refused.message.contains("user's"), "the message must say whose they are: {}", refused.message);
            }
            let discard = run(&mut s, Level::Control, &Command::from(crate::commands::editing::Discard { tab: tab() })).unwrap_err();
            assert_eq!((discard.code, discard.exit), ("would_discard_user_edits", exit::PERMISSION));
            assert!(discard.message.contains("user's"), "{}", discard.message);
            check_still_there(&s, "after an undo");
        }
        assert!(
            !s.lists().is_edited(id),
            "with both undone there's nothing unsaved, and that's when a close would have got through"
        );
        // And redo gets back all of it, exactly.
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Redo { tab: tab() })).unwrap();
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Redo { tab: tab() })).unwrap();
        assert_eq!(turned_pages(&s), users);
        // The agent's own work on top: edit, undo, edit again, all fine, and the user's stay under it.
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageRotate {
                tab: tab(),
                pages: "9".into(),
                degrees: 90,
            }),
        )
        .unwrap();
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageDelete {
                tab: tab(),
                pages: "10".into(),
            }),
        )
        .expect("only the agent's own undone step goes");
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();
        // Closing with the agent's own step undone still can't happen, because the user's edits are unsaved.
        let close = run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).unwrap_err();
        assert_eq!(close.code, "unsaved_changes");
        assert!(
            close.message.contains("user's") && close.next.contains(ASK_FULL),
            "{} {}",
            close.message,
            close.next
        );
        assert_eq!(turned_pages(&s), users, "nothing of the user's has gone");
        // At Full, it's the holder's call, as it was.
        run(&mut s, Level::Full, &Command::from(crate::commands::editing::Discard { tab: tab() })).unwrap();
        assert!(run(&mut s, Level::Full, &Command::from(crate::commands::tabs::Close { tab: tab() })).is_ok());
    }

    /// Failure: an agent's `undo` hides the user's edit. It's off the page, so every answer read `saved: true` and nothing said it was there to lose, and the session's own close, the app's way out, would have gone through. At Full it's still the holder's call, and goes through this time because it's said on purpose.
    #[test]
    fn undoing_the_users_edit_doesnt_read_as_nothing_there() {
        let mut s = edit_session();
        let id = TabId(1);
        let tab = || "#1".to_string();
        owners_rotation(&mut s, 0);
        let undone = run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();
        assert_eq!(
            (&undone["edited"], &undone["saved"], &undone["user_edits_undone"], &undone["can_redo"]),
            (&json!(false), &json!(false), &json!(true), &json!(true)),
            "{undone}"
        );
        assert!(human::render("undo", &undone).contains("recto redo"), "{undone}");
        let where_now = run(&mut s, Level::ReadOnly, &Command::from(crate::commands::reading::Where { tab: tab() })).unwrap();
        assert_eq!(where_now["user_edits_undone"], true, "{where_now}");
        assert!(human::render("where", &where_now).contains("recto redo"), "{where_now}");
        assert_eq!(
            run(&mut s, Level::ReadOnly, &Command::from(crate::commands::reading::Sidebar {})).unwrap()["undone_owner_steps"],
            true
        );
        assert_eq!(s.close(id), Err(SessionError::UnsavedChanges), "the app's route refuses too");
        assert_eq!(
            run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() }))
                .unwrap_err()
                .code,
            "would_clear_redo"
        );
        let closed = run(&mut s, Level::Full, &Command::from(crate::commands::tabs::Close { tab: tab() })).unwrap();
        assert_eq!(closed["count"], 0, "at Full it closes, as it did: {closed}");
    }

    /// Failure: an agent that does `page move`, `undo`, then anything else is refused and has to ask for Full to clear up after itself. A blind agent got stuck exactly here.
    #[test]
    fn an_agent_can_throw_away_its_own_edits() {
        let mut s = edit_session();
        let id = TabId(1);
        let tab = || "#1".to_string();
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageMove {
                tab: tab(),
                pages: "3".into(),
                to: 1,
            }),
        )
        .unwrap();
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageDelete { tab: tab(), pages: "2".into() }),
        )
        .expect("a new edit may clear its own undone step");
        assert!(!s.lists().get(id).unwrap().can_redo());
        // A close with unsaved edits stops, and says the way on, and that way works at Control.
        let close = run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).unwrap_err();
        assert_eq!((close.code, close.exit), ("unsaved_changes", exit::FAILED));
        assert!(
            close.next.contains("recto discard") && !close.next.contains("Full") && close.ask.is_none(),
            "{}",
            close.next
        );
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Discard { tab: tab() })).expect("its own edits are its own to throw away");
        assert!(!s.lists().is_edited(id));
        // Undone, with nothing unsaved: it can close, and lose only that.
        run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageRotate {
                tab: tab(),
                pages: "1".into(),
                degrees: 90,
            }),
        )
        .unwrap();
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();
        assert!(s.lists().get(id).unwrap().can_redo());
        run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).expect("nothing of the user's is at stake");
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
        assert_eq!(
            said(
                &mut s,
                Command::from(crate::commands::editing::PageRotate {
                    tab: tab(),
                    pages: "4".into(),
                    degrees: 270
                })
            )["did"],
            "Rotated page 4 by 270 degrees."
        );
        assert_eq!(
            said(
                &mut s,
                Command::from(crate::commands::editing::PageRotate {
                    tab: tab(),
                    pages: "1-2".into(),
                    degrees: -90
                })
            )["did"],
            "Rotated pages 1-2 by -90 degrees."
        );
        assert_eq!(
            said(
                &mut s,
                Command::from(crate::commands::editing::PageMove {
                    tab: tab(),
                    pages: "3".into(),
                    to: 1
                })
            )["did"],
            "Moved page 3 to 1."
        );
        assert_eq!(
            said(
                &mut s,
                Command::from(crate::commands::editing::PageMove {
                    tab: tab(),
                    pages: "4-5".into(),
                    to: 7
                })
            )["did"],
            "Moved pages 4-5 to 7."
        );
        assert_eq!(
            said(&mut s, Command::from(crate::commands::editing::PageDelete { tab: tab(), pages: "5".into() }))["did"],
            "Deleted page 5."
        );
        assert_eq!(
            said(
                &mut s,
                Command::from(crate::commands::editing::PageInsert {
                    tab: tab(),
                    path: "/d/b.pdf".into(),
                    at: 2
                })
            )["did"],
            "Added 10 pages at page 2."
        );
        assert_eq!(
            said(&mut s, Command::from(crate::commands::editing::Undo { tab: tab() }))["did"],
            "Undid the last edit."
        );
        assert_eq!(
            said(&mut s, Command::from(crate::commands::editing::Redo { tab: tab() }))["did"],
            "Redid the last undone edit."
        );
        let shown = human::render(
            "page_delete",
            &run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageDelete {
                    tab: tab(),
                    pages: "2-3".into(),
                }),
            )
            .unwrap(),
        );
        assert!(shown.starts_with("Deleted pages 2-3. 17 pages now, 10 in the file."), "{shown}");
        let tabs = run(
            &mut s,
            Level::ReadOnly,
            &Command::from(crate::commands::reading::Tabs { from: None, limit: None }),
        )
        .unwrap();
        assert_eq!((&tabs["tabs"][0]["pages"], &tabs["tabs"][0]["file_pages"]), (&json!(17), &json!(10)), "{tabs}");
        assert_eq!(
            said(&mut s, Command::from(crate::commands::editing::Discard { tab: tab() }))["did"],
            "Threw away this tab's edits."
        );
        assert_eq!(
            said(&mut s, Command::from(crate::commands::editing::Discard { tab: tab() }))["did"],
            "There were no edits to throw away."
        );
        let nothing = said(
            &mut s,
            Command::from(crate::commands::editing::PageMove {
                tab: tab(),
                pages: "3".into(),
                to: 3,
            }),
        );
        assert_eq!(
            (&nothing["did"], &nothing["edited"], &nothing["saved"]),
            (&json!("Page 3 is already at 3, so nothing changed."), &json!(false), &json!(true))
        );
        assert_eq!(
            said(
                &mut s,
                Command::from(crate::commands::editing::PageMove {
                    tab: tab(),
                    pages: "3-4".into(),
                    to: 3
                })
            )["did"],
            "Pages 3-4 are already at 3, so nothing changed."
        );
        assert!(
            run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).is_ok(),
            "and so it closes"
        );
    }

    /// Failure: an edit through the agent interface is recorded as the user's, so it's protected from the agent that made it, or recorded as nobody's. Every edit command must be an agent's, however it gets to the session.
    #[test]
    fn every_edit_through_the_agent_interface_is_an_agents() {
        let mut s = edit_session();
        let tab = || "#1".to_string();
        for command in [
            Command::from(crate::commands::editing::PageRotate {
                tab: tab(),
                pages: "1".into(),
                degrees: 90,
            }),
            Command::from(crate::commands::editing::PageMove {
                tab: tab(),
                pages: "1".into(),
                to: 2,
            }),
            Command::from(crate::commands::editing::PageDelete { tab: tab(), pages: "1".into() }),
            Command::from(crate::commands::editing::PageInsert {
                tab: tab(),
                path: "/d/b.pdf".into(),
                at: 1,
            }),
        ] {
            run(&mut s, Level::Control, &command).unwrap();
            assert!(!s.lists().get(TabId(1)).unwrap().holds_owner_edits(), "{}", command.tag());
        }
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Redo { tab: tab() })).unwrap();
        assert!(!s.lists().get(TabId(1)).unwrap().holds_owner_edits(), "undo and redo don't change that");
    }

    /// Failure: the person's edit through a command is recorded as an agent's, so an agent below Full may throw it away, undo it past them or lose it with a new
    /// edit; or the person's undo of an agent's step counts as the agent's. The same commands as the window sends, run as the person, are the owner's.
    #[test]
    fn every_edit_by_a_person_is_the_owners() {
        let tab = || "#1".to_string();
        let commands = [
            Command::from(crate::commands::editing::PageRotate {
                tab: tab(),
                pages: "all".into(),
                degrees: 90,
            }),
            Command::from(crate::commands::editing::PageMove {
                tab: tab(),
                pages: "1".into(),
                to: 2,
            }),
            Command::from(crate::commands::editing::PageDelete { tab: tab(), pages: "1".into() }),
            Command::from(crate::commands::editing::PageInsert {
                tab: tab(),
                path: "/d/b.pdf".into(),
                at: 1,
            }),
        ];
        for command in commands {
            let mut s = edit_session();
            execute(&Direct(RefCell::new(&mut s), None), Actor::Person, &command, &mut Vec::new()).unwrap();
            assert!(
                s.lists().get(TabId(1)).unwrap().holds_owner_edits(),
                "{} by the person is the owner's",
                command.tag()
            );
            // An agent below Full can't throw it away, as it couldn't one made through the window's own call.
            let discard = Command::from(crate::commands::editing::Discard { tab: tab() });
            assert_eq!(
                run(&mut s, Level::Control, &discard).unwrap_err().code,
                "would_discard_user_edits",
                "{}",
                command.tag()
            );
        }
        // An agent's step, undone and redone by the person: the person's undo is theirs, and leaves the agent's step the agent's.
        let mut s = edit_session();
        run(&mut s, Level::Control, &commands_rotate(tab())).unwrap();
        for step in [
            Command::from(crate::commands::editing::Undo { tab: tab() }),
            Command::from(crate::commands::editing::Redo { tab: tab() }),
        ] {
            execute(&Direct(RefCell::new(&mut s), None), Actor::Person, &step, &mut Vec::new()).unwrap();
        }
        assert!(!s.lists().get(TabId(1)).unwrap().holds_owner_edits(), "the agent's step stays the agent's");
    }

    fn commands_rotate(tab: String) -> Command {
        Command::from(crate::commands::editing::PageRotate {
            tab,
            pages: "1".into(),
            degrees: 90,
        })
    }

    /// Failure: an agent makes enough edits that the user's step falls off the capped list, and then its discard counts only what's left on the list, and throws away the user's edit, still in the page order, along with its own. And the reverse: an agent's step falling off must not stop it clearing up.
    #[test]
    fn a_step_past_the_undo_cap_still_counts_for_discard() {
        let mut s = edit_session();
        let tab = || "#1".to_string();
        owners_rotation(&mut s, 0);
        for _ in 0..recto_session::edit::MAX_UNDO - 1 {
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::editing::PageMove {
                    tab: tab(),
                    pages: "2".into(),
                    to: 3,
                }),
            )
            .unwrap();
        }
        // Below Full, the owner's step at the bottom can't be pushed out, and it says why. That's the gap in the plan: an agent editing past the cap so what was done can't be undone.
        let refused = run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::editing::PageMove {
                tab: tab(),
                pages: "2".into(),
                to: 3,
            }),
        )
        .unwrap_err();
        assert_eq!(refused.code, "history_full");
        assert!(!s.lists().get(TabId(1)).unwrap().history_dropped(), "and it's still there");
        // At Full it's the holder's call.
        run(
            &mut s,
            Level::Full,
            &Command::from(crate::commands::editing::PageMove {
                tab: tab(),
                pages: "2".into(),
                to: 3,
            }),
        )
        .unwrap();
        assert_eq!(
            run(&mut s, Level::Control, &Command::from(crate::commands::editing::Discard { tab: tab() }))
                .unwrap_err()
                .code,
            "would_discard_user_edits"
        );
        assert_eq!(turned_pages(&s)[0], 1, "the user's edit is still there");
        // The cap isn't an agent erasing anything: all the agent's steps can come off, and undo isn't refused.
        while run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).is_ok() {}
        assert!(s.lists().is_edited(TabId(1)), "it still differs from the file, and says so");
        let close = run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).unwrap_err();
        assert!(
            close.next.contains("past the undo limit"),
            "what it can't do must not be offered: {}",
            close.next
        );
        let mut own = edit_session();
        for _ in 0..=recto_session::edit::MAX_UNDO {
            run(
                &mut own,
                Level::Control,
                &Command::from(crate::commands::editing::PageMove {
                    tab: tab(),
                    pages: "2".into(),
                    to: 3,
                }),
            )
            .unwrap();
        }
        assert!(own.lists().get(TabId(1)).unwrap().history_dropped());
        run(&mut own, Level::Control, &Command::from(crate::commands::editing::Discard { tab: tab() })).expect("all of it is its own");
        assert!(run(&mut own, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).is_ok());
    }

    /// Failure: the refusal mentions only undone steps, so an agent with unsaved edits runs `redo` as told and brings back a delete it had backed out of, thinking that clears the way. It must name all that's at stake and each honest way on, including that saving doesn't exist. Nothing here may loosen the rule.
    #[test]
    fn a_refused_close_says_all_it_would_lose_and_every_way_on() {
        let mut s = edit_session();
        let tab = || "#1".to_string();
        owners_rotation(&mut s, 0);
        owners_rotation(&mut s, 1);
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();

        let both = run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).unwrap_err();
        assert_eq!(
            (both.code, both.exit),
            ("would_clear_redo", exit::PERMISSION),
            "below Full, this is still that refusal"
        );
        assert_eq!(both.ask.as_deref(), Some(ASK_FULL));
        assert!(
            both.message.contains("unsaved edits and undone steps") && both.message.contains("user's"),
            "{}",
            both.message
        );
        for must in [
            "recto redo",
            "anything you undid on purpose",
            "recto undo",
            "recto discard",
            "needs Full",
            "recto save",
            "Leave the tab open",
        ] {
            assert!(both.next.contains(must), "{must}: {}", both.next);
        }

        let at_full = run(&mut s, Level::Full, &Command::from(crate::commands::tabs::Close { tab: tab() })).unwrap_err();
        assert_eq!(
            (at_full.code, at_full.exit),
            ("unsaved_changes", exit::FAILED),
            "unsaved edits stop a close at any level"
        );
        assert!(!at_full.next.contains("needs Full"), "{}", at_full.next);

        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Undo { tab: tab() })).unwrap();
        let only_undone = run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).unwrap_err();
        assert_eq!(only_undone.code, "would_clear_redo");
        assert!(!only_undone.message.contains("unsaved"), "nothing's unsaved now: {}", only_undone.message);

        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Redo { tab: tab() })).unwrap();
        run(&mut s, Level::Control, &Command::from(crate::commands::editing::Redo { tab: tab() })).unwrap();
        let only_unsaved = run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Close { tab: tab() })).unwrap_err();
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
                Ok(DocFacts {
                    page_count: 1,
                    byte_len: 1,
                    encrypted: true,
                    version: "1.7".into(),
                    title: None,
                    toc: Toc::default(),
                    renderer_only: false,
                })
            }
        }
        let mut s = Session::new(Encrypted);
        s.open(&PathBuf::from("/d/locked.pdf")).unwrap();
        let facts = run(
            &mut s,
            Level::ReadOnly,
            &Command::from(crate::commands::reading::Facts { tab: "active".into() }),
        )
        .unwrap();
        assert!(facts["encryption_note"].as_str().unwrap().contains("no password"), "{facts}");
        assert!(human::render("facts", &facts).contains("no password"), "{facts}");
        let mut plain = with_tabs(&["a"]);
        assert!(
            run(
                &mut plain,
                Level::ReadOnly,
                &Command::from(crate::commands::reading::Facts { tab: "active".into() })
            )
            .unwrap()["encryption_note"]
                .is_null()
        );
    }

    /// Failure: the log says "by name", so nobody can tell afterwards which tab an agent acted on. It keeps the `#id`, never the name, and `open` gives its own.
    #[test]
    fn the_log_says_which_tab_by_id_and_never_by_name() {
        let mut s = with_tabs(&["secret report", "other"]);
        let by_name = Command::from(crate::commands::reading::Facts { tab: "secret".into() });
        let acted = acted_tab(&s, &by_name);
        assert_eq!(acted, Some(TabId(1)));
        let details = with_acted_tab(by_name.log_details(), acted, &Ok(json!({})), &by_name);
        assert_eq!(details["tab"], "#1");
        assert!(!details.to_string().contains("secret"), "{details}");
        let active = Command::from(crate::commands::text::Text { tab: "active".into(), page: 1 });
        assert_eq!(acted_tab(&s, &active), s.tabs().active());
        let nothing = Command::from(crate::commands::reading::Facts { tab: "nothing like it".into() });
        let details = with_acted_tab(nothing.log_details(), acted_tab(&s, &nothing), &Ok(json!({})), &nothing);
        assert_eq!(details["tab"], "by name", "{details}");
        assert!(!details.to_string().contains("nothing like it"), "{details}");
        let mut alike = with_tabs(&["report one", "report two"]);
        let ambiguous = Command::from(crate::commands::reading::Facts { tab: "report".into() });
        let acted = acted_tab(&alike, &ambiguous);
        assert_eq!(acted, None);
        let refused = run(&mut alike, Level::ReadOnly, &ambiguous);
        assert_eq!(
            with_acted_tab(ambiguous.log_details(), acted, &refused, &ambiguous)["tab"],
            "by name",
            "two tabs matched, and saying there's no such tab would be false"
        );
        let open = Command::from(crate::commands::tabs::Open {
            path: "/d/new.pdf".into(),
            page: None,
        });
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
        use recto_render_service::{RenderFailure, WallsDown};
        let unsandboxed = AgentError::from(RenderFailure::WallsDown(WallsDown::NotSandboxed));
        assert!(unsandboxed.message.contains("sandbox"), "{}", unsandboxed.message);
        assert!(unsandboxed.next.contains("signed wrong"), "{}", unsandboxed.next);
        // Only a helper signed without its sandbox is the install's fault. The rest are about where it runs, and must never send the user to reinstall.
        for failure in [
            RenderFailure::WallsDown(WallsDown::InsideAnotherSandbox),
            RenderFailure::WallsDown(WallsDown::NothingToDrop),
            RenderFailure::WallsDown(WallsDown::LaunchdReachable),
            RenderFailure::StartRefused,
        ] {
            let error = AgentError::from(failure);
            assert!(
                !error.message.contains("signed wrong") && !error.next.contains("signed wrong"),
                "{failure:?}: {} / {}",
                error.message,
                error.next
            );
            assert!(
                !error.next.starts_with("`recto status`"),
                "{failure:?} has no next step of its own: {}",
                error.next
            );
        }
        assert!(
            AgentError::from(RenderFailure::WallsDown(WallsDown::InsideAnotherSandbox))
                .message
                .contains("from inside another sandbox")
        );
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
        let error = run(
            &mut s,
            Level::Control,
            &Command::from(crate::commands::linking::Goto {
                tab: "#1".into(),
                page: Some(11),
                label: None,
                within: None,
            }),
        )
        .unwrap_err();
        assert_eq!((error.code, error.exit), ("no_such_page", exit::NOT_FOUND));
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::linking::Goto {
                    tab: "#1".into(),
                    page: Some(0),
                    label: None,
                    within: None,
                })
            )
            .unwrap_err()
            .code,
            "no_such_page"
        );
        assert_eq!(
            run(
                &mut s,
                Level::Control,
                &Command::from(crate::commands::linking::Goto {
                    tab: "#1".into(),
                    page: Some(10),
                    label: None,
                    within: None,
                })
            )
            .unwrap()["page"],
            10
        );
    }

    /// Failure: `active` with nothing open reads as an internal error, rather than saying to open something.
    #[test]
    fn no_active_tab_says_to_open_one() {
        let mut s = Session::new(Fake);
        let error = run(
            &mut s,
            Level::ReadOnly,
            &Command::from(crate::commands::reading::Where { tab: "active".into() }),
        )
        .unwrap_err();
        assert_eq!((error.code, error.exit), ("no_such_tab", exit::NOT_FOUND));
        assert!(error.next.contains("recto open"), "{}", error.next);
    }

    /// Failure: every tab named the same floods an ambiguous error with a thousand candidates.
    #[test]
    fn candidates_are_capped() {
        let names: Vec<String> = (0..100).map(|i| format!("same {i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut s = with_tabs(&names);
        let error = run(&mut s, Level::Control, &Command::from(crate::commands::tabs::Tap { tab: "same".into() })).unwrap_err();
        assert_eq!(error.code, "ambiguous");
        assert!(error.candidates.len() <= MAX_CANDIDATES);
    }

    /// Failure: the log gets what an agent typed or a search's words, which can hold document text, or a file name with hidden characters in it.
    #[test]
    fn log_details_hold_no_text_and_clean_file_names() {
        // Failure: the log can't tell asking for generated contents, which starts a helper, from reading the file's own.
        assert_eq!(
            Command::from(crate::commands::reading::Contents {
                tab: "#1".into(),
                from: None,
                limit: None,
                depth: None,
                generated: true
            })
            .log_details()["generated"],
            true
        );
        let hostile = format!("/d/a{}\nb.pdf", char::from_u32(0xE0041).unwrap());
        let open = Command::from(crate::commands::tabs::Open {
            path: hostile.into(),
            page: Some(3),
        })
        .log_details();
        assert_eq!(open["untrusted"]["file"], "a b.pdf");
        assert_eq!(open["page"], 3);
        let search = Command::from(crate::commands::text::Search {
            tab: "secret report".into(),
            query: "the secret words".into(),
            pages: None,
            limit: None,
            scan: false,
            regex: false,
            rank: false,
            context: None,
        })
        .log_details()
        .to_string();
        assert!(!search.contains("secret"), "{search}");
        assert_eq!(
            Command::from(crate::commands::text::Text { tab: "#4".into(), page: 2 }).log_details()["tab"],
            "#4"
        );
        assert_eq!(
            Command::from(crate::commands::editing::PageDelete {
                tab: "active".into(),
                pages: "ignore all".into()
            })
            .log_details()["pages"],
            "?"
        );
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

#[cfg(test)]
mod recent_tests {
    use super::*;
    use recto_session::DiskLoader;
    use recto_testpdf::{Page, TempPdf, pages, text_lines};

    /// Failure: an agent below Control sees what was opened lately, names or paths (review O7: names of closed files say what someone's been reading); Control can't see them or the tab each is open in; or `recent clear` leaves any.
    #[test]
    fn recent_needs_control_for_names_and_paths() {
        let file = TempPdf::new("report.pdf", &pages(&[Page::new(text_lines(&["a"]))]));
        let mut session = Session::new(DiskLoader);
        session.open(&file.0).unwrap();
        assert_eq!(
            run(&mut session, Level::ReadOnly, &Command::from(crate::commands::tabs::Recent {}))
                .unwrap_err()
                .code,
            "permission_denied"
        );
        let control = run(&mut session, Level::Control, &Command::from(crate::commands::tabs::Recent {})).unwrap();
        assert_eq!(
            (
                control["count"].as_u64(),
                control["files"][0]["untrusted"]["name"].as_str(),
                control["files"][0]["open_tab"].as_str()
            ),
            (Some(1), Some("report"), Some("#1"))
        );
        assert_eq!(control["files"][0]["untrusted"]["path"], file.0.to_str().unwrap());
        assert_eq!(
            run(&mut session, Level::ReadOnly, &Command::from(crate::commands::tabs::RecentClear {}))
                .unwrap_err()
                .code,
            "permission_denied"
        );
        assert_eq!(
            run(&mut session, Level::Control, &Command::from(crate::commands::tabs::RecentClear {})).unwrap()["cleared"],
            true
        );
        assert_eq!(
            run(&mut session, Level::Control, &Command::from(crate::commands::tabs::Recent {})).unwrap()["count"],
            0
        );
    }
}

#[cfg(test)]
mod protected_tests {
    use super::*;
    use recto_core::{DocFacts, NONE, OpenError, Toc, TocLine};
    use std::path::Path;

    /// A file whose facts were read with a password: its contents name a chapter nothing else would.
    #[derive(Clone)]
    struct Unlocked;

    impl DocLoader for Unlocked {
        fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
            if path.ends_with("plain.pdf") {
                return Ok(DocFacts {
                    page_count: 3,
                    byte_len: 1,
                    encrypted: false,
                    version: "1.7".into(),
                    title: None,
                    toc: Toc::default(),
                    renderer_only: false,
                });
            }
            let lines = [TocLine {
                title: "Secret chapter",
                page: 1,
                parent: NONE,
                depth: 0,
            }];
            Ok(DocFacts {
                page_count: 7,
                byte_len: 1,
                encrypted: true,
                version: "1.7".into(),
                title: Some("Secret title".into()),
                toc: Toc::from_lines(&lines, false),
                renderer_only: false,
            })
        }
    }

    /// Failure (review O1): once the person types a protected PDF's password, what was decrypted reaches an agent: its chapter titles in `sidebar`, its length or place in `where`, `tabs`, `session show` or a comparison, its pages through `goto`, `goto label`, `links`, `follow`, `back`, `forward`, `wait` or an open with `--page`, Find's hits through `find`, or anything from `contents`, `facts`, `search` or `text`, at any level. `encrypted` isn't the gate, as a file anyone may open is encrypted too.
    #[test]
    fn nothing_decrypted_reaches_an_agent_at_any_level() {
        let mut session = Session::new(Unlocked);
        let id = session.open(Path::new("/d/locked.pdf")).unwrap();
        let other = session.open(Path::new("/d/plain.pdf")).unwrap();
        session.tabs_mut().note_password_used(id);
        session.tap(id).unwrap();
        assert!(session.tabs().withheld_from_agents(id) && !session.tabs().withheld_from_agents(other));
        assert!(
            session.sidebar().rows.iter().any(|row| row.text == "Secret chapter"),
            "the person's sidebar has it, so there's something to keep from agents"
        );
        let leaks = |value: &Value| {
            let text = value.to_string();
            text.contains("Secret chapter") || text.contains("Secret title") || text.contains("\"pages\":7") || text.contains("of 7")
        };
        let tab = "#1".to_string();
        for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
            let commands = [
                Command::from(crate::commands::reading::Sidebar {}),
                Command::from(crate::commands::reading::Tabs { from: None, limit: None }),
                Command::from(crate::commands::reading::Where { tab: tab.clone() }),
                Command::from(crate::commands::reading::Facts { tab: tab.clone() }),
                Command::from(crate::commands::reading::Contents {
                    tab: tab.clone(),
                    from: None,
                    limit: None,
                    depth: None,
                    generated: false,
                }),
                Command::from(crate::commands::text::Search {
                    tab: tab.clone(),
                    query: "Secret".into(),
                    pages: None,
                    limit: None,
                    scan: false,
                    regex: false,
                    rank: false,
                    context: None,
                }),
                Command::from(crate::commands::text::Text { tab: tab.clone(), page: 1 }),
                Command::from(crate::commands::linking::Goto {
                    tab: tab.clone(),
                    page: Some(2),
                    label: None,
                    within: None,
                }),
                Command::from(crate::commands::linking::Goto {
                    tab: tab.clone(),
                    page: None,
                    label: Some("ii".into()),
                    within: None,
                }),
                Command::from(crate::commands::linking::Links { tab: tab.clone(), page: 1 }),
                Command::from(crate::commands::linking::Follow {
                    tab: tab.clone(),
                    page: 1,
                    link: 1,
                }),
                Command::from(crate::commands::linking::Back { tab: tab.clone() }),
                Command::from(crate::commands::linking::Forward { tab: tab.clone() }),
                Command::from(crate::commands::finding::Find {
                    query: Some("Secret".into()),
                    ..Default::default()
                }),
                Command::from(crate::commands::finding::Find {
                    next: true,
                    ..Default::default()
                }),
                Command::from(crate::commands::finding::Find::default()),
                // The forms C5g added: a step as a jump, a hit by its index, and opening Find, which finds the draft again.
                Command::from(crate::commands::finding::Find {
                    next: true,
                    jump: true,
                    ..Default::default()
                }),
                Command::from(crate::commands::finding::Find {
                    hit: Some(0),
                    ..Default::default()
                }),
                Command::from(crate::commands::finding::Find {
                    open: true,
                    ..Default::default()
                }),
                Command::from(crate::commands::editing::Edits { tab: tab.clone() }),
                Command::from(crate::commands::reading::Signatures { tab: tab.clone() }),
                Command::from(crate::commands::tabs::Open {
                    path: "/d/locked.pdf".into(),
                    page: Some(7),
                }),
                Command::from(crate::commands::tabs::Open {
                    path: "/d/locked.pdf".into(),
                    page: None,
                }),
                Command::from(crate::commands::compare::Compare {
                    tabs: Some("#1,#2".into()),
                    sync: None,
                    off: false,
                    toggle: None,
                    reading: None,
                }),
                // The protected tab's pane read, and it out and in again, as the window's own moves are (R2.3, C5e): said as protected, as `--tabs` is.
                Command::from(crate::commands::compare::Compare {
                    reading: Some("#1".into()),
                    ..Default::default()
                }),
                Command::from(crate::commands::compare::Compare {
                    toggle: Some("#1".into()),
                    ..Default::default()
                }),
                Command::from(crate::commands::compare::Compare {
                    toggle: Some("#1".into()),
                    ..Default::default()
                }),
            ];
            for command in &commands {
                match run(&mut session, level, command) {
                    Ok(data) => assert!(!leaks(&data), "{} at {level:?} gave {data}", command.tag()),
                    Err(error) => {
                        assert!(
                            error.exit == exit::PROTECTED || error.exit == exit::PERMISSION,
                            "{} at {level:?}: {error:?}",
                            command.tag()
                        );
                        assert!(!error.message.contains("Secret"), "{error:?}");
                    }
                }
            }
            if level >= Level::ReadOnly {
                for command in [
                    Command::from(crate::commands::reading::Facts { tab: tab.clone() }),
                    Command::from(crate::commands::reading::Contents {
                        tab: tab.clone(),
                        from: None,
                        limit: None,
                        depth: None,
                        generated: false,
                    }),
                    Command::from(crate::commands::text::Text { tab: tab.clone(), page: 1 }),
                    Command::from(crate::commands::reading::Where { tab: tab.clone() }),
                ] {
                    let result = run(&mut session, level, &command);
                    if command.tag() == "where" {
                        assert_eq!(result.unwrap()["protected"], true, "said, not just left blank");
                    } else {
                        assert_eq!(
                            result.unwrap_err().code,
                            "protected",
                            "{} at {level:?} is refused as protected, not as something else",
                            command.tag()
                        );
                    }
                }
            }
            let _ = run(
                &mut session,
                Level::Full,
                &Command::from(crate::commands::compare::Compare {
                    tabs: None,
                    sync: None,
                    off: true,
                    toggle: None,
                    reading: None,
                }),
            );
            if session.tabs().active() != Some(id) {
                session.tap(id).unwrap();
            }
        }
        let shown = session_show_json(&session, Level::Full, Value::Null);
        assert!(!leaks(&shown), "{shown}");
        assert!(!leaks(&page_changed(&session, id, 3).to_json()) && !leaks(&edits_changed(&session, id).to_json()));
        assert_eq!(page_changed(&session, other, 3).to_json()["page"], 3, "another tab's place is said as ever");
        let access = Direct(std::cell::RefCell::new(&mut session), None);
        assert!(matches!(check_condition(&access, Condition::PageDrawn, "#1", Some(1)), ConditionState::Failed(error) if error.code == "protected"));
        assert!(matches!(check_condition(&access, Condition::TabOpen, "#1", None), ConditionState::Met(data) if !leaks(&data)));
    }
}

#[cfg(test)]
mod linking_tests {
    use super::*;
    use recto_session::DiskLoader;
    use recto_testpdf::{TempPdf, linked_document};

    fn opened(file: &TempPdf) -> Session<DiskLoader> {
        let mut session = Session::new(DiskLoader);
        session.open(&file.0).unwrap();
        session
    }

    fn line(text: &str) -> Command {
        let args: Vec<String> = text.split(' ').map(str::to_string).collect();
        parse(&table::parse_cli(&args).unwrap().wire()).unwrap()
    }

    /// Failure: an agent can't see where a page's links go, or sees a script, a program or a file as something to follow; or an address reaches it outside `untrusted`.
    #[test]
    fn links_say_where_each_goes() {
        let file = TempPdf::new("links.pdf", &linked_document());
        let mut session = opened(&file);
        let answer = run(&mut session, Level::ReadOnly, &line("links 1")).unwrap();
        let links = answer["links"].as_array().unwrap();
        let goes: Vec<&str> = links.iter().map(|link| link["goes_to"].as_str().unwrap()).collect();
        assert_eq!(goes, ["page", "page", "web", "refused", "refused", "refused", "page", "nowhere"]);
        assert_eq!(links[0]["page"], 9);
        assert_eq!(links[0]["untrusted"]["label"], "5", "the ninth page is called 5");
        assert_eq!(links[2]["untrusted"]["address"], "https://example.com/report?id=7");
        assert!(links[2].get("address").is_none(), "an address only ever under untrusted");
        assert_eq!([&links[3]["refused"], &links[4]["refused"], &links[5]["refused"]], ["script", "launch", "file"]);
        assert!(human::render("links", &answer).contains("only the user can open"));
    }

    /// Failure (PLAN, links follow-ups): on a page with an unsaved quarter turn, `links` gives only the box where the file has it, so an agent looking at the page as the reader sees it points at the wrong place. Each link then has `shown_rect` too, turned with the page, and the answer says by how much.
    #[test]
    fn links_on_a_turned_page_say_where_they_are_as_its_seen() {
        let file = TempPdf::new("links.pdf", &linked_document());
        let mut session = opened(&file);
        let flat = run(&mut session, Level::ReadOnly, &line("links 1")).unwrap();
        assert!(flat.get("quarter_turns").is_none() && flat["links"][0].get("shown_rect").is_none(), "{flat}");
        run(&mut session, Level::Full, &line("page rotate 1 --by 90")).unwrap();
        let turned = run(&mut session, Level::ReadOnly, &line("links 1")).unwrap();
        assert_eq!(turned["quarter_turns"], 1, "{turned}");
        let rect: Vec<f64> = turned["links"][0]["rect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_f64().unwrap())
            .collect();
        let shown: Vec<f64> = turned["links"][0]["shown_rect"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_f64().unwrap())
            .collect();
        assert_eq!(
            rect,
            flat["links"][0]["rect"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_f64().unwrap())
                .collect::<Vec<_>>(),
            "the file's box stays"
        );
        // A quarter turn clockwise of a 612 x 792 page: x is 792 less the bottom, y is the old x, and the box is as tall as it was wide.
        assert_eq!(shown, [792.0 - (rect[1] + rect[3]), rect[0], rect[3], rect[2]], "{turned}");
    }

    /// Failure: following a link doesn't leave a way back, `back` and `forward` don't retrace it, `goto label` misses what the file calls a page, or an agent can open a web address by following its link.
    #[test]
    fn follow_back_forward_and_labels() {
        let file = TempPdf::new("links.pdf", &linked_document());
        let mut session = opened(&file);
        let went = run(&mut session, Level::Control, &line("follow --page 1 --link 1")).unwrap();
        assert_eq!(
            (went["page"].as_u64(), went["went"].as_str(), went["can_go_back"].as_bool()),
            (Some(9), Some("page"), Some(true))
        );
        assert_eq!(went["untrusted"]["label"], "5");
        let back = run(&mut session, Level::Control, &line("back")).unwrap();
        assert_eq!((back["page"].as_u64(), back["can_go_forward"].as_bool()), (Some(1), Some(true)));
        assert_eq!(run(&mut session, Level::Control, &line("forward")).unwrap()["page"], 9);
        assert_eq!(run(&mut session, Level::Control, &line("forward")).unwrap_err().code, "nothing_forward");
        let labelled = run(&mut session, Level::Control, &line("goto label A-2")).unwrap();
        assert_eq!(labelled["page"], 12);
        assert_eq!(run(&mut session, Level::Control, &line("goto label xx")).unwrap_err().code, "no_such_label");
        let web = run(&mut session, Level::Full, &line("follow --page 1 --link 3")).unwrap_err();
        assert_eq!((web.code, web.exit), ("web_link_needs_user", exit::PERMISSION), "not at any level");
        assert_eq!(
            web.untrusted.as_deref().map(|value| value["address"].clone()),
            Some(json!("https://example.com/report?id=7"))
        );
        assert_eq!(
            run(&mut session, Level::Full, &line("follow --page 1 --link 4")).unwrap_err().code,
            "link_refused"
        );
        assert_eq!(
            run(&mut session, Level::ReadOnly, &line("back")).unwrap_err().code,
            "permission_denied",
            "moving about is Control"
        );
        assert_eq!(
            run(&mut session, Level::Control, &line("follow --page 1 --link 99")).unwrap_err().code,
            "no_such_link"
        );
    }
}
