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
mod panes;
pub mod passwords;
mod printing;
mod qa;
mod saver;
mod saving;
mod sleeper;
mod snapshot_tool;
pub mod updates;
mod watchdog;

pub use menus::{HostAction, MenuEntry, MenuModel, PanelItem, PanelModel, PanelSection};
use qa::QaBridge;
use recto_agent::{AgentError, Command, Condition, ConditionState, DrawableLoader, Event, Level};
use recto_control::StatePaths;
use recto_control::host::{Host, SessionHost, kept_json, store_error};
use recto_control::indexing::Notifier;
use recto_control::record_store::{Flavor, FlushReport, Loaded, RecordStore, notice_for};
use recto_control::server::{Announcer, Limits, Server, Stopper};
use recto_core::{DocFacts, OpenError};
use recto_platform::FileStamp;
use recto_reader::engine::EngineEvents;
use recto_reader::tiles::SavedPlace;
use recto_reader::tiles::{MAX_ZOOM, MIN_ZOOM, Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_render_service::Policy;
use recto_session::HistorySource;
use recto_session::record::{
    self, AppRecord, Appearance, ContentsScroll, FileStatus, FocusRecord, FocusView, Rect, ScreenNow, ScreenRecord, SettingsWindow, WindowRecord, look_at_file,
    place_window,
};
use recto_session::{
    CONTENTS_ROW_HEIGHT, ContentsFollow, ContentsRowChange, ContentsView, DocLoader, DrawNote, FollowMove, Paper, RowKind as SessionRowKind, SessionError,
    TAB_ROW_HEIGHT, TabId, ViewSettings, WARM_PAPER, row_full_text, sidebar_heights_with_note,
};
use saver::Saver;
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
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

/// A contents row as it's kept, with no more in it than it needs. Rows are fetched by the host for what's on screen, so a long outline crosses only as far as it's shown.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ContentsEntry {
    tab: u32,
    text: Box<str>,
    page: Option<u32>,
    depth: u8,
    /// Its entry in the list, and whether it folds and is open.
    entry: u32,
    expandable: bool,
    expanded: bool,
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
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
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
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
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
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
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

/// What the Save icon says when it's off for a signed document.
const SAVE_OFF_TOOLTIP: &str = "This document is signed, and saving this change would take the signature off, so Save is off. Save As makes an unsigned copy.";

/// **Whether a Save over this file would be refused for its signature,** so Save is off and Save As's copy is unsigned (SPEC decision 27): a person's signature and a change that has to be written anew, or a certification or lock and any change. Usage rights alone don't. The same rule `recto_core::save::save` refuses by.
fn save_needs_copy(facts: recto_core::save::SignatureFacts, needs_rewrite: bool) -> bool {
    facts.has_persons_signature() && (needs_rewrite || facts.certified || facts.field_locked)
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
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
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
        // The sidebar hears of generated contents as watchers do, and looks on the session thread.
        let core = Arc::clone(&self.core);
        self.session.attach(Arc::new(move |events: Vec<Event>| {
            if events.iter().any(|event| event.name == "contents_generated") {
                core.push(CoreCall::GeneratedFinished);
            }
            // The Finder's news, for the sidebar's list and a step waiting for hits.
            if events.iter().any(|event| matches!(event.name, "find_changed" | "find_finished")) {
                core.push(CoreCall::Find(find::FindCall::News));
            }
            notify(events);
        }));
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
            let row_json = |row: &palette::PaletteRow| {
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
                        palette::close(&core.inner);
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
            Command::UpdateStatus {} | Command::UpdateCheck {} | Command::UpdateInstall {} | Command::Settings { .. }
        ) {
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

/// How many calls can wait for the session thread before notes of pages drawn stop being added.
const MAX_QUEUED_CALLS: usize = 4096;
/// Files being read at once off the session thread. Each is a thread that a dead volume can hold for as long as it likes, so there's a limit, and past it a tab or file doesn't open and says so.
const MAX_FILE_READS: usize = 4;

/// What the session thread is asked to do, in the order the user asked.
#[derive(Debug)]
enum CoreCall {
    Reconcile,
    Tap(TabId),
    /// File > Open Recent > Clear Menu.
    ClearRecent,
    /// A password typed for a protected file has been tried: what it opened, or that it didn't.
    Unlocked(passwords::Ask, Result<DocFacts, OpenError>, Option<FileStamp>, Arc<recto_core::Password>),
    /// The app's come to the front: the files showing are looked at, as another app may have changed them.
    LookAtShown,
    /// A jump, which Back comes back from: a contents entry, a link, or the go-to field, to a page and how far down it.
    Jump(TabId, u32, f64),
    /// What the go-to field had typed in it, for the tab showing.
    JumpTyped(TabId, String),
    /// Back, or forward, in the tab showing's trail: ⌘[ and ⌘], a swipe, or a link that says so.
    Step(bool),
    Open(PathBuf),
    /// The sidebar hidden, or not.
    SetSidebarHidden(bool),
    /// A tab laid out another way: the pane being read's when it was chosen, or the active tab's.
    SetSpread(Option<TabId>, record::Spread),
    /// ⌘-click or ⌃-click on a tab: into a comparison, or out of it.
    CompareClick(TabId),
    /// The reader's moved into another pane.
    FocusPane(TabId),
    /// Synced scroll, or separate.
    SetSynced(bool),
    /// The panel's Rotate, by the owner.
    Rotate(i32),
    /// Edit > Undo or Redo, by the owner: true for Undo.
    UndoOrRedo(bool),
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
    /// What a save off this thread did, for watchers.
    Events(Vec<Event>),
    /// Closes a tab, by the owner: with its edits thrown away first if they said Don't Save.
    CloseTab(TabId, bool),
    /// The close prompt's answer, for the tab it asked about.
    CloseAnswered(TabId, saving::CloseChoice),
    /// A save from the close prompt finished: the tab closes, letting go of steps that were undone, as the prompt said.
    CloseAfterSave(TabId),
    /// The reader tapped the note over a tab's contents.
    ContentsAction(TabId, recto_session::ContentsAction),
    /// Generated contents were made, or failed: tabs waiting for them look.
    GeneratedFinished,
    /// What a look off this thread found of a tab's generated contents, for the version of the file it looked at.
    GeneratedView(TabId, recto_session::GeneratedView, Option<FileStamp>),
    /// A contents section's chevron: fold it or open it.
    ToggleSection(TabId, u32),
    /// The Find field, its list, and ⌘G.
    Find(find::FindCall),
    /// Into edit mode or out, by the owner.
    SetEditMode(bool),
}

impl CoreCall {
    /// **Whether a hand-off's freeze holds it back** (review C8): what a person does, which past the hand-off's last look wouldn't come back, as what's on disk is what does. Every variant decides here, with no default, so a new one can't slip through by being forgotten.
    ///
    /// What isn't held back is the end of something already under way, or the system's or a worker's news. Dropping one of those would leave something waiting for good, or the session not answering, if the freeze lifts again: a quit that stays, or a hand-off that fails.
    fn is_user_action(&self) -> bool {
        match self {
            Self::Tap(_)
            | Self::ClearRecent
            | Self::Jump(..)
            | Self::JumpTyped(..)
            | Self::Step(_)
            | Self::Open(_)
            | Self::SetSidebarHidden(_)
            | Self::SetSpread(..)
            | Self::CompareClick(_)
            | Self::FocusPane(_)
            | Self::SetSynced(_)
            | Self::Rotate(_)
            | Self::UndoOrRedo(_)
            | Self::SetPaper(_)
            | Self::ToggleAgentIcon
            | Self::ClearEditHistory
            // Closing a tab, by File > Close or the close prompt's answer: a save or a throwing away of edits that the state already written wouldn't know of.
            | Self::CloseTab(..)
            | Self::CloseAnswered(..)
            // The note over the contents, and a section's chevron: both kept in the record.
            | Self::ContentsAction(..)
            | Self::ToggleSection(..)
            | Self::SetEditMode(_) => true,
            Self::Find(call) => call.is_user_action(),
            // A password tried on its own thread: dropped, its prompt would wait for good. Its file's read as a `Read` is.
            Self::Unlocked(..)
            // The end of a save the close prompt started, which a hand-off waits for: dropped, a quit that stays would leave the saved tab open.
            | Self::CloseAfterSave(_)
            | Self::Reconcile
            | Self::LookAtShown
            | Self::SleepTabs(..)
            | Self::MemoryPressure(_)
            | Self::WokenFileIsOther(..)
            | Self::FileLength(..)
            | Self::LookAgain(_)
            | Self::Page(..)
            | Self::Read(_)
            | Self::Flush(_)
            | Self::Restored(_)
            | Self::RestorePatienceOver
            | Self::Events(_)
            | Self::GeneratedFinished
            | Self::GeneratedView(..) => false,
        }
    }
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
        if self.frozen.load(Ordering::SeqCst) && call.is_user_action() {
            return;
        }
        let mut calls = lock(&self.calls);
        if let CoreCall::Page(page, failure, tab) = &call {
            // Only notes of what was drawn can go, as they're told on to watchers and no more. Many tiles of a page are one note, which is most of them, so this is hard to reach. With the session thread held up for some reason this stops them growing without end. Anything a person did always gets in.
            if calls.len() >= MAX_QUEUED_CALLS
                || matches!(calls.back(), Some(CoreCall::Page(last_page, last_failure, last_tab)) if last_page == page && last_failure == failure && last_tab == tab)
            {
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
    /// A comparison's panes past the first, and what the UI thread needs to keep them in step.
    panes: panes::Panes,
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
    /// Why something the reader chose couldn't be done, or what a save has to say, as a notice that goes by itself.
    action_notice: Mutex<Option<String>>,
    /// Passwords typed for protected files that are open, shared with every page view's workers. Memory only.
    keys: Arc<recto_reader::engine::Keyring>,
    /// Passwords being asked for, in turn, on the system's sheet, and what each file's tries have come to.
    password_asks: Mutex<passwords::Asks>,
    /// The tab showing when the window was last brought up to date: a notice is about what was on screen, so it goes with a change of tab.
    notice_tab: AtomicU32,
    /// A web or email address a click asked to open, waiting for the person's answer on the sheet.
    link_prompt: Mutex<Option<linking::LinkPrompt>>,
    /// Link sheets asked so far, which numbers each.
    link_prompts_made: AtomicU64,
    /// Saves from the window: how many run, and what one that failed says.
    save_ui: saving::SaveUi,
    /// The snapshot tool: on or off, the box being drawn, and what's on its way to the clipboard.
    tool: snapshot_tool::Tool,
    chrome: chrome::Chrome,
    /// A print being prepared, or waiting for the host to hand to the system.
    printing: printing::Printing,
    /// ⌘K's palette, on the UI thread.
    palette: Mutex<palette::State>,
    /// The Find field as the session thread last set it, for the UI thread.
    find_field: Mutex<find::Field>,
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

/// What a launch was asked for.
#[derive(Debug, Default, PartialEq)]
struct LaunchArgs {
    qa: bool,
    memory_limit_mib: Option<u64>,
    time_limit: Option<Duration>,
    /// The process a quiet run ends with, as its test or script: see [`qa::end_with_starter`].
    owner_pid: Option<u32>,
    open: Vec<PathBuf>,
}

fn parse_args(args: &[String]) -> Result<LaunchArgs, String> {
    let mut parsed = LaunchArgs::default();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--qa" => parsed.qa = true,
            "--qa-memory-limit-mib" => {
                parsed.memory_limit_mib = Some(
                    words
                        .next()
                        .and_then(|value| value.parse().ok())
                        .ok_or("--qa-memory-limit-mib takes a number.")?,
                )
            }
            "--qa-time-limit-ms" => {
                parsed.time_limit = Some(Duration::from_millis(
                    words.next().and_then(|value| value.parse().ok()).ok_or("--qa-time-limit-ms takes a number.")?,
                ))
            }
            "--qa-owner-pid" => {
                parsed.owner_pid = Some(
                    words
                        .next()
                        .and_then(|value| value.parse().ok())
                        .filter(|pid| *pid > 1)
                        .ok_or("--qa-owner-pid takes a process number.")?,
                )
            }
            // What macOS adds to a launch from elsewhere, and a file to open.
            other if other.starts_with("-psn_") || other.starts_with("-NS") || other.starts_with("-Apple") => {
                let _ = words.next();
            }
            other if other.starts_with('-') => return Err(format!("`{}` isn't an option of this app.", cleaned(other))),
            other => parsed.open.push(PathBuf::from(other)),
        }
    }
    if !parsed.qa && (parsed.memory_limit_mib.is_some() || parsed.time_limit.is_some() || parsed.owner_pid.is_some()) {
        return Err("Those options are only for a QA run.".into());
    }
    Ok(parsed)
}

/// Whether this launch is a quiet run, which is decided before AppKit does anything. The same parser as the rest of the launch, so the two can't disagree. A launch that won't parse ends straight away, and shows nothing on the way.
#[uniffi::export]
pub fn launch_is_quiet(args: Vec<String>) -> bool {
    parse_args(&args).map_or(true, |parsed| parsed.qa)
}

/// What a launch answers and ends with, before AppKit does anything: `--build-kind`, which says whether this was built as a release and takes a test's hooks, for `app/release-zip.sh` to check the app's own library and not only the installer's. Nothing for any other launch.
#[uniffi::export]
pub fn launch_answers_at_once(args: Vec<String>) -> Option<String> {
    (args == ["--build-kind"]).then(|| json!({ "release_build": recto_update::RELEASE_BUILD, "takes_test_hooks": !recto_update::RELEASE_BUILD }).to_string())
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
        });
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
            }),
            restored: AtomicBool::new(false),
            waiting_server: Mutex::new(server),
            holding_tab: Mutex::new(None),
            hand_off_state: AtomicU8::new(HandOffState::Idle as u8),
            qa_hold_reads: AtomicBool::new(false),
            reads_running: AtomicU32::new(0),
            sheets: AtomicU32::new(0),
            last_input_ms: AtomicU64::new(0),
            first_shown: Mutex::new(None),
            published: Mutex::new(Published {
                recent: Arc::new(Vec::new()),
                recent_key: None,
                pane_tocs: Vec::new(),
                contents: Arc::new(Vec::new()),
                rail_filled: 0,
                tabs: Arc::new(Vec::new()),
                sidebar: Arc::new(SidebarView {
                    tabs_version: 0,
                    tab_count: 0,
                    active_index: None,
                    contents_version: 0,
                    contents_count: 0,
                    contents_changed_from: 0,
                    contents_removed: 0,
                    contents_inserted: 0,
                    contents_place_shift: 0,
                    contents_current: None,
                    toc_open: false,
                    edited: false,
                    undone_owner_steps: false,
                    contents_note: None,
                }),
                reader: ReaderView {
                    panes: Vec::new(),
                    compare_synced: false,
                    home: true,
                    note: None,
                    sidebar_hidden: false,
                    layout: String::new(),
                    edited: false,
                    can_undo: false,
                    can_redo: false,
                    edit_mode: false,
                    can_rotate: false,
                    place: String::new(),
                    paper: Paper::White.name().to_string(),
                    paper_rgb: PaperKind::White.rgb(),
                    page_tint: None,
                    agent_icon_hidden: false,
                    sidebar_width: record::SIDEBAR_WIDTH,
                    highlight_dark_colour: false,
                    highlight_lighten: false,
                    selection_notice: None,
                    saving: false,
                    save_tooltip: String::new(),
                    save_problem: None,
                    save_as_note: None,
                    shown_tab: 0,
                    document_name: String::new(),
                    document_folder: String::new(),
                    save_button: false,
                    save_enabled: false,
                    print_ready: false,
                    can_go_back: false,
                    can_go_forward: false,
                    link_prompt: None,
                    page_text: String::new(),
                    password_prompt: None,
                },
            }),
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
            chrome: chrome::Chrome::default(),
            printing: printing::Printing::default(),
            keys,
            password_asks: Mutex::new(passwords::Asks::default()),
            notice_tab: AtomicU32::new(0),
            link_prompt: Mutex::new(None),
            link_prompts_made: AtomicU64::new(0),
            save_ui: saving::SaveUi::new(Arc::clone(&save_step)),
            palette: Mutex::new(palette::State::default()),
            find_field: Mutex::new(find::Field::default()),
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
            qa::end_with_starter(args.time_limit.unwrap_or(qa::QUIET_RUN_LIMIT), args.owner_pid, state);
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

    /// Rows closed or opened. Says how many points to move the scroll by with no animation, or nothing if the rows are to slide.
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
        let reader = lock(&self.inner.published).reader.clone();
        let recent = Arc::clone(&lock(&self.inner.published).recent);
        let items: Vec<menus::RecentItem> = recent
            .iter()
            .map(|file| menus::RecentItem {
                key: &file.key,
                name: &file.name,
            })
            .collect();
        let mut bar = menus::menu_bar_with(
            ViewSettings {
                paper: Paper::from_name(&reader.paper),
                sidebar_hidden: reader.sidebar_hidden,
                agent_icon_hidden: reader.agent_icon_hidden,
            },
            record::Spread::from_name(&reader.layout),
            &self.inner.updates.menu_item().title,
            &items,
        );
        bar.push(snapshot_tool::tools_menu(self.inner.tool.is_on()));
        bar
    }

    /// **The password sheet was answered:** for the prompt with this id, with what was typed, or nothing for Cancel. The answer goes to that prompt's file, whatever's waiting by now, and none if it's gone. It's tried on a thread of its own, as reading a file can wait on its disk, and kept nowhere but the open tab's keyring. The `String` UniFFI made is overwritten here, so Rust holds one copy.
    pub fn password_answered(&self, id: u64, password: Option<String>) {
        let mut asks = lock(&self.inner.password_asks);
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
            lock(&self.inner.password_asks).failed(id);
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
        snapshot_tool::add_to_panel(&mut panel, self.inner.tool.is_on(), reader.edit_mode);
        panel
    }

    /// Does what a menu item, a panel button or a key does, and says what's left for the host. The same methods as `recto view`.
    pub fn perform(&self, id: String) -> HostAction {
        // Past a hand-off's last look, what's done here wouldn't come back.
        if self.inner.core.frozen.load(Ordering::SeqCst) {
            return HostAction::None;
        }
        // Features that answer ids of their own: the snapshot tool, and printing.
        if snapshot_tool::perform(&self.inner, &id) || printing::perform(&self.inner, &id) {
            return HostAction::None;
        }
        let Some(action) = menus::action(&id) else { return HostAction::None };
        match action {
            menus::Action::Host(host) => return host,
            menus::Action::Copy { anyway } => self.inner.panes.reading_engine(&self.inner.engine).copy_selection(anyway),
            menus::Action::SelectAll => self.inner.panes.reading_engine(&self.inner.engine).select_all(),
            menus::Action::SetSynced(on) => self.inner.core.push(CoreCall::SetSynced(on)),
            menus::Action::PaletteOpen => {
                // The palette is in the sidebar, so ⌘K shows it.
                if lock(&self.inner.published).reader.sidebar_hidden {
                    self.inner.core.push(CoreCall::SetSidebarHidden(false));
                }
                palette::open(&self.inner);
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
        palette::typed(&self.inner, &text);
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
        palette::close(&self.inner);
        self.inner.listener.model_changed();
    }

    /// Whether a menu item can be chosen now. Asked when a menu opens and on its key. By the item's rule in `menus`: an id without one can't be chosen.
    pub fn menu_item_enabled(&self, id: String) -> bool {
        use menus::Enable;
        let Some(rule) = menus::enable_rule(&id) else { return false };
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
            Enable::SaveAsOrClose => reader(|reader| !reader.home && !reader.saving),
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
        saving::panel_for(reader.shown_tab, &reader.document_name, &reader.document_folder, reader.save_as_note)
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
        )
    }

    /// The close prompt's answer. Save closes once it's saved, and not if the save fails, which says why. Don't Save throws the edits away and closes. Save As opens its sheet, and the tab stays.
    pub fn close_answered(&self, tab: u32, choice: saving::CloseChoice) -> HostAction {
        if tab == 0 {
            return HostAction::None;
        }
        if choice == saving::CloseChoice::SaveAs {
            return HostAction::ShowSavePanel;
        }
        self.inner.core.push(CoreCall::CloseAnswered(TabId(tab), choice));
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
        self.inner.core.push(CoreCall::MemoryPressure(critical));
    }

    /// What a right-click on a page offers.
    /// What a right-click on a pane offers: its own layout ticked, so a pane that isn't being read shows its own.
    pub fn context_menu(&self, pane: u32) -> Vec<MenuEntry> {
        let reader = lock(&self.inner.published).reader.clone();
        let layout = reader.panes.get(pane as usize).map_or(reader.layout.as_str(), |view| view.layout.as_str());
        menus::context_menu(record::Spread::from_name(layout), (!reader.panes.is_empty()).then_some(reader.compare_synced))
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
        let mut asked = lock(&self.inner.link_prompt);
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
        menus::zoom_label(magnification, fit)
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
        for engine in self.inner.panes.engines(&self.inner.engine) {
            engine.window_opened();
        }
        lock(&self.inner.app).window_closed = false;
        self.inner.saver.view_changed();
    }

    /// Whether this launch opens a window. Any launch does but an update's arrival, which leaves things as they were: a window closed stays closed, rather than appearing behind whatever the user's doing.
    pub fn window_should_open(&self) -> bool {
        !(lock(&self.inner.app).window_closed && self.inner.updates.reopened_by_update())
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
            if let Some(delay) = qa::restore_delay(inner.qa.is_some()) {
                std::thread::sleep(delay);
            }
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
            inner.listener.quit_answer(if fine || unsaved.is_empty() {
                QuitAnswer {
                    go: true,
                    title: String::new(),
                    message: String::new(),
                    quit_button: String::new(),
                    stay_button: String::new(),
                }
            } else {
                QuitAnswer {
                    go: false,
                    title: "Your unsaved edits couldn't be kept".into(),
                    message: "RectoPDF couldn't write them to the state folder. If you quit now, they'll be lost.".into(),
                    quit_button: "Quit Anyway".into(),
                    stay_button: "Stay".into(),
                }
            });
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
                    app.window_closed = saved.window_closed;
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
                            lock(&inner.password_asks).failed(ask.id);
                            *lock(&inner.open_failure) = Some(passwords::CANT_KEEP.to_string());
                        }
                        Some(identity) => {
                            lock(&inner.password_asks).opened(ask.id);
                            inner.keys.set(identity, password);
                            for engine in inner.panes.engines(&inner.engine) {
                                engine.key_given(&ask.path);
                            }
                            match ask.target {
                                passwords::Target::File => match host.session.with_session(|session| session.open_unlocked(&ask.path, facts, stamp)) {
                                    Ok(id) => events.push(Event::new("tab_opened", json!({ "tab": id.0 }))),
                                    Err(error) => *lock(&inner.open_failure) = Some(error.message().to_string()),
                                },
                                passwords::Target::Tab(id) => host.session.with_session(|session| session.unlock(id, facts, stamp)),
                            }
                        }
                    }
                }
                Err(OpenError::PasswordNeeded) => {
                    if lock(&inner.password_asks).wrong(ask.id) == passwords::Wrong::Stopped {
                        *lock(&inner.action_notice) = Some(passwords::stopped_asking());
                    }
                }
                Err(error) => {
                    lock(&inner.password_asks).failed(ask.id);
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
            CoreCall::Jump(id, page, within) => {
                // Where the reader is, as the page view has it now, is what Back comes back to. `jump` clamps: a contents entry can't send the reader off the end.
                inner.take_places();
                if let Ok(event) = host.session.with_session(|session| {
                    session
                        .jump(id, page, within)
                        .map(|()| recto_agent::page_changed(session, id, session.tab(id).map_or(page, |tab| tab.page) + 1))
                }) {
                    events.push(event);
                }
            }
            CoreCall::JumpTyped(id, typed) => {
                inner.take_places();
                let went = host.session.with_session(|session| {
                    let page = session.page_for_typed(id, &typed)?;
                    session.jump(id, page, 0.0).ok()?;
                    Some(recto_agent::page_changed(session, id, page + 1))
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
                    moved
                        .ok()
                        .filter(|moved| *moved)
                        .and_then(|_| session.tab(id).map(|tab| recto_agent::page_changed(session, id, tab.page + 1)))
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
                lock(&inner.password_asks).opened_again(&[Some(path.clone()), own].into_iter().flatten().collect::<Vec<_>>(), tab);
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
                saving::CloseChoice::Save => {
                    if let Some(inner) = inner.saver.inner() {
                        saving::start_save(&inner, id, None, true);
                    }
                }
                saving::CloseChoice::DontSave => close_tab(host, id, true, &mut events),
                saving::CloseChoice::Cancel | saving::CloseChoice::SaveAs => {}
            },
            CoreCall::CloseAfterSave(id) => {
                // An edit that came in between the save and now isn't in the file: the tab stays, and says so, rather than lose it (review).
                let before = host.session.with_session(|session| session.active());
                match host
                    .session
                    .with_session(|session| session.close_after_save(id).map(|closed| (closed, session.active())))
                {
                    Ok((false, _)) => *lock(&inner.action_notice) = Some(saving::SAVED_NOT_CLOSED.to_string()),
                    Ok((true, now)) => {
                        events.push(Event::new("tab_closed", json!({ "tab": id.0 })));
                        if now != before
                            && let Some(active) = now
                        {
                            events.push(Event::new("tab_activated", json!({ "tab": active.0 })));
                        }
                    }
                    Err(_) => {}
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
                        if host.session.with_session(|session| session.focus_pane(id)) {
                            events.push(Event::new("tab_activated", json!({ "tab": id.0 })));
                        }
                    }
                }
            }
            CoreCall::SetSynced(on) => {
                if host.session.with_session(|session| session.set_compare_synced(on)) {
                    events.push(Event::new("view_changed", json!({ "sync": if on { "on" } else { "off" } })));
                }
            }
            CoreCall::SetSpread(tab, spread) => host.session.with_session(|session| {
                if let Some(id) = tab.filter(|tab| session.tab(*tab).is_some()).or(session.active())
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
                events.push(Event::new(
                    if failure.is_some() { "page_failed" } else { "page_drawn" },
                    json!({ "tab": tab.0, "page": page + 1 }),
                ));
                // Nothing the sidebar or the reader shows comes from this. Going round for it would rebuild the model per tile drawn.
                if let Some(announcer) = lock(&inner.announcer).as_ref() {
                    announcer.announce(events);
                }
                continue;
            }
        }
        generated_contents::look_if_chosen(inner);
        find::keep_up(inner);
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
    /// A ⌘-click on the first tab, with the second, cold, being read for it: the tab joining, or the pane read next.
    Compare(TabId, TabId),
    /// A cold tab in a comparison's pane, which the reader's moved into.
    Pane(TabId),
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
    /// It was read with the password the person typed for it, from the keyring.
    unlocked: bool,
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
                if host.session.with_session(|session| session.focus_pane(tab)) {
                    events.push(Event::new("tab_activated", json!({ "tab": tab.0 })));
                }
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
                    events.push(Event::new("tab_opened", json!({ "tab": id.0 })));
                }
                // A protected file asks for its password, on the system's sheet, rather than only saying it needs one. After any other file asking, each with its own answer.
                Err(SessionError::Open(OpenError::PasswordNeeded)) => {
                    lock(&inner.password_asks).ask(passwords::Target::File, &path);
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
        Ok(_) => {
            let panes = inner.host.session.with_session(|session| session.panes());
            events.push(Event::new(
                "view_changed",
                json!({ "comparison": panes.iter().map(|tab| tab.0).collect::<Vec<_>>() }),
            ));
        }
        Err(error) => *lock(&inner.action_notice) = Some(error.message().to_string()),
    }
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
fn place_shift(
    before: &[ContentsEntry],
    after: &[ContentsEntry],
    (from, removed, inserted): (u32, u32, u32),
    place_before: Option<u32>,
    place_after: Option<u32>,
) -> i32 {
    let (from, removed, inserted) = (from as usize, removed as usize, inserted as usize);
    let rows = |count: usize| i32::try_from(count).unwrap_or(i32::MAX);
    let Some(place) = place_after.map(|row| row as usize).filter(|&row| row < after.len()) else {
        return 0;
    };
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
    let end = before[start..]
        .iter()
        .rev()
        .zip(after[start..].iter().rev())
        .take_while(|(one, two)| one == two)
        .count();
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

/// What a pane shows, from the session.
struct PaneSnapshot {
    id: TabId,
    pages: u32,
    page: u32,
    map: PageMap,
    /// Tabs this pane's page view keeps warm besides: pane 0 has the warm tabs off screen, and the others none.
    warm: Vec<u32>,
    problem: Option<String>,
    saved: Option<SavedPlace>,
    spread: record::Spread,
    /// Its file's name, cleaned, for its pane's label.
    title: String,
    /// A jump the page view hasn't taken: where to land, as far down the page as it says.
    jump: Option<recto_session::trail::Stop>,
    /// Its contents and page labels, shared, so a link's hint can say a page by its label without asking the session.
    toc: Option<Arc<recto_core::Toc>>,
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

fn pane_snapshot<L: DocLoader>(session: &recto_session::Session<L>, id: TabId, first: bool, panes: &[TabId]) -> Option<PaneSnapshot> {
    let tab = session.tab(id)?;
    let own = session.path_of(id)?;
    let same_file = session.own_stamp(id).map(sleeper::version_of);
    // A save wrote these pages as they were shown: the page view goes on with its layout, place and selection.
    let follows = session.saved_from(id).map(sleeper::version_of);
    let mut map = match session.page_edits(id) {
        None => PageMap {
            files: vec![own],
            slots: None,
            own: same_file,
            turns: None,
            expected: Vec::new(),
            follows,
        },
        Some(edits) => {
            let mut files = vec![own];
            files.extend(edits.added_sources().iter().map(|added| added.path.to_path_buf()));
            // A page's quarter turns, one byte a page of an edited tab, and none at all while nothing's turned.
            let turns = edits
                .slots()
                .iter()
                .any(|slot| slot.quarter_turns % 4 != 0)
                .then(|| edits.slots().iter().map(|slot| slot.quarter_turns).collect::<Arc<[u8]>>());
            // Pages only turned are the file's own pages in its own order, which is what was drawn already: the same map, so nothing's dropped.
            let own_order = edits.added_sources().is_empty()
                && edits.len() == edits.file_pages()
                && edits.slots().iter().enumerate().all(|(at, slot)| slot.source == 0 && slot.page as usize == at);
            let slots = (!own_order).then(|| edits.slots().iter().map(|slot| (slot.source, slot.page)).collect());
            // What each added PDF was when its pages were added: a page from one that's changed since isn't drawn.
            let mut expected = vec![None];
            expected.extend(edits.added_sources().iter().map(|added| added.stamp));
            PageMap {
                files,
                slots,
                own: same_file,
                turns,
                expected,
                follows,
            }
        }
    };
    // The tab's view turned, as the state record carries it: every page drawn turned on top of what its edits turn, and nothing about the file. One byte a page of a hot or warm tab, and none while it's not turned.
    let view_turns = tab.view_rotation % 4;
    if view_turns != 0 {
        let pages = session.pages_in(id) as usize;
        let turns: Arc<[u8]> = (0..pages)
            .map(|at| (map.turns.as_ref().and_then(|turns| turns.get(at)).copied().unwrap_or(0) + view_turns) % 4)
            .collect();
        map.turns = Some(turns);
    }
    // Pane 0's page view keeps the warm tabs that aren't on screen, as many as the panes leave of the live documents. A tab in another pane is that pane's.
    let warm: Vec<u32> = if first { warm_beside_panes(session.warm_tabs(), panes) } else { Vec::new() };
    let problem = session
        .tab_notice_message(id)
        .map(str::to_string)
        .or_else(|| tab.problem.map(|problem| problem.message().to_string()));
    let saved = session.place_to_show(id).map(|place| SavedPlace {
        page: place.page,
        anchor: (place.anchor_page, place.within),
        x: place.x,
        y: place.y,
        document_height: place.document_height,
        magnification: place.zoom,
        page_size: (place.page_width > 0.0 && place.page_height > 0.0).then_some(recto_reader::layout::PageBox {
            width: place.page_width,
            height: place.page_height,
        }),
    });
    // Edits that can't be read just now can't say what's on the pages, so it's the file that's shown.
    let pages = if session.list_is_asleep(id) { tab.page_count } else { session.pages_in(id) };
    Some(PaneSnapshot {
        id,
        pages,
        page: tab.page,
        map,
        warm,
        problem,
        saved,
        spread: tab.spread,
        title: tab.name.to_string(),
        jump: None,
        toc: session.warm_toc(id),
    })
}

/// The warm tabs off screen that pane 0's page view keeps: most recent first, and only as many as leave the panes and these at [`recto_session::MAX_WARM`] live documents (SPEC §2).
fn warm_beside_panes(warm: impl Iterator<Item = TabId>, panes: &[TabId]) -> Vec<u32> {
    warm.filter(|other| !panes.contains(other))
        .take(recto_session::MAX_WARM.saturating_sub(panes.len()))
        .map(|other| other.0)
        .collect()
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
            *lock(&inner.link_prompt) = Some(linking::prompt_for(inner.link_prompts_made.fetch_add(1, Ordering::SeqCst) + 1, &address));
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
        let left = inner.pictures_of_those_leaving();
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
        inner.write_those_leaving(left);
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
        let before = session.active();
        if session.close(id).is_ok() {
            events.push(Event::new("tab_closed", json!({ "tab": id.0 })));
            if session.active() != before
                && let Some(active) = session.active()
            {
                events.push(Event::new("tab_activated", json!({ "tab": active.0 })));
            }
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
    let (snapshots, plan, sidebar, tabs, contents, rail_filled, view, place) = inner.host.session.with_session(|session| {
        let active = session.active();
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
        let mut snapshots: Vec<PaneSnapshot> = panes
            .iter()
            .enumerate()
            .filter_map(|(pane, id)| pane_snapshot(session, *id, pane == 0, &panes))
            .collect();
        for snapshot in &mut snapshots {
            snapshot.jump = session.take_jump(snapshot.id);
        }
        let plan = (
            session.comparison().cloned(),
            panes.iter().map(|id| session.pages_in(*id)).collect::<Vec<u32>>(),
            session.compare_note(),
            active,
        );
        let model = session.sidebar();
        let mut current = None;
        let mut active_index = None;
        let (mut tab_count, mut contents_row_count) = (0u32, 0u32);
        for row in &model.rows {
            if row.kind == SessionRowKind::Note {
                continue;
            }
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
        let tab_rows = || model.rows.iter().filter(|row| row.kind == SessionRowKind::Tab);
        let compared = |id: TabId| session.comparison().is_some_and(|comparison| comparison.contains(id));
        let same_tabs = held_tabs.len() == tab_count as usize
            && tab_rows().zip(held_tabs.iter()).all(|(row, tab)| {
                row.tab.0 == tab.tab
                    && row.current == tab.current
                    && row.text == tab.text.as_str()
                    && row.notice == tab.notice.as_deref()
                    && compared(row.tab) == tab.in_comparison
            });
        let tabs = if same_tabs {
            held_tabs
        } else {
            Arc::new(
                tab_rows()
                    .map(|row| SidebarRow {
                        kind: RowKind::Tab,
                        tab: row.tab.0,
                        text: row.text.to_string(),
                        page: row.page,
                        depth: row.depth,
                        current: row.current,
                        rail_filled: false,
                        rail_first: false,
                        rail_last: false,
                        notice: row.notice.map(str::to_string),
                        action: None,
                        expandable: false,
                        expanded: false,
                        in_comparison: compared(row.tab),
                    })
                    .collect(),
            )
        };
        let tabs_version = if same_tabs { was.tabs_version } else { was.tabs_version + 1 };
        let same = held.len() == contents_row_count as usize
            && model
                .rows
                .iter()
                .filter(|row| row.kind == SessionRowKind::Contents)
                .zip(held.iter())
                .all(|(row, entry)| {
                    row.tab.0 == entry.tab
                        && row.page == entry.page
                        && row.depth == entry.depth
                        && row.text == entry.text.as_ref()
                        && row.entry == entry.entry
                        && row.expanded == entry.expanded
                        && row.expandable == entry.expandable
                });
        let rail_filled = u32::try_from(model.rail_filled).unwrap_or(u32::MAX);
        let (contents, contents_version, change, contents_place_shift) = if same {
            (
                held,
                was.contents_version,
                (was.contents_changed_from, was.contents_removed, was.contents_inserted),
                was.contents_place_shift,
            )
        } else {
            let list: Arc<Vec<ContentsEntry>> = Arc::new(
                model
                    .rows
                    .iter()
                    .filter(|row| row.kind == SessionRowKind::Contents)
                    .map(|row| ContentsEntry {
                        tab: row.tab.0,
                        text: row.text.into(),
                        page: row.page,
                        depth: row.depth,
                        entry: row.entry,
                        expandable: row.expandable,
                        expanded: row.expanded,
                    })
                    .collect(),
            );
            let change = rows_changed(&held, &list);
            let shift = place_shift(&held, &list, change, was.contents_current, current);
            (list, was.contents_version + 1, change, shift)
        };
        // The note over the contents stays put while they scroll, so it's drawn apart from them.
        let contents_note = model.rows.iter().find(|row| row.kind == SessionRowKind::Note).map(|row| SidebarRow {
            kind: RowKind::Note,
            tab: row.tab.0,
            text: row.text.to_string(),
            page: None,
            depth: 0,
            current: false,
            rail_filled: false,
            rail_first: false,
            rail_last: false,
            notice: row.notice.map(str::to_string),
            action: row.action.map(|action| action.id().to_string()),
            expandable: false,
            expanded: false,
            in_comparison: false,
        });
        let view = SidebarView {
            contents_note,
            tabs_version,
            tab_count,
            active_index,
            contents_count: contents_row_count,
            contents_version,
            contents_changed_from: change.0,
            contents_removed: change.1,
            contents_inserted: change.2,
            contents_place_shift,
            contents_current: current,
            toc_open: model.toc_open,
            edited: model.edited,
            undone_owner_steps: model.undone_owner_steps,
        };
        (snapshots, plan, view, tabs, contents, rail_filled, session.view(), model.place.clone())
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
        for engine in inner.panes.engines(&inner.engine) {
            engine.set_paper(paper);
        }
    }
    let (comparison, pane_pages, compare_note, active) = plan;
    let reading_snapshot = snapshots.iter().find(|shown| Some(shown.id) == active).or(snapshots.first());
    let layout = reading_snapshot.map_or(String::new(), |shown| shown.spread.name().to_string());
    let (edited, (can_undo, can_redo), edit_mode, can_rotate, saving_now, file, needs_rewrite, (can_go_back, can_go_forward), page_text) =
        inner.host.session.with_session(|session| {
            let edit_mode = session.edit_mode();
            match reading_snapshot {
                Some(shown) => {
                    let page = session.tab(shown.id).map_or(0, |tab| tab.page);
                    let page_text = session.page_label(shown.id, page).unwrap_or_else(|| (page + 1).to_string());
                    (
                        session.is_edited(shown.id),
                        session.undo_and_redo(shown.id),
                        edit_mode,
                        session.can_edit_pages(shown.id),
                        session.is_saving(shown.id),
                        session
                            .path_of(shown.id)
                            .zip(session.own_stamp(shown.id))
                            .map(|(path, stamp)| (shown.id, path, stamp)),
                        session.needs_rewrite(shown.id),
                        session.can_go_back_and_forward(shown.id),
                        page_text,
                    )
                }
                None => (false, (false, false), edit_mode, false, false, None, false, (false, false), String::new()),
            }
        });
    let (shown_tab, document_name, document_folder) = file.as_ref().map_or((0, String::new(), String::new()), |(id, path, _)| {
        (
            id.0,
            path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default(),
            path.parent().map(|folder| folder.display().to_string()).unwrap_or_default(),
        )
    });
    // A notice about one document doesn't stay over another: a save's note, seen on the next tab, would read as being about that one.
    if inner.notice_tab.swap(shown_tab, Ordering::SeqCst) != shown_tab {
        lock(&inner.action_notice).take();
        snapshot_tool::tab_came_forward(inner);
    }
    // **Rust decides what a signed file's Save does** (SPEC decision 27): an edit a Save over it would break the signature with turns Save off, and Save As says its copy won't be signed.
    // Asked only with edits to save, off this thread, and the window's brought up to date again once it's known.
    let core_queue = Arc::clone(&inner.core);
    let facts = if edited {
        file.as_ref()
            .and_then(|(id, path, stamp)| inner.save_ui.signature_facts(*id, path, *stamp, move || core_queue.push(CoreCall::Reconcile)))
            .unwrap_or_default()
    } else {
        recto_core::save::SignatureFacts::default()
    };
    let save_off = save_needs_copy(facts, needs_rewrite);
    let save_as_note = save_off.then(|| saving::UNSIGNED_COPY.to_string());
    let problem = reading_snapshot.and_then(|shown| shown.problem.clone());
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
    let count = snapshots.len().max(1);
    if synced.count != count {
        let window_open = !lock(&inner.app).window_closed;
        inner.panes.set_count(&inner.engine, count, paper, window_open);
        synced.count = count;
    }
    synced.panes.resize_with(count, PaneSynced::default);
    if snapshots.is_empty() && synced.panes[0].tab.take().is_some() {
        inner.engine.deactivate();
    }
    let mut panes_changed = false;
    for (pane, snapshot) in snapshots.iter().enumerate() {
        let Some(engine) = inner.panes.engine(&inner.engine, pane) else { continue };
        panes_changed |= synced.panes[pane].tab != Some(snapshot.id);
        show_in_pane(inner, &engine, &mut synced.panes[pane], snapshot);
    }
    let shown_tabs: Vec<TabId> = snapshots.iter().map(|shown| shown.id).collect();
    let reading = active.and_then(|id| shown_tabs.iter().position(|shown| *shown == id)).unwrap_or(0);
    let comparing = comparison.is_some();
    inner.panes.lay_out(shown_tabs, reading, comparison, pane_pages);
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
    let asked = lock(&inner.password_asks).tabs_asked();
    let closed: Vec<TabId> = if asked.is_empty() {
        Vec::new()
    } else {
        inner
            .host
            .session
            .with_session(|session| asked.into_iter().filter(|id| session.tab(*id).is_none()).collect())
    };
    let password_prompt = {
        let mut asks = lock(&inner.password_asks);
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
    let session_notice = lock(&inner.host.notice).as_deref().and_then(record::notices::words).map(str::to_string);
    let save_failed = inner.saver.has_failed().then(|| record::notices::SAVE_FAILED.to_string());
    let reading_engine = inner.panes.reading_engine(&inner.engine);
    let note = lock(&inner.open_failure)
        .clone()
        .or(save_failed)
        .or(problem)
        .or(session_notice)
        .or(compare_note)
        .or_else(|| reading_engine.note());
    let holding = lock(&inner.holding_tab).is_some();
    let sidebar_width = lock(&inner.app).sidebar_width;
    let highlight = paper.highlight();
    let selection_notice = reading_engine
        .selection_notice()
        .map(|notice| SelectionNotice {
            text: notice.text,
            action_title: notice.offers_copy_anyway.then(|| "Copy anyway".to_string()),
            action_id: notice.offers_copy_anyway.then(|| "edit.copy_anyway".to_string()),
            fades: notice.fades,
        })
        .or_else(|| inner.printing.selection_notice())
        .or_else(|| inner.tool.selection_notice())
        .or_else(|| {
            lock(&inner.action_notice).clone().map(|text| SelectionNotice {
                text,
                action_title: None,
                action_id: None,
                fades: true,
            })
        })
        .or_else(|| {
            inner.updates.notice().map(|notice| SelectionNotice {
                text: notice.text,
                action_title: None,
                action_id: None,
                fades: notice.fades,
            })
        });
    let pane_views: Vec<PaneView> = if comparing {
        snapshots
            .iter()
            .enumerate()
            .map(|(pane, shown)| PaneView {
                tab: shown.id.0,
                title: shown.title.clone(),
                reading: Some(shown.id) == active,
                layout: shown.spread.name().to_string(),
                note: shown
                    .problem
                    .clone()
                    .or_else(|| inner.panes.engine(&inner.engine, pane).and_then(|engine| engine.note())),
            })
            .collect()
    } else {
        Vec::new()
    };
    let compare_synced = inner
        .host
        .session
        .with_session(|session| session.comparison().is_some_and(|comparison| comparison.synced));
    let reader = ReaderView {
        panes: pane_views,
        compare_synced,
        home: synced.panes.first().is_none_or(|pane| pane.tab.is_none()) && !holding,
        note,
        sidebar_hidden: view.sidebar_hidden,
        layout,
        edited,
        can_undo,
        can_redo,
        edit_mode,
        can_rotate,
        place,
        paper: shown.name().to_string(),
        paper_rgb: paper.rgb(),
        page_tint,
        agent_icon_hidden: view.agent_icon_hidden,
        sidebar_width,
        highlight_dark_colour: highlight.dark_colour,
        highlight_lighten: highlight.lighten,
        selection_notice,
        saving: saving_now,
        save_tooltip: if save_off { SAVE_OFF_TOOLTIP.to_string() } else { inner.save_ui.tooltip() },
        save_problem: inner.save_ui.problem(),
        save_as_note,
        shown_tab,
        document_name,
        document_folder,
        save_button: shown_tab != 0 && (edited || saving_now),
        save_enabled: !save_off && !saving_now,
        print_ready: inner.printing.has_ready(),
        can_go_back,
        can_go_forward,
        link_prompt: lock(&inner.link_prompt).clone(),
        page_text,
        password_prompt,
    };
    // Open Recent is built again only when its list has moved, or the tabs have, which say which of its files are open: not at every reconcile.
    let recent_key = (inner.host.session.with_session(|session| session.recent_version()), sidebar.tabs_version);
    let recent = (lock(&inner.published).recent_key != Some(recent_key)).then(|| {
        inner
            .host
            .session
            .with_session(|session| session.recent_files())
            .into_iter()
            .map(|file| RecentFile {
                key: menus::recent_key(&file.path),
                name: recto_core::clean_line(&file.name),
                folder: file.path.parent().map(|folder| folder.display().to_string()).unwrap_or_default(),
                path: file.path.to_string_lossy().into_owned(),
                open: file.open.is_some(),
            })
            .collect::<Vec<RecentFile>>()
    });
    let pane_tocs: Vec<Option<Arc<recto_core::Toc>>> = snapshots.iter().map(|shown| shown.toc.clone()).collect();
    let mut published = lock(&inner.published);
    published.pane_tocs = pane_tocs;
    published.recent_key = Some(recent_key);
    let recent_changed = recent.is_some_and(|recent| {
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

    /// Failure: rows opening as the reader enters a chapter jump the list under a smooth drift, because the host wasn't told which went and which came. It must say exactly, a chapter closing and the next opening together included, and a change of nothing says nothing.
    #[test]
    fn opening_and_closing_rows_says_what_changed() {
        let row = |text: &str| ContentsEntry {
            tab: 1,
            text: text.into(),
            page: Some(1),
            depth: 0,
            entry: 0,
            expandable: false,
            expanded: false,
        };
        let list = |names: &[&str]| names.iter().map(|name| row(name)).collect::<Vec<_>>();
        let before = list(&["1", "2", "3"]);
        assert_eq!(rows_changed(&before, &list(&["1", "2", "2.1", "2.2", "3"])), (2, 0, 2));
        assert_eq!(rows_changed(&list(&["1", "2", "2.1", "2.2", "3"]), &before), (2, 2, 0));
        assert_eq!(
            rows_changed(&list(&["1", "2", "2.1", "3", "4"]), &list(&["1", "2", "3", "3.1", "4"])),
            (2, 2, 2),
            "2.1 and 3 go, and 3 and 3.1 come"
        );
        assert_eq!(rows_changed(&before, &before), (0, 0, 0));
        assert_eq!(rows_changed(&[], &before), (0, 0, 3));
        assert_eq!(rows_changed(&before, &[]), (0, 3, 0));
    }

    /// Failure: a chapter closes as the reader leaves it, the place drops up the list by the rows that went, and the contents make a second move straight after the first to bring it back. How far the place was carried has to come out right wherever it starts and ends.
    #[test]
    fn a_row_change_says_how_far_it_carried_the_place() {
        let row = |text: &str| ContentsEntry {
            tab: 1,
            text: text.into(),
            page: Some(1),
            depth: u8::try_from(text.matches('.').count()).unwrap(),
            entry: 0,
            expandable: false,
            expanded: false,
        };
        let list = |names: &[&str]| names.iter().map(|name| row(name)).collect::<Vec<_>>();
        let shift = |one: &[&str], place_before: u32, two: &[&str], place_after: u32| {
            let (before, after) = (list(one), list(two));
            place_shift(&before, &after, rows_changed(&before, &after), Some(place_before), Some(place_after))
        };
        let in_two = ["1", "2", "2.1", "2.2", "3", "4"];
        let in_three = ["1", "2", "3", "3.1", "4"];
        assert_eq!(shift(&in_two, 3, &in_three, 2), -2, "forward: 3 was listed, two rows further down");
        assert_eq!(shift(&in_two, 3, &in_three, 3), -2, "forward, to a section that wasn't listed: its chapter was");
        assert_eq!(
            shift(&in_three, 2, &in_two, 3),
            2,
            "back: 2.2 wasn't listed, but 3, the row it came from, still is"
        );
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
        queue.push(CoreCall::Jump(TabId(1), 2, 0.0));
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

    /// One of every call, and the find calls one of each, with its place in the list. `place` names every variant with no default, so a new one doesn't compile here until it has a place, and the test below fails until it's sampled.
    fn every_call() -> Vec<CoreCall> {
        let tab = TabId(1);
        let stamp = recto_platform::FileStamp {
            size: 1,
            mtime_ns: 1,
            device: 1,
            inode: 1,
        };
        let ask = passwords::Ask {
            id: 1,
            target: passwords::Target::File,
            path: PathBuf::from("/d/a.pdf"),
            wrong: 0,
            trying: true,
        };
        let read = FileRead {
            what: Reading::Tab(tab),
            loaded: Err(recto_core::OpenError::NotFound),
            stamp: None,
            status: None,
            history: None,
            unlocked: false,
        };
        let hit = recto_agent::find::Hit {
            page: 0,
            start: 0,
            end: 1,
            before: "".into(),
            matched: "".into(),
            after: "".into(),
            hidden: false,
        };
        vec![
            CoreCall::Reconcile,
            CoreCall::Tap(tab),
            CoreCall::ClearRecent,
            CoreCall::Unlocked(
                ask,
                Err(recto_core::OpenError::NotFound),
                Some(stamp),
                Arc::new(recto_core::Password::new("pw").unwrap()),
            ),
            CoreCall::LookAtShown,
            CoreCall::Jump(tab, 2, 0.0),
            CoreCall::JumpTyped(tab, "3".into()),
            CoreCall::Step(true),
            CoreCall::Open(PathBuf::from("/d/a.pdf")),
            CoreCall::SetSidebarHidden(true),
            CoreCall::SetSpread(None, record::Spread::TwoUp),
            CoreCall::CompareClick(tab),
            CoreCall::FocusPane(tab),
            CoreCall::SetSynced(true),
            CoreCall::Rotate(1),
            CoreCall::UndoOrRedo(true),
            CoreCall::SetPaper(Paper::Dark),
            CoreCall::ToggleAgentIcon,
            CoreCall::ClearEditHistory,
            CoreCall::SleepTabs(vec![tab], sleeper::Why::Asked),
            CoreCall::MemoryPressure(false),
            CoreCall::WokenFileIsOther(tab, FileStatus::Same(stamp)),
            CoreCall::FileLength(tab, 1, 1),
            CoreCall::LookAgain(tab),
            CoreCall::Page(1, None, tab),
            CoreCall::Read(Box::new(read)),
            CoreCall::Flush(Arc::new(Flushed::default())),
            CoreCall::Restored(Box::new(Loaded::Nothing)),
            CoreCall::RestorePatienceOver,
            CoreCall::Events(Vec::new()),
            CoreCall::CloseTab(tab, false),
            CoreCall::CloseAnswered(tab, saving::CloseChoice::Save),
            CoreCall::CloseAfterSave(tab),
            CoreCall::ContentsAction(tab, recto_session::ContentsAction::Generate),
            CoreCall::GeneratedFinished,
            CoreCall::GeneratedView(tab, recto_session::GeneratedView::Making, None),
            CoreCall::ToggleSection(tab, 0),
            CoreCall::Find(find::FindCall::Typed("a".into())),
            CoreCall::Find(find::FindCall::Open),
            CoreCall::Find(find::FindCall::Close),
            CoreCall::Find(find::FindCall::Step(true, recto_agent::find::Going::Jump)),
            CoreCall::Find(find::FindCall::Choose(0)),
            CoreCall::Find(find::FindCall::News),
            CoreCall::Find(find::FindCall::Reveal(1, hit)),
            CoreCall::SetEditMode(true),
        ]
    }

    /// Each variant's place, and each find call's: `(place, how many places in all)`.
    fn place(call: &CoreCall) -> (usize, usize) {
        let place = match call {
            CoreCall::Reconcile => 0,
            CoreCall::Tap(_) => 1,
            CoreCall::ClearRecent => 2,
            CoreCall::Unlocked(..) => 3,
            CoreCall::LookAtShown => 4,
            CoreCall::Jump(..) => 5,
            CoreCall::JumpTyped(..) => 6,
            CoreCall::Step(_) => 7,
            CoreCall::Open(_) => 8,
            CoreCall::SetSidebarHidden(_) => 9,
            CoreCall::SetSpread(..) => 10,
            CoreCall::CompareClick(_) => 11,
            CoreCall::FocusPane(_) => 12,
            CoreCall::SetSynced(_) => 13,
            CoreCall::Rotate(_) => 14,
            CoreCall::UndoOrRedo(_) => 15,
            CoreCall::SetPaper(_) => 16,
            CoreCall::ToggleAgentIcon => 17,
            CoreCall::ClearEditHistory => 18,
            CoreCall::SleepTabs(..) => 19,
            CoreCall::MemoryPressure(_) => 20,
            CoreCall::WokenFileIsOther(..) => 21,
            CoreCall::FileLength(..) => 22,
            CoreCall::LookAgain(_) => 23,
            CoreCall::Page(..) => 24,
            CoreCall::Read(_) => 25,
            CoreCall::Flush(_) => 26,
            CoreCall::Restored(_) => 27,
            CoreCall::RestorePatienceOver => 28,
            CoreCall::Events(_) => 29,
            CoreCall::CloseTab(..) => 30,
            CoreCall::CloseAnswered(..) => 31,
            CoreCall::CloseAfterSave(_) => 32,
            CoreCall::ContentsAction(..) => 33,
            CoreCall::GeneratedFinished => 34,
            CoreCall::GeneratedView(..) => 35,
            CoreCall::ToggleSection(..) => 36,
            CoreCall::Find(find_call) => match find_call {
                find::FindCall::Typed(_) => 37,
                find::FindCall::Open => 38,
                find::FindCall::Close => 39,
                find::FindCall::Step(..) => 40,
                find::FindCall::Choose(_) => 41,
                find::FindCall::News => 42,
                find::FindCall::Reveal(..) => 43,
            },
            CoreCall::SetEditMode(_) => 44,
        };
        (place, 45)
    }

    /// Failure (review C8): a call a person makes isn't on the hand-off freeze's list, so it's taken while a hand-off writes what comes back, and lost: closing a tab, the close prompt's answer, the contents' note and a section's chevron were. Every variant is sampled, and those a person makes, named here apart from the code that decides, are exactly the ones held back. The rest get through.
    #[test]
    fn every_person_originated_call_is_frozen_during_a_hand_off() {
        let calls = every_call();
        let mut places: Vec<usize> = calls.iter().map(|call| place(call).0).collect();
        places.sort_unstable();
        assert_eq!(
            places,
            (0..place(&CoreCall::Reconcile).1).collect::<Vec<_>>(),
            "one sample of every call, and of every find call"
        );
        let a_persons = |call: &CoreCall| {
            matches!(
                call,
                CoreCall::Tap(_)
                    | CoreCall::ClearRecent
                    | CoreCall::Jump(..)
                    | CoreCall::JumpTyped(..)
                    | CoreCall::Step(_)
                    | CoreCall::Open(_)
                    | CoreCall::SetSidebarHidden(_)
                    | CoreCall::SetSpread(..)
                    | CoreCall::CompareClick(_)
                    | CoreCall::FocusPane(_)
                    | CoreCall::SetSynced(_)
                    | CoreCall::Rotate(_)
                    | CoreCall::UndoOrRedo(_)
                    | CoreCall::SetPaper(_)
                    | CoreCall::ToggleAgentIcon
                    | CoreCall::ClearEditHistory
                    | CoreCall::CloseTab(..)
                    | CoreCall::CloseAnswered(..)
                    | CoreCall::ContentsAction(..)
                    | CoreCall::ToggleSection(..)
                    | CoreCall::SetEditMode(_)
                    | CoreCall::Find(
                        find::FindCall::Typed(_)
                            | find::FindCall::Open
                            | find::FindCall::Close
                            | find::FindCall::Step(..)
                            | find::FindCall::Choose(_)
                            | find::FindCall::Reveal(..)
                    )
            )
        };
        for call in calls {
            let queue = CoreQueue::default();
            queue.frozen.store(true, Ordering::SeqCst);
            let persons = a_persons(&call);
            let name = format!("{call:?}");
            queue.push(call);
            assert_eq!(
                lock(&queue.calls).is_empty(),
                persons,
                "{name}: a person's call is held back during a hand-off, and nothing else is"
            );
        }
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

    /// Failure (review 1): pane 0's page view keeps every warm tab off screen beside a comparison's panes, so three panes and two recent tabs are five live documents where SPEC §2 allows three.
    #[test]
    fn panes_and_warm_tabs_stay_within_the_live_documents() {
        let tabs = |ids: &[u32]| ids.iter().copied().map(TabId).collect::<Vec<_>>();
        assert_eq!(
            warm_beside_panes(tabs(&[5, 4, 3]).into_iter(), &tabs(&[5])),
            vec![4, 3],
            "one tab showing keeps two recent"
        );
        assert_eq!(
            warm_beside_panes(tabs(&[5, 4, 3]).into_iter(), &tabs(&[1, 2])),
            vec![5],
            "two panes leave room for one"
        );
        assert!(
            warm_beside_panes(tabs(&[5, 4, 3]).into_iter(), &tabs(&[1, 2, 3])).is_empty(),
            "three panes are all of them"
        );
        for panes in [tabs(&[1]), tabs(&[1, 2]), tabs(&[1, 2, 3])] {
            assert!(panes.len() + warm_beside_panes(tabs(&[9, 8, 7, 6]).into_iter(), &panes).len() <= recto_session::MAX_WARM);
        }
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
        let host = AppHost {
            engine: Engine::new(EngineConfig::default(), Arc::new(TestEngineEvents)),
            session: SessionHost::new(DrawableLoader),
            core: Arc::new(CoreQueue::default()),
            qa: None,
            saver: Arc::new(Saver::new(
                RecordStore::open(
                    &StatePaths::overridden(&std::env::temp_dir().join(format!("rf-{}", std::process::id()))).unwrap(),
                    Flavor::App,
                ),
                Arc::new(CoreQueue::default()),
            )),
            running: AtomicU32::new(0),
            handing_off: AtomicBool::new(false),
            notice: Mutex::new(None),
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

    /// Failure: QA options work on a normal launch, so a shipped app can be started with its limits changed, or a file named at launch is taken for an option.
    #[test]
    fn qa_options_only_go_with_a_qa_run() {
        let words = |list: &[&str]| list.iter().map(|word| (*word).to_string()).collect::<Vec<_>>();
        assert!(parse_args(&words(&["--qa-memory-limit-mib", "64"])).is_err());
        assert!(parse_args(&words(&["--qa-time-limit-ms", "5"])).is_err());
        assert!(parse_args(&words(&["--qa-owner-pid", "5"])).is_err());
        assert_eq!(parse_args(&words(&["--qa", "--qa-owner-pid", "77"])).unwrap().owner_pid, Some(77));
        assert!(
            parse_args(&words(&["--qa", "--qa-owner-pid", "1"])).is_err(),
            "launchd adopts orphans, so it'd be no owner"
        );
        let parsed = parse_args(&words(&["--qa", "--qa-memory-limit-mib", "64", "/d/a.pdf"])).unwrap();
        assert!(parsed.qa);
        assert_eq!(parsed.memory_limit_mib, Some(64));
        assert_eq!(parsed.open, vec![PathBuf::from("/d/a.pdf")]);
        assert!(parse_args(&words(&["--nope"])).is_err());
        assert!(parse_args(&words(&["--qa-memory-limit-mib"])).is_err());
        assert_eq!(parse_args(&words(&["-psn_0_12345"])).unwrap(), LaunchArgs::default(), "what macOS adds");
    }
}
