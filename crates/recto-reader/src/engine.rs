//! One thread that runs what [`Reader`] asks for. It's parked with nothing to do: no timer, no wake-ups. A page blocks this thread and never the UI thread, and the one lock is never held across a render.

use crate::paper::PaperKind;
use crate::selection::Notice;
use crate::tiles::{MAX_ATTACH_PER_TURN, Op, PageMap, Reader, ReaderReport, SavedPlace, Viewport, Zoom};
use recto_render_service::{Policy, ServiceConfig};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::Thread;
use std::time::{Duration, Instant};

mod keys;
mod links;
mod look;
mod points;
mod qa_holds;
mod run;
mod selection;
mod waking;
mod workers;

pub use keys::{FileIdentity, Keyring, MAX_KEYS};
pub use links::Links;
pub use points::Points;
#[cfg(not(any(test, feature = "qa")))]
use qa_holds::Ask;
#[cfg(any(test, feature = "qa"))]
pub use qa_holds::{Ask, Qa};
use run::run_guarded;
pub use selection::Selection;
pub use waking::Wake;
pub use workers::WorkerFigures;

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
    /// The ticks of the newest asks applied to the view: **the newer asked wins in the view, whoever gets the lock** ([`asked`]).
    applied: Applied,
    /// For QA ([`Qa`]): a busy view made to last, and asks held between their tick and the lock. Only with the `qa` feature (or in this crate's tests): a
    /// release app has no such field.
    #[cfg(any(test, feature = "qa"))]
    qa: qa_holds::QaHolds,
    hidden: Mutex<crate::tiles::HiddenSets>, // The file's highlights each tab shown has taken out, unsaved: `Selection::show_hidden`.
}

/// The tick ([`asked`]) of the newest ask of each kind applied to the view, each moved on only with the view's lock held, so an ask asked before it isn't
/// applied after it, whichever thread got the lock first: one atomic each, no lock and no wait on the UI thread. The reader's own scroll in this view
/// ([`Engine::set_viewport_scrolled`]) takes no tick: it drops a follow left pending, and a jump asked before it that gets the lock after it still moves
/// the view.
#[derive(Default)]
struct Applied {
    /// Where the view is: a jump ([`Engine::jump_within`], [`Engine::jump_to`]), a follow ([`Engine::follow_soon`]) or another tab ([`Engine::activate`]).
    place: AtomicU64,
    /// The zoom: [`Engine::zoom`] and [`Engine::zoom_soon`].
    zoom: AtomicU64,
}

/// Whether an ask ticked `when` is newer than every one of its kind applied (`applied`), and if so makes it the newest. Only with the view's lock held.
fn newest(applied: &AtomicU64, when: u64) -> bool {
    applied.fetch_max(when, Ordering::SeqCst) < when
}

/// A tick of the order in which things are asked of a page view, taken before its lock is waited for, so "newer" is when something was asked and not who got
/// the lock: an ask is applied only if no ask of its kind asked after it has been ([`Applied`]). Its own count: the place clock
/// ([`crate::tiles::place_stamp`]) ticks when a place is sent, which is after an ask gets the lock.
fn asked() -> u64 {
    ASKED.fetch_add(1, Ordering::SeqCst) + 1
}

/// The count [`asked`] moves on.
static ASKED: AtomicU64 = AtomicU64::new(0);

/// **One tick of when something was asked, for asking it of several page views:** the comparison's one zoom is asked of every pane, and each view orders
/// it against another zoom by this same moment, so two zooms that interleave (the person's and an agent's) end the same in every pane. Taken once, before
/// any view's lock is waited for; each method here is the [`Engine`] method of its name, asked at this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Asked(u64);

impl Asked {
    /// The tick for an ask made now.
    #[must_use]
    pub fn now() -> Self {
        Self(asked())
    }

    /// [`Engine::zoom`], asked at this tick.
    pub fn zoom(self, engine: &Engine, zoom: Zoom) -> Option<f64> {
        engine.shared.after_asking(Ask::Zoom);
        zoom_at(engine, self.0, zoom)
    }

    /// [`Engine::zoom_soon`], asked at this tick.
    pub fn zoom_soon(self, engine: &Engine, value: f64) -> bool {
        engine.shared.after_asking(Ask::ZoomSoon);
        zoom_soon_at(engine, self.0, value)
    }
}

/// What's waiting for a page view that was busy when the UI thread asked. Only the latest of each counts.
#[derive(Default)]
struct Pending {
    /// Synced scroll's place: a page and how far down it.
    follow: Option<(u32, f64)>,
    /// When `follow` was asked for ([`asked`]): it's applied, by whichever route takes it, only if no jump, follow or tab change asked after it has been;
    /// and a follow asked before it isn't left over it. Kept when the follow is dropped, so one asked before that isn't left after.
    follow_asked: u64,
    /// The one zoom of a comparison.
    zoom: Option<f64>,
    /// When `zoom` was asked for: applied only if no zoom asked after it has been, and a zoom asked before it isn't left over it.
    zoom_asked: u64,
    /// Another pane took the selection.
    clear_selection: bool,
}

/// The page view's running half.
#[derive(Clone)]
pub struct Engine {
    shared: Arc<Shared>,
    thread: Arc<Mutex<Option<Thread>>>,
}

// size: two `Arc`s, on 2026-11-13 before R3.6. A handle every caller copies and every facade borrows: held so nothing is put in it by the way.
const _: () = assert!(size_of::<Engine>() <= 16, "the page view's handle grew");

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
            applied: Applied::default(),
            #[cfg(any(test, feature = "qa"))]
            qa: qa_holds::QaHolds::default(),
            hidden: Mutex::default(),
        });
        Self {
            shared,
            thread: Arc::new(Mutex::new(None)),
        }
    }

    /// Starts the thread the first time there's a document, and wakes it. `unpark` costs one atomic when it's already running.
    fn unpark_thread(&self) {
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

    fn after(&self, reader: MutexGuard<'_, Reader>) {
        tell_what_changed(&self.shared, reader);
        self.unpark_thread();
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
        let mut reader = lock(&self.shared.reader);
        let reader = &mut *reader;
        reader.cache.set_share(views, &mut reader.host);
    }

    /// The least room either side of the pages, as a comparison's pane has it.
    pub fn set_margins(&self, left: f64, right: f64) {
        let mut reader = lock(&self.shared.reader);
        reader.set_margins(left, right);
        self.after(reader);
    }

    /// The tab showing's magnification, counting what's pending ([`Self::places`]): it's read to decide by (the one zoom of a comparison), so a zoom another pane left for this view counts.
    pub fn magnification(&self) -> Option<f64> {
        read_place(&self.shared, |reader| reader.docs.shown().map(|doc| doc.magnification()))
    }

    /// Where the reader is in the tab showing, as a page and how far down it, counting what's pending ([`Self::places`]).
    pub fn reading_anchor(&self) -> Option<(u32, f64)> {
        read_place(&self.shared, |reader| reader.docs.shown().map(|doc| doc.reading_anchor()))
    }

    /// **Synced scroll, from the UI thread, which never waits for it.** If the page view's free, the view goes there now, and anything left before is dropped, as this is newer. If its thread holds it, as it does to take a finished tile, the place is left for that thread to take as it lets go, the latest only, so a scroll step costs a look at a lock and never a wait on one. Says whether it went now.
    ///
    /// **Newer by when it was asked:** a follow asked before the last jump, follow or tab change applied to this view isn't applied, now or by whichever
    /// route takes it later ([`Applied`]).
    pub fn follow_soon(&self, page: u32, within: f64) -> bool {
        let when = asked();
        self.shared.after_asking(Ask::Follow);
        follow_soon_at(self, when, page, within)
    }

    /// The comparison's one zoom, from the UI thread, the same way: now if the page view's free, and otherwise left for its thread, the latest only; and
    /// not at all once a zoom asked after it has been applied ([`Self::zoom`] from another thread).
    pub fn zoom_soon(&self, value: f64) -> bool {
        Asked::now().zoom_soon(self, value)
    }

    /// Does something with the page view's lock if it's free, and otherwise leaves it for its own thread. Never waits for the lock: the UI thread calls this.
    fn now_or_soon(&self, leave: impl FnOnce(&mut Pending), now: impl FnOnce(&mut Reader, &mut Pending)) -> bool {
        // Held busy for QA, it's as though the lock were taken.
        let tried = if self.shared.held_busy() {
            Err(std::sync::TryLockError::WouldBlock)
        } else {
            self.shared.reader.try_lock()
        };
        let reader = match tried {
            Ok(reader) => reader,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                leave(&mut lock(&self.shared.pending));
                self.unpark_thread();
                return false;
            }
        };
        let mut reader = reader;
        now(&mut reader, &mut lock(&self.shared.pending));
        self.after(reader);
        true
    }

    /// Whether two handles are the same page view.
    pub fn same_as(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Which tab this page view shows.
    pub fn hot_tab(&self) -> Option<u32> {
        lock(&self.shared.reader).docs.hot_tab()
    }

    /// Makes a tab the visible one. Off the UI thread, as the session decides this.
    #[allow(clippy::too_many_arguments)]
    ///
    /// The newest ask of the view's place, as a jump is: a follow asked before it, for what was showing, isn't about what's coming, and would land a middle
    /// pane's place on the tab that moves into it, left pending or arriving after by the UI thread's own way; one asked after it lands.
    pub fn activate(&self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32], saved: Option<SavedPlace>, spread: crate::layout::Spread) {
        let when = asked();
        self.shared.after_asking(Ask::Activate);
        let mut reader = lock(&self.shared.reader);
        drop_follows_asked_before(&self.shared, when);
        newest(&self.shared.applied.place, when);
        reader.activate(tab, pages, page, map, warm, saved, spread);
        self.after(reader);
    }

    /// Lays the tab showing out another way, keeping the place in it.
    pub fn set_spread(&self, spread: crate::layout::Spread) {
        let mut reader = lock(&self.shared.reader);
        reader.set_spread(spread);
        self.after(reader);
    }

    /// The system wants memory back, so what's off screen goes, and workers with nothing to do.
    pub fn memory_pressure(&self, critical: bool) {
        lock(&self.shared.reader).memory_pressure(critical);
        self.shared.kill_idle_workers.store(true, Ordering::SeqCst);
        self.unpark_thread();
    }

    /// For QA: the system taking back what's marked discardable, without a word. Says how many surfaces went.
    pub fn empty_discardable(&self) -> usize {
        lock(&self.shared.reader).cache.empty_discardable()
    }

    /// Exactly where each tab it holds is, with the place clock as it was read ([`crate::tiles::place_stamp`]): what's read is of that moment, and a jump
    /// this view was sent that came to the clock at or before the stamp is in it. A short lock.
    ///
    /// **What the UI thread has already asked of this view counts:** a follow synced scroll left pending for its own thread, which takes it only as it
    /// next comes round (after a drawing job, say), is applied first, under the same lock, if no jump or tab change asked after it has been ([`Applied`]),
    /// so the read says where the person's scroll in another pane has sent this one, and the place clock's tick for that follow is before the stamp. Read
    /// before it, a command marked where the view was before the person moved it on.
    pub fn places(&self) -> (u64, Vec<(u32, SavedPlace)>) {
        read_place(&self.shared, |reader| (crate::tiles::place_stamp(), reader.places()))
    }

    /// Zooms, wherever it's asked from. Says what to: the magnification the view has after it.
    ///
    /// **Newer by when it was asked:** what another pane left for this view goes first, so a step is from where the view was asked to be and an older ask
    /// doesn't land after; and if a zoom asked after this one has been applied already ([`Self::zoom_soon`] from the UI thread while this waited for the
    /// lock), this one isn't, and what's said is the zoom the view has.
    pub fn zoom(&self, zoom: Zoom) -> Option<f64> {
        Asked::now().zoom(self, zoom)
    }

    /// The tab showing's fit, document points per PDF point, if a tab is showing: what a zoom level is worked out from.
    pub fn fit(&self) -> Option<f64> {
        lock(&self.shared.reader).docs.shown().map(|doc| doc.fit())
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

    /// Files whose renderer found they need a password it has none for: see [`crate::tiles::Work::take_keyless`].
    pub fn take_keyless(&self) -> Vec<PathBuf> {
        lock(&self.shared.reader).work.take_keyless()
    }

    /// A password's been given for a file: see [`Reader::key_given`].
    pub fn key_given(&self, path: &Path) {
        let mut reader = lock(&self.shared.reader);
        reader.key_given(path);
        self.after(reader);
    }

    /// A jump to a page's top, the margin above the first page included: [`Self::jump_within`] with nothing down the page.
    pub fn jump_to(&self, page: u32) {
        jump_after(self, page, 0.0);
    }

    /// A jump to a place in a page, as a link or Back goes: the page's top, margin above the first page included, or that far down it. Says where the
    /// place clock had come to once it was sent, taken with the lock still held, so every read of this view's places after it carries that stamp or a later
    /// one and every read before it an earlier one; `None` if it wasn't sent, with no tab showing.
    pub fn jump_within(&self, page: u32, within: f64) -> Option<u64> {
        jump_after(self, page, within)
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
        let scrolled = reader.place.take_user_scrolled();
        if scrolled {
            // The reader's own scroll here is newer than any place synced scroll left for this pane, and than Find's hit waiting for its text.
            lock(&self.shared.pending).follow = None;
            reader.pointer.forget_reveal();
        }
        let (magnification, anchor) = reader.docs.shown().map(|doc| (doc.magnification(), doc.reading_anchor())).unzip();
        let moved = reader.place.has_moved_page();
        let more = reader.host.has_ops();
        let note = reader.note().map(str::to_string);
        drop(reader);
        note_change(&self.shared, note);
        if moved {
            self.shared.events.page_moved();
        }
        if more {
            signal_ops(&self.shared);
        }
        self.unpark_thread();
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
        let more = reader.host.has_ops();
        let selecting = selecting_news(&mut reader);
        drop(reader);
        tell_selecting(&self.shared, selecting);
        if more {
            signal_ops(&self.shared);
        }
        self.unpark_thread();
        ops
    }

    pub fn take_moved_page(&self) -> Option<u32> {
        lock(&self.shared.reader).place.take_moved_page()
    }

    /// From the UI thread, when told there are ops.
    pub fn drain(&self) -> Vec<Op> {
        self.shared.ops_signalled.store(false, Ordering::SeqCst);
        let mut reader = lock(&self.shared.reader);
        let ops = reader.drain(MAX_ATTACH_PER_TURN);
        let more = reader.host.has_ops();
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

    /// Blocks until everything visible is settled, or the time runs out. Never for the UI thread. Drawing for a tab off screen's place isn't waited for: it's nothing visible, and how long it takes is a file's to say. `qa state`'s `reader.sleep_draws.under_way` says whether it's going.
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
}

/// What a viewport did, for a comparison's other panes.
pub struct ViewportOutcome {
    pub ops: Vec<Op>,
    /// It was the reader's own scroll.
    pub scrolled: bool,
    pub magnification: Option<f64>,
    pub anchor: Option<(u32, f64)>,
}

/// A read of where the view is, with what's pending applied first ([`Engine::places`]). Off the UI thread's scroll path, which leaves what's pending and
/// never reads the place this way.
///
/// **What applying it changed is told, as by every other way it's applied:** the ops (the scroll that sends the view there, a selection's highlight gone),
/// a note, and the selection's notice (a stopped copy's that doesn't fade went with a selection another pane took, and the window kept it till something
/// else changed on the pane), with whoever waits for the view settled woken. The page the reader moved to isn't said here, as the UI thread's own follow
/// doesn't say it either: the host hears it when it arrives at the scroll it's sent ([`Engine::set_viewport_scrolled`]), and the session thread's look,
/// which reads this way, takes the page in the same look.
fn read_place<T>(shared: &Shared, read: impl FnOnce(&Reader) -> T) -> T {
    let mut reader = lock(&shared.reader);
    let applied = apply_pending(shared, &mut reader);
    let value = read(&reader);
    if applied {
        tell_what_changed(shared, reader);
    }
    value
}

/// Tells the host and whoever waits what's changed of the view, its lock let go first: the ops, a note, the selection's notice and a copy that's ready.
fn tell_what_changed(shared: &Shared, mut reader: MutexGuard<'_, Reader>) {
    let changed = Changed::of(&mut reader);
    drop(reader);
    changed.tell(shared);
}

/// What's changed of the view that the host is told of, taken with its lock held, to tell with it let go ([`tell_what_changed`]).
struct Changed {
    has_ops: bool,
    note: Option<String>,
    selecting: SelectingNews,
}

impl Changed {
    fn of(reader: &mut Reader) -> Self {
        Self {
            has_ops: reader.host.has_ops(),
            note: reader.note().map(str::to_string),
            selecting: selecting_news(reader),
        }
    }

    fn tell(self, shared: &Shared) {
        shared.settled.notify_all();
        note_change(shared, self.note);
        tell_selecting(shared, self.selecting);
        if self.has_ops {
            signal_ops(shared);
        }
    }
}

/// The view's own thread takes what the UI thread left for this view, with the lock held, by [`apply_pending`]'s rule. Says whether any was applied. Held
/// off by [`Qa::hold_busy`].
fn take_pending_follow(shared: &Shared, reader: &mut Reader) -> bool {
    !shared.held_busy() && apply_pending(shared, reader)
}

/// A jump to a place in a page, the one way [`Engine::jump_to`] and [`Engine::jump_within`] go, with the tick of when it was asked taken before the lock
/// is waited for.
///
/// **The newer asked wins in the view, whoever gets the lock:** a follow left for this view that was asked before this jump is dropped, as the jump would
/// undo it at once, and one asked after it is applied first. If a jump, follow or tab change asked after this jump has been applied (that follow just now,
/// or one the UI thread or the view's own thread or a read applied while this waited for the lock), this jump isn't applied to the view. The session sent
/// it and has its place, so it's still said sent, and the view then tells where it is as it does for a scroll of its own: the page it's on as the page the
/// reader moved to, with the place clock moved on, so every read of its places from here carries a stamp at or past the one said, and the session's next
/// look takes the view's page and place as the reader's (`recto_app`'s `core/places.rs`). Find's hit on the jump's page, waiting for its text, went with the
/// jump and is dropped with it, so the text's arrival doesn't take the view back over the newer move.
fn jump_after(engine: &Engine, page: u32, within: f64) -> Option<u64> {
    let when = asked();
    engine.shared.after_asking(Ask::Jump);
    let mut reader = lock(&engine.shared.reader);
    drop_follows_asked_before(&engine.shared, when);
    apply_pending(&engine.shared, &mut reader);
    let (sent, told) = if newest(&engine.shared.applied.place, when) {
        (reader.jump_to(page, within), false)
    } else {
        // Find's hit on this page went with this jump, and goes with it.
        reader.pointer.forget_reveal_with(page);
        let told = reader.docs.shown().is_some() && reader.place.tell_again();
        (told, told)
    };
    let sent = sent.then(crate::tiles::place_stamp);
    engine.after(reader);
    if told {
        engine.shared.events.page_moved();
    }
    sent
}

/// [`Engine::follow_soon`], asked at `when`: now if the view's free, and otherwise left pending.
fn follow_soon_at(engine: &Engine, when: u64, page: u32, within: f64) -> bool {
    engine.now_or_soon(
        // Only over one asked before it: one asked after it, left while this waited, is newer.
        |pending| {
            if pending.follow_asked < when {
                pending.follow = Some((page, within));
                pending.follow_asked = when;
            }
        },
        |reader, pending| {
            if pending.follow_asked < when {
                pending.follow = None;
            }
            if newest(&engine.shared.applied.place, when) {
                reader.pointer.forget_reveal();
                reader.follow(page, within);
            }
        },
    )
}

/// [`Engine::zoom`], asked at `when`.
fn zoom_at(engine: &Engine, when: u64, zoom: Zoom) -> Option<f64> {
    let mut reader = lock(&engine.shared.reader);
    apply_pending(&engine.shared, &mut reader);
    let value = if newest(&engine.shared.applied.zoom, when) {
        reader.zoom(zoom)
    } else {
        reader.docs.shown().map(|doc| doc.magnification())
    };
    engine.after(reader);
    value
}

/// [`Engine::zoom_soon`], asked at `when`: now if the view's free, and otherwise left pending.
fn zoom_soon_at(engine: &Engine, when: u64, value: f64) -> bool {
    engine.now_or_soon(
        // Only over one asked before it, as a follow's.
        |pending| {
            if pending.zoom_asked < when {
                pending.zoom = Some(value);
                pending.zoom_asked = when;
            }
        },
        |reader, pending| {
            if pending.zoom_asked < when {
                pending.zoom = None;
            }
            if newest(&engine.shared.applied.zoom, when) {
                reader.zoom(Zoom::To(value));
            }
        },
    )
}

/// Drops a follow synced scroll left that was asked before `when`.
fn drop_follows_asked_before(shared: &Shared, when: u64) {
    let mut pending = lock(&shared.pending);
    if pending.follow_asked < when {
        pending.follow = None;
    }
}

/// Applies what the UI thread left for this view, with the lock held, each only if no ask of its kind asked after it has been applied ([`Applied`]).
/// Says whether any was.
fn apply_pending(shared: &Shared, reader: &mut Reader) -> bool {
    let pending = std::mem::take(&mut *lock(&shared.pending));
    let zoom = pending.zoom.filter(|_| newest(&shared.applied.zoom, pending.zoom_asked));
    if let Some(value) = zoom {
        reader.zoom(Zoom::To(value));
    }
    if pending.clear_selection {
        reader.clear_selection();
    }
    let follow = pending.follow.filter(|_| newest(&shared.applied.place, pending.follow_asked));
    if let Some((page, within)) = follow {
        reader.pointer.forget_reveal();
        reader.follow(page, within);
    }
    zoom.is_some() || pending.clear_selection || follow.is_some()
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
        notice: reader.pointer.selection_notice(),
        copy_ready: reader.pointer.take_copy_signal(),
    }
}

/// Tells the host what changed of the selection: the notice, if it's another, and a copy that's ready. The host is never called with the notice's lock held, so one that pulls from inside a call can't deadlock on it.
fn tell_selecting(shared: &Shared, news: SelectingNews) {
    let changed = {
        let mut last = lock(&shared.last_notice);
        let changed = *last != news.notice;
        if changed {
            *last = news.notice;
        }
        changed
    };
    if changed {
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

#[cfg(test)]
mod tests {
    use super::run::onto_dark_paper;
    use super::*;
    use crate::paper::{DarkTile, dark_rgb, dim_rgb};
    use recto_proto::TileRect;
    use recto_render_service::TileKind;

    struct Quiet;
    impl EngineEvents for Quiet {
        fn ops_ready(&self) {}
        fn note_changed(&self) {}
        fn page_moved(&self) {}
        fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
    }

    /// Failure: synced scroll makes the UI thread wait for another pane's page view while that pane's own thread holds it, which is how a scroll step reached 4 to 9 ms. With the page view busy, the place is left for its own thread, the latest only, and it gets there as soon as that thread lets go. It waits for the activation's thread to let go first, as a call made while it held the view failed under load (2026-11-11).
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
        // The page view's own thread holds it a moment for the activation, a file that isn't there, which never settles. Once that thread has let go, free over several looks in a row, the first call on a free view goes at once.
        let (began, mut free_looks) = (Instant::now(), 0);
        while free_looks < 5 {
            free_looks = if engine.shared.reader.try_lock().is_ok() { free_looks + 1 } else { 0 };
            assert!(began.elapsed() < Duration::from_secs(5), "the activation's thread never let go");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(engine.follow_soon(10, 0.5), "free: it goes now");
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
            engine.unpark_thread();
            std::thread::sleep(Duration::from_millis(5));
        }
        engine.shut_down();
    }

    /// A page view with no thread of its own, so what's left pending stays until a test looks: as a busy thread would leave it.
    fn without_its_thread() -> Engine {
        without_its_thread_telling(Arc::new(Quiet))
    }

    /// [`without_its_thread`], telling `events`.
    fn without_its_thread_telling(events: Arc<dyn EngineEvents>) -> Engine {
        let engine = Engine::new(EngineConfig::default(), events);
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
        assert_eq!(reader.docs.shown().map(|doc| doc.reading_anchor()), Some((30, 0.5)));
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

    /// Failure: a place synced scroll left for a busy pane lands after a jump the session sent the pane since to a page's top (an agent's `goto`, Back to
    /// where a page began, the session's own page for the pane), and takes the pane back to where the other pane's scroll had it, undoing the newer jump;
    /// a jump a way down a page already dropped it. The jump is newer, as a newer follow is.
    #[test]
    fn a_jump_to_a_pages_top_drops_a_place_left_for_the_pane() {
        for by_page in [true, false] {
            let engine = without_its_thread();
            let held = engine.shared.reader.lock().unwrap();
            assert!(!engine.follow_soon(60, 0.5));
            drop(held);
            if by_page {
                engine.jump_to(12);
            } else {
                assert!(engine.jump_within(12, 0.0).is_some());
            }
            // Its own thread comes round.
            let mut reader = lock(&engine.shared.reader);
            take_pending_follow(&engine.shared, &mut reader);
            assert_eq!(
                reader.docs.shown().map(|doc| doc.reading_anchor()),
                Some((12, 0.0)),
                "by the page alone: {by_page}"
            );
        }
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

    /// Failure: an older synced zoom lands after a newer one. A pinch in another pane leaves `zoom_soon(1.5)` pending for this busy pane, the person then
    /// presses ⌘+ here: the zoom stepped from the magnification before the pending one, then the pane's thread applied 1.5 over it, so this pane ended at 1.5
    /// while the others got the ⌘+ value. The newer ask applies what's pending first, and what's read to decide anything (`magnification`) counts it.
    #[test]
    fn a_newer_zoom_goes_from_and_over_a_zoom_left_pending() {
        let engine = without_its_thread();
        let held = engine.shared.reader.lock().unwrap();
        assert!(!engine.zoom_soon(1.5));
        drop(held);
        assert_eq!(engine.magnification(), Some(1.5), "a read to decide by counts what the pane was asked");
        assert!(lock(&engine.shared.pending).zoom.is_none(), "and takes it in");
        let held = engine.shared.reader.lock().unwrap();
        assert!(!engine.zoom_soon(1.2));
        drop(held);
        let value = engine.zoom(Zoom::In).expect("zoomed");
        assert!(
            (value - 1.2 * crate::tiles::ZOOM_STEP).abs() < 1e-9,
            "a step from the zoom the pane was asked for, not from before it: {value}"
        );
        let mut reader = lock(&engine.shared.reader);
        assert!(
            !take_pending_follow(&engine.shared, &mut reader),
            "nothing older left to land after the newer zoom"
        );
        assert_eq!(reader.docs.shown().map(|doc| doc.magnification()), Some(value));
    }

    /// Failure: a jump drops a follow the person asked for after it, because the jump took its turn when it got the lock and not when it was asked: the jump
    /// waits on the lock while the person's scroll in another pane leaves a newer follow, and then clears it. And the same through either way a jump is
    /// sent: [`Engine::jump_to`] (a page's top) was a second copy of [`Engine::jump_within`], so one could take its tick after the lock and the other not.
    /// Forced: the jump is held between its tick and the lock, this view's own hold, when the follow is asked with the view held busy; the process-wide
    /// count another test's ask moves on (review round 2) isn't watched.
    #[test]
    fn a_jump_takes_its_turn_when_it_was_asked_by_either_way_it_is_sent() {
        for by_page in [true, false] {
            let engine = without_its_thread();
            engine.qa().hold_asks(Ask::Jump, true);
            std::thread::scope(|scope| {
                let jumping = scope.spawn(|| {
                    if by_page {
                        engine.jump_to(12);
                    } else {
                        assert!(engine.jump_within(12, 0.0).is_some());
                    }
                });
                assert!(engine.qa().wait_for_held_asks(1), "the jump asked");
                engine.qa().hold_busy(true);
                assert!(!engine.follow_soon(60, 0.5), "the view's busy: the follow is left");
                engine.qa().hold_busy(false);
                engine.qa().hold_asks(Ask::Jump, false);
                jumping.join().unwrap();
            });
            let mut reader = lock(&engine.shared.reader);
            take_pending_follow(&engine.shared, &mut reader);
            assert_eq!(
                reader.docs.shown().map(|doc| doc.reading_anchor()),
                Some((60, 0.5)),
                "the follow was asked after the jump (by the page alone: {by_page})"
            );
        }
    }

    /// Failure (found in review round 1 of the page view follow-ups): a jump to a page's top leaves the page a follow before it moved the view to as the
    /// page the reader moved to, so the session's next look takes that page from the view (the view's own, as it's been given everything) while the view
    /// sits at the jump: the session and the window disagree, and the reader is told they went to a page they're not on. A follow synced scroll sends while
    /// the session thread's look is between reading the views and sending its jump does this. After a jump the page moved to is the jump's, or none.
    #[test]
    fn a_jump_to_a_pages_top_leaves_no_page_an_earlier_follow_moved_to() {
        for by_page in [true, false] {
            let engine = without_its_thread();
            assert!(engine.follow_soon(60, 0.5), "free: it goes now");
            if by_page {
                engine.jump_to(12);
            } else {
                assert!(engine.jump_within(12, 0.0).is_some());
            }
            assert_eq!(engine.reading_anchor(), Some((12, 0.0)));
            let moved = engine.take_moved_page();
            assert!(
                moved.is_none_or(|page| page == 12),
                "the page the reader moved to is the jump's, or none, not the follow's: {moved:?} (by the page alone: {by_page})"
            );
        }
    }

    /// What a page view told, counted.
    #[derive(Default)]
    struct Counted {
        page_moved: AtomicU32,
        note_changed: AtomicU32,
    }
    impl EngineEvents for Counted {
        fn ops_ready(&self) {}
        fn note_changed(&self) {
            self.note_changed.fetch_add(1, Ordering::SeqCst);
        }
        fn page_moved(&self) {
            self.page_moved.fetch_add(1, Ordering::SeqCst);
        }
        fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
    }

    /// Each route by which a follow the UI thread asked for reaches the view.
    #[derive(Clone, Copy, Debug)]
    enum Route {
        /// [`Engine::follow_soon`] finds the lock free and goes now.
        Now,
        /// Left pending, the view's own thread takes it as it next comes round.
        OwnThread,
        /// Left pending, a read of where the view is applies it first ([`Engine::places`]).
        Read,
        /// Left pending, a zoom applies it first ([`Engine::zoom`]).
        Zoom,
    }

    /// The UI thread's follow to 0.5 of page 61, by `route`.
    fn follow_by(engine: &Engine, route: Route) {
        if matches!(route, Route::Now) {
            assert!(engine.follow_soon(60, 0.5), "the lock's free: it goes now");
            return;
        }
        engine.qa().hold_busy(true);
        assert!(!engine.follow_soon(60, 0.5), "busy: left pending");
        engine.qa().hold_busy(false);
        match route {
            Route::OwnThread => assert!(take_pending_follow(&engine.shared, &mut lock(&engine.shared.reader))),
            Route::Read => drop(engine.places()),
            Route::Zoom => drop(engine.zoom(Zoom::To(1.0))),
            Route::Now => unreachable!(),
        }
    }

    /// Failure (review round 1 of the page view follow-ups): a jump asked before the person's scroll in another pane, and so older than the follow that
    /// scroll sends this pane, lands after the follow and undoes it whenever the follow reached the view first: by the UI thread finding the lock free, by
    /// the view's own thread taking it, or by a read or a zoom applying it. Who got the lock decided, not who asked first. The follow's place stands by every
    /// route, and the session, which sent the jump and has its place, is told the view's: the page the view's on is the page the reader moved to, the host
    /// hears the page moved, and every read of the view after the jump carries a stamp at or past the one the jump was said sent with, so the session's
    /// newer-read rule takes it. Through both ways a jump is sent. Forced: the jump has asked and is held before the lock while the follow goes.
    #[test]
    fn a_follow_asked_after_a_jump_stands_by_every_route_it_reaches_the_view() {
        for route in [Route::Now, Route::OwnThread, Route::Read, Route::Zoom] {
            for by_page in [true, false] {
                let counted = Arc::new(Counted::default());
                let engine = without_its_thread_telling(counted.clone());
                engine.qa().hold_asks(Ask::Jump, true);
                let (sent, moved_before) = std::thread::scope(|scope| {
                    let jumping = scope.spawn(|| {
                        if by_page {
                            engine.jump_to(12);
                            None
                        } else {
                            engine.jump_within(12, 0.0)
                        }
                    });
                    assert!(engine.qa().wait_for_held_asks(1), "the jump asked");
                    follow_by(&engine, route);
                    let moved_before = counted.page_moved.load(Ordering::SeqCst);
                    engine.qa().hold_asks(Ask::Jump, false);
                    (jumping.join().unwrap(), moved_before)
                });
                let case = format!("{route:?}, by the page alone: {by_page}");
                if !by_page {
                    assert!(sent.is_some(), "said sent, as the session sent it: {case}");
                }
                let (stamp, _) = engine.places();
                assert!(sent.is_none_or(|sent| stamp >= sent), "a read after it is as new as the jump's send: {case}");
                assert_eq!(engine.reading_anchor(), Some((60, 0.5)), "the follow's place stands: {case}");
                assert!(
                    counted.page_moved.load(Ordering::SeqCst) > moved_before,
                    "the host hears the page moved: {case}"
                );
                assert_eq!(engine.take_moved_page(), Some(60), "the page the reader moved to is the view's: {case}");
            }
        }
    }

    /// Failure (the same review): a follow asked before a jump, by a UI thread preempted between taking its tick and the lock, lands after the newer jump
    /// and takes the view back: now, with the lock free when it gets there, or (review round 2) left pending for a busy view after the jump, and applied by
    /// whichever route takes it next. The jump's place stands, and nothing tells the session of a move.
    #[test]
    fn a_follow_asked_before_a_jump_is_not_applied_after_it() {
        for busy in [false, true] {
            for by_page in [true, false] {
                let engine = without_its_thread();
                engine.qa().hold_asks(Ask::Follow, true);
                std::thread::scope(|scope| {
                    let following = scope.spawn(|| engine.follow_soon(60, 0.5));
                    assert!(engine.qa().wait_for_held_asks(1), "the follow asked");
                    if by_page {
                        engine.jump_to(12);
                    } else {
                        assert!(engine.jump_within(12, 0.0).is_some());
                    }
                    engine.qa().hold_busy(busy);
                    engine.qa().hold_asks(Ask::Follow, false);
                    assert_eq!(following.join().unwrap(), !busy, "left pending only when busy");
                });
                let case = format!("busy: {busy}, by the page alone: {by_page}");
                assert_eq!(engine.qa().follow_is_pending(), busy, "{case}");
                engine.qa().hold_busy(false);
                let mut reader = lock(&engine.shared.reader);
                take_pending_follow(&engine.shared, &mut reader);
                drop(reader);
                assert_eq!(engine.reading_anchor(), Some((12, 0.0)), "the jump's place stands ({case})");
                assert!(engine.take_moved_page().is_none_or(|page| page == 12), "no move to tell of ({case})");
            }
        }
    }

    /// Failure: a follow asked for the tab a pane showed before another tab moved into it reaches the view after the tab change by the UI thread's own way
    /// (the lock free when it got there), and lands that tab's place on the new one; and the guard must not stop a follow asked after the tab change, by
    /// either route: now, or (review round 2) left pending while the tab change, asked first, waited for the lock, which then dropped it.
    #[test]
    fn a_follow_asked_before_another_tab_comes_isnt_applied_to_it_and_one_after_is() {
        let other = || PageMap {
            files: vec![PathBuf::from("/nowhere/b.pdf")],
            ..PageMap::default()
        };
        let engine = without_its_thread();
        engine.qa().hold_asks(Ask::Follow, true);
        std::thread::scope(|scope| {
            let following = scope.spawn(|| engine.follow_soon(30, 0.5));
            assert!(engine.qa().wait_for_held_asks(1), "the follow asked");
            engine.activate(2, 40, 3, other(), &[], None, crate::layout::Spread::Single);
            engine.qa().hold_asks(Ask::Follow, false);
            following.join().unwrap();
        });
        assert_eq!(engine.reading_anchor(), Some((3, 0.0)), "the new tab stays on its page");
        assert!(engine.follow_soon(20, 0.25), "free: now");
        assert_eq!(engine.reading_anchor(), Some((20, 0.25)), "a follow asked after the tab change lands");
        let engine = without_its_thread();
        engine.qa().hold_asks(Ask::Activate, true);
        std::thread::scope(|scope| {
            let activating = scope.spawn(|| engine.activate(2, 40, 3, other(), &[], None, crate::layout::Spread::Single));
            assert!(engine.qa().wait_for_held_asks(1), "the tab change asked");
            engine.qa().hold_busy(true);
            assert!(!engine.follow_soon(20, 0.25), "busy: left pending");
            engine.qa().hold_busy(false);
            engine.qa().hold_asks(Ask::Activate, false);
            activating.join().unwrap();
        });
        assert_eq!(engine.hot_tab(), Some(2));
        assert_eq!(
            engine.reading_anchor(),
            Some((20, 0.25)),
            "a follow asked after the tab change and left pending lands"
        );
    }

    /// Failure (review round 1, note 8): an agent's zoom from another thread, asked before the person's pinch settled in another pane, gets the lock after
    /// that pinch's `zoom_soon` (applied now, or left pending and applied first by this zoom) and goes over it; or a `zoom_soon` asked before the agent's zoom
    /// lands after it. The newer asked wins in the view either way, and the zoom says the zoom the view has.
    #[test]
    fn the_newer_asked_zoom_wins_whoever_gets_the_lock() {
        for pending in [false, true] {
            let engine = without_its_thread();
            engine.qa().hold_asks(Ask::Zoom, true);
            let said = std::thread::scope(|scope| {
                let zooming = scope.spawn(|| engine.zoom(Zoom::To(2.0)));
                assert!(engine.qa().wait_for_held_asks(1), "the agent's zoom asked");
                engine.qa().hold_busy(pending);
                assert_eq!(engine.zoom_soon(1.5), !pending);
                engine.qa().hold_busy(false);
                engine.qa().hold_asks(Ask::Zoom, false);
                zooming.join().unwrap()
            });
            assert_eq!(said, Some(1.5), "the zoom says the zoom the view has (left pending: {pending})");
            assert_eq!(engine.magnification(), Some(1.5), "the newer zoom stands (left pending: {pending})");
        }
        // And a pinch's zoom asked before the agent's lands after it neither now nor (review round 2) left pending for a busy view.
        for pending in [false, true] {
            let engine = without_its_thread();
            engine.qa().hold_asks(Ask::ZoomSoon, true);
            std::thread::scope(|scope| {
                let zooming = scope.spawn(|| engine.zoom_soon(1.5));
                assert!(engine.qa().wait_for_held_asks(1), "the pinch's zoom asked");
                assert_eq!(engine.zoom(Zoom::To(2.0)), Some(2.0));
                engine.qa().hold_busy(pending);
                engine.qa().hold_asks(Ask::ZoomSoon, false);
                assert_eq!(zooming.join().unwrap(), !pending, "left pending only when busy");
            });
            assert_eq!(lock(&engine.shared.pending).zoom.is_some(), pending);
            engine.qa().hold_busy(false);
            assert_eq!(
                engine.magnification(),
                Some(2.0),
                "the older zoom isn't applied after the newer (left pending: {pending})"
            );
        }
    }

    /// Failure (review round 2 of the page view follow-ups): a zoom left pending overwrites a newer one left before it. The person's ⌘+ on the UI thread
    /// ticks and is preempted before it looks at the lock; an agent's zoom ticks after it, finds the pane busy and leaves its zoom; the UI thread then finds
    /// it busy too and leaves its older zoom over the agent's, and the pane lands at the person's zoom while the agent's was the newer. Forced: the person's
    /// ask is held between its tick and the lock while the agent's, ticked after it, is left; the pane is held busy for both.
    #[test]
    fn an_older_zoom_left_pending_doesnt_overwrite_a_newer_one() {
        let engine = without_its_thread();
        engine.qa().hold_asks(Ask::ZoomSoon, true);
        std::thread::scope(|scope| {
            let persons = scope.spawn(|| engine.zoom_soon(1.5));
            assert!(engine.qa().wait_for_held_asks(1), "the person's zoom asked");
            engine.qa().hold_busy(true);
            assert!(!zoom_soon_at(&engine, asked(), 2.0), "busy: the agent's zoom is left");
            engine.qa().hold_asks(Ask::ZoomSoon, false);
            assert!(!persons.join().unwrap(), "busy: the person's zoom is left too");
        });
        engine.qa().hold_busy(false);
        assert_eq!(engine.magnification(), Some(2.0), "the newer asked zoom stands");
    }

    /// Failure (the same review): the same for synced scroll's place: an older follow left pending overwrites a newer one left before it.
    #[test]
    fn an_older_follow_left_pending_doesnt_overwrite_a_newer_one() {
        let engine = without_its_thread();
        engine.qa().hold_asks(Ask::Follow, true);
        std::thread::scope(|scope| {
            let older = scope.spawn(|| engine.follow_soon(60, 0.5));
            assert!(engine.qa().wait_for_held_asks(1), "the older follow asked");
            engine.qa().hold_busy(true);
            assert!(!follow_soon_at(&engine, asked(), 20, 0.25), "busy: the newer follow is left");
            engine.qa().hold_asks(Ask::Follow, false);
            assert!(!older.join().unwrap(), "busy: the older follow is left too");
        });
        engine.qa().hold_busy(false);
        assert_eq!(engine.reading_anchor(), Some((20, 0.25)), "the newer asked follow stands");
    }

    /// A viewport of the test's page view: 800 by 900 at `y`, the host having arrived at the scroll it was sent numbered `scrolled_to`.
    fn view_at(y: f64, scrolled_to: u64) -> Viewport {
        Viewport {
            x: 0.0,
            y,
            width: 800.0,
            height: 900.0,
            frame_width: 800.0,
            magnification: 1.0,
            backing: 2.0,
            live: false,
            scrolled_to,
        }
    }

    /// The host arrives where the view was last sent. Says where that is.
    fn arrive(engine: &Engine) -> (f64, u64) {
        let sent = engine
            .drain()
            .into_iter()
            .rev()
            .find_map(|op| if let Op::ScrollTo { y, seq } = op { Some((y, seq)) } else { None })
            .expect("the view was sent somewhere");
        engine.set_viewport(view_at(sent.0, sent.1));
        sent
    }

    /// A page's text comes in, as its job's outcome: "find me here" in a line at `pdf_y` up the page, so Find's hit at characters 0 to 4 has a place.
    fn text_lands(engine: &Engine, page: u32, pdf_y: f32) {
        let frame = recto_proto::PageFrame {
            turns: 0,
            left: 0.0,
            bottom: 0.0,
            right: 612.0,
            top: 792.0,
        };
        let chars: Vec<recto_proto::CharRecord> = "find me here".chars().map(|ch| recto_proto::CharRecord::new(ch as u32, 0)).collect();
        let boxes: Vec<recto_proto::CharBox> = (0..chars.len())
            .map(|at| [72.0 + at as f32 * 6.0, pdf_y, 77.0 + at as f32 * 6.0, pdf_y + 10.0])
            .collect();
        let text = Arc::new(recto_index::select::PageText::build(&chars, &boxes, frame, true, false));
        let job = crate::tiles::Job {
            tab: 1,
            position: page,
            file: PathBuf::from("/nowhere/a.pdf"),
            source_page: page,
            generation: u64::MAX,
            kind: crate::tiles::JobKind::Text { check: false },
        };
        let mut reader = lock(&engine.shared.reader);
        reader.finish(job, crate::tiles::Outcome::Text { text, checked: true });
        engine.after(reader);
    }

    /// Failure (review round 2 of the page view follow-ups): Find's hit waiting for its page's text takes the view back to it when the text comes, after a
    /// follow the person's scroll in another pane sent this pane since, by every route a follow reaches the view: the person had moved on, and the view
    /// went back to the hit over their newer move.
    #[test]
    fn a_follow_after_finds_hit_keeps_the_view_when_the_hits_text_comes() {
        for route in [Route::Now, Route::OwnThread, Route::Read, Route::Zoom] {
            let engine = without_its_thread();
            engine.selection().reveal(30, 0, 4);
            follow_by(&engine, route);
            text_lands(&engine, 30, 400.0);
            assert_eq!(engine.reading_anchor(), Some((60, 0.5)), "the follow's place stands: {route:?}");
        }
    }

    /// Failure (the same review): the same after the person scrolls this pane themselves: the hit's text takes the view back from where they scrolled to.
    #[test]
    fn the_persons_own_scroll_after_finds_hit_keeps_the_view_when_the_hits_text_comes() {
        let engine = without_its_thread();
        engine.selection().reveal(30, 0, 4);
        let (y, seq) = arrive(&engine);
        assert!(engine.set_viewport_scrolled(view_at(y + 2000.0, seq)).scrolled, "the person's own scroll");
        let scrolled_to = engine.reading_anchor();
        text_lands(&engine, 30, 400.0);
        assert_eq!(engine.reading_anchor(), scrolled_to, "where the person scrolled to stands");
    }

    /// Failure (the same review): the session's jump to Find's hit's page is overtaken by the person's follow, asked after it, before it gets to the view,
    /// and the hit, on the page in view and waiting for its text, is still brought into view when the text comes, over the newer follow. The hit goes with
    /// its jump. Forced: the jump is held between its tick and the lock while the follow lands and the hit is selected.
    #[test]
    fn finds_hit_goes_with_its_jump_when_a_newer_follow_overtakes_it() {
        for by_page in [true, false] {
            let engine = without_its_thread();
            engine.qa().hold_asks(Ask::Jump, true);
            std::thread::scope(|scope| {
                let jumping = scope.spawn(|| {
                    if by_page {
                        engine.jump_to(20);
                    } else {
                        assert!(engine.jump_within(20, 0.0).is_some());
                    }
                });
                assert!(engine.qa().wait_for_held_asks(1), "the jump asked");
                assert!(engine.follow_soon(20, 0.25), "the person's follow, asked after the jump, lands");
                arrive(&engine);
                engine.selection().reveal(20, 0, 4);
                engine.qa().hold_asks(Ask::Jump, false);
                jumping.join().unwrap();
            });
            // The hit near the page's top, above the view a quarter down it: brought into view, it would move the view.
            text_lands(&engine, 20, 770.0);
            assert_eq!(
                engine.reading_anchor(),
                Some((20, 0.25)),
                "the follow's place stands (by the page alone: {by_page})"
            );
        }
    }

    /// Failure (review round 1 of the page view follow-ups, note 7): a read of where the view is (as the session thread's look and every command make)
    /// applies a selection cleared by another pane that was left pending, and the host isn't told: the window kept the notice of a selection that was gone
    /// ("This file changed on disk, so the copy stopped.", one that doesn't fade) until something else on that pane changed. A read that applies what's
    /// pending tells what changed, as every other way it's applied does.
    #[test]
    fn a_read_that_applies_a_pending_clear_tells_the_host_the_notice_went() {
        let counted = Arc::new(Counted::default());
        let engine = without_its_thread_telling(counted.clone());
        engine.selection().select_all();
        engine.selection().copy(false);
        lock(&engine.shared.reader).file_changed(1);
        engine.set_margins(0.0, 0.0);
        assert!(engine.selection().notice().is_some(), "the copy that stopped says so");
        engine.qa().hold_busy(true);
        assert!(!engine.selection().clear_soon(), "left pending");
        engine.qa().hold_busy(false);
        let told = counted.note_changed.load(Ordering::SeqCst);
        drop(engine.places());
        assert!(engine.selection().notice().is_none(), "the read applied the clear");
        assert!(counted.note_changed.load(Ordering::SeqCst) > told, "and the host heard the notice went");
    }

    /// Failure (review 1): another pane's zoom or a selection taken in another pane makes the UI thread wait on this pane's lock while its thread holds it. Both are left for that thread instead, and taken when it next holds its lock.
    #[test]
    fn zoom_and_a_selection_elsewhere_never_wait_for_a_busy_pane() {
        let engine = without_its_thread();
        let held = engine.shared.reader.lock().unwrap();
        let started = Instant::now();
        assert!(!engine.zoom_soon(2.0));
        assert!(!engine.selection().clear_soon());
        assert!(started.elapsed() < Duration::from_millis(50));
        drop(held);
        let mut reader = lock(&engine.shared.reader);
        assert!(take_pending_follow(&engine.shared, &mut reader));
        assert_eq!(reader.docs.shown().map(|doc| doc.magnification()), Some(2.0));
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

    /// Failure (2026-11-13, the scripted hour of tab switching: `IOSurfaceCreate` "out of memory" for a preview): a page in view whose surface the system
    /// refused parked the thread that draws for good with the job still planned, so the page stayed bare paper and the view never settled, until something
    /// else woke the thread. With the next three surfaces refused (the first wait 20 ms, not a second, by the test's seam), the page is drawn once they
    /// can be made and the view settles; while one's waited for, the view isn't settled and `reader.surfaces` says a retry is pending; after, it says how
    /// many were refused and that none is pending, so a view at rest has no timed wake.
    #[test]
    fn a_page_whose_surface_was_refused_is_drawn_once_one_can_be_made() {
        let file = recto_testpdf::TempPdf::new(
            "refused.pdf",
            &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&[
                "A page, drawn once a surface can be made",
            ]))]),
        );
        crate::tiles::surface_seam::refuse(3, Duration::from_millis(20));
        let engine = Engine::new(EngineConfig::default(), Arc::new(Quiet));
        engine.activate(
            1,
            1,
            0,
            PageMap {
                files: vec![file.0.clone()],
                ..PageMap::default()
            },
            &[],
            None,
            crate::layout::Spread::Single,
        );
        let view = crate::tiles::Viewport {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 900.0,
            frame_width: 800.0,
            magnification: 1.0,
            backing: 2.0,
            live: false,
            scrolled_to: u64::MAX,
        };
        let release = |ops: Vec<Op>| {
            for op in ops {
                if let Op::Attach { surface, .. } = op {
                    recto_platform::surface::PixelSurface::release_for_host(surface);
                }
            }
        };
        release(engine.set_viewport(view));
        let (began, mut saw_pending) = (Instant::now(), false);
        loop {
            release(engine.drain());
            engine.applied();
            let report = engine.report().0;
            if report["surfaces"]["retry_pending"] == true {
                saw_pending = true;
                assert_eq!(report["settled"], false, "waiting for a surface isn't settled: {report}");
            }
            if engine.wait_settled(Duration::from_millis(10)) {
                break;
            }
            // Far past the 140 ms the three waits take: only a thread that never asks again reaches it.
            assert!(began.elapsed() < Duration::from_secs(20), "the page was never drawn: {report}");
        }
        let report = engine.report().0;
        assert!(saw_pending, "{report}");
        assert_eq!(report["surfaces"]["refused"], 3, "{report}");
        assert_eq!(report["surfaces"]["retry_pending"], false, "{report}");
        assert!(report["tiles"]["attached"].as_u64().unwrap() >= 1, "{report}");
        engine.shut_down();
    }

    /// The engine for one page of `file`, with every surface it asks for refused from the start (the first wait `first`), and what it hands the host
    /// let go of as the host would. Its size and traits jobs start the file's worker, which needs no surface, and the draw after them waits.
    fn waiting_for_a_surface(file: &recto_testpdf::TempPdf, policy: Option<recto_render_service::Policy>, first: Duration) -> Engine {
        crate::tiles::surface_seam::refuse(u32::MAX, first);
        let engine = Engine::new(
            EngineConfig {
                policy,
                ..EngineConfig::default()
            },
            Arc::new(Quiet),
        );
        engine.activate(
            1,
            1,
            0,
            PageMap {
                files: vec![file.0.clone()],
                ..PageMap::default()
            },
            &[],
            None,
            crate::layout::Spread::Single,
        );
        let view = crate::tiles::Viewport {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 900.0,
            frame_width: 800.0,
            magnification: 1.0,
            backing: 2.0,
            live: false,
            scrolled_to: u64::MAX,
        };
        for op in engine.set_viewport(view) {
            if let Op::Attach { surface, .. } = op {
                recto_platform::surface::PixelSurface::release_for_host(surface);
            }
        }
        let began = Instant::now();
        while !(engine.report().0["surfaces"]["retry_pending"] == true && !engine.workers().pids().is_empty()) {
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "a worker and a wait for a surface: {}",
                engine.report().0
            );
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        }
        engine
    }

    /// Waits, at most 20 s, for the engine to have no worker, and says whether it came to that.
    fn workers_gone(engine: &Engine) -> bool {
        let began = Instant::now();
        while !engine.workers().pids().is_empty() {
            if began.elapsed() >= Duration::from_secs(20) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        }
        true
    }

    /// Failure (review round 1 of the measured faults, S2): while a page in view waits for a surface the system refused, the thread that draws only
    /// parked out the wait, and never did what it does at rest: so memory pressure, which comes just when the system's short and refusing, never let an
    /// idle worker go while refusals lasted, and a worker holding a heavy page's hundreds of megabytes kept them through the outage. With every surface
    /// refused and the first wait a minute, memory pressure lets the idle worker go at once, and a worker counted big (any worker, here) goes at its
    /// time of 300 ms, both far inside the wait.
    #[test]
    fn workers_are_let_go_while_a_surface_is_waited_for() {
        let file = recto_testpdf::TempPdf::new(
            "waiting.pdf",
            &recto_testpdf::pages(&[recto_testpdf::Page::new(recto_testpdf::text_lines(&["A page whose surface is refused"]))]),
        );
        let engine = waiting_for_a_surface(&file, None, Duration::from_secs(60));
        engine.memory_pressure(false);
        assert!(workers_gone(&engine), "memory pressure let the idle worker go: {}", engine.report().0);
        engine.shut_down();
        let policy = recto_render_service::Policy {
            retire_bytes: 1,
            heavy_idle_exit: Duration::from_millis(300),
            ..recto_render_service::Policy::default()
        };
        let engine = waiting_for_a_surface(&file, Some(policy), Duration::from_secs(60));
        assert!(workers_gone(&engine), "the big worker went at its time: {}", engine.report().0);
        assert_eq!(engine.report().0["surfaces"]["retry_pending"], true, "while the surface is still waited for");
        crate::tiles::surface_seam::refuse(0, Duration::from_millis(1));
        engine.shut_down();
    }
}
