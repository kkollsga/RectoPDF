//! The bridge to the host, with no rules in it. The host draws what comes out of here and calls these for everything the user does.
//!
//! **Every call is cheap or returns at once.** Anything that can take time goes on the session thread, which is parked at rest, and the host hears when there's something new. Pixels never cross this: tiles travel as surfaces.
//! The session here is the one on the control socket, so `recto tabs` and `recto goto` act on what's on screen. The app's own actions go through `with_session`, as the owner's.

#![allow(unsafe_code)] // What UniFFI's macros generate. Nothing here writes any of its own.

pub mod chrome;
mod find;
pub mod linking;
mod menus;
mod palette;
pub mod passwords;
mod printing;
mod qa;
mod records;
mod saving;
mod sidebar_access;
mod snapshot_tool;
pub mod updates;

pub use menus::{HostAction, MenuEntry, MenuModel, PanelItem, PanelModel, PanelSection};
pub use recto_app::PaneView;
pub use recto_app::RecentFile;
pub use recto_app::SelectionNotice;
use recto_app::core::*;
pub use recto_app::hand_off::{BusyItem, HandOffCheck, HandOffError, HandOffReport};
pub use recto_app::listener::QuitAnswer;
use recto_app::privacy;
use recto_app::queue::CoreCall;
pub use recto_app::reader_view::ReaderView;
pub use recto_app::records::{HighlightRect, ReaderOp, ScreenInfo};
pub use recto_app::{RowKind, SidebarRow, SidebarView};
use recto_control::StatePaths;
use recto_reader::tiles::{MAX_ZOOM, MIN_ZOOM, Viewport};
use recto_session::record::{self, Appearance, FocusView, Rect, ScreenNow, ScreenRecord, SettingsWindow, WindowRecord, place_window};
use recto_session::{CONTENTS_ROW_HEIGHT, ContentsRowChange, ContentsView, FollowMove, TAB_ROW_HEIGHT, TabId, row_full_text, sidebar_heights_with_note};
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

uniffi::setup_scaffolding!();

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
#[uniffi::remote(Record)]
pub struct QuitAnswer {
    /// Go ahead. Otherwise the words below are for a question: quit anyway, or stay.
    pub go: bool,
    pub title: String,
    pub message: String,
    pub quit_button: String,
    pub stay_button: String,
}

/// A screen as the host sees it now.
#[uniffi::remote(Record)]
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
#[uniffi::remote(Record)]
pub struct BusyItem {
    pub code: String,
    pub message: String,
}

/// The cheap look at whether an update could go now, from a thread or the UI thread.
#[uniffi::remote(Record)]
pub struct HandOffCheck {
    pub safe: bool,
    pub busy: Vec<BusyItem>,
    /// Since the user last touched the app.
    pub seconds_since_input: f64,
}

/// Why a hand-off isn't going ahead.
#[uniffi::remote(Error)]
pub enum HandOffError {
    /// Something's in flight, and an update should wait.
    Busy { items: Vec<BusyItem> },
    /// The state couldn't be written and confirmed.
    NotSaved { code: String, message: String },
}

/// How the state's write and read-back went.
#[uniffi::remote(Record)]
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

#[uniffi::remote(Enum)]
pub enum RowKind {
    Tab,
    Contents,
    /// A quiet line of Rust's over the contents, about generated contents. Its `notice` is what it means, for the pointer resting on it, and `action` what a tap does.
    Note,
}

/// A row of the sidebar. At rest it's a name. Its page number shows on hover.
#[uniffi::remote(Record)]
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
#[uniffi::remote(Record)]
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
#[uniffi::remote(Record)]
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
#[uniffi::remote(Enum)]
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
        let (qa_mode, open) = (args.qa, args.open.clone());
        let inner = Inner::start(Start {
            listener,
            clock: Arc::new(recto_app::clock::SystemClock::new()),
            update_source: recto_update::source::from_env(paths.overridden),
            paths,
            args,
            socket: true,
            indexing: true,
        })
        .map_err(refused)?;
        updates::note_asked_to_open(&open);
        Ok(Arc::new(Self {
            inner,
            qa_mode,
            start_with: open,
        }))
    }

    /// Whether this is a quiet run: off every screen, never active, with no Dock icon.
    pub fn is_qa(&self) -> bool {
        self.qa_mode
    }

    /// The window is up. In a QA run this tells whoever's waiting, on one line, and any files named at launch start opening.
    pub fn ready(&self) {
        recto_app::core::printing::sweep_at_launch();
        self.inner.ready(&self.start_with);
        if self.qa_mode {
            let socket = self.inner.serving.paths.as_ref().map(|paths| paths.socket.clone());
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
        (*lock(&self.inner.published).sidebar).clone()
    }

    /// The tabs, one row each. Worth pulling only when `tabs_version` has moved: this copies all of them.
    pub fn sidebar_tabs(&self) -> Vec<SidebarRow> {
        self.inner.sidebar_tabs()
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
        lock(&self.inner.window.contents_follow).user_scrolled(self.inner.life.started.elapsed().as_secs_f64());
    }

    /// The pointer came into the contents or left them.
    pub fn contents_pointer(&self, inside: bool) {
        lock(&self.inner.window.contents_follow).pointer(inside);
    }

    /// The place being read is now on this row. What to do about it.
    pub fn contents_reached(&self, row: Option<u32>, scroll_y: f64, height: f64, reduce_motion: bool) -> ContentsMove {
        let rows = lock(&self.inner.published).sidebar.contents_count;
        let now = self.inner.life.started.elapsed().as_secs_f64();
        match lock(&self.inner.window.contents_follow).reached(row, ContentsView { scroll_y, height, rows }, now, reduce_motion) {
            FollowMove::Stay => ContentsMove::Stay,
            FollowMove::SetTo(y) => ContentsMove::SetTo { y },
            FollowMove::Move => ContentsMove::Move,
        }
    }

    /// One frame of a move: where the scroll goes next, or nothing when it's over and the frames should stop. An atomic-sized lock and some arithmetic.
    pub fn contents_step(&self, position: f64, dt: f64) -> Option<f64> {
        lock(&self.inner.window.contents_follow).step(position, dt)
    }

    /// The contents' area changed height.
    pub fn contents_view_changed(&self) {
        lock(&self.inner.window.contents_follow).view_changed();
    }

    /// Rows closed or opened. Says how many points to move the scroll by with no animation, or nothing if the rows are to change in view, where the host fades them over.
    pub fn contents_row_change(&self, scroll_y: f64, from: u32, removed: u32, inserted: u32, place_shift: i32) -> Option<f64> {
        lock(&self.inner.window.contents_follow).row_change(
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
        lock(&self.inner.window.contents_follow).shifted(by);
    }

    pub fn contents_is_following(&self) -> bool {
        lock(&self.inner.window.contents_follow).is_following()
    }

    pub fn contents_is_moving(&self) -> bool {
        lock(&self.inner.window.contents_follow).is_moving()
    }

    pub fn reader(&self) -> ReaderView {
        lock(&self.inner.published).reader.clone()
    }

    /// A contents row's chevron was tapped: its sections fold or open. On the session thread, as every change to what the sidebar shows is.
    pub fn toggle_contents_section(&self, first: u32) {
        self.inner.toggle_contents_section(first);
    }

    /// A row was tapped. Goes on the session thread, as waking a cold tab reads its file.
    pub fn tap_row(&self, row: SidebarRow) {
        self.inner.tap_row(row);
    }

    /// ⌘-click or ⌃-click on a row: a tab goes into a comparison, or out of it (owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view"). A contents row does what a click does.
    pub fn tap_row_adding(&self, row: SidebarRow) {
        self.inner.tap_row_adding(row);
    }

    /// The pointer went down in a pane: it's the one being read. Cheap, and the session hears on its own thread.
    pub fn pane_focused(&self, pane: u32) {
        if let Some(tab) = self.inner.panes.focus(pane as usize) {
            self.focus_moved(tab);
        }
    }

    pub fn open_file(&self, path: String) {
        self.inner.open_file(PathBuf::from(path));
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
        self.inner.window.dark.store(dark, Ordering::SeqCst);
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
        self.inner.bar_now()
    }

    /// **The password sheet was answered:** for the prompt with this id, with what was typed, or nothing for Cancel. The answer goes to that prompt's file, whatever's waiting by now, and none if it's gone. It's tried on a thread of its own, as reading a file can wait on its disk, and kept nowhere but the open tab's keyring. The `String` UniFFI made is overwritten here, so Rust holds one copy.
    pub fn password_answered(&self, id: u64, password: Option<String>) {
        self.inner.password_answered(id, password);
    }

    /// **Files opened lately, newest first,** for ⌘K and anything else that offers them. From what the session thread last published: it never waits.
    pub fn recent_files(&self) -> Vec<RecentFile> {
        self.inner.recent_files()
    }

    /// Opens a file from that list by its key, as File > Open Recent does: the very file that was offered, however the list has moved since, and nothing if it's left the list. Its tab if it's open, and otherwise it's read as any open is, and says so if it's gone.
    pub fn open_recent(&self, key: String) {
        self.inner.open_recent(key);
    }

    /// The sidebar's panel as it stands.
    pub fn panel(&self) -> PanelModel {
        let reader = lock(&self.inner.published).reader.clone();
        let mut panel = menus::panel(self.inner.window.panel_open.load(Ordering::SeqCst), reader.edit_mode, reader.can_rotate);
        recto_app::snapshot_menu::add_to_panel(&mut panel, self.inner.tools.tool.is_on(), reader.edit_mode);
        panel
    }

    /// Does what a menu item, a panel button or a key does, and says what's left for the host. The same methods as `recto view`.
    pub fn perform(&self, id: String) -> HostAction {
        self.inner.perform(id)
    }

    /// Everything ⌘K's palette draws: its rows, ranked, worded and with their keys, all Rust's. Short locks, never the session's.
    pub fn palette_view(&self) -> palette::PaletteView {
        recto_app::core::palette::view(&self.inner)
    }

    /// What's in the palette's field now, as it's typed.
    pub fn palette_typed(&self, text: String) {
        recto_app::core::palette::typed(&self.inner, &text);
    }

    /// ↑ (-1) and ↓ (1) in the palette.
    pub fn palette_step(&self, by: i32) {
        recto_app::core::palette::step(&self.inner, by);
    }

    /// A click on a row of the palette.
    pub fn palette_run(&self, id: String) -> HostAction {
        recto_app::core::palette::run(&self.inner, &id)
    }

    /// Escape, or the keyboard going elsewhere.
    pub fn palette_close(&self) {
        recto_app::core::palette::close(&self.inner);
    }

    /// Whether a menu item can be chosen now. Asked when a menu opens and on its key. By the item's rule in `menus`: an id without one can't be chosen.
    pub fn menu_item_enabled(&self, id: String) -> bool {
        self.inner.menu_item_enabled(id)
    }

    /// Whether Save does anything now: the tab showing has unsaved edits and isn't being saved.
    fn can_save(&self) -> bool {
        self.inner.can_save()
    }

    /// What Save As's sheet says, for the tab showing. Never waits: the path is read only if the session's free, and the signature line is what the session thread found.
    pub fn save_panel(&self) -> saving::SavePanelModel {
        self.inner.save_panel()
    }

    /// Save As's sheet was answered with this path. `replace` is whether the person agreed to replace a file that's there, which the sheet asks.
    pub fn save_as(&self, tab: u32, path: String) {
        self.inner.save_as(tab, path);
    }

    /// What closing the document showing asks.
    pub fn close_prompt(&self) -> saving::ClosePrompt {
        self.inner.close_prompt()
    }

    /// The close prompt's answer. Save closes once it's saved, and not if the save fails, which says why. Don't Save throws the edits away and closes. Save As opens its sheet, and the tab stays.
    pub fn close_answered(&self, tab: u32, choice: saving::CloseChoice) -> HostAction {
        self.inner.close_answered(tab, choice)
    }

    /// A save's problem was answered: with Save As, its sheet opens.
    pub fn save_problem_answered(&self, save_as: bool) -> HostAction {
        self.inner.save_problem_answered(save_as)
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
        if self.inner.tools.tool.is_on() {
            return self.inner.tools.tool.press(&engine, pane as usize, x, y);
        }
        engine.selection_press(x, y, clicks, shift).into_iter().map(ReaderOp::from).collect()
    }

    pub fn selection_drag(&self, pane: u32, x: f64, y: f64) -> Vec<ReaderOp> {
        let Some(engine) = self.inner.panes.engine(&self.inner.engine, pane as usize) else {
            return Vec::new();
        };
        if self.inner.tools.tool.is_drawing() {
            return self.inner.tools.tool.drag(&engine, pane as usize, x, y);
        }
        engine.selection_drag(x, y).into_iter().map(ReaderOp::from).collect()
    }

    /// The mouse went up. A still single click on a link follows it, by Rust's rule: within the document at once, a web or email address only once the sheet's been answered, and anything else never, with a notice saying why.
    pub fn selection_release(&self, pane: u32) -> Vec<ReaderOp> {
        let Some(engine) = self.inner.panes.engine(&self.inner.engine, pane as usize) else {
            return Vec::new();
        };
        if self.inner.tools.tool.is_drawing() {
            return self.inner.tools.tool.release(&self.inner, &engine, pane as usize);
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
        if self.inner.tools.tool.is_on() {
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
        self.inner.link_prompt_answered(id, open)
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
        self.inner.page_typed(&text)
    }

    /// A swipe on the page, as the system reports it: Back or Forward, as in Safari.
    pub fn swiped(&self, delta_x: f64) {
        self.inner.swiped(delta_x);
    }

    /// Whether the pointer's over text, for the I-beam. A page not read yet says no, and its text is read when there's nothing better to do.
    pub fn pointer_is_over_text(&self, pane: u32, x: f64, y: f64) -> bool {
        // The snapshot tool's pointer is a crosshair wherever it is.
        if self.inner.tools.tool.is_on() {
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
        self.inner.dismiss_selection_notice();
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
        self.inner.toggle_sidebar();
    }

    pub fn window_closed(&self) {
        self.inner.window_closed();
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
        let mut app = lock(&self.inner.window.app);
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
        if let Some(window) = lock(&self.inner.window.app).window.as_mut() {
            window.minimised = minimised;
        }
        self.inner.saver.view_changed();
    }

    /// The app was hidden or shown again, by the system's notices, or QA. Going out of sight writes what's changed, as going to the background does.
    pub fn app_hidden_changed(&self, hidden: bool) {
        lock(&self.inner.window.app).app_hidden = hidden;
        if hidden {
            self.inner.saver.write_soon();
        } else {
            self.inner.saver.view_changed();
        }
    }

    /// Whether this launch hides the app as soon as it's up: an update arriving to an app that was hidden. Any other launch shows it, and the record's told so.
    pub fn app_comes_back_hidden(&self) -> bool {
        let mut app = lock(&self.inner.window.app);
        let hidden = updates::stays_out_of_sight(app.app_hidden, self.inner.updates.reopened_by_update());
        app.app_hidden = hidden;
        app.hidden_decided = true;
        hidden
    }

    pub fn window_opened(&self) {
        self.inner.window_opened();
    }

    /// Whether this launch opens a window. Any launch does but an update's arrival, which leaves things as they were: a window closed stays closed, rather than appearing behind whatever the user's doing.
    pub fn window_should_open(&self) -> bool {
        self.inner.window_should_open()
    }

    /// Whether the state's been read and taken in. A window that went up without it, past the patience, is made again from it once it has: see the host's `restoreReady`.
    pub fn state_read(&self) -> bool {
        self.inner.state_read()
    }

    /// What the host shows has been made from the state that was read.
    pub fn shown_with_state(&self) {
        self.inner.shown_with_state();
    }

    /// Whether the window takes what a person does: once it's been made from the state, or once the state's been waited for long enough. Until then it's shown and takes nothing, as it's made again from the state when that comes, and what was done to it would go with it.
    pub fn window_interactive(&self) -> bool {
        self.inner.window_interactive()
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
        self.inner.window.watchdog.busy();
    }

    /// The UI thread is about to sleep.
    pub fn main_idle(&self) {
        self.inner.window.watchdog.idle();
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
        self.inner.begin_restore();
    }

    /// The state hasn't been read in time, or a restored tab's file isn't answering. What's on the screen goes on without it.
    pub fn restore_patience_over(&self) {
        self.inner.restore_patience_over();
    }

    /// Whether what the window would show now is right: the place on the layers, or nothing to wait for. Then, or once the patience is over, it shows. A short lock, asked only until it's showing.
    pub fn ready_to_show(&self) -> bool {
        if self.inner.life.started.elapsed() >= SHOW_PATIENCE + RESTORE_PATIENCE {
            return true;
        }
        if !self.inner.life.restored.load(Ordering::SeqCst) || lock(&self.inner.life.holding_tab).is_some() {
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
        *lock(&self.inner.life.first_shown) = Some(
            json!({ "home": home, "page": report["page"], "scroll_y": report["scroll_y"], "scroll_x": report["scroll_x"], "magnification": report["magnification"], "settled": report["settled"], "tab": report["tab"], "tile_layers": tile_layers, "ms_from_start": self.inner.life.started.elapsed().as_secs_f64() * 1000.0 }),
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
            let mut app = lock(&self.inner.window.app);
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
        let mut app = lock(&self.inner.window.app);
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
        lock(&self.inner.window.app).sidebar_width = record::sidebar_width(width);
        self.inner.core.push(CoreCall::Reconcile);
    }

    /// Where the contents are scrolled to now.
    pub fn contents_position(&self, y: f64) {
        lock(&self.inner.window.app).contents_y = y.is_finite().then_some(y);
        self.inner.saver.view_changed();
    }

    /// What has the keyboard.
    pub fn focus_changed(&self, view: String) {
        if let Some(view) = FocusView::from_name(&view) {
            lock(&self.inner.window.app).focus.view = view;
            self.inner.saver.view_changed();
        }
    }

    pub fn focus_view(&self) -> String {
        lock(&self.inner.window.app).focus.view.name().to_string()
    }

    /// The app became the active one or stopped, and the window key or not. Going to the background writes what's changed: from there, nobody's watching.
    pub fn frontmost_changed(&self, app: bool, window_key: bool) {
        self.inner.frontmost_changed(app, window_key);
    }

    /// Light or dark as chosen, or `system`.
    pub fn appearance_choice(&self) -> String {
        match lock(&self.inner.window.app).appearance {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
        .to_string()
    }

    pub fn set_appearance_choice(&self, choice: String) {
        lock(&self.inner.window.app).appearance = match choice.as_str() {
            "light" => Appearance::Light,
            "dark" => Appearance::Dark,
            _ => Appearance::System,
        };
        self.inner.saver.view_changed();
    }

    /// A sheet or panel opened over the window, or closed. An update waits for it.
    pub fn sheet_changed(&self, open: bool) {
        self.inner.sheet_changed(open);
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
        updates::opens_in_background(self.inner.updates.reopened_by_update(), lock(&self.inner.window.app).focus.app_frontmost)
    }

    /// A quit was asked for, by the menu or the system. Everything's written, on a thread of its own, and `quit_answer` says what to do. Mac apps with Resume don't ask on the way out, and neither does this, as unsaved edits come back: closing a tab with them is where Don't Save, Cancel or Save belongs. It asks only if they couldn't be kept.
    pub fn quit_requested(&self) {
        self.inner.quit_requested();
    }

    /// The quit's question was answered with Stay: the app goes on, and isn't quitting after all.
    pub fn quit_stayed(&self) {
        self.inner.quit_stayed();
    }

    /// Ends the socket, on the way out. Returns at once.
    pub fn shutdown(&self) {
        self.inner.shutdown();
    }
}

impl AppCore {
    /// The pane being read changed, by the pointer or a scroll: the session's told on its own thread, and nothing waits.
    fn focus_moved(&self, tab: TabId) {
        self.inner.core.push(CoreCall::FocusPane(tab));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

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
