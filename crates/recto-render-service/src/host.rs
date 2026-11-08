//! A document's renderer, from the host's side. See the crate's notes for the rules.

pub mod contents;

use crate::transport::{Frame, FrameReader, ReadError};
use crate::{control, exit_code};
use recto_platform::isolate::{self, Channel, ChildExit, ChildProcess, Handle, IdleRule, SharedMemory, Wait};
use recto_platform::surface::{LinkInvite, PixelSurface, SurfaceSender};
use recto_platform::{FileStamp, file_stamp};
use recto_proto::{self as proto, CharBox, CharRecord, FailCode, Kind, PageFrame, PageSize, ProtoError, RegionReply, RenderJob, TextJob, TextReply, TileKind, TileRect, TileReply, TileStatus, device_size};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// The memory holding a tile's pixels: written by the worker, read by the host. Host-owned, because the host hands it over for one job at a time.
pub type Surface = SharedMemory;

/// Where a tile is to be drawn. Either memory the host reads back, or a surface the compositor can show as it is, with no copy anywhere: see `recto_platform::surface`. Everything else about a job is the same.
#[derive(Clone, Copy, Debug)]
pub enum Target<'a> {
    Shared(&'a SharedMemory),
    Pixels(&'a PixelSurface),
}

impl Target<'_> {
    fn len(self) -> usize {
        match self {
            Self::Shared(memory) => memory.len(),
            Self::Pixels(surface) => surface.len(),
        }
    }

    fn handles(&self) -> Vec<&Handle> {
        match self {
            Self::Shared(memory) => vec![memory.handle()],
            Self::Pixels(_) => Vec::new(),
        }
    }

    fn pixels(self) -> Option<PixelSurface> {
        match self {
            Self::Shared(_) => None,
            Self::Pixels(surface) => Some(surface.clone()),
        }
    }
}

impl<'a> From<&'a SharedMemory> for Target<'a> {
    fn from(memory: &'a SharedMemory) -> Self {
        Self::Shared(memory)
    }
}

impl<'a> From<&'a PixelSurface> for Target<'a> {
    fn from(surface: &'a PixelSurface) -> Self {
        Self::Pixels(surface)
    }
}

/// What a worker may do. Defaults are the decisions from the spike; see `docs/ENGINEERING.md`.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Past this footprint during a job, the worker is killed.
    pub memory_limit_bytes: u64,
    /// The limit for one page retried alone, after it passed `memory_limit_bytes`. Not above `memory_limit_bytes` turns this step off.
    pub heavy_limit_bytes: u64,
    /// That retry happens only with at least this much of the Mac's memory free, as `kern.memorystatus_level` reads it.
    pub heavy_min_free_percent: u32,
    /// How long an export waits for the larger limit before it's refused.
    pub export_slot_wait: Duration,
    /// Past this footprint after a job, the worker is retired before its next job for another page.
    pub retire_bytes: u64,
    /// Bytes of surfaces a worker may be sent before it's retired, as it could be holding on to them all, and no limit on its own memory would see that. Twice this and it goes at once. What a worker can pin is held to this.
    pub surface_bytes_cap: u64,
    /// A job running longer than this has its worker killed.
    pub page_deadline: Duration,
    /// How long a stale job may take to return before its worker is killed.
    pub cancel_grace: Duration,
    /// The worker leaves after this long with no job.
    pub idle_exit: Duration,
    /// A heavy worker leaves after this long with no job, so its memory and the slot don't wait on a page nobody's using.
    pub heavy_idle_exit: Duration,
    /// How many times one document may start a heavy worker. Past this its pages draw at a lower resolution, so a hostile file can't keep asking for a gigabyte.
    pub max_heavy_retries: u32,
    /// How many workers one document may have killed for overrunning. Past this it draws nothing more until asked, so a file of many bad pages ends quickly too.
    pub max_kills_per_document: u32,
    /// A worker still here this long after it should have left is killed, because it's ignoring its own exit.
    pub idle_grace: Duration,
    /// How often a running job's worker is checked.
    pub watch_step: Duration,
    /// How long a new worker gets to say it's ready. Loading PDFium took 2 ms in the spike, so this is only for one that isn't going to.
    pub start_timeout: Duration,
    /// How long a worker's "this can't be opened right now" is taken as the answer, where that isn't about the file's bytes. Long enough that a search of a thousand pages doesn't start a thousand processes, and short enough that it passes by itself.
    pub open_retry_pause: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            // SPEC §3 and spike §12: about 512 MiB, because the worst real page reaches 493 MiB and nothing holds it under that.
            memory_limit_bytes: 512 * 1024 * 1024,
            // Owner, 2026-11-06: 512 MiB normally, one page alone at up to 1 GiB, then a lower resolution.
            heavy_limit_bytes: 1024 * 1024 * 1024,
            // On 16 GB this is about 4.8 GiB free, several times what the retry can use. Low enough to fire on a working Mac, and high enough that a Mac
            // already short of memory isn't asked for another gigabyte. An 8 GB Mac has 2.4 GiB at this level, still over twice the need.
            heavy_min_free_percent: 30,
            // Far longer than an idle heavy worker can hold the slot, which is 8 s at most, so only real use of it can make an export time out.
            export_slot_wait: Duration::from_secs(60),
            retire_bytes: 64 * 1024 * 1024,
            // About 80 pages of text a tile at a time, or 30 regions of a heavy map, for a restart of 17 ms or the map's own load.
            surface_bytes_cap: 1024 * 1024 * 1024,
            // Platform §2.6. The slowest real page at 2x took 939 ms.
            page_deadline: Duration::from_secs(5),
            // Spike §12: a stale render is gone within 100 ms, by return or kill.
            cancel_grace: Duration::from_millis(90),
            idle_exit: Duration::from_secs(30),
            // Long enough to bridge a pause in scrolling, and a reload costs less than holding a gigabyte.
            heavy_idle_exit: Duration::from_secs(3),
            max_heavy_retries: 8,
            max_kills_per_document: 128,
            idle_grace: Duration::from_secs(5),
            // The worst growth measured is about 650 MiB/s, so this overshoots by a few MiB.
            watch_step: Duration::from_millis(5),
            start_timeout: Duration::from_secs(10),
            open_retry_pause: Duration::from_secs(2),
        }
    }
}

/// Where the worker and PDFium are.
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub worker_exe: PathBuf,
    pub pdfium_lib: PathBuf,
    /// At most one heavy worker at a time across everything sharing this.
    pub heavy_slot: HeavySlot,
    /// For tests that stand in a different worker. Arguments replace the normal ones when `replace_args` is set.
    pub extra_env: Vec<(String, String)>,
    pub replace_args: Option<Vec<OsString>>,
}

/// The one place a heavy worker may run. A viewer takes it without waiting: whoever finds it taken draws a lower-resolution version for now. An export waits for it, on its own job
/// thread, and ahead of viewers while it does. Goes free when the heavy process has really gone, however it went.
#[derive(Clone, Debug, Default)]
pub struct HeavySlot(Arc<SlotShared>);

#[derive(Debug, Default)]
struct SlotShared {
    taken: Mutex<bool>,
    freed: Condvar,
    /// Outside the lock, so a cancel with nobody waiting stays one atomic read.
    waiting: AtomicU32,
}

/// Why a wait for the slot ended without it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotWaitEnd {
    Cancelled,
    TimedOut,
}

impl HeavySlot {
    /// A slot of its own, for a test that shouldn't meet other tests' heavy workers.
    pub fn new() -> Self {
        Self::default()
    }

    /// The session's slot, shared by every document in this process.
    fn session() -> Self {
        static SLOT: OnceLock<HeavySlot> = OnceLock::new();
        SLOT.get_or_init(Self::new).clone()
    }

    fn taken(&self) -> MutexGuard<'_, bool> {
        self.0.taken.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn is_taken(&self) -> bool {
        *self.taken()
    }

    /// How many exports are waiting for it.
    pub fn waiting(&self) -> u32 {
        self.0.waiting.load(Ordering::SeqCst)
    }

    /// For a viewer: at once or not at all, and never ahead of an export that's waiting.
    fn try_take(&self) -> Option<HeavyTicket> {
        let mut taken = self.taken();
        if *taken || self.waiting() > 0 {
            return None;
        }
        *taken = true;
        Some(HeavyTicket(self.clone()))
    }

    /// For an export: sleeps until the slot frees, the time runs out, or `is_cancelled` says so. Blocks its caller, with no polling. Its place in the wait goes with it on every way out.
    fn wait_take(&self, limit: Duration, is_cancelled: impl Fn() -> bool) -> Result<HeavyTicket, SlotWaitEnd> {
        struct Place<'a>(&'a AtomicU32);
        impl Drop for Place<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        // Counted before `is_cancelled` is read. A canceller stores first and reads this after, so at least one of the two sees the other.
        self.0.waiting.fetch_add(1, Ordering::SeqCst);
        let _place = Place(&self.0.waiting);
        let deadline = Instant::now() + limit;
        let mut taken = self.taken();
        loop {
            if is_cancelled() {
                return Err(SlotWaitEnd::Cancelled);
            }
            if !*taken {
                *taken = true;
                return Ok(HeavyTicket(self.clone()));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(SlotWaitEnd::TimedOut);
            }
            taken = self.0.freed.wait_timeout(taken, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
    }

    /// Wakes anyone waiting, so a cancel doesn't have to wait for the slot or the time to run out. Costs one atomic read if there's nobody.
    fn wake_waiters(&self) {
        if self.waiting() > 0 {
            // Taking the lock means a waiter is either before its check or asleep, so this can't slip between them.
            drop(self.taken());
            self.0.freed.notify_all();
        }
    }
}

/// Holds the slot. Lives as long as the heavy process: dropped by the end action when it has gone, however it went.
#[derive(Debug)]
struct HeavyTicket(HeavySlot);

impl Drop for HeavyTicket {
    fn drop(&mut self) {
        *self.0.taken() = false;
        self.0.0.freed.notify_all();
    }
}

/// What a drawing is for. It decides what happens when a page can't have the larger limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DrawPurpose {
    /// A viewer's tile: never waits, and comes back at a lower resolution, saying so, if it must.
    #[default]
    OnScreen,
    /// A file, a print or a snapshot: waits for the larger limit on its job thread, and either comes out sharp or is refused. Never a lower resolution. Owner, 2026-11-07.
    Export,
}

/// Why an export was refused, with nothing written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportRefusal {
    /// This Mac is short of the memory the page needs.
    MemoryTight,
    /// Another heavy page kept the larger limit until the wait ran out.
    WaitTimedOut,
    /// The page passed the larger limit too.
    TooHeavy,
}

impl ServiceConfig {
    /// Finds the worker: in the app's sandboxed helper (`Contents/Helpers/RectoRender.app`) for a program in the app's `Contents/MacOS`, and, in a development build only, beside this
    /// program, or one folder up for a test binary in `deps/`. A release looks nowhere but the helper. This program's path is followed through links first, so `recto` linked onto `PATH` finds the same helper the app uses. PDFium is beside the worker,
    /// in the helper's `Frameworks`, or in the `pdfium` folder `tools/pdfium.sh` fills. In debug builds only, `RECTO_RENDER_SERVICE` and `RECTO_PDFIUM_LIB` override: nothing shipped loads
    /// code from a path in its environment.
    pub fn discover() -> Result<Self, RenderFailure> {
        let exe_name = format!("{}{}", crate::bundle::WORKER, std::env::consts::EXE_SUFFIX);
        let worker_exe = match dev_override("RECTO_RENDER_SERVICE") {
            Some(path) => PathBuf::from(path),
            None => {
                let this = std::env::current_exe().map_err(|_| RenderFailure::WorkerMissing)?;
                let this = std::fs::canonicalize(&this).unwrap_or(this);
                let here = this.parent().ok_or(RenderFailure::WorkerMissing)?;
                // A release starts only the sandboxed helper's worker: nothing beside it could stand in.
                let development = !crate::bundle::RELEASE;
                [crate::bundle::helper_worker(here), development.then(|| here.join(&exe_name)), here.parent().filter(|_| development).map(|up| up.join(&exe_name))]
                    .into_iter()
                    .flatten()
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
                let up = dir.parent();
                let frameworks = up.map(|parent| parent.join("Frameworks").join(&library));
                let fetched = up.map(|parent| parent.join("pdfium").join(&library));
                [Some(beside), frameworks, fetched].into_iter().flatten().find(|path| path.is_file()).ok_or(RenderFailure::LibraryMissing)?
            }
        };
        Ok(Self { worker_exe, pdfium_lib, heavy_slot: HeavySlot::session(), extra_env: Vec::new(), replace_args: None })
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderFailure {
    /// The worker binary isn't where it should be.
    WorkerMissing,
    /// PDFium isn't where it should be. `tools/pdfium.sh` puts it there.
    LibraryMissing,
    /// The worker's walls weren't up, so it read nothing, and why.
    WallsDown(WallsDown),
    /// macOS refused to start the worker at all: permission denied.
    StartRefused,
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
    /// Reading this page's size or text stopped its worker. Not tried again until [`DocRenderer::forget_failures`].
    PageReadFailed { page: u32, reason: KillReason },
    /// An export of this page was refused, and nothing should be written.
    ExportRefused { page: u32, why: ExportRefusal },
    /// The job's generation went stale. For an export, nothing should be written.
    Cancelled,
    /// This page can't be drawn right now: it needs the larger limit, and its smaller version doesn't fit either. Asking again when the reason has cleared may work.
    NotDrawableNow { page: u32, why: LowResWhy },
    /// This document has had too many workers killed. It draws nothing more until [`DocRenderer::forget_failures`].
    DocumentStopped,
    /// Drawing the whole page ran past its time cap. No worker was at fault.
    TookTooLong,
}

/// Why a worker's walls weren't up. Only the first says the app is signed wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallsDown {
    /// It isn't in the App Sandbox: the helper was signed without it, or a release was given a worker that says it isn't walled.
    NotSandboxed,
    /// It was started from inside another sandbox, an agent's for one, and macOS won't start an app's sandbox inside it.
    InsideAnotherSandbox,
    /// macOS gave it no Mach rights to let go of.
    NothingToDrop,
    /// launchd still answered after it let go of its rights.
    LaunchdReachable,
}

/// Why a tile holds a lower-resolution version of its page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LowResWhy {
    /// The page passed the largest limit there is. Trying again wouldn't help.
    TooHeavy,
    /// The page needs the larger limit, and this Mac is short of memory right now.
    MemoryTight,
    /// The page needs the larger limit, and another page has it right now.
    HeavyBusy,
}

impl LowResWhy {
    /// A stable code for agents.
    pub fn code(self) -> &'static str {
        match self {
            Self::TooHeavy => "page_too_heavy",
            Self::MemoryTight => "memory_tight",
            Self::HeavyBusy => "another_heavy_page",
        }
    }

    /// Plain words for a person.
    pub fn message(self) -> &'static str {
        match self {
            Self::TooHeavy => "This page needs more memory than RectoPDF allows, so this is a lower-resolution version.",
            Self::MemoryTight => "This Mac is short of memory right now, so this is a lower-resolution version of a heavy page.",
            Self::HeavyBusy => "Another heavy page is being drawn, so this is a lower-resolution version for now.",
        }
    }

    /// Whether asking again later may get the sharp page.
    pub fn may_clear(self) -> bool {
        !matches!(self, Self::TooHeavy)
    }
}

/// The lower-resolution version in a tile's surface: `rect` of the page at `scale`, for the host to stretch over the place it asked to fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LowRes {
    pub scale: f32,
    pub rect: TileRect,
    pub why: LowResWhy,
}

/// What's known about drawing a page, for the view model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageDrawState {
    /// Nothing has gone wrong.
    Sharp,
    /// It passed the normal limit, so it needs the larger one.
    NeedsHeavy,
    /// It passed the larger limit too, so it's drawn at a lower resolution from now on.
    LowResOnly,
    /// It can't be drawn, and won't be tried again until asked.
    Failed(KillReason),
}

impl RenderFailure {
    /// A stable code for agents.
    pub fn code(self) -> &'static str {
        match self {
            Self::WorkerMissing => "render_worker_missing",
            Self::LibraryMissing => "render_library_missing",
            Self::WallsDown(WallsDown::NotSandboxed) => "renderer_not_sandboxed",
            Self::WallsDown(WallsDown::InsideAnotherSandbox) => "renderer_inside_another_sandbox",
            Self::WallsDown(WallsDown::NothingToDrop) => "renderer_mach_drop_failed",
            Self::WallsDown(WallsDown::LaunchdReachable) => "renderer_launchd_reachable",
            Self::StartRefused => "renderer_start_refused",
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
            Self::PageReadFailed { reason: KillReason::Rejected(_), .. } => "renderer_misbehaved",
            Self::PageReadFailed { reason: KillReason::MemoryLimit, .. } => "page_too_large_to_read",
            Self::PageReadFailed { .. } => "page_unreadable",
            Self::ExportRefused { why: ExportRefusal::MemoryTight, .. } => "export_memory_tight",
            Self::ExportRefused { why: ExportRefusal::WaitTimedOut, .. } => "export_wait_timed_out",
            Self::ExportRefused { why: ExportRefusal::TooHeavy, .. } => "export_page_too_heavy",
            Self::Cancelled => "cancelled",
            Self::NotDrawableNow { .. } => "page_not_drawable_now",
            Self::DocumentStopped => "document_stopped",
            Self::TookTooLong => "render_took_too_long",
        }
    }

    /// Plain words for a person.
    pub fn message(self) -> String {
        match self {
            Self::WorkerMissing => "RectoPDF's page renderer, `recto-render-service`, wasn't found in `Contents/Helpers/RectoRender.app` or beside `recto`, so no page can be read or drawn.".into(),
            Self::LibraryMissing => "RectoPDF's PDF library, `libpdfium.dylib`, wasn't found beside `recto-render-service`, in `../Frameworks/` or in `../pdfium/`, so no page can be read or drawn.".into(),
            Self::WallsDown(WallsDown::NotSandboxed) => "RectoPDF's page renderer wouldn't start, because its sandbox isn't on: this copy of RectoPDF is signed wrong, so it won't read any PDF. Install it again from a release.".into(),
            Self::WallsDown(WallsDown::InsideAnotherSandbox) => "RectoPDF can't start its page renderer from inside another sandbox, such as one an agent runs `recto` in: the renderer has a sandbox of its own, and macOS won't start one inside another. Nothing is wrong with the install.".into(),
            Self::WallsDown(WallsDown::NothingToDrop) => "RectoPDF's page renderer couldn't close itself off from the rest of macOS: this version of macOS gave it no system connections to let go of, which RectoPDF doesn't expect, so it won't read any PDF. Nothing is wrong with the install.".into(),
            Self::WallsDown(WallsDown::LaunchdReachable) => "RectoPDF's page renderer couldn't close itself off from the rest of macOS: it could still reach system services after letting go of its connections, so it won't read any PDF. Nothing is wrong with the install.".into(),
            Self::StartRefused => "macOS refused to start RectoPDF's page renderer (permission denied), so no page can be read or drawn. Something on this Mac, such as a security tool or a sandbox around `recto`, is blocking it.".into(),
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
                format!("Page {} can't be drawn: it needs more memory than RectoPDF allows, even at a lower resolution.", page + 1)
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
            Self::PageReadFailed { page, reason: KillReason::MemoryLimit } => format!("Reading page {} needs more memory than a renderer is allowed.", page + 1),
            Self::PageReadFailed { page, reason: KillReason::Deadline } => format!("Reading page {} took too long.", page + 1),
            Self::PageReadFailed { page, reason: KillReason::Crashed } => format!("Reading page {} stopped the renderer.", page + 1),
            Self::PageReadFailed { page, reason: KillReason::Rejected(_) } => {
                format!("The renderer sent something invalid for page {}, so it was stopped. This file may be malicious.", page + 1)
            }
            Self::ExportRefused { page, why: ExportRefusal::MemoryTight } => {
                format!("Page {} wasn't exported, and no file was written: this Mac is short of the memory it needs. Free some memory and try again.", page + 1)
            }
            Self::ExportRefused { page, why: ExportRefusal::WaitTimedOut } => {
                format!("Page {} wasn't exported, and no file was written: another heavy page was still being drawn when the wait ran out. Try again when it's done.", page + 1)
            }
            Self::ExportRefused { page, why: ExportRefusal::TooHeavy } => {
                format!("Page {} wasn't exported, and no file was written: it needs more memory than RectoPDF allows one page.", page + 1)
            }
            Self::Cancelled => "This was cancelled, and nothing was written.".into(),
            Self::NotDrawableNow { page, why: LowResWhy::HeavyBusy } => {
                format!("Page {} can't be drawn right now: another heavy page is using the memory it needs. It may draw when that's done.", page + 1)
            }
            Self::NotDrawableNow { page, .. } => format!("Page {} can't be drawn right now: this Mac is short of the memory it needs. It may draw when more is free.", page + 1),
            Self::DocumentStopped => "This document has used too much memory too many times, so RectoPDF has stopped drawing it. It may be malicious.".into(),
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
    /// Times a worker ran out of time it hadn't been given: never ready, or too long opening its document with little processor time to show for it. That's the Mac being busy, and says nothing about the file.
    pub too_busy: u32,
    pub cancel_kills: u32,
    pub crashes: u32,
    pub rejections: u32,
    pub idle_exits: u32,
    /// Times a page was retried alone at the larger limit.
    pub heavy_retries: u32,
    /// Workers that said in their hello that their walls were up: the App Sandbox on and their Mach rights dropped.
    pub walled_spawns: u32,
    /// Pages that have needed a heavy worker: it's these that count towards [`Policy::max_heavy_retries`]. Starting one again for the same page, after a rest or after it was sent its fill of surfaces, says nothing new about the document.
    pub heavy_pages: u32,
    /// Tiles that came back at a lower resolution.
    pub low_res_tiles: u32,
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
    /// Where surfaces go, unless this worker didn't make the link, as a stand-in for a test may not.
    surfaces: Option<SurfaceSender>,
    /// Text and drawing jobs finished. Asking a page's size or what's on it doesn't count: that leaves nothing behind that could explain a later failure.
    jobs_done: u32,
    /// This worker's own hard limit.
    limit: u64,
    /// The bytes of surfaces this worker has been sent. Its own footprint doesn't show them, since the host is charged, and a compromised worker can hold on to every one it's given, for as long as it lives.
    surface_bytes_sent: u64,
    /// A heavy worker. Its ticket is in its process's end action, so the slot goes free when the process has really gone.
    heavy: bool,
    /// It said its walls were up.
    walled: bool,
}

/// The kind of worker a job needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerClass {
    Normal,
    Heavy,
}

/// The word that says which generations are stale. The worker sees a shared copy and may be hostile, so the host decides from its own: a worker
/// that scribbles on the shared one can only cancel its own jobs, not make the host believe everything is stale.
#[derive(Debug)]
struct CancelWord {
    shared: SharedMemory,
    cutoff: AtomicU64,
    /// Woken on a cancel, in case an export is asleep waiting for it.
    slot: HeavySlot,
}

impl CancelWord {
    fn is_stale(&self, generation: u64) -> bool {
        generation < self.cutoff.load(Ordering::SeqCst)
    }
}

/// Cancels jobs by generation, from any thread.
#[derive(Clone, Debug)]
pub struct CancelHandle(Arc<CancelWord>);

impl CancelHandle {
    /// Every job with a generation below this is stale. Only ever moves up. Safe from a UI thread: atomics, plus a brief lock and a wake-up only if an export is waiting.
    pub fn cancel_before(&self, generation: u64) {
        // The host's own word first, so by the time a worker can see a job is stale, the host agrees.
        let cutoff = self.0.cutoff.fetch_max(generation, Ordering::SeqCst).max(generation);
        if let Some(word) = self.0.shared.atomic_u64(control::CANCEL_BEFORE_OFFSET) {
            word.store(cutoff, Ordering::SeqCst);
        }
        self.0.slot.wake_waiters();
    }
}

/// Pages whose images are remembered, at 64 boxes of 8 bytes each at most. Many more than the tiles and previews kept, so a page can't have tiles on the screen from before it was forgotten.
const MAX_IMAGE_PLACES: usize = 4096;

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
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub kind: TileKind,
    /// Time inside the worker, from the worker: for reports, not decisions.
    pub render_us: u32,
    /// Set when the surface holds a lower-resolution version rather than what was asked for.
    pub low_res: Option<LowRes>,
    /// For a region: what each tile holds, in [`recto_proto::region_tiles`] order. Empty for one tile, or if `low_res` is set, when `kind` covers all of it.
    pub region_kinds: Vec<TileKind>,
}

/// How a tile call ended, short of failure.
#[derive(Clone, Debug, PartialEq)]
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
    /// The page as it's shown, if boxes were asked for.
    pub frame: PageFrame,
    /// The file's permissions ask that its text isn't copied.
    pub copy_restricted: bool,
}

/// How long to wait for a killed worker's final numbers.
const FINAL_NUMBERS_WAIT: Duration = Duration::from_millis(200);
/// How long to wait to learn why a worker that closed its channel left.
const EXIT_STATUS_WAIT: Duration = Duration::from_secs(2);

/// How many pages that stopped a worker are remembered, so a bomb isn't retried on every scroll.
const MAX_FAILED_PAGES: usize = 64;

/// What's been learned about a page the hard way. Remembered so a hostile page ends quickly: each kind of job that stopped a worker isn't run again.
#[derive(Clone, Copy, Debug)]
struct PageMemory {
    page: u32,
    /// The lowest scale at which it has passed the normal limit, if any. What's learned at a scale holds at that scale and above, since a higher zoom costs more, and a lower zoom gets its own try. Each of these only ever moves down, so what a lower zoom learns doesn't take away what a higher one learned, nor the other way round.
    needs_heavy_from: Option<f32>,
    /// The lowest scale at which it has passed the larger limit too.
    low_res_from: Option<f32>,
    /// The lowest scale at which it couldn't be drawn at all, and why.
    failed_from: Option<(f32, KillReason)>,
    /// Its lower-resolution version stopped a worker while the larger limit was out of reach. Once that's been tried and failed too, every step has failed once.
    low_res_stopped: bool,
    /// It's counted towards the document's allowance of pages that need a heavy worker.
    counted_as_heavy: bool,
    text: Option<KillReason>,
    size: Option<KillReason>,
    /// Drawing it for the hidden-text check stopped a worker. Nothing about drawing it for the screen follows from that.
    check_stopped: bool,
    /// Reads of its text in a row that ran out of time with the Mac too busy to say anything about the page.
    busy_reads: u8,
}

/// Busy reads of a page in a row past which it's remembered as unreadable after all: a worker that hangs looks starved too, and this bounds what a hostile page can cost to that many deadlines.
const MAX_BUSY_READS: u8 = 3;

/// Workers the hidden-text check may have killed in one document, past which pages go unchecked. They don't count towards `max_kills_per_document`, so a copy of a file of slow pages can't stop it drawing, and this keeps them from costing a respawn each for ever.
const MAX_CHECK_KILLS: u32 = 16;

/// How far below the scale a page's state was learned at before a zoom counts as different. So that small changes in zoom don't each run the kills again.
const LOWER_ZOOM_RATIO: f32 = 0.7;

/// How much lower the lower-resolution version is, from the platform report's page texture at about a quarter of the scale (§2.5).
const LOW_RES_DIVISOR: u32 = 4;

/// What a failed open is known to mean, and for how long.
#[derive(Clone, Copy, Debug)]
struct KnownOpenFailure {
    failure: RenderFailure,
    /// When it may have passed. `None` if it's about the file's bytes, which holds until the file is a different one.
    try_again_at: Option<Instant>,
}

/// One document's renderer. Holds the file and a control page, and a worker only while one is needed. A cold tab never has one of these.
pub struct DocRenderer {
    config: ServiceConfig,
    policy: Policy,
    /// Kept to see whether the file there is still this one, and to open it again if a failure was about the old one.
    path: PathBuf,
    file: File,
    /// What the file was when it was opened. A failure about its bytes holds only as long as this does.
    opened_stamp: Option<FileStamp>,
    /// Which version of the file the worker found, by what PDFium read to open it.
    version: Option<u64>,
    password: Option<Vec<u8>>,
    control: Arc<CancelWord>,
    worker: Option<Worker>,
    page_count: Option<u32>,
    next_job: u32,
    slot_wait_total: Duration,
    /// Where the caps start from, moved by `forget_failures`.
    kills_counted_from: u32,
    /// Of those since then, what the hidden-text check took.
    check_kills: u32,
    /// Of those since then, deadlines the Mac was too busy for the worker to meet: nothing about the document, so they don't count towards stopping it.
    busy_kills: u32,
    /// `image_places` has been forgotten, past its cap, and nobody's been told.
    image_places_cleared: bool,
    heavy_retries_counted_from: u32,
    remembered: Vec<PageMemory>,
    /// Each page's largest image in source pixels, for those asked about.
    traits: Vec<(u32, u64)>,
    /// Where each page's images are, for those asked about.
    image_places: HashMap<u32, Option<Vec<[u16; 4]>>>,
    file_changed: bool,
    /// The slot, from an export's wait, on its way to the worker it's for.
    waited_ticket: Option<HeavyTicket>,
    /// Why opening the file failed, for as long as that can be taken as the answer. See [`Self::known_open_failure`].
    open_failure: Option<KnownOpenFailure>,
    /// The page the worker last worked on, while its footprint is past the retirement line. It goes before the next job for any other page.
    retire_after_page: Option<u32>,
    stats: RenderStats,
}

impl DocRenderer {
    /// **Reads the file's structure through the very file this renderer has open,** with the password it opened with, if any, so what's read is of the version being drawn: a file replaced by a rename isn't what this reads, as its pages aren't what's showing. In this process, and the password never leaves it. Nothing if the file can't be shared just now.
    pub fn read_structure<R>(&self, read: impl FnOnce(File, Option<&[u8]>) -> R) -> Option<R> {
        let file = self.file.try_clone().ok()?;
        Some(read(file, self.password.as_deref()))
    }

    /// Whether the file's been seen to be another than the one opened, and that hasn't been taken up yet. It's left to be taken.
    pub fn file_seen_changed(&self) -> bool {
        self.file_changed
    }

    /// Opens the file for reading and prepares a renderer. No process starts until something needs one.
    pub fn new(path: &Path, password: Option<&[u8]>, config: ServiceConfig, policy: Policy) -> Result<Self, RenderFailure> {
        Self::with_file(Self::open_file(path)?, path, password, config, policy)
    }

    /// The part of [`Self::new`] that can block, on a file on a dead volume, for a caller that can't be held up.
    pub fn open_file(path: &Path) -> Result<File, RenderFailure> {
        File::open(path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => RenderFailure::NotFound,
            _ => RenderFailure::Unavailable,
        })
    }

    /// The rest of [`Self::new`], with the file open.
    pub fn with_file(file: File, path: &Path, password: Option<&[u8]>, config: ServiceConfig, policy: Policy) -> Result<Self, RenderFailure> {
        let shared = SharedMemory::create(control::LEN).map_err(|_| RenderFailure::Unavailable)?;
        let slot = config.heavy_slot.clone();
        Ok(Self {
            config,
            policy,
            path: path.to_path_buf(),
            opened_stamp: file_stamp(path).ok(),
            version: None,
            file,
            password: password.map(<[u8]>::to_vec),
            control: Arc::new(CancelWord { shared, cutoff: AtomicU64::new(0), slot }),
            worker: None,
            page_count: None,
            next_job: 0,
            slot_wait_total: Duration::ZERO,
            kills_counted_from: 0,
            heavy_retries_counted_from: 0,
            remembered: Vec::new(),
            traits: Vec::new(),
            image_places: HashMap::new(),
            image_places_cleared: false,
            check_kills: 0,
            busy_kills: 0,
            file_changed: false,
            waited_ticket: None,
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

    /// Whether this generation has been cancelled, so an answer given for it may only mean that.
    pub fn is_stale(&self, generation: u64) -> bool {
        self.control.is_stale(generation)
    }

    /// Whether the file has been seen to be another one since this was last asked: on a page count that no longer agrees when a worker opens it, or when a failed file is tried again. So what was worked out from it no longer holds.
    /// Which version of the file a worker's said this is, if one has. Asks nothing.
    pub fn known_version(&self) -> Option<u64> {
        self.version
    }

    pub fn take_file_changed(&mut self) -> bool {
        std::mem::take(&mut self.file_changed)
    }

    /// Whether the places of a page's images have been forgotten since this was last asked, with a lot of pages looked up. It says nothing about the file: it's for whoever decided something from those places, which might not be decided the same way twice.
    pub fn take_image_places_cleared(&mut self) -> bool {
        std::mem::take(&mut self.image_places_cleared)
    }

    /// Leaves the running worker's surface link broken, as a worker having gone in the moment since it was last looked at leaves it. That moment can't be hit from outside.
    #[doc(hidden)]
    pub fn break_surface_link_for_test(&mut self) {
        if let Some(worker) = &mut self.worker {
            worker.surfaces = Some(SurfaceSender::dead());
        }
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

    /// Whether the current worker said its walls were up, if there is one.
    pub fn worker_walled(&mut self) -> Option<bool> {
        self.reap_if_gone();
        self.worker.as_ref().map(|worker| worker.walled)
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

    /// Clears the memory of which pages stopped a worker, and the count that stops a document. For the reader's "try again". In ordinary use failures also clear by themselves: a lower zoom gets its
    /// own try, a failed open is tried again once the file is a different one or the moment has passed, and closing the tab drops all of this. An agent's command starts with a clean slate each time.
    pub fn forget_failures(&mut self) {
        self.remembered.clear();
        self.open_failure = None;
        self.kills_counted_from = self.kills();
        self.check_kills = 0;
        self.busy_kills = 0;
        self.heavy_retries_counted_from = self.stats.heavy_pages;
    }

    /// The page count, if a worker's said. Asks nothing.
    pub fn known_page_count(&self) -> Option<u32> {
        self.page_count
    }

    /// Starts a worker if none is running, and returns the page count. Most callers can just ask for what they want.
    pub fn page_count(&mut self) -> Result<u32, RenderFailure> {
        self.ensure_worker(WorkerClass::Normal, None).map_err(RenderFailure::from)?;
        self.page_count.ok_or(RenderFailure::Unavailable)
    }

    /// The worst that's known about drawing a page, at whatever scale taught it, for the view model.
    pub fn page_draw_state(&self, page: u32) -> PageDrawState {
        self.page_draw_state_at(page, f32::INFINITY)
    }

    /// What's known about drawing a page at this scale. A page too heavy at a high zoom may be fine at a lower one, so a lower one has its own answer, and what each zoom learned stays with it.
    pub fn page_draw_state_at(&self, page: u32, scale: f32) -> PageDrawState {
        let Some(memory) = self.remembered(page) else { return PageDrawState::Sharp };
        let reaches = |from: f32| scale >= from * LOWER_ZOOM_RATIO;
        match memory {
            PageMemory { failed_from: Some((from, reason)), .. } if reaches(*from) => PageDrawState::Failed(*reason),
            PageMemory { low_res_from: Some(from), .. } if reaches(*from) => PageDrawState::LowResOnly,
            PageMemory { needs_heavy_from: Some(from), .. } if reaches(*from) => PageDrawState::NeedsHeavy,
            _ => PageDrawState::Sharp,
        }
    }

    /// Notes what a scale taught. Each only moves down: a page that needed more at a lower zoom needs at least that at a higher one.
    fn set_draw(&mut self, page: u32, scale: f32, draw: PageDrawState) {
        let lowest = |known: Option<f32>| Some(known.map_or(scale, |known| known.min(scale)));
        let memory = self.remember(page);
        match draw {
            PageDrawState::Sharp => {}
            PageDrawState::NeedsHeavy => memory.needs_heavy_from = lowest(memory.needs_heavy_from),
            PageDrawState::LowResOnly => memory.low_res_from = lowest(memory.low_res_from),
            PageDrawState::Failed(reason) => memory.failed_from = Some((lowest(memory.failed_from.map(|(from, _)| from)).unwrap_or(scale), reason)),
        }
    }

    /// A page's size in points, after rotation, validated.
    pub fn page_size(&mut self, page: u32) -> Result<(f32, f32), RenderFailure> {
        if let Some(reason) = self.remembered(page).and_then(|memory| memory.size) {
            return Err(RenderFailure::PageReadFailed { page, reason });
        }
        let job_id = self.next_job_id();
        let frame = proto::encode_size_request(job_id, page);
        let size = self.run_job(job_id, page, false, WorkerClass::Normal, None, &[], &frame, |reply| match reply.kind {
            Kind::Size => {
                let size = PageSize::decode(&reply.payload)?;
                proto::validate_page_size(job_id, &size)?;
                Ok(size)
            }
            _ => Err(ProtoError("the wrong kind of reply to a size request")),
        });
        match size {
            Ok(size) => Ok((size.width, size.height)),
            Err(JobFailure::Stopped(reason)) => {
                self.remember(page).size = Some(reason);
                Err(RenderFailure::PageReadFailed { page, reason })
            }
            Err(failure) => Err(failure.into()),
        }
    }

    /// Draws one tile into `surface` and validates the reply before saying so. Blocks its caller, though never on another page: never call this from the UI thread.
    ///
    /// A page too heavy for its worker goes down a sequence, and each step is remembered so a hostile page ends quickly (owner, 2026-11-06):
    /// 1. The normal limit.
    /// 2. That page alone at the larger limit, if the Mac has the memory and no other page has the slot.
    /// 3. A lower-resolution version, reported in [`Tile::low_res`].
    /// 4. [`RenderFailure::PageFailed`], remembered.
    pub fn render_tile<'a>(&mut self, request: &TileRequest, surface: impl Into<Target<'a>>) -> Result<TileResult, RenderFailure> {
        self.draw_with(request, surface.into(), DrawPurpose::OnScreen, 0)
    }

    /// The largest image on a page in source pixels, a hint for how to draw it. Remembered per page, up to a cap. If looking at the page stops a worker, that's noted as drawing would have noted
    /// it, and the answer is zero. Likewise if `generation` goes stale, without remembering that.
    pub fn largest_image_pixels(&mut self, page: u32, generation: u64) -> Result<u64, RenderFailure> {
        if let Some(&(_, pixels)) = self.traits.iter().find(|(known, _)| *known == page) {
            return Ok(pixels);
        }
        if self.page_draw_state(page) != PageDrawState::Sharp || self.control.is_stale(generation) || self.stopped() {
            return Ok(0);
        }
        let job_id = self.next_job_id();
        let frame = proto::encode_traits_request(job_id, page);
        // Loads this page, which is what the next job for it needs anyway, so it leaves nothing that could explain that job failing.
        let reply = self.run_job(job_id, page, false, WorkerClass::Normal, Some(generation), &[], &frame, |reply| match reply.kind {
            Kind::Traits => {
                let traits = proto::PageTraits::decode(&reply.payload)?;
                if traits.job_id == job_id { Ok(traits.largest_image_pixels) } else { Err(ProtoError("traits answer a different job")) }
            }
            _ => Err(ProtoError("the wrong kind of reply to a traits request")),
        });
        let pixels = match reply {
            Ok(pixels) => pixels,
            Err(JobFailure::StaleKilled) => return Ok(0),
            // At what scale isn't known, so it holds at every one, until a lower zoom's own draw says otherwise. Which is the same as the page's load being too heavy.
            Err(JobFailure::Stopped(KillReason::MemoryLimit)) => {
                self.set_draw(page, 0.0, PageDrawState::NeedsHeavy);
                0
            }
            Err(JobFailure::Stopped(reason)) => return Err(self.give_up_drawing(page, 0.0, reason)),
            Err(failure) => return Err(failure.into()),
        };
        if self.traits.len() >= MAX_FAILED_PAGES {
            self.traits.remove(0);
        }
        self.traits.push((page, pixels));
        Ok(pixels)
    }

    /// Where a page's images are, as parts of the page in 65535ths, or `None` if it isn't known: this job went stale, the page isn't one that can be asked about, or the asking failed. Those aren't "there are no images", and a caller mustn't treat them so. Asked once per page, up to a cap, only for dark paper. Whatever stops the answer gives none, since it only decides how pixels are coloured, and drawing will find the same trouble and deal with it.
    pub fn image_boxes(&mut self, page: u32, generation: u64) -> Option<Vec<[u16; 4]>> {
        if let Some(known) = self.image_places.get(&page) {
            return known.clone();
        }
        // Stale says nothing about the page, so nothing's remembered.
        if self.control.is_stale(generation) {
            return None;
        }
        // Only a page that can't be drawn as it is goes unasked. One that needed the larger limit is an ordinary page of a certain size, a page of text with a large figure say.
        let state = self.page_draw_state(page);
        let boxes = if matches!(state, PageDrawState::LowResOnly | PageDrawState::Failed(_)) || self.stopped() {
            None
        } else {
            // The worker that's there, which has just drawn it. With none, one that can hold the page.
            let class = if self.worker.is_none() && state == PageDrawState::NeedsHeavy { WorkerClass::Heavy } else { WorkerClass::Normal };
            let job_id = self.next_job_id();
            let frame = proto::encode_images_request(job_id, page);
            let reply = self.run_job(job_id, page, false, class, Some(generation), &[], &frame, |reply| match reply.kind {
                Kind::Images => {
                    let images = proto::PageImages::decode(&reply.payload)?;
                    if images.job_id == job_id { Ok(images.boxes) } else { Err(ProtoError("image boxes answer a different job")) }
                }
                _ => Err(ProtoError("the wrong kind of reply to an images request")),
            });
            if self.control.is_stale(generation) {
                return None;
            }
            reply.ok()
        };
        // One decision a page: its tiles are drawn at different times, and none of them may disagree. What's cached past this cap is long off the screen.
        if self.image_places.len() >= MAX_IMAGE_PLACES {
            self.image_places.clear();
            self.image_places_cleared = true;
        }
        self.image_places.insert(page, boxes.clone());
        boxes
    }

    /// Draws a region of the page in one pass, cut into tiles of `tile_edge` for the caller to hand out. Cheaper than one pass per tile whenever several tiles are needed: see
    /// `docs/ENGINEERING.md`. Goes down the same sequence as a tile, is cancelled by generation like one, and is limited to [`recto_proto::MAX_REGION_BYTES`], so its surface is
    /// part of what the worker's memory limit sees. Rows sit `request.stride` apart in `surface`.
    pub fn render_region<'a>(&mut self, request: &TileRequest, tile_edge: u32, surface: impl Into<Target<'a>>) -> Result<TileResult, RenderFailure> {
        if tile_edge == 0 {
            return Err(RenderFailure::BadRequest);
        }
        self.draw_with(request, surface.into(), DrawPurpose::OnScreen, tile_edge)
    }

    /// As [`Self::render_tile`], for a stated purpose. [`DrawPurpose::Export`] is for a file, a print or a snapshot, and differs in one way: when the page needs the larger limit, it sleeps
    /// until it can have it, up to [`Policy::export_slot_wait`], and then draws sharp, or fails with [`RenderFailure::ExportRefused`]. It never draws a lower resolution. So it can block
    /// for a long time: call it only on a worker or job thread, never a UI thread. [`CancelHandle::cancel_before`] ends the wait at once, with [`TileResult::Cancelled`].
    pub fn render_for(&mut self, purpose: DrawPurpose, request: &TileRequest, surface: &Surface) -> Result<TileResult, RenderFailure> {
        self.draw_with(request, surface.into(), purpose, 0)
    }

    /// How long this document's exports have slept waiting for the larger limit. Time spent asleep isn't time spent drawing, so a cap on drawing can leave it out.
    pub fn time_waiting_for_slot(&self) -> Duration {
        self.slot_wait_total
    }

    fn draw_with(&mut self, request: &TileRequest, surface: Target<'_>, purpose: DrawPurpose, tile_edge: u32) -> Result<TileResult, RenderFailure> {
        let page = request.page;
        // Checked before anything else, so a request that can't be met starts no process.
        let check = RenderJob { generation: request.generation, job_id: 0, page, scale: request.scale, rect: request.rect, stride: request.stride, tile_edge };
        check.check().map_err(|_| RenderFailure::BadRequest)?;
        if check.bytes_needed() > surface.len() as u64 {
            return Err(RenderFailure::BadRequest);
        }
        if self.control.is_stale(request.generation) {
            return Ok(TileResult::Cancelled { worker_killed: false });
        }
        self.reap_if_gone();
        self.retire_if_other_page(page);
        let scale = request.scale;
        loop {
            // A cancel between steps ends it here: a stale job takes no slot and starts no heavy worker.
            if self.control.is_stale(request.generation) {
                return Ok(TileResult::Cancelled { worker_killed: false });
            }
            let state = self.page_draw_state_at(page, scale);
            if !matches!(state, PageDrawState::Failed(_)) && self.stopped() {
                return Err(RenderFailure::DocumentStopped);
            }
            match state {
                PageDrawState::Failed(reason) => return Err(RenderFailure::PageFailed { page, reason }),
                PageDrawState::LowResOnly => match purpose {
                    DrawPurpose::OnScreen if self.remembered(page).is_some_and(|memory| memory.low_res_stopped) => {
                        // Its smaller version already failed, and now the larger limit has too. That's every step, once.
                        return Err(self.give_up_drawing(page, scale / LOW_RES_DIVISOR as f32, KillReason::MemoryLimit));
                    }
                    DrawPurpose::OnScreen => return self.low_res(request, surface, LowResWhy::TooHeavy),
                    DrawPurpose::Export => return Err(RenderFailure::ExportRefused { page, why: ExportRefusal::TooHeavy }),
                },
                PageDrawState::Sharp => match self.draw_once(request, surface, WorkerClass::Normal, tile_edge) {
                    Err(JobFailure::Stopped(KillReason::MemoryLimit)) => self.set_draw(page, scale, PageDrawState::NeedsHeavy),
                    Err(JobFailure::Stopped(reason)) => return Err(self.give_up_drawing(page, 0.0, reason)),
                    other => return other_result(other),
                },
                PageDrawState::NeedsHeavy => {
                    let retries = self.stats.heavy_pages - self.heavy_retries_counted_from;
                    let counted = self.remembered(page).is_some_and(|memory| memory.counted_as_heavy);
                    let already_heavy = self.worker.as_ref().is_some_and(|worker| worker.heavy);
                    if self.policy.heavy_limit_bytes <= self.policy.memory_limit_bytes || (!already_heavy && !counted && retries >= self.policy.max_heavy_retries) {
                        self.set_draw(page, scale, PageDrawState::LowResOnly);
                        continue;
                    }
                    if !already_heavy && !self.memory_to_spare() {
                        return match purpose {
                            DrawPurpose::OnScreen => self.low_res_for_now(request, surface, LowResWhy::MemoryTight),
                            DrawPurpose::Export => Err(RenderFailure::ExportRefused { page, why: ExportRefusal::MemoryTight }),
                        };
                    }
                    if !counted {
                        self.remember(page).counted_as_heavy = true;
                        self.stats.heavy_pages += 1;
                    }
                    if purpose == DrawPurpose::Export && !already_heavy && self.waited_ticket.is_none() {
                        match self.wait_for_slot(request.generation) {
                            Ok(ticket) => self.waited_ticket = Some(ticket),
                            Err(SlotWaitEnd::Cancelled) => return Ok(TileResult::Cancelled { worker_killed: false }),
                            Err(SlotWaitEnd::TimedOut) => return Err(RenderFailure::ExportRefused { page, why: ExportRefusal::WaitTimedOut }),
                        }
                        // A long wait may have changed that. Dropping the ticket frees the slot for whoever's next.
                        if !self.memory_to_spare() {
                            self.waited_ticket = None;
                            return Err(RenderFailure::ExportRefused { page, why: ExportRefusal::MemoryTight });
                        }
                    }
                    let drawn = self.draw_once(request, surface, WorkerClass::Heavy, tile_edge);
                    // If no worker took it, as when the file can't open, it mustn't stay here holding the slot.
                    self.waited_ticket = None;
                    match drawn {
                        Err(JobFailure::HeavyBusy) => match purpose {
                            DrawPurpose::OnScreen => return self.low_res_for_now(request, surface, LowResWhy::HeavyBusy),
                            // Its worker left between jobs and someone else has the slot now. Back to waiting, within the same limits.
                            DrawPurpose::Export => {}
                        },
                        Err(JobFailure::Stopped(KillReason::MemoryLimit)) => self.set_draw(page, scale, PageDrawState::LowResOnly),
                        Err(JobFailure::Stopped(reason)) => return Err(self.give_up_drawing(page, 0.0, reason)),
                        other => return other_result(other),
                    }
                }
            }
        }
    }

    /// Whether this document has had too many workers killed.
    fn stopped(&self) -> bool {
        (self.kills() - self.kills_counted_from).saturating_sub(self.check_kills).saturating_sub(self.busy_kills) >= self.policy.max_kills_per_document
    }

    fn kills(&self) -> u32 {
        self.stats.memory_kills + self.stats.deadline_kills + self.stats.crashes + self.stats.rejections
    }

    /// The lower-resolution version while the larger limit is out of reach. If that version is known not to fit, says so without another try, and the page isn't given up on: the larger limit
    /// was never tried, and it may yet be.
    fn low_res_for_now(&mut self, request: &TileRequest, surface: Target<'_>, why: LowResWhy) -> Result<TileResult, RenderFailure> {
        if self.remembered(request.page).is_some_and(|memory| memory.low_res_stopped) {
            return Err(RenderFailure::NotDrawableNow { page: request.page, why });
        }
        self.low_res(request, surface, why)
    }

    fn memory_to_spare(&self) -> bool {
        recto_platform::memory_free_percent().is_some_and(|free| free >= self.policy.heavy_min_free_percent)
    }

    /// Sleeps for the slot. This document's own heavy worker can't be what's holding it: `retire_if_other_page` and the check above saw to that.
    fn wait_for_slot(&mut self, generation: u64) -> Result<HeavyTicket, SlotWaitEnd> {
        let started = Instant::now();
        let control = self.control.clone();
        let result = self.config.heavy_slot.wait_take(self.policy.export_slot_wait, || control.is_stale(generation));
        self.slot_wait_total += started.elapsed();
        result
    }

    /// Step 3. Draws the part of the page this tile covers, at a quarter of the scale, into the same surface: it needs less room, never more.
    fn low_res(&mut self, request: &TileRequest, surface: Target<'_>, why: LowResWhy) -> Result<TileResult, RenderFailure> {
        let page = request.page;
        let scale = request.scale / LOW_RES_DIVISOR as f32;
        // Nothing lower to draw at this zoom, and that says nothing about other zooms, so nothing is remembered.
        if scale < proto::MIN_SCALE {
            return Err(RenderFailure::TooLargeAtThisScale);
        }
        let (width, height) = self.page_size(page)?;
        let (full_width, full_height) = device_size(width, height, scale).map_err(|_| RenderFailure::TooLargeAtThisScale)?;
        let x = (request.rect.x / LOW_RES_DIVISOR).min(full_width - 1);
        let y = (request.rect.y / LOW_RES_DIVISOR).min(full_height - 1);
        let right = (request.rect.x.saturating_add(request.rect.width)).div_ceil(LOW_RES_DIVISOR).clamp(x + 1, full_width);
        let bottom = (request.rect.y.saturating_add(request.rect.height)).div_ceil(LOW_RES_DIVISOR).clamp(y + 1, full_height);
        let rect = TileRect { x, y, width: right - x, height: bottom - y };
        let smaller = TileRequest { rect, scale, ..*request };
        match self.draw_once(&smaller, surface, WorkerClass::Normal, 0) {
            Ok(TileResult::Done(tile)) => {
                self.stats.low_res_tiles += 1;
                Ok(TileResult::Done(Tile { low_res: Some(LowRes { scale, rect, why }), ..tile }))
            }
            Err(JobFailure::Stopped(KillReason::MemoryLimit)) if why.may_clear() => {
                self.remember(page).low_res_stopped = true;
                Err(RenderFailure::NotDrawableNow { page, why })
            }
            Err(JobFailure::Stopped(KillReason::MemoryLimit)) => Err(self.give_up_drawing(page, scale, KillReason::MemoryLimit)),
            Err(JobFailure::Stopped(reason)) => Err(self.give_up_drawing(page, 0.0, reason)),
            other => other_result(other),
        }
    }

    /// One job, in one class of worker. A stopped worker comes back as [`JobFailure::Stopped`], for the sequence to decide on.
    fn draw_once(&mut self, request: &TileRequest, surface: Target<'_>, class: WorkerClass, tile_edge: u32) -> Result<TileResult, JobFailure> {
        let job = RenderJob {
            generation: request.generation,
            job_id: self.next_job_id(),
            page: request.page,
            scale: request.scale,
            rect: request.rect,
            stride: request.stride,
            tile_edge,
        };
        job.check().map_err(|_| RenderFailure::BadRequest)?;
        if job.bytes_needed() > surface.len() as u64 {
            return Err(RenderFailure::BadRequest.into());
        }
        let frame = job.encode();
        let surface_len = surface.len() as u64;
        let control = self.control.clone();
        let handles = surface.handles();
        let sends = surface.pixels();
        let reply = self.run_job_with(job.job_id, request.page, true, class, Some(request.generation), &handles, sends.as_ref(), &frame, |reply| {
            let (tile, kinds) = match (reply.kind, tile_edge) {
                (Kind::Tile, 0) => {
                    let tile = TileReply::decode(&reply.payload)?;
                    proto::validate_tile(&job, &tile, surface_len)?;
                    (tile, Vec::new())
                }
                (Kind::Region, 1..) => {
                    let region = RegionReply::decode(&reply.payload)?;
                    proto::validate_region(&job, &region, surface_len)?;
                    (region.tile, region.kinds)
                }
                _ => return Err(ProtoError("the wrong kind of reply to a tile")),
            };
            // The host's word moves first, so a worker can't honestly see a cancel the host hasn't made.
            if tile.status == TileStatus::Cancelled && !control.is_stale(job.generation) {
                return Err(ProtoError("a job cancelled that nobody cancelled"));
            }
            Ok((tile, kinds))
        });
        match reply {
            Ok((tile, region_kinds)) => Ok(match tile.status {
                // Cancelled while it was finishing. The caller moved on, so this tile must never reach the screen.
                TileStatus::Done(_) if self.control.is_stale(request.generation) => TileResult::Cancelled { worker_killed: false },
                TileStatus::Done(kind) => TileResult::Done(Tile { kind, render_us: tile.render_us, low_res: None, region_kinds }),
                TileStatus::Cancelled => TileResult::Cancelled { worker_killed: false },
            }),
            Err(JobFailure::StaleKilled) => Ok(TileResult::Cancelled { worker_killed: true }),
            Err(failure) => Err(failure),
        }
    }

    /// The end of the sequence: remembered, and said plainly. `from_scale` is the lowest scale that failed. Zero means every scale, for what isn't about memory.
    fn give_up_drawing(&mut self, page: u32, from_scale: f32, reason: KillReason) -> RenderFailure {
        self.set_draw(page, from_scale, PageDrawState::Failed(reason));
        RenderFailure::PageFailed { page, reason }
    }

    /// A page's characters, with a box per character if `boxes`, and the hidden-text check if `hidden_check`. If drawing the page for the check stops the worker, or a page is known to, the text still comes back with `hidden_checked` false, so "not known" never reads as "nothing hidden". **A check that fails is about the check.** It draws the whole page at 2x, which says nothing about drawing a tile of it at the zoom on screen, so nothing about the page's drawing is touched: a page that drew in 3.2 s at half zoom used to say "took too long to draw" for the rest of the session after one copy. It's remembered as not to be tried again, and the kills it took don't count towards stopping the document.
    pub fn page_characters(&mut self, page: u32, boxes: bool, hidden_check: bool) -> Result<PageCharacters, RenderFailure> {
        self.read_characters(page, boxes, hidden_check, None)
    }

    /// The same, for a job someone can lose interest in: past `generation`, the check stops where it is, and a worker that doesn't stop goes. See [`Self::cancel_handle`].
    pub fn page_characters_for(&mut self, page: u32, boxes: bool, hidden_check: bool, generation: u64) -> Result<PageCharacters, RenderFailure> {
        self.read_characters(page, boxes, hidden_check, Some(generation))
    }

    fn read_characters(&mut self, page: u32, boxes: bool, hidden_check: bool, generation: Option<u64>) -> Result<PageCharacters, RenderFailure> {
        if let Some(reason) = self.remembered(page).and_then(|memory| memory.text) {
            return Err(RenderFailure::PageReadFailed { page, reason });
        }
        if self.stopped() {
            return Err(RenderFailure::DocumentStopped);
        }
        let may_check = hidden_check && self.check_kills < MAX_CHECK_KILLS && !self.remembered(page).is_some_and(|memory| memory.check_stopped) && self.page_draw_state_at(page, 2.0) == PageDrawState::Sharp;
        if may_check {
            let kills = self.kills();
            let busy_before = self.busy_kills;
            match self.text_job(page, boxes, true, generation) {
                // A worker that breaks the protocol is that worker's doing, whatever it was asked.
                Err(JobFailure::Stopped(reason @ KillReason::Rejected(_))) => {
                    self.remember(page).text = Some(reason);
                    return Err(RenderFailure::PageReadFailed { page, reason });
                }
                Err(JobFailure::Stopped(_)) => {
                    // A busy kill's already left out of the count; it isn't left out twice.
                    let busy = self.busy_kills - busy_before;
                    self.check_kills = self.check_kills.saturating_add((self.kills() - kills).saturating_sub(busy));
                    self.remember(page).check_stopped = true;
                }
                other => return other.map_err(RenderFailure::from),
            }
        }
        let busy = self.stats.too_busy;
        match self.text_job(page, boxes, false, generation) {
            // The Mac was too busy for the read to get its time: nothing's known about the page, so it isn't remembered, and the next ask reads it again. Before, a busy moment left a page unreadable for the session, the copy of it saying it took too long. A worker that hangs looks the same, so after [`MAX_BUSY_READS`] of them in a row the page is remembered after all, and a hostile page costs a deadline at most that many times.
            Err(JobFailure::Stopped(reason @ KillReason::Deadline)) if self.stats.too_busy > busy => {
                let memory = self.remember(page);
                memory.busy_reads = memory.busy_reads.saturating_add(1);
                if memory.busy_reads >= MAX_BUSY_READS {
                    memory.text = Some(reason);
                }
                Err(RenderFailure::PageReadFailed { page, reason })
            }
            Err(JobFailure::Stopped(reason)) => {
                self.remember(page).text = Some(reason);
                Err(RenderFailure::PageReadFailed { page, reason })
            }
            Ok(read) => {
                if let Some(memory) = self.remembered.iter_mut().find(|memory| memory.page == page) {
                    memory.busy_reads = 0;
                }
                Ok(read)
            }
            other => other.map_err(RenderFailure::from),
        }
    }

    fn text_job(&mut self, page: u32, boxes: bool, hidden_check: bool, generation: Option<u64>) -> Result<PageCharacters, JobFailure> {
        let job = TextJob { job_id: self.next_job_id(), page, boxes, hidden_check, generation: generation.unwrap_or(proto::NEVER_STALE) };
        let frame = job.encode();
        let reply = self.run_job(job.job_id, page, true, WorkerClass::Normal, generation, &[], &frame, |reply| match reply.kind {
            Kind::Text => {
                let text = TextReply::decode(&reply.payload)?;
                proto::validate_text(&job, &text)?;
                Ok(text)
            }
            _ => Err(ProtoError("the wrong kind of reply to a text request")),
        })?;
        Ok(PageCharacters { chars: reply.chars, boxes: reply.boxes, hidden_checked: reply.hidden_checked, frame: reply.frame, copy_restricted: reply.copy_restricted })
    }

    fn remembered(&self, page: u32) -> Option<&PageMemory> {
        self.remembered.iter().find(|memory| memory.page == page)
    }

    /// This page's memory, made if need be. Oldest goes first past the cap, and a forgotten page only costs another try.
    fn remember(&mut self, page: u32) -> &mut PageMemory {
        let at = match self.remembered.iter().position(|memory| memory.page == page) {
            Some(at) => at,
            None => {
                if self.remembered.len() >= MAX_FAILED_PAGES {
                    self.remembered.remove(0);
                }
                self.remembered.push(PageMemory { page, counted_as_heavy: false, needs_heavy_from: None, low_res_from: None, failed_from: None, low_res_stopped: false, text: None, size: None, check_stopped: false, busy_reads: 0 });
                self.remembered.len() - 1
            }
        };
        &mut self.remembered[at]
    }

    /// Retires a worker waiting to go, if this job is for another page.
    fn retire_if_other_page(&mut self, page: u32) {
        if self.retire_after_page.is_some_and(|last| last != page) {
            self.stats.retirements += 1;
            self.bury_worker();
        }
    }

    fn next_job_id(&mut self) -> u32 {
        self.next_job = self.next_job.wrapping_add(1);
        self.next_job
    }

    /// Sends a job and interprets the reply, starting and replacing workers as needed. A stopped worker gets one more try in a fresh normal one, because what
    /// stopped it may be what earlier pages left behind. After that, the caller decides what a stopped worker means. A heavy worker gets no such retry: it holds
    /// one page alone, so there's nothing to blame but the page.
    #[allow(clippy::too_many_arguments)]
    fn run_job<T>(
        &mut self,
        job_id: u32,
        page: u32,
        leaves_something_behind: bool,
        class: WorkerClass,
        generation: Option<u64>,
        handles: &[&Handle],
        frame: &[u8],
        decode: impl Fn(&mut Frame) -> Result<T, ProtoError>,
    ) -> Result<T, JobFailure> {
        self.run_job_with(job_id, page, leaves_something_behind, class, generation, handles, None, frame, decode)
    }

    /// As [`Self::run_job`], with a surface for the worker to draw in. It goes ahead of the frame, each time the job is tried.
    #[allow(clippy::too_many_arguments)]
    fn run_job_with<T>(
        &mut self,
        job_id: u32,
        page: u32,
        leaves_something_behind: bool,
        class: WorkerClass,
        generation: Option<u64>,
        handles: &[&Handle],
        surface: Option<&PixelSurface>,
        frame: &[u8],
        decode: impl Fn(&mut Frame) -> Result<T, ProtoError>,
    ) -> Result<T, JobFailure> {
        self.retire_if_other_page(page);
        // Past twice the cap it goes whatever page it's on: a one-page file would otherwise never get to its retirement.
        if self.worker.as_ref().is_some_and(|worker| worker.surface_bytes_sent > self.policy.surface_bytes_cap.saturating_mul(2)) {
            self.stats.retirements += 1;
            self.bury_worker();
        }
        let mut retried = false;
        let mut idle_retries = 0;
        loop {
            self.ensure_worker(class, generation)?;
            let Some(worker) = self.worker.as_mut() else { return Err(RenderFailure::Unavailable.into()) };
            let fresh = worker.jobs_done == 0;
            let is_heavy = worker.heavy;
            let watch = Watch { limit: worker.limit, policy: &self.policy, control: &self.control, generation };
            // A worker with no link can't take this. That's only a stand-in, so it's a plain failure rather than a kill.
            if let Some(surface) = surface {
                let Some(link) = &worker.surfaces else { return Err(RenderFailure::Unavailable.into()) };
                // The send failing means the worker's gone, likely by its own idle rule since it was last looked at. The same as finding that out from its reply: it's nothing about the document.
                if link.send(surface).is_err() {
                    self.bury_worker();
                    self.stats.idle_exits += 1;
                    idle_retries += 1;
                    if idle_retries > 2 {
                        self.stats.crashes += 1;
                        return Err(JobFailure::Stopped(KillReason::Crashed));
                    }
                    continue;
                }
                worker.surface_bytes_sent += surface.len() as u64;
            }
            let outcome = worker.exchange(frame, handles, &watch).and_then(|mut reply| {
                if reply.kind == Kind::Failed {
                    let (answered, code) = proto::decode_failed(&reply.payload).map_err(|error| Stop::Killed(KillReason::Rejected(error.0)))?;
                    if answered != job_id {
                        return Err(Stop::Killed(KillReason::Rejected("a failure for a different job")));
                    }
                    return Err(Stop::Refused(code));
                }
                decode(&mut reply).map_err(|error| Stop::Killed(KillReason::Rejected(error.0)))
            });
            self.stats.jobs += 1;
            match outcome {
                Ok(value) => {
                    if leaves_something_behind {
                        worker.jobs_done += 1;
                    }
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
                        return Err(JobFailure::Stopped(KillReason::Crashed));
                    }
                }
                Err(Stop::Killed(reason)) => {
                    let cpu_ns = self.bury_worker_noting_cpu();
                    // Out of time with too little of the processor to have been the cause: the Mac was busy, as `open_in` reckons it, and it says nothing about the page.
                    if matches!(reason, KillReason::Deadline) && cpu_ns.is_some_and(|cpu| was_starved(cpu, self.policy.page_deadline)) {
                        self.stats.too_busy += 1;
                        self.busy_kills = self.busy_kills.saturating_add(1);
                    }
                    match reason {
                        KillReason::MemoryLimit => self.stats.memory_kills += 1,
                        KillReason::Deadline => self.stats.deadline_kills += 1,
                        KillReason::Crashed => self.stats.crashes += 1,
                        KillReason::Rejected(_) => self.stats.rejections += 1,
                    }
                    let worth_retrying = matches!(reason, KillReason::MemoryLimit | KillReason::Crashed);
                    if worth_retrying && !fresh && !retried && !is_heavy {
                        retried = true;
                        continue;
                    }
                    return Err(JobFailure::Stopped(reason));
                }
            }
        }
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
        // A heavy worker holds one page alone, so it goes before any other page whatever its size.
        self.retire_after_page = (footprint > self.policy.retire_bytes || worker.heavy || worker.surface_bytes_sent > self.policy.surface_bytes_cap).then_some(page);
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

    /// [`Self::bury_worker`], saying how much of the processor the worker had used in its life, from its final numbers.
    fn bury_worker_noting_cpu(&mut self) -> Option<u64> {
        self.retire_after_page = None;
        let worker = self.worker.take()?;
        worker.child.kill_now();
        worker.child.wait_bounded(FINAL_NUMBERS_WAIT);
        self.note_peak(&worker);
        worker.child.memory().map(|memory| memory.cpu_ns)
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

    /// Starts a worker of this class and opens the document in it, unless one that will do is already running. A normal job will do in a heavy worker, but not the other way round. A failure
    /// that trying again wouldn't change is remembered, so the next call doesn't start another process to learn the same thing. `generation` is the job this is for, so a long open can be cancelled.
    fn ensure_worker(&mut self, class: WorkerClass, generation: Option<u64>) -> Result<(), JobFailure> {
        if let Some(failure) = self.known_open_failure() {
            return Err(failure.into());
        }
        self.reopen_if_replaced();
        if let Some(failure) = self.known_open_failure() {
            return Err(failure.into());
        }
        self.reap_if_gone();
        let needs_heavy = class == WorkerClass::Heavy && !self.worker.as_ref().is_some_and(|worker| worker.heavy);
        if !needs_heavy && self.worker.is_some() {
            return Ok(());
        }
        // Taken before anything else: if another page has it, the normal worker stays, and the caller draws its lower-resolution version in it. An export has waited for it already.
        let ticket = if needs_heavy { Some(self.waited_ticket.take().or_else(|| self.config.heavy_slot.try_take()).ok_or(JobFailure::HeavyBusy)?) } else { None };
        if needs_heavy {
            self.bury_worker();
        }
        let (limit, idle_exit) = if ticket.is_some() { (self.policy.heavy_limit_bytes, self.policy.heavy_idle_exit) } else { (self.policy.memory_limit_bytes, self.policy.idle_exit) };
        let started = Worker::start(&self.config, &self.policy, limit, idle_exit);
        // No file has reached it yet, so whatever kept it from saying hello isn't the file's doing.
        if matches!(started, Err(StartFailure::TimedOut)) {
            self.stats.too_busy += 1;
        }
        let (mut worker, ready) = started.map_err(|failure| match failure {
            StartFailure::NoLibrary => RenderFailure::LibraryMissing,
            StartFailure::Walls(why) => RenderFailure::WallsDown(why),
            StartFailure::Refused => RenderFailure::StartRefused,
            StartFailure::TimedOut | StartFailure::Other => RenderFailure::Unavailable,
        })?;
        if worker.walled {
            self.stats.walled_spawns += 1;
        }
        if let Some(ticket) = ticket {
            self.stats.heavy_retries += 1;
            worker.heavy = true;
            worker.child.on_end(Box::new(move || drop(ticket)));
        }
        self.stats.spawns += 1;
        self.stats.ready_footprint_bytes = ready;
        self.stats.peak_footprint_bytes = self.stats.peak_footprint_bytes.max(ready);
        self.open_in(worker, generation)
            .inspect_err(|failure| {
                // `Unavailable` here means it never got to ask, which trying again may mend. What the worker itself said was noted where it was heard.
                if let JobFailure::Failed(failure) = failure
                    && !matches!(failure, RenderFailure::Unavailable)
                {
                    self.note_open_failure(*failure);
                }
            })
    }

    /// Remembers a failed open. "Not a PDF", a password, a worker the file stopped: those are about the file's bytes, and hold while it's the same file. `Unavailable` covers a failed read of its size and errors PDFium gives no reason for, so it holds only for a moment.
    fn note_open_failure(&mut self, failure: RenderFailure) {
        let try_again_at = matches!(failure, RenderFailure::Unavailable).then(|| Instant::now() + self.policy.open_retry_pause);
        self.open_failure = Some(KnownOpenFailure { failure, try_again_at });
    }

    /// **A file that's been replaced since it was opened is opened again, before a job goes to a worker that has the old one.** A worker reads through the file it was given, so after a save or another app's, without this it would go on drawing what was there. A look at the file's stamp, which is microseconds. What's been drawn till now says it's of another version: [`Self::take_file_changed`].
    fn reopen_if_replaced(&mut self) {
        // Gone from its path isn't replaced: the file that's open is still there to read, as it is for anyone who has it open.
        let Ok(now) = file_stamp(&self.path) else { return };
        if self.open_failure.is_some() || Some(now) == self.opened_stamp {
            return;
        }
        self.bury_worker();
        self.file_changed = true;
        self.forget_failures();
        self.traits.clear();
        self.image_places.clear();
        self.page_count = None;
        self.version = None;
        match File::open(&self.path) {
            Ok(file) => {
                self.file = file;
                self.opened_stamp = file_stamp(&self.path).ok();
            }
            Err(error) => {
                self.opened_stamp = None;
                self.note_open_failure(if error.kind() == io::ErrorKind::NotFound { RenderFailure::NotFound } else { RenderFailure::Unavailable });
            }
        }
    }

    /// What's known about opening this file, if that still holds. The app shouldn't have to remember to ask again: a file that's been replaced gets a fresh try, opened anew, and so does one whose moment has passed. Costs a `stat` only once something has failed, and nothing for a file that opens.
    fn known_open_failure(&mut self) -> Option<RenderFailure> {
        let known = self.open_failure?;
        // A file seen to change while it was open is what that failure says. The page numbers it went by are no good, and someone has to start again.
        if matches!(known.failure, RenderFailure::SourceChanged) {
            return Some(known.failure);
        }
        if file_stamp(&self.path).ok() != self.opened_stamp {
            self.file_changed = true;
            self.forget_failures();
            self.traits.clear();
            self.image_places.clear();
            self.page_count = None;
            self.version = None;
            return match File::open(&self.path) {
                Ok(file) => {
                    self.file = file;
                    self.opened_stamp = file_stamp(&self.path).ok();
                    None
                }
                // It's not there to open, or not now. That holds in the same way, and this renderer's old file isn't what anyone's asking about.
                Err(error) => {
                    self.opened_stamp = None;
                    let failure = if error.kind() == io::ErrorKind::NotFound { RenderFailure::NotFound } else { RenderFailure::Unavailable };
                    self.note_open_failure(failure);
                    Some(failure)
                }
            };
        }
        if known.try_again_at.is_some_and(|at| Instant::now() >= at) {
            self.open_failure = None;
            return None;
        }
        Some(known.failure)
    }

    fn open_in(&mut self, mut worker: Worker, generation: Option<u64>) -> Result<(), JobFailure> {
        let file = Handle::from_file(&self.file).map_err(|_| RenderFailure::Unavailable)?;
        let frame = proto::encode_open(self.password.as_deref().unwrap_or_default()).map_err(|_| RenderFailure::NeedsPassword)?;
        let watch = Watch { limit: worker.limit, policy: &self.policy, control: &self.control, generation };
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
                if matches!(reason, KillReason::Deadline) && worker.child.memory().is_some_and(|memory| was_starved(memory.cpu_ns, self.policy.page_deadline)) {
                    self.stats.too_busy += 1;
                }
                return Err(killed(&mut self.stats, reason).into());
            }
            // A stale job has no use for the document open. Nothing is wrong with it, so nothing is remembered.
            Err(Stop::StaleKilled) => {
                self.stats.cancel_kills += 1;
                return Err(JobFailure::StaleKilled);
            }
            Err(_) => return Err(RenderFailure::Unavailable.into()),
        };
        let rejected = |stats: &mut RenderStats, why: &'static str| killed(stats, KillReason::Rejected(why));
        let pages = match reply.kind {
            Kind::Opened => proto::decode_opened(&reply.payload).map_err(|error| JobFailure::from(rejected(&mut self.stats, error.0)))?,
            Kind::Failed => {
                let (answered, code) = proto::decode_failed(&reply.payload).map_err(|error| JobFailure::from(rejected(&mut self.stats, error.0)))?;
                if answered != 0 {
                    return Err(rejected(&mut self.stats, "a failure for a different job").into());
                }
                // Without this, a search of a file PDFium can't read starts a process per page to hear the same answer.
                let failure = failure_from_code(code);
                self.note_open_failure(failure);
                return Err(failure.into());
            }
            _ => return Err(rejected(&mut self.stats, "the wrong kind of reply to opening").into()),
        };
        let (pages, version) = pages;
        // The same file can't come back as another: the pages anyone has are of the one it was.
        if self.page_count.is_some_and(|known| known != pages) || self.version.is_some_and(|known| known != version) {
            self.file_changed = true;
            return Err(RenderFailure::SourceChanged.into());
        }
        self.page_count = Some(pages);
        self.version = Some(version);
        if let Some(memory) = worker.child.memory() {
            self.stats.opened_footprint_bytes = memory.footprint_bytes;
            self.stats.peak_footprint_bytes = self.stats.peak_footprint_bytes.max(memory.peak_footprint_bytes);
        }
        self.worker = Some(worker);
        Ok(())
    }
}

/// Internal: what can stop a job, for the sequence above it to decide on.
enum JobFailure {
    Failed(RenderFailure),
    /// A stale job took its worker with it.
    StaleKilled,
    /// Its worker was stopped, for this reason, with nothing left to retry at this level.
    Stopped(KillReason),
    /// It needs the heavy slot, and another page has it.
    HeavyBusy,
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
            // Each of these is handled by the caller that can cause it, and reaching here would be a slip, not a hostile file.
            JobFailure::StaleKilled | JobFailure::Stopped(_) | JobFailure::HeavyBusy => RenderFailure::Unavailable,
        }
    }
}

/// A tile job's result once the sequence has dealt with what it decides on.
fn other_result(result: Result<TileResult, JobFailure>) -> Result<TileResult, RenderFailure> {
    match result {
        Ok(tile) => Ok(tile),
        Err(JobFailure::StaleKilled) => Ok(TileResult::Cancelled { worker_killed: true }),
        Err(failure) => Err(failure.into()),
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
    limit: u64,
    policy: &'a Policy,
    control: &'a CancelWord,
    generation: Option<u64>,
}

impl Watch<'_> {
    fn is_stale(&self) -> bool {
        self.generation.is_some_and(|generation| self.control.is_stale(generation))
    }
}

/// Whether a worker that ran out of time had too little of the processor to have been the cause. A file that's slow to open uses what it's given. A worker that's hung looks starved too, and all that costs is its time being asked for again, a whole deadline apart.
fn was_starved(cpu_ns: u64, deadline: Duration) -> bool {
    u128::from(cpu_ns) < deadline.as_nanos() / 2
}

/// Why a worker didn't come up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartFailure {
    NoLibrary,
    TimedOut,
    Walls(WallsDown),
    /// The system refused to start it: `EPERM` or `EACCES` from the spawn, both `PermissionDenied`.
    Refused,
    Other,
}

impl Worker {
    /// Starts a worker, waits for its hello, and returns it with its footprint at that moment.
    fn start(config: &ServiceConfig, policy: &Policy, limit: u64, idle_exit: Duration) -> Result<(Self, u64), StartFailure> {
        let (ours, theirs) = Channel::pair().map_err(|_| StartFailure::Other)?;
        let default_args: Vec<OsString>;
        let args: &[OsString] = match &config.replace_args {
            Some(args) => args,
            None => {
                default_args = vec![
                    "--pdfium".into(),
                    config.pdfium_lib.clone().into_os_string(),
                    "--idle-exit-ms".into(),
                    idle_exit.as_millis().to_string().into(),
                    "--memory-limit".into(),
                    limit.to_string().into(),
                    "--deadline-ms".into(),
                    policy.page_deadline.as_millis().to_string().into(),
                ];
                &default_args
            }
        };
        let arg_refs: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
        // A stand-in worker gets no arguments of ours, so this is how it learns its limit. The real one has it in its arguments and an empty environment.
        let limit_text = limit.to_string();
        let mut env_refs: Vec<(&str, &str)> = config.extra_env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        if config.replace_args.is_some() {
            env_refs.push(("RECTO_WORKER_LIMIT", &limit_text));
        }
        let rule = IdleRule { after: idle_exit.saturating_add(policy.idle_grace), memory_limit_bytes: limit };
        // The one seam that says how a worker comes to exist. With no invitation it still runs: it just can't draw into surfaces.
        let invite = LinkInvite::new().ok();
        let child = ChildProcess::spawn_linked(&config.worker_exe, &arg_refs, &env_refs, theirs, Some(rule), invite.as_ref()).map_err(|error| if error.kind() == io::ErrorKind::PermissionDenied { StartFailure::Refused } else { StartFailure::Other })?;
        // Busy until the document is open, whose exchange ends that: a start isn't idleness, and `start_timeout` and the open's own watch decide how long it may take. Marked idle here, a start slower than the idle rule was killed half way.
        child.note_activity(true);
        ours.set_nonblocking(true).map_err(|_| StartFailure::Other)?;
        let mut worker = Self { child, channel: ours, reader: FrameReader::default(), surfaces: None, jobs_done: 0, surface_bytes_sent: 0, limit, heavy: false, walled: false };
        let started = Instant::now();
        let limit = policy.start_timeout;
        let hello = loop {
            match worker.reader.read(&worker.channel) {
                Ok(Some(frame)) => break frame,
                Ok(None) => {}
                Err(_) => {
                    worker.child.kill_now();
                    return Err(match worker.child.wait_bounded(EXIT_STATUS_WAIT) {
                        Some(ChildExit::Code(exit_code::NO_LIBRARY)) => StartFailure::NoLibrary,
                        Some(ChildExit::Code(exit_code::NO_WALL)) => StartFailure::Walls(WallsDown::NotSandboxed),
                        Some(ChildExit::Code(exit_code::NO_RIGHTS_TO_DROP)) => StartFailure::Walls(WallsDown::NothingToDrop),
                        Some(ChildExit::Code(exit_code::LAUNCHD_REACHABLE)) => StartFailure::Walls(WallsDown::LaunchdReachable),
                        // An app's sandbox can't start inside another: the system ends the helper before its first line. Measured: SIGTRAP under `sandbox-exec`.
                        _ if crate::bundle::walls_required(Some(&config.worker_exe), crate::bundle::RELEASE) && recto_platform::wall::in_a_sandbox() => StartFailure::Walls(WallsDown::InsideAnotherSandbox),
                        _ => StartFailure::Other,
                    });
                }
            }
            if started.elapsed() > limit {
                return Err(StartFailure::TimedOut);
            }
            worker.channel.wait_readable(Some(policy.watch_step)).map_err(|_| StartFailure::Other)?;
        };
        let said = (hello.kind == Kind::Hello && hello.handles.is_empty()).then(|| proto::decode_hello(&hello.payload).ok()).flatten().ok_or(StartFailure::Other)?;
        // A release, or a worker from a helper, has to have its walls up, and says so. One that doesn't is killed before it sees a document.
        if !said.walled && (crate::bundle::RELEASE || crate::bundle::walls_required(Some(&config.worker_exe), false)) {
            worker.child.kill_now();
            return Err(StartFailure::Walls(WallsDown::NotSandboxed));
        }
        worker.walled = said.walled;
        // A worker answers the invitation before it says hello, so by now its message is waiting, or isn't coming.
        worker.surfaces = invite.and_then(|invite| invite.accept(Duration::ZERO));
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
            if self.child.memory().is_some_and(|memory| memory.footprint_bytes > watch.limit) {
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
