//! The bridge to the host, with no rules in it. The host draws what comes out of here and calls these for everything the user does.
//!
//! **Every call is cheap or returns at once.** Anything that can take time goes on the session thread, which is parked at rest, and the host hears when there's something new. Pixels never cross this: tiles travel as surfaces.
//! The session here is the one on the control socket, so `recto tabs` and `recto goto` act on what's on screen. The app's own actions go through `with_session`, as the owner's.

#![allow(unsafe_code)] // What UniFFI's macros generate. Nothing here writes any of its own.

pub mod chrome;
mod find;
mod generated_contents;
pub mod linking;
mod menus;
mod palette;
pub mod passwords;
mod printing;
mod qa;
mod records;
mod saver;
mod saving;
mod sidebar_access;
mod sleeper;
mod snapshot_tool;
pub mod updates;

pub use menus::{HostAction, MenuEntry, MenuModel, PanelItem, PanelModel, PanelSection};
use qa::QaBridge;
use recto_agent::{AgentError, Command, Condition, ConditionState, DrawableLoader, Event, Level};
pub use recto_app::PaneView;
pub use recto_app::RecentFile;
pub use recto_app::SelectionNotice;
use recto_app::queue::{CoreCall, CoreQueue, FileRead, Flushed, Reading};
pub use recto_app::reader_view::ReaderView;
use recto_app::watchdog::Watchdog;
use recto_app::{ContentsEntry, PaneSnapshot, panes, privacy};
use recto_control::StatePaths;
use recto_control::host::{Host, SessionHost, kept_json, store_error};
use recto_control::indexing::Notifier;
use recto_control::record_store::{Flavor, FlushReport, Loaded, RecordStore, notice_for};
use recto_control::server::{Announcer, Limits, Server, Stopper};
use recto_core::{DocFacts, OpenError};
use recto_platform::FileStamp;
use recto_reader::engine::EngineEvents;
use recto_reader::tiles::{MAX_ZOOM, MIN_ZOOM, Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_render_service::Policy;
use recto_session::HistorySource;
use recto_session::record::{
    self, AppRecord, Appearance, ContentsScroll, FileStatus, FocusRecord, FocusView, Rect, ScreenNow, ScreenRecord, SettingsWindow, WindowRecord, look_at_file,
    place_window,
};
use recto_session::{
    CONTENTS_ROW_HEIGHT, ContentsFollow, ContentsRowChange, ContentsView, DocLoader, DrawNote, FollowMove, Paper, SessionError, TAB_ROW_HEIGHT, TabId,
    ViewSettings, row_full_text, sidebar_heights_with_note,
};
use saver::Saver;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

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
const QUIT_PATIENCE: Duration = Duration::from_millis(recto_app::listener::QUIT_PATIENCE_MS);

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
    /// It goes into the Dock, as it was there when the update came. Shown, it's where the rest of this says.
    pub minimised: bool,
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
        Self {
            tabs: report.tabs as u64,
            edited_tabs: report.edited_tabs as u64,
            record_bytes: report.record_bytes,
            journal_bytes: report.journal_bytes,
            wrote_bytes: report.wrote_bytes,
            force_us: report.force_us,
            total_us: report.total_us,
        }
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
    /// A quiet line of Rust's over the contents, about generated contents. Its `notice` is what it means, for the pointer resting on it, and `action` what a tap does.
    Note,
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
    /// Something's wrong with this tab's file, in Rust's words. On a note, what it means.
    pub notice: Option<String>,
    /// What tapping a note does, by Rust's own name for it.
    pub action: Option<String>,
    /// A contents row with sections under it, which its chevron folds, and whether they show.
    pub expandable: bool,
    pub expanded: bool,
    /// A tab side by side with others, which reads as the active one does.
    pub in_comparison: bool,
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
    pub edited: bool,
    pub undone_owner_steps: bool,
    /// A quiet line over the active tab's contents, about generated contents, which stays put while they scroll. Drawn only while they're showing.
    pub contents_note: Option<SidebarRow>,
}

/// The scroll thumb, for every scrolling area of the window: `recto_session::SCROLL_THUMB`, where its widths and colours are decided and why.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct ScrollThumbLook {
    pub width: f64,
    pub hover_width: f64,
    pub hit_width: f64,
    pub inset: f64,
    pub min_length: f64,
    pub light_rgb: u32,
    pub light_alpha: f64,
    pub light_hover_alpha: f64,
    pub dark_rgb: u32,
    pub dark_alpha: f64,
    pub dark_hover_alpha: f64,
}

/// Rust's height rule, in points.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct SidebarLayout {
    pub tab_row: f64,
    pub contents_row: f64,
    pub contents_height: f64,
    pub tabs_scroll: bool,
    /// The note's height over the contents, or nothing.
    pub note_height: f64,
    /// Find's whole block over the tabs, and its list's part of that, both 0 while it's closed. The tabs and contents divide what's left.
    pub find_block: f64,
    pub find_list: f64,
    pub find_row: f64,
    pub find_field: f64,
    pub find_status: f64,
}

/// What to do with the contents' scroll, from Rust's rule about following.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Enum)]
pub enum ContentsMove {
    Stay,
    SetTo {
        y: f64,
    },
    /// Run frames, calling `contents_step` on each.
    Move,
}

/// A comparison's pane, as its label says it.
#[uniffi::remote(Record)]
pub struct PaneView {
    pub tab: u32,
    /// Its file's name, cleaned, so it's only ever drawn as text.
    pub title: String,
    /// It's the pane being read: the page number and the contents are its.
    pub reading: bool,
    /// Its tab's layout, by name, which its right-click ticks.
    pub layout: String,
    /// What's wrong with its file, in Rust's words, shown in the pane: a pane that isn't read has no other place to say so.
    pub note: Option<String>,
}

/// What the page area shows.
#[uniffi::remote(Record)]
pub struct ReaderView {
    /// Tabs side by side, left to right, a pane each. Empty with one tab showing, which is one pane with no label.
    pub panes: Vec<PaneView>,
    /// Synced scroll's on, which the right-click menu ticks.
    pub compare_synced: bool,
    /// No PDF is open, so it's the home state.
    pub home: bool,
    /// Something the reader needs to be told, such as a page at a lower resolution, in words.
    pub note: Option<String>,
    /// The sidebar's hidden, by its button or the View menu.
    pub sidebar_hidden: bool,
    /// The tab showing's layout, by its name: `single`, `two-up` or `two-up-cover`. Empty with none showing.
    pub layout: String,
    /// The tab showing has unsaved edits, which the red window button's dot shows.
    pub edited: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Edit mode's on, which the panel's Rotate goes by. Published, so drawing the panel never waits on the session.
    pub edit_mode: bool,
    /// Whether the panel's Rotate can turn the tab showing: there is one, more than the renderer can read it, and its edits can be read.
    pub can_rotate: bool,
    /// The page number in the lower right, such as "32 of 96". Empty with nothing open. The sidebar has no footer saying it again (the owner's default of 2026-11-08).
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
    /// A save of the tab showing is under way.
    pub saving: bool,
    /// What the Save icon beside the window's controls says, and the step a long save is at.
    pub save_tooltip: String,
    /// A save that didn't happen, for an alert over the window until it's answered.
    pub save_problem: Option<saving::SaveProblem>,
    /// What Save As's sheet says about this document, as for a signed one.
    pub save_as_note: Option<String>,
    /// The tab showing, which an action from the window is about. 0 with none.
    pub shown_tab: u32,
    /// Its file's name and folder, for Save As's sheet and the close prompt, so neither waits on the session.
    pub document_name: String,
    pub document_folder: String,
    /// Whether the Save icon is beside the window's controls: with unsaved edits, or while saving.
    pub save_button: bool,
    /// A print's pages are ready: call `take_print`, and hand the file to the system's printing.
    pub print_ready: bool,
    /// A password being asked for, for the system's sheet with a secure field.
    pub password_prompt: Option<passwords::PasswordPrompt>,
    /// Whether it does anything: not while saving, and not when Save over the file would break a person's signature, which Save As's unsigned copy is the way round.
    pub save_enabled: bool,
    /// Back and Forward can go somewhere in the tab being read: Go > Back and Forward, ⌘[ and ⌘], and a swipe.
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// A web or email address a click asked to open, for the sheet that asks first.
    pub link_prompt: Option<linking::LinkPrompt>,
    /// The page being read, by its label if it has one: what the go-to field starts with.
    pub page_text: String,
}

/// A file opened lately, for File > Open Recent and for ⌘K to offer. The worker building ⌘K calls `AppCore::recent_files` and opens one with `AppCore::open_recent`.
#[uniffi::remote(Record)]
pub struct RecentFile {
    /// Which file this is, however the list moves: what its Open Recent item's id carries, as `recent.<key>`.
    pub key: String,
    /// As a tab shows it: the file's name without `.pdf`, cleaned.
    pub name: String,
    /// The folder it's in, as a path, for telling two of the same name apart.
    pub folder: String,
    pub path: String,
    /// It's open in a tab now, so choosing it goes there.
    pub open: bool,
}

/// **`qa save-from`'s signature check,** on what was found of the file's signatures: a person's signature refuses it, and so does a file that couldn't be looked
/// at, as the save itself refuses what it can't look through (review round 2).
fn save_from_signature_check(facts: Result<recto_core::save::SignatureFacts, recto_core::OpenError>) -> Result<(), AgentError> {
    match facts {
        Ok(facts) if !facts.has_persons_signature() => Ok(()),
        Ok(_) => Err(AgentError::new(
            "signed_needs_copy",
            recto_core::save::SaveError::SignedNeedsCopy.message(),
            recto_agent::exit::FAILED,
        )),
        Err(error) => Err(AgentError::new(
            error.code(),
            format!("{} Its signatures couldn't be looked at, so nothing was put in place.", error.message()),
            recto_agent::exit::FAILED,
        )),
    }
}

/// What to say by the page about a copy.
#[uniffi::remote(Record)]
pub struct SelectionNotice {
    pub text: String,
    /// The button, if there's one.
    pub action_title: Option<String>,
    /// What the button does, for `perform`: `edit.copy_anyway` for a selection's, or an id naming the notice that offered it, so a button from a notice that's gone does nothing.
    pub action_id: Option<String>,
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
    /// The empty window's quiet line about the daily update check, or empty for none: [`privacy::home_note`].
    pub home_note: String,
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
    /// The view moved by the reader's own hand, a scroll or a drag of the scroller, and not by the host applying what it was told or the window changing. Synced scroll follows only that.
    pub by_reader: bool,
}

/// What the host does to its layers, in order.
#[derive(Clone, Debug, PartialEq, uniffi::Enum)]
pub enum ReaderOp {
    Clear,
    /// The document's size, and how far the view may scroll past its top and its end: the rest of the margin at each end.
    Document {
        width: f64,
        height: f64,
        top_room: f64,
        bottom_room: f64,
    },
    Magnify {
        value: f64,
    },
    ScrollTo {
        y: f64,
        seq: u64,
    },
    /// Across, when zoomed in.
    ScrollAcross {
        x: f64,
    },
    /// `width` and `height` are the page before it's turned, and `turns` quarter turns clockwise turn it, with its turned top left at `x` and `y`.
    ShowPage {
        page: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        fit: f64,
        paper_rgb: u32,
        turns: u8,
    },
    HidePage {
        page: u32,
    },
    /// `surface` is an `IOSurfaceRef` the host now owns one reference to, as a number, or zero if `rgb` is the colour.
    Attach {
        id: u64,
        page: u32,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        surface: u64,
        rgb: Option<u32>,
        crop_width: f64,
        crop_height: f64,
        below: bool,
    },
    Detach {
        id: u64,
    },
    /// All of what's selected on a page, in its own coordinates. Nothing clears it.
    Highlight {
        page: u32,
        rects: Vec<HighlightRect>,
    },
    /// The snapshot tool's box on a page, in its own coordinates, or none to take it off.
    SnapshotBox {
        page: u32,
        rect: Option<HighlightRect>,
    },
}

impl From<Op> for ReaderOp {
    fn from(op: Op) -> Self {
        match op {
            Op::Clear => Self::Clear,
            Op::Document {
                width,
                height,
                top_room,
                bottom_room,
            } => Self::Document {
                width,
                height,
                top_room,
                bottom_room,
            },
            Op::Magnify { value } => Self::Magnify { value },
            Op::ScrollTo { y, seq } => Self::ScrollTo { y, seq },
            Op::ScrollAcross { x } => Self::ScrollAcross { x },
            Op::ShowPage {
                page,
                x,
                y,
                width,
                height,
                fit,
                paper_rgb,
                turns,
            } => Self::ShowPage {
                page,
                x,
                y,
                width,
                height,
                fit,
                paper_rgb,
                turns,
            },
            Op::HidePage { page } => Self::HidePage { page },
            Op::Attach {
                id,
                page,
                x,
                y,
                width,
                height,
                surface,
                rgb,
                crop_width,
                crop_height,
                below,
            } => Self::Attach {
                id,
                page,
                x,
                y,
                width,
                height,
                surface,
                rgb,
                crop_width,
                crop_height,
                below,
            },
            Op::Detach { id } => Self::Detach { id },
            Op::Highlight { page, rects } => Self::Highlight {
                page,
                rects: rects
                    .into_iter()
                    .map(|rect| HighlightRect {
                        x: f64::from(rect.x),
                        y: f64::from(rect.y),
                        width: f64::from(rect.width),
                        height: f64::from(rect.height),
                    })
                    .collect(),
            },
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
    /// Who's watching over the socket, once it runs: the session's news goes on to them. See [`AppHost::listen`].
    watchers: Mutex<Option<Notifier>>,
}

impl AppHost {
    /// **The session's news reaches the app from the start, and watchers once the socket runs** (slow mode, 2026-11-11: with the socket's first answer late, Find came back with its hits and not the hit gone to). The app heard of Find's hits and of generated contents only once the socket's server ran and attached, so what finished before that, or all of it with the socket another session's, was heard by nobody. Given once, as the session takes it once.
    fn listen(self: &Arc<Self>) {
        let core = Arc::clone(&self.core);
        let host = Arc::downgrade(self);
        self.session.attach(Arc::new(move |events: Vec<Event>| {
            if events.iter().any(|event| event.name() == "contents_generated") {
                core.push(CoreCall::GeneratedFinished);
            }
            // The Finder's news, for the sidebar's list and a step waiting for hits.
            if events.iter().any(|event| matches!(event.name(), "find_changed" | "find_finished")) {
                core.push(CoreCall::Find(find::FindCall::News));
            }
            if let Some(notify) = host.upgrade().and_then(|host| lock(&host.watchers).clone()) {
                notify(events);
            }
        }));
    }

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
                let (warm, list_asleep) = self
                    .session
                    .with_session(|session| (session.warm_tabs().any(|other| other == id), session.list_is_asleep(id)));
                Ok(json!({ "tab": id.0, "dormant": !warm, "list_asleep": list_asleep }))
            }
            Command::SessionSave {} => {
                let report = self.saver.flush(false, Duration::from_secs(5)).map_err(store_error)?;
                Ok(
                    json!({ "saved": true, "tabs": report.tabs, "edited_tabs": report.edited_tabs, "record_bytes": report.record_bytes, "journal_bytes": report.journal_bytes, "wrote_bytes": report.wrote_bytes, "total_ms": report.total_us / 1000, "total_us": report.total_us, "write_us": report.write_us, "force_us": report.force_us, "verify_us": report.verify_us }),
                )
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
        let asks_only = matches!(
            command,
            Command::QaRead { .. }
                | Command::QaDrive { .. }
                | Command::SessionCheck {}
                | Command::SessionShow {}
                | Command::SessionSave {}
                | Command::UpdateStatus {}
                | Command::UpdateDevBuilds { set: None }
                | Command::Settings { auto_update: None }
        );
        // Counted first, and then looked: a hand-off that's set this flag and sees no count can't have this one slip past both looks. Past the last look, a change would be lost: what's been written is what comes back.
        if !asks_only {
            self.running.fetch_add(1, Ordering::SeqCst);
            if self.handing_off.load(Ordering::SeqCst) {
                self.running.fetch_sub(1, Ordering::SeqCst);
                return Err(
                    AgentError::new("busy", "The app is being updated, so this wasn't done.", recto_agent::exit::BUSY)
                        .then("Try again in a moment, when it's back."),
                );
            }
        }
        if matches!(
            command,
            Command::SessionSleep { .. } | Command::SessionShow {} | Command::SessionSave {} | Command::SessionRestore {}
        ) {
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
        if self.saver.store.holds_the_record() {
            Vec::new()
        } else {
            self.session.unsaved_tabs()
        }
    }

    fn acted_tab(&self, command: &Command) -> Option<TabId> {
        self.session.acted_tab(command)
    }

    fn attach(&self, notify: Notifier) {
        // The session's been told since the start, by `listen`: from now its news goes on to watchers too.
        *lock(&self.watchers) = Some(notify);
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
        if let Command::View {
            paper,
            sidebar,
            agent_icon,
            zoom: Some(word),
            layout,
        } = command
        {
            let zoom = menus::zoom_from_word(word).ok_or_else(|| {
                AgentError::new(
                    "usage",
                    format!(
                        "`{}` isn't a zoom. It's `in`, `out`, `fit`, `actual` or a number.",
                        recto_core::clean_line(word)
                    ),
                    recto_agent::exit::USAGE,
                )
            })?;
            let rest = Command::View {
                paper: paper.clone(),
                sidebar: sidebar.clone(),
                agent_icon: agent_icon.clone(),
                zoom: None,
                layout: layout.clone(),
            };
            let mut answer = self.session.run(level, &rest, events)?;
            let inner = self.saver.inner();
            let reading = inner
                .as_ref()
                .map_or_else(|| self.engine.clone(), |inner| inner.panes.reading_engine(&inner.engine));
            let value = inner
                .as_ref()
                .map_or_else(|| self.engine.zoom(zoom), |inner| inner.panes.zoom_all(&inner.engine, zoom))
                .ok_or_else(|| AgentError::new("no_active_tab", "There's no page to zoom.", recto_agent::exit::NOT_FOUND))?;
            answer["zoom"] = json!(value);
            if let Some(fit) = reading.fit() {
                answer["zoom_level"] = json!(menus::zoom_label(value, fit).text);
            }
            self.core.push(CoreCall::Reconcile);
            return Ok(answer);
        }
        if let Command::Palette { query, run, limit } = command {
            if !recto_agent::allowed(level, command.access()) {
                return Err(recto_agent::refusal(recto_agent::table::display_name(command.tag()), level, command.access()));
            }
            let inner = self
                .saver
                .inner()
                .ok_or_else(|| AgentError::new("busy", "The app is going away.", recto_agent::exit::BUSY))?;
            let core = AppCore {
                inner,
                qa_mode: false,
                start_with: Vec::new(),
            };
            // Nothing of a tab opened with a password, whose name may have been read with it, and files opened lately only at `recto recent`'s level.
            let whom = palette::ForWhom {
                hidden_tabs: self.session.with_session(|session| {
                    session
                        .tabs()
                        .iter()
                        .filter(|tab| session.withheld_from_agents(tab.id))
                        .map(|tab| tab.id.0)
                        .collect()
                }),
                recent: recto_agent::table::by_tag("recent").is_some_and(|spec| recto_agent::allowed(level, spec.access)),
            };
            let rows = palette::found_for(&core, query.as_deref().unwrap_or(""), &whom);
            // A tab's name is the file's, so it's marked as the document's words.
            let row_json = |row: &recto_app::palette::PaletteRow| {
                if row.from_file {
                    json!({ "id": row.id, "detail": row.detail, "shortcut": row.shortcut, "untrusted": { "title": row.title } })
                } else {
                    json!({ "id": row.id, "title": row.title, "detail": row.detail, "shortcut": row.shortcut })
                }
            };
            let mut answer = json!({ "query": query, "items": rows.iter().take(limit.unwrap_or(20).clamp(1, 50) as usize).map(row_json).collect::<Vec<_>>() });
            if *run {
                let first = rows.first().ok_or_else(|| {
                    AgentError::new("nothing_found", "Nothing matches that, so nothing was run.", recto_agent::exit::NOT_FOUND)
                        .then("`recto palette` lists what there is.")
                })?;
                let reader = lock(&core.inner.published).reader.clone();
                // What it comes to as an agent's own command runs as that, at the agent's level, with that command's checks, and as the agent's step. What has none, or writes the clipboard, needs what the direct way needs: Full, or `copy`'s level.
                let host = match palette::as_command(&first.id, &reader, &core.recent_files()) {
                    Some(direct) => {
                        core.inner.palette.close();
                        self.run_counted(level, &direct, events)?;
                        HostAction::None
                    }
                    None => {
                        let needed = if first.id.starts_with("edit.copy") {
                            recto_agent::table::by_tag("copy").map_or(recto_agent::table::Access::At(Level::Full), |spec| spec.access)
                        } else {
                            recto_agent::table::Access::At(Level::Full)
                        };
                        if !recto_agent::allowed(level, needed) {
                            return Err(recto_agent::refusal(&format!("palette --run for {}", first.id), level, needed)
                                .then("Run that command itself, if it has one, at the level it needs."));
                        }
                        palette::run(&core, &first.id)
                    }
                };
                let ran = if first.from_file {
                    json!({ "id": first.id, "untrusted": { "title": first.title }, "host": palette::host_word(host) })
                } else {
                    json!({ "id": first.id, "title": first.title, "host": palette::host_word(host) })
                };
                answer["ran"] = ran;
                self.core.push(CoreCall::Reconcile);
            }
            return Ok(answer);
        }
        if matches!(
            command,
            Command::UpdateStatus {} | Command::UpdateCheck {} | Command::UpdateInstall { .. } | Command::UpdateDevBuilds { .. } | Command::Settings { .. }
        ) {
            let inner = self.saver.inner().ok_or_else(recto_agent::update_needs_app)?;
            if !recto_agent::allowed(level, command.access()) {
                return Err(recto_agent::refusal(recto_agent::table::display_name(command.tag()), level, command.access()));
            }
            return inner.update_command(command);
        }
        if matches!(command, Command::SessionCheck {}) {
            let check = hand_off_check(self);
            let busy: Vec<Value> = check.busy.iter().map(|item| json!({ "code": item.code, "message": item.message })).collect();
            return Ok(json!({ "safe": check.safe, "busy": busy, "seconds_since_input": check.seconds_since_input }));
        }
        let result = self.session.run(level, command, events);
        // `recto find --next` went to a hit: the page view selects it, as ⌘G's does.
        if let (Command::Find { .. }, Ok(answer)) = (command, &result)
            && let Some(went) = answer.get("went_to").filter(|went| went.is_object())
            && let (Some(tab), Some(page), Some(start), Some(end)) = (went["tab"].as_u64(), went["page"].as_u64(), went["start"].as_u64(), went["end"].as_u64())
        {
            let hit = recto_agent::find::Hit {
                page: (page as u32).saturating_sub(1),
                start: start as u32,
                end: end as u32,
                before: "".into(),
                matched: "".into(),
                after: "".into(),
                hidden: false,
            };
            self.core.push(CoreCall::Find(find::FindCall::Reveal(tab as u32, hit)));
        }
        self.core.push(CoreCall::Reconcile);
        result
    }
}

/// Files being read at once off the session thread. Each is a thread that a dead volume can hold for as long as it likes, so there's a limit, and past it a tab or file doesn't open and says so.
const MAX_FILE_READS: usize = 4;

struct EngineHooks {
    core: Arc<CoreQueue>,
    listener: Arc<dyn recto_app::Listener>,
}

impl EngineEvents for EngineHooks {
    fn ops_ready(&self) {
        if recto_platform::slow::is_on() {
            let listener = Arc::clone(&self.listener);
            recto_platform::slow::later(recto_platform::slow::Seam::Draw, move || listener.ops_ready());
        } else {
            self.listener.ops_ready();
        }
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
    /// Each pane, left to right: one, or a comparison's.
    panes: Vec<PaneSynced>,
    paper: Option<PaperKind>,
    /// The tab being read, whose contents the sidebar has.
    active: Option<TabId>,
    /// How many page views there are.
    count: usize,
}

/// The state the UI thread pulls, swapped whole. A short lock, never held across anything slow.
struct Published {
    /// Files opened lately, newest first, for Open Recent and ⌘K, so neither asks the session.
    recent: Arc<Vec<RecentFile>>,
    /// What `recent` was built from: the session's list's version and the tabs' version.
    recent_key: Option<(u64, u64)>,
    /// Each pane's contents and page labels, for a link's hint: the UI thread reads them here and never asks the session.
    pane_tocs: Vec<Option<Arc<recto_core::Toc>>>,
    sidebar: Arc<recto_app::SidebarView>,
    tabs: Arc<Vec<recto_app::SidebarRow>>,
    contents: Arc<Vec<ContentsEntry>>,
    rail_filled: u32,
    reader: ReaderView,
}

impl Published {
    /// What's published before the session thread's first pass: no tabs, the home screen, white paper.
    fn empty() -> Self {
        Self {
            recent: Arc::new(Vec::new()),
            recent_key: None,
            pane_tocs: Vec::new(),
            contents: Arc::new(Vec::new()),
            rail_filled: 0,
            tabs: Arc::new(Vec::new()),
            sidebar: Arc::new(recto_app::empty_sidebar()),
            reader: ReaderView::home(),
        }
    }
}

struct Inner {
    /// Small state, and no thread or timer of its own at rest.
    updates: Arc<recto_update::updater::Updater>,
    /// What the host knows of this copy, for the thread that reads the state.
    update_facts: Mutex<Option<recto_update::updater::Facts>>,
    host: Arc<AppHost>,
    engine: Engine,
    /// A comparison's panes past the first, and what the UI thread needs to keep them in step.
    panes: panes::Panes,
    core: Arc<CoreQueue>,
    listener: Arc<dyn recto_app::Listener>,
    published: Mutex<Published>,
    contents_follow: Mutex<ContentsFollow>,
    started: Instant,
    saver: Arc<Saver>,
    app: Mutex<AppState>,
    /// The record's been read and applied, whatever it said.
    restored: AtomicBool,
    /// The host has made what it shows from the state that was read: at once, or by making the window again when it came in late. What `qa wait-settled` waits for first.
    shown_with_state: AtomicBool,
    /// The window went up without the state, past its patience.
    shown_without_state: AtomicBool,
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
    /// Why something the reader chose couldn't be done, or what a save has to say, as a notice that goes by itself.
    action_notice: Mutex<Option<String>>,
    /// Passwords typed for protected files that are open, shared with every page view's workers. Memory only.
    keys: Arc<recto_reader::engine::Keyring>,
    /// The tab showing when the window was last brought up to date: a notice is about what was on screen, so it goes with a change of tab.
    notice_tab: AtomicU32,
    /// The sheets that ask: a password, or whether to open a link.
    prompts: recto_app::prompts::Prompts,
    /// Saves from the window: how many run, and what one that failed says.
    save_ui: saving::SaveUi,
    /// The snapshot tool: on or off, the box being drawn, and what's on its way to the clipboard.
    tool: snapshot_tool::Tool,
    folds: recto_app::sidebar_access::Folds,
    chrome: recto_app::chrome::Chrome,
    /// A print being prepared, or waiting for the host to hand to the system.
    printing: printing::Printing,
    /// ⌘K's palette, on the UI thread.
    palette: recto_app::palette::Palette,
    /// The Find field as the session thread last set it, for the UI thread.
    find_field: Mutex<recto_app::find::Field>,
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
    /// Hidden with ⌘H, as the record has it.
    app_hidden: bool,
    /// The host has asked whether the app comes back hidden, and done it: a restore after that keeps what it did.
    hidden_decided: bool,
    /// The same for the window in the Dock.
    minimised_decided: bool,
    /// And for whether there's a window at all.
    window_decided: bool,
    /// What the state said, as it was taken in after a window went up without it. That window can report its own size, focus and the rest before it's made again, which a full check on a slow machine saw, so the one made again goes by this: see [`AppCore::window_replaced`].
    from_state: Option<Box<AppState>>,
    /// What each pane had selected as its window went for another made from the state, to be selected again on the new one.
    selections_held: Vec<(usize, recto_index::select::Range)>,
}

/// What's in flight, for an update to wait for. A save from the window counts here, and an agent's save or an export is a command.
fn hand_off_check_parts(inner_reads: u32, sheets: u32, commands: u32, saves: u32, printing: bool) -> Vec<BusyItem> {
    let item = |code: &str, message: &str| BusyItem {
        code: code.to_string(),
        message: message.to_string(),
    };
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
    // An update waits for a save to finish, then goes on with what it wrote (SPEC decision 24).
    if saves > 0 {
        busy.push(item("saving", "A document is being saved."));
    }
    // A print's pages are drawn by a command-like thread, but once they're ready the system's printing has them, which no thread here counts.
    if printing {
        busy.push(item("printing", "A print is being prepared or printed."));
    }
    busy
}

fn hand_off_check(host: &AppHost) -> HandOffCheck {
    let Some(inner) = host.saver.inner() else {
        return HandOffCheck {
            safe: false,
            busy: Vec::new(),
            seconds_since_input: 0.0,
        };
    };
    inner.hand_off_check()
}

impl Inner {
    fn hand_off_check(&self) -> HandOffCheck {
        let busy = hand_off_check_parts(
            self.reads_running.load(Ordering::SeqCst),
            self.sheets.load(Ordering::SeqCst),
            self.host.running.load(Ordering::SeqCst),
            self.save_ui.running.load(Ordering::SeqCst),
            self.printing.busy(),
        );
        let last = self.last_input_ms.load(Ordering::Relaxed);
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        HandOffCheck {
            safe: busy.is_empty(),
            busy,
            seconds_since_input: now.saturating_sub(last) as f64 / 1000.0,
        }
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
                Err(HandOffError::NotSaved {
                    code: error.code.to_string(),
                    message: error.message,
                })
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
        AppRecord {
            window: app.window,
            appearance: app.appearance,
            sidebar_width: Some(app.sidebar_width),
            panel_open: self.panel_open.load(Ordering::SeqCst),
            contents_scroll: Some(ContentsScroll {
                y: app.contents_y.unwrap_or(0.0),
                following,
            }),
            focus: app.focus,
            notice,
            settings_window: app.settings_window,
            window_closed: app.window_closed,
            app_hidden: app.app_hidden,
        }
    }

    /// Everything the record holds and QA compares: the record as it would be written, and each tab's edits in full.
    pub(crate) fn state_json(&self) -> Value {
        let (record, _) = self.gather_record();
        let mut value = serde_json::to_value(&record).unwrap_or(Value::Null);
        let lists = self.host.session.with_session(|session| {
            session
                .tabs()
                .iter()
                .filter_map(|tab| Some((tab.id.0.to_string(), serde_json::to_value(session.list_record_anywhere(tab.id)?).ok()?)))
                .collect::<serde_json::Map<_, _>>()
        });
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
            // What ⌘F and Escape do.
            "find" => {
                self.core
                    .push(CoreCall::Find(if value == "open" { find::FindCall::Open } else { find::FindCall::Close }));
                self.core.flush(Duration::from_secs(5));
                Ok(())
            }
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
            // What going in does, and what Done does.
            "edit_mode" => {
                self.core.push(CoreCall::SetEditMode(value == "on"));
                self.core.flush(Duration::from_secs(5));
                Ok(())
            }
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
            // What Save As's sheet does when it's answered with this path: the same call.
            "save_as" => match self.saver.inner() {
                Some(inner) if value.starts_with('/') => match lock(&inner.published).reader.shown_tab {
                    0 => Err(AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND)),
                    tab => {
                        saving::start_save(&inner, TabId(tab), Some(PathBuf::from(value)), false);
                        Ok(())
                    }
                },
                _ => Err(usage("It's the whole path of a new file.".to_string())),
            },
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
                for engine in self.panes.engines(&self.engine) {
                    engine.hold_drawing(value == "on");
                }
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
                // What the host's dispatch source calls, through the same method, which in a quiet run only counts it.
                "system-warning" | "system-critical" => {
                    self.system_pressure(value == "system-critical");
                    Ok(())
                }
                // The system taking back what's marked discardable, which it does without a word whenever it needs the pages.
                "discard" => {
                    for engine in self.panes.engines(&self.engine) {
                        engine.empty_discardable();
                    }
                    Ok(())
                }
                _ => Err(usage("It's `warning`, `critical` or `discard`.".to_string())),
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

    /// **What a Save does around its writing, with the writing done by the test,** for crash and restore tests: keeps what's needed, notes what's coming and flushes that, puts the file in place, and confirms. Every step but the writing is the real one, and it's refused where Save would be.
    fn qa_save_from(&self, written: &str) -> Result<(), AgentError> {
        let failed = |what: &str| AgentError::new("save_failed", format!("{what} failed."), recto_agent::exit::FAILED);
        let written = PathBuf::from(written);
        let target = recto_platform::file_stamp(&written).map_err(|_| failed("Looking at the written file"))?;
        // The checks Save makes before it writes: never over a signed file, never over one changed on disk, never while it's being saved.
        let (path, known, saving) = self
            .host
            .session
            .with_session(|session| {
                session
                    .active()
                    .and_then(|id| Some((session.path_of(id)?, session.own_stamp(id), session.is_saving(id))))
            })
            .ok_or_else(|| AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND))?;
        if saving {
            return Err(AgentError::new("saving", "That document is being saved.", recto_agent::exit::FAILED));
        }
        if known.is_none_or(|known| recto_platform::file_stamp(&path).ok() != Some(known)) {
            return Err(AgentError::new(
                "file_changed",
                recto_core::save::SaveError::FileChanged.message(),
                recto_agent::exit::FAILED,
            ));
        }
        save_from_signature_check(recto_core::save::signature_facts_of(&path))?;
        saving::save_from_gate(&path, known)?;
        let (id, path, pages, needed) = self
            .host
            .session
            .with_session(|session| {
                let id = session.active()?;
                let needed = session.page_edits(id).map(recto_session::edit::PageEdits::own_pages_needed).unwrap_or_default();
                Some((id, session.path_of(id)?, session.pages_in(id), needed))
            })
            .ok_or_else(|| AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND))?;
        let kept = self.saver.store.keep_pages_of(&path, target, &needed);
        self.host.session.with_session(|session| session.save_prepared(id, target, kept.as_ref()));
        self.saver
            .flush(false, Duration::from_secs(5))
            .map_err(|_| failed("Writing what's about to happen"))?;
        std::fs::rename(&written, &path).map_err(|_| failed("Putting the file in place"))?;
        let now = recto_platform::file_stamp(&path).map_err(|_| failed("Looking at the saved file"))?;
        self.host.session.with_session(|session| session.saved(id, now, pages, None, kept));
        Ok(())
    }

    /// An edit as the owner's, as the app's own tools will make it: they aren't built yet.
    fn owner_edit(&self, value: &str) -> Result<(), AgentError> {
        let usage = || {
            AgentError::new(
                "usage",
                "It's `rotate:PAGE`, `delete:PAGE`, `undo` or `redo`.".to_string(),
                recto_agent::exit::USAGE,
            )
        };
        self.host.session.with_session(|session| {
            let id = session
                .active()
                .ok_or_else(|| AgentError::new("no_active_tab", "No tab is open.", recto_agent::exit::NOT_FOUND))?;
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
        self.last_input_ms
            .store(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    fn note_input(&self) {
        self.last_input_ms
            .store(u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Relaxed);
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

/// Whether this launch is a quiet run, which is decided before AppKit does anything. The same parser as the rest of the launch, so the two can't disagree. A launch that won't parse ends straight away, and shows nothing on the way.
#[uniffi::export]
pub fn launch_is_quiet(args: Vec<String>) -> bool {
    recto_app::launch::parse(&args).map_or(true, |parsed| parsed.qa)
}

/// What a launch answers and ends with, before AppKit does anything: `--build-kind`, which says whether this was built as a release and takes a test's hooks, for `app/release-zip.sh` to check the app's own library and not only the installer's. Nothing for any other launch.
#[uniffi::export]
pub fn launch_answers_at_once(args: Vec<String>) -> Option<String> {
    (args == ["--build-kind"]).then(|| json!({ "release_build": recto_update::RELEASE_BUILD, "takes_test_hooks": !recto_update::RELEASE_BUILD }).to_string())
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
        let listener: Arc<dyn recto_app::Listener> = Arc::new(records::HostListener(listener));
        let args = recto_app::launch::parse(&args).map_err(refused)?;
        recto_platform::slow::arm(args.qa).map_err(refused)?;
        let paths = state_for(args.qa).map_err(refused)?;
        let server = match Server::bind(&paths) {
            Ok(server) => Some(server),
            Err(error) if args.qa => return Err(refused(AgentError::from(error).message)),
            Err(_) => None,
        };
        let core = Arc::new(CoreQueue::default());
        let hooks: Arc<dyn EngineEvents> = Arc::new(EngineHooks {
            core: Arc::clone(&core),
            listener: Arc::clone(&listener),
        });
        let mut policy = Policy::default();
        if let Some(limit) = args.memory_limit_mib {
            policy.memory_limit_bytes = limit * 1024 * 1024;
            // At or below the normal limit, this step is off, so a page reaches its lower-resolution version straight away.
            policy.heavy_limit_bytes = policy.memory_limit_bytes;
        }
        let keys = Arc::new(recto_reader::engine::Keyring::default());
        let config = EngineConfig {
            service: None,
            policy: Some(policy),
            keys: Some(Arc::clone(&keys)),
        };
        let engine = Engine::new(config.clone(), Arc::clone(&hooks));
        let watchdog = Watchdog::new();
        let qa = args
            .qa
            .then(|| Arc::new(QaBridge::new(Arc::clone(&listener), engine.clone(), watchdog.clone(), Arc::clone(&core))));
        // Takes the record's lock, and reads nothing: that's `begin_restore`'s, on a thread that can wait for it.
        let saver = Arc::new(Saver::new(RecordStore::open(&paths, Flavor::App), Arc::clone(&core)));
        let host = Arc::new(AppHost {
            engine: engine.clone(),
            session: SessionHost::new(DrawableLoader).with_indexing(&paths).with_generating(&paths),
            core: Arc::clone(&core),
            qa: qa.clone(),
            saver: Arc::clone(&saver),
            running: AtomicU32::new(0),
            handing_off: AtomicBool::new(false),
            notice: Mutex::new(None),
            watchers: Mutex::new(None),
        });
        host.listen();
        // The person's own Find reads a document they opened with a password through its key, as the page views draw it.
        let find_keys = Arc::clone(&keys);
        host.session.set_find_keys(Arc::new(move |path: &std::path::Path| find_keys.for_path(path)));
        let backups = recto_control::host::backups_folder(&paths);
        recto_core::save::prune_backups(&backups, std::time::SystemTime::now());
        let save_step = Arc::new(AtomicU8::new(0));
        host.session.set_saving(Arc::new(saving::AppSaving {
            saver: Arc::clone(&saver),
            backups,
            step: Arc::clone(&save_step),
            core: Arc::clone(&core),
        }));
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
            panes: panes::Panes::new(hooks, config),
            core: Arc::clone(&core),
            listener: Arc::clone(&listener),
            contents_follow: Mutex::new(ContentsFollow::default()),
            started: Instant::now(),
            saver: Arc::clone(&saver),
            sleeper: sleeper::Sleeper::new(),
            app: Mutex::new(AppState {
                window: None,
                appearance: Appearance::System,
                sidebar_width: record::SIDEBAR_WIDTH,
                contents_y: None,
                contents_resume: None,
                selection_resume: None,
                focus: FocusRecord::default(),
                settings_window: None,
                window_closed: false,
                app_hidden: false,
                hidden_decided: false,
                minimised_decided: false,
                window_decided: false,
                from_state: None,
                selections_held: Vec::new(),
            }),
            restored: AtomicBool::new(false),
            shown_with_state: AtomicBool::new(false),
            shown_without_state: AtomicBool::new(false),
            waiting_server: Mutex::new(server),
            holding_tab: Mutex::new(None),
            hand_off_state: AtomicU8::new(HandOffState::Idle as u8),
            qa_hold_reads: AtomicBool::new(false),
            reads_running: AtomicU32::new(0),
            sheets: AtomicU32::new(0),
            last_input_ms: AtomicU64::new(0),
            first_shown: Mutex::new(None),
            published: Mutex::new(Published::empty()),
            watchdog,
            stopper: Mutex::new(stopper),
            announcer: Mutex::new(announcer),
            dark: AtomicBool::new(false),
            panel_open: AtomicBool::new(false),
            qa,
            paths: Some(paths),
            open_failure: Mutex::new(None),
            action_notice: Mutex::new(None),
            tool: snapshot_tool::Tool::default(),
            folds: recto_app::sidebar_access::Folds::default(),
            chrome: recto_app::chrome::Chrome::default(),
            printing: printing::Printing::default(),
            keys,
            notice_tab: AtomicU32::new(0),
            prompts: recto_app::prompts::Prompts::default(),
            save_ui: saving::SaveUi::new(Arc::clone(&save_step)),
            palette: recto_app::palette::Palette::default(),
            find_field: Mutex::new(recto_app::find::Field::default()),
        });
        saver.attach(Arc::downgrade(&inner));
        update_hooks.attach(Arc::downgrade(&inner));
        if let Some(qa) = &inner.qa {
            qa.attach(Arc::downgrade(&inner));
        }
        let session_side = Arc::clone(&inner);
        std::thread::Builder::new()
            .name("recto-session".into())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || session_thread(&session_side))
            .map_err(|_| refused("The app couldn't start its session thread.".into()))?;
        if args.qa {
            let state = inner.paths.as_ref().map(|paths| paths.root.clone());
            qa::end_with_starter(args.time_limit.unwrap_or(qa::QUIET_RUN_LIMIT), args.owner_pid, state, Arc::downgrade(&inner));
        }
        Ok(Arc::new(Self {
            inner,
            qa_mode: args.qa,
            start_with: {
                updates::note_asked_to_open(&args.open);
                args.open
            },
        }))
    }

    /// Whether this is a quiet run: off every screen, never active, with no Dock icon.
    pub fn is_qa(&self) -> bool {
        self.qa_mode
    }

    /// The window is up. In a QA run this tells whoever's waiting, on one line, and any files named at launch start opening.
    pub fn ready(&self) {
        printing::sweep_at_launch();
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
            home_note: privacy::home_note(self.inner.updates.source().checks_by_itself()).unwrap_or_default().into(),
        }
    }

    pub fn limits(&self) -> ReaderLimits {
        ReaderLimits {
            min_zoom: MIN_ZOOM,
            max_zoom: MAX_ZOOM,
            window_width: 1440.0,
            window_height: 900.0,
            sidebar_width: record::SIDEBAR_WIDTH,
            reveal_delay_seconds: 0.0,
            restore_patience_seconds: RESTORE_PATIENCE.as_secs_f64(),
            show_patience_seconds: SHOW_PATIENCE.as_secs_f64(),
            autoscroll_every_seconds: recto_reader::selection::AUTOSCROLL_EVERY,
            notice_seconds: 6.0,
        }
    }

    /// The sidebar as it stands, but for its tabs. A short lock and a small copy.
    pub fn sidebar(&self) -> SidebarView {
        SidebarView::from((*lock(&self.inner.published).sidebar).clone())
    }

    /// The tabs, one row each. Worth pulling only when `tabs_version` has moved: this copies all of them.
    pub fn sidebar_tabs(&self) -> Vec<SidebarRow> {
        let tabs = Arc::clone(&lock(&self.inner.published).tabs);
        tabs.iter().cloned().map(SidebarRow::from).collect()
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
                Some(SidebarRow {
                    kind: RowKind::Contents,
                    tab: entry.tab,
                    text: entry.text.to_string(),
                    page: entry.page,
                    depth: entry.depth,
                    current: current == Some(index),
                    rail_filled: index < rail_filled,
                    rail_first: index == 0,
                    rail_last: index + 1 == total,
                    notice: None,
                    action: None,
                    expandable: entry.expandable,
                    expanded: entry.expanded,
                    in_comparison: false,
                })
            })
            .collect()
    }

    /// The scroll thumb's look, the same for the sidebar's lists and the page area.
    pub fn scroll_thumb(&self) -> ScrollThumbLook {
        let thumb = recto_session::SCROLL_THUMB;
        ScrollThumbLook {
            width: thumb.width,
            hover_width: thumb.hover_width,
            hit_width: thumb.hit_width,
            inset: thumb.inset,
            min_length: thumb.min_length,
            light_rgb: thumb.light.0,
            light_alpha: thumb.light.1,
            light_hover_alpha: thumb.light_hover.1,
            dark_rgb: thumb.dark.0,
            dark_alpha: thumb.dark.1,
            dark_hover_alpha: thumb.dark_hover.1,
        }
    }

    /// The sidebar's height rule, for this much room between its header and footer.
    pub fn sidebar_layout(&self, room: f64) -> SidebarLayout {
        let published = lock(&self.inner.published);
        let tabs = published.sidebar.tab_count;
        let rows = if published.sidebar.toc_open { published.sidebar.contents_count } else { 0 };
        let note = published.sidebar.toc_open && published.sidebar.contents_note.is_some();
        drop(published);
        // Find's block comes off the top first, and the rest divides as it always has.
        let finding = find::view(&self.inner);
        let found = find::heights(room, finding.open, finding.row_count);
        let heights = sidebar_heights_with_note((room - found.block).max(0.0), tabs, rows, note);
        SidebarLayout {
            tab_row: TAB_ROW_HEIGHT,
            contents_row: CONTENTS_ROW_HEIGHT,
            contents_height: heights.contents,
            tabs_scroll: heights.tabs_scroll,
            note_height: heights.note,
            find_block: found.block,
            find_list: found.list,
            find_row: find::ROW_HEIGHT,
            find_field: find::FIELD_HEIGHT,
            find_status: find::STATUS_HEIGHT,
        }
    }

    /// Everything Find shows but its rows. Short locks, never the session's.
    pub fn find_view(&self) -> find::FindView {
        find::view(&self.inner)
    }

    /// Some of Find's rows, for what's on screen.
    pub fn find_rows(&self, first: u32, count: u32) -> Vec<find::FindRow> {
        find::rows(&self.inner, first, count)
    }

    /// What's in the Find field now, as it's typed: found at once.
    pub fn find_typed(&self, text: String) {
        self.inner.core.push(CoreCall::Find(find::FindCall::Typed(text)));
    }

    /// ⌘F: Find opens, with what was last typed.
    pub fn find_open(&self) {
        self.inner.core.push(CoreCall::Find(find::FindCall::Open));
    }

    /// Escape in the field, or its close button.
    pub fn find_close(&self) {
        self.inner.core.push(CoreCall::Find(find::FindCall::Close));
    }

    /// Return (forward) or ⇧Return (back) in the field: the person's jump to the hit, so Back comes here again. ⌘G and ⇧⌘G are the menu's, and only step.
    pub fn find_return(&self, forward: bool) {
        self.inner
            .core
            .push(CoreCall::Find(find::FindCall::Step(forward, recto_agent::find::Going::Jump)));
    }

    /// A click on a row of Find's list.
    pub fn find_choose(&self, row: u32) {
        self.inner.core.push(CoreCall::Find(find::FindCall::Choose(row)));
    }

    /// What QA reads of Find, as JSON.
    pub fn find_report(&self) -> String {
        find::report(&self.inner).to_string()
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

    /// Rows closed or opened. Says how many points to move the scroll by with no animation, or nothing if the rows are to change in view, where the host fades them over.
    pub fn contents_row_change(&self, scroll_y: f64, from: u32, removed: u32, inserted: u32, place_shift: i32) -> Option<f64> {
        lock(&self.inner.contents_follow).row_change(
            ContentsRowChange {
                from,
                removed,
                inserted,
                place_shift,
            },
            scroll_y,
        )
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

    /// A contents row's chevron was tapped: its sections fold or open. On the session thread, as every change to what the sidebar shows is.
    pub fn toggle_contents_section(&self, first: u32) {
        let entry = lock(&self.inner.published)
            .contents
            .get(first as usize)
            .map(|entry| (entry.tab, entry.entry, entry.expandable));
        if let Some((tab, entry, true)) = entry {
            self.inner.core.push(CoreCall::ToggleSection(TabId(tab), entry));
        }
    }

    /// A row was tapped. Goes on the session thread, as waking a cold tab reads its file.
    pub fn tap_row(&self, row: SidebarRow) {
        let call = match (row.kind, row.page) {
            (RowKind::Tab, _) => CoreCall::Tap(TabId(row.tab)),
            (RowKind::Contents, Some(page)) => CoreCall::Jump(TabId(row.tab), page.saturating_sub(1), 0.0),
            (RowKind::Contents, None) => return,
            (RowKind::Note, _) => match row.action.as_deref().and_then(recto_session::ContentsAction::from_id) {
                Some(action) => CoreCall::ContentsAction(TabId(row.tab), action),
                None => return,
            },
        };
        self.inner.core.push(call);
    }

    /// ⌘-click or ⌃-click on a row: a tab goes into a comparison, or out of it (owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view"). A contents row does what a click does.
    pub fn tap_row_adding(&self, row: SidebarRow) {
        match row.kind {
            RowKind::Tab => self.inner.core.push(CoreCall::CompareClick(TabId(row.tab))),
            RowKind::Contents | RowKind::Note => self.tap_row(row),
        }
    }

    /// The pointer went down in a pane: it's the one being read. Cheap, and the session hears on its own thread.
    pub fn pane_focused(&self, pane: u32) {
        if let Some(tab) = self.inner.panes.focus(pane as usize) {
            self.focus_moved(tab);
        }
    }

    pub fn open_file(&self, path: String) {
        let path = PathBuf::from(path);
        // Asked for before the state's read: what a launch that goes back passes on.
        if !self.inner.restored.load(Ordering::SeqCst) {
            updates::note_asked_to_open(std::slice::from_ref(&path));
        }
        self.inner.core.push(CoreCall::Open(path));
    }

    /// A pane's view moved. On every scroll: a short lock and a few map lookups, and in a comparison with synced scroll, a short lock on each other pane to send it along, whose ops come by `ops_ready`. Apply what comes back in the same transaction.
    pub fn set_viewport(&self, pane: u32, viewport: ViewportInput) -> Vec<ReaderOp> {
        self.inner.saver.view_changed();
        let (ops, focused) = self.inner.panes.viewport(
            &self.inner.engine,
            pane as usize,
            viewport.by_reader,
            Viewport {
                x: viewport.x,
                y: viewport.y,
                width: viewport.width,
                height: viewport.height,
                frame_width: viewport.frame_width,
                magnification: viewport.magnification,
                backing: viewport.backing,
                live: viewport.live,
                scrolled_to: viewport.scrolled_to,
            },
        );
        if let Some(tab) = focused {
            self.focus_moved(tab);
        }
        ops.into_iter().map(ReaderOp::from).collect()
    }

    /// What's waiting for a pane's layers, at most a few tiles at a time. If there's more, `ops_ready` comes again.
    pub fn drain_ops(&self, pane: u32) -> Vec<ReaderOp> {
        self.inner
            .panes
            .engine(&self.inner.engine, pane as usize)
            .map(|engine| engine.drain().into_iter().map(ReaderOp::from).collect())
            .unwrap_or_default()
    }

    /// A pane's layers have what they were given, so what's off them can go.
    pub fn ops_applied(&self, pane: u32) {
        if let Some(engine) = self.inner.panes.engine(&self.inner.engine, pane as usize) {
            engine.applied();
        }
    }

    /// How many panes there are now, for the host to have a page area each. One with no comparison.
    pub fn pane_count(&self) -> u32 {
        u32::try_from(self.inner.panes.count()).unwrap_or(1)
    }

    pub fn set_dark_appearance(&self, dark: bool) {
        self.inner.dark.store(dark, Ordering::SeqCst);
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// What hovering over a row reveals: its full text, clean and bounded, and what it is.
    pub fn row_reveal(&self, row: SidebarRow) -> RowReveal {
        let full = row_full_text(&row.text, row.kind == RowKind::Tab);
        RowReveal {
            text: full.text,
            is_file_name: full.is_file_name,
        }
    }

    /// The menu bar as it stands. The host rebuilds it only when this differs.
    pub fn menu_bar(&self) -> Vec<MenuModel> {
        menus::bar_now(self).into_iter().map(Into::into).collect()
    }

    /// **The password sheet was answered:** for the prompt with this id, with what was typed, or nothing for Cancel. The answer goes to that prompt's file, whatever's waiting by now, and none if it's gone. It's tried on a thread of its own, as reading a file can wait on its disk, and kept nowhere but the open tab's keyring. The `String` UniFFI made is overwritten here, so Rust holds one copy.
    pub fn password_answered(&self, id: u64, password: Option<String>) {
        let mut asks = lock(&self.inner.prompts.passwords);
        let Some(typed) = password else {
            asks.cancel(id);
            drop(asks);
            self.inner.core.push(CoreCall::Reconcile);
            return;
        };
        let password = recto_core::Password::take(typed);
        let Some(password) = password else {
            // Not something a password could be, which counts as a wrong one.
            if asks.wrong(id) == passwords::Wrong::Stopped {
                *lock(&self.inner.action_notice) = Some(passwords::stopped_asking());
            }
            drop(asks);
            self.inner.core.push(CoreCall::Reconcile);
            return;
        };
        let Some(asked) = asks.start_trying(id) else { return };
        drop(asks);
        let core = Arc::clone(&self.inner.core);
        let password = Arc::new(password);
        let spawned = std::thread::Builder::new().name("recto-unlock".into()).stack_size(512 * 1024).spawn(move || {
            let stamp = recto_platform::file_stamp(&asked.path).ok();
            let loaded = recto_core::inspect_with_password(&asked.path, &password);
            core.push(CoreCall::Unlocked(asked, loaded, stamp, password));
        });
        if spawned.is_err() {
            lock(&self.inner.prompts.passwords).failed(id);
        }
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// **Files opened lately, newest first,** for ⌘K and anything else that offers them. From what the session thread last published: it never waits.
    pub fn recent_files(&self) -> Vec<RecentFile> {
        lock(&self.inner.published).recent.as_ref().clone()
    }

    /// Opens a file from that list by its key, as File > Open Recent does: the very file that was offered, however the list has moved since, and nothing if it's left the list. Its tab if it's open, and otherwise it's read as any open is, and says so if it's gone.
    pub fn open_recent(&self, key: String) {
        let path = lock(&self.inner.published)
            .recent
            .iter()
            .find(|file| file.key == key)
            .map(|file| PathBuf::from(&file.path));
        if let Some(path) = path {
            self.inner.core.push(CoreCall::Open(path));
        }
    }

    /// The sidebar's panel as it stands.
    pub fn panel(&self) -> PanelModel {
        let reader = lock(&self.inner.published).reader.clone();
        let mut panel = menus::panel(self.inner.panel_open.load(Ordering::SeqCst), reader.edit_mode, reader.can_rotate);
        recto_app::snapshot_menu::add_to_panel(&mut panel, self.inner.tool.is_on(), reader.edit_mode);
        panel.into()
    }

    /// Does what a menu item, a panel button or a key does, and says what's left for the host. The same methods as `recto view`.
    pub fn perform(&self, id: String) -> HostAction {
        // Past a hand-off's last look, what's done here wouldn't come back.
        if self.inner.core.frozen.load(Ordering::SeqCst) {
            return HostAction::None;
        }
        // A window up without the state is made again once it's in, and what was done to it would go with it.
        if !updates::takes_input_now(self.window_interactive(), &id) {
            return HostAction::None;
        }
        // Features that answer ids of their own: the snapshot tool, and printing.
        if snapshot_tool::perform(&self.inner, &id) || printing::perform(&self.inner, &id) {
            return HostAction::None;
        }
        let Some(action) = menus::action(&id) else { return HostAction::None };
        match action {
            menus::Action::Host(host) => return host.into(),
            menus::Action::Copy { anyway } => self.inner.panes.reading_engine(&self.inner.engine).copy_selection(anyway),
            menus::Action::SelectAll => self.inner.panes.reading_engine(&self.inner.engine).select_all(),
            menus::Action::SetSynced(on) => self.inner.core.push(CoreCall::SetSynced(on)),
            menus::Action::PaletteOpen => {
                // The palette is in the sidebar, so ⌘K shows it.
                if lock(&self.inner.published).reader.sidebar_hidden {
                    self.inner.core.push(CoreCall::SetSidebarHidden(false));
                }
                self.inner.palette.open();
                self.inner.listener.model_changed();
                return HostAction::FocusPalette;
            }
            menus::Action::OpenRecent(key) => self.open_recent(key),
            menus::Action::ClearRecent => self.inner.core.push(CoreCall::ClearRecent),
            menus::Action::FindOpen => {
                self.inner.core.push(CoreCall::Find(find::FindCall::Open));
                return HostAction::FocusFind;
            }
            menus::Action::FindStep(forward) => self
                .inner
                .core
                .push(CoreCall::Find(find::FindCall::Step(forward, recto_agent::find::Going::Step))),
            menus::Action::LeaveEditMode => self.inner.core.push(CoreCall::SetEditMode(false)),
            menus::Action::Step(back) => self.inner.core.push(CoreCall::Step(back)),
            menus::Action::ClearEditHistory => self.inner.core.push(CoreCall::ClearEditHistory),
            menus::Action::Zoom(zoom) => {
                self.inner.panes.zoom_all(&self.inner.engine, zoom);
            }
            menus::Action::ToggleSidebar => self.toggle_sidebar(),
            // The pane being read as it's chosen, which a right-click on a pane has just made it, whatever the session's still to hear.
            menus::Action::SetSpread(spread) => self.inner.core.push(CoreCall::SetSpread(
                (self.inner.panes.count() > 1)
                    .then(|| self.inner.panes.tab_of(self.inner.panes.reading()))
                    .flatten(),
                spread,
            )),
            menus::Action::Rotate(turns) => self.inner.core.push(CoreCall::Rotate(turns)),
            menus::Action::Undo => self.inner.core.push(CoreCall::UndoOrRedo(true)),
            menus::Action::Redo => self.inner.core.push(CoreCall::UndoOrRedo(false)),
            menus::Action::ToggleAgentIcon => self.inner.core.push(CoreCall::ToggleAgentIcon),
            menus::Action::SetPaper(paper) => self.inner.core.push(CoreCall::SetPaper(paper)),
            menus::Action::TogglePanel => {
                self.inner.panel_open.fetch_xor(true, Ordering::SeqCst);
                self.inner.listener.model_changed();
            }
            // Off this thread by itself, and it says how it went.
            menus::Action::Update => self.inner.updates.menu_chosen(),
            menus::Action::Save => {
                let reader = lock(&self.inner.published).reader.clone();
                if self.can_save() && reader.shown_tab != 0 {
                    saving::start_save(&self.inner, TabId(reader.shown_tab), None, false);
                }
            }
            menus::Action::CloseDocument => {
                let reader = lock(&self.inner.published).reader.clone();
                if reader.home || reader.saving {
                    return HostAction::None;
                }
                // Unsaved edits, or the owner's undone ones, which closing would lose: it asks, as any Mac app does.
                if reader.edited || lock(&self.inner.published).sidebar.undone_owner_steps {
                    return HostAction::ConfirmClose;
                }
                if reader.shown_tab != 0 {
                    self.inner.core.push(CoreCall::CloseTab(TabId(reader.shown_tab), false));
                }
            }
        }
        HostAction::None
    }

    /// Everything ⌘K's palette draws: its rows, ranked, worded and with their keys, all Rust's. Short locks, never the session's.
    pub fn palette_view(&self) -> palette::PaletteView {
        palette::view(self)
    }

    /// What's in the palette's field now, as it's typed.
    pub fn palette_typed(&self, text: String) {
        self.inner.palette.typed(&text);
    }

    /// ↑ (-1) and ↓ (1) in the palette.
    pub fn palette_step(&self, by: i32) {
        palette::step(self, by);
    }

    /// A click on a row of the palette.
    pub fn palette_run(&self, id: String) -> HostAction {
        palette::run(self, &id)
    }

    /// Escape, or the keyboard going elsewhere.
    pub fn palette_close(&self) {
        self.inner.palette.close();
        self.inner.listener.model_changed();
    }

    /// Whether a menu item can be chosen now. Asked when a menu opens and on its key. By the item's rule in `menus`: an id without one can't be chosen.
    pub fn menu_item_enabled(&self, id: String) -> bool {
        use menus::Enable;
        let Some(rule) = menus::enable_rule(&id) else { return false };
        if !updates::takes_input_now(self.window_interactive(), &id) {
            return false;
        }
        let reader = |pick: fn(&ReaderView) -> bool| pick(&lock(&self.inner.published).reader);
        match rule {
            Enable::Always => true,
            Enable::Document | Enable::Selection => {
                let reading = self.inner.panes.reading_engine(&self.inner.engine);
                menus::enabled_by_reading(rule, reading.has_document(), reading.has_selection()).unwrap_or(false)
            }
            Enable::Update => self.inner.updates.menu_item().enabled,
            Enable::Save => self.can_save(),
            Enable::Print => reader(|reader| !reader.home && reader.shown_tab != 0) && !self.inner.printing.busy(),
            Enable::SaveAs => reader(|reader| !reader.home && !reader.saving) && self.inner.save_ui.disagreement_now().is_none(),
            Enable::Close => reader(|reader| !reader.home && !reader.saving),
            Enable::ClearRecent => !lock(&self.inner.published).recent.is_empty(),
            Enable::Undo => reader(|reader| reader.can_undo),
            Enable::Redo => reader(|reader| reader.can_redo),
            Enable::Back => reader(|reader| reader.can_go_back),
            Enable::Forward => reader(|reader| reader.can_go_forward),
        }
    }

    /// Whether Save does anything now: the tab showing has unsaved edits and isn't being saved.
    fn can_save(&self) -> bool {
        let reader = lock(&self.inner.published).reader.clone();
        // Not under a sheet: the close prompt's own Save is how to save then.
        !reader.home && reader.edited && reader.save_enabled && self.inner.sheets.load(Ordering::SeqCst) == 0
    }

    /// What Save As's sheet says, for the tab showing. Never waits: the path is read only if the session's free, and the signature line is what the session thread found.
    pub fn save_panel(&self) -> saving::SavePanelModel {
        let reader = lock(&self.inner.published).reader.clone();
        saving::panel_for(reader.shown_tab, &reader.document_name, &reader.document_folder, reader.save_as_note).into()
    }

    /// Save As's sheet was answered with this path. `replace` is whether the person agreed to replace a file that's there, which the sheet asks.
    pub fn save_as(&self, tab: u32, path: String) {
        if self.inner.core.frozen.load(Ordering::SeqCst) || tab == 0 {
            return;
        }
        saving::start_save(&self.inner, TabId(tab), Some(PathBuf::from(path)), false);
    }

    /// What closing the document showing asks.
    pub fn close_prompt(&self) -> saving::ClosePrompt {
        let published = lock(&self.inner.published);
        // Save offered only where Save works: with it off for a signed document, the way on is Save As's unsigned copy.
        let save_off = published.reader.edited && !published.reader.save_enabled && !published.reader.saving;
        saving::close_prompt(
            published.reader.shown_tab,
            &published.reader.document_name,
            published.reader.edited,
            published.sidebar.undone_owner_steps,
            save_off,
            self.inner.save_ui.disagreement_now().as_deref(),
        )
        .into()
    }

    /// The close prompt's answer. Save closes once it's saved, and not if the save fails, which says why. Don't Save throws the edits away and closes. Save As opens its sheet, and the tab stays.
    pub fn close_answered(&self, tab: u32, choice: saving::CloseChoice) -> HostAction {
        if tab == 0 {
            return HostAction::None;
        }
        if choice == saving::CloseChoice::SaveAs {
            return HostAction::ShowSavePanel;
        }
        self.inner.core.push(CoreCall::CloseAnswered(TabId(tab), choice.into()));
        HostAction::None
    }

    /// A save's problem was answered: with Save As, its sheet opens.
    pub fn save_problem_answered(&self, save_as: bool) -> HostAction {
        self.inner.save_ui.set_problem(None);
        self.inner.core.push(CoreCall::Reconcile);
        if save_as { HostAction::ShowSavePanel } else { HostAction::None }
    }

    /// The system says memory's short. From a dispatch source the host keeps, which is all that's the host's about it.
    pub fn memory_pressure(&self, critical: bool) {
        self.inner.system_pressure(critical);
    }

    /// What a right-click on a page offers.
    /// What a right-click on a pane offers: its own layout ticked, so a pane that isn't being read shows its own.
    pub fn context_menu(&self, pane: u32) -> Vec<MenuEntry> {
        let reader = lock(&self.inner.published).reader.clone();
        let layout = reader.panes.get(pane as usize).map_or(reader.layout.as_str(), |view| view.layout.as_str());
        menus::context_menu(record::Spread::from_name(layout), (!reader.panes.is_empty()).then_some(reader.compare_synced))
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// The mouse went down on a page, in the document view's coordinates, the same at any zoom. Apply what comes back in the same transaction. It looks through text already here and never waits for any.
    /// In a comparison, a press in a pane makes it the one being read, and what was selected in another goes, so there's one selection to copy.
    pub fn selection_press(&self, pane: u32, x: f64, y: f64, clicks: u32, shift: bool) -> Vec<ReaderOp> {
        let Some(engine) = self.inner.panes.engine(&self.inner.engine, pane as usize) else {
            return Vec::new();
        };
        if let Some(tab) = self.inner.panes.focus(pane as usize) {
            // Never a wait on another pane: a busy one lets go of its selection from its own thread.
            for other in self.inner.panes.engines(&self.inner.engine) {
                if !other.same_as(&engine) {
                    other.clear_selection_soon();
                }
            }
            self.focus_moved(tab);
        }
        // With the snapshot tool on, a press is the tool's, not a selection's.
        if self.inner.tool.is_on() {
            return self.inner.tool.press(&engine, pane as usize, x, y);
        }
        engine.selection_press(x, y, clicks, shift).into_iter().map(ReaderOp::from).collect()
    }

    pub fn selection_drag(&self, pane: u32, x: f64, y: f64) -> Vec<ReaderOp> {
        let Some(engine) = self.inner.panes.engine(&self.inner.engine, pane as usize) else {
            return Vec::new();
        };
        if self.inner.tool.is_drawing() {
            return self.inner.tool.drag(&engine, pane as usize, x, y);
        }
        engine.selection_drag(x, y).into_iter().map(ReaderOp::from).collect()
    }

    /// The mouse went up. A still single click on a link follows it, by Rust's rule: within the document at once, a web or email address only once the sheet's been answered, and anything else never, with a notice saying why.
    pub fn selection_release(&self, pane: u32) -> Vec<ReaderOp> {
        let Some(engine) = self.inner.panes.engine(&self.inner.engine, pane as usize) else {
            return Vec::new();
        };
        if self.inner.tool.is_drawing() {
            return self.inner.tool.release(&self.inner, &engine, pane as usize);
        }
        let ops = engine.selection_release().into_iter().map(ReaderOp::from).collect();
        if let Some(hit) = engine.take_clicked_link() {
            follow_link(&self.inner, pane as usize, &engine, hit);
        }
        ops
    }

    /// **What the pointer looks like over a page, and what a link under it says.** A link's page not read yet says nothing for now and is read next, so the look comes with the next move. A short lock on the page view and the published state, never the session's.
    pub fn pointer_look(&self, pane: u32, x: f64, y: f64) -> linking::PointerLook {
        let Some(engine) = self.inner.panes.engine(&self.inner.engine, pane as usize) else {
            return linking::PointerLook {
                cursor: "arrow".into(),
                hint: None,
            };
        };
        // The snapshot tool's pointer is a crosshair wherever it is, and a link isn't followed while it's on.
        if self.inner.tool.is_on() {
            return linking::PointerLook {
                cursor: "crosshair".into(),
                hint: None,
            };
        }
        if let Some(hit) = engine.pointer_link(x, y) {
            let hint = match &hit.target {
                recto_core::nav::LinkTarget::Page { page, .. } => match engine.position_in_same_file(hit.page, *page) {
                    Some(position) => {
                        let toc = lock(&self.inner.published).pane_tocs.get(pane as usize).cloned().flatten();
                        Some(linking::page_words(position, engine.own_file_page(position), toc.as_deref()))
                    }
                    None => Some("The page this link goes to isn't in the document as it's been edited".to_string()),
                },
                other => linking::hint_for(other),
            };
            return linking::PointerLook { cursor: "link".into(), hint };
        }
        linking::PointerLook {
            cursor: if engine.pointer_is_over_text(x, y) { "ibeam" } else { "arrow" }.into(),
            hint: None,
        }
    }

    /// The sheet asking about a web or email address was answered, by its id. With Open, the address comes back for the host to hand to the system, exactly as the sheet showed it; with Cancel, nothing does.
    pub fn link_prompt_answered(&self, id: u64, open: bool) -> Option<String> {
        let mut asked = lock(&self.inner.prompts.link);
        // An answer is to the sheet that was shown: a later link's asking is left for its own answer.
        if asked.as_ref().is_none_or(|prompt| prompt.id != id) {
            return None;
        }
        let prompt = asked.take();
        drop(asked);
        self.inner.core.push(CoreCall::Reconcile);
        prompt.filter(|_| open).map(|prompt| prompt.address)
    }

    /// What the go-to field starts with, and its tooltip.
    pub fn page_field(&self) -> linking::PageField {
        let reader = lock(&self.inner.published).reader.clone();
        linking::PageField {
            editing_text: reader.page_text,
            tooltip: "Page. Click to type a page number, or what the document calls a page, such as iv".into(),
        }
    }

    /// Return in the go-to field: a label the file uses, or a page number. Says whether it was something to look up; one that names no page says so by the page.
    pub fn page_typed(&self, text: String) -> bool {
        let typed = text.trim();
        if typed.is_empty() || typed.len() > linking::MAX_TYPED_PAGE || self.inner.core.frozen.load(Ordering::SeqCst) {
            return false;
        }
        let reading = self.inner.panes.reading();
        let Some(tab) = self.inner.panes.tab_of(reading) else { return false };
        self.inner.core.push(CoreCall::JumpTyped(tab, typed.to_string()));
        true
    }

    /// A swipe on the page, as the system reports it: Back or Forward, as in Safari.
    pub fn swiped(&self, delta_x: f64) {
        if let Some(back) = linking::swipe_goes_back(delta_x) {
            self.inner.core.push(CoreCall::Step(back));
        }
    }

    /// Whether the pointer's over text, for the I-beam. A page not read yet says no, and its text is read when there's nothing better to do.
    pub fn pointer_is_over_text(&self, pane: u32, x: f64, y: f64) -> bool {
        // The snapshot tool's pointer is a crosshair wherever it is.
        if self.inner.tool.is_on() {
            return false;
        }
        self.inner
            .panes
            .engine(&self.inner.engine, pane as usize)
            .is_some_and(|engine| engine.pointer_is_over_text(x, y))
    }

    /// How far to scroll per step of a drag this far past the view's edge.
    pub fn autoscroll_step(&self, beyond: f64) -> f64 {
        recto_reader::selection::autoscroll_step(beyond)
    }

    /// The selection's text if it's all here, for what can't wait, as the Services menu can't. Nothing if not.
    pub fn selection_text_now(&self) -> Option<String> {
        self.inner.panes.reading_engine(&self.inner.engine).selection_text_now()
    }

    /// Whether there's text the Services menu can be offered, which it asks often, on the UI thread. Builds nothing.
    pub fn selection_can_be_sent(&self) -> bool {
        self.inner.panes.reading_engine(&self.inner.engine).selection_can_be_sent()
    }

    /// The reader has dismissed the notice, or its time's up.
    pub fn dismiss_selection_notice(&self) {
        for engine in self.inner.panes.engines(&self.inner.engine) {
            engine.dismiss_selection_notice();
        }
        lock(&self.inner.action_notice).take();
        self.inner.tool.dismiss_notice();
        self.inner.printing.dismiss_notice();
        self.inner.updates.dismiss_notice();
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// Where a point on a page is in the document, for QA to go through what the mouse goes through.
    pub fn document_point(&self, pane: u32, page: u32, x: f64, y: f64) -> Option<DocumentPoint> {
        self.inner
            .panes
            .engine(&self.inner.engine, pane as usize)?
            .document_point(page, x, y)
            .map(|(x, y)| DocumentPoint { x, y })
    }

    /// The pane being read, for QA and the host.
    pub fn reading_pane(&self) -> u32 {
        u32::try_from(self.inner.panes.reading()).unwrap_or(0)
    }

    /// The id a key does with nothing held, or none.
    pub fn plain_key_action(&self, characters: String) -> Option<String> {
        menus::plain_key(&characters).map(str::to_string)
    }

    /// The zoom level in the top left, as the host's magnification and the page's fit make it. Cheap: on every change of zoom.
    pub fn zoom_label(&self, magnification: f64, fit: f64) -> menus::ZoomLabel {
        menus::zoom_label(magnification, fit).into()
    }

    /// What the reader typed in the zoom level's field, with their Mac's decimal sign. Says whether it was a level: if not, nothing changes and the field shows what it was. Held to the limits as every zoom is.
    pub fn zoom_typed(&self, text: String, decimal_separator: String) -> bool {
        let decimal = decimal_separator.chars().next().unwrap_or('.');
        let Some(zoom) = recto_reader::tiles::typed_zoom(&text, decimal) else {
            return false;
        };
        self.inner.panes.zoom_all(&self.inner.engine, zoom).is_some()
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
        for engine in self.inner.panes.engines(&self.inner.engine) {
            engine.window_closed();
        }
        let mut app = lock(&self.inner.app);
        app.window_closed = true;
        // A window opened again isn't in the Dock.
        if let Some(window) = app.window.as_mut() {
            window.minimised = false;
        }
        drop(app);
        self.inner.saver.view_changed();
    }

    /// The window's going, for another made at once in its place, from a state that came in late: what it showed goes, as for a closed window, and nothing's recorded as closed.
    pub fn window_replaced(&self) {
        let engines = self.inner.panes.engines(&self.inner.engine);
        // What's selected goes with the window's layers, and comes back on the new one (a selection and Find's hit, under a late state).
        let held: Vec<(usize, recto_index::select::Range)> = engines
            .iter()
            .enumerate()
            .filter_map(|(pane, engine)| Some((pane, engine.hot_selection()?.1?)))
            .collect();
        for engine in &engines {
            engine.window_closed();
        }
        // What the going window said about itself since the state came in isn't kept: the new one comes from the state. What the host chose about what's out of sight is.
        let mut app = lock(&self.inner.app);
        if let Some(state) = app.from_state.take() {
            let minimised = app.window.as_ref().is_some_and(|window| window.minimised);
            let (hidden, closed) = (app.app_hidden, app.window_closed);
            let decided = (app.hidden_decided, app.minimised_decided, app.window_decided);
            *app = *state;
            app.app_hidden = hidden;
            app.window_closed = closed;
            (app.hidden_decided, app.minimised_decided, app.window_decided) = decided;
            if let Some(window) = app.window.as_mut() {
                window.minimised = minimised;
            }
        }
        app.selections_held = held;
    }

    /// The window went into the Dock or came out of it. Off the window's delegate, or QA.
    pub fn window_minimised_changed(&self, minimised: bool) {
        if let Some(window) = lock(&self.inner.app).window.as_mut() {
            window.minimised = minimised;
        }
        self.inner.saver.view_changed();
    }

    /// The app was hidden or shown again, by the system's notices, or QA. Going out of sight writes what's changed, as going to the background does.
    pub fn app_hidden_changed(&self, hidden: bool) {
        lock(&self.inner.app).app_hidden = hidden;
        if hidden {
            self.inner.saver.write_soon();
        } else {
            self.inner.saver.view_changed();
        }
    }

    /// Whether this launch hides the app as soon as it's up: an update arriving to an app that was hidden. Any other launch shows it, and the record's told so.
    pub fn app_comes_back_hidden(&self) -> bool {
        let mut app = lock(&self.inner.app);
        let hidden = updates::stays_out_of_sight(app.app_hidden, self.inner.updates.reopened_by_update());
        app.app_hidden = hidden;
        app.hidden_decided = true;
        hidden
    }

    pub fn window_opened(&self) {
        let engines = self.inner.panes.engines(&self.inner.engine);
        for engine in &engines {
            engine.window_opened();
        }
        let held = std::mem::take(&mut lock(&self.inner.app).selections_held);
        for (pane, range) in held {
            if let Some(engine) = engines.get(pane) {
                engine.restore_selection((range.start.page, range.start.at), (range.end.page, range.end.at));
            }
        }
        // A new window's contents start at the top: they go back to where they were, following or not, as at a launch. A window made again from a state read late is one (CI, 2026-11-11).
        let contents_y = lock(&self.inner.app).contents_y;
        if let Some(y) = contents_y {
            let mut follow = lock(&self.inner.contents_follow);
            let following = follow.is_following();
            follow.new_contents();
            follow.resume_at(y, following);
        }
        lock(&self.inner.app).window_closed = false;
        self.inner.saver.view_changed();
    }

    /// Whether this launch opens a window. Any launch does but an update's arrival, which leaves things as they were: a window closed stays closed, rather than appearing behind whatever the user's doing.
    pub fn window_should_open(&self) -> bool {
        let reopened = self.inner.updates.reopened_by_update();
        let mut app = lock(&self.inner.app);
        app.window_decided = true;
        !(app.window_closed && reopened)
    }

    /// Whether the state's been read and taken in. A window that went up without it, past the patience, is made again from it once it has: see the host's `restoreReady`.
    pub fn state_read(&self) -> bool {
        self.inner.restored.load(Ordering::SeqCst)
    }

    /// What the host shows has been made from the state that was read.
    pub fn shown_with_state(&self) {
        self.inner.shown_with_state.store(true, Ordering::SeqCst);
    }

    /// Whether the window takes what a person does: once it's been made from the state, or once the state's been waited for long enough. Until then it's shown and takes nothing, as it's made again from the state when that comes, and what was done to it would go with it.
    pub fn window_interactive(&self) -> bool {
        updates::takes_input(self.inner.shown_with_state.load(Ordering::SeqCst), self.inner.started.elapsed())
    }

    /// Whether a state that's come in late may still have the window made again from it: not once the window's taken input without it.
    pub fn may_remake_window(&self) -> bool {
        !self.window_interactive()
    }

    /// There's no window to wait for, so this launch has come up as far as it goes.
    pub fn no_window_to_show(&self) {
        let updates = Arc::clone(&self.inner.updates);
        let _ = std::thread::Builder::new()
            .name("recto-update-frame".into())
            .stack_size(UPDATE_FRAME_STACK)
            .spawn(move || updates::first_frame_shown(&updates));
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
    pub fn qa_note_step(&self, micros: u64, cpu_micros: u64, contents_y: f64, current_visible: bool) {
        if let Some(qa) = &self.inner.qa {
            qa.note_step(micros, cpu_micros, contents_y, current_visible);
        }
    }

    /// Reads the state, on a thread of its own, and hears `restore_ready` when it's applied. It's what the window waits for, up to `restore_patience_seconds`: its frame and the place on it come from what's read.
    pub fn begin_restore(&self) {
        let inner = Arc::clone(&self.inner);
        let spawned = std::thread::Builder::new().name("recto-restore".into()).stack_size(1024 * 1024).spawn(move || {
            // Before the record: a version that keeps failing goes back here, and not by reading on.
            let facts = lock(&inner.update_facts).take();
            let misreads = updates::misreads_the_record(facts.as_ref());
            if let Some(facts) = facts
                && inner.updates.launched(facts) == recto_update::updater::AtLaunch::Exit
            {
                std::process::exit(0);
            }
            recto_platform::slow::pause(recto_platform::slow::Seam::State);
            let loaded = if misreads { Loaded::Nothing } else { inner.saver.store.load() };
            let loaded = updates::fall_back_if_unreadable(&inner, loaded);
            updates::note_tabs_read(&loaded);
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
        let Some(active) = self.inner.host.session.try_with_session(|session| session.active()) else {
            return false;
        };
        active.is_none() || self.inner.engine.is_on_place() || self.inner.engine.is_showable()
    }

    /// The window has just been put on the screen, showing this. For QA to see what the first frame was.
    pub fn window_shown(&self, home: bool, tile_layers: u32) {
        for engine in self.inner.panes.engines(&self.inner.engine) {
            engine.window_up();
        }
        let report = self.inner.engine.report().0;
        *lock(&self.inner.first_shown) = Some(
            json!({ "home": home, "page": report["page"], "scroll_y": report["scroll_y"], "scroll_x": report["scroll_x"], "magnification": report["magnification"], "settled": report["settled"], "tab": report["tab"], "tile_layers": tile_layers, "ms_from_start": self.inner.started.elapsed().as_secs_f64() * 1000.0 }),
        );
        // The installer's waiting for this, off this thread, as it writes a small file and may start a worker, and only for a version that's new here.
        let updates = Arc::clone(&self.inner.updates);
        let _ = std::thread::Builder::new()
            .name("recto-update-frame".into())
            .stack_size(UPDATE_FRAME_STACK)
            .spawn(move || updates::first_frame_shown(&updates));
    }

    /// Where the new window goes, on the screens there are now. Rust's rule, from what was saved. In a quiet run it keeps its place off every screen, and only its size comes back.
    pub fn window_placement(&self, screens: Vec<ScreenInfo>) -> WindowPlacement {
        let app = {
            let mut app = lock(&self.inner.app);
            // In the Dock across an update, as it was. Any other launch shows it, and the record's told so.
            let arrived = self.inner.updates.reopened_by_update();
            if let Some(window) = app.window.as_mut() {
                window.minimised = updates::stays_out_of_sight(window.minimised, arrived);
            }
            app.minimised_decided = true;
            app.clone()
        };
        let minimised = app.window.as_ref().is_some_and(|window| window.minimised);
        let activate = app.focus.app_frontmost;
        let focus_sidebar = app.focus.view == FocusView::Sidebar;
        let in_background = updates::opens_in_background(self.inner.updates.reopened_by_update(), activate);
        let Some(saved) = app.window else {
            return WindowPlacement {
                saved: false,
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
                full_screen: false,
                activate,
                focus_sidebar,
                in_background,
                full_screen_when_active: false,
                minimised: false,
            };
        };
        if self.qa_mode {
            return WindowPlacement {
                saved: true,
                x: -40_000.0,
                y: -40_000.0,
                width: saved.frame.width.max(record::WINDOW_MIN.0),
                height: saved.frame.height.max(record::WINDOW_MIN.1),
                full_screen: saved.full_screen,
                activate,
                focus_sidebar,
                in_background: false,
                full_screen_when_active: false,
                minimised,
            };
        }
        let now: Vec<ScreenNow> = screens
            .into_iter()
            .map(|screen| ScreenNow {
                id: screen.id,
                name: screen.name,
                frame: Rect {
                    x: screen.x,
                    y: screen.y,
                    width: screen.width,
                    height: screen.height,
                },
                visible: Rect {
                    x: screen.visible_x,
                    y: screen.visible_y,
                    width: screen.visible_width,
                    height: screen.visible_height,
                },
            })
            .collect();
        let place = place_window(&saved, &now);
        let (full_screen, full_screen_when_active) = updates::full_screen_on_showing(place.full_screen, in_background);
        WindowPlacement {
            saved: true,
            x: place.frame.x,
            y: place.frame.y,
            width: place.frame.width,
            height: place.frame.height,
            full_screen,
            activate,
            focus_sidebar,
            in_background,
            full_screen_when_active,
            minimised,
        }
    }

    /// The window moved, resized or went full screen. Off a window's delegate, so at most as often as those.
    pub fn window_changed(&self, frame: Option<Vec<f64>>, screen: Option<ScreenInfo>, full_screen: bool) {
        let mut app = lock(&self.inner.app);
        let saved = app.window.get_or_insert(WindowRecord {
            frame: Rect {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            screen: None,
            full_screen: false,
            minimised: false,
        });
        // Its frame in full screen isn't where leaving full screen takes it.
        if let Some([x, y, width, height]) = frame.as_deref() {
            saved.frame = Rect {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
            };
            saved.screen = screen.map(|screen| ScreenRecord {
                id: screen.id,
                name: recto_core::clean_line(&screen.name),
                frame: Rect {
                    x: screen.x,
                    y: screen.y,
                    width: screen.width,
                    height: screen.height,
                },
            });
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
        let came_back = !state.focus.app_frontmost && app;
        state.focus.app_frontmost = app;
        state.focus.window_key = window_key;
        drop(state);
        if went_back {
            self.inner.saver.write_soon()
        } else {
            self.inner.saver.view_changed()
        }
        // Back from another app, which may have moved, replaced or deleted a file that's showing: each is looked at, a stat on a thread of its own, and said if it's something else now.
        if came_back {
            self.inner.core.push(CoreCall::LookAtShown);
        }
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
    pub fn set_update_facts(&self, version: String, dev_version: Option<String>, bundle_path: String, system_version: String) {
        self.inner.note_update_facts(version, dev_version, bundle_path, system_version);
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

    /// What an update chosen from the menu came to, as an alert, when there's no window to say it in. Taken once: the host shows it, app-modal.
    pub fn take_update_alert(&self) -> Option<updates::UpdateAlert> {
        self.inner.take_update_alert()
    }

    /// The alert's been answered: what it said has been read.
    pub fn update_alert_answered(&self) {
        self.inner.updates.dismiss_notice();
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
        updates::opens_in_background(self.inner.updates.reopened_by_update(), lock(&self.inner.app).focus.app_frontmost)
    }

    /// A quit was asked for, by the menu or the system. Everything's written, on a thread of its own, and `quit_answer` says what to do. Mac apps with Resume don't ask on the way out, and neither does this, as unsaved edits come back: closing a tab with them is where Don't Save, Cancel or Save belongs. It asks only if they couldn't be kept.
    pub fn quit_requested(&self) {
        let inner = Arc::clone(&self.inner);
        // What's done from here wouldn't come back. If it turns out it isn't quitting, it's taken again.
        inner.set_frozen(true);
        inner.updates.stop();
        inner.tool.quitting();
        inner.printing.quitting();
        let _ = std::thread::Builder::new().name("recto-quit".into()).stack_size(256 * 1024).spawn(move || {
            let (sender, receiver) = std::sync::mpsc::channel();
            let flushing = Arc::clone(&inner);
            let _ = std::thread::Builder::new().name("recto-flush".into()).stack_size(512 * 1024).spawn(move || {
                let _ = sender.send(flushing.saver.flush(false, Duration::from_secs(2)));
            });
            let outcome = receiver.recv_timeout(QUIT_PATIENCE);
            let fine = matches!(outcome, Ok(Ok(_))) || !inner.saver.store.holds_the_record();
            let unsaved = inner.host.session.unsaved_tabs();
            // Asked for, Quit Anyway included, so this run of a version being watched didn't end early. Stay says otherwise.
            inner.updates.quitting_as_asked();
            if !(fine || unsaved.is_empty()) {
                inner.set_frozen(false);
                inner.updates.stop_over();
            }
            inner.listener.quit_answer(recto_app::listener::QuitAnswer::for_quit(fine, unsaved.len()));
        });
    }

    /// The quit's question was answered with Stay: the app goes on, and isn't quitting after all.
    pub fn quit_stayed(&self) {
        self.inner.updates.quit_not_taken();
    }

    /// Ends the socket, on the way out. Returns at once.
    pub fn shutdown(&self) {
        if let Some(stopper) = lock(&self.inner.stopper).take() {
            stopper.stop();
        }
        self.inner.forget_a_quiet_copy();
    }
}

impl AppCore {
    /// The pane being read changed, by the pointer or a scroll: the session's told on its own thread, and nothing waits.
    fn focus_moved(&self, tab: TabId) {
        self.inner.core.push(CoreCall::FocusPane(tab));
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
    if synced.panes.iter_mut().any(|pane| pane.tab.take().is_some()) {
        inner.engine.deactivate();
        inner
            .panes
            .set_count(&inner.engine, 1, synced.paper.unwrap_or(PaperKind::White), !lock(&inner.app).window_closed);
        synced.count = 1;
        synced.panes.truncate(1);
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
                    app.window_closed = updates::out_of_sight_when_restored(saved.window_closed, app.window_decided.then_some(app.window_closed));
                    // Once the host's shown or hidden things without the state, what it did is how things are, and the record says that, not what was saved.
                    app.app_hidden = updates::out_of_sight_when_restored(saved.app_hidden, app.hidden_decided.then_some(app.app_hidden));
                    let minimised_now = app.minimised_decided.then(|| app.window.as_ref().is_some_and(|window| window.minimised));
                    app.window = saved.window.clone();
                    if let Some(window) = app.window.as_mut() {
                        window.minimised = updates::out_of_sight_when_restored(window.minimised, minimised_now);
                    }
                    app.appearance = saved.appearance;
                    app.focus = saved.focus;
                    let active = restored.record.active.map(TabId);
                    app.contents_resume = active.zip(saved.contents_scroll);
                    app.selection_resume = active.and_then(|id| {
                        restored
                            .record
                            .tabs
                            .iter()
                            .find(|tab| tab.id == id.0)?
                            .selection
                            .map(|selection| (id, selection))
                    });
                    app.contents_y = saved.contents_scroll.map(|scroll| scroll.y);
                    // Taken in with a window already up without it: the window made again from it goes by what it said.
                    if app.window_decided {
                        let state = AppState {
                            from_state: None,
                            ..app.clone()
                        };
                        app.from_state = Some(Box::new(state));
                    }
                }
                // A code this build doesn't know has no words, and goes.
                *lock(&host.notice) = saved.notice.clone().filter(|code| record::notices::words(code).is_some());
                holding = host
                    .session
                    .with_session(|session| session.active().and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id)))));
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
    let paper = recto_platform::local::read_private_file(&paths.root.join(PAPER_FILE), 16)
        .ok()
        .flatten()
        .and_then(|bytes| std::str::from_utf8(&bytes).ok().and_then(Paper::from_name));
    let hidden = paths.root.join(AGENT_ICON_FILE).exists();
    if paper.is_none() && !hidden {
        return;
    }
    inner.host.session.with_session(|session| {
        session.set_view(ViewSettings {
            paper: paper.or(session.view().paper),
            agent_icon_hidden: hidden,
            ..session.view()
        })
    });
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
    let _ = std::thread::Builder::new().name("recto-control".into()).stack_size(512 * 1024).spawn(move || {
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
        recto_platform::slow::pause(recto_platform::slow::Seam::Socket);
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
            CoreCall::LookAtShown => {
                if let Some(inner) = inner.saver.inner() {
                    for id in host.session.with_session(|session| session.panes()) {
                        sleeper::check_woken_file(&inner, id);
                    }
                }
            }
            CoreCall::Unlocked(ask, loaded, stamp, password) => match loaded {
                Ok(facts) => {
                    // Kept by which file it is, so a move keeps it and a new file in its place doesn't get it. A file with no such numbers can't have its password kept for drawing it, so it's said rather than asked again for ever.
                    let identity = stamp.map(|stamp| (stamp.device, stamp.inode)).filter(|identity| *identity != (0, 0));
                    match identity {
                        None => {
                            lock(&inner.prompts.passwords).failed(ask.id);
                            *lock(&inner.open_failure) = Some(passwords::CANT_KEEP.to_string());
                        }
                        Some(identity) => {
                            lock(&inner.prompts.passwords).opened(ask.id);
                            inner.keys.set(identity, password);
                            for engine in inner.panes.engines(&inner.engine) {
                                engine.key_given(&ask.path);
                            }
                            match ask.target {
                                passwords::Target::File => match host.session.with_session(|session| session.open_unlocked(&ask.path, facts, stamp)) {
                                    Ok(id) => events.push(host.session.with_session(|session| recto_agent::event::tab_opened(session, id))),
                                    Err(error) => *lock(&inner.open_failure) = Some(error.message().to_string()),
                                },
                                passwords::Target::Tab(id) => host.session.with_session(|session| session.unlock(id, facts, stamp)),
                            }
                        }
                    }
                }
                Err(OpenError::PasswordNeeded) => {
                    if lock(&inner.prompts.passwords).wrong(ask.id) == passwords::Wrong::Stopped {
                        *lock(&inner.action_notice) = Some(passwords::stopped_asking());
                    }
                }
                Err(error) => {
                    lock(&inner.prompts.passwords).failed(ask.id);
                    *lock(&inner.open_failure) = Some(error.message().to_string());
                }
            },
            CoreCall::ClearRecent => {
                host.session.with_session(|session| session.clear_recent());
                inner.saver.write_soon();
            }
            CoreCall::Restored(loaded) => {
                let first_time = !inner.restored.load(Ordering::SeqCst);
                apply_restored(inner, *loaded, &mut synced, &mut reads, &mut deferred);
                // What every call ends with, and before the host's told, as it makes the window from what's published now: Find's field among it, or a field that had the keyboard comes back without it (CI, run 43062872948).
                after_call(inner, &mut synced, &mut reads, events);
                if first_time {
                    // A test's slow window: the window that went up without the state is made again from it this much later, so what's done in between can be seen to wait for it.
                    let listener = Arc::clone(&inner.listener);
                    if inner.shown_without_state.load(Ordering::SeqCst) {
                        recto_platform::slow::later(recto_platform::slow::Seam::Remake, move || listener.restore_ready());
                    } else {
                        listener.restore_ready();
                    }
                }
                recto_platform::slow::pause(recto_platform::slow::Seam::Session);
                continue;
            }
            CoreCall::RestorePatienceOver => {
                if !inner.restored.load(Ordering::SeqCst) {
                    match updates::restore_patience(inner.updates.may_be_reopened_by_update(), inner.started.elapsed()) {
                        // The state hasn't come. The window goes up without it, and when it does come it's taken in.
                        updates::Patience::ShowNow => updates::show_without_state(&inner.shown_without_state, &*inner.listener),
                        updates::Patience::WaitFor(left) => updates::look_again_after(inner, left),
                    }
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
            CoreCall::Jump(id, page, within) => {
                // Where the reader is, as the page view has it now, is what Back comes back to. `jump` clamps: a contents entry can't send the reader off the end.
                inner.take_places();
                if let Ok(event) = host
                    .session
                    .with_session(|session| session.jump(id, page, within).map(|()| recto_agent::page_now(session, id)))
                {
                    events.push(event);
                }
            }
            CoreCall::JumpTyped(id, typed) => {
                inner.take_places();
                let went = host.session.with_session(|session| {
                    let page = session.page_for_typed(id, &typed)?;
                    session.jump(id, page, 0.0).ok()?;
                    Some(recto_agent::page_now(session, id))
                });
                match went {
                    Some(event) => events.push(event),
                    None => *lock(&inner.action_notice) = Some(linking::no_such_page(&typed)),
                }
            }
            CoreCall::Step(back) => {
                inner.take_places();
                let moved = host.session.with_session(|session| {
                    let id = session.active()?;
                    let moved = if back { session.go_back(id) } else { session.go_forward(id) };
                    moved.ok().filter(|moved| *moved).map(|_| recto_agent::page_now(session, id))
                });
                if let Some(event) = moved {
                    events.push(event);
                }
            }
            CoreCall::Open(path) => {
                *lock(&inner.open_failure) = None;
                // Opening a file again is what starts its password tries afresh (SPEC).
                let (tab, own) = host.session.with_session(|session| {
                    let tab = session.tab_for_path(&path);
                    (tab, tab.and_then(|id| session.path_of(id)))
                });
                lock(&inner.prompts.passwords).opened_again(&[Some(path.clone()), own].into_iter().flatten().collect::<Vec<_>>(), tab);
                let cold = host
                    .session
                    .with_session(|session| session.tab_for_path(&path).and_then(|id| session.wake_for_tap(id)));
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
                let reread = host.session.with_session(|session| {
                    session
                        .active()
                        .filter(|active| *active == id)
                        .and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id))))
                });
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
                let reread = host.session.with_session(|session| {
                    session
                        .active()
                        .filter(|active| wrong && *active == id)
                        .and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id))))
                });
                if let Some((id, path, stamp)) = reread {
                    reads.wanted_tab = Some(id);
                    reads.start_restored(inner, id, path, stamp);
                }
            }
            CoreCall::Events(list) => events.extend(list),
            CoreCall::CloseTab(id, discard) => close_tab(host, id, discard, &mut events),
            CoreCall::CloseAnswered(id, choice) => match choice {
                recto_app::saving::CloseChoice::Save => {
                    if let Some(inner) = inner.saver.inner() {
                        saving::start_save(&inner, id, None, true);
                    }
                }
                recto_app::saving::CloseChoice::DontSave => close_tab(host, id, true, &mut events),
                recto_app::saving::CloseChoice::Cancel | recto_app::saving::CloseChoice::SaveAs => {}
            },
            CoreCall::CloseAfterSave(id) => {
                // An edit that came in between the save and now isn't in the file: the tab stays, and says so, rather than lose it (review).
                let closed = host.session.with_session(|session| {
                    session.close_after_save(id).inspect(|closed| {
                        if let Some(closed) = closed {
                            recto_agent::event::closed(session, *closed, &mut events);
                        }
                    })
                });
                if let Ok(None) = closed {
                    *lock(&inner.action_notice) = Some(saving::SAVED_NOT_CLOSED.to_string());
                }
            }
            CoreCall::ClearEditHistory => inner.host.session.with_session(|session| {
                if let Some(id) = session.active() {
                    session.clear_history(id, recto_session::ClearReason::UserAsked);
                }
            }),
            CoreCall::ToggleAgentIcon => set_view(inner, &mut events, |view| view.agent_icon_hidden = !view.agent_icon_hidden),
            CoreCall::ContentsAction(id, action) => generated_contents::act(inner, id, action),
            CoreCall::GeneratedFinished => generated_contents::look_again(inner),
            CoreCall::GeneratedView(id, view, version) => host.session.with_session(|session| session.set_generated_view_for(id, view, version)),
            CoreCall::ToggleSection(id, entry) => host.session.with_session(|session| session.toggle_contents_section(id, entry)),
            CoreCall::Find(call) => find::act(inner, call, &mut events),
            CoreCall::SetEditMode(on) => host.session.with_session(|session| session.set_edit_mode(on)),
            CoreCall::SetSidebarHidden(hidden) => set_view(inner, &mut events, |view| view.sidebar_hidden = hidden),
            CoreCall::Rotate(turns) => {
                let refused = host.session.with_session(|session| match session.active() {
                    None => Some(recto_session::SessionError::NoSuchTab),
                    Some(id) => match session.rotate_from_panel(id, turns) {
                        Ok(()) => {
                            events.push(recto_agent::edits_changed(session, id));
                            None
                        }
                        Err(error) => Some(error),
                    },
                });
                // It says why, by the page, and goes by itself.
                *lock(&inner.action_notice) = refused.map(|error| format!("Couldn't rotate: {}", error.message()));
            }
            CoreCall::UndoOrRedo(undo) => host.session.with_session(|session| {
                if let Some(id) = session.active()
                    && (if undo {
                        session.undo_edit_as(id, recto_session::Author::Owner)
                    } else {
                        session.redo_edit_as(id, recto_session::Author::Owner)
                    })
                    .unwrap_or(false)
                {
                    events.push(recto_agent::edits_changed(session, id));
                }
            }),
            CoreCall::CompareClick(id) => {
                // The tab joining, or the pane read next if the one read leaves, is read first with the session free, as a tap's is.
                let cold = host.session.with_session(|session| session.wake_for_compare_click(id));
                match cold {
                    Some((tab, path)) => {
                        reads.wanted_tab = Some(tab);
                        reads.start(inner, Reading::Compare(id, tab), path);
                    }
                    None => compare_click_now(inner, id, &mut events),
                }
            }
            CoreCall::FocusPane(id) => {
                let cold = host.session.with_session(|session| {
                    session
                        .comparison()
                        .is_some_and(|comparison| comparison.contains(id))
                        .then(|| session.wake_for_tap(id))
                        .flatten()
                });
                match cold {
                    Some((tab, path)) => {
                        reads.wanted_tab = Some(tab);
                        reads.start(inner, Reading::Pane(tab), path);
                    }
                    None => {
                        events.extend(
                            host.session
                                .with_session(|session| session.focus_pane(id).then(|| recto_agent::event::tab_activated(session, id))),
                        );
                    }
                }
            }
            CoreCall::SetSynced(on) => {
                events.extend(host.session.with_session(|session| {
                    session
                        .set_compare_synced(on)
                        .filter(|changed| *changed)
                        .map(|_| recto_agent::event::comparison_changed(session))
                }));
            }
            CoreCall::SetSpread(tab, spread) => host.session.with_session(|session| {
                if let Some(id) = tab.filter(|tab| session.tab(*tab).is_some()).or(session.active())
                    && session.set_spread(id, spread)
                {
                    events.push(Event::LayoutChanged {
                        tab: id,
                        layout: spread.name(),
                    });
                }
            }),
            // What was asked before this includes any file being read for it: let go at the end of this call if nothing's being read, or once the last read's done.
            CoreCall::Flush(marker) => reads.flushes.push(marker),
            CoreCall::Page(page, failure, tab) => {
                host.session.with_session(|session| session.note_drawn(DrawNote { tab, page, failure }));
                events.push(match failure {
                    None => Event::PageDrawn { tab, page: page + 1 },
                    Some(code) => Event::PageFailed { tab, page: page + 1, code },
                });
                // Nothing the sidebar or the reader shows comes from this. Going round for it would rebuild the model per tile drawn.
                if let Some(announcer) = lock(&inner.announcer).as_ref() {
                    announcer.announce(events);
                }
                continue;
            }
        }
        after_call(inner, &mut synced, &mut reads, events);
    }
}

/// **What every call on the session thread ends with,** the state's coming in too (review 1: it went on without the events it made being announced, or a flush that waited on a read being told it was done): generated contents and Find kept up with the session, the host's view made from it, flushes with nothing left to read let go, and what happened announced.
fn after_call(inner: &Arc<Inner>, synced: &mut Synced, reads: &mut FileReads, mut events: Vec<Event>) {
    generated_contents::look_if_chosen(inner);
    find::keep_up(inner);
    reconcile(inner, synced, &mut events);
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

/// Reads a file's facts off the session thread: with the password the person typed for it, if the keyring has one for that file, so a protected tab that went cold and wakes doesn't ask again; and otherwise as any file is. Whether a password was used.
fn read_facts(keys: &recto_reader::engine::Keyring, path: &std::path::Path) -> (Result<DocFacts, OpenError>, bool) {
    match keys.for_path(path) {
        Some(key) => (recto_core::inspect_with_password(path, &key), true),
        None => (DrawableLoader.load(path), false),
    }
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
        let keys = Arc::clone(&inner.keys);
        let spawned = std::thread::Builder::new().name("recto-read".into()).stack_size(1024 * 1024).spawn(move || {
            let status = look_at_file(&path, stamp);
            let path = if let FileStatus::Moved(moved, _) = &status { moved.clone() } else { path };
            let stamp = recto_platform::file_stamp(&path).ok();
            let (loaded, unlocked) = read_facts(&keys, &path);
            core.push(CoreCall::Read(Box::new(FileRead {
                what,
                loaded,
                stamp,
                status: Some(status),
                history: None,
                unlocked,
            })));
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
            let (loaded, unlocked) = read_facts(&inner_for_read.keys, &path);
            // A PDF already open by another name keeps what it has. The session says for sure when this gets there.
            let already =
                matches!(reading, Reading::File(_)) && inner_for_read.host.session.with_session(|session| session.tab_for_file(&path, stamp).is_some());
            // Only after a good read: taking it up marks it as open, and a file that couldn't be read won't have a tab to hold it.
            let history = if matches!(reading, Reading::File(_)) && !already && loaded.is_ok() {
                saver.store.history_of(stamp, &path)
            } else {
                None
            };
            if let Some(before) = before {
                before.wait(OPEN_ORDER_WAIT);
            }
            core.push(CoreCall::Read(Box::new(FileRead {
                what: reading,
                loaded,
                stamp,
                status: None,
                history,
                unlocked,
            })));
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
        // Read with a password: the tab's marked before anything read takes its place, so no agent can see it.
        if read.unlocked
            && let Reading::Restored(tab) | Reading::Tab(tab) | Reading::Compare(_, tab) | Reading::Pane(tab) = &read.what
        {
            host.session.with_session(|session| session.note_password_used(*tab));
        }
        match read.what {
            Reading::Restored(tab) => {
                if let Some(status) = read.status {
                    host.session.with_session(|session| session.apply_file_status(tab, status));
                }
                host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                // The tab it's for may have been left already.
                if lock(&inner.holding_tab).take().is_some() || self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                    host.session.with_session(|session| {
                        if session.active() == Some(tab) {
                            session.wake_active()
                        }
                    });
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
            Reading::Compare(clicked, tab) => {
                host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                if self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                }
                compare_click_now(inner, clicked, events);
            }
            Reading::Pane(tab) => {
                host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                if self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                }
                events.extend(
                    host.session
                        .with_session(|session| session.focus_pane(tab).then(|| recto_agent::event::tab_activated(session, tab))),
                );
            }
            Reading::File(path) => match read.loaded.map_err(SessionError::Open).and_then(|facts| {
                host.session.with_session(|session| {
                    if read.unlocked {
                        session.open_unlocked(&path, facts, read.stamp)
                    } else {
                        session.open_with_facts(&path, facts, read.stamp, read.history)
                    }
                })
            }) {
                Ok(id) => {
                    events.push(host.session.with_session(|session| recto_agent::event::tab_opened(session, id)));
                }
                // A protected file asks for its password, on the system's sheet, rather than only saying it needs one. After any other file asking, each with its own answer.
                Err(SessionError::Open(OpenError::PasswordNeeded)) => {
                    lock(&inner.prompts.passwords).ask(passwords::Target::File, &path);
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

/// What a ⌘-click on a tab does, once whatever it needs from disk is here. Too many panes is said by the page, and goes by itself.
fn compare_click_now(inner: &Inner, id: TabId, events: &mut Vec<Event>) {
    let outcome = inner.host.session.with_session(|session| session.toggle_compare(id));
    match outcome {
        Ok(recto_session::compare::CompareChange::Nothing) => {}
        Ok(_) => events.push(inner.host.session.with_session(|session| recto_agent::event::comparison_changed(session))),
        Err(error) => *lock(&inner.action_notice) = Some(error.message().to_string()),
    }
}

/// What a tap on a tab does, once whatever it needs from disk is here.
fn tap_now(inner: &Inner, id: TabId, events: &mut Vec<Event>) {
    let host = &inner.host;
    host.session.with_session(|session| {
        if let Ok(switched) = session.tap(id) {
            recto_agent::event::switched(session, switched, events);
        }
    });
}

/// The session's word for a layout, as the page view has it.
fn reader_spread(spread: record::Spread) -> recto_reader::layout::Spread {
    match spread {
        record::Spread::Single => recto_reader::layout::Spread::Single,
        record::Spread::TwoUp => recto_reader::layout::Spread::TwoUp,
        record::Spread::TwoUpCover => recto_reader::layout::Spread::TwoUpCover,
    }
}

/// What a page view was last told about the tab in its pane.
#[derive(Default)]
struct PaneSynced {
    tab: Option<TabId>,
    page: u32,
    pages: u32,
    map: PageMap,
    warm: Vec<u32>,
    spread: Option<record::Spread>,
}

/// **Follows a link a click landed on,** by Rust's rule: a page of this document at once, as a jump; a web or email address only once the sheet's been answered; Back and Forward; and anything else never, with a notice saying why.
fn follow_link(inner: &Inner, pane: usize, engine: &Engine, hit: recto_reader::links::LinkHit) {
    if inner.core.frozen.load(Ordering::SeqCst) {
        return;
    }
    let Some(tab) = inner.panes.tab_of(pane) else { return };
    match hit.target {
        recto_core::nav::LinkTarget::Page { page, within } => match engine.position_in_same_file(hit.page, page) {
            Some(position) => inner.core.push(CoreCall::Jump(tab, position, within.map_or(0.0, f64::from))),
            None => notice(inner, "The page this link goes to isn't in the document as it's been edited."),
        },
        recto_core::nav::LinkTarget::History { back } => inner.core.push(CoreCall::Step(back)),
        recto_core::nav::LinkTarget::Web(address) => {
            inner.prompts.ask_link(&address);
            inner.core.push(CoreCall::Reconcile);
        }
        recto_core::nav::LinkTarget::Refused(why) => notice(inner, why.message()),
        recto_core::nav::LinkTarget::Nowhere => notice(inner, "This link goes nowhere in this document."),
    }
}

/// A line by the page that goes by itself, in Rust's words.
fn notice(inner: &Inner, words: &str) {
    *lock(&inner.action_notice) = Some(words.to_string());
    inner.core.push(CoreCall::Reconcile);
}

/// **Shows a tab in a pane,** or brings what's shown up to date: another tab, another list of pages, a layout, a page an agent sent it to, or the warm tabs. A tab a pane's page view hasn't held starts from its sleep file's pictures, as any wake does.
fn show_in_pane(inner: &Inner, engine: &Engine, synced: &mut PaneSynced, shown: &PaneSnapshot) {
    let id = shown.id;
    // Its file's being looked at: nothing goes on the page until that's known.
    let holding = *lock(&inner.holding_tab) == Some(id) && synced.tab.is_none();
    if holding {
        return;
    }
    if synced.tab != Some(id) || synced.pages != shown.pages || synced.map != shown.map {
        inner.queue_those_leaving();
        // Whichever way it's been asked for, the first frame's what its sleep file has, if it's got one and nothing's held of this tab.
        if !engine.holds(id.0) {
            engine.note_wake(id.0, Instant::now());
            inner.give_sleep_view(engine, id);
        }
        engine.activate(
            id.0,
            shown.pages,
            shown.page,
            shown.map.clone(),
            &shown.warm,
            shown.saved,
            reader_spread(shown.spread),
        );
        synced.spread = Some(shown.spread);
        // What was selected comes back on the tab it was on, once.
        let selection = {
            let mut app = lock(&inner.app);
            if app.selection_resume.as_ref().is_some_and(|(tab, _)| *tab == id) {
                app.selection_resume.take().map(|(_, selection)| selection)
            } else {
                None
            }
        };
        if let Some(selection) = selection {
            engine.restore_selection((selection.from_page, selection.from_char), (selection.to_page, selection.to_char));
        }
        synced.tab = Some(id);
        synced.pages = shown.pages;
        synced.map = shown.map.clone();
        synced.page = shown.page;
        synced.warm = shown.warm.clone();
        return;
    }
    if synced.spread != Some(shown.spread) {
        engine.set_spread(reader_spread(shown.spread));
        synced.spread = Some(shown.spread);
    }
    // A jump lands where it says, on its page and as far down it, even on the page the reader's on already, as Back to a place higher up the same page does.
    if let Some(stop) = shown.jump {
        engine.jump_within(stop.page, f64::from(stop.within));
        synced.page = shown.page;
    } else if synced.page != shown.page {
        engine.jump_to(shown.page);
        synced.page = shown.page;
    }
    // A warm tab closed or gone cold: what it held goes now, and not at the next switch.
    if synced.warm != shown.warm {
        engine.set_warm(&shown.warm);
        synced.warm = shown.warm.clone();
    }
}

/// Brings everything into line with the session, on the session thread. The place in a tab can move two ways, by the reader or by an agent, and each is passed to the other side.
/// Closes a tab by the owner's word, throwing away its edits and history first if `discard`. A close the session refuses, as for edits made meanwhile, leaves it open.
fn close_tab(host: &AppHost, id: TabId, discard: bool, events: &mut Vec<Event>) {
    host.session.with_session(|session| {
        if discard {
            session.discard_edits(id);
        }
        if let Ok(closed) = session.close(id) {
            recto_agent::event::closed(session, closed, events);
        }
    });
}

fn reconcile(inner: &Inner, synced: &mut Synced, events: &mut Vec<Event>) {
    let nothing_held = lock(&inner.holding_tab).is_none();
    // A tab about to be shown in a pane that wasn't: its file's looked at first, so nothing of a version that's been replaced goes up, held or not.
    let panes_now = inner.host.session.with_session(|session| session.panes());
    for (pane, id) in panes_now.iter().enumerate() {
        if synced.panes.get(pane).and_then(|shown| shown.tab) != Some(*id) && nothing_held {
            sleeper::look_before_showing(inner, *id);
        }
    }
    let engines = inner.panes.engines(&inner.engine);
    let moved: Vec<(usize, u32)> = engines
        .iter()
        .enumerate()
        .filter_map(|(pane, engine)| Some((pane, engine.take_moved_page()?)))
        .collect();
    // What the page views hold of tabs they're about to let go of goes in the session first, as a cold tab holds nothing there.
    inner.take_places();
    let shown_before: Vec<Option<TabId>> = synced.panes.iter().map(|pane| pane.tab).collect();
    let held = {
        let published = lock(&inner.published);
        recto_app::Held {
            tabs: Arc::clone(&published.tabs),
            contents: Arc::clone(&published.contents),
            sidebar: Arc::clone(&published.sidebar),
            recent_key: published.recent_key,
        }
    };
    // One look at the session for the whole pass, so what's projected is of one moment: the places the page views moved first, then everything the window shows.
    let mut facts = inner.host.session.with_session(|session| {
        let panes = session.panes();
        for (pane, page) in &moved {
            if let (Some(id), Some(Some(shown))) = (panes.get(*pane).copied(), shown_before.get(*pane))
                && *shown == id
                && session.go_to(id, *page).is_ok()
            {
                let page = session.tab(id).map_or(*page, |tab| tab.page);
                if let Some(pane) = synced.panes.get_mut(*pane) {
                    pane.page = page;
                }
                events.push(recto_agent::page_changed(session, id, page + 1));
            }
        }
        let mut facts = recto_app::read(session, &held);
        for snapshot in &mut facts.panes {
            snapshot.jump = session.take_jump(snapshot.id);
        }
        facts
    });
    // The paper first, so what's asked for next is drawn right.
    let look = recto_app::paper_look(facts.chosen_paper, inner.dark.load(Ordering::SeqCst));
    let paper = look.kind;
    if synced.paper != Some(paper) {
        synced.paper = Some(paper);
        for engine in inner.panes.engines(&inner.engine) {
            engine.set_paper(paper);
        }
    }
    let active = facts.active;
    let comparison = facts.comparison.take();
    let pane_pages = std::mem::take(&mut facts.pane_pages);
    let shown_tab = facts.reading.shown_tab();
    // A notice about one document doesn't stay over another: a save's note, seen on the next tab, would read as being about that one.
    if inner.notice_tab.swap(shown_tab, Ordering::SeqCst) != shown_tab {
        lock(&inner.action_notice).take();
        snapshot_tool::tab_came_forward(inner);
    }
    // **Rust decides what a signed file's Save does** (SPEC decision 27): an edit a Save over it would break the signature with turns Save off, and Save As says its copy won't be signed.
    // Asked only with edits to save, off this thread, and the window's brought up to date again once it's known.
    let core_queue = Arc::clone(&inner.core);
    let reading = &facts.reading;
    let signature = if reading.edited {
        reading
            .file
            .as_ref()
            .and_then(|(id, path, stamp)| inner.save_ui.signature_facts(*id, path, *stamp, move || core_queue.push(CoreCall::Reconcile)))
            .unwrap_or_default()
    } else {
        recto_core::save::SignatureFacts::default()
    };
    let core_queue = Arc::clone(&inner.core);
    let save = inner.save_ui.save_state(
        signature,
        reading.needs_rewrite,
        reading
            .file
            .as_ref()
            .filter(|_| reading.edited)
            .map(|(_, path, stamp)| (path.as_path(), *stamp)),
        move || core_queue.push(CoreCall::Reconcile),
    );
    let problem = facts.reading_pane().and_then(|shown| shown.problem.clone());
    // The contents follow the tab being read, whichever pane it's in.
    let holding_now = *lock(&inner.holding_tab);
    if synced.active != active && active.is_some() && holding_now != active {
        let mut follow = lock(&inner.contents_follow);
        follow.new_contents();
        let mut app = lock(&inner.app);
        match app.contents_resume.take() {
            Some((tab, scroll)) if Some(tab) == active => follow.resume_at(scroll.y, scroll.following),
            _ => {}
        }
        synced.active = active;
    }
    // A page view for each pane, and no more: one that's gone ends with what it held.
    let count = facts.panes.len().max(1);
    if synced.count != count {
        let window_open = !lock(&inner.app).window_closed;
        inner.panes.set_count(&inner.engine, count, paper, window_open);
        synced.count = count;
    }
    synced.panes.resize_with(count, PaneSynced::default);
    if facts.panes.is_empty() && synced.panes[0].tab.take().is_some() {
        inner.engine.deactivate();
    }
    let mut panes_changed = false;
    for (pane, snapshot) in facts.panes.iter().enumerate() {
        let Some(engine) = inner.panes.engine(&inner.engine, pane) else { continue };
        panes_changed |= synced.panes[pane].tab != Some(snapshot.id);
        show_in_pane(inner, &engine, &mut synced.panes[pane], snapshot);
    }
    let shown_tabs: Vec<TabId> = facts.panes.iter().map(|shown| shown.id).collect();
    let reading_pane = active.and_then(|id| shown_tabs.iter().position(|shown| *shown == id)).unwrap_or(0);
    let comparing = comparison.is_some();
    inner.panes.lay_out(shown_tabs, reading_pane, comparison, pane_pages);
    if panes_changed && comparing {
        inner.panes.even_out_zoom(&inner.engine);
    }
    // A file whose renderer found it needs a password the keyring has none for, as one moved to another volume or replaced while it was open, needs it again: its tab says so, and is asked about as any is.
    for engine in inner.panes.engines(&inner.engine) {
        for path in engine.take_keyless() {
            inner.host.session.with_session(|session| {
                if let Some(id) = session.tab_for_path(&path) {
                    session.needs_password_again(id);
                }
            });
        }
    }
    // A protected tab showing with no password given is asked about, unless the person said Cancel and hasn't gone to it again, or its tries have run out.
    let (wants, active_tab) = inner.host.session.with_session(|session| {
        let active = session.active();
        (
            active.filter(|id| session.needs_password(*id)).and_then(|id| Some((id, session.path_of(id)?))),
            active,
        )
    });
    let asked = lock(&inner.prompts.passwords).tabs_asked();
    let closed: Vec<TabId> = if asked.is_empty() {
        Vec::new()
    } else {
        inner
            .host
            .session
            .with_session(|session| asked.into_iter().filter(|id| session.tab(*id).is_none()).collect())
    };
    let password_prompt = {
        let mut asks = lock(&inner.prompts.passwords);
        asks.showing_tab(active_tab, &closed);
        if let Some((id, path)) = wants
            && !asks.declined(id)
        {
            asks.ask(passwords::Target::Tab(id), &path);
        }
        asks.prompt()
    };
    // A password goes with its tab: one for a file no tab has open now is let go, and overwritten.
    if !inner.keys.is_empty() {
        let open: Vec<(u64, u64)> = inner.host.session.with_session(|session| {
            session
                .tabs()
                .iter()
                .filter_map(|tab| session.own_stamp(tab.id))
                .map(|stamp| (stamp.device, stamp.inode))
                .collect()
        });
        inner.keys.keep_only(&open);
    }
    let reading_engine = inner.panes.reading_engine(&inner.engine);
    let note = recto_app::NoteSources {
        open_failure: lock(&inner.open_failure).clone(),
        save_failed: inner.saver.has_failed(),
        problem,
        session: lock(&inner.host.notice).as_deref().and_then(record::notices::words).map(str::to_string),
        compare: facts.compare_note.take(),
        engine: reading_engine.note(),
    }
    .note();
    let selection_notice = recto_app::SelectionSources {
        copy: reading_engine.selection_notice().map(|notice| recto_app::CopyNotice {
            text: notice.text,
            offers_copy_anyway: notice.offers_copy_anyway,
            fades: notice.fades,
        }),
        printing: inner.printing.selection_notice(),
        tool: inner.tool.selection_notice(),
        action: lock(&inner.action_notice).clone(),
        update: inner.updates.notice().map(|notice| (notice.text, notice.fades)),
    }
    .notice();
    let engine_notes: Vec<Option<String>> = if comparing {
        (0..facts.panes.len())
            .map(|pane| inner.panes.engine(&inner.engine, pane).and_then(|engine| engine.note()))
            .collect()
    } else {
        Vec::new()
    };
    let model = recto_app::reader_view(
        &facts,
        recto_app::Surfaces {
            paper: look,
            note,
            selection_notice,
            pane_views: recto_app::pane_views(&facts.panes, active, comparing, &engine_notes),
            holding: lock(&inner.holding_tab).is_some(),
            first_pane_empty: synced.panes.first().is_none_or(|pane| pane.tab.is_none()),
            sidebar_width: lock(&inner.app).sidebar_width,
            save: recto_app::SaveFacts {
                off: save.off,
                note: save.note,
                tooltip: save.tooltip,
            },
            print_ready: inner.printing.has_ready(),
        },
    );
    // The sheets that ask something are the bridge's, and ride beside the model. These three assignments go when the saving, linking and passwords modules move to recto-app.
    let mut reader = ReaderView::from(model);
    reader.save_problem = inner.save_ui.problem();
    reader.link_prompt = lock(&inner.prompts.link).clone();
    reader.password_prompt = password_prompt;
    let pane_tocs: Vec<Option<Arc<recto_core::Toc>>> = facts.panes.iter().map(|shown| shown.toc.clone()).collect();
    let recto_app::SidebarParts {
        view: sidebar,
        tabs,
        contents,
        rail_filled,
    } = facts.sidebar;
    let mut published = lock(&inner.published);
    published.pane_tocs = pane_tocs;
    // Open Recent is built again only when its list has moved, or the tabs have, which say which of its files are open: not at every reconcile.
    published.recent_key = Some(facts.recent_key);
    let recent_changed = facts.recent.is_some_and(|recent| {
        let changed = *published.recent != recent;
        if changed {
            published.recent = Arc::new(recent);
        }
        changed
    });
    let changed = recent_changed
        || *published.sidebar != sidebar
        || published.reader != reader
        || published.rail_filled != rail_filled
        || !Arc::ptr_eq(&published.contents, &contents);
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
    let (retired, journal_work) = inner
        .host
        .session
        .with_session(|session| (session.has_retired_journals(), session.has_journal_work()));
    if retired {
        inner.saver.write_soon();
    } else {
        if journal_work {
            inner.saver.edit_made();
        }
        inner.saver.view_changed();
    }
}

#[cfg(test)]
mod tests {
    /// Failure (review round 2 of stage 2): `qa save-from` lets a file whose signatures couldn't be looked at through the signature check, so a crash test can put a file in place over a signed one the save itself would refuse. A signed file and one that won't open are both refused; an unsigned one goes.
    #[test]
    fn save_from_refuses_a_file_it_cant_look_at() {
        use recto_core::save::SignatureFacts;
        assert!(super::save_from_signature_check(Ok(SignatureFacts::default())).is_ok());
        assert_eq!(
            super::save_from_signature_check(Ok(SignatureFacts {
                signed: true,
                ..SignatureFacts::default()
            }))
            .unwrap_err()
            .code,
            "signed_needs_copy"
        );
        let unreadable = recto_core::save::signature_facts_of(std::path::Path::new("/nowhere/at/all.pdf"));
        assert!(unreadable.is_err());
        assert_eq!(super::save_from_signature_check(unreadable).unwrap_err().code, "not_found");
    }

    use super::*;

    /// Failure: an update goes ahead while a document is being saved from the window, so the app quits under the save (SPEC decision 24: a running save finishes first). Each thing in flight is named, and a save is one of them.
    #[test]
    fn a_hand_off_waits_for_a_save() {
        assert!(hand_off_check_parts(0, 0, 0, 0, false).is_empty());
        let busy = hand_off_check_parts(0, 0, 0, 1, false);
        assert_eq!(busy.iter().map(|item| item.code.as_str()).collect::<Vec<_>>(), ["saving"]);
        assert_eq!(hand_off_check_parts(1, 1, 1, 1, true).len(), 5);
    }

    /// Failure: an update quits the app mid-print, because the check only counted running commands, and a print's thread is done before the system's printing has its file (review, 2026-11-08).
    #[test]
    fn a_hand_off_waits_for_a_print() {
        let busy = hand_off_check_parts(0, 0, 0, 0, true);
        assert_eq!(busy.iter().map(|item| item.code.as_str()).collect::<Vec<_>>(), ["printing"]);
    }

    struct TestEngineEvents;
    impl EngineEvents for TestEngineEvents {
        fn ops_ready(&self) {}
        fn note_changed(&self) {}
        fn page_moved(&self) {}
        fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
    }

    /// Failure (review 1 of the slow mode): an arm of the session thread's loop left it early, with `continue`, and skipped what every call ends with. The state's coming in announced nothing it did and left Find's field behind for the window made from it (CI, run 43062872948), and a flush's own changes were never announced. Only these leave early, each for its reason: an open before the state's in, put off whole; a drawn page, which announces itself and makes no view; and the state's coming in, after its tail, before the host's told.
    #[test]
    fn every_call_on_the_session_thread_ends_with_what_every_call_ends_with() {
        let source = include_str!("lib.rs");
        let body = &source[source.find("fn session_thread(").unwrap()..source.find("fn after_call(").unwrap()];
        let body = &body[body.find("match call {").unwrap()..];
        let mut arm = "";
        let mut early = Vec::new();
        let mut tail_before = std::collections::HashSet::new();
        for line in body.lines() {
            let trimmed = line.trim_start();
            if line.len() - trimmed.len() == 12 && trimmed.starts_with("CoreCall::") {
                arm = trimmed.split(['(', ' ']).next().unwrap();
            }
            if trimmed.starts_with("after_call(") {
                tail_before.insert(arm);
            }
            if trimmed == "continue;" {
                early.push(arm);
            }
        }
        assert_eq!(early, ["CoreCall::Restored", "CoreCall::Open", "CoreCall::Page"], "the arms that leave early");
        assert!(
            tail_before.contains("CoreCall::Restored"),
            "the state's coming in ends with the tail before it goes"
        );
    }

    /// Failure: an agent at Control quits the app out from under the person using it, with no prompt, by `recto stop`. In a normal app it's refused, and the words are for whoever's asking.
    #[test]
    fn stop_is_refused_by_a_normal_app() {
        let state = recto_testpdf::TempDir::new("rf-");
        let host = AppHost {
            engine: Engine::new(EngineConfig::default(), Arc::new(TestEngineEvents)),
            session: SessionHost::new(DrawableLoader),
            core: Arc::new(CoreQueue::default()),
            qa: None,
            saver: Arc::new(Saver::new(
                RecordStore::open(&StatePaths::overridden(&state).unwrap(), Flavor::App),
                Arc::new(CoreQueue::default()),
            )),
            running: AtomicU32::new(0),
            handing_off: AtomicBool::new(false),
            notice: Mutex::new(None),
            watchers: Mutex::new(None),
        };
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

    /// Failure (review 1): a release's app library is built without `RECTO_RELEASE` while its installer is, and takes a test's hooks, and nothing checks it. `--build-kind` says which it is, and nothing else answers at once.
    #[test]
    fn the_build_kind_is_said_at_once_and_only_when_asked() {
        let answer: Value = serde_json::from_str(&launch_answers_at_once(vec!["--build-kind".into()]).unwrap()).unwrap();
        assert_eq!(answer["release_build"], recto_update::RELEASE_BUILD);
        assert_eq!(answer["takes_test_hooks"], !recto_update::RELEASE_BUILD);
        assert_eq!(launch_answers_at_once(vec![]), None);
        assert_eq!(launch_answers_at_once(vec!["--qa".into(), "--build-kind".into()]), None);
    }
}
