//! One thread that runs what [`Reader`] asks for. It's parked with nothing to do: no timer, no wake-ups. A page blocks this thread and never the UI thread, and the one lock is never held across a render.

use crate::layout::PageBox;
use crate::paper::{PaperKind, dark_rgb, turn_over};
use crate::tiles::{DrawKind, Job, JobKind, MAX_ATTACH_PER_TURN, Op, Outcome, PageMap, Reader, ReaderReport, Viewport};
use recto_platform::isolate;
use recto_render_service::{DocRenderer, Policy, RenderFailure, ServiceConfig, TileKind, TileRequest, TileResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
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
}

/// How the engine runs.
#[derive(Clone, Debug, Default)]
pub struct EngineConfig {
    /// Where the worker is. Found beside this program if left out.
    pub service: Option<ServiceConfig>,
    pub policy: Option<Policy>,
}

struct Shared {
    reader: Mutex<Reader>,
    /// Woken when a job ends or the host says it has applied ops, for whoever's waiting to see everything settled.
    settled: Condvar,
    events: Arc<dyn EngineEvents>,
    ops_signalled: AtomicBool,
    worker_pids: Mutex<Vec<u32>>,
    last_note: Mutex<Option<String>>,
    config: EngineConfig,
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
        let shared = Arc::new(Shared { reader: Mutex::new(Reader::new()), settled: Condvar::new(), events, ops_signalled: AtomicBool::new(false), worker_pids: Mutex::new(Vec::new()), last_note: Mutex::new(None), config });
        Self { shared, thread: Arc::new(Mutex::new(None)) }
    }

    /// Starts the thread the first time there's a document, and wakes it. `unpark` costs one atomic when it's already running.
    fn wake(&self) {
        let mut thread = lock(&self.thread);
        match &*thread {
            Some(running) => running.unpark(),
            None => {
                let shared = Arc::clone(&self.shared);
                // Small: everything it holds is on the heap.
                if let Ok(handle) = std::thread::Builder::new().name("recto-pages".into()).stack_size(512 * 1024).spawn(move || run(&shared)) {
                    *thread = Some(handle.thread().clone());
                }
            }
        }
    }

    fn after(&self, reader: MutexGuard<'_, Reader>) {
        let has_ops = reader.has_ops();
        let note = reader.note().map(str::to_string);
        drop(reader);
        self.shared.settled.notify_all();
        note_change(&self.shared, note);
        if has_ops {
            signal_ops(&self.shared);
        }
        self.wake();
    }

    /// Makes a tab the visible one. Off the UI thread, as the session decides this.
    pub fn activate(&self, tab: u32, pages: u32, page: u32, map: PageMap, warm: &[u32]) {
        let mut reader = lock(&self.shared.reader);
        reader.activate(tab, pages, page, map, warm);
        self.after(reader);
    }

    pub fn deactivate(&self) {
        let mut reader = lock(&self.shared.reader);
        reader.deactivate();
        self.after(reader);
    }

    pub fn jump_to(&self, page: u32) {
        let mut reader = lock(&self.shared.reader);
        reader.jump_to(page);
        self.after(reader);
    }

    pub fn set_paper(&self, paper: PaperKind) {
        let mut reader = lock(&self.shared.reader);
        reader.set_paper(paper);
        self.after(reader);
    }

    pub fn set_focus(&self, focus: bool) {
        let mut reader = lock(&self.shared.reader);
        reader.set_focus(focus);
        self.after(reader);
    }

    /// From the UI thread, on every scroll. A short lock and a few map lookups. What comes back is for the host to apply now.
    pub fn set_viewport(&self, viewport: Viewport) -> Vec<Op> {
        let mut reader = lock(&self.shared.reader);
        let ops = reader.set_viewport(viewport);
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
        ops
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

    /// The footprint of every worker running now, from outside.
    pub fn workers_footprint(&self) -> (u64, u64) {
        let pids = lock(&self.shared.worker_pids).clone();
        pids.iter().filter_map(|pid| isolate::process_memory(*pid)).fold((0, 0), |(now, peak), memory| (now + memory.footprint_bytes, peak + memory.peak_footprint_bytes))
    }
}

fn note_change(shared: &Shared, note: Option<String>) {
    let mut last = lock(&shared.last_note);
    if *last != note {
        *last = note;
        drop(last);
        shared.events.note_changed();
    }
}

fn signal_ops(shared: &Shared) {
    if !shared.ops_signalled.swap(true, Ordering::SeqCst) {
        shared.events.ops_ready();
    }
}

fn run(shared: &Shared) {
    let mut renderers: HashMap<PathBuf, DocRenderer> = HashMap::new();
    let mut wanted = lock(&shared.reader).wanted_files();
    let service = shared.config.service.clone().map_or_else(ServiceConfig::discover, Ok);
    let policy = shared.config.policy.clone().unwrap_or_default();
    loop {
        let job = {
            let mut reader = lock(&shared.reader);
            let files = reader.wanted_files();
            if !Arc::ptr_eq(&files, &wanted) {
                wanted = files;
            }
            reader.next_job()
        };
        // Only hot and warm tabs' files keep a renderer. Dropping one kills its worker and closes its file.
        renderers.retain(|path, _| wanted.contains(path));
        publish_workers(shared, &mut renderers);
        let Some(job) = job else {
            shared.settled.notify_all();
            std::thread::park();
            continue;
        };
        let cancel = match renderer_for(&mut renderers, &job.file, &service, &policy) {
            Ok(doc) => Some(doc.cancel_handle()),
            Err(_) => None,
        };
        lock(&shared.reader).note_started(&job, cancel);
        let outcome = match renderer_for(&mut renderers, &job.file, &service, &policy) {
            Ok(doc) => do_job(doc, &job),
            Err(failure) => Outcome::DocumentFailed(failure.message()),
        };
        publish_workers(shared, &mut renderers);
        let drawn = match (&job.kind, &outcome) {
            (JobKind::Draw { what: DrawKind::Tile | DrawKind::Region, .. }, Outcome::Drawn { .. }) => Some(None),
            (JobKind::Draw { .. }, Outcome::PageFailed { .. }) => Some(Some("page_failed")),
            _ => None,
        };
        let (tab, page) = (job.tab, job.position);
        let mut reader = lock(&shared.reader);
        reader.finish(job, outcome);
        let has_ops = reader.has_ops();
        let note = reader.note().map(str::to_string);
        drop(reader);
        note_change(shared, note);
        if let Some(failure) = drawn {
            shared.events.page_drawn(tab, page, failure);
        }
        if has_ops {
            signal_ops(shared);
        }
        shared.settled.notify_all();
    }
}

fn publish_workers(shared: &Shared, renderers: &mut HashMap<PathBuf, DocRenderer>) {
    let pids: Vec<u32> = renderers.values_mut().filter_map(DocRenderer::worker_pid).collect();
    *lock(&shared.worker_pids) = pids;
}

fn renderer_for<'a>(renderers: &'a mut HashMap<PathBuf, DocRenderer>, path: &Path, service: &Result<ServiceConfig, RenderFailure>, policy: &Policy) -> Result<&'a mut DocRenderer, RenderFailure> {
    if !renderers.contains_key(path) {
        let config = service.as_ref().map_err(|failure| *failure)?.clone();
        let doc = DocRenderer::new(path, None, config, policy.clone())?;
        renderers.insert(path.to_path_buf(), doc);
    }
    renderers.get_mut(path).ok_or(RenderFailure::Unavailable)
}

fn do_job(doc: &mut DocRenderer, job: &Job) -> Outcome {
    match &job.kind {
        JobKind::Size => match doc.page_size(job.source_page) {
            Ok((width, height)) => Outcome::Size(PageBox { width, height }),
            Err(failure) if whole_file(failure) => Outcome::DocumentFailed(failure.message()),
            Err(failure) => Outcome::PageFailed { message: failure.message(), may_pass: false },
        },
        JobKind::Traits => match doc.largest_image_pixels(job.source_page, job.generation) {
            // A stale question answers zero, which isn't an answer about the page. Remembering it would draw a heavy page a tile at a time.
            Ok(_) if doc.is_stale(job.generation) => Outcome::Cancelled,
            Ok(pixels) => Outcome::Heavy(pixels >= recto_render_service::REGION_MIN_IMAGE_PIXELS),
            Err(failure) if whole_file(failure) => Outcome::DocumentFailed(failure.message()),
            Err(_) => Outcome::Heavy(false),
        },
        JobKind::Draw { scale, rect, surface, what, paper } => {
            let request = TileRequest { page: job.source_page, rect: *rect, scale: *scale, stride: surface.stride(), generation: job.generation };
            let result = match what {
                DrawKind::Region => doc.render_region(&request, recto_proto::TILE_EDGE, surface),
                DrawKind::Tile | DrawKind::Preview => doc.render_tile(&request, surface),
            };
            match result {
                Ok(TileResult::Done(tile)) => {
                    let mut kind = tile.kind;
                    let low_res = tile.low_res.map(|low| (low.rect, low.why));
                    if *paper == PaperKind::Dark {
                        let drawn = low_res.map_or(*rect, |(drawn, _)| drawn);
                        let grey = matches!(kind, TileKind::Grey);
                        match kind {
                            TileKind::Blank { rgb } => kind = TileKind::Blank { rgb: dark_rgb(rgb) },
                            _ => {
                                let stride = surface.stride();
                                let _ = surface.with_pixels(|pixels| turn_over(pixels, drawn.width, drawn.height, stride, grey));
                            }
                        }
                        // A region's or a preview's surface stays in use, however blank it was, so it's turned over too.
                        if let (TileKind::Blank { .. }, DrawKind::Region | DrawKind::Preview) = (kind, what) {
                            let stride = surface.stride();
                            let _ = surface.with_pixels(|pixels| turn_over(pixels, drawn.width, drawn.height, stride, false));
                        }
                    }
                    Outcome::Drawn { kind, low_res }
                }
                Ok(TileResult::Cancelled { .. }) => Outcome::Cancelled,
                Err(failure) if whole_file(failure) => Outcome::DocumentFailed(failure.message()),
                Err(failure @ RenderFailure::NotDrawableNow { .. }) => Outcome::PageFailed { message: failure.message(), may_pass: true },
                Err(failure) => Outcome::PageFailed { message: failure.message(), may_pass: false },
            }
        }
    }
}

/// Failures about the file or the renderer rather than one page.
fn whole_file(failure: RenderFailure) -> bool {
    !matches!(failure, RenderFailure::BadPage | RenderFailure::BadRequest | RenderFailure::TooLargeAtThisScale | RenderFailure::PageFailed { .. } | RenderFailure::PageReadFailed { .. } | RenderFailure::NotDrawableNow { .. } | RenderFailure::ExportRefused { .. })
}
