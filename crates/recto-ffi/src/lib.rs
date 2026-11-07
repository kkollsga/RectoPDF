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
use recto_session::{CONTENTS_ROW_HEIGHT, ContentsFollow, ContentsRowChange, ContentsView, DocLoader, FollowMove, WARM_PAPER, row_full_text, DrawNote, Paper, RowKind as SessionRowKind, SessionError, TAB_ROW_HEIGHT, TabId, ViewSettings, sidebar_heights};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use watchdog::Watchdog;

uniffi::setup_scaffolding!();

/// Where the chosen paper is remembered, in the state folder.
const PAPER_FILE: &str = "paper";

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
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ReaderView {
    /// No PDF is open, so it's the home state.
    pub home: bool,
    /// Something the reader needs to be told, such as a page at a lower resolution, in words.
    pub note: Option<String>,
    pub focus: bool,
    pub place: String,
    /// The paper's name, which QA and the menu use.
    pub paper: String,
    /// The paper's colour, `0xRRGGBB`.
    pub paper_rgb: u32,
    /// A colour to multiply over each page in the compositor, for warm paper. No tile changes for it.
    pub page_tint: Option<u32>,
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
    pub menu_open: String,
    pub menu_close_window: String,
    pub menu_quit: String,
    pub menu_focus: String,
    pub menu_zoom_in: String,
    pub menu_zoom_out: String,
    pub menu_actual_size: String,
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

    /// Quitting is the person's. A quiet run is someone's test, and stops when it's told to.
    fn stop_refusal(&self) -> Option<AgentError> {
        self.qa.is_none().then(recto_agent::only_the_user_quits)
    }

    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        self.session.unsaved_tabs()
    }

    fn acted_tab(&self, command: &Command) -> Option<TabId> {
        self.session.acted_tab(command)
    }
}

/// How many calls can wait for the session thread.
const MAX_QUEUED_CALLS: usize = 4096;

/// What the session thread is asked to do, in the order the user asked.
#[derive(Debug)]
enum CoreCall {
    Reconcile,
    Tap(TabId),
    GoTo(TabId, u32),
    Open(PathBuf),
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
#[derive(Default)]
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
        let mut calls = lock(&self.calls);
        // With the session thread held up by a dead volume, this would grow for as long as tiles and clicks kept coming. Past the cap what's asked is dropped, as it's long out of date by the time it could run. A flush is someone waiting, with a limit of their own, so it's kept.
        if calls.len() >= MAX_QUEUED_CALLS && !matches!(call, CoreCall::Flush(_)) {
            return;
        }
        calls.push_back(call);
        drop(calls);
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
    warm: Vec<u32>,
    paper: Option<PaperKind>,
    chosen_paper: Option<Option<Paper>>,
    focus: Option<bool>,
    dark: bool,
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
            published: Mutex::new(Published { contents: Arc::new(Vec::new()), rail_filled: 0, tabs: Arc::new(Vec::new()), sidebar: Arc::new(SidebarView { tabs_version: 0, tab_count: 0, active_index: None, contents_version: 0, contents_count: 0, contents_changed_from: 0, contents_removed: 0, contents_inserted: 0, contents_place_shift: 0, contents_current: None, toc_open: false, place: String::new(), edited: false, undone_owner_steps: false }), reader: ReaderView { home: true, note: None, focus: false, place: String::new(), paper: Paper::White.name().to_string(), paper_rgb: PaperKind::White.rgb(), page_tint: None } }),
            watchdog,
            stopper: Mutex::new(stopper),
            announcer: Mutex::new(announcer),
            dark: AtomicBool::new(false),
            qa,
            paths: Some(paths),
            open_failure: Mutex::new(None),
        });
        // The paper chosen last time. What the file says is only taken if it's one of the words.
        if let Some(paths) = &inner.paths
            && let Ok(Some(bytes)) = recto_platform::local::read_private_file(&paths.root.join(PAPER_FILE), 16)
            && let Some(paper) = std::str::from_utf8(&bytes).ok().and_then(Paper::from_name)
        {
            inner.host.session.with_session(|session| session.set_view(ViewSettings { paper: Some(paper), ..session.view() }));
        }
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
            menu_file: "File".into(),
            menu_view: "View".into(),
        }
    }

    pub fn limits(&self) -> ReaderLimits {
        ReaderLimits { min_zoom: MIN_ZOOM, max_zoom: MAX_ZOOM, zoom_step: 1.25, window_width: 1440.0, window_height: 900.0, sidebar_width: 272.0 }
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
                Some(SidebarRow { kind: RowKind::Contents, tab: entry.tab, text: entry.text.to_string(), page: entry.page, depth: entry.depth, current: current == Some(index), rail_filled: index < rail_filled, rail_first: index == 0, rail_last: index + 1 == total })
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

    /// What hovering over a row reveals: its full text, clean and bounded, and what it is.
    pub fn row_reveal(&self, row: SidebarRow) -> RowReveal {
        let full = row_full_text(&row.text, row.kind == RowKind::Tab);
        RowReveal { text: full.text, is_file_name: full.is_file_name }
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
                // A cold tab's file is read with the session free, as an `open` is, so a dead volume holds up this tap and nothing that asks about tabs.
                if let Some((tab, path)) = host.session.with_session(|session| session.wake_for_tap(id)) {
                    let stamp = recto_platform::file_stamp(&path).ok();
                    let loaded = DrawableLoader.load(&path);
                    host.session.with_session(|session| session.give_woken(tab, loaded, stamp));
                }
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
            CoreCall::SetFocus(focus) => {
                let view = host.session.with_session(|session| {
                    let view = ViewSettings { focus, ..session.view() };
                    session.set_view(view);
                    view
                });
                events.push(Event::new("view_changed", json!({ "paper": recto_agent::paper_name(view), "focus": view.focus })));
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
                // Nothing the sidebar or the reader shows comes from this. Going round for it would rebuild the model per tile drawn.
                if let Some(announcer) = lock(&inner.announcer).as_ref() {
                    announcer.announce(events);
                }
                continue;
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
        let same_tabs = held_tabs.len() == tab_count as usize && tab_rows().zip(held_tabs.iter()).all(|(row, tab)| row.tab.0 == tab.tab && row.current == tab.current && row.text == tab.text.as_str());
        let tabs = if same_tabs {
            held_tabs
        } else {
            Arc::new(tab_rows().map(|row| SidebarRow { kind: RowKind::Tab, tab: row.tab.0, text: row.text.to_string(), page: row.page, depth: row.depth, current: row.current, rail_filled: false, rail_first: false, rail_last: false }).collect())
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
    // The paper and focus first, so what's asked for next is drawn right.
    let dark = inner.dark.load(Ordering::SeqCst) || synced.dark;
    let shown = Paper::resolved(view.paper, dark);
    // Warm paper is white paper with a tint over it in the compositor, so going to it or from it draws nothing and holds nothing more.
    let (paper, page_tint) = match shown {
        Paper::White => (PaperKind::White, None),
        Paper::Warm => (PaperKind::White, Some(WARM_PAPER)),
        Paper::Dark => (PaperKind::Dark, None),
    };
    // Remembered per app, as the state folder is. A write that fails only means it's forgotten next time.
    if synced.chosen_paper != Some(view.paper) {
        if synced.chosen_paper.is_some()
            && let Some(paths) = &inner.paths
        {
            let _ = recto_platform::local::write_private_file(&paths.root.join(PAPER_FILE), recto_agent::paper_name(view).as_bytes());
        }
        synced.chosen_paper = Some(view.paper);
    }
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
                synced.warm = warm;
            } else {
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
    let note = lock(&inner.open_failure).clone().or(problem).or_else(|| inner.engine.note());
    let reader = ReaderView { home: synced.tab.is_none(), note, focus: view.focus, place: sidebar.place.clone(), paper: shown.name().to_string(), paper_rgb: paper.rgb(), page_tint };
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

    /// Failure: with the session thread held up by a file on a dead volume, every drawn tile and click queues for it, and the queue grows without end. It stops at its cap, and someone waiting for a flush still gets in.
    #[test]
    fn the_queue_for_the_session_thread_has_a_cap() {
        let queue = CoreQueue::default();
        for page in 0..(MAX_QUEUED_CALLS as u32 + 100) {
            queue.push(CoreCall::Page(page, None, TabId(1)));
        }
        assert_eq!(lock(&queue.calls).len(), MAX_QUEUED_CALLS);
        queue.push(CoreCall::Flush(Arc::new(Flushed::default())));
        assert_eq!(lock(&queue.calls).len(), MAX_QUEUED_CALLS + 1, "someone is waiting on that");
    }

    /// Failure: an agent at Control quits the app out from under the person using it, with no prompt, by `recto stop`. In a normal app it's refused, and the words are for whoever's asking.
    #[test]
    fn stop_is_refused_by_a_normal_app() {
        let host = AppHost { session: SessionHost::new(DrawableLoader), core: Arc::new(CoreQueue::default()), qa: None };
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
