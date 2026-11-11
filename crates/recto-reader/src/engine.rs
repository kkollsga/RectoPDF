//! One thread that runs what [`Reader`] asks for. It's parked with nothing to do: no timer, no wake-ups. A page blocks this thread and never the UI thread, and the one lock is never held across a render.

use crate::layout::PageBox;
use crate::paper::{DarkTile, PageLook, PaperKind, dark_rgb, dim_page, dim_rgb, is_grey, turn_over};
use crate::selection::Notice;
use crate::tiles::{DrawKind, Job, JobKind, JobPlan, MAX_ATTACH_PER_TURN, Op, Outcome, PageMap, Reader, ReaderReport, SavedPlace, Viewport, Zoom};
use recto_index::select::PageText;
use recto_platform::isolate;
use recto_platform::surface::PixelSurface;
use recto_proto::TileRect;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig, TileKind, TileRequest, TileResult};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// What the engine tells whoever's drawing. Called off the UI thread, and must return at once.
pub trait EngineEvents: Send + Sync {
    /// There are ops for the host.
    fn ops_ready(&self);
    /// Something the host shows from the reader's report changed.
    fn note_changed(&self);
    /// The reader scrolled to another page, for the session to take as its place.
    fn page_moved(&self);
    /// A page was drawn, or couldn't be.
    fn page_drawn(&self, tab: u32, page: u32, failure: Option<&'static str>);
    /// A copy is ready for the host to take.
    fn copy_ready(&self) {}
    /// A file's been seen to be another than the one opened, so the session should look at it.
    fn file_replaced(&self, _tab: u32) {}
    /// The worker has a tab's own file open, and this is how long it says it is, and which version.
    fn file_length(&self, _tab: u32, _pages: u32, _version: u64) {}
}

/// How the engine runs.
#[derive(Clone, Debug, Default)]
pub struct EngineConfig {
    /// Where the worker is. Found beside this program if left out.
    pub service: Option<ServiceConfig>,
    pub policy: Option<Policy>,
    /// Passwords the person has typed for protected files, shared by every page view, for their workers to open those files with.
    pub keys: Option<Arc<Keyring>>,
}

/// **Passwords for protected files, while they're open,** by which file each is for: its device and inode (`recto_platform::secret::file_identity`), not its path, so a file moved while it's open keeps its password, and another file put at its path doesn't get it. In memory only: never written, logged or journaled, and each is overwritten when its last holder lets go (`recto_core::Password`). The app forgets one when no tab has its file open. At most [`MAX_KEYS`].
#[derive(Default)]
pub struct Keyring {
    keys: Mutex<Vec<(FileIdentity, Arc<recto_core::Password>)>>,
}

/// A file's device and inode: which file it is, wherever it's moved to on its volume.
pub type FileIdentity = (u64, u64);

/// Passwords held at once: one per protected file open, and more open protected files than this is past anything a person types passwords for. Past it, the oldest goes, and its file asks again.
pub const MAX_KEYS: usize = 64;

impl std::fmt::Debug for Keyring {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Keyring({} held)", lock(&self.keys).len())
    }
}

impl Keyring {
    /// Keeps a password for the file with this identity, in place of any it had.
    pub fn set(&self, identity: (u64, u64), password: Arc<recto_core::Password>) {
        let mut keys = lock(&self.keys);
        keys.retain(|(known, _)| *known != identity);
        if keys.len() >= MAX_KEYS {
            keys.remove(0);
        }
        keys.push((identity, password));
    }

    /// The password for the file with this identity, if one was typed.
    pub fn get(&self, identity: (u64, u64)) -> Option<Arc<recto_core::Password>> {
        lock(&self.keys)
            .iter()
            .find(|(known, _)| *known == identity)
            .map(|(_, password)| Arc::clone(password))
    }

    /// The password for the file at this path now, if one was typed for it.
    pub fn for_path(&self, path: &Path) -> Option<Arc<recto_core::Password>> {
        if self.is_empty() {
            return None;
        }
        self.get(recto_platform::secret::file_identity(path)?)
    }

    /// Keeps only the passwords for these files, the ones open in tabs: any other goes, and is overwritten.
    pub fn keep_only(&self, open: &[(u64, u64)]) {
        lock(&self.keys).retain(|(identity, _)| open.contains(identity));
    }

    pub fn len(&self) -> usize {
        lock(&self.keys).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Files that can be being opened at once. Past this, more that aren't answering wait for one that is.
const MAX_OPENINGS: usize = 4;
/// How long a job waits for its file to open before the page says so. It stays being tried.
const OPEN_PATIENCE: Duration = Duration::from_secs(3);
const OPEN_LOOK: Duration = Duration::from_millis(20);
/// A page from a PDF added to this one, which has changed since: its page numbers are about the version it was, so what's there now isn't drawn as it.
const ADDED_FILE_CHANGED: &str = "This page comes from a PDF that has changed or moved since it was added, so it isn't drawn.";
/// What a page says while its file doesn't answer.
const NOT_ANSWERING: &str = "This file isn't answering. Its disk may have gone to sleep or been disconnected.";

/// Panics of the thread that draws that it comes back from, close together. Past this it would only panic again.
const MAX_PANICS_IN_A_ROW: u32 = 4;
/// A panic this long after the last one is a new story.
const PANICS_FORGOTTEN_AFTER: Duration = Duration::from_secs(60);

/// How long a worker past the retirement line may sit with nothing to do. As long as a heavy worker's idle rule, for the same reason.
const BIG_WORKER_REST: Duration = Duration::from_secs(3);

struct Shared {
    reader: Mutex<Reader>,
    /// Woken when a job ends or the host says it has applied ops, for whoever's waiting to see everything settled.
    settled: Condvar,
    events: Arc<dyn EngineEvents>,
    ops_signalled: AtomicBool,
    worker_pids: Mutex<Vec<u32>>,
    /// Of those, the workers that said their walls were up.
    walled_workers: AtomicU32,
    workers_by_file: Mutex<Vec<(PathBuf, u32)>>,
    /// The system's asked for memory back: workers with nothing to do go the next time this thread comes round.
    kill_idle_workers: AtomicBool,
    last_note: Mutex<Option<String>>,
    last_notice: Mutex<Option<Notice>>,
    config: EngineConfig,
    /// This page view's done with, as a comparison's pane is when it closes: its thread ends, and its workers and everything it drew go with it.
    stopping: AtomicBool,
    /// What a comparison's other panes asked of this view from the UI thread while its own thread held it: the latest of each, taken by that thread the next time it holds its lock. See [`Engine::follow_soon`].
    pending: Mutex<Pending>,
}

/// What's waiting for a page view that was busy when the UI thread asked. Only the latest of each counts.
#[derive(Default)]
struct Pending {
    /// Synced scroll's place: a page and how far down it.
    follow: Option<(u32, f64)>,
    /// The one zoom of a comparison.
    zoom: Option<f64>,
    /// Another pane took the selection.
    clear_selection: bool,
}

/// The page view's running half.
#[derive(Clone)]
pub struct Engine {
    shared: Arc<Shared>,
    thread: Arc<Mutex<Option<Thread>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Engine {
    pub fn new(config: EngineConfig, events: Arc<dyn EngineEvents>) -> Self {
        let shared = Arc::new(Shared {
            reader: Mutex::new(Reader::new()),
            settled: Condvar::new(),
            events,
            ops_signalled: AtomicBool::new(false),
            worker_pids: Mutex::new(Vec::new()),
            walled_workers: AtomicU32::new(0),
            workers_by_file: Mutex::new(Vec::new()),
            kill_idle_workers: AtomicBool::new(false),
            last_note: Mutex::new(None),
            last_notice: Mutex::new(None),
            config,
            stopping: AtomicBool::new(false),
            pending: Mutex::new(Pending::default()),
        });
        Self {
            shared,
            thread: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts the thread the first time there's a document, and wakes it. `unpark` costs one atomic when it's already running.
    fn wake(&self) {
        if self.shared.stopping.load(Ordering::SeqCst) {
            return;
        }
        let mut thread = lock(&self.thread);
        match &*thread {
            Some(running) => running.unpark(),
            None => {
                let shared = Arc::clone(&self.shared);
                // Small: everything it holds is on the heap.
                if let Ok(handle) = std::thread::Builder::new()
                    .name("recto-pages".into())
                    .stack_size(512 * 1024)
                    .spawn(move || run_guarded(&shared))
                {
                    *thread = Some(handle.thread().clone());
                }
            }
        }
    }

    fn after(&self, mut reader: MutexGuard<'_, Reader>) {
        let has_ops = reader.has_ops();
        let note = reader.note().map(str::to_string);
        let selecting = selecting_news(&mut reader);
        drop(reader);
        self.shared.settled.notify_all();
        note_change(&self.shared, note);
        tell_selecting(&self.shared, selecting);
        if has_ops {
            signal_ops(&self.shared);
        }
        self.wake();
    }

    /// **Ends this page view for good:** what's on screen comes off, its thread ends at its next turn, and its workers and everything drawn go with it. A comparison's pane closing does this, so a pane that's gone costs no thread, worker or surface.
    pub fn shut_down(&self) {
        let mut reader = lock(&self.shared.reader);
        *lock(&self.shared.pending) = Pending::default();
        reader.deactivate();
        drop(reader);
        self.shared.stopping.store(true, Ordering::SeqCst);
        if let Some(thread) = &*lock(&self.thread) {
            thread.unpark();
        }
    }

    /// Whether this page view's thread is running, for QA: a pane that's closed has none.
    pub fn has_thread(&self) -> bool {
        lock(&self.thread).is_some() && !self.shared.stopping.load(Ordering::SeqCst)
    }

    /// Shares what's kept off screen with this many page views in all.
    pub fn set_share(&self, views: usize) {
        lock(&self.shared.reader).set_share(views);
    }

    /// The least room either side of the pages, as a comparison's pane has it.
    pub fn set_margins(&self, left: f64, right: f64) {
        let mut reader = lock(&self.shared.reader);
        reader.set_margins(left, right);
        self.after(reader);
    }

    /// The tab showing's magnification.
    pub fn magnification(&self) -> Option<f64> {
        lock(&self.shared.reader).magnification()
    }

    /// Where the reader is in the tab showing, as a page and how far down it.
    pub fn reading_anchor(&self) -> Option<(u32, f64)> {
        lock(&self.shared.reader).reading_anchor()
    }

    /// **Synced scroll, from the UI thread, which never waits for it.** If the page view's free, the view goes there now, and anything left before is dropped, as this is newer. If its thread holds it, as it does to take a finished tile, the place is left for that thread to take as it lets go, the latest only, so a scroll step costs a look at a lock and never a wait on one. Says whether it went now.
    pub fn follow_soon(&self, page: u32, within: f64) -> bool {
        self.now_or_soon(
            |pending| pending.follow = Some((page, within)),
            |reader, pending| {
                pending.follow = None;
                reader.follow(page, within);
            },
        )
    }

    /// The comparison's one zoom, from the UI thread, the same way: now if the page view's free, and otherwise left for its thread, the latest only.
    pub fn zoom_soon(&self, value: f64) -> bool {
        self.now_or_soon(
            |pending| pending.zoom = Some(value),
            |reader, pending| {
                pending.zoom = None;
                reader.zoom(Zoom::To(value));
            },
        )
    }

    /// Another pane took the selection, from the UI thread, the same way.
    pub fn clear_selection_soon(&self) -> bool {
        self.now_or_soon(
            |pending| pending.clear_selection = true,
            |reader, pending| {
                pending.clear_selection = false;
                reader.clear_selection();
            },
        )
    }

    /// Does something with the page view's lock if it's free, and otherwise leaves it for its own thread. Never waits for the lock: the UI thread calls this.
    fn now_or_soon(&self, leave: impl FnOnce(&mut Pending), now: impl FnOnce(&mut Reader, &mut Pending)) -> bool {
        let reader = match self.shared.reader.try_lock() {
            Ok(reader) => reader,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                leave(&mut lock(&self.shared.pending));
                self.wake();
                return false;
            }
        };
        let mut reader = reader;
        now(&mut reader, &mut lock(&self.shared.pending));
        self.after(reader);
        true
    }

    /// Sends the view to a place in a page, for synced scroll. Says which page, held to the document.
    pub fn follow(&self, page: u32, within: f64) -> Option<u32> {
        let mut reader = lock(&self.shared.reader);
        let went = reader.follow(page, within);
        self.after(reader);
        went
    }

    /// Whether two handles are the same page view.
    pub fn same_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Which tab this page view shows.
    pub fn hot_tab(&self) -> Option<u32> {
        lock(&self.shared.reader).hot_tab()
    }

    /// Makes a tab the visible one. Off the UI thread, as the session decides this.
    #[allow(clippy::too_many_arguments)]
    pub fn activate(&self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32], saved: Option<SavedPlace>, spread: crate::layout::Spread) {
        let mut reader = lock(&self.shared.reader);
        // A place synced scroll left for what was showing isn't about what's coming: it would land a middle pane's place on the tab that moves into it.
        lock(&self.shared.pending).follow = None;
        reader.activate(tab, pages, page, map, warm, saved, spread);
        self.after(reader);
    }

    /// Lays the tab showing out another way, keeping the place in it.
    pub fn set_spread(&self, spread: crate::layout::Spread) {
        let mut reader = lock(&self.shared.reader);
        reader.set_spread(spread);
        self.after(reader);
    }

    /// What a tab's first frame shows, for its next [`Self::activate`].
    pub fn give_sleep(&self, tab: u32, view: crate::tiles::SleepView) {
        lock(&self.shared.reader).give_sleep(tab, view);
    }

    /// The window's gone up.
    pub fn window_up(&self) {
        lock(&self.shared.reader).window_up();
    }

    /// A tab's being woken, as of now.
    pub fn note_wake(&self, tab: u32, began: Instant) {
        lock(&self.shared.reader).note_wake(tab, began);
    }

    /// Whether a view of this tab's held.
    pub fn holds(&self, tab: u32) -> bool {
        lock(&self.shared.reader).holds(tab)
    }

    /// What a tab's sleep file gets from here.
    pub fn sleep_material(&self, tab: u32, own: Option<recto_platform::FileStamp>) -> Option<crate::tiles::SleepMaterial> {
        lock(&self.shared.reader).sleep_material(tab, own)
    }

    /// For QA: nothing's drawn till this is let go, so a first frame stays to be looked at.
    pub fn hold_drawing(&self, on: bool) {
        lock(&self.shared.reader).hold_drawing(on);
        self.wake();
    }

    /// Whether a window can go up with what's on the layers.
    pub fn is_showable(&self) -> bool {
        lock(&self.shared.reader).is_showable()
    }

    /// The system wants memory back, so what's off screen goes, and workers with nothing to do.
    pub fn memory_pressure(&self, critical: bool) {
        lock(&self.shared.reader).memory_pressure(critical);
        self.shared.kill_idle_workers.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// For QA: the system taking back what's marked discardable, without a word. Says how many surfaces went.
    pub fn empty_discardable(&self) -> usize {
        lock(&self.shared.reader).empty_discardable()
    }

    /// What this tab's files' workers hold now, from outside. A call to the kernel for each, of which there are few.
    pub fn worker_bytes(&self, tab: u32) -> u64 {
        let files = lock(&self.shared.reader).files_of(tab);
        lock(&self.shared.workers_by_file)
            .iter()
            .filter(|(path, _)| files.contains(path))
            .filter_map(|(_, pid)| isolate::process_memory(*pid))
            .map(|memory| memory.footprint_bytes)
            .sum()
    }

    /// Exactly where each tab it holds is. A short lock.
    pub fn places(&self) -> Vec<(u32, SavedPlace)> {
        lock(&self.shared.reader).places()
    }

    /// Whether the place is on the layers, and nothing's on its way. A short lock, asked only until the window is showing.
    pub fn is_on_place(&self) -> bool {
        lock(&self.shared.reader).is_on_place()
    }

    /// Zooms, wherever it's asked from. Says what to.
    pub fn zoom(&self, zoom: Zoom) -> Option<f64> {
        let mut reader = lock(&self.shared.reader);
        let value = reader.zoom(zoom);
        self.after(reader);
        value
    }

    /// The tab showing's fit, document points per PDF point, if a tab is showing: what a zoom level is worked out from.
    pub fn fit(&self) -> Option<f64> {
        lock(&self.shared.reader).fit()
    }

    /// Which tabs are warm now, with no change of tab. Anything that's stopped being warm lets go of what it holds.
    pub fn set_warm(&self, warm: &[u32]) {
        let mut reader = lock(&self.shared.reader);
        reader.set_warm(warm);
        self.after(reader);
    }

    pub fn deactivate(&self) {
        let mut reader = lock(&self.shared.reader);
        lock(&self.shared.pending).follow = None;
        reader.deactivate();
        self.after(reader);
    }

    /// Files whose renderer found they need a password it has none for: see [`Reader::take_keyless`].
    pub fn take_keyless(&self) -> Vec<PathBuf> {
        lock(&self.shared.reader).take_keyless()
    }

    /// A password's been given for a file: see [`Reader::key_given`].
    pub fn key_given(&self, path: &Path) {
        let mut reader = lock(&self.shared.reader);
        reader.key_given(path);
        self.after(reader);
    }

    pub fn jump_to(&self, page: u32) {
        let mut reader = lock(&self.shared.reader);
        reader.jump_to(page);
        self.after(reader);
    }

    /// A jump to a place in a page, as a link or Back goes: the page's top, margin above the first page included, or that far down it.
    pub fn jump_within(&self, page: u32, within: f64) {
        let mut reader = lock(&self.shared.reader);
        if within.is_finite() && within > 0.0 {
            lock(&self.shared.pending).follow = None;
            reader.follow(page, within);
        } else {
            reader.jump_to(page);
        }
        self.after(reader);
    }

    /// The link under the pointer, if its page's links are here. A page not read yet is read next. A short lock, on the UI thread.
    pub fn pointer_link(&self, x: f64, y: f64) -> Option<crate::links::LinkHit> {
        let mut reader = lock(&self.shared.reader);
        let hit = reader.pointer_link(x, y);
        drop(reader);
        self.wake();
        hit
    }

    /// The link the last click landed on, taken once.
    pub fn take_clicked_link(&self) -> Option<crate::links::LinkHit> {
        lock(&self.shared.reader).take_clicked_link()
    }

    /// Where a link's page is now: see [`Reader::position_in_same_file`].
    pub fn position_in_same_file(&self, from: u32, file_page: u32) -> Option<u32> {
        lock(&self.shared.reader).position_in_same_file(from, file_page)
    }

    /// Which page of the tab's own file a page of the view is, if it's one.
    pub fn own_file_page(&self, position: u32) -> Option<u32> {
        lock(&self.shared.reader).own_file_page(position)
    }

    /// Pages whose links are held, for QA.
    pub fn link_pages_held(&self) -> usize {
        lock(&self.shared.reader).link_pages_held()
    }

    pub fn set_paper(&self, paper: PaperKind) {
        let mut reader = lock(&self.shared.reader);
        reader.set_paper(paper);
        self.after(reader);
    }

    /// From the UI thread, on every scroll. A short lock and a few map lookups. What comes back is for the host to apply now.
    pub fn set_viewport(&self, viewport: Viewport) -> Vec<Op> {
        self.set_viewport_scrolled(viewport).ops
    }

    /// The same, and what synced scroll and zoom need of it, from the one lock: whether it was the reader scrolling, the magnification the view's taken, and where the reader is.
    pub fn set_viewport_scrolled(&self, viewport: Viewport) -> ViewportOutcome {
        // What this returns is what was waiting, so nothing's waiting to be told about.
        self.shared.ops_signalled.store(false, Ordering::SeqCst);
        let mut reader = lock(&self.shared.reader);
        let ops = reader.set_viewport(viewport);
        let scrolled = reader.take_user_scrolled();
        if scrolled {
            // The reader's own scroll here is newer than any place synced scroll left for this pane.
            lock(&self.shared.pending).follow = None;
        }
        let (magnification, anchor) = (reader.magnification(), reader.reading_anchor());
        let moved = reader.has_moved_page();
        let more = reader.has_ops();
        let note = reader.note().map(str::to_string);
        drop(reader);
        note_change(&self.shared, note);
        if moved {
            self.shared.events.page_moved();
        }
        if more {
            signal_ops(&self.shared);
        }
        self.wake();
        ViewportOutcome {
            ops,
            scrolled,
            magnification,
            anchor,
        }
    }

    /// Runs what the mouse did, from the UI thread: a short lock, and at worst a look through one page already here. What comes back is for the host to apply now.
    fn pointer_did(&self, did: impl FnOnce(&mut Reader)) -> Vec<Op> {
        self.shared.ops_signalled.store(false, Ordering::SeqCst);
        let mut reader = lock(&self.shared.reader);
        did(&mut reader);
        let ops = reader.drain(0);
        let more = reader.has_ops();
        let selecting = selecting_news(&mut reader);
        drop(reader);
        tell_selecting(&self.shared, selecting);
        if more {
            signal_ops(&self.shared);
        }
        self.wake();
        ops
    }

    pub fn selection_press(&self, x: f64, y: f64, clicks: u32, shift: bool) -> Vec<Op> {
        self.pointer_did(|reader| reader.selection_press(x, y, clicks, shift))
    }

    pub fn selection_drag(&self, x: f64, y: f64) -> Vec<Op> {
        self.pointer_did(|reader| reader.selection_drag(x, y))
    }

    pub fn selection_release(&self) -> Vec<Op> {
        self.pointer_did(Reader::selection_release)
    }

    /// Whether the selection can be sent to the Services menu as it stands, which it's asked on the UI thread. Nothing's built.
    pub fn selection_can_be_sent(&self) -> bool {
        lock(&self.shared.reader).selection_can_be_sent()
    }

    /// Whether the pointer's on text. If its page hasn't been read, it's wanted, and this says no for now.
    pub fn pointer_is_over_text(&self, x: f64, y: f64) -> bool {
        let mut reader = lock(&self.shared.reader);
        let over = reader.pointer_is_over_text(x, y);
        drop(reader);
        self.wake();
        over
    }

    pub fn select_all(&self) {
        let mut reader = lock(&self.shared.reader);
        reader.select_all();
        self.after(reader);
    }

    pub fn clear_selection(&self) {
        let mut reader = lock(&self.shared.reader);
        reader.clear_selection();
        self.after(reader);
    }

    /// Which tab is showing, and what's selected on it. A short lock, for the state record.
    pub fn hot_selection(&self) -> Option<(u32, Option<recto_index::select::Range>)> {
        let reader = lock(&self.shared.reader);
        reader.hot_tab().map(|tab| (tab, reader.selection_range()))
    }

    /// Puts back what was selected, from the state record.
    pub fn restore_selection(&self, from: (u32, u32), to: (u32, u32)) {
        let mut reader = lock(&self.shared.reader);
        reader.restore_selection(recto_index::select::Range {
            start: recto_index::select::Place { page: from.0, at: from.1 },
            end: recto_index::select::Place { page: to.0, at: to.1 },
        });
        self.after(reader);
    }

    pub fn has_selection(&self) -> bool {
        lock(&self.shared.reader).has_selection()
    }

    /// Selects Find's hit and brings it into view: characters `start..end` of a page, from 0. Off the UI thread.
    pub fn reveal(&self, page: u32, start: u32, end: u32) {
        let mut reader = lock(&self.shared.reader);
        reader.reveal_range(recto_index::select::Range {
            start: recto_index::select::Place { page, at: start },
            end: recto_index::select::Place { page, at: end },
        });
        self.after(reader);
    }

    pub fn has_document(&self) -> bool {
        lock(&self.shared.reader).has_document()
    }

    /// Starts the selection on its way to the clipboard. It goes page by page on the engine's thread, and `EngineEvents::copy_ready` says when.
    pub fn copy_selection(&self, anyway: bool) {
        let mut reader = lock(&self.shared.reader);
        reader.start_copy(anyway);
        self.after(reader);
    }

    pub fn take_copied(&self) -> Option<recto_index::select::CopiedText> {
        let mut reader = lock(&self.shared.reader);
        let copied = reader.take_copied();
        drop(reader);
        self.shared.settled.notify_all();
        copied
    }

    pub fn selection_text_now(&self) -> Option<String> {
        lock(&self.shared.reader).selection_text_now()
    }

    pub fn selection_notice(&self) -> Option<Notice> {
        lock(&self.shared.reader).selection_notice()
    }

    pub fn dismiss_selection_notice(&self) {
        let mut reader = lock(&self.shared.reader);
        reader.dismiss_selection_notice();
        self.after(reader);
    }

    /// Where a point in the document is on a page as it's shown, with that page's size, for the snapshot tool. A short lock and some sums.
    pub fn point_on_page(&self, x: f64, y: f64) -> Option<(recto_index::select::PagePoint, (f32, f32))> {
        let reader = lock(&self.shared.reader);
        let point = reader.point_on_page(x, y)?;
        Some((point, reader.page_size_shown(point.page)?))
    }

    /// Where a point in the document is on `page`, whichever page it's over, past its edges if it must: a snapshot's box stays on the page it was pressed on. A short lock and some sums.
    pub fn point_on_given_page(&self, page: u32, x: f64, y: f64) -> Option<recto_index::select::PagePoint> {
        lock(&self.shared.reader).point_on_given_page(page, x, y)
    }

    /// How many pages the tab shown has, edits and all. A short lock, for what can't wait, as the print panel can't.
    pub fn page_count(&self) -> u32 {
        lock(&self.shared.reader).page_count()
    }

    /// A page's size as it's shown, in its own coordinates. A short lock.
    pub fn page_size_shown(&self, page: u32) -> Option<(f32, f32)> {
        lock(&self.shared.reader).page_size_shown(page)
    }

    /// How far a page is shown turned, in quarter turns clockwise. A short lock.
    pub fn page_turns(&self, page: u32) -> u8 {
        lock(&self.shared.reader).page_turns(page)
    }

    /// Where a point on a page is in the document.
    pub fn document_point(&self, page: u32, x: f64, y: f64) -> Option<(f64, f64)> {
        lock(&self.shared.reader).document_point(page, x, y)
    }

    pub fn take_moved_page(&self) -> Option<u32> {
        lock(&self.shared.reader).take_moved_page()
    }

    /// From the UI thread, when told there are ops.
    pub fn drain(&self) -> Vec<Op> {
        self.shared.ops_signalled.store(false, Ordering::SeqCst);
        let mut reader = lock(&self.shared.reader);
        let ops = reader.drain(MAX_ATTACH_PER_TURN);
        let more = reader.has_ops();
        drop(reader);
        if more {
            signal_ops(&self.shared);
        }
        ops
    }

    /// From the UI thread, once applied.
    pub fn applied(&self) {
        lock(&self.shared.reader).applied();
        self.shared.settled.notify_all();
    }

    pub fn window_closed(&self) {
        let mut reader = lock(&self.shared.reader);
        reader.window_closed();
        self.after(reader);
    }

    pub fn window_opened(&self) {
        // Whatever was said while the window was closed had no host to take it. If that's still raised, nobody hears of anything from here on.
        self.shared.ops_signalled.store(false, Ordering::SeqCst);
        let mut reader = lock(&self.shared.reader);
        reader.window_opened();
        self.after(reader);
    }

    pub fn note(&self) -> Option<String> {
        lock(&self.shared.reader).note().map(str::to_string)
    }

    pub fn report(&self) -> ReaderReport {
        lock(&self.shared.reader).report()
    }

    /// Blocks until everything visible is settled, or the time runs out. Never for the UI thread.
    pub fn wait_settled(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut reader = lock(&self.shared.reader);
        loop {
            if reader.is_settled() {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            reader = self.shared.settled.wait_timeout(reader, left).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Every worker running now.
    pub fn worker_pids(&self) -> Vec<u32> {
        lock(&self.shared.worker_pids).clone()
    }

    /// How many of the workers running now said their walls were up: the App Sandbox on and their Mach rights dropped.
    pub fn walled_workers(&self) -> u32 {
        self.shared.walled_workers.load(Ordering::Relaxed)
    }

    /// The footprint of every worker running now, from outside.
    pub fn workers_footprint(&self) -> (u64, u64) {
        let pids = lock(&self.shared.worker_pids).clone();
        pids.iter()
            .filter_map(|pid| isolate::process_memory(*pid))
            .fold((0, 0), |(now, peak), memory| (now + memory.footprint_bytes, peak + memory.peak_footprint_bytes))
    }
}

/// What a viewport did, for a comparison's other panes.
pub struct ViewportOutcome {
    pub ops: Vec<Op>,
    /// It was the reader's own scroll.
    pub scrolled: bool,
    pub magnification: Option<f64>,
    pub anchor: Option<(u32, f64)>,
}

/// Takes what the UI thread left for this view, with the lock held. Says whether there was any.
fn take_pending_follow(shared: &Shared, reader: &mut Reader) -> bool {
    let pending = std::mem::take(&mut *lock(&shared.pending));
    if let Some(value) = pending.zoom {
        reader.zoom(Zoom::To(value));
    }
    if pending.clear_selection {
        reader.clear_selection();
    }
    if let Some((page, within)) = pending.follow {
        reader.follow(page, within);
    }
    pending.zoom.is_some() || pending.clear_selection || pending.follow.is_some()
}

fn note_change(shared: &Shared, note: Option<String>) {
    let mut last = lock(&shared.last_note);
    if *last != note {
        *last = note;
        drop(last);
        shared.events.note_changed();
    }
}

/// What a selection has to say to anyone, taken with the reader's lock, to say with it let go.
struct SelectingNews {
    notice: Option<Notice>,
    copy_ready: bool,
}

fn selecting_news(reader: &mut Reader) -> SelectingNews {
    SelectingNews {
        notice: reader.selection_notice(),
        copy_ready: reader.take_copy_signal(),
    }
}

fn tell_selecting(shared: &Shared, news: SelectingNews) {
    let mut last = lock(&shared.last_notice);
    if *last != news.notice {
        *last = news.notice;
        drop(last);
        shared.events.note_changed();
    }
    if news.copy_ready {
        shared.events.copy_ready();
    }
}

fn signal_ops(shared: &Shared) {
    if !shared.ops_signalled.swap(true, Ordering::SeqCst) {
        shared.events.ops_ready();
    }
}

/// Goes round [`run`] again if it panics, so a panic can't leave pages silently undrawn with nothing left to draw them. What's held in it, workers included, goes with it, and pages are asked for again. A few in a row means trying again won't help, and the page says so.
fn run_guarded(shared: &Shared) {
    let mut panics_in_a_row = 0;
    let mut last_panic: Option<Instant> = None;
    loop {
        // Everything shared is behind locks that don't care about poison, and is brought back into line below.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(shared))).is_ok() {
            return;
        }
        panics_in_a_row = if last_panic.is_some_and(|last| last.elapsed() < PANICS_FORGOTTEN_AFTER) {
            panics_in_a_row + 1
        } else {
            1
        };
        last_panic = Some(Instant::now());
        let give_up = panics_in_a_row >= MAX_PANICS_IN_A_ROW;
        let mut reader = lock(&shared.reader);
        reader.thread_restarted(give_up);
        let has_ops = reader.has_ops();
        let note = reader.note().map(str::to_string);
        let selecting = selecting_news(&mut reader);
        drop(reader);
        note_change(shared, note);
        tell_selecting(shared, selecting);
        *lock(&shared.worker_pids) = Vec::new();
        shared.walled_workers.store(0, Ordering::Relaxed);
        shared.settled.notify_all();
        if has_ops {
            signal_ops(shared);
        }
        if give_up {
            return;
        }
    }
}

fn run(shared: &Shared) {
    let mut renderers: HashMap<PathBuf, OpenDoc> = HashMap::new();
    let mut wanted = lock(&shared.reader).wanted_files();
    let service = shared.config.service.clone().map_or_else(ServiceConfig::discover, Ok);
    let policy = shared.config.policy.clone().unwrap_or_default();
    let mut resting_since: Option<Instant> = None;
    let mut openings: HashMap<PathBuf, Receiver<Result<File, RenderFailure>>> = HashMap::new();
    loop {
        if shared.stopping.load(Ordering::SeqCst) {
            // Dropping them kills each worker and closes its file.
            renderers.clear();
            publish_workers(shared, &mut renderers);
            return;
        }
        let plan = {
            let mut reader = lock(&shared.reader);
            let files = reader.wanted_files();
            if !Arc::ptr_eq(&files, &wanted) {
                wanted = files;
            }
            // Dropping one kills its worker and closes its file, and the next job opens what's there now.
            for file in reader.take_reopen_files() {
                renderers.remove(&file);
                openings.remove(&file);
            }
            if take_pending_follow(shared, &mut reader) && reader.has_ops() {
                signal_ops(shared);
            }
            reader.next_plan()
        };
        // Its surface is made with the lock let go, which the UI thread needs on every scroll.
        let job = plan.and_then(JobPlan::ready);
        // Only hot and warm tabs' files keep a renderer. Dropping one kills its worker and closes its file.
        renderers.retain(|path, _| wanted.contains(path));
        openings.retain(|path, _| wanted.contains(path));
        publish_workers(shared, &mut renderers);
        let Some(job) = job else {
            shared.settled.notify_all();
            if shared.kill_idle_workers.swap(false, Ordering::SeqCst) {
                for open in renderers.values_mut() {
                    open.doc.kill_worker();
                }
                publish_workers(shared, &mut renderers);
            }
            // With nothing to do, this thread is asleep and costs nothing. The one exception: a worker holding a heavy page's memory, which only goes when it does. It gets long enough to bridge a pause in scrolling, and then it's let go, so a heavy page at rest doesn't hold hundreds of megabytes for the 30 s of the idle rule.
            let big = renderers
                .values_mut()
                .any(|open| open.doc.worker_footprint().is_some_and(|footprint| footprint > policy.retire_bytes));
            if !big {
                std::thread::park();
                resting_since = None;
                continue;
            }
            let since = *resting_since.get_or_insert_with(Instant::now);
            let left = BIG_WORKER_REST.saturating_sub(since.elapsed());
            if left.is_zero() {
                for open in renderers.values_mut() {
                    if open.doc.worker_footprint().is_some_and(|footprint| footprint > policy.retire_bytes) {
                        open.doc.kill_worker();
                    }
                }
                resting_since = None;
            } else {
                std::thread::park_timeout(left);
            }
            continue;
        };
        resting_since = None;
        lock(&shared.reader).note_started(&job, None);
        if matches!(job.kind, JobKind::Links) {
            // Read from the file's structure, here and with no worker. Only a file a worker already has open and answering, so a disk gone to sleep holds this thread no longer than a draw would: a page that isn't drawn yet has its links read when it is.
            // Through the renderer's own open file and its password, so the links are of the version being drawn, a protected file's too. A file seen replaced and not taken up yet offers none: they'd be the new version's over the old pages.
            let links = renderers
                .get(&job.file)
                .filter(|open| !open.replaced && !open.doc.file_seen_changed())
                .and_then(|open| {
                    open.doc.read_structure(|file, password| {
                        recto_core::nav::page_links_in(file, password, &[job.source_page])
                            .ok()
                            .and_then(|read| read.into_iter().next())
                            .map(|(_, links)| Arc::new(links))
                    })
                });
            let mut reader = lock(&shared.reader);
            let outcome = match links {
                Some(found) => Outcome::Links(found),
                None => Outcome::Cancelled,
            };
            reader.finish(job, outcome);
            drop(reader);
            shared.settled.notify_all();
            continue;
        }
        let mut file_changed = false;
        // **An added PDF that's changed isn't drawn from** (the saved point's review): a stat, before each job, since drawing goes by path.
        let expected = lock(&shared.reader).expected_stamp(job.tab, &job.file);
        let added_changed =
            expected.is_some_and(|expected| !recto_platform::file_stamp(&job.file).is_ok_and(|now| now.same_contents(expected) && now.inode == expected.inode));
        let outcome = if added_changed {
            match job.kind {
                JobKind::Text { .. } => Outcome::TextFailed,
                _ => Outcome::PageFailed {
                    message: ADDED_FILE_CHANGED.to_string(),
                    may_pass: false,
                },
            }
        } else {
            match renderer_for(shared, &mut renderers, &mut openings, &job.file, &service, &policy) {
                Ok(open) => {
                    lock(&shared.reader).set_cancel(open.doc.cancel_handle());
                    let outcome = do_job(open, &job);
                    file_changed = seen_replaced(open) | std::mem::take(&mut open.replaced);
                    if file_changed {
                        open.looks.clear();
                        open.length_told = false;
                        shared.events.file_replaced(job.tab);
                    }
                    // Which version a worker has, before what it drew is kept, so that's what it's kept as.
                    if let Some(version) = open.doc.known_version() {
                        lock(&shared.reader).note_version(&job.file, version);
                    }
                    if !open.length_told
                        && let (Some(pages), Some(version)) = (open.doc.known_page_count(), open.doc.known_version())
                    {
                        open.length_told = true;
                        if lock(&shared.reader).is_own_file(job.tab, &job.file) {
                            shared.events.file_length(job.tab, pages, version);
                        }
                    }
                    outcome
                }
                Err(Unopened::Failed(failure)) => document_failed(failure),
                Err(Unopened::NotWanted) => Outcome::Cancelled,
                Err(Unopened::NotAnswering) => Outcome::PageFailed {
                    message: NOT_ANSWERING.to_string(),
                    may_pass: true,
                },
            }
        };
        publish_workers(shared, &mut renderers);
        let drawn = match (&job.kind, &outcome) {
            (
                JobKind::Draw {
                    what: DrawKind::Tile | DrawKind::Region,
                    ..
                },
                Outcome::Drawn { .. },
            ) => Some(None),
            (JobKind::Draw { .. }, Outcome::PageFailed { .. }) => Some(Some("page_failed")),
            _ => None,
        };
        let (tab, page) = (job.tab, job.position);
        let mut reader = lock(&shared.reader);
        if file_changed {
            reader.file_changed(tab);
        }
        // What a text job read isn't about what's selected now, if that's gone with the file.
        let outcome = if file_changed && matches!(job.kind, JobKind::Text { .. }) {
            Outcome::Cancelled
        } else {
            outcome
        };
        reader.finish(job, outcome);
        take_pending_follow(shared, &mut reader);
        let has_ops = reader.has_ops();
        let note = reader.note().map(str::to_string);
        let selecting = selecting_news(&mut reader);
        drop(reader);
        note_change(shared, note);
        tell_selecting(shared, selecting);
        if let Some(failure) = drawn {
            shared.events.page_drawn(tab, page, failure);
        }
        if has_ops {
            signal_ops(shared);
        }
        shared.settled.notify_all();
    }
}

fn publish_workers(shared: &Shared, renderers: &mut HashMap<PathBuf, OpenDoc>) {
    let mut walled = 0;
    let by_file: Vec<(PathBuf, u32)> = renderers
        .iter_mut()
        .filter_map(|(path, open)| {
            let pid = open.doc.worker_pid()?;
            walled += u32::from(open.doc.worker_walled() == Some(true));
            Some((path.clone(), pid))
        })
        .collect();
    shared.walled_workers.store(walled, Ordering::Relaxed);
    *lock(&shared.worker_pids) = by_file.iter().map(|(_, pid)| *pid).collect();
    *lock(&shared.workers_by_file) = by_file;
}

/// A file's renderer, and what's been decided about its pages on dark paper. Each page gets one decision, however many tiles it comes to, at whatever scale.
struct OpenDoc {
    /// Whoever's asked has been told how long this file is.
    length_told: bool,
    doc: DocRenderer,
    /// Of the file that's open now. A replacement clears them before anything goes by them: see [`seen_replaced`].
    looks: HashMap<u32, Arc<PageLook>>,
    /// The file's been seen to be another one in the middle of a job, and whoever's asked hasn't been told yet.
    replaced: bool,
}

/// **The renderer has seen its file replaced: what was decided about the old one's pages goes, now, before anything goes by it.** The renderer opens the new file at the start of whichever job comes next, a draw included, so the tile that job draws is of the new file, and the look it's put on dark paper by has to be too. Says whether it had.
fn seen_replaced(open: &mut OpenDoc) -> bool {
    if !open.doc.take_file_changed() {
        return false;
    }
    open.looks.clear();
    open.replaced = true;
    true
}

/// Pages whose look is remembered. Many more than the tiles and previews kept, so a page can't have tiles on the screen from before it was forgotten. Each is at most 64 boxes.
const MAX_LOOKS: usize = 4096;
/// The longest side of the picture of a page that's drawn to judge it, if there's no preview to use. Enough to see paper between lines of text, and a mebibyte of memory for a moment.
const LOOK_PICTURE_EDGE: f32 = 512.0;

/// The look of a page: known, or decided now from what's on hand, a preview if this job's drawn one, otherwise a small picture drawn for it. A page with no images needs neither. `None` means this job went stale on the way, which says nothing about the page, and nothing is kept.
fn page_look(open: &mut OpenDoc, page: u32, generation: u64, whole: Option<(&PixelSurface, TileRect)>) -> Option<Arc<PageLook>> {
    // The places of its images are what a look was decided from, and they may not come out the same twice, so neither of a page's tiles may go by an old answer.
    if open.doc.take_image_places_cleared() {
        open.looks.clear();
    }
    if let Some(known) = open.looks.get(&page) {
        return Some(Arc::clone(known));
    }
    let boxes = open.doc.image_boxes(page, generation);
    if open.doc.is_stale(generation) {
        return None;
    }
    let look = if boxes.as_ref().is_some_and(Vec::is_empty) {
        PageLook::text()
    } else if let Some((surface, rect)) = whole {
        let stride = surface.stride() as usize;
        surface
            .with_pixels(|pixels| PageLook::decide(boxes.as_deref(), pixels, stride, rect.width, rect.height))
            .unwrap_or_else(|_| PageLook::unknown())
    } else {
        let doc = &mut open.doc;
        let picture = doc.page_size(page).ok().and_then(|(width, height)| {
            let scale = (LOOK_PICTURE_EDGE / width.max(height).max(1.0)).clamp(recto_proto::MIN_SCALE, recto_proto::MAX_SCALE);
            let (pixel_width, pixel_height) = recto_proto::device_size(width, height, scale).ok()?;
            let surface = PixelSurface::create(pixel_width, pixel_height).ok()?;
            Some((
                scale,
                TileRect {
                    x: 0,
                    y: 0,
                    width: pixel_width,
                    height: pixel_height,
                },
                surface,
            ))
        });
        match picture {
            Some((scale, rect, surface)) => match doc.render_tile(
                &TileRequest {
                    page,
                    rect,
                    scale,
                    stride: surface.stride(),
                    generation,
                },
                &surface,
            ) {
                Ok(TileResult::Done(tile)) => {
                    let drawn = tile.low_res.map_or(rect, |low| low.rect);
                    let stride = surface.stride() as usize;
                    surface
                        .with_pixels(|pixels| PageLook::decide(boxes.as_deref(), pixels, stride, drawn.width, drawn.height))
                        .unwrap_or_else(|_| PageLook::unknown())
                }
                Ok(TileResult::Cancelled { .. }) => return None,
                Err(_) => PageLook::unknown(),
            },
            None => PageLook::unknown(),
        }
    };
    if open.looks.len() >= MAX_LOOKS {
        open.looks.clear();
    }
    let look = Arc::new(look);
    open.looks.insert(page, Arc::clone(&look));
    Some(look)
}

/// Why there's no renderer for a job, for now or for good.
enum Unopened {
    Failed(RenderFailure),
    /// The reader left while the file was being opened, so the thread is free for what's wanted now.
    NotWanted,
    /// It hasn't answered in [`OPEN_PATIENCE`], and it's still being tried.
    NotAnswering,
}

/// Opens a file's renderer on a thread of its own, the one part of this that a dead volume can hold up for ever, and waits for it only while the job's still wanted. One file not answering is no reason for a healthy tab to get no tiles. The wait is a look every [`OPEN_LOOK`], and only while a file's being opened.
fn renderer_for<'a>(
    shared: &Shared,
    renderers: &'a mut HashMap<PathBuf, OpenDoc>,
    openings: &mut HashMap<PathBuf, Receiver<Result<File, RenderFailure>>>,
    path: &Path,
    service: &Result<ServiceConfig, RenderFailure>,
    policy: &Policy,
) -> Result<&'a mut OpenDoc, Unopened> {
    if !renderers.contains_key(path) {
        let config = service.as_ref().map_err(|failure| Unopened::Failed(*failure))?.clone();
        if !openings.contains_key(path) {
            if openings.len() >= MAX_OPENINGS {
                return Err(Unopened::NotAnswering);
            }
            let (sender, receiver) = sync_channel(1);
            let opening = path.to_path_buf();
            std::thread::Builder::new()
                .name("recto-open".into())
                .stack_size(128 * 1024)
                .spawn(move || {
                    let _ = sender.send(DocRenderer::open_file(&opening));
                })
                .map_err(|_| Unopened::Failed(RenderFailure::Unavailable))?;
            openings.insert(path.to_path_buf(), receiver);
        }
        let started = Instant::now();
        let opened = loop {
            let receiver = openings.get(path).ok_or(Unopened::Failed(RenderFailure::Unavailable))?;
            match receiver.recv_timeout(OPEN_LOOK) {
                Ok(opened) => break opened,
                Err(RecvTimeoutError::Disconnected) => break Err(RenderFailure::Unavailable),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if !lock(&shared.reader).running_is_wanted() {
                return Err(Unopened::NotWanted);
            }
            if started.elapsed() >= OPEN_PATIENCE {
                return Err(Unopened::NotAnswering);
            }
        };
        openings.remove(path);
        let file = opened.map_err(Unopened::Failed)?;
        // By the file that was opened, not whatever's at its path now.
        let key = shared
            .config
            .keys
            .as_ref()
            .filter(|keys| !keys.is_empty())
            .and_then(|keys| keys.get(recto_platform::secret::open_file_identity(&file)?));
        let doc = DocRenderer::with_file(file, path, key, config, policy.clone()).map_err(Unopened::Failed)?;
        renderers.insert(
            path.to_path_buf(),
            OpenDoc {
                length_told: false,
                doc,
                looks: HashMap::new(),
                replaced: false,
            },
        );
    }
    renderers.get_mut(path).ok_or(Unopened::Failed(RenderFailure::Unavailable))
}

/// A failure that's about the whole file, saying when it's for want of a password, which the app can ask for again.
fn document_failed(failure: RenderFailure) -> Outcome {
    Outcome::DocumentFailed {
        message: failure.message(),
        needs_key: matches!(failure, RenderFailure::NeedsPassword),
    }
}

fn do_job(open: &mut OpenDoc, job: &Job) -> Outcome {
    if let JobKind::Text { check } = job.kind {
        let read = open.doc.page_characters_for(job.source_page, true, check, job.generation);
        // Told to stop, a check that didn't finish says nothing about the page, so it's as though it was never tried. What was read on the way is good, and a read that went with its worker isn't a failure of the page's.
        let stopped = open.doc.is_stale(job.generation);
        return match read {
            Ok(page) => Outcome::Text {
                text: Arc::new(PageText::build(&page.chars, &page.boxes, page.frame, page.hidden_checked, page.copy_restricted)),
                checked: check && !stopped,
            },
            Err(_) if stopped => Outcome::Cancelled,
            Err(_) => Outcome::TextFailed,
        };
    }
    let doc = &mut open.doc;
    match &job.kind {
        JobKind::Text { .. } => Outcome::TextFailed,
        // Read before a renderer's asked for: see the thread's loop.
        JobKind::Links => Outcome::Links(None),
        JobKind::Size => match doc.page_size(job.source_page) {
            Ok((width, height)) => Outcome::Size(PageBox { width, height }),
            Err(failure) if whole_file(failure) => document_failed(failure),
            Err(failure) => Outcome::PageFailed {
                message: failure.message(),
                may_pass: false,
            },
        },
        JobKind::Traits => match doc.largest_image_pixels(job.source_page, job.generation) {
            // A stale question answers zero, which isn't an answer about the page. Remembering it would draw a heavy page a tile at a time.
            Ok(_) if doc.is_stale(job.generation) => Outcome::Cancelled,
            Ok(pixels) => Outcome::Heavy(pixels >= recto_render_service::REGION_MIN_IMAGE_PIXELS),
            Err(failure) if whole_file(failure) => document_failed(failure),
            Err(_) => Outcome::Heavy(false),
        },
        JobKind::Draw {
            scale,
            rect,
            surface,
            what,
            paper,
        } => {
            let request = TileRequest {
                page: job.source_page,
                rect: *rect,
                scale: *scale,
                stride: surface.stride(),
                generation: job.generation,
            };
            let result = match what {
                DrawKind::Region => doc.render_region(&request, recto_proto::TILE_EDGE, surface),
                DrawKind::Tile | DrawKind::Preview => doc.render_tile(&request, surface),
            };
            match result {
                Ok(TileResult::Done(tile)) => {
                    let mut kind = tile.kind;
                    let low_res = tile.low_res.map(|low| (low.rect, low.why));
                    let low_res_scale = tile.low_res.map(|low| low.scale);
                    if *paper == PaperKind::Dark {
                        let drawn = low_res.map_or(*rect, |(drawn, _)| drawn);
                        let drawn_scale = low_res_scale.unwrap_or(*scale);
                        // A preview is a picture of the whole page, as drawn for white paper, which is what's wanted to judge it: no extra draw.
                        let whole = (*what == DrawKind::Preview && low_res.is_none()).then_some(*rect);
                        // Replaced as this was drawn, so it's of the new file, and no look of the old one's may go for it.
                        seen_replaced(open);
                        let look = page_look(open, job.source_page, job.generation, whole.map(|rect| (surface, rect)));
                        let page = look
                            .as_ref()
                            .and_then(|_| open.doc.page_size(job.source_page).ok())
                            .and_then(|(width, height)| recto_proto::device_size(width, height, drawn_scale).ok());
                        // Replaced after it was drawn, while its look or size was being asked: the tile's of the old file, and the look may be of either, or of a draw that failed for it. Neither is kept, and the tile's drawn again.
                        if seen_replaced(open) {
                            return Outcome::Cancelled;
                        }
                        let Some(look) = look else { return Outcome::Cancelled };
                        // What keeps no surface comes down to its one colour. With the lower-resolution version, the surface is kept whatever's in it.
                        let colour_only = *what == DrawKind::Tile && low_res.is_none();
                        let dark = look.in_tile(page, drawn);
                        let _ = surface.with_pixels(|pixels| kind = onto_dark_paper(pixels, drawn, surface.stride(), kind, dark, colour_only));
                    }
                    Outcome::Drawn { kind, low_res }
                }
                Ok(TileResult::Cancelled { .. }) => Outcome::Cancelled,
                Err(failure) if whole_file(failure) => document_failed(failure),
                Err(failure @ RenderFailure::NotDrawableNow { .. }) => Outcome::PageFailed {
                    message: failure.message(),
                    may_pass: true,
                },
                Err(failure) => Outcome::PageFailed {
                    message: failure.message(),
                    may_pass: false,
                },
            }
        }
    }
}

/// Puts one tile on dark paper, as the rule says, and says what it is now. With `colour_only` a tile of one colour needs nothing done to its pixels, as it won't keep them.
fn onto_dark_paper(pixels: &mut [u8], drawn: recto_proto::TileRect, stride: u32, kind: TileKind, dark: DarkTile, colour_only: bool) -> TileKind {
    match (dark, kind) {
        (DarkTile::Dim, TileKind::Blank { rgb }) if colour_only => TileKind::Blank { rgb: dim_rgb(rgb) },
        (DarkTile::TurnOver(keeps, whiten), TileKind::Blank { rgb }) if keeps.is_empty() && whiten.is_empty() && colour_only => {
            TileKind::Blank { rgb: dark_rgb(rgb) }
        }
        (DarkTile::Dim, _) => {
            dim_page(pixels, drawn.width, drawn.height, stride);
            if matches!(kind, TileKind::Blank { .. }) { TileKind::Colour } else { kind }
        }
        (DarkTile::TurnOver(keeps, whiten), _) => {
            turn_over(pixels, drawn.width, drawn.height, stride, is_grey(kind), &keeps, &whiten);
            // Part of it stays a picture, so it isn't one colour now.
            if matches!(kind, TileKind::Blank { .. }) { TileKind::Colour } else { kind }
        }
    }
}

/// Failures about the file or the renderer rather than one page.
fn whole_file(failure: RenderFailure) -> bool {
    !matches!(
        failure,
        RenderFailure::BadPage
            | RenderFailure::BadRequest
            | RenderFailure::TooLargeAtThisScale
            | RenderFailure::PageFailed { .. }
            | RenderFailure::PageReadFailed { .. }
            | RenderFailure::NotDrawableNow { .. }
            | RenderFailure::ExportRefused { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Quiet;
    impl EngineEvents for Quiet {
        fn ops_ready(&self) {}
        fn note_changed(&self) {}
        fn page_moved(&self) {}
        fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
    }

    /// Failure: synced scroll makes the UI thread wait for another pane's page view while that pane's own thread holds it, which is how a scroll step reached 4 to 9 ms. With the page view busy, the place is left for its own thread, the latest only, and it gets there as soon as that thread lets go. It waits for the page view to come free first, as its own thread may hold it from the activation (failed once under load, 2026-11-11).
    #[test]
    fn synced_scroll_never_waits_for_a_busy_pane() {
        let engine = Engine::new(EngineConfig::default(), Arc::new(Quiet));
        engine.activate(
            1,
            100,
            0,
            PageMap {
                files: vec![PathBuf::from("/nowhere/a.pdf")],
                ..PageMap::default()
            },
            &[],
            None,
            crate::layout::Spread::Single,
        );
        engine.set_viewport(crate::tiles::Viewport {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 900.0,
            frame_width: 800.0,
            magnification: 1.0,
            backing: 2.0,
            live: false,
            scrolled_to: 0,
        });
        // The page view's own thread can still hold it from the activation for a moment, which is the busy case: so once it's free, the call goes at once. A first call that found it busy left its place for that thread, which this one replaces.
        let began = Instant::now();
        while !engine.follow_soon(10, 0.5) {
            assert!(began.elapsed() < Duration::from_secs(5), "the page view never came free");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(engine.reading_anchor(), Some((10, 0.5)), "free: it goes now");
        let held = engine.shared.reader.lock().unwrap();
        let started = Instant::now();
        assert!(!engine.follow_soon(20, 0.25), "busy: left for later");
        assert!(!engine.follow_soon(30, 0.75), "and the latest is what's kept");
        assert!(started.elapsed() < Duration::from_millis(50), "it didn't wait: {:?}", started.elapsed());
        drop(held);
        let began = Instant::now();
        while engine.reading_anchor() != Some((30, 0.75)) {
            assert!(
                began.elapsed() < Duration::from_secs(5),
                "its own thread takes it: {:?}",
                engine.reading_anchor()
            );
            engine.wake();
            std::thread::sleep(Duration::from_millis(5));
        }
        engine.shut_down();
    }

    /// A page view with no thread of its own, so what's left pending stays until a test looks: as a busy thread would leave it.
    fn without_its_thread() -> Engine {
        let engine = Engine::new(EngineConfig::default(), Arc::new(Quiet));
        engine.shut_down();
        engine.activate(
            1,
            100,
            0,
            PageMap {
                files: vec![PathBuf::from("/nowhere/a.pdf")],
                ..PageMap::default()
            },
            &[],
            None,
            crate::layout::Spread::Single,
        );
        let view = |y: f64, scrolled_to: u64| crate::tiles::Viewport {
            x: 0.0,
            y,
            width: 800.0,
            height: 900.0,
            frame_width: 800.0,
            magnification: 1.0,
            backing: 2.0,
            live: false,
            scrolled_to,
        };
        let mut seq = 0;
        let mut y = 0.0;
        for _ in 0..3 {
            for op in engine.set_viewport(view(y, seq)) {
                if let Op::ScrollTo { y: to, seq: sent } = op {
                    (y, seq) = (to, sent);
                }
            }
        }
        engine
    }

    /// Failure (review 1): a place left while the pane was busy is applied after a newer one that got the lock, so the pane goes to step k after step k+1. The newer one drops it.
    #[test]
    fn a_newer_place_drops_one_left_before() {
        let engine = without_its_thread();
        let held = engine.shared.reader.lock().unwrap();
        assert!(!engine.follow_soon(20, 0.25));
        drop(held);
        assert!(engine.follow_soon(30, 0.5));
        assert!(lock(&engine.shared.pending).follow.is_none(), "nothing older left to land after it");
        let mut reader = lock(&engine.shared.reader);
        assert!(!take_pending_follow(&engine.shared, &mut reader));
        assert_eq!(reader.reading_anchor(), Some((30, 0.5)));
    }

    /// Failure (review 1): a place left by synced scroll lands after the reader has scrolled this pane themselves, and takes them back.
    #[test]
    fn the_readers_own_scroll_drops_a_place_left_for_the_pane() {
        let engine = without_its_thread();
        let held = engine.shared.reader.lock().unwrap();
        assert!(!engine.follow_soon(60, 0.0));
        drop(held);
        let seq = lock(&engine.shared.reader).viewport_scrolled_to();
        let outcome = engine.set_viewport_scrolled(crate::tiles::Viewport {
            x: 0.0,
            y: 3000.0,
            width: 800.0,
            height: 900.0,
            frame_width: 800.0,
            magnification: 1.0,
            backing: 2.0,
            live: false,
            scrolled_to: seq,
        });
        assert!(outcome.scrolled);
        assert!(lock(&engine.shared.pending).follow.is_none());
    }

    /// Failure (review 1): with a middle pane gone, the tab that moves into its page view gets the place synced scroll left for the one before.
    #[test]
    fn another_tab_in_the_pane_drops_a_place_left_for_the_last() {
        let engine = without_its_thread();
        let held = engine.shared.reader.lock().unwrap();
        assert!(!engine.follow_soon(70, 0.5));
        assert!(!engine.zoom_soon(1.5));
        drop(held);
        engine.activate(
            2,
            40,
            3,
            PageMap {
                files: vec![PathBuf::from("/nowhere/b.pdf")],
                ..PageMap::default()
            },
            &[],
            None,
            crate::layout::Spread::Single,
        );
        let pending = lock(&engine.shared.pending);
        assert!(pending.follow.is_none(), "the old tab's place is gone");
        assert_eq!(pending.zoom, Some(1.5), "the comparison's one zoom still holds");
        drop(pending);
        engine.deactivate();
        let held = engine.shared.reader.lock().unwrap();
        drop(held);
        assert!(lock(&engine.shared.pending).follow.is_none());
    }

    /// Failure (review 1): another pane's zoom or a selection taken in another pane makes the UI thread wait on this pane's lock while its thread holds it. Both are left for that thread instead, and taken when it next holds its lock.
    #[test]
    fn zoom_and_a_selection_elsewhere_never_wait_for_a_busy_pane() {
        let engine = without_its_thread();
        let held = engine.shared.reader.lock().unwrap();
        let started = Instant::now();
        assert!(!engine.zoom_soon(2.0));
        assert!(!engine.clear_selection_soon());
        assert!(started.elapsed() < Duration::from_millis(50));
        drop(held);
        let mut reader = lock(&engine.shared.reader);
        assert!(take_pending_follow(&engine.shared, &mut reader));
        assert_eq!(reader.magnification(), Some(2.0));
        drop(reader);
        assert!(engine.zoom_soon(1.25), "free: now");
        assert_eq!(engine.magnification(), Some(1.25));
    }

    /// Failure: a blank tile of a lower-resolution page keeps its surface, but only its colour number is dimmed, so a tile at full white sits beside dimmed ones. Its pixels are what's shown, so they're what's changed.
    #[test]
    fn a_blank_tile_that_keeps_its_surface_has_its_pixels_changed() {
        let rect = TileRect {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        let white = || [0xFFu8; 8];
        let mut pixels = white();
        let kind = onto_dark_paper(&mut pixels, rect, 8, TileKind::Blank { rgb: 0xFF_FF_FF }, DarkTile::Dim, false);
        assert_eq!(u32::from(pixels[0]), dim_rgb(0xFF_FF_FF) & 0xFF, "dimmed");
        assert!(matches!(kind, TileKind::Colour), "{kind:?}");
        let mut pixels = white();
        let kind = onto_dark_paper(
            &mut pixels,
            rect,
            8,
            TileKind::Blank { rgb: 0xFF_FF_FF },
            DarkTile::TurnOver(Vec::new(), Vec::new()),
            false,
        );
        assert_eq!(u32::from(pixels[0]), dark_rgb(0xFF_FF_FF) & 0xFF, "turned over");
        assert!(matches!(kind, TileKind::Colour), "{kind:?}");
        let mut pixels = white();
        let kind = onto_dark_paper(&mut pixels, rect, 8, TileKind::Blank { rgb: 0xFF_FF_FF }, DarkTile::Dim, true);
        assert!(matches!(kind, TileKind::Blank { rgb } if rgb == dim_rgb(0xFF_FF_FF)), "{kind:?}");
    }
}
