//! A document's renderer, from the host's side. See the crate's notes for the rules.

use crate::transport::{Frame, FrameReader, ReadError};
use crate::{control, exit_code};
use recto_platform::isolate::{self, Channel, ChildExit, ChildProcess, Handle, IdleRule, SharedMemory, Wait};
use recto_proto::{self as proto, CharBox, CharRecord, FailCode, Kind, PageSize, ProtoError, RenderJob, TextJob, TextReply, TileKind, TileRect, TileReply, TileStatus};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The memory holding a tile's pixels: written by the worker, read by the host and later the compositor. Host-owned, because the host hands
/// it over for one job at a time. Replaced by IOSurface when the app needs it.
pub type Surface = SharedMemory;

/// What a worker may do. Defaults are the decisions from the spike; see `docs/ENGINEERING.md`.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Past this footprint during a job, the worker is killed.
    pub memory_limit_bytes: u64,
    /// Past this footprint after a job, the worker is retired before its next job for another page.
    pub retire_bytes: u64,
    /// A job running longer than this has its worker killed.
    pub page_deadline: Duration,
    /// How long a stale job may take to return before its worker is killed.
    pub cancel_grace: Duration,
    /// The worker leaves after this long with no job.
    pub idle_exit: Duration,
    /// A worker still here this long after it should have left is killed, because it's ignoring its own exit.
    pub idle_grace: Duration,
    /// How often a running job's worker is checked.
    pub watch_step: Duration,
    /// How long a new worker gets to say it's ready. Loading PDFium took 2 ms in the spike, so this is only for one that isn't going to.
    pub start_timeout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            // SPEC §3 and spike §12: about 512 MiB, because the worst real page reaches 493 MiB and nothing holds it under that.
            memory_limit_bytes: 512 * 1024 * 1024,
            retire_bytes: 64 * 1024 * 1024,
            // Platform §2.6. The slowest real page at 2x took 939 ms.
            page_deadline: Duration::from_secs(5),
            // Spike §12: a stale render is gone within 100 ms, by return or kill.
            cancel_grace: Duration::from_millis(90),
            idle_exit: Duration::from_secs(30),
            idle_grace: Duration::from_secs(5),
            // The worst growth measured is about 650 MiB/s, so this overshoots by a few MiB.
            watch_step: Duration::from_millis(5),
            start_timeout: Duration::from_secs(10),
        }
    }
}

/// Where the worker and PDFium are.
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub worker_exe: PathBuf,
    pub pdfium_lib: PathBuf,
    /// For tests that stand in a different worker. Arguments replace the normal ones when `replace_args` is set.
    pub extra_env: Vec<(String, String)>,
    pub replace_args: Option<Vec<OsString>>,
}

impl ServiceConfig {
    /// Finds the worker beside this program (or one folder up, for a test binary in `deps/`) and PDFium beside the worker or in the
    /// `pdfium` folder `tools/pdfium.sh` fills. In debug builds only, `RECTO_RENDER_SERVICE` and `RECTO_PDFIUM_LIB` override: nothing shipped loads code from
    /// a path in its environment.
    pub fn discover() -> Result<Self, RenderFailure> {
        let exe_name = format!("recto-render-service{}", std::env::consts::EXE_SUFFIX);
        let worker_exe = match dev_override("RECTO_RENDER_SERVICE") {
            Some(path) => PathBuf::from(path),
            None => {
                let this = std::env::current_exe().map_err(|_| RenderFailure::WorkerMissing)?;
                let here = this.parent().ok_or(RenderFailure::WorkerMissing)?;
                [Some(here), here.parent()]
                    .into_iter()
                    .flatten()
                    .map(|dir| dir.join(&exe_name))
                    .find(|path| path.is_file())
                    .ok_or(RenderFailure::WorkerMissing)?
            }
        };
        Self::for_worker(worker_exe)
    }

    /// Finds PDFium for a known worker binary.
    pub fn for_worker(worker_exe: PathBuf) -> Result<Self, RenderFailure> {
        if !worker_exe.is_file() {
            return Err(RenderFailure::WorkerMissing);
        }
        let library = isolate::library_file_name("pdfium");
        let pdfium_lib = match dev_override("RECTO_PDFIUM_LIB") {
            Some(path) => PathBuf::from(path),
            None => {
                let dir = worker_exe.parent().ok_or(RenderFailure::WorkerMissing)?;
                let beside = dir.join(&library);
                let fetched = dir.parent().map(|parent| parent.join("pdfium").join(&library));
                [Some(beside), fetched].into_iter().flatten().find(|path| path.is_file()).ok_or(RenderFailure::LibraryMissing)?
            }
        };
        Ok(Self { worker_exe, pdfium_lib, extra_env: Vec::new(), replace_args: None })
    }
}

/// A development override from the environment. Ignored in a release build.
fn dev_override(name: &str) -> Option<std::ffi::OsString> {
    if cfg!(debug_assertions) { std::env::var_os(name) } else { None }
}

/// Why a worker stopped being one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillReason {
    /// Past the hard memory limit.
    MemoryLimit,
    /// Past the page deadline.
    Deadline,
    /// Ended without being asked.
    Crashed,
    /// Sent something the protocol or the job rules out. The text is static, so it can't echo anything hostile.
    Rejected(&'static str),
}

/// Why a call didn't produce what was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderFailure {
    /// The worker binary isn't where it should be.
    WorkerMissing,
    /// PDFium isn't where it should be. `tools/pdfium.sh` puts it there.
    LibraryMissing,
    /// The file couldn't be opened or the worker couldn't be started.
    Unavailable,
    NotFound,
    NeedsPassword,
    NotPdf,
    Security,
    /// The file changed underneath us: a new worker saw a different page count.
    SourceChanged,
    BadPage,
    BadRequest,
    /// The page can't be drawn at this scale within the caps on the wire.
    TooLargeAtThisScale,
    /// This page stopped its worker, in a worker with nothing else in it. It won't be tried again until [`DocRenderer::forget_failures`].
    PageFailed { page: u32, reason: KillReason },
    /// Opening the file stopped its worker, before any page was involved. Not tried again until [`DocRenderer::forget_failures`].
    OpenFailed { reason: KillReason },
    /// Drawing the whole page ran past its time cap. No worker was at fault.
    TookTooLong,
}

impl RenderFailure {
    /// A stable code for agents.
    pub fn code(self) -> &'static str {
        match self {
            Self::WorkerMissing => "render_worker_missing",
            Self::LibraryMissing => "render_library_missing",
            Self::Unavailable => "render_unavailable",
            Self::NotFound => "file_not_found",
            Self::NeedsPassword => "needs_password",
            Self::NotPdf => "not_a_pdf",
            Self::Security => "unsupported_encryption",
            Self::SourceChanged => "file_changed",
            Self::BadPage => "no_such_page",
            Self::BadRequest => "bad_request",
            Self::TooLargeAtThisScale => "too_large_at_this_scale",
            Self::PageFailed { reason: KillReason::MemoryLimit, .. } => "page_too_large",
            Self::PageFailed { reason: KillReason::Deadline, .. } => "page_too_slow",
            Self::PageFailed { reason: KillReason::Crashed, .. } => "page_crashed_renderer",
            Self::PageFailed { reason: KillReason::Rejected(_), .. } | Self::OpenFailed { reason: KillReason::Rejected(_) } => "renderer_misbehaved",
            Self::OpenFailed { reason: KillReason::MemoryLimit } => "file_too_large_to_open",
            Self::OpenFailed { reason: KillReason::Deadline } => "file_too_slow_to_open",
            Self::OpenFailed { reason: KillReason::Crashed } => "file_crashed_renderer",
            Self::TookTooLong => "render_took_too_long",
        }
    }

    /// Plain words for a person.
    pub fn message(self) -> String {
        match self {
            Self::WorkerMissing => "RectoPDF's page renderer isn't installed where it should be.".into(),
            Self::LibraryMissing => "RectoPDF's PDF library isn't installed where it should be.".into(),
            Self::Unavailable => "The page renderer couldn't be started.".into(),
            Self::NotFound => "That file couldn't be found.".into(),
            Self::NeedsPassword => "This PDF needs a password.".into(),
            Self::NotPdf => "This file isn't a PDF, or it's too damaged to open.".into(),
            Self::Security => "This PDF uses encryption RectoPDF can't read.".into(),
            Self::SourceChanged => "This file changed on disk while it was open.".into(),
            Self::BadPage => "That page doesn't exist or couldn't be read.".into(),
            Self::BadRequest => "That isn't a part of the page that can be drawn.".into(),
            Self::TooLargeAtThisScale => "This page is too large to draw at this zoom.".into(),
            Self::PageFailed { page, reason: KillReason::MemoryLimit } => {
                format!("Page {} needs more memory than a renderer is allowed. A lower zoom may fit.", page + 1)
            }
            Self::PageFailed { page, reason: KillReason::Deadline } => format!("Page {} took too long to draw.", page + 1),
            Self::PageFailed { page, reason: KillReason::Crashed } => format!("Page {} stopped the renderer. The rest of the document is fine.", page + 1),
            Self::PageFailed { page, reason: KillReason::Rejected(_) } => {
                format!("The renderer sent something invalid for page {}, so it was stopped. This file may be malicious.", page + 1)
            }
            Self::OpenFailed { reason: KillReason::MemoryLimit } => "Opening this file needs more memory than a renderer is allowed.".into(),
            Self::OpenFailed { reason: KillReason::Deadline } => "This file took too long to open.".into(),
            Self::OpenFailed { reason: KillReason::Crashed } => "Opening this file stopped the renderer.".into(),
            Self::OpenFailed { reason: KillReason::Rejected(_) } => {
                "The renderer sent something invalid while opening this file, so it was stopped. This file may be malicious.".into()
            }
            Self::TookTooLong => "Drawing this page at this zoom would take too long.".into(),
        }
    }
}

/// How one job stopped, inside one worker.
#[derive(Debug)]
enum Stop {
    /// The worker is gone.
    Killed(KillReason),
    /// It left because it had been idle: try again in a new one. Not a failure of anything.
    WentIdle,
    /// It was stale and didn't return in time, so it was killed. Not a failure of the page.
    StaleKilled,
    /// The worker is fine and says this can't be done.
    Refused(FailCode),
}

/// Numbers for `budget render` and QA. Measured from outside the worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub spawns: u32,
    pub retirements: u32,
    pub memory_kills: u32,
    pub deadline_kills: u32,
    pub cancel_kills: u32,
    pub crashes: u32,
    pub rejections: u32,
    pub idle_exits: u32,
    pub jobs: u64,
    /// Footprint of the most recent worker when it reported ready, before its document.
    pub ready_footprint_bytes: u64,
    /// Footprint of the most recent worker just after it opened its document.
    pub opened_footprint_bytes: u64,
    /// The highest footprint of any worker this renderer has had.
    pub peak_footprint_bytes: u64,
}

/// One running worker.
struct Worker {
    child: ChildProcess,
    channel: Channel,
    reader: FrameReader,
    jobs_done: u32,
}

/// The word that says which generations are stale. The worker sees a shared copy and may be hostile, so the host decides from its own: a worker
/// that scribbles on the shared one can only cancel its own jobs, not make the host believe everything is stale.
#[derive(Debug)]
struct CancelWord {
    shared: SharedMemory,
    cutoff: AtomicU64,
}

impl CancelWord {
    fn is_stale(&self, generation: u64) -> bool {
        generation < self.cutoff.load(Ordering::SeqCst)
    }
}

/// Cancels jobs by generation, from any thread, with atomic stores and no system call.
#[derive(Clone, Debug)]
pub struct CancelHandle(Arc<CancelWord>);

impl CancelHandle {
    /// Every job with a generation below this is stale. Only ever moves up.
    pub fn cancel_before(&self, generation: u64) {
        // The host's own word first, so by the time a worker can see a job is stale, the host agrees.
        let cutoff = self.0.cutoff.fetch_max(generation, Ordering::SeqCst).max(generation);
        if let Some(word) = self.0.shared.atomic_u64(control::CANCEL_BEFORE_OFFSET) {
            word.store(cutoff, Ordering::SeqCst);
        }
    }
}

/// One tile to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRequest {
    pub page: u32,
    pub rect: TileRect,
    pub scale: f32,
    pub stride: u32,
    pub generation: u64,
}

/// A tile that's been validated. Its pixels are in the surface the call was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub kind: TileKind,
    /// Time inside the worker, from the worker: for reports, not decisions.
    pub render_us: u32,
}

/// How a tile call ended, short of failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileResult {
    Done(Tile),
    /// The generation went stale. The surface holds nothing useful.
    Cancelled { worker_killed: bool },
}

/// A page's characters, validated. One record per character, plus a box each if asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageCharacters {
    pub chars: Vec<CharRecord>,
    pub boxes: Vec<CharBox>,
    /// The pixel check for hidden text ran. False if it wasn't asked for, or if the page was too heavy to draw for it.
    pub hidden_checked: bool,
}

/// How long to wait for a killed worker's final numbers.
const FINAL_NUMBERS_WAIT: Duration = Duration::from_millis(200);
/// How long to wait to learn why a worker that closed its channel left.
const EXIT_STATUS_WAIT: Duration = Duration::from_secs(2);

/// How many pages that stopped a worker are remembered, so a bomb isn't retried on every scroll.
const MAX_FAILED_PAGES: usize = 64;

/// How much of a page a job touches, lightest first. A page that stopped a job also stops anything heavier, but not something lighter: a page too heavy to
/// draw may still give up its text, and a page too heavy to load may still say its size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum JobWeight {
    Size,
    Text,
    Draw,
}

/// A page that stopped a worker, and the lightest job that did it.
#[derive(Clone, Copy, Debug)]
struct FailedPage {
    page: u32,
    weight: JobWeight,
    reason: KillReason,
}

/// One document's renderer. Holds the file and a control page, and a worker only while one is needed. A cold tab never has one of these.
pub struct DocRenderer {
    config: ServiceConfig,
    policy: Policy,
    file: File,
    password: Option<Vec<u8>>,
    control: Arc<CancelWord>,
    worker: Option<Worker>,
    page_count: Option<u32>,
    next_job: u32,
    failed: Vec<FailedPage>,
    /// Why opening the file failed, if it's something trying again wouldn't change. The descriptor is fixed, so its bytes are too.
    open_failure: Option<RenderFailure>,
    /// The page the worker last worked on, while its footprint is past the retirement line. It goes before the next job for any other page.
    retire_after_page: Option<u32>,
    stats: RenderStats,
}

impl DocRenderer {
    /// Opens the file for reading and prepares a renderer. No process starts until something needs one.
    pub fn new(path: &Path, password: Option<&[u8]>, config: ServiceConfig, policy: Policy) -> Result<Self, RenderFailure> {
        let file = File::open(path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => RenderFailure::NotFound,
            _ => RenderFailure::Unavailable,
        })?;
        let shared = SharedMemory::create(control::LEN).map_err(|_| RenderFailure::Unavailable)?;
        Ok(Self {
            config,
            policy,
            file,
            password: password.map(<[u8]>::to_vec),
            control: Arc::new(CancelWord { shared, cutoff: AtomicU64::new(0) }),
            worker: None,
            page_count: None,
            next_job: 0,
            failed: Vec::new(),
            open_failure: None,
            retire_after_page: None,
            stats: RenderStats::default(),
        })
    }

    pub fn stats(&self) -> RenderStats {
        self.stats
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// A handle that cancels this document's jobs from any thread.
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle(self.control.clone())
    }

    /// Whether a worker is running. Never true for a cold tab, because a cold tab has no `DocRenderer`.
    pub fn has_worker(&mut self) -> bool {
        self.reap_if_gone();
        self.worker.is_some()
    }

    /// The process id of the current worker, if there is one.
    pub fn worker_pid(&mut self) -> Option<u32> {
        self.reap_if_gone();
        self.worker.as_ref().map(|worker| worker.child.pid())
    }

    /// The current worker's footprint, from outside.
    pub fn worker_footprint(&mut self) -> Option<u64> {
        self.reap_if_gone();
        self.worker.as_ref().and_then(|worker| worker.child.memory()).map(|memory| memory.footprint_bytes)
    }

    /// Kills the worker now, for a window closing or a test. The next job starts a new one. Never blocks, so it's safe from any thread: the worker is
    /// signalled and let go, and the reaper collects it when it ends, however long a read from a dead volume keeps it.
    pub fn kill_worker(&mut self) {
        self.retire_after_page = None;
        if let Some(worker) = self.worker.take() {
            self.note_peak(&worker);
            worker.child.kill_now();
        }
    }

    /// Clears the memory of which pages stopped a worker. For a "try again", or after the zoom changes.
    pub fn forget_failures(&mut self) {
        self.failed.clear();
        self.open_failure = None;
    }

    /// Starts a worker if none is running, and returns the page count. Most callers can just ask for what they want.
    pub fn page_count(&mut self) -> Result<u32, RenderFailure> {
        self.ensure_worker()?;
        self.page_count.ok_or(RenderFailure::Unavailable)
    }

    /// A page's size in points, after rotation, validated.
    pub fn page_size(&mut self, page: u32) -> Result<(f32, f32), RenderFailure> {
        let job_id = self.next_job_id();
        let frame = proto::encode_size_request(job_id, page);
        let size = self.run_job(job_id, page, JobWeight::Size, None, &[], &frame, |reply| match reply.kind {
            Kind::Size => {
                let size = PageSize::decode(&reply.payload)?;
                proto::validate_page_size(job_id, &size)?;
                Ok(size)
            }
            _ => Err(ProtoError("the wrong kind of reply to a size request")),
        })?;
        Ok((size.width, size.height))
    }

    /// Draws one tile into `surface` and validates the reply before saying so. Blocks its caller: never call this from the UI thread.
    pub fn render_tile(&mut self, request: &TileRequest, surface: &Surface) -> Result<TileResult, RenderFailure> {
        let job = RenderJob {
            generation: request.generation,
            job_id: self.next_job_id(),
            page: request.page,
            scale: request.scale,
            rect: request.rect,
            stride: request.stride,
        };
        job.check().map_err(|_| RenderFailure::BadRequest)?;
        if job.bytes_needed() > surface.len() as u64 {
            return Err(RenderFailure::BadRequest);
        }
        if self.control.is_stale(request.generation) {
            return Ok(TileResult::Cancelled { worker_killed: false });
        }
        let frame = job.encode();
        let surface_len = surface.len() as u64;
        let control = self.control.clone();
        let reply = self.run_job(job.job_id, request.page, JobWeight::Draw, Some(request.generation), &[surface.handle()], &frame, |reply| match reply.kind {
            Kind::Tile => {
                let tile = TileReply::decode(&reply.payload)?;
                proto::validate_tile(&job, &tile, surface_len)?;
                // The host's word moves first, so a worker can't honestly see a cancel the host hasn't made.
                if tile.status == TileStatus::Cancelled && !control.is_stale(job.generation) {
                    return Err(ProtoError("a job cancelled that nobody cancelled"));
                }
                Ok(tile)
            }
            _ => Err(ProtoError("the wrong kind of reply to a tile")),
        });
        match reply {
            Ok(tile) => Ok(match tile.status {
                // Cancelled while it was finishing. The caller moved on, so this tile must never reach the screen.
                TileStatus::Done(_) if self.control.is_stale(request.generation) => TileResult::Cancelled { worker_killed: false },
                TileStatus::Done(kind) => TileResult::Done(Tile { kind, render_us: tile.render_us }),
                TileStatus::Cancelled => TileResult::Cancelled { worker_killed: false },
            }),
            Err(JobFailure::StaleKilled) => Ok(TileResult::Cancelled { worker_killed: true }),
            Err(JobFailure::Failed(failure)) => Err(failure),
        }
    }

    /// A page's characters, with a box per character if `boxes`, and the hidden-text check if `hidden_check`. If drawing the page for the check stops the
    /// worker, or is already known to, the text still comes back with `hidden_checked` false, so "not known" never reads as "nothing hidden". The page stays
    /// remembered as one that can't be drawn.
    pub fn page_characters(&mut self, page: u32, boxes: bool, hidden_check: bool) -> Result<PageCharacters, RenderFailure> {
        if hidden_check && self.known_failure(page, JobWeight::Draw).is_none() {
            match self.text_job(page, boxes, true) {
                Err(RenderFailure::PageFailed { reason: KillReason::MemoryLimit | KillReason::Deadline | KillReason::Crashed, .. }) => {}
                other => return other,
            }
        }
        self.text_job(page, boxes, false)
    }

    fn text_job(&mut self, page: u32, boxes: bool, hidden_check: bool) -> Result<PageCharacters, RenderFailure> {
        let job = TextJob { job_id: self.next_job_id(), page, boxes, hidden_check };
        let frame = job.encode();
        let weight = if hidden_check { JobWeight::Draw } else { JobWeight::Text };
        let reply = self.run_job(job.job_id, page, weight, None, &[], &frame, |reply| match reply.kind {
            Kind::Text => {
                let text = TextReply::decode(&reply.payload)?;
                proto::validate_text(&job, &text)?;
                Ok(text)
            }
            _ => Err(ProtoError("the wrong kind of reply to a text request")),
        })?;
        Ok(PageCharacters { chars: reply.chars, boxes: reply.boxes, hidden_checked: reply.hidden_checked })
    }

    /// Why a job this heavy shouldn't be tried on this page, if something already showed that.
    fn known_failure(&self, page: u32, weight: JobWeight) -> Option<KillReason> {
        self.failed.iter().find(|failed| failed.page == page && failed.weight <= weight).map(|failed| failed.reason)
    }

    fn next_job_id(&mut self) -> u32 {
        self.next_job = self.next_job.wrapping_add(1);
        self.next_job
    }

    /// Sends a job and interprets the reply, starting and replacing workers as needed. This is where a failure becomes a retry, a remembered
    /// page or an error. A stopped worker gets one more try in a fresh one, because what stopped it may be what earlier pages left behind.
    /// A fresh worker failing means the page itself did it, unless the time ran out, which wouldn't go better a second time.
    #[allow(clippy::too_many_arguments)]
    fn run_job<T>(
        &mut self,
        job_id: u32,
        page: u32,
        weight: JobWeight,
        generation: Option<u64>,
        handles: &[&Handle],
        frame: &[u8],
        decode: impl Fn(&Frame) -> Result<T, ProtoError>,
    ) -> Result<T, JobFailure> {
        if let Some(reason) = self.known_failure(page, weight) {
            return Err(RenderFailure::PageFailed { page, reason }.into());
        }
        if self.retire_after_page.is_some_and(|last| last != page) {
            self.stats.retirements += 1;
            self.bury_worker();
        }
        let mut retried = false;
        let mut idle_retries = 0;
        loop {
            self.ensure_worker()?;
            let Some(worker) = self.worker.as_mut() else { return Err(RenderFailure::Unavailable.into()) };
            let fresh = worker.jobs_done == 0;
            let watch = Watch { policy: &self.policy, control: &self.control, generation };
            let outcome = worker.exchange(frame, handles, &watch).and_then(|reply| {
                if reply.kind == Kind::Failed {
                    let (answered, code) = proto::decode_failed(&reply.payload).map_err(|error| Stop::Killed(KillReason::Rejected(error.0)))?;
                    if answered != job_id {
                        return Err(Stop::Killed(KillReason::Rejected("a failure for a different job")));
                    }
                    return Err(Stop::Refused(code));
                }
                decode(&reply).map_err(|error| Stop::Killed(KillReason::Rejected(error.0)))
            });
            self.stats.jobs += 1;
            match outcome {
                Ok(value) => {
                    worker.jobs_done += 1;
                    self.after_job(page);
                    return Ok(value);
                }
                Err(Stop::Refused(code)) => {
                    self.after_job(page);
                    return Err(failure_from_code(code).into());
                }
                Err(Stop::StaleKilled) => {
                    self.bury_worker();
                    self.stats.cancel_kills += 1;
                    return Err(JobFailure::StaleKilled);
                }
                Err(Stop::WentIdle) => {
                    self.bury_worker();
                    self.stats.idle_exits += 1;
                    idle_retries += 1;
                    // A real idle exit happens at most once per job. More than that isn't idleness.
                    if idle_retries > 2 {
                        self.stats.crashes += 1;
                        return Err(self.page_failed(page, weight, KillReason::Crashed));
                    }
                }
                Err(Stop::Killed(reason)) => {
                    self.bury_worker();
                    match reason {
                        KillReason::MemoryLimit => self.stats.memory_kills += 1,
                        KillReason::Deadline => self.stats.deadline_kills += 1,
                        KillReason::Crashed => self.stats.crashes += 1,
                        KillReason::Rejected(_) => self.stats.rejections += 1,
                    }
                    let worth_retrying = matches!(reason, KillReason::MemoryLimit | KillReason::Crashed);
                    if worth_retrying && !fresh && !retried {
                        retried = true;
                        continue;
                    }
                    return Err(self.page_failed(page, weight, reason));
                }
            }
        }
    }

    fn page_failed(&mut self, page: u32, weight: JobWeight, reason: KillReason) -> JobFailure {
        self.failed.retain(|failed| failed.page != page);
        if self.failed.len() >= MAX_FAILED_PAGES {
            self.failed.remove(0);
        }
        self.failed.push(FailedPage { page, weight, reason });
        RenderFailure::PageFailed { page, reason }.into()
    }

    /// Notes a worker past the retirement line. Only exit returns the memory (spike §6), but the worker goes before the next job for another
    /// page rather than now. A page can pass the line by itself, and retiring between its tiles would make it reload for each one: measured on
    /// the USGS map, 72 respawns and 14 s to draw one page.
    fn after_job(&mut self, page: u32) {
        let Some(worker) = self.worker.as_mut() else { return };
        let footprint = worker.child.memory().map_or(0, |memory| {
            self.stats.peak_footprint_bytes = self.stats.peak_footprint_bytes.max(memory.peak_footprint_bytes);
            memory.footprint_bytes
        });
        self.retire_after_page = (footprint > self.policy.retire_bytes).then_some(page);
    }

    /// Takes the worker away, killed and collected, keeping its last numbers.
    fn bury_worker(&mut self) {
        self.retire_after_page = None;
        if let Some(worker) = self.worker.take() {
            worker.child.kill_now();
            // Long enough to read its final numbers, usually under a millisecond. A worker stuck in the kernel isn't waited for.
            worker.child.wait_bounded(FINAL_NUMBERS_WAIT);
            self.note_peak(&worker);
        }
    }

    fn note_peak(&mut self, worker: &Worker) {
        if let Some(memory) = worker.child.memory() {
            self.stats.peak_footprint_bytes = self.stats.peak_footprint_bytes.max(memory.peak_footprint_bytes);
        }
    }

    /// Collects a worker that left on its own, such as after its idle time.
    fn reap_if_gone(&mut self) {
        let gone = self.worker.as_ref().is_some_and(|worker| worker.child.try_wait().is_some());
        if gone {
            self.stats.idle_exits += 1;
            self.bury_worker();
        }
    }

    /// Starts a worker and opens the document in it, if there isn't one already. A failure that trying again wouldn't change is remembered, so the next call
    /// doesn't start another process to learn the same thing.
    fn ensure_worker(&mut self) -> Result<(), RenderFailure> {
        if let Some(failure) = self.open_failure {
            return Err(failure);
        }
        self.reap_if_gone();
        if self.worker.is_some() {
            return Ok(());
        }
        let (worker, ready) = Worker::start(&self.config, &self.policy).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => RenderFailure::LibraryMissing,
            _ => RenderFailure::Unavailable,
        })?;
        self.stats.spawns += 1;
        self.stats.ready_footprint_bytes = ready;
        self.stats.peak_footprint_bytes = self.stats.peak_footprint_bytes.max(ready);
        self.open_in(worker).inspect_err(|failure| {
            if !matches!(failure, RenderFailure::Unavailable) {
                self.open_failure = Some(*failure);
            }
        })
    }

    fn open_in(&mut self, mut worker: Worker) -> Result<(), RenderFailure> {
        let file = Handle::from_file(&self.file).map_err(|_| RenderFailure::Unavailable)?;
        let frame = proto::encode_open(self.password.as_deref().unwrap_or_default()).map_err(|_| RenderFailure::NeedsPassword)?;
        let watch = Watch { policy: &self.policy, control: &self.control, generation: None };
        let reply = worker.exchange(&frame, &[&file, self.control.shared.handle()], &watch);
        let killed = |stats: &mut RenderStats, reason: KillReason| {
            match reason {
                KillReason::MemoryLimit => stats.memory_kills += 1,
                KillReason::Deadline => stats.deadline_kills += 1,
                KillReason::Crashed => stats.crashes += 1,
                KillReason::Rejected(_) => stats.rejections += 1,
            }
            RenderFailure::OpenFailed { reason }
        };
        let reply = match reply {
            Ok(reply) => reply,
            Err(Stop::Killed(reason)) => {
                worker.child.kill_now();
                worker.child.wait_bounded(FINAL_NUMBERS_WAIT);
                self.note_peak(&worker);
                return Err(killed(&mut self.stats, reason));
            }
            Err(_) => return Err(RenderFailure::Unavailable),
        };
        let rejected = |stats: &mut RenderStats, why: &'static str| killed(stats, KillReason::Rejected(why));
        let pages = match reply.kind {
            Kind::Opened => proto::decode_opened(&reply.payload).map_err(|error| rejected(&mut self.stats, error.0))?,
            Kind::Failed => {
                let (answered, code) = proto::decode_failed(&reply.payload).map_err(|error| rejected(&mut self.stats, error.0))?;
                if answered != 0 {
                    return Err(rejected(&mut self.stats, "a failure for a different job"));
                }
                return Err(failure_from_code(code));
            }
            _ => return Err(rejected(&mut self.stats, "the wrong kind of reply to opening")),
        };
        if self.page_count.is_some_and(|known| known != pages) {
            return Err(RenderFailure::SourceChanged);
        }
        self.page_count = Some(pages);
        if let Some(memory) = worker.child.memory() {
            self.stats.opened_footprint_bytes = memory.footprint_bytes;
            self.stats.peak_footprint_bytes = self.stats.peak_footprint_bytes.max(memory.peak_footprint_bytes);
        }
        self.worker = Some(worker);
        Ok(())
    }
}

/// Internal: a failure, or a stale job that took its worker with it.
enum JobFailure {
    Failed(RenderFailure),
    StaleKilled,
}

impl From<RenderFailure> for JobFailure {
    fn from(failure: RenderFailure) -> Self {
        Self::Failed(failure)
    }
}

impl From<JobFailure> for RenderFailure {
    fn from(failure: JobFailure) -> Self {
        match failure {
            JobFailure::Failed(failure) => failure,
            // Only a job with a generation can be stale, and those handle this themselves.
            JobFailure::StaleKilled => RenderFailure::Unavailable,
        }
    }
}

fn failure_from_code(code: FailCode) -> RenderFailure {
    match code {
        FailCode::BadPage => RenderFailure::BadPage,
        FailCode::BadRequest | FailCode::BadSurface => RenderFailure::BadRequest,
        FailCode::TooLarge => RenderFailure::TooLargeAtThisScale,
        FailCode::NeedsPassword => RenderFailure::NeedsPassword,
        FailCode::NotPdf => RenderFailure::NotPdf,
        FailCode::Security => RenderFailure::Security,
        FailCode::Unreadable | FailCode::Internal | FailCode::NoDocument => RenderFailure::Unavailable,
    }
}

/// What one exchange watches for.
struct Watch<'a> {
    policy: &'a Policy,
    control: &'a CancelWord,
    generation: Option<u64>,
}

impl Watch<'_> {
    fn is_stale(&self) -> bool {
        self.generation.is_some_and(|generation| self.control.is_stale(generation))
    }
}

impl Worker {
    /// Starts a worker, waits for its hello, and returns it with its footprint at that moment.
    fn start(config: &ServiceConfig, policy: &Policy) -> io::Result<(Self, u64)> {
        let (ours, theirs) = Channel::pair()?;
        let default_args: Vec<OsString>;
        let args: &[OsString] = match &config.replace_args {
            Some(args) => args,
            None => {
                default_args = vec![
                    "--pdfium".into(),
                    config.pdfium_lib.clone().into_os_string(),
                    "--idle-exit-ms".into(),
                    policy.idle_exit.as_millis().to_string().into(),
                    "--memory-limit".into(),
                    policy.memory_limit_bytes.to_string().into(),
                    "--deadline-ms".into(),
                    policy.page_deadline.as_millis().to_string().into(),
                ];
                &default_args
            }
        };
        let arg_refs: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
        let env_refs: Vec<(&str, &str)> = config.extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let rule = IdleRule { after: policy.idle_exit.saturating_add(policy.idle_grace), memory_limit_bytes: policy.memory_limit_bytes };
        let child = ChildProcess::spawn(&config.worker_exe, &arg_refs, &env_refs, theirs, Some(rule))?;
        child.note_activity(false);
        ours.set_nonblocking(true)?;
        let mut worker = Self { child, channel: ours, reader: FrameReader::default(), jobs_done: 0 };
        let started = Instant::now();
        let limit = policy.start_timeout;
        let hello = loop {
            match worker.reader.read(&worker.channel) {
                Ok(Some(frame)) => break frame,
                Ok(None) => {}
                Err(_) => {
                    worker.child.kill_now();
                    let no_library = worker.child.wait_bounded(EXIT_STATUS_WAIT) == Some(ChildExit::Code(exit_code::NO_LIBRARY));
                    return Err(if no_library { io::ErrorKind::NotFound } else { io::ErrorKind::BrokenPipe }.into());
                }
            }
            if started.elapsed() > limit {
                return Err(io::ErrorKind::TimedOut.into());
            }
            worker.channel.wait_readable(Some(policy.watch_step))?;
        };
        if hello.kind != Kind::Hello || proto::decode_hello(&hello.payload).is_err() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let footprint = worker.child.memory().map_or(0, |memory| memory.footprint_bytes);
        Ok((worker, footprint))
    }

    /// Sends one frame and waits for one back, watching the worker the whole time. On any stop except a refusal, the worker is finished and the
    /// caller must drop it.
    fn exchange(&mut self, frame: &[u8], handles: &[&Handle], watch: &Watch<'_>) -> Result<Frame, Stop> {
        self.child.note_activity(true);
        let result = self.exchange_watched(frame, handles, watch);
        self.child.note_activity(false);
        result
    }

    fn exchange_watched(&mut self, frame: &[u8], handles: &[&Handle], watch: &Watch<'_>) -> Result<Frame, Stop> {
        let started = Instant::now();
        let deadline = started + watch.policy.page_deadline;
        let mut sent = 0;
        let mut stale_since: Option<Instant> = None;
        loop {
            if sent < frame.len() {
                let attached = if sent == 0 { handles } else { &[] };
                match self.channel.send(&frame[sent..], attached) {
                    Ok(count) => sent += count,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    // The worker has gone before taking the job. Reading finds out why.
                    Err(_) => return Err(self.departed()),
                }
            }
            let read = self.reader.read(&self.channel);
            // Handles only ever go to a worker. Checked before anything else, and they're closed along with the reader's frame.
            if self.reader.has_handles() || matches!(&read, Ok(Some(reply)) if !reply.handles.is_empty()) {
                return Err(Stop::Killed(KillReason::Rejected("a worker sent handles")));
            }
            match read {
                Ok(Some(reply)) => return Ok(reply),
                Ok(None) => {}
                Err(ReadError::Closed | ReadError::Io(_)) => return Err(self.departed()),
                Err(ReadError::Bad(error)) => return Err(Stop::Killed(KillReason::Rejected(error.0))),
            }
            if self.child.memory().is_some_and(|memory| memory.footprint_bytes > watch.policy.memory_limit_bytes) {
                return Err(Stop::Killed(KillReason::MemoryLimit));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Stop::Killed(KillReason::Deadline));
            }
            if watch.is_stale() {
                let since = *stale_since.get_or_insert(now);
                if now.duration_since(since) >= watch.policy.cancel_grace {
                    return Err(Stop::StaleKilled);
                }
            }
            let step = watch.policy.watch_step.min(deadline.saturating_duration_since(now));
            let waited = if sent < frame.len() { self.channel.wait_writable(Some(step)) } else { self.channel.wait_readable(Some(step)) };
            match waited {
                Ok(Wait::Ready | Wait::TimedOut) => {}
                Err(_) => return Err(self.departed()),
            }
        }
    }

    /// The channel closed. Reads the exit status, to tell a clean idle exit from a crash or the worker's own limits. A worker that closed its channel
    /// but won't end reads as a crash, and is left to the reaper.
    fn departed(&mut self) -> Stop {
        self.child.kill_now();
        match self.child.wait_bounded(EXIT_STATUS_WAIT) {
            Some(ChildExit::Code(exit_code::IDLE)) => Stop::WentIdle,
            Some(ChildExit::Code(exit_code::MEMORY)) => Stop::Killed(KillReason::MemoryLimit),
            Some(ChildExit::Code(exit_code::DEADLINE)) => Stop::Killed(KillReason::Deadline),
            _ => Stop::Killed(KillReason::Crashed),
        }
    }
}
