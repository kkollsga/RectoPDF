//! The bridge to the host, with no rules in it. The host draws what comes out of here and calls these for everything the user does.
//!
//! **Every call is cheap or returns at once.** Anything that can take time goes on the session thread, which is parked at rest, and the host hears when there's something new. Pixels never cross this: tiles travel as surfaces.
//! The session here is the one on the control socket, so `recto tabs` and `recto goto` act on what's on screen. The app's own actions go through `with_session`, as the owner's.

#![allow(unsafe_code)] // What UniFFI's macros generate. Nothing here writes any of its own.

mod qa;
mod watchdog;

use qa::QaBridge;
use recto_agent::{AgentError, Command, Condition, ConditionState, DrawableLoader, Event, Level};
use recto_control::StatePaths;
use recto_control::host::{Host, SessionHost};
use recto_control::server::{Announcer, Limits, Server, Stopper};
use recto_reader::engine::EngineEvents;
use recto_reader::tiles::{MAX_ZOOM, MIN_ZOOM, Op, PageMap, Viewport};
use recto_reader::{Engine, EngineConfig, PaperKind};
use recto_render_service::Policy;
use recto_session::{CONTENTS_ROW_HEIGHT, ContentsFollow, ContentsView, DocLoader, FollowMove, DrawNote, Paper, RowKind as SessionRowKind, SessionError, TAB_ROW_HEIGHT, TabId, ViewSettings, sidebar_heights};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use watchdog::Watchdog;

uniffi::setup_scaffolding!();

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
    /// One row per tab. The active tab's contents go under it, as many as `contents_count`.
    pub tabs: Vec<SidebarRow>,
    pub contents_count: u32,
    /// Rows from here on were opened or closed by the last change, moving what's below by `contents_moved` rows. The host keeps what's on screen where it is by moving its scroll the same.
    pub contents_changed_from: u32,
    pub contents_moved: i32,
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
    /// Run frames, calling `contents_step` on each, starting from `from` if there is one.
    Drift { from: Option<f64> },
}

/// What the page area shows.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ReaderView {
    /// No PDF is open, so it's the home state.
    pub home: bool,
    /// Something the reader needs to be told, such as a page at a lower resolution, in words.
    pub note: Option<String>,
    pub focus: bool,
    pub place: String,
    /// The paper's colour, `0xRRGGBB`.
    pub paper_rgb: u32,
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
    pub menu_open: String,
    pub menu_close_window: String,
    pub menu_quit: String,
    pub menu_focus: String,
    pub menu_zoom_in: String,
    pub menu_zoom_out: String,
    pub menu_actual_size: String,
    pub menu_white_paper: String,
    pub menu_file: String,
    pub menu_view: String,
}

/// Limits the host applies to native controls. The zoom limits are Rust's, and it says them once.
#[derive(Clone, Copy, Debug, uniffi::Record)]
pub struct ReaderLimits {
    pub min_zoom: f64,
    pub max_zoom: f64,
    pub zoom_step: f64,
    pub window_width: f64,
    pub window_height: f64,
    pub sidebar_width: f64,
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
    ShowPage { page: u32, x: f64, y: f64, width: f64, height: f64, fit: f64, paper_rgb: u32 },
    HidePage { page: u32 },
    /// `surface` is an `IOSurfaceRef` the host now owns one reference to, as a number, or zero if `rgb` is the colour.
    Attach { id: u64, page: u32, x: f64, y: f64, width: f64, height: f64, surface: u64, rgb: Option<u32>, crop_width: f64, crop_height: f64, below: bool },
    Detach { id: u64 },
}

impl From<Op> for ReaderOp {
    fn from(op: Op) -> Self {
        match op {
            Op::Clear => Self::Clear,
            Op::Document { width, height } => Self::Document { width, height },
            Op::Magnify { value } => Self::Magnify { value },
            Op::ScrollTo { y, seq } => Self::ScrollTo { y, seq },
            Op::ShowPage { page, x, y, width, height, fit, paper_rgb } => Self::ShowPage { page, x, y, width, height, fit, paper_rgb },
            Op::HidePage { page } => Self::HidePage { page },
            Op::Attach { id, page, x, y, width, height, surface, rgb, crop_width, crop_height, below } => Self::Attach { id, page, x, y, width, height, surface, rgb, crop_width, crop_height, below },
            Op::Detach { id } => Self::Detach { id },
        }
    }
}

/// The session the socket and the app share.
pub(crate) struct AppHost {
    session: SessionHost<DrawableLoader>,
    core: Arc<CoreQueue>,
    qa: Option<Arc<QaBridge>>,
}

impl Host for AppHost {
    fn run(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        // The level was checked by the server against the row, and again by `execute`. QA doesn't reach `execute`, which would say it needs an app.
        if matches!(command, Command::QaRead { .. } | Command::QaDrive { .. }) {
            return match &self.qa {
                Some(qa) => qa.run(command),
                None => Err(recto_agent::qa_needs_app()),
            };
        }
        let result = self.session.run(level, command, events);
        self.core.push(CoreCall::Reconcile);
        result
    }

    fn check(&self, condition: Condition, tab: &str, page: Option<u32>) -> ConditionState {
        self.session.check(condition, tab, page)
    }

    fn tab_count(&self) -> u32 {
        self.session.tab_count()
    }

    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        self.session.unsaved_tabs()
    }

    fn acted_tab(&self, command: &Command) -> Option<TabId> {
        self.session.acted_tab(command)
    }
}

/// What the session thread is asked to do, in the order the user asked.
#[derive(Debug)]
enum CoreCall {
    Reconcile,
    Tap(TabId),
    GoTo(TabId, u32),
    Open(PathBuf),
    SetPaper(Paper),
    SetFocus(bool),
    Dark(bool),
    Page(u32, Option<&'static str>, TabId),
    /// Everything asked before this has been done.
    Flush(Arc<Flushed>),
}

/// A marker through the session thread's queue, for QA to know what it asked for has happened.
#[derive(Debug, Default)]
pub(crate) struct Flushed {
    done: Mutex<bool>,
    ready: Condvar,
}

/// The session thread's queue. A parked thread at rest, so an open app takes no wake-ups for it.
pub(crate) struct CoreQueue {
    calls: Mutex<VecDeque<CoreCall>>,
    ready: Condvar,
    reconcile_queued: AtomicBool,
}

impl CoreQueue {
    fn push(&self, call: CoreCall) {
        // Many reasons to look can wait for one look.
        if matches!(call, CoreCall::Reconcile) && self.reconcile_queued.swap(true, Ordering::SeqCst) {
            return;
        }
        lock(&self.calls).push_back(call);
        self.ready.notify_one();
    }

    /// Blocks until the session thread has got through everything already asked of it. For QA, on a job thread. A file on a dead volume could hold this up, so it has a limit.
    pub(crate) fn flush(&self, limit: Duration) -> bool {
        let marker = Arc::new(Flushed::default());
        self.push(CoreCall::Flush(Arc::clone(&marker)));
        let deadline = Instant::now() + limit;
        let mut done = lock(&marker.done);
        while !*done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            done = marker.ready.wait_timeout(done, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }

    fn next(&self) -> CoreCall {
        let mut calls = lock(&self.calls);
        loop {
            if let Some(call) = calls.pop_front() {
                if matches!(call, CoreCall::Reconcile) {
                    self.reconcile_queued.store(false, Ordering::SeqCst);
                }
                return call;
            }
            calls = self.ready.wait(calls).unwrap_or_else(PoisonError::into_inner);
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
}

/// What the session thread remembers between looks.
#[derive(Default)]
struct Synced {
    tab: Option<TabId>,
    page: u32,
    pages: u32,
    map: PageMap,
    paper: Option<PaperKind>,
    focus: Option<bool>,
    dark: bool,
}

/// The state the UI thread pulls, swapped whole. A short lock, never held across anything slow.
struct Published {
    sidebar: Arc<SidebarView>,
    contents: Arc<Vec<ContentsEntry>>,
    rail_filled: u32,
    reader: ReaderView,
}

struct Inner {
    host: Arc<AppHost>,
    engine: Engine,
    core: Arc<CoreQueue>,
    listener: Arc<dyn AppListener>,
    published: Mutex<Published>,
    contents_follow: Mutex<ContentsFollow>,
    started: Instant,
    watchdog: Watchdog,
    stopper: Mutex<Option<Stopper>>,
    announcer: Mutex<Option<Announcer>>,
    dark: AtomicBool,
    qa: Option<Arc<QaBridge>>,
    paths: Option<StatePaths>,
    open_failure: Mutex<Option<String>>,
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
        let core = Arc::new(CoreQueue { calls: Mutex::new(VecDeque::new()), ready: Condvar::new(), reconcile_queued: AtomicBool::new(false) });
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
        let host = Arc::new(AppHost { session: SessionHost::new(DrawableLoader), core: Arc::clone(&core), qa: qa.clone() });
        let stopper = server.as_ref().and_then(|server| server.stopper().ok());
        let announcer = server.as_ref().and_then(|server| server.announcer().ok());
        let inner = Arc::new(Inner {
            host: Arc::clone(&host),
            engine,
            core: Arc::clone(&core),
            listener: Arc::clone(&listener),
            contents_follow: Mutex::new(ContentsFollow::default()),
            started: Instant::now(),
            published: Mutex::new(Published { contents: Arc::new(Vec::new()), rail_filled: 0, sidebar: Arc::new(SidebarView { tabs: Vec::new(), contents_count: 0, contents_changed_from: 0, contents_moved: 0, contents_current: None, toc_open: false, place: String::new(), edited: false, undone_owner_steps: false }), reader: ReaderView { home: true, note: None, focus: false, place: String::new(), paper_rgb: PaperKind::White.rgb() } }),
            watchdog,
            stopper: Mutex::new(stopper),
            announcer: Mutex::new(announcer),
            dark: AtomicBool::new(false),
            qa,
            paths: Some(paths),
            open_failure: Mutex::new(None),
        });
        let session_side = Arc::clone(&inner);
        std::thread::Builder::new().name("recto-session".into()).stack_size(8 * 1024 * 1024).spawn(move || session_thread(&session_side)).map_err(|_| refused("The app couldn't start its session thread.".into()))?;
        if let Some(server) = server {
            let ending = Arc::clone(&listener);
            let served = Arc::clone(&host);
            // Blocked in `poll` with nothing happening, as `recto serve` is.
            std::thread::Builder::new()
                .name("recto-control".into())
                .stack_size(512 * 1024)
                .spawn(move || {
                    let _ = server.run(served, Limits::default());
                    ending.session_ended();
                })
                .map_err(|_| refused("The app couldn't start its control socket.".into()))?;
        }
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
            menu_open: "Open…".into(),
            menu_close_window: "Close Window".into(),
            menu_quit: "Quit RectoPDF".into(),
            menu_focus: "Focus View".into(),
            menu_zoom_in: "Zoom In".into(),
            menu_zoom_out: "Zoom Out".into(),
            menu_actual_size: "Fit Width".into(),
            menu_white_paper: "White Paper in Dark Mode".into(),
            menu_file: "File".into(),
            menu_view: "View".into(),
        }
    }

    pub fn limits(&self) -> ReaderLimits {
        ReaderLimits { min_zoom: MIN_ZOOM, max_zoom: MAX_ZOOM, zoom_step: 1.25, window_width: 1440.0, window_height: 900.0, sidebar_width: 272.0 }
    }

    /// The sidebar as it stands. A short lock and a copy.
    pub fn sidebar(&self) -> SidebarView {
        (*lock(&self.inner.published).sidebar).clone()
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
                Some(SidebarRow { kind: RowKind::Contents, tab: entry.tab, text: entry.text.to_string(), page: entry.page, depth: entry.depth, current: current == Some(index), rail_filled: index < rail_filled, rail_first: index == 0, rail_last: index + 1 == total })
            })
            .collect()
    }

    /// The sidebar's height rule, for this much room between its header and footer.
    pub fn sidebar_layout(&self, room: f64) -> SidebarLayout {
        let published = lock(&self.inner.published);
        let tabs = u32::try_from(published.sidebar.tabs.len()).unwrap_or(u32::MAX);
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
            FollowMove::Drift { from } => ContentsMove::Drift { from },
        }
    }

    /// One frame of a drift: where the scroll goes next, or nothing when it's over and the frames should stop. An atomic-sized lock and some arithmetic.
    pub fn contents_step(&self, position: f64, dt: f64) -> Option<f64> {
        lock(&self.inner.contents_follow).step(position, dt)
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
        self.inner.core.push(CoreCall::Dark(dark));
    }

    pub fn toggle_white_paper(&self) {
        let paper = self.inner.host.session.with_session(|session| session.view().paper);
        self.inner.core.push(CoreCall::SetPaper(if paper == Paper::White { Paper::Dark } else { Paper::White }));
    }

    pub fn is_white_paper(&self) -> bool {
        lock(&self.inner.published).reader.paper_rgb == PaperKind::White.rgb()
    }

    pub fn toggle_focus(&self) {
        let focus = !lock(&self.inner.published).reader.focus;
        self.inner.core.push(CoreCall::SetFocus(focus));
    }

    pub fn window_closed(&self) {
        self.inner.engine.window_closed();
    }

    pub fn window_opened(&self) {
        self.inner.engine.window_opened();
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

    /// Ends the socket, on the way out. Returns at once.
    pub fn shutdown(&self) {
        if let Some(stopper) = lock(&self.inner.stopper).take() {
            stopper.stop();
        }
    }
}

fn session_thread(inner: &Arc<Inner>) {
    let mut synced = Synced::default();
    loop {
        let call = inner.core.next();
        let mut events = Vec::new();
        let host = &inner.host;
        match call {
            CoreCall::Reconcile => {}
            CoreCall::Tap(id) => {
                let was = host.session.with_session(|session| session.active());
                let tapped = host.session.with_session(|session| session.tap(id));
                if tapped.is_ok() && was != Some(id) {
                    events.push(Event::new("tab_activated", json!({ "tab": id.0 })));
                }
            }
            CoreCall::GoTo(id, page) => {
                // `go_to` clamps: a contents entry can't send the reader off the end.
                if host.session.with_session(|session| session.go_to(id, page)).is_ok() {
                    events.push(Event::new("page_changed", json!({ "tab": id.0, "page": page + 1 })));
                }
            }
            CoreCall::Open(path) => open_file(inner, &path, &mut events),
            CoreCall::SetPaper(paper) => {
                let view = host.session.with_session(|session| {
                    let view = ViewSettings { paper, ..session.view() };
                    session.set_view(view);
                    view
                });
                events.push(Event::new("view_changed", json!({ "paper": view.paper.name(), "focus": view.focus })));
            }
            CoreCall::SetFocus(focus) => {
                let view = host.session.with_session(|session| {
                    let view = ViewSettings { focus, ..session.view() };
                    session.set_view(view);
                    view
                });
                events.push(Event::new("view_changed", json!({ "paper": view.paper.name(), "focus": view.focus })));
            }
            CoreCall::Dark(dark) => synced.dark = dark,
            CoreCall::Flush(marker) => {
                reconcile(inner, &mut synced, &mut events);
                *lock(&marker.done) = true;
                marker.ready.notify_all();
                continue;
            }
            CoreCall::Page(page, failure, tab) => {
                host.session.with_session(|session| session.note_drawn(DrawNote { tab, page, failure }));
                events.push(Event::new(if failure.is_some() { "page_failed" } else { "page_drawn" }, json!({ "tab": tab.0, "page": page + 1 })));
            }
        }
        reconcile(inner, &mut synced, &mut events);
        if !events.is_empty()
            && let Some(announcer) = lock(&inner.announcer).as_ref()
        {
            announcer.announce(events);
        }
    }
}

/// Where two lists of contents rows differ and how much what's below moved: the rows opened or closed as the reader enters a chapter. Each list's matching start and end are compared, which is the whole of what changes, and it's only looked at when the lists differ.
fn rows_moved(before: &[ContentsEntry], after: &[ContentsEntry]) -> (u32, i32) {
    if std::ptr::eq(before, after) || before.is_empty() || after.is_empty() {
        return (0, 0);
    }
    let same_start = before.iter().zip(after).take_while(|(one, two)| one == two).count();
    // Past the matching start, whole rows were added or removed if everything after matches at the end.
    let moved = i32::try_from(after.len()).unwrap_or(i32::MAX) - i32::try_from(before.len()).unwrap_or(i32::MAX);
    (u32::try_from(same_start).unwrap_or(u32::MAX), moved)
}

fn open_file(inner: &Inner, path: &Path, events: &mut Vec<Event>) {
    let host = &inner.host;
    *lock(&inner.open_failure) = None;
    if host.session.with_session(|session| session.go_to_open_file(path)).is_some() {
        return;
    }
    // The file is read with the session free, so an agent's `status` or `tabs` isn't held up by a slow volume.
    let stamp = recto_platform::file_stamp(path).ok();
    let opened = DrawableLoader.load(path).map_err(SessionError::Open).and_then(|facts| host.session.with_session(|session| session.open_with_facts(path, facts, stamp)));
    match opened {
        Ok(id) => events.push(Event::new("tab_opened", json!({ "tab": id.0 }))),
        Err(error) => *lock(&inner.open_failure) = Some(error.message().to_string()),
    }
}

/// Brings everything into line with the session, on the session thread. The place in a tab can move two ways, by the reader or by an agent, and each is passed to the other side.
fn reconcile(inner: &Inner, synced: &mut Synced, events: &mut Vec<Event>) {
    let moved = inner.engine.take_moved_page();
    let synced_tab = synced.tab;
    let (snapshot, sidebar, contents, rail_filled, view) = inner.host.session.with_session(|session| {
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
            let map = match session.page_edits(id) {
                None => PageMap { files: vec![own], slots: None },
                Some(edits) => {
                    let mut files = vec![own];
                    files.extend(edits.added_sources().iter().map(|added| added.path.to_path_buf()));
                    PageMap { files, slots: Some(edits.slots().iter().map(|slot| (slot.source, slot.page)).collect()) }
                }
            };
            let warm: Vec<u32> = session.warm_tabs().filter(|other| *other != id).map(|other| other.0).collect();
            Some((id, session.pages_in(id), tab.page, map, warm, tab.problem.map(|problem| problem.message().to_string())))
        });
        let model = session.sidebar();
        let mut index = 0u32;
        let mut current = None;
        let mut tabs = Vec::new();
        let mut contents_row_count = 0u32;
        for row in &model.rows {
            if row.kind == SessionRowKind::Contents {
                if row.current {
                    current = Some(index);
                }
                index += 1;
                contents_row_count += 1;
            } else {
                tabs.push(SidebarRow { kind: RowKind::Tab, tab: row.tab.0, text: row.text.to_string(), page: row.page, depth: row.depth, current: row.current, rail_filled: false, rail_first: false, rail_last: false });
            }
        }
        // Copied only when they've changed: for a long outline, a page crossed mustn't mean thousands of strings.
        let held = Arc::clone(&lock(&inner.published).contents);
        let same = held.len() == contents_row_count as usize && model.rows.iter().filter(|row| row.kind == SessionRowKind::Contents).zip(held.iter()).all(|(row, entry)| row.tab.0 == entry.tab && row.page == entry.page && row.depth == entry.depth && row.text == &*entry.text);
        let held_before = Arc::clone(&held);
        let contents = if same {
            held
        } else {
            Arc::new(model.rows.iter().filter(|row| row.kind == SessionRowKind::Contents).map(|row| ContentsEntry { tab: row.tab.0, text: row.text.into(), page: row.page, depth: row.depth }).collect())
        };
        let rail_filled = u32::try_from(model.rail_filled).unwrap_or(u32::MAX);
        let (contents_changed_from, contents_moved) = rows_moved(&held_before, &contents);
        (snapshot, SidebarView { tabs, contents_count: contents_row_count, contents_changed_from, contents_moved, contents_current: current, toc_open: model.toc_open, place: model.place.clone(), edited: model.edited, undone_owner_steps: model.undone_owner_steps }, contents, rail_filled, session.view())
    });
    // The paper and focus first, so what's asked for next is drawn right.
    let dark = inner.dark.load(Ordering::SeqCst) || synced.dark;
    let paper = if dark && view.paper == Paper::Dark { PaperKind::Dark } else { PaperKind::White };
    if synced.paper != Some(paper) {
        synced.paper = Some(paper);
        inner.engine.set_paper(paper);
    }
    if synced.focus != Some(view.focus) {
        synced.focus = Some(view.focus);
        inner.engine.set_focus(view.focus);
    }
    let mut problem = None;
    match snapshot {
        None => {
            if synced.tab.take().is_some() {
                inner.engine.deactivate();
            }
        }
        Some((id, pages, page, map, warm, tab_problem)) => {
            problem = tab_problem;
            if synced.tab != Some(id) {
                lock(&inner.contents_follow).new_contents();
            }
            if synced.tab != Some(id) || synced.pages != pages || synced.map != map {
                inner.engine.activate(id.0, pages, page, map.clone(), &warm);
                synced.tab = Some(id);
                synced.pages = pages;
                synced.map = map;
                synced.page = page;
            } else if synced.page != page {
                inner.engine.jump_to(page);
                synced.page = page;
            }
        }
    }
    let note = lock(&inner.open_failure).clone().or(problem).or_else(|| inner.engine.note());
    let reader = ReaderView { home: synced.tab.is_none(), note, focus: view.focus, place: sidebar.place.clone(), paper_rgb: paper.rgb() };
    let mut published = lock(&inner.published);
    let changed = *published.sidebar != sidebar || published.reader != reader || published.rail_filled != rail_filled || !Arc::ptr_eq(&published.contents, &contents);
    if changed {
        published.sidebar = Arc::new(sidebar);
        published.reader = reader;
        published.contents = contents;
        published.rail_filled = rail_filled;
    }
    drop(published);
    if changed {
        inner.listener.model_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: rows opening as the reader enters a chapter shift everything below, so the list seems to jump under a smooth drift. It must say from where and by how much, and a change of nothing says nothing.
    #[test]
    fn opening_and_closing_rows_says_what_moved() {
        let row = |text: &str| ContentsEntry { tab: 1, text: text.into(), page: Some(1), depth: 0 };
        let before = vec![row("1"), row("2"), row("3")];
        let after = vec![row("1"), row("2"), row("2.1"), row("2.2"), row("3")];
        assert_eq!(rows_moved(&before, &after), (2, 2));
        assert_eq!(rows_moved(&after, &before), (2, -2));
        assert_eq!(rows_moved(&before, &before), (0, 0));
        assert_eq!(rows_moved(&[], &after), (0, 0));
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
