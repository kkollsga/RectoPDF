//! One thread that runs what [`Reader`] asks for. It's parked with nothing to do: no timer, no wake-ups. A page blocks this thread and never the UI thread, and the one lock is never held across a render.

use crate::paper::PaperKind;
use crate::selection::Notice;
use crate::tiles::{MAX_ATTACH_PER_TURN, Op, PageMap, Reader, ReaderReport, SavedPlace, Viewport, Zoom};
use recto_render_service::{Policy, ServiceConfig};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::Thread;
use std::time::{Duration, Instant};

mod keys;
mod links;
mod points;
mod qa_holds;
mod run;
mod selection;
mod waking;
mod workers;

pub use keys::{FileIdentity, Keyring, MAX_KEYS};
pub use links::Links;
pub use points::Points;
#[cfg(any(test, feature = "qa"))]
pub use qa_holds::Qa;
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
    /// For QA ([`Qa::hold_busy`]): this view acts as though its own thread held it at every call from the UI thread and hadn't come round since. Only with the `qa` feature (or in this crate's tests): a release app has no such field.
    #[cfg(any(test, feature = "qa"))]
    qa_busy: AtomicBool,
}

/// A tick of the order in which things are asked of a page view, taken before its lock is waited for, so "newer" is when something was asked and not who got
/// the lock. Its own count: the place clock ([`crate::tiles::place_stamp`]) ticks when a place is sent, and nothing asked here sends one yet.
fn asked() -> u64 {
    static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    ASKED.fetch_add(1, Ordering::SeqCst) + 1
}

/// What's waiting for a page view that was busy when the UI thread asked. Only the latest of each counts.
#[derive(Default)]
struct Pending {
    /// Synced scroll's place: a page and how far down it.
    follow: Option<(u32, f64)>,
    /// When `follow` was asked for ([`asked`]): a jump drops only a follow asked before the jump was, whoever then gets the lock first.
    follow_asked: u64,
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
            #[cfg(any(test, feature = "qa"))]
            qa_busy: AtomicBool::new(false),
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

    fn after(&self, mut reader: MutexGuard<'_, Reader>) {
        let has_ops = reader.host.has_ops();
        let note = reader.note().map(str::to_string);
        let selecting = selecting_news(&mut reader);
        drop(reader);
        self.shared.settled.notify_all();
        note_change(&self.shared, note);
        tell_selecting(&self.shared, selecting);
        if has_ops {
            signal_ops(&self.shared);
        }
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
    pub fn follow_soon(&self, page: u32, within: f64) -> bool {
        let when = asked();
        self.now_or_soon(
            |pending| {
                pending.follow = Some((page, within));
                pending.follow_asked = when;
            },
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
        lock(&self.shared.reader).docs.hot_tab()
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
    /// next comes round (after a drawing job, say), is applied first, under the same lock, so the read says where the person's scroll in another pane has
    /// sent this one, and the follow's tick is before the stamp. Read before it, a command marked where the view was before the person moved it on.
    pub fn places(&self) -> (u64, Vec<(u32, SavedPlace)>) {
        read_place(&self.shared, |reader| (crate::tiles::place_stamp(), reader.places()))
    }

    /// Zooms, wherever it's asked from. Says what to.
    pub fn zoom(&self, zoom: Zoom) -> Option<f64> {
        let mut reader = lock(&self.shared.reader);
        // This is newer than a zoom (or a place) another pane left for this view: it goes first, so a step is from where the view was asked to be and the older ask doesn't land after.
        apply_pending(&self.shared, &mut reader);
        let value = reader.zoom(zoom);
        self.after(reader);
        value
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

    pub fn jump_to(&self, page: u32) {
        let when = asked();
        let mut reader = lock(&self.shared.reader);
        // Newer than any place synced scroll left for this view before this was asked, which would otherwise land after it.
        drop_follows_asked_before(&self.shared, when);
        reader.jump_to(page, 0.0);
        self.after(reader);
    }

    /// A jump to a place in a page, as a link or Back goes: the page's top, margin above the first page included, or that far down it. Says where the
    /// place clock had come to once it was sent, taken with the lock still held, so every read of this view's places after it carries that stamp or a later
    /// one and every read before it an earlier one; `None` if it wasn't sent, with no tab showing.
    pub fn jump_within(&self, page: u32, within: f64) -> Option<u64> {
        jump_after(self, asked(), page, within)
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
            // The reader's own scroll here is newer than any place synced scroll left for this pane.
            lock(&self.shared.pending).follow = None;
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

/// A read of where the view is, with what's pending applied first ([`Engine::places`]): the host is told of the scroll that sends it there. Off the UI
/// thread's scroll path, which leaves what's pending and never reads the place this way.
fn read_place<T>(shared: &Shared, read: impl FnOnce(&Reader) -> T) -> T {
    let mut reader = lock(&shared.reader);
    let applied = apply_pending(shared, &mut reader);
    let value = read(&reader);
    let has_ops = applied && reader.host.has_ops();
    drop(reader);
    if has_ops {
        signal_ops(shared);
    }
    value
}

/// The view's own thread takes what the UI thread left for this view, with the lock held. Says whether there was any. Held off by [`Qa::hold_busy`].
fn take_pending_follow(shared: &Shared, reader: &mut Reader) -> bool {
    !shared.held_busy() && apply_pending(shared, reader)
}

/// A jump to a place in a page, with the tick of when it was asked taken before the lock was waited for: drops only a follow asked before it.
fn jump_after(engine: &Engine, when: u64, page: u32, within: f64) -> Option<u64> {
    let mut reader = lock(&engine.shared.reader);
    drop_follows_asked_before(&engine.shared, when);
    let sent = reader.jump_to(page, within).then(crate::tiles::place_stamp);
    engine.after(reader);
    sent
}

/// Drops a follow synced scroll left that was asked before `when`. One asked after stays, whoever got the lock first.
fn drop_follows_asked_before(shared: &Shared, when: u64) {
    let mut pending = lock(&shared.pending);
    if pending.follow_asked < when {
        pending.follow = None;
    }
}

/// Applies what the UI thread left for this view, with the lock held. Says whether there was any.
fn apply_pending(shared: &Shared, reader: &mut Reader) -> bool {
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

    /// Failure: a jump drops a follow the person asked for after it. The jump blocks on the lock while the person's scroll in another pane leaves a newer
    /// follow pending, and then clears it: "newer" went by who got the lock. A follow is dropped only if it was asked before the jump was.
    #[test]
    fn a_jump_drops_only_a_follow_asked_before_it() {
        for follow_first in [true, false] {
            let engine = without_its_thread();
            let held = engine.shared.reader.lock().unwrap();
            let jump_asked = if follow_first {
                assert!(!engine.follow_soon(60, 0.5));
                asked()
            } else {
                let jump_asked = asked();
                assert!(!engine.follow_soon(60, 0.5));
                jump_asked
            };
            drop(held);
            jump_after(&engine, jump_asked, 12, 0.0);
            let mut reader = lock(&engine.shared.reader);
            take_pending_follow(&engine.shared, &mut reader);
            let want = if follow_first { (12, 0.0) } else { (60, 0.5) };
            assert_eq!(
                reader.docs.shown().map(|doc| doc.reading_anchor()),
                Some(want),
                "the follow was asked {} the jump was",
                if follow_first { "before" } else { "after" }
            );
        }
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
}
