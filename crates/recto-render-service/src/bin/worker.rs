//! The renderer worker: one document, one thread drawing, killed from outside at any moment. Holds no policy beyond its own second-line
//! limits. Designed so XPC can replace the transport: everything crosses as frames and handles on the channel, argv names only limits and
//! the library, the document arrives as a descriptor, and the process has no file path to it.

use recto_platform::isolate::{self, Channel, Handle, SharedMemory, Wait};
use recto_platform::surface::SurfaceReceiver;
use recto_proto::{self as proto, FailCode, Kind, PageSize, RegionReply, RenderJob, TextJob, TextReply, TileReply, TileStatus};
use recto_render::{Cancel, Pdfium, PdfiumBackend, RenderBackend, TextOptions, TileOutcome};
use recto_render_service::{control, exit_code, transport};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use transport::{FrameReader, ReadError, send_all};

struct Args {
    pdfium: PathBuf,
    idle_exit: Duration,
    memory_limit: u64,
    deadline: Duration,
}

fn parse_args() -> Option<Args> {
    let mut args = std::env::args_os().skip(1);
    let (mut pdfium, mut idle_ms, mut memory, mut deadline_ms) = (None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next()?;
        match flag.to_str()? {
            "--pdfium" => pdfium = Some(PathBuf::from(value)),
            "--idle-exit-ms" => idle_ms = value.to_str()?.parse::<u64>().ok(),
            "--memory-limit" => memory = value.to_str()?.parse::<u64>().ok(),
            "--deadline-ms" => deadline_ms = value.to_str()?.parse::<u64>().ok(),
            _ => return None,
        }
    }
    Some(Args { pdfium: pdfium?, idle_exit: Duration::from_millis(idle_ms?), memory_limit: memory?, deadline: Duration::from_millis(deadline_ms?) })
}

fn main() {
    let Some(args) = parse_args() else { std::process::exit(exit_code::BAD_ARGUMENTS) };
    isolate::harden_this_process();
    let Ok(channel) = Channel::inherited() else { std::process::exit(exit_code::BAD_ARGUMENTS) };
    // Before the hello, so the host finds it waiting. The host may have sent no invitation: then there's no link, and shared memory does.
    let surfaces = SurfaceReceiver::from_invite();
    let Ok(pdfium) = Pdfium::load(&args.pdfium) else { std::process::exit(exit_code::NO_LIBRARY) };
    if send_all(&channel, &proto::encode_hello(std::process::id()), &[]).is_err() {
        std::process::exit(exit_code::ORPHANED);
    }
    let code = serve(channel, &pdfium, &args, surfaces.as_ref());
    // Skips destructors on purpose. Exit is the only thing that returns PDFium's memory, and tearing it down first only costs time.
    isolate::exit_now(code);
}

/// The word that says which generations are stale, or none yet.
struct Generation<'a> {
    word: Option<&'a AtomicU64>,
    job: u64,
}

impl Cancel for Generation<'_> {
    fn is_cancelled(&self) -> bool {
        self.word.is_some_and(|word| self.job < word.load(Ordering::Relaxed))
    }
}

fn serve(channel: Channel, pdfium: &Pdfium, args: &Args, surfaces: Option<&SurfaceReceiver>) -> i32 {
    let channel = Arc::new(channel);
    let mut watchdog = Watchdog::new(channel.clone(), args);
    let mut reader = FrameReader::default();
    let mut backend: Option<PdfiumBackend> = None;
    let mut control_page: Option<SharedMemory> = None;
    loop {
        // No job means no work and no wake-ups, until the host writes or the idle time is up.
        match channel.wait_readable(Some(args.idle_exit)) {
            Ok(Wait::Ready) => {}
            Ok(Wait::TimedOut) => return exit_code::IDLE,
            Err(_) => return exit_code::ORPHANED,
        }
        let frame = match reader.read_blocking(&channel) {
            Ok(frame) => frame,
            Err(ReadError::Closed) => return exit_code::IDLE,
            Err(ReadError::Bad(_) | ReadError::Io(_)) => return exit_code::PROTOCOL,
        };
        watchdog.arm();
        let reply = handle(frame, pdfium, &mut backend, &mut control_page, surfaces);
        watchdog.disarm();
        match reply {
            Some(bytes) => {
                if send_all(&channel, &bytes, &[]).is_err() {
                    return exit_code::ORPHANED;
                }
            }
            None => return exit_code::PROTOCOL,
        }
    }
}

/// Answers one frame. `None` means the host broke the protocol, and this process has no reason to go on.
fn handle(frame: transport::Frame, pdfium: &Pdfium, backend: &mut Option<PdfiumBackend>, control_page: &mut Option<SharedMemory>, surfaces: Option<&SurfaceReceiver>) -> Option<Vec<u8>> {
    let mut handles = frame.handles;
    match frame.kind {
        Kind::Open => {
            if handles.len() != 2 || backend.is_some() {
                return None;
            }
            let control_handle = handles.pop()?;
            let file = handles.pop()?.into_file();
            *control_page = Some(SharedMemory::map(control_handle, control::LEN).ok()?);
            let password = (!frame.payload.is_empty()).then_some(frame.payload.as_slice());
            Some(match PdfiumBackend::open(pdfium, file, password) {
                Ok(opened) => {
                    let pages = opened.page_count();
                    *backend = Some(opened);
                    proto::encode_opened(pages)
                }
                Err(code) => proto::encode_failed(0, code),
            })
        }
        Kind::Render => {
            let job = RenderJob::decode(&frame.payload).ok()?;
            let surface = handles.pop();
            Some(render(&job, surface, surfaces, backend, control_page.as_ref()))
        }
        Kind::SizeRequest => {
            let (job_id, page) = proto::decode_size_request(&frame.payload).ok()?;
            let Some(backend) = backend else { return Some(proto::encode_failed(job_id, FailCode::NoDocument)) };
            Some(match backend.page_size(page) {
                Ok(size) => PageSize { job_id, width: size.width, height: size.height }.encode(),
                Err(code) => proto::encode_failed(job_id, code),
            })
        }
        Kind::TraitsRequest => {
            let (job_id, page) = proto::decode_size_request(&frame.payload).ok()?;
            let Some(backend) = backend else { return Some(proto::encode_failed(job_id, FailCode::NoDocument)) };
            Some(match backend.largest_image_pixels(page) {
                Ok(largest_image_pixels) => proto::PageTraits { job_id, largest_image_pixels }.encode(),
                Err(code) => proto::encode_failed(job_id, code),
            })
        }
        Kind::ImagesRequest => {
            let (job_id, page) = proto::decode_size_request(&frame.payload).ok()?;
            let Some(backend) = backend else { return Some(proto::encode_failed(job_id, FailCode::NoDocument)) };
            Some(match backend.image_boxes(page) {
                Ok(boxes) => proto::PageImages { job_id, boxes }.encode(),
                Err(code) => proto::encode_failed(job_id, code),
            })
        }
        Kind::TextRequest => {
            let job = TextJob::decode(&frame.payload).ok()?;
            let Some(backend) = backend else { return Some(proto::encode_failed(job.job_id, FailCode::NoDocument)) };
            Some(match backend.page_text(job.page, TextOptions { boxes: job.boxes, hidden_check: job.hidden_check }) {
                Ok(page) => {
                    let reply = TextReply { job_id: job.job_id, page: job.page, hidden_checked: page.hidden_checked, chars: page.chars, boxes: page.boxes };
                    reply.encode().unwrap_or_else(|_| proto::encode_failed(job.job_id, FailCode::TooLarge))
                }
                Err(code) => proto::encode_failed(job.job_id, code),
            })
        }
        _ => None,
    }
}

/// How long a job's surface may lag behind its frame. The host sends it first, so this only ever runs out on a host that's broken.
const SURFACE_WAIT: Duration = Duration::from_secs(1);

fn render(job: &RenderJob, surface: Option<Handle>, surfaces: Option<&SurfaceReceiver>, backend: &mut Option<PdfiumBackend>, control_page: Option<&SharedMemory>) -> Vec<u8> {
    let started = Instant::now();
    let Some(backend) = backend else { return proto::encode_failed(job.job_id, FailCode::NoDocument) };
    let Ok(needed) = usize::try_from(job.bytes_needed()) else { return proto::encode_failed(job.job_id, FailCode::BadSurface) };
    let cancel = Generation { word: control_page.and_then(|page| page.atomic_u64(control::CANCEL_BEFORE_OFFSET)), job: job.generation };
    let mut kinds = Vec::new();
    // Either way a worker holds a surface only while drawing into it. Platform report, §2.7.
    let outcome = match (surface, surfaces) {
        (Some(handle), _) => {
            let Ok(mut mapping) = SharedMemory::map(handle, needed) else { return proto::encode_failed(job.job_id, FailCode::BadSurface) };
            draw(job, backend, mapping.as_mut_slice(), &cancel, &mut kinds)
        }
        (None, Some(link)) => {
            let Some(target) = link.next(SURFACE_WAIT) else { return proto::encode_failed(job.job_id, FailCode::BadSurface) };
            if target.len() < needed || target.stride() < job.stride {
                return proto::encode_failed(job.job_id, FailCode::BadSurface);
            }
            match target.with_pixels(|pixels| draw(job, backend, pixels, &cancel, &mut kinds)) {
                Ok(outcome) => outcome,
                Err(_) => return proto::encode_failed(job.job_id, FailCode::BadSurface),
            }
        }
        (None, None) => return proto::encode_failed(job.job_id, FailCode::BadSurface),
    };
    let status = match outcome {
        Ok(TileOutcome::Done(kind)) => TileStatus::Done(kind),
        Ok(TileOutcome::Cancelled) => TileStatus::Cancelled,
        Err(code) => return proto::encode_failed(job.job_id, code),
    };
    let tile = TileReply {
        generation: job.generation,
        job_id: job.job_id,
        width: job.rect.width,
        height: job.rect.height,
        stride: job.stride,
        status,
        render_us: u32::try_from(started.elapsed().as_micros()).unwrap_or(u32::MAX),
    };
    if job.tile_edge == 0 { tile.encode() } else { RegionReply { tile, kinds }.encode() }
}

fn draw(job: &RenderJob, backend: &mut PdfiumBackend, out: &mut [u8], cancel: &Generation<'_>, kinds: &mut Vec<recto_render::TileKind>) -> Result<TileOutcome, FailCode> {
    if job.tile_edge == 0 {
        backend.render_tile(job.page, job.rect, job.scale, out, job.stride, cancel)
    } else {
        backend.render_region(job.page, job.rect, job.scale, job.tile_edge, out, job.stride, cancel, kinds)
    }
}

/// The second line. The host enforces every limit from outside, but if the host dies during a hostile page nobody would stop it, so while
/// a job runs this thread checks the footprint, the deadline and whether the host is still there. Started with the first job and parked
/// between jobs, so an idle worker has no timer and no extra thread.
struct Watchdog {
    state: Arc<WatchState>,
    channel: Arc<Channel>,
    limit: u64,
    deadline: Duration,
    thread: Option<std::thread::Thread>,
}

struct WatchState {
    /// When the armed job began, in ms since `epoch`, plus one. Zero means no job.
    armed_at: AtomicU64,
    epoch: Instant,
}

/// Slack above the host's limit, so the host normally acts first and can say why.
const SELF_LIMIT_SLACK: u64 = 64 * 1024 * 1024;
const WATCH_STEP: Duration = Duration::from_millis(50);

impl Watchdog {
    fn new(channel: Arc<Channel>, args: &Args) -> Self {
        Self {
            state: Arc::new(WatchState { armed_at: AtomicU64::new(0), epoch: Instant::now() }),
            channel,
            limit: args.memory_limit.saturating_add(SELF_LIMIT_SLACK),
            // Twice the host's deadline: the host decides, as long as it's alive.
            deadline: args.deadline.saturating_mul(2),
            thread: None,
        }
    }

    fn arm(&mut self) {
        self.state.armed_at.store(self.state.epoch.elapsed().as_millis() as u64 + 1, Ordering::Release);
        if self.thread.is_none() {
            let (state, channel, limit, deadline) = (self.state.clone(), self.channel.clone(), self.limit, self.deadline);
            // Without this thread the host still holds every limit, so failing to start it isn't fatal.
            let spawned = std::thread::Builder::new().stack_size(64 * 1024).spawn(move || watch(&state, &channel, limit, deadline));
            self.thread = spawned.ok().map(|handle| handle.thread().clone());
        } else if let Some(thread) = &self.thread {
            thread.unpark();
        }
    }

    fn disarm(&self) {
        self.state.armed_at.store(0, Ordering::Release);
    }
}

fn watch(state: &WatchState, channel: &Channel, limit: u64, deadline: Duration) {
    let pid = std::process::id();
    loop {
        let armed_at = state.armed_at.load(Ordering::Acquire);
        if armed_at == 0 {
            std::thread::park();
            continue;
        }
        if isolate::process_memory(pid).is_some_and(|memory| memory.footprint_bytes > limit) {
            isolate::exit_now(exit_code::MEMORY);
        }
        if channel.peer_gone() {
            isolate::exit_now(exit_code::ORPHANED);
        }
        let now = state.epoch.elapsed().as_millis() as u64 + 1;
        if u128::from(now.saturating_sub(armed_at)) > deadline.as_millis() {
            isolate::exit_now(exit_code::DEADLINE);
        }
        std::thread::sleep(WATCH_STEP);
    }
}
