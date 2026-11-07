//! The bridge to the host, with no rules in it. The host draws what comes out of here and calls these for everything the user does.
//!
//! **Every call is cheap or returns at once.** Anything that can take time goes on the session thread, which is parked at rest, and the host hears when there's something new. Pixels never cross this: tiles travel as surfaces.
//! The session here is the one on the control socket, so `recto tabs` and `recto goto` act on what's on screen. The app's own actions go through `with_session`, as the owner's.

#![allow(unsafe_code)] // What UniFFI's macros generate. Nothing here writes any of its own.

mod menus;
mod qa;
mod saver;
mod sleeper;
pub mod updates;
mod watchdog;

use qa::QaBridge;
use recto_core::{DocFacts, OpenError};
use recto_platform::FileStamp;
use recto_agent::{AgentError, Command, Condition, ConditionState, DrawableLoader, Event, Level};
use recto_control::StatePaths;
use recto_control::host::{Host, SessionHost, kept_json, store_error};
use recto_control::indexing::Notifier;
use recto_control::record_store::{Flavor, FlushReport, Loaded, RecordStore, notice_for};
use recto_control::server::{Announcer, Limits, Server, Stopper};
use recto_reader::engine::EngineEvents;
use recto_reader::tiles::{MAX_ZOOM, MIN_ZOOM, Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_render_service::Policy;
use recto_reader::tiles::SavedPlace;
use recto_session::HistorySource;
use recto_session::record::{self, Appearance, AppRecord, ContentsScroll, FileStatus, FocusRecord, FocusView, Rect, ScreenNow, ScreenRecord, SettingsWindow, WindowRecord, look_at_file, place_window};
use recto_session::{CONTENTS_ROW_HEIGHT, ContentsFollow, ContentsRowChange, ContentsView, DocLoader, FollowMove, WARM_PAPER, row_full_text, DrawNote, Paper, RowKind as SessionRowKind, SessionError, TAB_ROW_HEIGHT, TabId, ViewSettings, sidebar_heights};
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use saver::Saver;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
pub use menus::{HostAction, MenuEntry, MenuModel, PanelItem, PanelModel, PanelSection};
use watchdog::Watchdog;

uniffi::setup_scaffolding!();

/// Where the chosen paper is remembered, in the state folder.
const PAPER_FILE: &str = "paper";
/// Whether the agent icon is hidden, there too. It's here only if it is.
const AGENT_ICON_FILE: &str = "agent-icon-hidden";

/// How long the window waits for the state to be read. Longer means the state's disk isn't answering, and the app starts without it for now.
const RESTORE_PATIENCE: Duration = Duration::from_secs(1);
/// How long the window waits for the page to be drawn before it shows anyway. About what a heavy page takes.
const SHOW_PATIENCE: Duration = Duration::from_millis(1200);
/// Where a hand-off has got to. Rust's to know, so the app can't quit on one that didn't happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum HandOffState {
    Idle = 0,
    /// Taking no commands, and no user's changes, and writing.
    Writing = 1,
    /// Written, forced to the drive and read back. Quitting now is what's left.
    Done = 2,
}

/// The thread a first frame's news goes on: room to start a worker and draw, and only address space until it's used.
const UPDATE_FRAME_STACK: usize = 1024 * 1024;
/// How long a socket that's taken is tried again, in case what has it is on its way out.
const SOCKET_PATIENCE: Duration = Duration::from_secs(1);
/// How long a quit waits for the state to go out, before asking.
const QUIT_PATIENCE: Duration = Duration::from_secs(5);

/// The most contents rows one call hands over. More than any screen shows.
const MAX_CONTENTS_FETCH: u32 = 256;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the host hears. Called from background threads, and must return at once: the host hops to its UI thread.
#[uniffi::export(with_foreign)]
pub trait AppListener: Send + Sync {
    /// The sidebar or the reader's state changed: pull them.
    fn model_changed(&self);
    /// There are layer changes: call `drain_ops` on the UI thread.
    fn ops_ready(&self);
    /// A QA command, to run on the UI thread as the UI would, and answer with `qa_answer`. `id` zero wants no answer.
    fn qa_request(&self, id: u64, verb: String, value: String);
    /// The session was stopped over the socket.
    fn session_ended(&self);
    /// A copy is ready: call `take_copied` on the UI thread, and put it on the clipboard.
    fn copy_ready(&self);
    /// The state's been read and applied: the window can be made, in the right place.
    fn restore_ready(&self);
    /// What to do about a quit that was asked for, once everything's been written.
    fn quit_answer(&self, answer: QuitAnswer);
    /// A hand-off has written and read back the state, so the app quits now, with no prompt of any kind.
    fn hand_off_quit(&self);
}

/// What comes of asking to quit.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct QuitAnswer {
    /// Go ahead. Otherwise the words below are for a question: quit anyway, or stay.
    pub go: bool,
    pub title: String,
    pub message: String,
    pub quit_button: String,
    pub stay_button: String,
}

/// A screen as the host sees it now.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ScreenInfo {
    pub id: u64,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub visible_x: f64,
    pub visible_y: f64,
    pub visible_width: f64,
    pub visible_height: f64,
}

/// Where a new window goes, as Rust's rule has it.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct WindowPlacement {
    /// There's a place to go: otherwise it's a new window, wherever the host starts one.
    pub saved: bool,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Go into full screen on showing. Only for an app that's come forward: it moves the user to another Space.
    pub full_screen: bool,
    /// Come forward. An app that was in the background when it quit reopens without taking the focus from anyone.
    pub activate: bool,
    pub focus_sidebar: bool,
    /// This is an update arriving with the user doing something else. Nothing goes over what they're looking at, and nothing's made key: the window goes behind everything, and takes it from there when they come to it.
    pub in_background: bool,
    /// And it was in full screen, which it goes back into when they come to it.
    pub full_screen_when_active: bool,
}

/// How it stands for a hand-off.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BusyItem {
    pub code: String,
    pub message: String,
}

/// The cheap look at whether an update could go now, from a thread or the UI thread.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct HandOffCheck {
    pub safe: bool,
    pub busy: Vec<BusyItem>,
    /// Since the user last touched the app.
    pub seconds_since_input: f64,
}

/// Why a hand-off isn't going ahead.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Error)]
pub enum HandOffError {
    /// Something's in flight, and an update should wait.
    Busy { items: Vec<BusyItem> },
    /// The state couldn't be written and confirmed.
    NotSaved { code: String, message: String },
}

impl std::fmt::Display for HandOffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy { items } => write!(f, "{}", items.iter().map(|item| item.message.as_str()).collect::<Vec<_>>().join(" ")),
            Self::NotSaved { message, .. } => f.write_str(message),
        }
    }
}

impl std::error::Error for HandOffError {}

/// How the state's write and read-back went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct HandOffReport {
    pub tabs: u64,
    pub edited_tabs: u64,
    pub record_bytes: u64,
    pub journal_bytes: u64,
    pub wrote_bytes: u64,
    /// How much of the total was making the drive flush.
    pub force_us: u64,
    pub total_us: u64,
}

impl From<FlushReport> for HandOffReport {
    fn from(report: FlushReport) -> Self {
        Self { tabs: report.tabs as u64, edited_tabs: report.edited_tabs as u64, record_bytes: report.record_bytes, journal_bytes: report.journal_bytes, wrote_bytes: report.wrote_bytes, force_us: report.force_us, total_us: report.total_us }
    }
}

#[derive(Debug, uniffi::Error)]
pub enum StartError {
    Refused { message: String },
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for StartError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RowKind {
    Tab,
    Contents,
}

/// A row of the sidebar. At rest it's a name. Its page number shows on hover.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SidebarRow {
    pub kind: RowKind,
    pub tab: u32,
    /// From the file, so it's only ever drawn as text.
    pub text: String,
    pub page: Option<u32>,
    pub depth: u8,
    pub current: bool,
    /// The rail is filled down to here.
    pub rail_filled: bool,
    /// A contents row of the tab that has a rail: the first and last draw its ends.
    pub rail_first: bool,
    pub rail_last: bool,
    /// Something's wrong with this tab's file, in Rust's words.
    pub notice: Option<String>,
}

/// A contents row as it's kept, with no more in it than it needs. Rows are fetched by the host for what's on screen, so a long outline crosses only as far as it's shown.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ContentsEntry {
    tab: u32,
    text: Box<str>,
    page: Option<u32>,
    depth: u8,
}

/// Everything the sidebar draws, but for the contents' rows, which are pulled with `contents_rows` as they come on screen.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SidebarView {
    /// Goes up when the tabs change, which is when to pull them with `sidebar_tabs`. A page crossed doesn't copy them all, with thousands of them, on the UI thread.
    pub tabs_version: u64,
    pub tab_count: u32,
    /// Which tab is the active one, whose contents go under it, as many as `contents_count`.
    pub active_index: Option<u32>,
    pub contents_count: u32,
    /// Goes up by one each time the contents' rows change. What follows says how the list at this version differs from the one before, and stays the same until the next change, so a host a version behind applies it once, a host already there applies nothing, and a host further behind reloads.
    pub contents_version: u64,
    /// The last change closed `contents_removed` rows here and opened `contents_inserted`: a chapter closing as the reader leaves it, and the next one opening.
    pub contents_changed_from: u32,
    pub contents_removed: u32,
    pub contents_inserted: u32,
    /// By how many rows that moved what the reader is looking at, so it can stay where it is on screen.
    pub contents_place_shift: i32,
    /// The contents row of the place being read.
    pub contents_current: Option<u32>,
    pub toc_open: bool,
    /// "32 of 96". Empty with nothing open.
    pub place: String,
    pub edited: bool,
    pub undone_owner_steps: bool,
}

/// Rust's height rule, in points.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct SidebarLayout {
    pub tab_row: f64,
    pub contents_row: f64,
    pub contents_height: f64,
    pub tabs_scroll: bool,
}

/// What to do with the contents' scroll, from Rust's rule about following.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Enum)]
pub enum ContentsMove {
    Stay,
    SetTo { y: f64 },
    /// Run frames, calling `contents_step` on each.
    Move,
}

/// What the page area shows.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ReaderView {
    /// No PDF is open, so it's the home state.
    pub home: bool,
    /// Something the reader needs to be told, such as a page at a lower resolution, in words.
    pub note: Option<String>,
    /// The sidebar's hidden, by its button or the View menu.
    pub sidebar_hidden: bool,
    /// The tab showing's layout, by its name: `single`, `two-up` or `two-up-cover`. Empty with none showing.
    pub layout: String,
    pub place: String,
    /// The paper's name, which QA and the menu use.
    pub paper: String,
    /// The paper's colour, `0xRRGGBB`.
    pub paper_rgb: u32,
    /// Whether the star at the top right is hidden.
    pub agent_icon_hidden: bool,
    /// A colour to multiply over each page in the compositor, for warm paper. No tile changes for it.
    pub page_tint: Option<u32>,
    pub sidebar_width: f64,
    /// Which of the system's selection colours a highlight uses, going by the page and not the window.
    pub highlight_dark_colour: bool,
    /// A highlight lightens the page under it, for a dark page, or multiplies it. See `recto_reader::paper::HighlightLook`.
    pub highlight_lighten: bool,
    /// A line by the page about a copy, going by itself if it says so.
    pub selection_notice: Option<SelectionNotice>,
}

/// What to say by the page about a copy.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SelectionNotice {
    pub text: String,
    /// The button, if there's one: it's `edit.copy_anyway`.
    pub action_title: Option<String>,
    pub fades: bool,
}

/// A piece of a highlight, in the page's own coordinates, which are its layer's.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct HighlightRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A place in the document, in its view's coordinates.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct DocumentPoint {
    pub x: f64,
    pub y: f64,
}

/// What hovering over a row reveals.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RowReveal {
    pub text: String,
    pub is_file_name: bool,
}

/// All the wording the host draws that doesn't come from a document.
#[derive(Clone, Debug, uniffi::Record)]
pub struct AppStrings {
    pub app_name: String,
    pub home_hint: String,
    pub open_button: String,
    pub open_panel_title: String,
    pub add_tooltip: String,
    pub agent_tooltip: String,
    /// The sidebar's button, beside the window's controls.
    pub sidebar_button_tooltip: String,
    /// What goes beside a tab's name when something's wrong with its file. Its notice is what hovering over it says.
    pub notice_mark: String,
}

/// Limits the host applies to native controls. The zoom limits are Rust's, and it says them once.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct ReaderLimits {
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub window_width: f64,
    pub window_height: f64,
    pub sidebar_width: f64,
    /// How long the pointer rests on a shortened name before it opens.
    pub reveal_delay_seconds: f64,
    /// How long the window waits for the state to be read, and then for the page to be drawn, before it shows anyway.
    pub restore_patience_seconds: f64,
    pub show_patience_seconds: f64,
    /// How often a drag past the view's edge scrolls.
    pub autoscroll_every_seconds: f64,
    /// How long a notice that fades stays.
    pub notice_seconds: f64,
}

/// The scrolled view, on each scroll.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct ViewportInput {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub frame_width: f64,
    pub magnification: f64,
    pub backing: f64,
    pub live: bool,
    pub scrolled_to: u64,
}

/// What the host does to its layers, in order.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum ReaderOp {
    Clear,
    Document { width: f64, height: f64 },
    Magnify { value: f64 },
    ScrollTo { y: f64, seq: u64 },
    /// Across, when zoomed in.
    ScrollAcross { x: f64 },
    ShowPage { page: u32, x: f64, y: f64, width: f64, height: f64, fit: f64, paper_rgb: u32 },
    HidePage { page: u32 },
    /// `surface` is an `IOSurfaceRef` the host now owns one reference to, as a number, or zero if `rgb` is the colour.
    Attach { id: u64, page: u32, x: f64, y: f64, width: f64, height: f64, surface: u64, rgb: Option<u32>, crop_width: f64, crop_height: f64, below: bool },
    Detach { id: u64 },
    /// All of what's selected on a page, in its own coordinates. Nothing clears it.
    Highlight { page: u32, rects: Vec<HighlightRect> },
}

impl From<Op> for ReaderOp {
    fn from(op: Op) -> Self {
        match op {
            Op::Clear => Self::Clear,
            Op::Document { width, height } => Self::Document { width, height },
            Op::Magnify { value } => Self::Magnify { value },
            Op::ScrollTo { y, seq } => Self::ScrollTo { y, seq },
            Op::ScrollAcross { x } => Self::ScrollAcross { x },
            Op::ShowPage { page, x, y, width, height, fit, paper_rgb } => Self::ShowPage { page, x, y, width, height, fit, paper_rgb },
            Op::HidePage { page } => Self::HidePage { page },
            Op::Attach { id, page, x, y, width, height, surface, rgb, crop_width, crop_height, below } => Self::Attach { id, page, x, y, width, height, surface, rgb, crop_width, crop_height, below },
            Op::Detach { id } => Self::Detach { id },
            Op::Highlight { page, rects } => Self::Highlight { page, rects: rects.into_iter().map(|rect| HighlightRect { x: f64::from(rect.x), y: f64::from(rect.y), width: f64::from(rect.width), height: f64::from(rect.height) }).collect() },
        }
    }
}

/// The session the socket and the app share.
pub(crate) struct AppHost {
    engine: Engine,
    session: SessionHost<DrawableLoader>,
    core: Arc<CoreQueue>,
    qa: Option<Arc<QaBridge>>,
    saver: Arc<Saver>,
    /// Commands running now, which an update waits for. Not the ones that only ask about the state.
    running: AtomicU32,
    /// An update is under way, past the last look: what's written is what comes back, so nothing more is done.
    handing_off: AtomicBool,
    /// What a read of the record told the reader, while they've not moved on.
    notice: Mutex<Option<String>>,
}

impl AppHost {
    fn record_command(&self, level: Level, command: &Command) -> Result<Value, AgentError> {
        match command {
            Command::SessionShow {} => {
                let notice = lock(&self.notice).clone();
                let kept = kept_json(&self.saver.store, notice.as_deref().and_then(record::notices::words));
                Ok(self.session.with_session(|session| recto_agent::session_show_json(session, level, kept)))
            }
            Command::SessionSleep { tab } => {
                let id = self.session.with_session(|session| recto_agent::select_tab(session, tab))?;
                self.core.push(CoreCall::SleepTabs(vec![id], sleeper::Why::Asked));
                self.core.flush(Duration::from_secs(5));
                let (warm, list_asleep) = self.session.with_session(|session| (session.warm_tabs().any(|other| other == id), session.list_is_asleep(id)));
                Ok(json!({ "tab": id.0, "dormant": !warm, "list_asleep": list_asleep }))
            }
            Command::SessionSave {} => {
                let report = self.saver.flush(false, Duration::from_secs(5)).map_err(store_error)?;
                Ok(json!({ "saved": true, "tabs": report.tabs, "edited_tabs": report.edited_tabs, "record_bytes": report.record_bytes, "journal_bytes": report.journal_bytes, "wrote_bytes": report.wrote_bytes, "total_ms": report.total_us / 1000, "total_us": report.total_us, "write_us": report.write_us, "force_us": report.force_us, "verify_us": report.verify_us }))
            }
            Command::SessionRestore {} => {
                if level < Level::Full && self.session.with_session(|session| session.holds_owner_edits()) {
                    return Err(recto_agent::restore_would_lose_owners_edits());
                }
                let loaded = self.saver.store.load();
                let notice = notice_for(&loaded).and_then(record::notices::words);
                let restored = matches!(loaded, Loaded::Restored(_));
                self.core.push(CoreCall::Restored(Box::new(loaded)));
                self.core.flush(Duration::from_secs(5));
                let (count, active) = self.session.with_session(|session| (session.tabs().len(), session.active().map(|id| id.0)));
                Ok(json!({ "restored": restored, "count": count, "active": active, "notice": notice }))
            }
            _ => Err(recto_agent::qa_needs_app()),
        }
    }
}

impl Host for AppHost {
    fn run(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        let asks_only = matches!(command, Command::QaRead { .. } | Command::QaDrive { .. } | Command::SessionCheck {} | Command::SessionShow {} | Command::SessionSave {} | Command::UpdateStatus {} | Command::Settings { auto_update: None });
        // Counted first, and then looked: a hand-off that's set this flag and sees no count can't have this one slip past both looks. Past the last look, a change would be lost: what's been written is what comes back.
        if !asks_only {
            self.running.fetch_add(1, Ordering::SeqCst);
            if self.handing_off.load(Ordering::SeqCst) {
                self.running.fetch_sub(1, Ordering::SeqCst);
                return Err(AgentError::new("busy", "The app is being updated, so this wasn't done.", recto_agent::exit::BUSY).then("Try again in a moment, when it's back."));
            }
        }
        if matches!(command, Command::SessionSleep { .. } | Command::SessionShow {} | Command::SessionSave {} | Command::SessionRestore {}) {
            let result = self.record_command(level, command);
            if !asks_only {
                self.running.fetch_sub(1, Ordering::SeqCst);
            }
            return result;
        }
        let result = self.run_counted(level, command, events);
        if !asks_only {
            self.running.fetch_sub(1, Ordering::SeqCst);
        }
        result
    }

    fn check(&self, condition: Condition, tab: &str, page: Option<u32>) -> ConditionState {
        self.session.check(condition, tab, page)
    }

    fn tab_count(&self) -> u32 {
        self.session.tab_count()
    }

    /// Quitting is the person's. A quiet run is someone's test, and stops when it's told to.
    fn stop_refusal(&self) -> Option<AgentError> {
        self.qa.is_none().then(recto_agent::only_the_user_quits)
    }

    /// Nothing's lost by this session ending: its edits are in the state record, if it's the one keeping it.
    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        if self.saver.store.holds_the_record() { Vec::new() } else { self.session.unsaved_tabs() }
    }

    fn acted_tab(&self, command: &Command) -> Option<TabId> {
        self.session.acted_tab(command)
    }

    fn attach(&self, notify: Notifier) {
        self.session.attach(notify);
    }

    fn shut_down(&self) {
        self.session.shut_down();
    }

    fn access_is_off(&self) {
        self.session.access_is_off();
    }
}

impl AppHost {
    fn run_counted(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        // The level was checked by the server against the row, and again by `execute`. QA doesn't reach `execute`, which would say it needs an app.
        if matches!(command, Command::QaRead { .. } | Command::QaDrive { .. }) {
            return match &self.qa {
                Some(qa) => qa.run(command),
                None => Err(recto_agent::qa_needs_app()),
            };
        }
        // Zoom is about what's on the screen, so it's the page view's. The rest of the command is the session's.
        if let Command::View { paper, sidebar, agent_icon, zoom: Some(word), layout } = command {
            let zoom = menus::zoom_from_word(word).ok_or_else(|| AgentError::new("usage", format!("`{}` isn't a zoom. It's `in`, `out`, `fit`, `actual` or a number.", recto_core::clean_line(word)), recto_agent::exit::USAGE))?;
            let rest = Command::View { paper: paper.clone(), sidebar: sidebar.clone(), agent_icon: agent_icon.clone(), zoom: None, layout: layout.clone() };
            let mut answer = self.session.run(level, &rest, events)?;
            let value = self.engine.zoom(zoom).ok_or_else(|| AgentError::new("no_active_tab", "There's no page to zoom.", recto_agent::exit::NOT_FOUND))?;
            answer["zoom"] = json!(value);
            if let Some(fit) = self.engine.fit() {
                answer["zoom_level"] = json!(menus::zoom_label(value, fit).text);
            }
            self.core.push(CoreCall::Reconcile);
            return Ok(answer);
        }
        if matches!(command, Command::UpdateStatus {} | Command::UpdateCheck {} | Command::UpdateInstall {} | Command::Settings { .. }) {
            if !recto_agent::allowed(level, command.access()) {
                return Err(recto_agent::refusal(recto_agent::table::display_name(command.tag()), level, command.access()));
            }
            let inner = self.saver.inner().ok_or_else(recto_agent::update_needs_app)?;
            return inner.update_command(command);
        }
        if matches!(command, Command::SessionCheck {}) {
            let check = hand_off_check(self);
            let busy: Vec<Value> = check.busy.iter().map(|item| json!({ "code": item.code, "message": item.message })).collect();
            return Ok(json!({ "safe": check.safe, "busy": busy, "seconds_since_input": check.seconds_since_input }));
        }
        let result = self.session.run(level, command, events);
        self.core.push(CoreCall::Reconcile);
        result
    }
}

/// How many calls can wait for the session thread before notes of pages drawn stop being added.
const MAX_QUEUED_CALLS: usize = 4096;
/// Files being read at once off the session thread. Each is a thread that a dead volume can hold for as long as it likes, so there's a limit, and past it a tab or file doesn't open and says so.
const MAX_FILE_READS: usize = 4;

/// What the session thread is asked to do, in the order the user asked.
#[derive(Debug)]
enum CoreCall {
    Reconcile,
    Tap(TabId),
    GoTo(TabId, u32),
    Open(PathBuf),
    /// The sidebar hidden, or not.
    SetSidebarHidden(bool),
    /// The tab showing laid out another way.
    SetSpread(record::Spread),
    SetPaper(Paper),
    ToggleAgentIcon,
    ClearEditHistory,
    /// Those tabs go dormant.
    SleepTabs(Vec<TabId>, sleeper::Why),
    /// The system's short of memory, and whether it's critical.
    MemoryPressure(bool),
    /// A tab woken from its sleep file has another file there now.
    WokenFileIsOther(TabId, FileStatus),
    /// The worker says how long a tab's own file is, and which version.
    FileLength(TabId, u32, u64),
    /// A worker's found a tab's file replaced, so it's looked at.
    LookAgain(TabId),
    Page(u32, Option<&'static str>, TabId),
    /// A file read off the session thread, for a tab being woken or a file being opened.
    Read(Box<FileRead>),
    /// Everything asked before this has been done.
    Flush(Arc<Flushed>),
    /// The state record has been read, on a thread that could wait for it.
    Restored(Box<Loaded>),
    /// Patience is over for reading the state, or the file a restored tab is on.
    RestorePatienceOver,
}

/// A marker through the session thread's queue, for QA to know what it asked for has happened.
#[derive(Debug, Default)]
pub(crate) struct Flushed {
    done: Mutex<bool>,
    ready: Condvar,
}

impl Flushed {
    fn finished(&self) {
        *lock(&self.done) = true;
        self.ready.notify_all();
    }

    /// Waits for it, up to a limit, and says whether it came.
    fn wait(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut done = lock(&self.done);
        while !*done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            done = self.ready.wait_timeout(done, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }
}

/// The session thread's queue. A parked thread at rest, so an open app takes no wake-ups for it.
#[derive(Default)]
pub(crate) struct CoreQueue {
    calls: Mutex<VecDeque<CoreCall>>,
    ready: Condvar,
    reconcile_queued: AtomicBool,
    /// A hand-off or a quit is writing what comes back, so what a person does from here wouldn't: taps, opens and the rest aren't taken.
    frozen: AtomicBool,
}

impl CoreQueue {
    pub(crate) fn push(&self, call: CoreCall) {
        // Many reasons to look can wait for one look.
        if matches!(call, CoreCall::Reconcile) && self.reconcile_queued.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.frozen.load(Ordering::SeqCst) && matches!(call, CoreCall::ClearEditHistory | CoreCall::Tap(_) | CoreCall::GoTo(..) | CoreCall::Open(_) | CoreCall::SetSidebarHidden(_) | CoreCall::SetSpread(_) | CoreCall::SetPaper(_) | CoreCall::ToggleAgentIcon) {
            return;
        }
        let mut calls = lock(&self.calls);
        if let CoreCall::Page(page, failure, tab) = &call {
            // Only notes of what was drawn can go, as they're told on to watchers and no more. Many tiles of a page are one note, which is most of them, so this is hard to reach. With the session thread held up for some reason this stops them growing without end. Anything a person did always gets in.
            if calls.len() >= MAX_QUEUED_CALLS || matches!(calls.back(), Some(CoreCall::Page(last_page, last_failure, last_tab)) if last_page == page && last_failure == failure && last_tab == tab) {
                return;
            }
        }
        calls.push_back(call);
        drop(calls);
        self.ready.notify_one();
    }

    /// Blocks until the session thread has got through everything already asked of it. For QA, on a job thread. A file on a dead volume could hold this up, so it has a limit.
    pub(crate) fn flush(&self, limit: Duration) -> bool {
        let marker = Arc::new(Flushed::default());
        self.push(CoreCall::Flush(Arc::clone(&marker)));
        marker.wait(limit)
    }

    /// Wakes whoever's waiting on this, to look at its deadline again.
    pub(crate) fn nudge(&self) {
        let _calls = lock(&self.calls);
        self.ready.notify_one();
    }

    /// The next call, or `None` when `deadline` says something's due: one wait for both, so the state's one deadline costs no thread or timer of its own. `deadline` is asked again on every wake, and no more than that.
    fn next_or_due(&self, deadline: impl Fn() -> Option<Duration>) -> Option<CoreCall> {
        let mut calls = lock(&self.calls);
        loop {
            if let Some(call) = calls.pop_front() {
                if matches!(call, CoreCall::Reconcile) {
                    self.reconcile_queued.store(false, Ordering::SeqCst);
                }
                return Some(call);
            }
            calls = match deadline() {
                None => self.ready.wait(calls).unwrap_or_else(PoisonError::into_inner),
                Some(wait) if wait.is_zero() => return None,
                Some(wait) => self.ready.wait_timeout(calls, wait).unwrap_or_else(PoisonError::into_inner).0,
            };
        }
    }

    #[cfg(test)]
    fn next(&self) -> CoreCall {
        loop {
            if let Some(call) = self.next_or_due(|| None) {
                return call;
            }
        }
    }
}

struct EngineHooks {
    core: Arc<CoreQueue>,
    listener: Arc<dyn AppListener>,
}

impl EngineEvents for EngineHooks {
    fn ops_ready(&self) {
        self.listener.ops_ready();
    }

    fn note_changed(&self) {
        self.core.push(CoreCall::Reconcile);
    }

    fn page_moved(&self) {
        self.core.push(CoreCall::Reconcile);
    }

    fn page_drawn(&self, tab: u32, page: u32, failure: Option<&'static str>) {
        self.core.push(CoreCall::Page(page, failure, TabId(tab)));
    }

    fn file_length(&self, tab: u32, pages: u32, version: u64) {
        self.core.push(CoreCall::FileLength(TabId(tab), pages, version));
    }

    fn file_replaced(&self, tab: u32) {
        self.core.push(CoreCall::LookAgain(TabId(tab)));
    }

    fn copy_ready(&self) {
        self.listener.copy_ready();
    }
}

/// What the session thread remembers between looks.
#[derive(Default)]
struct Synced {
    tab: Option<TabId>,
    page: u32,
    pages: u32,
    map: PageMap,
    warm: Vec<u32>,
    paper: Option<PaperKind>,
    /// How the tab showing is laid out, as the page view was last told.
    spread: Option<record::Spread>,
}

/// The state the UI thread pulls, swapped whole. A short lock, never held across anything slow.
struct Published {
    sidebar: Arc<SidebarView>,
    tabs: Arc<Vec<SidebarRow>>,
    contents: Arc<Vec<ContentsEntry>>,
    rail_filled: u32,
    reader: ReaderView,
}

struct Inner {
    /// Small state, and no thread or timer of its own at rest.
    updates: Arc<recto_update::updater::Updater>,
    /// What the host knows of this copy, for the thread that reads the state.
    update_facts: Mutex<Option<recto_update::updater::Facts>>,
    host: Arc<AppHost>,
    engine: Engine,
    core: Arc<CoreQueue>,
    listener: Arc<dyn AppListener>,
    published: Mutex<Published>,
    contents_follow: Mutex<ContentsFollow>,
    started: Instant,
    saver: Arc<Saver>,
    app: Mutex<AppState>,
    /// The record's been read and applied, whatever it said.
    restored: AtomicBool,
    /// The control socket, ready to run once the session's as it was, so nothing asks about an empty one or opens over it.
    waiting_server: Mutex<Option<Server>>,
    /// Reading the file a restored active tab is on. Nothing's drawn until it's looked at, so the wrong thing can't show.
    holding_tab: Mutex<Option<TabId>>,
    sleeper: sleeper::Sleeper,
    /// Where a hand-off has got to.
    hand_off_state: AtomicU8,
    /// In a QA run, files being read wait here, as they would for a slow disk.
    qa_hold_reads: AtomicBool,
    /// Files being read now, which a hand-off waits for.
    reads_running: AtomicU32,
    /// Sheets and panels over the window now, which a hand-off waits for.
    sheets: AtomicU32,
    /// When the user last touched the app, in milliseconds of `started`. One number, set by what the event monitor calls, with no timer.
    last_input_ms: AtomicU64,
    /// What the window's first showing had on it.
    first_shown: Mutex<Option<Value>>,

    watchdog: Watchdog,
    stopper: Mutex<Option<Stopper>>,
    announcer: Mutex<Option<Announcer>>,
    dark: AtomicBool,
    panel_open: AtomicBool,
    qa: Option<Arc<QaBridge>>,
    paths: Option<StatePaths>,
    open_failure: Mutex<Option<String>>,
}

/// What the host tells Rust about the window, kept for the record. A short lock: the UI thread sets these and a writer copies them.
#[derive(Clone, Debug)]
struct AppState {
    window: Option<WindowRecord>,
    appearance: Appearance,
    sidebar_width: f64,
    /// The active tab's contents. A copy, as they're the host's to scroll.
    contents_y: Option<f64>,
    /// Where they go on the tab's first showing, and whose they are then.
    contents_resume: Option<(TabId, ContentsScroll)>,
    /// What was selected, to put back on the tab's first showing.
    selection_resume: Option<(TabId, record::Selection)>,
    focus: FocusRecord,
    settings_window: Option<SettingsWindow>,
    window_closed: bool,
}

/// What's in flight, for an update to wait for. A running save goes here when it exists, and an export is a command.
fn hand_off_check_parts(inner_reads: u32, sheets: u32, commands: u32) -> Vec<BusyItem> {
    let item = |code: &str, message: &str| BusyItem { code: code.to_string(), message: message.to_string() };
    let mut busy = Vec::new();
    if sheets > 0 {
        busy.push(item("sheet_open", "A sheet or panel is open over the window."));
    }
    if commands > 0 {
        busy.push(item("agent_command", "An agent's command is running."));
    }
    if inner_reads > 0 {
        busy.push(item("file_opening", "A file is being opened."));
    }
    busy
}

fn hand_off_check(host: &AppHost) -> HandOffCheck {
    let Some(inner) = host.saver.inner() else { return HandOffCheck { safe: false, busy: Vec::new(), seconds_since_input: 0.0 } };
    inner.hand_off_check()
}

impl Inner {
    fn hand_off_check(&self) -> HandOffCheck {
        let busy = hand_off_check_parts(self.reads_running.load(Ordering::SeqCst), self.sheets.load(Ordering::SeqCst), self.host.running.load(Ordering::SeqCst));
        let last = self.last_input_ms.load(Ordering::Relaxed);
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        HandOffCheck { safe: busy.is_empty(), busy, seconds_since_input: now.saturating_sub(last) as f64 / 1000.0 }
    }

    /// **The hand-off.** If nothing's in flight, stops taking commands and what a person does, writes everything, forces it all to the drive, and reads it back. On success the state is exactly what comes back, and the caller quits with no prompt, by [`AppCore::hand_off_quit_now`], which goes only after this has said yes. Otherwise it says why, everything's taken again, and nothing's changed. Waits on the disk, so never on the UI thread.
    fn hand_off(&self, limit: Duration) -> Result<HandOffReport, HandOffError> {
        let check = self.hand_off_check();
        if !check.safe {
            return Err(HandOffError::Busy { items: check.busy });
        }
        self.set_frozen(true);
        self.hand_off_state.store(HandOffState::Writing as u8, Ordering::SeqCst);
        let undo = || {
            self.hand_off_state.store(HandOffState::Idle as u8, Ordering::SeqCst);
            self.set_frozen(false);
        };
        // What slipped in as that was set is counted by now, and what's coming will see the flag.
        let check = self.hand_off_check();
        if !check.safe {
            undo();
            return Err(HandOffError::Busy { items: check.busy });
        }
        match self.saver.flush(true, limit) {
            Ok(report) => {
                // A file still being read when that gave up would have shown as an error: this is what says nothing's left over.
                let check = self.hand_off_check();
                if !check.safe {
                    undo();
                    return Err(HandOffError::Busy { items: check.busy });
                }
                self.hand_off_state.store(HandOffState::Done as u8, Ordering::SeqCst);
                Ok(report.into())
            }
            Err(error) => {
                undo();
                Err(HandOffError::NotSaved { code: error.code.to_string(), message: error.message })
            }
        }
    }

    /// Whether commands and what a person does are being taken.
    fn set_frozen(&self, frozen: bool) {
        self.host.handing_off.store(frozen, Ordering::SeqCst);
        self.core.frozen.store(frozen, Ordering::SeqCst);
    }

    /// Quits now, if a hand-off has really got this far. Says whether it did.
    fn hand_off_quit_now(&self) -> bool {
        if self.hand_off_state.load(Ordering::SeqCst) != HandOffState::Done as u8 {
            return false;
        }
        self.listener.hand_off_quit();
        true
    }

    /// The reader's moved on from what they were told about the session as a whole.
    fn notice_read(&self) {
        if lock(&self.host.notice).take().is_some() {
            self.saver.view_changed();
            self.core.push(CoreCall::Reconcile);
        }
    }

    /// The host's own part of the record.
    fn app_record(&self) -> AppRecord {
        let app = lock(&self.app).clone();
        let following = lock(&self.contents_follow).is_following();
        let notice = lock(&self.host.notice).clone();
        AppRecord { window: app.window, appearance: app.appearance, sidebar_width: Some(app.sidebar_width), panel_open: self.panel_open.load(Ordering::SeqCst), contents_scroll: Some(ContentsScroll { y: app.contents_y.unwrap_or(0.0), following }), focus: app.focus, notice, settings_window: app.settings_window, window_closed: app.window_closed }
    }

    /// Everything the record holds and QA compares: the record as it would be written, and each tab's edits in full.
    pub(crate) fn state_json(&self) -> Value {
        let (record, _) = self.gather_record();
        let mut value = serde_json::to_value(&record).unwrap_or(Value::Null);
        let lists = self.host.session.with_session(|session| session.tabs().iter().filter_map(|tab| Some((tab.id.0.to_string(), serde_json::to_value(session.list_record_anywhere(tab.id)?).ok()?))).collect::<serde_json::Map<_, _>>());
        value["edits_full"] = Value::Object(lists);
        value
    }

    /// What QA reads of how a restore and the record went, none of it what the user sees.
    pub(crate) fn record_report(&self) -> Value {
        let (writes, appends) = self.saver.store.writes();
        json!({ "first_shown": *lock(&self.first_shown), "sleep": self.sleeper.report(), "writes": writes, "appends": appends, "wrote_bytes": self.saver.store.wrote_bytes(), "failed": self.saver.has_failed() })
    }

    /// A QA verb that needs no window. `None` if it's for the host. Calls what the UI will.
    pub(crate) fn qa_session_verb(&self, verb: &str, value: &str) -> Option<Result<(), AgentError>> {
        let usage = |words: String| AgentError::new("usage", words, recto_agent::exit::USAGE);
        let session = &self.host.session;
        let result = match verb {
            "find" => session.with_session(|session| {
                let find = session.find().clone();
                session.set_find(value == "open", &find.query, &find.draft);
                Ok(())
            }),
            "find_query" => session.with_session(|session| {
                let find = session.find().clone();
                session.set_find(find.open, value, &find.draft);
                Ok(())
            }),
            "find_draft" => session.with_session(|session| {
                let find = session.find().clone();
                session.set_find(find.open, &find.query, value);
                Ok(())
            }),
            "edit_mode" => session.with_session(|session| {
                session.set_edit_mode(value == "on");
                Ok(())
            }),
            "view_rotation" => match value.parse::<u8>() {
                Ok(turns) if turns < 4 => session.with_session(|session| {
                    if let Some(id) = session.active() {
                        session.set_view_rotation(id, turns);
                    }
                    Ok(())
                }),
                _ => Err(usage("It's a number of quarter turns, from 0 to 3.".to_string())),
            },
            "owner_edit" => self.owner_edit(value),
            "save_from" => self.qa_save_from(value),
            "dormant_after" => match value.parse::<u32>() {
                Ok(seconds) => {
                    self.sleeper.set_after(seconds);
                    self.core.push(CoreCall::Reconcile);
                    Ok(())
                }
                Err(_) => Err(usage("It's a number of seconds.".to_string())),
            },
            "live_budget" => match value.parse::<u64>() {
                Ok(mib) => {
                    self.sleeper.set_budget(mib.saturating_mul(1024 * 1024));
                    Ok(())
                }
                Err(_) => Err(usage("It's a number of MiB.".to_string())),
            },
            "hold_drawing" => {
                self.engine.hold_drawing(value == "on");
                Ok(())
            }
            "sleep_write_gap" => match value.parse::<u32>() {
                Ok(seconds) => {
                    self.sleeper.set_write_gap(seconds);
                    Ok(())
                }
                Err(_) => Err(usage("It's a number of seconds.".to_string())),
            },
            "memory_pressure" => match value {
                "warning" | "critical" => {
                    self.core.push(CoreCall::MemoryPressure(value == "critical"));
                    Ok(())
                }
                _ => Err(usage("It's `warning` or `critical`.".to_string())),
            },
            "note_input" => {
                self.note_input();
                Ok(())
            }
            "hold_reads" => {
                self.qa_hold_reads.store(value == "on", Ordering::SeqCst);
                Ok(())
            }
            _ => return None,
        };
        self.core.push(CoreCall::Reconcile);
        Some(result)
    }

    /// **What a Save does around its writing, with the writing done by the test,** as Save isn't built: keeps what's needed, notes what's coming and flushes that, puts the file in place, and confirms. Every step's the real one.
    fn qa_save_from(&self, written: &str) -> Result<(), AgentError> {
        let failed = |what: &str| AgentError::new("save_failed", format!("{what} failed."), recto_agent::exit::FAILED);
        let written = PathBuf::from(written);
        let target = recto_platform::file_stamp(&written).map_err(|_| failed("Looking at the written file"))?;
        let (id, path, file_pages, pages, needs) = self.host.session.with_session(|session| {
            let id = session.active()?;
            let list = session.page_edits(id);
            Some((id, session.path_of(id)?, session.tab(id)?.page_count, session.pages_in(id), list.is_some_and(recto_session::edit::PageEdits::needs_own_pages)))
        }).ok_or_else(|| AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND))?;
        let kept = needs.then(|| self.saver.store.keep_pages_of(&path, target, file_pages)).flatten();
        self.host.session.with_session(|session| session.save_prepared(id, target, kept.as_ref()));
        self.saver.flush(false, Duration::from_secs(5)).map_err(|_| failed("Writing what's about to happen"))?;
        std::fs::rename(&written, &path).map_err(|_| failed("Putting the file in place"))?;
        let now = recto_platform::file_stamp(&path).map_err(|_| failed("Looking at the saved file"))?;
        self.host.session.with_session(|session| session.saved(id, now, pages, None, kept));
        Ok(())
    }

    /// An edit as the owner's, as the app's own tools will make it: they aren't built yet.
    fn owner_edit(&self, value: &str) -> Result<(), AgentError> {
        let usage = || AgentError::new("usage", "It's `rotate:PAGE`, `delete:PAGE`, `undo` or `redo`.".to_string(), recto_agent::exit::USAGE);
        self.host.session.with_session(|session| {
            let id = session.active().ok_or_else(|| AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND))?;
            let page = |word: &str| word.parse::<u32>().ok().and_then(|page| page.checked_sub(1)).ok_or_else(usage);
            match value.split_once(':') {
                Some(("rotate", word)) => {
                    let page = page(word)?;
                    session.edit_pages(id, |edits| edits.rotate(page, 1, 1))?;
                }
                Some(("delete", word)) => {
                    let page = page(word)?;
                    session.edit_pages(id, |edits| edits.delete(page, 1))?;
                }
                None if value == "undo" => {
                    session.undo_edit(id);
                }
                None if value == "redo" => {
                    session.redo_edit(id);
                }
                _ => return Err(usage()),
            }
            Ok(())
        })
    }

    fn note_nearby(&self) {
        self.last_input_ms.store(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    fn note_input(&self) {
        self.last_input_ms.store(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
        // One lock per input, and an uncontended one. It's what lets a notice go when the reader has moved on.
        self.notice_read();
    }
}

/// The app's Rust half.
#[derive(uniffi::Object)]
pub struct AppCore {
    inner: Arc<Inner>,
    qa_mode: bool,
    start_with: Vec<PathBuf>,
}

/// What a launch was asked for.
#[derive(Debug, Default, PartialEq)]
struct LaunchArgs {
    qa: bool,
    memory_limit_mib: Option<u64>,
    time_limit: Option<Duration>,
    open: Vec<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<LaunchArgs, String> {
    let mut parsed = LaunchArgs::default();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--qa" => parsed.qa = true,
            "--qa-memory-limit-mib" => parsed.memory_limit_mib = Some(words.next().and_then(|value| value.parse().ok()).ok_or("--qa-memory-limit-mib takes a number.")?),
            "--qa-time-limit-ms" => parsed.time_limit = Some(Duration::from_millis(words.next().and_then(|value| value.parse().ok()).ok_or("--qa-time-limit-ms takes a number.")?)),
            // What macOS adds to a launch from elsewhere, and a file to open.
            other if other.starts_with("-psn_") || other.starts_with("-NS") || other.starts_with("-Apple") => {
                let _ = words.next();
            }
            other if other.starts_with('-') => return Err(format!("`{}` isn't an option of this app.", cleaned(other))),
            other => parsed.open.push(PathBuf::from(other)),
        }
    }
    if !parsed.qa && (parsed.memory_limit_mib.is_some() || parsed.time_limit.is_some()) {
        return Err("Those options are only for a QA run.".into());
    }
    Ok(parsed)
}

/// Whether this launch is a quiet run, which is decided before AppKit does anything. The same parser as the rest of the launch, so the two can't disagree. A launch that won't parse ends straight away, and shows nothing on the way.
#[uniffi::export]
pub fn launch_is_quiet(args: Vec<String>) -> bool {
    parse_args(&args).map_or(true, |parsed| parsed.qa)
}

fn cleaned(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(64).collect()
}

/// What a QA run must be: on a state folder that was set on purpose, never the real one. `recto qa start` writes the level there, since nothing else can.
fn state_for(qa: bool) -> Result<StatePaths, String> {
    let paths = StatePaths::from_env().map_err(|error| error.message)?;
    if qa && !paths.overridden {
        return Err("A QA run needs RECTO_STATE_DIR set to a temp folder. It won't touch the real state.".into());
    }
    Ok(paths)
}

#[uniffi::export]
impl AppCore {
    /// Joins the session and the socket. Checks the arguments and binds, which is quick. A socket another session holds doesn't stop a normal run: the reader works, and `recto` keeps talking to that session.
    #[uniffi::constructor]
    pub fn start(args: Vec<String>, listener: Arc<dyn AppListener>) -> Result<Arc<Self>, StartError> {
        let refused = |message: String| StartError::Refused { message };
        let args = parse_args(&args).map_err(refused)?;
        let paths = state_for(args.qa).map_err(refused)?;
        let server = match Server::bind(&paths) {
            Ok(server) => Some(server),
            Err(error) if args.qa => return Err(refused(AgentError::from(error).message)),
            Err(_) => None,
        };
        let core = Arc::new(CoreQueue::default());
        let hooks: Arc<dyn EngineEvents> = Arc::new(EngineHooks { core: Arc::clone(&core), listener: Arc::clone(&listener) });
        let mut policy = Policy::default();
        if let Some(limit) = args.memory_limit_mib {
            policy.memory_limit_bytes = limit * 1024 * 1024;
            // At or below the normal limit, this step is off, so a page reaches its lower-resolution version straight away.
            policy.heavy_limit_bytes = policy.memory_limit_bytes;
        }
        let engine = Engine::new(EngineConfig { service: None, policy: Some(policy) }, hooks);
        let watchdog = Watchdog::new();
        let qa = args.qa.then(|| Arc::new(QaBridge::new(Arc::clone(&listener), engine.clone(), watchdog.clone(), Arc::clone(&core))));
        // Takes the record's lock, and reads nothing: that's `begin_restore`'s, on a thread that can wait for it.
        let saver = Arc::new(Saver::new(RecordStore::open(&paths, Flavor::App), Arc::clone(&core)));
        let host = Arc::new(AppHost { engine: engine.clone(), session: SessionHost::new(DrawableLoader).with_indexing(&paths), core: Arc::clone(&core), qa: qa.clone(), saver: Arc::clone(&saver), running: AtomicU32::new(0), handing_off: AtomicBool::new(false), notice: Mutex::new(None) });
        let stopper = server.as_ref().and_then(|server| server.stopper().ok());
        let announcer = server.as_ref().and_then(|server| server.announcer().ok());
        let update_hooks = updates::Hooks::new();
        let update_source = recto_update::source::from_env(paths.overridden);
        let updates = recto_update::updater::Updater::new(&paths.root, update_source, Arc::clone(&update_hooks) as Arc<dyn recto_update::updater::Host>);
        let inner = Arc::new(Inner {
            updates,
            update_facts: Mutex::new(None),
            host: Arc::clone(&host),
            engine,
            core: Arc::clone(&core),
            listener: Arc::clone(&listener),
            contents_follow: Mutex::new(ContentsFollow::default()),
            started: Instant::now(),
            saver: Arc::clone(&saver),
            sleeper: sleeper::Sleeper::new(),
            app: Mutex::new(AppState { window: None, appearance: Appearance::System, sidebar_width: record::SIDEBAR_WIDTH, contents_y: None, contents_resume: None, selection_resume: None, focus: FocusRecord::default(), settings_window: None, window_closed: false }),
            restored: AtomicBool::new(false),
            waiting_server: Mutex::new(server),
            holding_tab: Mutex::new(None),
            hand_off_state: AtomicU8::new(HandOffState::Idle as u8),
            qa_hold_reads: AtomicBool::new(false),
            reads_running: AtomicU32::new(0),
            sheets: AtomicU32::new(0),
            last_input_ms: AtomicU64::new(0),
            first_shown: Mutex::new(None),
            published: Mutex::new(Published { contents: Arc::new(Vec::new()), rail_filled: 0, tabs: Arc::new(Vec::new()), sidebar: Arc::new(SidebarView { tabs_version: 0, tab_count: 0, active_index: None, contents_version: 0, contents_count: 0, contents_changed_from: 0, contents_removed: 0, contents_inserted: 0, contents_place_shift: 0, contents_current: None, toc_open: false, place: String::new(), edited: false, undone_owner_steps: false }), reader: ReaderView { home: true, note: None, sidebar_hidden: false, layout: String::new(), place: String::new(), paper: Paper::White.name().to_string(), paper_rgb: PaperKind::White.rgb(), page_tint: None, agent_icon_hidden: false, sidebar_width: record::SIDEBAR_WIDTH, highlight_dark_colour: false, highlight_lighten: false, selection_notice: None } }),
            watchdog,
            stopper: Mutex::new(stopper),
            announcer: Mutex::new(announcer),
            dark: AtomicBool::new(false),
            panel_open: AtomicBool::new(false),
            qa,
            paths: Some(paths),
            open_failure: Mutex::new(None),
        });
        saver.attach(Arc::downgrade(&inner));
        update_hooks.attach(Arc::downgrade(&inner));
        if let Some(qa) = &inner.qa {
            qa.attach(Arc::downgrade(&inner));
        }
        let session_side = Arc::clone(&inner);
        std::thread::Builder::new().name("recto-session".into()).stack_size(8 * 1024 * 1024).spawn(move || session_thread(&session_side)).map_err(|_| refused("The app couldn't start its session thread.".into()))?;
        if let Some(limit) = args.time_limit.or(args.qa.then_some(Duration::from_secs(600))) {
            // Every QA run has a limit, so a run nobody ended can't sit on the owner's Mac.
            let _ = std::thread::Builder::new().name("recto-qa-limit".into()).stack_size(64 * 1024).spawn(move || {
                std::thread::sleep(limit);
                std::process::exit(124);
            });
        }
        Ok(Arc::new(Self { inner, qa_mode: args.qa, start_with: args.open }))
    }

    /// Whether this is a quiet run: off every screen, never active, with no Dock icon.
    pub fn is_qa(&self) -> bool {
        self.qa_mode
    }

    /// The window is up. In a QA run this tells whoever's waiting, on one line, and any files named at launch start opening.
    pub fn ready(&self) {
        for path in &self.start_with {
            self.inner.core.push(CoreCall::Open(path.clone()));
        }
        self.inner.core.push(CoreCall::Reconcile);
        if self.qa_mode {
            let socket = self.inner.paths.as_ref().map(|paths| paths.socket.clone());
            let line = json!({ "ok": true, "data": { "running": true, "pid": std::process::id(), "socket": socket } }).to_string();
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    }

    pub fn strings(&self) -> AppStrings {
        AppStrings {
            app_name: "RectoPDF".into(),
            home_hint: "Drop a PDF here, or".into(),
            open_button: "Open a PDF".into(),
            open_panel_title: "Open a PDF".into(),
            add_tooltip: "Open another PDF".into(),
            agent_tooltip: "The agent isn't built yet".into(),
            sidebar_button_tooltip: "Show or hide the sidebar (⌃⌘S)".into(),
            notice_mark: "\u{26A0}\u{FE0E}".into(),
        }
    }

    pub fn limits(&self) -> ReaderLimits {
        ReaderLimits { min_zoom: MIN_ZOOM, max_zoom: MAX_ZOOM, window_width: 1440.0, window_height: 900.0, sidebar_width: record::SIDEBAR_WIDTH, reveal_delay_seconds: 0.5, restore_patience_seconds: RESTORE_PATIENCE.as_secs_f64(), show_patience_seconds: SHOW_PATIENCE.as_secs_f64(), autoscroll_every_seconds: recto_reader::selection::AUTOSCROLL_EVERY, notice_seconds: 6.0 }
    }

    /// The sidebar as it stands, but for its tabs. A short lock and a small copy.
    pub fn sidebar(&self) -> SidebarView {
        (*lock(&self.inner.published).sidebar).clone()
    }

    /// The tabs, one row each. Worth pulling only when `tabs_version` has moved: this copies all of them.
    pub fn sidebar_tabs(&self) -> Vec<SidebarRow> {
        let tabs = Arc::clone(&lock(&self.inner.published).tabs);
        (*tabs).clone()
    }

    /// Some of the contents' rows, for what's on screen. A short lock and a copy of at most [`MAX_CONTENTS_FETCH`].
    pub fn contents_rows(&self, first: u32, count: u32) -> Vec<SidebarRow> {
        let published = lock(&self.inner.published);
        let (contents, rail_filled, current) = (Arc::clone(&published.contents), published.rail_filled, published.sidebar.contents_current);
        drop(published);
        let total = u32::try_from(contents.len()).unwrap_or(u32::MAX);
        let end = first.saturating_add(count.min(MAX_CONTENTS_FETCH)).min(total);
        (first..end)
            .filter_map(|index| {
                let entry = contents.get(index as usize)?;
                Some(SidebarRow { kind: RowKind::Contents, tab: entry.tab, text: entry.text.to_string(), page: entry.page, depth: entry.depth, current: current == Some(index), rail_filled: index < rail_filled, rail_first: index == 0, rail_last: index + 1 == total, notice: None })
            })
            .collect()
    }

    /// The sidebar's height rule, for this much room between its header and footer.
    pub fn sidebar_layout(&self, room: f64) -> SidebarLayout {
        let published = lock(&self.inner.published);
        let tabs = published.sidebar.tab_count;
        let rows = if published.sidebar.toc_open { published.sidebar.contents_count } else { 0 };
        drop(published);
        let heights = sidebar_heights(room, tabs, rows);
        SidebarLayout { tab_row: TAB_ROW_HEIGHT, contents_row: CONTENTS_ROW_HEIGHT, contents_height: heights.contents, tabs_scroll: heights.tabs_scroll }
    }

    /// The reader scrolled the contents themselves.
    pub fn contents_user_scrolled(&self) {
        lock(&self.inner.contents_follow).user_scrolled(self.inner.started.elapsed().as_secs_f64());
    }

    /// The pointer came into the contents or left them.
    pub fn contents_pointer(&self, inside: bool) {
        lock(&self.inner.contents_follow).pointer(inside);
    }

    /// The place being read is now on this row. What to do about it.
    pub fn contents_reached(&self, row: Option<u32>, scroll_y: f64, height: f64, reduce_motion: bool) -> ContentsMove {
        let rows = lock(&self.inner.published).sidebar.contents_count;
        let now = self.inner.started.elapsed().as_secs_f64();
        match lock(&self.inner.contents_follow).reached(row, ContentsView { scroll_y, height, rows }, now, reduce_motion) {
            FollowMove::Stay => ContentsMove::Stay,
            FollowMove::SetTo(y) => ContentsMove::SetTo { y },
            FollowMove::Move => ContentsMove::Move,
        }
    }

    /// One frame of a move: where the scroll goes next, or nothing when it's over and the frames should stop. An atomic-sized lock and some arithmetic.
    pub fn contents_step(&self, position: f64, dt: f64) -> Option<f64> {
        lock(&self.inner.contents_follow).step(position, dt)
    }

    /// The contents' area changed height.
    pub fn contents_view_changed(&self) {
        lock(&self.inner.contents_follow).view_changed();
    }

    /// Rows closed or opened. Says how many points to move the scroll by with no animation, or nothing if the rows are to slide.
    pub fn contents_row_change(&self, scroll_y: f64, from: u32, removed: u32, inserted: u32, place_shift: i32) -> Option<f64> {
        lock(&self.inner.contents_follow).row_change(ContentsRowChange { from, removed, inserted, place_shift }, scroll_y)
    }

    /// Rows above changed, and the host moved its scroll by this many points to match.
    pub fn contents_shifted(&self, by: f64) {
        lock(&self.inner.contents_follow).shifted(by);
    }

    pub fn contents_is_following(&self) -> bool {
        lock(&self.inner.contents_follow).is_following()
    }

    pub fn contents_is_moving(&self) -> bool {
        lock(&self.inner.contents_follow).is_moving()
    }

    pub fn reader(&self) -> ReaderView {
        lock(&self.inner.published).reader.clone()
    }

    /// A row was tapped. Goes on the session thread, as waking a cold tab reads its file.
    pub fn tap_row(&self, row: SidebarRow) {
        let call = match (row.kind, row.page) {
            (RowKind::Tab, _) => CoreCall::Tap(TabId(row.tab)),
            (RowKind::Contents, Some(page)) => CoreCall::GoTo(TabId(row.tab), page.saturating_sub(1)),
            (RowKind::Contents, None) => return,
        };
        self.inner.core.push(call);
    }

    pub fn open_file(&self, path: String) {
        self.inner.core.push(CoreCall::Open(PathBuf::from(path)));
    }

    /// The view moved. On every scroll: a short lock and a few map lookups. Apply what comes back in the same transaction.
    pub fn set_viewport(&self, viewport: ViewportInput) -> Vec<ReaderOp> {
        self.inner.saver.view_changed();
        let ops = self.inner.engine.set_viewport(Viewport { x: viewport.x, y: viewport.y, width: viewport.width, height: viewport.height, frame_width: viewport.frame_width, magnification: viewport.magnification, backing: viewport.backing, live: viewport.live, scrolled_to: viewport.scrolled_to });
        ops.into_iter().map(ReaderOp::from).collect()
    }

    /// What's waiting for the layers, at most a few tiles at a time. If there's more, `ops_ready` comes again.
    pub fn drain_ops(&self) -> Vec<ReaderOp> {
        self.inner.engine.drain().into_iter().map(ReaderOp::from).collect()
    }

    /// The layers have what they were given, so what's off them can go.
    pub fn ops_applied(&self) {
        self.inner.engine.applied();
    }

    pub fn set_dark_appearance(&self, dark: bool) {
        self.inner.dark.store(dark, Ordering::SeqCst);
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// What hovering over a row reveals: its full text, clean and bounded, and what it is.
    pub fn row_reveal(&self, row: SidebarRow) -> RowReveal {
        let full = row_full_text(&row.text, row.kind == RowKind::Tab);
        RowReveal { text: full.text, is_file_name: full.is_file_name }
    }

    /// The menu bar as it stands. The host rebuilds it only when this differs.
    pub fn menu_bar(&self) -> Vec<MenuModel> {
        let reader = lock(&self.inner.published).reader.clone();
        menus::menu_bar(ViewSettings { sidebar_hidden: reader.sidebar_hidden, agent_icon_hidden: reader.agent_icon_hidden, ..ViewSettings::default() }, record::Spread::from_name(&reader.layout), &self.inner.updates.menu_item().title)
    }

    /// The sidebar's panel as it stands.
    pub fn panel(&self) -> PanelModel {
        let reader = lock(&self.inner.published).reader.clone();
        let shown = Paper::from_name(&reader.paper).unwrap_or(Paper::White);
        menus::panel(self.inner.panel_open.load(Ordering::SeqCst), shown)
    }

    /// Does what a menu item, a panel button or a key does, and says what's left for the host. The same methods as `recto view`.
    pub fn perform(&self, id: String) -> HostAction {
        let Some(action) = menus::action(&id) else { return HostAction::None };
        // Past a hand-off's last look, what's done here wouldn't come back.
        if self.inner.core.frozen.load(Ordering::SeqCst) {
            return HostAction::None;
        }
        match action {
            menus::Action::Host(host) => return host,
            menus::Action::Copy { anyway } => self.inner.engine.copy_selection(anyway),
            menus::Action::SelectAll => self.inner.engine.select_all(),
            menus::Action::ClearEditHistory => self.inner.core.push(CoreCall::ClearEditHistory),
            menus::Action::Zoom(zoom) => {
                self.inner.engine.zoom(zoom);
            }
            menus::Action::ToggleSidebar => self.toggle_sidebar(),
            menus::Action::SetSpread(spread) => self.inner.core.push(CoreCall::SetSpread(spread)),
            menus::Action::ToggleAgentIcon => self.inner.core.push(CoreCall::ToggleAgentIcon),
            menus::Action::SetPaper(paper) => self.inner.core.push(CoreCall::SetPaper(paper)),
            menus::Action::TogglePanel => {
                self.inner.panel_open.fetch_xor(true, Ordering::SeqCst);
                self.inner.listener.model_changed();
            }
            // Off this thread by itself, and it says how it went.
            menus::Action::Update => self.inner.updates.menu_chosen(),
        }
        HostAction::None
    }

    /// Whether a menu item can be chosen now. Asked when a menu opens and on its key.
    pub fn menu_item_enabled(&self, id: String) -> bool {
        if id == "app.update" {
            return self.inner.updates.menu_item().enabled;
        }
        menus::enabled(&id, self.inner.engine.has_document(), self.inner.engine.has_selection())
    }

    /// The system says memory's short. From a dispatch source the host keeps, which is all that's the host's about it.
    pub fn memory_pressure(&self, critical: bool) {
        self.inner.core.push(CoreCall::MemoryPressure(critical));
    }

    /// What a right-click on a page offers.
    pub fn context_menu(&self) -> Vec<MenuEntry> {
        menus::context_menu(record::Spread::from_name(&lock(&self.inner.published).reader.layout))
    }

    /// The mouse went down on a page, in the document view's coordinates, the same at any zoom. Apply what comes back in the same transaction. It looks through text already here and never waits for any.
    pub fn selection_press(&self, x: f64, y: f64, clicks: u32, shift: bool) -> Vec<ReaderOp> {
        self.inner.engine.selection_press(x, y, clicks, shift).into_iter().map(ReaderOp::from).collect()
    }

    pub fn selection_drag(&self, x: f64, y: f64) -> Vec<ReaderOp> {
        self.inner.engine.selection_drag(x, y).into_iter().map(ReaderOp::from).collect()
    }

    pub fn selection_release(&self) -> Vec<ReaderOp> {
        self.inner.engine.selection_release().into_iter().map(ReaderOp::from).collect()
    }

    /// Whether the pointer's over text, for the I-beam. A page not read yet says no, and its text is read when there's nothing better to do.
    pub fn pointer_is_over_text(&self, x: f64, y: f64) -> bool {
        self.inner.engine.pointer_is_over_text(x, y)
    }

    /// How far to scroll per step of a drag this far past the view's edge.
    pub fn autoscroll_step(&self, beyond: f64) -> f64 {
        recto_reader::selection::autoscroll_step(beyond)
    }

    /// What a copy has for the clipboard. `copy_ready` says when.
    pub fn take_copied(&self) -> Option<String> {
        self.inner.engine.take_copied().map(|copied| copied.text)
    }

    /// The selection's text if it's all here, for what can't wait, as the Services menu can't. Nothing if not.
    pub fn selection_text_now(&self) -> Option<String> {
        self.inner.engine.selection_text_now()
    }

    /// Whether there's text the Services menu can be offered, which it asks often, on the UI thread. Builds nothing.
    pub fn selection_can_be_sent(&self) -> bool {
        self.inner.engine.selection_can_be_sent()
    }

    /// The reader has dismissed the notice, or its time's up.
    pub fn dismiss_selection_notice(&self) {
        self.inner.engine.dismiss_selection_notice();
        self.inner.updates.dismiss_notice();
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// Where a point on a page is in the document, for QA to go through what the mouse goes through.
    pub fn document_point(&self, page: u32, x: f64, y: f64) -> Option<DocumentPoint> {
        self.inner.engine.document_point(page, x, y).map(|(x, y)| DocumentPoint { x, y })
    }

    /// The id a key does with nothing held, or none.
    pub fn plain_key_action(&self, characters: String) -> Option<String> {
        menus::plain_key(&characters).map(str::to_string)
    }

    /// The zoom level in the top left, as the host's magnification and the page's fit make it. Cheap: on every change of zoom.
    pub fn zoom_label(&self, magnification: f64, fit: f64) -> menus::ZoomLabel {
        menus::zoom_label(magnification, fit)
    }

    /// What the reader typed in the zoom level's field. Says whether it was a level: if not, nothing changes and the field shows what it was. Held to the limits as every zoom is.
    pub fn zoom_typed(&self, text: String) -> bool {
        let Some(zoom) = recto_reader::tiles::typed_zoom(&text) else { return false };
        self.inner.engine.zoom(zoom).is_some()
    }

    /// The zoom after this much ⌘-scroll, from where it is.
    pub fn zoom_after_scroll(&self, current: f64, points: f64) -> f64 {
        recto_reader::tiles::zoom_after_scroll(current, points)
    }

    /// The sidebar's button, beside the window's controls: shows the sidebar if it's hidden, and hides it if not.
    pub fn toggle_sidebar(&self) {
        let hidden = !lock(&self.inner.published).reader.sidebar_hidden;
        self.inner.core.push(CoreCall::SetSidebarHidden(hidden));
    }

    pub fn window_closed(&self) {
        self.inner.engine.window_closed();
        lock(&self.inner.app).window_closed = true;
        self.inner.saver.view_changed();
    }

    pub fn window_opened(&self) {
        self.inner.engine.window_opened();
        lock(&self.inner.app).window_closed = false;
        self.inner.saver.view_changed();
    }

    /// Whether this launch opens a window. Any launch does but an update's arrival, which leaves things as they were: a window closed stays closed, rather than appearing behind whatever the user's doing.
    pub fn window_should_open(&self) -> bool {
        !(lock(&self.inner.app).window_closed && self.inner.updates.arrived_by_update())
    }

    /// There's no window to wait for, so this launch has come up as far as it goes.
    pub fn no_window_to_show(&self) {
        let updates = Arc::clone(&self.inner.updates);
        let _ = std::thread::Builder::new().name("recto-update-frame".into()).stack_size(UPDATE_FRAME_STACK).spawn(move || updates.first_frame(recto_update::wall_ms()));
    }

    /// The UI thread has woken.
    pub fn main_busy(&self) {
        self.inner.watchdog.busy();
    }

    /// The UI thread is about to sleep.
    pub fn main_idle(&self) {
        self.inner.watchdog.idle();
    }

    /// The answer to a QA request, as JSON. `ok` false makes it a failure, with `json` as the words.
    pub fn qa_answer(&self, id: u64, ok: bool, json: String) {
        if let Some(qa) = &self.inner.qa {
            qa.answer(id, ok, json);
        }
    }

    /// How long a scroll step of `qa sweep` took on the UI thread, and where the contents were on it: their scroll, less any change of rows above, and whether the place being read was showing.
    pub fn qa_note_step(&self, micros: u64, contents_y: f64, current_visible: bool) {
        if let Some(qa) = &self.inner.qa {
            qa.note_step(micros, contents_y, current_visible);
        }
    }

    /// Reads the state, on a thread of its own, and hears `restore_ready` when it's applied. It's what the window waits for, up to `restore_patience_seconds`: its frame and the place on it come from what's read.
    pub fn begin_restore(&self) {
        let inner = Arc::clone(&self.inner);
        let spawned = std::thread::Builder::new().name("recto-restore".into()).stack_size(1024 * 1024).spawn(move || {
            // Before the record: a version that keeps failing goes back here, and not by reading on.
            if let Some(facts) = lock(&inner.update_facts).take()
                && inner.updates.launched(facts) == recto_update::updater::AtLaunch::Exit
            {
                std::process::exit(0);
            }
            let loaded = inner.saver.store.load();
            inner.core.push(CoreCall::Restored(Box::new(loaded)));
        });
        if spawned.is_err() {
            self.inner.core.push(CoreCall::Restored(Box::new(Loaded::Nothing)));
        }
    }

    /// The state hasn't been read in time, or a restored tab's file isn't answering. What's on the screen goes on without it.
    pub fn restore_patience_over(&self) {
        self.inner.core.push(CoreCall::RestorePatienceOver);
    }

    /// Whether what the window would show now is right: the place on the layers, or nothing to wait for. Then, or once the patience is over, it shows. A short lock, asked only until it's showing.
    pub fn ready_to_show(&self) -> bool {
        if self.inner.started.elapsed() >= SHOW_PATIENCE + RESTORE_PATIENCE {
            return true;
        }
        if !self.inner.restored.load(Ordering::SeqCst) || lock(&self.inner.holding_tab).is_some() {
            return false;
        }
        // It's asked on the UI thread, which doesn't wait for whatever has the session: this comes round again.
        let Some(active) = self.inner.host.session.try_with_session(|session| session.active()) else { return false };
        active.is_none() || self.inner.engine.is_on_place() || self.inner.engine.is_showable()
    }

    /// The window has just been put on the screen, showing this. For QA to see what the first frame was.
    pub fn window_shown(&self, home: bool, tile_layers: u32) {
        self.inner.engine.window_up();
        let report = self.inner.engine.report().0;
        *lock(&self.inner.first_shown) = Some(json!({ "home": home, "page": report["page"], "scroll_y": report["scroll_y"], "scroll_x": report["scroll_x"], "magnification": report["magnification"], "settled": report["settled"], "tab": report["tab"], "tile_layers": tile_layers, "ms_from_start": self.inner.started.elapsed().as_secs_f64() * 1000.0 }));
        // The installer's waiting for this, off this thread, as it writes a small file and may start a worker, and only for a version that's new here.
        let updates = Arc::clone(&self.inner.updates);
        let _ = std::thread::Builder::new().name("recto-update-frame".into()).stack_size(UPDATE_FRAME_STACK).spawn(move || updates.first_frame(recto_update::wall_ms()));
    }

    /// Where the new window goes, on the screens there are now. Rust's rule, from what was saved. In a quiet run it keeps its place off every screen, and only its size comes back.
    pub fn window_placement(&self, screens: Vec<ScreenInfo>) -> WindowPlacement {
        let app = lock(&self.inner.app).clone();
        let activate = app.focus.app_frontmost;
        let focus_sidebar = app.focus.view == FocusView::Sidebar;
        let in_background = updates::opens_in_background(self.inner.updates.arrived_by_update(), activate);
        let Some(saved) = app.window else { return WindowPlacement { saved: false, x: 0.0, y: 0.0, width: 0.0, height: 0.0, full_screen: false, activate, focus_sidebar, in_background, full_screen_when_active: false } };
        if self.qa_mode {
            return WindowPlacement { saved: true, x: -40_000.0, y: -40_000.0, width: saved.frame.width.max(record::WINDOW_MIN.0), height: saved.frame.height.max(record::WINDOW_MIN.1), full_screen: saved.full_screen, activate, focus_sidebar, in_background: false, full_screen_when_active: false };
        }
        let now: Vec<ScreenNow> = screens.into_iter().map(|screen| ScreenNow { id: screen.id, name: screen.name, frame: Rect { x: screen.x, y: screen.y, width: screen.width, height: screen.height }, visible: Rect { x: screen.visible_x, y: screen.visible_y, width: screen.visible_width, height: screen.visible_height } }).collect();
        let place = place_window(&saved, &now);
        let (full_screen, full_screen_when_active) = updates::full_screen_on_showing(place.full_screen, in_background);
        WindowPlacement { saved: true, x: place.frame.x, y: place.frame.y, width: place.frame.width, height: place.frame.height, full_screen, activate, focus_sidebar, in_background, full_screen_when_active }
    }

    /// The window moved, resized or went full screen. Off a window's delegate, so at most as often as those.
    pub fn window_changed(&self, frame: Option<Vec<f64>>, screen: Option<ScreenInfo>, full_screen: bool) {
        let mut app = lock(&self.inner.app);
        let saved = app.window.get_or_insert(WindowRecord { frame: Rect { x: 0.0, y: 0.0, width: 1440.0, height: 900.0 }, screen: None, full_screen: false });
        // Its frame in full screen isn't where leaving full screen takes it.
        if let Some([x, y, width, height]) = frame.as_deref() {
            saved.frame = Rect { x: *x, y: *y, width: *width, height: *height };
            saved.screen = screen.map(|screen| ScreenRecord { id: screen.id, name: recto_core::clean_line(&screen.name), frame: Rect { x: screen.x, y: screen.y, width: screen.width, height: screen.height } });
        }
        saved.full_screen = full_screen;
        drop(app);
        self.inner.saver.view_changed();
    }

    /// The sidebar's edge was dragged, or QA said so. Rust holds it to its limits.
    pub fn set_sidebar_width(&self, width: f64) {
        lock(&self.inner.app).sidebar_width = record::sidebar_width(width);
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// Where the contents are scrolled to now.
    pub fn contents_position(&self, y: f64) {
        lock(&self.inner.app).contents_y = y.is_finite().then_some(y);
        self.inner.saver.view_changed();
    }

    /// What has the keyboard.
    pub fn focus_changed(&self, view: String) {
        if let Some(view) = FocusView::from_name(&view) {
            lock(&self.inner.app).focus.view = view;
            self.inner.saver.view_changed();
        }
    }

    pub fn focus_view(&self) -> String {
        lock(&self.inner.app).focus.view.name().to_string()
    }

    /// The app became the active one or stopped, and the window key or not. Going to the background writes what's changed: from there, nobody's watching.
    pub fn frontmost_changed(&self, app: bool, window_key: bool) {
        let mut state = lock(&self.inner.app);
        let went_back = state.focus.app_frontmost && !app;
        state.focus.app_frontmost = app;
        state.focus.window_key = window_key;
        drop(state);
        if went_back { self.inner.saver.write_soon() } else { self.inner.saver.view_changed() }
    }

    /// Light or dark as chosen, or `system`.
    pub fn appearance_choice(&self) -> String {
        match lock(&self.inner.app).appearance {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
        .to_string()
    }

    pub fn set_appearance_choice(&self, choice: String) {
        lock(&self.inner.app).appearance = match choice.as_str() {
            "light" => Appearance::Light,
            "dark" => Appearance::Dark,
            _ => Appearance::System,
        };
        self.inner.saver.view_changed();
    }

    /// A sheet or panel opened over the window, or closed. An update waits for it.
    pub fn sheet_changed(&self, open: bool) {
        if open {
            self.inner.sheets.fetch_add(1, Ordering::SeqCst);
        } else {
            let _ = self.inner.sheets.try_update(Ordering::SeqCst, Ordering::SeqCst, |count| count.checked_sub(1));
        }
    }

    /// The user touched the app. One number stored, on each event, with no timer: an automatic update asks how long it's been.
    pub fn note_user_input(&self) {
        self.inner.note_input();
    }

    /// The pointer moved over the app, or something else that shows someone's there and isn't an action. Owner, 2026-11-07: "hasn't touched". One number stored, and it's only pressed keys and clicks that say a notice has been read.
    pub fn note_user_nearby(&self) {
        self.inner.note_nearby();
    }

    /// Seconds since the app was last touched, for QA.
    pub fn seconds_since_input(&self) -> f64 {
        self.inner.hand_off_check().seconds_since_input
    }

    /// Whether an update could go now. Cheap: three counters and a clock.
    pub fn hand_off_check(&self) -> HandOffCheck {
        self.inner.hand_off_check()
    }

    /// **For the updater.** Writes everything, forces it to the drive, and reads it back, refusing if something's in flight. Goes ahead only on `Ok`, then calls `hand_off_quit_now`. Blocks on the disk, so call it from its own thread.
    pub fn hand_off(&self, limit_ms: u64) -> Result<HandOffReport, HandOffError> {
        self.inner.hand_off(Duration::from_millis(limit_ms))
    }

    /// After a good `hand_off`: quits with no prompt. Does nothing, and says so, if there hasn't been one.
    pub fn hand_off_quit_now(&self) -> bool {
        self.inner.hand_off_quit_now()
    }

    /// The hand-off isn't going ahead after all: commands and what a person does are taken again.
    pub fn hand_off_cancelled(&self) {
        self.inner.hand_off_cancelled();
    }

    /// What this copy of the app is, from the host. Before `begin_restore`, which is what reads what the updater remembers.
    pub fn set_update_facts(&self, version: String, bundle_path: String, system_version: String) {
        self.inner.note_update_facts(version, bundle_path, system_version);
    }

    /// When the host's one timer should go off, if at all. It's the same until something happens, and the host arms again only when it isn't.
    pub fn update_timer(&self) -> Option<updates::UpdateTimer> {
        updates::timer_for(self.inner.updates.next_wake(), self.inner.updates.now_ms())
    }

    /// The timer's gone off. Quick: what it starts is on a thread of its own.
    pub fn update_timer_fired(&self) {
        self.inner.update_timer_fired();
    }

    /// The Settings window, as it stands.
    pub fn settings_model(&self) -> updates::SettingsModel {
        self.inner.settings_model()
    }

    /// A switch in Settings was flipped.
    pub fn setting_chosen(&self, id: String, on: bool) {
        self.inner.setting_chosen(&id, on);
    }

    /// The Settings window opened, moved or closed. It's part of what comes back.
    pub fn settings_window_changed(&self, open: bool, x: f64, y: f64) {
        self.inner.settings_window_changed(open.then_some(SettingsWindow { x, y }));
    }

    /// Whether the Settings window was open.
    pub fn settings_window_was_open(&self) -> bool {
        self.inner.settings_window().is_some()
    }

    /// Where the Settings window goes on the screens there are now: where it was, if that's still on one. Otherwise wherever the host starts one.
    pub fn settings_window_place(&self, screens: Vec<ScreenInfo>) -> Option<Vec<f64>> {
        updates::settings_place(self.inner.settings_window(), &screens)
    }

    /// Whether this launch comes up behind what the user's doing: see [`WindowPlacement::in_background`].
    pub fn opens_in_background(&self) -> bool {
        updates::opens_in_background(self.inner.updates.arrived_by_update(), lock(&self.inner.app).focus.app_frontmost)
    }

    /// A quit was asked for, by the menu or the system. Everything's written, on a thread of its own, and `quit_answer` says what to do. Mac apps with Resume don't ask on the way out, and neither does this, as unsaved edits come back: closing a tab with them is where Don't Save, Cancel or Save belongs. It asks only if they couldn't be kept.
    pub fn quit_requested(&self) {
        let inner = Arc::clone(&self.inner);
        // What's done from here wouldn't come back. If it turns out it isn't quitting, it's taken again.
        inner.set_frozen(true);
        inner.updates.stop();
        let _ = std::thread::Builder::new().name("recto-quit".into()).stack_size(256 * 1024).spawn(move || {
            let (sender, receiver) = std::sync::mpsc::channel();
            let flushing = Arc::clone(&inner);
            let _ = std::thread::Builder::new().name("recto-flush".into()).stack_size(512 * 1024).spawn(move || {
                let _ = sender.send(flushing.saver.flush(false, Duration::from_secs(2)));
            });
            let outcome = receiver.recv_timeout(QUIT_PATIENCE);
            let fine = matches!(outcome, Ok(Ok(_))) || !inner.saver.store.holds_the_record();
            let unsaved = inner.host.session.unsaved_tabs();
            if !(fine || unsaved.is_empty()) {
                inner.set_frozen(false);
                inner.updates.stop_over();
            }
            inner.listener.quit_answer(if fine || unsaved.is_empty() {
                QuitAnswer { go: true, title: String::new(), message: String::new(), quit_button: String::new(), stay_button: String::new() }
            } else {
                QuitAnswer { go: false, title: "Your unsaved edits couldn't be kept".into(), message: "RectoPDF couldn't write them to the state folder. If you quit now, they'll be lost.".into(), quit_button: "Quit Anyway".into(), stay_button: "Stay".into() }
            });
        });
    }

    /// Ends the socket, on the way out. Returns at once.
    pub fn shutdown(&self) {
        if let Some(stopper) = lock(&self.inner.stopper).take() {
            stopper.stop();
        }
    }
}

/// Puts the session as what was read says, and lets everything that was waiting for that go on: the window, the socket, files named at launch.
fn apply_restored(inner: &Arc<Inner>, loaded: Loaded, synced: &mut Synced, reads: &mut FileReads, deferred: &mut Vec<PathBuf>) {
    let source: Arc<dyn HistorySource> = Arc::clone(&inner.saver.store) as Arc<dyn HistorySource>;
    inner.host.session.with_session(|session| {
        session.set_history_source(source);
        session.set_sleep_files(true);
    });
    // This goes for `session restore` too: nothing drawn stays from before.
    if synced.tab.take().is_some() {
        inner.engine.deactivate();
    }
    let mut notice = notice_for(&loaded);
    let host = &inner.host;
    let mut holding = None;
    let holds_the_record = inner.saver.store.holds_the_record();
    host.session.with_session(|session| session.set_journaling(holds_the_record));
    let first_time = !inner.restored.load(Ordering::SeqCst);
    match loaded {
        Loaded::Restored(restored) => {
            let restored = *restored;
            let application = host.session.with_session(|session| session.restore(&restored.record, restored.journals));
            // It's been through every check, so this isn't expected, and it isn't to be passed over.
            if application.is_err() {
                notice = Some(record::notices::DAMAGED);
            }
            if application.is_ok() {
                let saved = &restored.record.app;
                if let Some(paper_set) = saved.sidebar_width {
                    lock(&inner.app).sidebar_width = record::sidebar_width(paper_set);
                }
                inner.panel_open.store(saved.panel_open, Ordering::SeqCst);
                {
                    let mut app = lock(&inner.app);
                    app.settings_window = saved.settings_window;
                    app.window_closed = saved.window_closed;
                    app.window = saved.window.clone();
                    app.appearance = saved.appearance;
                    app.focus = saved.focus;
                    let active = restored.record.active.map(TabId);
                    app.contents_resume = active.zip(saved.contents_scroll);
                    app.selection_resume = active.and_then(|id| restored.record.tabs.iter().find(|tab| tab.id == id.0)?.selection.map(|selection| (id, selection)));
                    app.contents_y = saved.contents_scroll.map(|scroll| scroll.y);
                }
                // A code this build doesn't know has no words, and goes.
                *lock(&host.notice) = saved.notice.clone().filter(|code| record::notices::words(code).is_some());
                holding = host.session.with_session(|session| session.active().and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id)))));
            }
        }
        Loaded::Nothing if first_time => migrate_older_settings(inner),
        _ => {}
    }
    if let Some(words) = notice {
        *lock(&host.notice) = Some(words.to_string());
    }
    if let Some((id, path, stamp)) = holding {
        // With a sleep file, the first frame has what it needs. Nothing waits for the PDF, and it's looked at on the way past.
        if inner.host.session.with_session(|session| session.wake_from_sleep_file(id)) {
            inner.host.session.with_session(|session| session.wake_active());
            inner.engine.note_wake(id.0, Instant::now());
        } else {
            *lock(&inner.holding_tab) = Some(id);
            reads.wanted_tab = Some(id);
            reads.start_restored(inner, id, path, stamp);
        }
        look_at_other_files(inner, id);
    }
    inner.saver.start();
    inner.restored.store(true, Ordering::SeqCst);
    if !first_time {
        return;
    }
    start_serving(inner);
    for path in deferred.drain(..) {
        inner.core.push(CoreCall::Open(path));
    }
}

/// The two small files that held the paper and the agent icon before there was a record. Read once, and they go when the record's written.
fn migrate_older_settings(inner: &Inner) {
    let Some(paths) = &inner.paths else { return };
    let paper = recto_platform::local::read_private_file(&paths.root.join(PAPER_FILE), 16).ok().flatten().and_then(|bytes| std::str::from_utf8(&bytes).ok().and_then(Paper::from_name));
    let hidden = paths.root.join(AGENT_ICON_FILE).exists();
    if paper.is_none() && !hidden {
        return;
    }
    inner.host.session.with_session(|session| session.set_view(ViewSettings { paper: paper.or(session.view().paper), agent_icon_hidden: hidden, ..session.view() }));
    inner.saver.write_soon();
    let _ = std::fs::remove_file(paths.root.join(PAPER_FILE));
    let _ = std::fs::remove_file(paths.root.join(AGENT_ICON_FILE));
}

/// One pass by one thread over every other tab's file, once after a restore. A stat each, or a lookup by number if that wasn't it. It's gone as soon as it's been round, with nothing kept for a file that's as it was, and it can sit in a file on a dead volume as long as that likes.
fn look_at_other_files(inner: &Arc<Inner>, active: TabId) {
    let inner = Arc::clone(inner);
    let _ = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
        let files = inner.host.session.with_session(|session| session.files_to_look_at());
        let mut changed = false;
        for (id, path, stamp) in files {
            if id == active {
                continue;
            }
            let status = look_at_file(&path, stamp);
            let is_as_it_was = matches!(&status, FileStatus::Same(now) if Some(*now) == stamp) || matches!(status, FileStatus::NotLooked);
            if !is_as_it_was {
                inner.host.session.with_session(|session| session.apply_file_status(id, status));
                changed = true;
            }
        }
        if changed {
            inner.core.push(CoreCall::Reconcile);
        }
    });
}

/// Runs the socket from here on, now the session's as it was: nothing asked of it could be about an empty one, or open over what's coming back.
fn start_serving(inner: &Arc<Inner>) {
    let waiting = lock(&inner.waiting_server).take();
    let ending = Arc::clone(&inner.listener);
    let served = Arc::clone(&inner.host);
    let inner = Arc::clone(inner);
    // Blocked in `poll` with nothing happening, as `recto serve` is.
    let _ = std::thread::Builder::new()
        .name("recto-control".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            // The socket wasn't free at the start. If that's the app this one takes over from, on its way out, it soon will be, and an update mustn't lose the socket to that. If it's another session it stays that one's, and this thread goes.
            let began = Instant::now();
            let mut server = waiting;
            while server.is_none() && began.elapsed() < SOCKET_PATIENCE {
                std::thread::sleep(Duration::from_millis(20));
                server = inner.paths.as_ref().and_then(|paths| Server::bind(paths).ok());
                if let Some(bound) = &server {
                    *lock(&inner.stopper) = bound.stopper().ok();
                    *lock(&inner.announcer) = bound.announcer().ok();
                }
            }
            let Some(server) = server else { return };
            drop(inner);
            let _ = server.run(served, Limits::default());
            ending.session_ended();
        });
}

fn session_thread(inner: &Arc<Inner>) {
    let mut synced = Synced::default();
    let mut reads = FileReads::default();
    let mut deferred: Vec<PathBuf> = Vec::new();
    loop {
        // Parked for the queue and the state's one deadline, and nothing else.
        let Some(call) = inner.core.next_or_due(|| match (inner.saver.wait_for_due(), inner.sleeper.wait_for_due()) {
            (Some(one), Some(other)) => Some(one.min(other)),
            (one, other) => one.or(other),
        }) else {
            if inner.saver.wait_for_due().is_some_and(|wait| wait.is_zero()) {
                inner.saver.spawn_writer();
            }
            if inner.sleeper.wait_for_due().is_some_and(|wait| wait.is_zero())
                && let Some(due) = inner.dormancy_due()
            {
                inner.go_dormant(&due, sleeper::Why::LeftAlone);
                // With what's gone, the page view lets go of it too, and the next deadline's worked out.
                inner.core.push(CoreCall::Reconcile);
            }
            continue;
        };
        let mut events = Vec::new();
        let host = &inner.host;
        match call {
            CoreCall::Reconcile => {}
            CoreCall::Restored(loaded) => {
                let first_time = !inner.restored.load(Ordering::SeqCst);
                apply_restored(inner, *loaded, &mut synced, &mut reads, &mut deferred);
                reconcile(inner, &mut synced, &mut events);
                if first_time {
                    inner.listener.restore_ready();
                }
                continue;
            }
            CoreCall::RestorePatienceOver => {
                if !inner.restored.load(Ordering::SeqCst) {
                    // The state hasn't come. The window goes up without it, and when it does come it's taken in.
                    inner.listener.restore_ready();
                }
                // A tab's file not answering: on to what's on the record. The page says it isn't answering.
                if lock(&inner.holding_tab).take().is_some() {
                    host.session.with_session(|session| session.wake_active());
                }
            }
            CoreCall::Open(path) if !inner.restored.load(Ordering::SeqCst) => {
                deferred.push(path);
                continue;
            }
            CoreCall::Tap(id) => {
                inner.engine.note_wake(id.0, Instant::now());
                // A cold tab's file is read on a thread of its own, as an `open` is, so a file on a dead volume holds up neither this thread nor anything that asks about tabs, and a later tap goes ahead.
                match host.session.with_session(|session| session.wake_for_tap(id)) {
                    Some((tab, path)) => {
                        reads.wanted_tab = Some(tab);
                        reads.start(inner, Reading::Tab(tab), path);
                    }
                    None => {
                        reads.wanted_tab = None;
                        tap_now(inner, id, &mut events);
                    }
                }
            }
            CoreCall::GoTo(id, page) => {
                // `go_to` clamps: a contents entry can't send the reader off the end.
                if host.session.with_session(|session| session.go_to(id, page)).is_ok() {
                    events.push(Event::new("page_changed", json!({ "tab": id.0, "page": page + 1 })));
                }
            }
            CoreCall::Open(path) => {
                *lock(&inner.open_failure) = None;
                let cold = host.session.with_session(|session| session.tab_for_path(&path).and_then(|id| session.wake_for_tap(id)));
                if let Some((tab, file)) = cold {
                    reads.wanted_tab = Some(tab);
                    reads.start(inner, Reading::Tab(tab), file);
                } else if host.session.with_session(|session| session.go_to_open_file(&path)).is_some() {
                    reads.wanted_tab = None;
                } else {
                    // A link or a moved name is found once it's been looked at, on the thread reading it.
                    reads.wanted_tab = None;
                    reads.start(inner, Reading::File(path.clone()), path);
                }
            }
            CoreCall::Read(read) => reads.finished(inner, *read, &mut events),
            CoreCall::SetPaper(paper) => set_view(inner, &mut events, |view| view.paper = Some(paper)),
            CoreCall::SleepTabs(tabs, why) => {
                inner.go_dormant(&tabs, why);
            }
            CoreCall::MemoryPressure(critical) => inner.memory_pressure(critical),
            CoreCall::WokenFileIsOther(id, status) => {
                host.session.with_session(|session| {
                    session.apply_file_status(id, status);
                    session.forget_read(id);
                });
                let reread = host.session.with_session(|session| session.active().filter(|active| *active == id).and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id)))));
                if let Some((id, path, stamp)) = reread {
                    reads.wanted_tab = Some(id);
                    reads.start_restored(inner, id, path, stamp);
                }
            }
            CoreCall::LookAgain(id) => {
                if let Some(inner) = inner.saver.inner() {
                    sleeper::check_woken_file(&inner, id);
                }
            }
            CoreCall::FileLength(id, pages, version) => {
                // What a sleep file said of it is wrong, so it's read after all.
                let wrong = host.session.with_session(|session| session.file_length_is(id, pages, version));
                let reread = host.session.with_session(|session| session.active().filter(|active| wrong && *active == id).and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id)))));
                if let Some((id, path, stamp)) = reread {
                    reads.wanted_tab = Some(id);
                    reads.start_restored(inner, id, path, stamp);
                }
            }
            CoreCall::ClearEditHistory => inner.host.session.with_session(|session| {
                if let Some(id) = session.active() {
                    session.clear_history(id, recto_session::ClearReason::UserAsked);
                }
            }),
            CoreCall::ToggleAgentIcon => set_view(inner, &mut events, |view| view.agent_icon_hidden = !view.agent_icon_hidden),
            CoreCall::SetSidebarHidden(hidden) => set_view(inner, &mut events, |view| view.sidebar_hidden = hidden),
            CoreCall::SetSpread(spread) => host.session.with_session(|session| {
                if let Some(id) = session.active()
                    && session.set_spread(id, spread)
                {
                    events.push(Event::new("view_changed", json!({ "tab": id.0, "layout": spread.name() })));
                }
            }),
            CoreCall::Flush(marker) => {
                reconcile(inner, &mut synced, &mut events);
                // What was asked before this includes any file being read for it.
                if reads.running.is_empty() {
                    marker.finished();
                } else {
                    reads.flushes.push(marker);
                }
                continue;
            }
            CoreCall::Page(page, failure, tab) => {
                host.session.with_session(|session| session.note_drawn(DrawNote { tab, page, failure }));
                events.push(Event::new(if failure.is_some() { "page_failed" } else { "page_drawn" }, json!({ "tab": tab.0, "page": page + 1 })));
                // Nothing the sidebar or the reader shows comes from this. Going round for it would rebuild the model per tile drawn.
                if let Some(announcer) = lock(&inner.announcer).as_ref() {
                    announcer.announce(events);
                }
                continue;
            }
        }
        reconcile(inner, &mut synced, &mut events);
        if reads.running.is_empty() {
            for marker in reads.flushes.drain(..) {
                marker.finished();
            }
        }
        if !events.is_empty()
            && let Some(announcer) = lock(&inner.announcer).as_ref()
        {
            announcer.announce(events);
        }
    }
}

/// What a file's being read for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Reading {
    Tab(TabId),
    File(PathBuf),
    /// A restored active tab: what's become of its file, first, then its facts.
    Restored(TabId),
}

/// A file read off the session thread, on its way back to it.
#[derive(Debug)]
struct FileRead {
    what: Reading,
    loaded: Result<DocFacts, OpenError>,
    stamp: Option<FileStamp>,
    /// What a look found, for a restored tab.
    status: Option<FileStatus>,
    /// The history it was left with, for a file being opened.
    history: Option<recto_session::TakenHistory>,
}

/// Reads of files under way off the session thread. A file on a dead volume holds up the one thread reading it, and neither this one nor anyone else's tap.
#[derive(Default)]
struct FileReads {
    running: HashSet<Reading>,
    /// The tab the reader last asked for that's being read. Reading it isn't asking again: they may have gone elsewhere by the time it's done.
    wanted_tab: Option<TabId>,
    flushes: Vec<Arc<Flushed>>,
    /// The last file being opened, so several opened together keep their order unless one isn't answering.
    last_open: Option<Arc<Flushed>>,
}

/// What's said when there are more files not answering than can be waited on.
const TOO_MANY_READS: &str = "Several files aren't answering, so this one wasn't opened. Their disk may have gone to sleep or been disconnected.";

/// How long a file being opened waits for the one opened before it, so tabs come in the order asked for, unless that one isn't answering.
const OPEN_ORDER_WAIT: Duration = Duration::from_secs(1);

impl FileReads {
    /// Reads a restored active tab's file: what's become of it first, as it may be somewhere else now. On a thread of its own, as any other is.
    fn start_restored(&mut self, inner: &Arc<Inner>, id: TabId, path: PathBuf, stamp: Option<FileStamp>) {
        let what = Reading::Restored(id);
        if !self.running.insert(what.clone()) {
            return;
        }
        inner.reads_running.fetch_add(1, Ordering::SeqCst);
        let core = Arc::clone(&inner.core);
        let spawned = std::thread::Builder::new().name("recto-read".into()).stack_size(1024 * 1024).spawn(move || {
            let status = look_at_file(&path, stamp);
            let path = if let FileStatus::Moved(moved, _) = &status { moved.clone() } else { path };
            let stamp = recto_platform::file_stamp(&path).ok();
            let loaded = DrawableLoader.load(&path);
            core.push(CoreCall::Read(Box::new(FileRead { what, loaded, stamp, status: Some(status), history: None })));
        });
        if spawned.is_err() {
            self.running.remove(&Reading::Restored(id));
            inner.reads_running.fetch_sub(1, Ordering::SeqCst);
            *lock(&inner.holding_tab) = None;
            inner.host.session.with_session(|session| session.wake_active());
        }
    }

    fn start(&mut self, inner: &Arc<Inner>, what: Reading, path: PathBuf) {
        if self.running.contains(&what) {
            return;
        }
        if self.running.len() >= MAX_FILE_READS {
            *lock(&inner.open_failure) = Some(TOO_MANY_READS.to_string());
            return;
        }
        let core = Arc::clone(&inner.core);
        let (before, done) = if matches!(what, Reading::File(_)) {
            let done = Arc::new(Flushed::default());
            (self.last_open.replace(Arc::clone(&done)), Some(done))
        } else {
            (None, None)
        };
        let reading = what.clone();
        let saver = Arc::clone(&inner.saver);
        let inner_for_read = Arc::clone(inner);
        let spawned = std::thread::Builder::new().name("recto-read".into()).stack_size(1024 * 1024).spawn(move || {
            while inner_for_read.qa_hold_reads.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let stamp = recto_platform::file_stamp(&path).ok();
            let loaded = DrawableLoader.load(&path);
            // A PDF already open by another name keeps what it has. The session says for sure when this gets there.
            let already = matches!(reading, Reading::File(_)) && inner_for_read.host.session.with_session(|session| session.tab_for_file(&path, stamp).is_some());
            // Only after a good read: taking it up marks it as open, and a file that couldn't be read won't have a tab to hold it.
            let history = if matches!(reading, Reading::File(_)) && !already && loaded.is_ok() { saver.store.history_of(stamp, &path) } else { None };
            if let Some(before) = before {
                before.wait(OPEN_ORDER_WAIT);
            }
            core.push(CoreCall::Read(Box::new(FileRead { what: reading, loaded, stamp, status: None, history })));
            if let Some(done) = done {
                done.finished();
            }
        });
        if spawned.is_ok() {
            self.running.insert(what);
            inner.reads_running.fetch_add(1, Ordering::SeqCst);
        } else {
            *lock(&inner.open_failure) = Some(TOO_MANY_READS.to_string());
        }
    }

    fn finished(&mut self, inner: &Arc<Inner>, read: FileRead, events: &mut Vec<Event>) {
        self.running.remove(&read.what);
        inner.reads_running.fetch_sub(1, Ordering::SeqCst);
        let host = &inner.host;
        match read.what {
            Reading::Restored(tab) => {
                if let Some(status) = read.status {
                    host.session.with_session(|session| session.apply_file_status(tab, status));
                }
                host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                // The tab it's for may have been left already.
                if lock(&inner.holding_tab).take().is_some() || self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                    host.session.with_session(|session| if session.active() == Some(tab) { session.wake_active() });
                }
            }
            Reading::Tab(tab) => {
                // Only if it's still what's wanted. If not, there's no telling how long till it's used, and it'll be read fresh when it's asked for.
                if self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                    host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                    tap_now(inner, tab, events);
                }
            }
            Reading::File(path) => match read.loaded.map_err(SessionError::Open).and_then(|facts| host.session.with_session(|session| session.open_with_facts(&path, facts, read.stamp, read.history))) {
                Ok(id) => {
                    events.push(Event::new("tab_opened", json!({ "tab": id.0 })));
                }
                Err(error) => *lock(&inner.open_failure) = Some(error.message().to_string()),
            },
        }
    }
}

/// A change to how pages are shown, as `recto view` makes it.
fn set_view(inner: &Inner, events: &mut Vec<Event>, change: impl FnOnce(&mut ViewSettings)) {
    let view = inner.host.session.with_session(|session| {
        let mut view = session.view();
        change(&mut view);
        session.set_view(view);
        view
    });
    events.push(recto_agent::view_changed(view));
}

/// What a tap on a tab does, once whatever it needs from disk is here.
fn tap_now(inner: &Inner, id: TabId, events: &mut Vec<Event>) {
    let host = &inner.host;
    let was = host.session.with_session(|session| session.active());
    let tapped = host.session.with_session(|session| session.tap(id));
    if tapped.is_ok() && was != Some(id) {
        events.push(Event::new("tab_activated", json!({ "tab": id.0 })));
    }
}

/// By how many rows a change moved the place, for the host to take back so it stays put on screen: the place's row if it was listed before, or else the row it came from if that's listed still, or else the nearest chapter of it that was.
fn place_shift(before: &[ContentsEntry], after: &[ContentsEntry], (from, removed, inserted): (u32, u32, u32), place_before: Option<u32>, place_after: Option<u32>) -> i32 {
    let (from, removed, inserted) = (from as usize, removed as usize, inserted as usize);
    let rows = |count: usize| i32::try_from(count).unwrap_or(i32::MAX);
    let Some(place) = place_after.map(|row| row as usize).filter(|&row| row < after.len()) else { return 0 };
    if place < from {
        return 0;
    }
    if place >= from + inserted {
        return rows(inserted) - rows(removed);
    }
    let (gone, came) = (&before[from..from + removed], &after[from..from + inserted]);
    let find = |list: &[ContentsEntry], wanted: &ContentsEntry| list.iter().position(|entry| entry == wanted);
    if let Some(was) = find(gone, &after[place]) {
        return rows(place - from) - rows(was);
    }
    match place_before.map(|row| row as usize) {
        Some(old) if old >= from + removed => return rows(inserted) - rows(removed),
        Some(old) if old >= from => {
            if let Some(now) = find(came, &before[old]) {
                return rows(now) - rows(old - from);
            }
        }
        _ => {}
    }
    let mut depth = after[place].depth;
    for (index, entry) in came[..place - from].iter().enumerate().rev() {
        if entry.depth < depth {
            depth = entry.depth;
            if let Some(was) = find(gone, entry) {
                return rows(index) - rows(was);
            }
        }
    }
    0
}

/// Where two lists of contents rows differ: from which row, how many went, and how many came. Their matching starts and ends are compared, and it's only asked when the lists differ, which is when a chapter closes or opens.
fn rows_changed(before: &[ContentsEntry], after: &[ContentsEntry]) -> (u32, u32, u32) {
    if std::ptr::eq(before, after) {
        return (0, 0, 0);
    }
    let start = before.iter().zip(after).take_while(|(one, two)| one == two).count();
    let end = before[start..].iter().rev().zip(after[start..].iter().rev()).take_while(|(one, two)| one == two).count();
    let count = |list: &[ContentsEntry]| u32::try_from(list.len() - start - end).unwrap_or(u32::MAX);
    (u32::try_from(start).unwrap_or(u32::MAX), count(before), count(after))
}

/// The session's word for a layout, as the page view has it.
fn reader_spread(spread: record::Spread) -> recto_reader::layout::Spread {
    match spread {
        record::Spread::Single => recto_reader::layout::Spread::Single,
        record::Spread::TwoUp => recto_reader::layout::Spread::TwoUp,
        record::Spread::TwoUpCover => recto_reader::layout::Spread::TwoUpCover,
    }
}

/// Brings everything into line with the session, on the session thread. The place in a tab can move two ways, by the reader or by an agent, and each is passed to the other side.
fn reconcile(inner: &Inner, synced: &mut Synced, events: &mut Vec<Event>) {
    // A tab about to be shown that wasn't: its file's looked at first, so nothing of a version that's been replaced goes up, held or not.
    let showing = inner.host.session.with_session(|session| session.active()).filter(|id| synced.tab != Some(*id) && lock(&inner.holding_tab).is_none());
    if let Some(id) = showing {
        sleeper::look_before_showing(inner, id);
    }
    let moved = inner.engine.take_moved_page();
    // What the page view holds of tabs it's about to let go of goes in the session first, as a cold tab holds nothing there.
    inner.take_places();
    let synced_tab = synced.tab;
    let (snapshot, sidebar, tabs, contents, rail_filled, view) = inner.host.session.with_session(|session| {
        let active = session.active();
        if let (Some(id), Some(page)) = (active, moved)
            && synced_tab == Some(id)
            && session.go_to(id, page).is_ok()
        {
            synced.page = session.tab(id).map_or(page, |tab| tab.page);
            events.push(Event::new("page_changed", json!({ "tab": id.0, "page": synced.page + 1 })));
        }
        let snapshot = active.and_then(|id| {
            let tab = session.tab(id)?;
            let own = session.path_of(id)?;
            let same_file = session.own_stamp(id).map(sleeper::version_of);
            let map = match session.page_edits(id) {
                None => PageMap { files: vec![own], slots: None, own: same_file },
                Some(edits) => {
                    let mut files = vec![own];
                    files.extend(edits.added_sources().iter().map(|added| added.path.to_path_buf()));
                    PageMap { files, slots: Some(edits.slots().iter().map(|slot| (slot.source, slot.page)).collect()), own: same_file }
                }
            };
            let warm: Vec<u32> = session.warm_tabs().filter(|other| *other != id).map(|other| other.0).collect();
            let problem = session.tab_notice_message(id).map(str::to_string).or_else(|| tab.problem.map(|problem| problem.message().to_string()));
            let saved = session.place_to_show(id).map(|place| SavedPlace { page: place.page, anchor: (place.anchor_page, place.within), x: place.x, y: place.y, document_height: place.document_height, magnification: place.zoom, page_size: (place.page_width > 0.0 && place.page_height > 0.0).then_some(recto_reader::layout::PageBox { width: place.page_width, height: place.page_height }) });
            // Edits that can't be read just now can't say what's on the pages, so it's the file that's shown.
            let pages = if session.list_is_asleep(id) { tab.page_count } else { session.pages_in(id) };
            Some((id, pages, tab.page, map, warm, problem, saved, tab.spread))
        });
        let model = session.sidebar();
        let mut current = None;
        let mut active_index = None;
        let (mut tab_count, mut contents_row_count) = (0u32, 0u32);
        for row in &model.rows {
            if row.kind == SessionRowKind::Contents {
                if row.current {
                    current = Some(contents_row_count);
                }
                contents_row_count += 1;
            } else {
                if row.current {
                    active_index = Some(tab_count);
                }
                tab_count += 1;
            }
        }
        // Copied only when they've changed: a page crossed mustn't mean thousands of strings, for a long outline or for many tabs.
        let (held_tabs, held, was) = {
            let published = lock(&inner.published);
            (Arc::clone(&published.tabs), Arc::clone(&published.contents), Arc::clone(&published.sidebar))
        };
        let tab_rows = || model.rows.iter().filter(|row| row.kind != SessionRowKind::Contents);
        let same_tabs = held_tabs.len() == tab_count as usize && tab_rows().zip(held_tabs.iter()).all(|(row, tab)| row.tab.0 == tab.tab && row.current == tab.current && row.text == tab.text.as_str() && row.notice == tab.notice.as_deref());
        let tabs = if same_tabs {
            held_tabs
        } else {
            Arc::new(tab_rows().map(|row| SidebarRow { kind: RowKind::Tab, tab: row.tab.0, text: row.text.to_string(), page: row.page, depth: row.depth, current: row.current, rail_filled: false, rail_first: false, rail_last: false, notice: row.notice.map(str::to_string) }).collect())
        };
        let tabs_version = if same_tabs { was.tabs_version } else { was.tabs_version + 1 };
        let same = held.len() == contents_row_count as usize && model.rows.iter().filter(|row| row.kind == SessionRowKind::Contents).zip(held.iter()).all(|(row, entry)| row.tab.0 == entry.tab && row.page == entry.page && row.depth == entry.depth && row.text == entry.text.as_ref());
        let rail_filled = u32::try_from(model.rail_filled).unwrap_or(u32::MAX);
        let (contents, contents_version, change, contents_place_shift) = if same {
            (held, was.contents_version, (was.contents_changed_from, was.contents_removed, was.contents_inserted), was.contents_place_shift)
        } else {
            let list: Arc<Vec<ContentsEntry>> = Arc::new(model.rows.iter().filter(|row| row.kind == SessionRowKind::Contents).map(|row| ContentsEntry { tab: row.tab.0, text: row.text.into(), page: row.page, depth: row.depth }).collect());
            let change = rows_changed(&held, &list);
            let shift = place_shift(&held, &list, change, was.contents_current, current);
            (list, was.contents_version + 1, change, shift)
        };
        let view = SidebarView { tabs_version, tab_count, active_index, contents_count: contents_row_count, contents_version, contents_changed_from: change.0, contents_removed: change.1, contents_inserted: change.2, contents_place_shift, contents_current: current, toc_open: model.toc_open, place: model.place.clone(), edited: model.edited, undone_owner_steps: model.undone_owner_steps };
        (snapshot, view, tabs, contents, rail_filled, session.view())
    });
    // The paper first, so what's asked for next is drawn right.
    let dark = inner.dark.load(Ordering::SeqCst);
    let shown = Paper::resolved(view.paper, dark);
    // Warm paper is white paper with a tint over it in the compositor, so going to it or from it draws nothing and holds nothing more.
    let (paper, page_tint) = match shown {
        Paper::White => (PaperKind::White, None),
        Paper::Warm => (PaperKind::White, Some(WARM_PAPER)),
        Paper::Dark => (PaperKind::Dark, None),
    };
    if synced.paper != Some(paper) {
        synced.paper = Some(paper);
        inner.engine.set_paper(paper);
    }
    let mut problem = None;
    let layout = snapshot.as_ref().map_or(String::new(), |shown| shown.7.name().to_string());
    match snapshot {
        None => {
            if synced.tab.take().is_some() {
                inner.engine.deactivate();
            }
        }
        Some((id, pages, page, map, warm, tab_problem, saved, spread)) => {
            problem = tab_problem;
            // Its file's being looked at: nothing goes on the page until that's known.
            let holding = *lock(&inner.holding_tab) == Some(id) && synced.tab.is_none();
            if synced.tab != Some(id) && !holding {
                let mut follow = lock(&inner.contents_follow);
                follow.new_contents();
                let mut app = lock(&inner.app);
                match app.contents_resume.take() {
                    Some((tab, scroll)) if tab == id => follow.resume_at(scroll.y, scroll.following),
                    _ => {}
                }
            }
            if holding {
            } else if synced.tab != Some(id) || synced.pages != pages || synced.map != map {
                let left = inner.pictures_of_those_leaving();
                // Whichever way it's been asked for, the first frame's what its sleep file has, if it's got one and nothing's held of this tab.
                if !inner.engine.holds(id.0) {
                    inner.engine.note_wake(id.0, Instant::now());
                    inner.give_sleep_view(id);
                }
                inner.engine.activate(id.0, pages, page, map.clone(), &warm, saved, reader_spread(spread));
                synced.spread = Some(spread);
                // What was selected comes back on the tab it was on, once.
                let selection = lock(&inner.app).selection_resume.take().filter(|(tab, _)| *tab == id).map(|(_, selection)| selection);
                if let Some(selection) = selection {
                    inner.engine.restore_selection((selection.from_page, selection.from_char), (selection.to_page, selection.to_char));
                }
                inner.write_those_leaving(left);
                synced.tab = Some(id);
                synced.pages = pages;
                synced.map = map;
                synced.page = page;
                synced.warm = warm;
            } else {
                if synced.spread != Some(spread) {
                    inner.engine.set_spread(reader_spread(spread));
                    synced.spread = Some(spread);
                }
                if synced.page != page {
                    inner.engine.jump_to(page);
                    synced.page = page;
                }
                // A warm tab closed or gone cold: what it held goes now, and not at the next switch.
                if synced.warm != warm {
                    inner.engine.set_warm(&warm);
                    synced.warm = warm;
                }
            }
        }
    }
    let session_notice = lock(&inner.host.notice).as_deref().and_then(record::notices::words).map(str::to_string);
    let save_failed = inner.saver.has_failed().then(|| record::notices::SAVE_FAILED.to_string());
    let note = lock(&inner.open_failure).clone().or(save_failed).or(problem).or(session_notice).or_else(|| inner.engine.note());
    let holding = lock(&inner.holding_tab).is_some();
    let sidebar_width = lock(&inner.app).sidebar_width;
    let highlight = paper.highlight();
    let selection_notice = inner.engine.selection_notice().map(|notice| SelectionNotice { text: notice.text, action_title: notice.offers_copy_anyway.then(|| "Copy anyway".to_string()), fades: notice.fades }).or_else(|| inner.updates.notice().map(|notice| SelectionNotice { text: notice.text, action_title: None, fades: notice.fades }));
    let reader = ReaderView { home: synced.tab.is_none() && !holding, note, sidebar_hidden: view.sidebar_hidden, layout, place: sidebar.place.clone(), paper: shown.name().to_string(), paper_rgb: paper.rgb(), page_tint, agent_icon_hidden: view.agent_icon_hidden, sidebar_width, highlight_dark_colour: highlight.dark_colour, highlight_lighten: highlight.lighten, selection_notice };
    let mut published = lock(&inner.published);
    let changed = *published.sidebar != sidebar || published.reader != reader || published.rail_filled != rail_filled || !Arc::ptr_eq(&published.contents, &contents);
    if changed {
        published.tabs = tabs;
        published.sidebar = Arc::new(sidebar);
        published.reader = reader;
        published.contents = contents;
        published.rail_filled = rail_filled;
    }
    drop(published);
    if changed {
        inner.listener.model_changed();
    }
    // What's left alone comes due from here, and what's off screen may be over its budget.
    inner.check_woken_files();
    inner.rearm_dormancy();
    inner.look_at_budget();
    // Whatever just happened may be something to keep. Nothing's written unless it really differs when its time comes.
    if inner.host.session.with_session(|session| session.has_retired_journals()) {
        inner.saver.write_soon();
    } else {
        if inner.host.session.with_session(|session| session.has_journal_work()) {
            inner.saver.edit_made();
        }
        inner.saver.view_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: rows opening as the reader enters a chapter jump the list under a smooth drift, because the host wasn't told which went and which came. It must say exactly, a chapter closing and the next opening together included, and a change of nothing says nothing.
    #[test]
    fn opening_and_closing_rows_says_what_changed() {
        let row = |text: &str| ContentsEntry { tab: 1, text: text.into(), page: Some(1), depth: 0 };
        let list = |names: &[&str]| names.iter().map(|name| row(name)).collect::<Vec<_>>();
        let before = list(&["1", "2", "3"]);
        assert_eq!(rows_changed(&before, &list(&["1", "2", "2.1", "2.2", "3"])), (2, 0, 2));
        assert_eq!(rows_changed(&list(&["1", "2", "2.1", "2.2", "3"]), &before), (2, 2, 0));
        assert_eq!(rows_changed(&list(&["1", "2", "2.1", "3", "4"]), &list(&["1", "2", "3", "3.1", "4"])), (2, 2, 2), "2.1 and 3 go, and 3 and 3.1 come");
        assert_eq!(rows_changed(&before, &before), (0, 0, 0));
        assert_eq!(rows_changed(&[], &before), (0, 0, 3));
        assert_eq!(rows_changed(&before, &[]), (0, 3, 0));
    }

    /// Failure: a chapter closes as the reader leaves it, the place drops up the list by the rows that went, and the contents make a second move straight after the first to bring it back. How far the place was carried has to come out right wherever it starts and ends.
    #[test]
    fn a_row_change_says_how_far_it_carried_the_place() {
        let row = |text: &str| ContentsEntry { tab: 1, text: text.into(), page: Some(1), depth: u8::try_from(text.matches('.').count()).unwrap() };
        let list = |names: &[&str]| names.iter().map(|name| row(name)).collect::<Vec<_>>();
        let shift = |one: &[&str], place_before: u32, two: &[&str], place_after: u32| {
            let (before, after) = (list(one), list(two));
            place_shift(&before, &after, rows_changed(&before, &after), Some(place_before), Some(place_after))
        };
        let in_two = ["1", "2", "2.1", "2.2", "3", "4"];
        let in_three = ["1", "2", "3", "3.1", "4"];
        assert_eq!(shift(&in_two, 3, &in_three, 2), -2, "forward: 3 was listed, two rows further down");
        assert_eq!(shift(&in_two, 3, &in_three, 3), -2, "forward, to a section that wasn't listed: its chapter was");
        assert_eq!(shift(&in_three, 2, &in_two, 3), 2, "back: 2.2 wasn't listed, but 3, the row it came from, still is");
        assert_eq!(shift(&in_two, 5, &["1", "2", "2.1", "2.2", "3", "4", "4.1"], 5), 0, "everything happened below");
        assert_eq!(shift(&in_two, 5, &in_three, 4), -1, "everything happened above");
    }

    /// Failure: notes of tiles drawn fill the session thread's queue, and past the cap what a person did is thrown away: a tap, an open, the appearance. A look dropped when it's full leaves "a look is queued" set, so none is ever queued again. Only those notes can go, and repeats of one don't take a place.
    #[test]
    fn a_full_queue_loses_only_notes_of_pages_drawn() {
        let queue = CoreQueue::default();
        for page in 0..(MAX_QUEUED_CALLS as u32 + 100) {
            queue.push(CoreCall::Page(page, None, TabId(1)));
        }
        assert_eq!(lock(&queue.calls).len(), MAX_QUEUED_CALLS, "it stops at the cap");
        queue.push(CoreCall::Tap(TabId(2)));
        queue.push(CoreCall::Open(PathBuf::from("/d/a.pdf")));
        queue.push(CoreCall::SetSidebarHidden(true));
        queue.push(CoreCall::Reconcile);
        queue.push(CoreCall::Reconcile);
        queue.push(CoreCall::Flush(Arc::new(Flushed::default())));
        assert_eq!(lock(&queue.calls).len(), MAX_QUEUED_CALLS + 5, "all of those are in, one look among them");
        // Drained, the look it had queued is seen, and another can be asked for.
        while !lock(&queue.calls).is_empty() {
            queue.next();
        }
        assert!(!queue.reconcile_queued.load(Ordering::SeqCst));
        queue.push(CoreCall::Reconcile);
        assert_eq!(lock(&queue.calls).len(), 1);
    }

    /// Failure (review T4): past a hand-off's last look, a tap or an open is taken, and it's lost as what's on disk is what comes back. They aren't taken while it's frozen, and what isn't a person's doing still gets through, or the session would stop answering.
    #[test]
    fn a_frozen_queue_takes_nothing_a_person_does() {
        let queue = CoreQueue::default();
        queue.frozen.store(true, Ordering::SeqCst);
        queue.push(CoreCall::Tap(TabId(1)));
        queue.push(CoreCall::GoTo(TabId(1), 2));
        queue.push(CoreCall::Open(PathBuf::from("/d/a.pdf")));
        queue.push(CoreCall::SetSidebarHidden(true));
        queue.push(CoreCall::SetPaper(Paper::Dark));
        queue.push(CoreCall::ToggleAgentIcon);
        assert_eq!(lock(&queue.calls).len(), 0);
        queue.push(CoreCall::Reconcile);
        queue.push(CoreCall::Flush(Arc::new(Flushed::default())));
        assert_eq!(lock(&queue.calls).len(), 2);
        queue.frozen.store(false, Ordering::SeqCst);
        queue.push(CoreCall::Tap(TabId(1)));
        assert_eq!(lock(&queue.calls).len(), 3);
    }

    /// Failure: each tile of a page is a note, so an ordinary page of tiles uses a dozen places, and the queue's cap is reached by reading alone. In a row, one note for one page is enough.
    #[test]
    fn repeated_notes_of_a_page_take_one_place() {
        let queue = CoreQueue::default();
        for _ in 0..100 {
            queue.push(CoreCall::Page(3, None, TabId(1)));
        }
        assert_eq!(lock(&queue.calls).len(), 1);
        queue.push(CoreCall::Page(3, Some("page_failed"), TabId(1)));
        queue.push(CoreCall::Page(4, None, TabId(1)));
        assert_eq!(lock(&queue.calls).len(), 3);
    }

    struct TestEngineEvents;
    impl EngineEvents for TestEngineEvents {
        fn ops_ready(&self) {}
        fn note_changed(&self) {}
        fn page_moved(&self) {}
        fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
    }

    /// Failure: an agent at Control quits the app out from under the person using it, with no prompt, by `recto stop`. In a normal app it's refused, and the words are for whoever's asking.
    #[test]
    fn stop_is_refused_by_a_normal_app() {
        let host = AppHost { engine: Engine::new(EngineConfig::default(), Arc::new(TestEngineEvents)), session: SessionHost::new(DrawableLoader), core: Arc::new(CoreQueue::default()), qa: None, saver: Arc::new(Saver::new(RecordStore::open(&StatePaths::overridden(&std::env::temp_dir().join(format!("rf-{}", std::process::id()))).unwrap(), Flavor::App), Arc::new(CoreQueue::default()))), running: AtomicU32::new(0), handing_off: AtomicBool::new(false), notice: Mutex::new(None) };
        assert_eq!(host.stop_refusal().expect("refused").code, "only_the_user_quits");
    }

    /// Failure: the host and the core decide a quiet run differently: a word after `-NS...` is that option's value here, while the host's own search found `--qa` in it. It would launch off every screen with nothing watching for a QA run, or the other way round, and take the screen.
    #[test]
    fn a_launch_is_quiet_only_as_the_parser_says() {
        let words = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert!(launch_is_quiet(words(&["--qa"])));
        assert!(!launch_is_quiet(words(&[])));
        assert!(!launch_is_quiet(words(&["-NSSomething", "--qa"])), "that's the option's value");
        assert!(!launch_is_quiet(words(&["/d/--qa.pdf"])));
        assert!(launch_is_quiet(words(&["--not-an-option"])), "it won't get as far as a window");
    }

    /// Failure: QA options work on a normal launch, so a shipped app can be started with its limits changed, or a file named at launch is taken for an option.
    #[test]
    fn qa_options_only_go_with_a_qa_run() {
        let words = |list: &[&str]| list.iter().map(|word| (*word).to_string()).collect::<Vec<_>>();
        assert!(parse_args(&words(&["--qa-memory-limit-mib", "64"])).is_err());
        assert!(parse_args(&words(&["--qa-time-limit-ms", "5"])).is_err());
        let parsed = parse_args(&words(&["--qa", "--qa-memory-limit-mib", "64", "/d/a.pdf"])).unwrap();
        assert!(parsed.qa);
        assert_eq!(parsed.memory_limit_mib, Some(64));
        assert_eq!(parsed.open, vec![PathBuf::from("/d/a.pdf")]);
        assert!(parse_args(&words(&["--nope"])).is_err());
        assert!(parse_args(&words(&["--qa-memory-limit-mib"])).is_err());
        assert_eq!(parse_args(&words(&["-psn_0_12345"])).unwrap(), LaunchArgs::default(), "what macOS adds");
    }
}
