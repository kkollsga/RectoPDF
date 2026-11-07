//! Measures a worker over the same frames and the same process rules whichever way it was started: a plain child (`--plain <worker> <pdfium>`), or the XPC service in this
//! bundle (`--xpc <pdfium>`). The plain mode exists so a difference can't come from this harness.
//!
//!   instances                 Two connections: one process or two?
//!   cold <n>                  Start, hello, kill, n times.
//!   roundtrip <pdf> <pages>   Whole pages at 2x, one frame round trip each.
//!   bomb <pdf>                The limit from outside: 512 MiB, kill, a fresh worker at 1 GiB, kill.
//!   retire <n>                A worker leaves by itself when idle, then the next one starts: n times.
//!   after_long <seconds>      A worker runs that long and is killed, then the next one starts: does a long run escape launchd's delay?
//!   probe <args...>           The walls, from inside the service (XPC only).

#[path = "../xpc_ffi.rs"]
mod xpc_ffi;

use recto_platform::isolate::{self, Channel, ChildProcess, Handle, SharedMemory, Wait};
use recto_proto::{self as proto, Kind, PageSize, RenderJob, TileRect, TileReply};
use recto_render_service::control;
use recto_render_service::transport::{Frame, FrameReader, send_all};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use xpc_ffi::*;

const SERVICE: &CStr = c"no.rectospike.render";
const MIB: u64 = 1 << 20;

#[derive(Clone)]
enum How {
    Plain { worker: PathBuf, pdfium: PathBuf },
    Xpc { pdfium: PathBuf },
}

/// Microseconds since `EPOCH` when XPC last said the service went away.
static LAST_INTERRUPT_US: AtomicU64 = AtomicU64::new(0);
static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn now_us() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as u64
}

extern "C" fn on_event(_block: *const Block, event: XpcObject) {
    // SAFETY: only compares the event's type, which is valid for any object XPC delivers.
    if unsafe { xpc_get_type(event) } == (&raw const _xpc_type_error).cast() {
        LAST_INTERRUPT_US.store(now_us().max(1), Ordering::SeqCst);
    }
}

fn connect() -> XpcObject {
    // SAFETY: the name is NUL-terminated, a null queue is the documented default, and the block lives for ever.
    unsafe {
        let connection = xpc_connection_create(SERVICE.as_ptr(), std::ptr::null_mut());
        xpc_connection_set_event_handler(connection, global_block(on_event));
        xpc_connection_resume(connection);
        connection
    }
}

fn pid_from_ping(connection: XpcObject) -> i64 {
    // SAFETY: a new dictionary sent on a live connection, and the reply is read before release.
    unsafe {
        let message = xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0);
        xpc_dictionary_set_string(message, c"ping".as_ptr(), c"1".as_ptr());
        let reply = xpc_connection_send_message_with_reply_sync(connection, message);
        xpc_release(message);
        let pid = xpc_dictionary_get_int64(reply, c"pid".as_ptr());
        xpc_release(reply);
        pid
    }
}

/// A started worker, whichever way.
struct Remote {
    channel: Channel,
    reader: FrameReader,
    pid: libc::pid_t,
    child: Option<ChildProcess>,
    connection: Option<XpcObject>,
}

impl Remote {
    /// Starts one and waits for its hello. Returns it and how long that took.
    fn start(how: &How, limit: u64) -> (Self, Duration) {
        Self::start_idle(how, limit, 30_000)
    }

    fn start_idle(how: &How, limit: u64, idle_ms: u64) -> (Self, Duration) {
        let started = Instant::now();
        let (ours, theirs) = Channel::pair().unwrap();
        let args = |pdfium: &PathBuf| -> Vec<OsString> {
            vec!["--pdfium".into(), pdfium.clone().into(), "--idle-exit-ms".into(), idle_ms.to_string().into(), "--memory-limit".into(), limit.to_string().into(), "--deadline-ms".into(), "60000".into()]
        };
        let (child, connection, reply_pid) = match how {
            How::Plain { worker, pdfium } => {
                let args = args(pdfium);
                let refs: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
                (Some(ChildProcess::spawn(worker, &refs, &[], theirs, None).unwrap()), None, 0)
            }
            How::Xpc { pdfium } => {
                let connection = connect();
                let joined = args(pdfium).iter().map(|a| a.to_str().unwrap().to_string()).collect::<Vec<_>>().join("\u{1f}");
                let joined = CString::new(joined).unwrap();
                let fd = fd_from_channel(theirs);
                // SAFETY: a new dictionary on a live connection, with NUL-terminated keys and a descriptor XPC dups.
                let pid = unsafe {
                    let message = xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0);
                    xpc_dictionary_set_string(message, c"start".as_ptr(), joined.as_ptr());
                    xpc_dictionary_set_fd(message, c"fd".as_ptr(), fd.as_raw_fd());
                    let reply = xpc_connection_send_message_with_reply_sync(connection, message);
                    xpc_release(message);
                    assert_eq!(xpc_get_type(reply), (&raw const _xpc_type_dictionary).cast(), "the service couldn't be reached");
                    assert_eq!(xpc_dictionary_get_int64(reply, c"refused".as_ptr()), 0, "the service already has a worker");
                    let pid = xpc_dictionary_get_int64(reply, c"pid".as_ptr());
                    xpc_release(reply);
                    pid as libc::pid_t
                };
                // See `hold` and `unclamp` in the service. Sent if asked for, and with no reply expected.
                for key in [c"hold", c"unclamp"] {
                    if std::env::var_os(key.to_str().unwrap().to_uppercase()).is_some() {
                        // SAFETY: a new dictionary on a live connection, with NUL-terminated keys.
                        unsafe {
                            let message = xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0);
                            xpc_dictionary_set_string(message, key.as_ptr(), c"1".as_ptr());
                            xpc_connection_send_message(connection, message);
                            xpc_release(message);
                        }
                    }
                }
                (None, Some(connection), pid)
            }
        };
        let mut remote = Self { channel: ours, reader: FrameReader::default(), pid: reply_pid, child, connection };
        let hello = remote.read_frame(Duration::from_secs(30), None).expect("no hello");
        assert_eq!(hello.kind, Kind::Hello);
        remote.pid = proto::decode_hello(&hello.payload).unwrap() as libc::pid_t;
        (remote, started.elapsed())
    }

    /// Reads one frame, up to `limit`. With a memory limit, polls the footprint every 5 ms from outside, and kills on passing it, as the host does. Then returns `None`.
    fn read_frame(&mut self, limit: Duration, memory_limit: Option<u64>) -> Option<Frame> {
        let started = Instant::now();
        self.channel.set_nonblocking(true).unwrap();
        loop {
            if let Some(frame) = self.reader.read(&self.channel).ok()? {
                return Some(frame);
            }
            if started.elapsed() > limit {
                return None;
            }
            if let Some(cap) = memory_limit
                && isolate::process_memory(self.pid as u32).is_some_and(|memory| memory.footprint_bytes > cap)
            {
                self.kill();
                return None;
            }
            match self.channel.wait_readable(Some(Duration::from_millis(5))).unwrap() {
                Wait::Ready | Wait::TimedOut => {}
            }
        }
    }

    fn kill(&self) {
        match &self.child {
            Some(child) => {
                child.kill_now();
            }
            // SAFETY: no pointers. A pid from outside can have been recycled, which is one of this mechanism's costs: it isn't this process's child, so nothing holds the number.
            None => unsafe {
                libc::kill(self.pid, libc::SIGKILL);
            },
        }
    }

    fn send(&self, bytes: &[u8], handles: &[&Handle]) {
        self.channel.set_nonblocking(false).unwrap();
        send_all(&self.channel, bytes, handles).unwrap();
        self.channel.set_nonblocking(true).unwrap();
    }

    /// Opens a document on this worker. Returns its page count.
    fn open(&mut self, path: &str, control_page: &SharedMemory) -> u32 {
        let file = std::fs::File::open(path).unwrap();
        let file_handle = Handle::from_file(&file).unwrap();
        self.send(&proto::encode_open(b"").unwrap(), &[&file_handle, control_page.handle()]);
        let frame = self.read_frame(Duration::from_secs(30), None).expect("no answer to open");
        assert_eq!(frame.kind, Kind::Opened, "couldn't open");
        proto::decode_opened(&frame.payload).unwrap()
    }

    fn page_size(&mut self, page: u32) -> (f32, f32) {
        self.send(&proto::encode_size_request(1, page), &[]);
        let frame = self.read_frame(Duration::from_secs(30), None).expect("no size");
        let size = PageSize::decode(&frame.payload).unwrap();
        (size.width, size.height)
    }

    /// Draws one whole page at `scale`. `None` means it was killed for memory or timed out.
    fn draw(&mut self, page: u32, scale: f32, memory_limit: Option<u64>) -> Option<TileReply> {
        let (width, height) = self.page_size(page);
        let (w, h) = proto::device_size(width, height, scale).unwrap();
        let job = RenderJob { generation: 1, job_id: page + 1, page, scale, rect: TileRect { x: 0, y: 0, width: w, height: h }, stride: w * 4, tile_edge: 0 };
        let surface = SharedMemory::create(usize::try_from(job.bytes_needed()).unwrap()).unwrap();
        self.send(&job.encode(), &[surface.handle()]);
        let frame = self.read_frame(Duration::from_secs(60), memory_limit)?;
        assert_eq!(frame.kind, Kind::Tile);
        Some(TileReply::decode(&frame.payload).unwrap())
    }

    fn peak(&self) -> Option<u64> {
        match &self.child {
            Some(child) => child.memory().map(|memory| memory.peak_footprint_bytes),
            None => isolate::process_memory(self.pid as u32).map(|memory| memory.peak_footprint_bytes),
        }
    }

    /// Waits until the process has gone, by polling, as nothing here is its parent in XPC's case. Returns how long that took.
    fn wait_gone(&self) -> Duration {
        let started = Instant::now();
        loop {
            // SAFETY: signal 0 delivers nothing.
            let alive = match &self.child {
                Some(child) => child.try_wait().is_none(),
                None => unsafe { libc::kill(self.pid, 0) == 0 },
            };
            if !alive || started.elapsed() > Duration::from_secs(10) {
                return started.elapsed();
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.kill();
        if let Some(connection) = self.connection {
            // SAFETY: the connection is live, and this is its last use.
            unsafe { xpc_connection_cancel(connection) };
        }
    }
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() as f64 - 1.0) * p).round() as usize]
}

fn cold(how: &How, count: usize) {
    let mut hello = Vec::new();
    let mut gone_notice = Vec::new();
    let mut footprint = Vec::new();
    for _ in 0..count {
        LAST_INTERRUPT_US.store(0, Ordering::SeqCst);
        let (remote, took) = Remote::start(how, 512 * MIB);
        hello.push(took.as_secs_f64() * 1000.0);
        footprint.push(isolate::process_memory(remote.pid as u32).map_or(0.0, |memory| memory.footprint_bytes as f64 / MIB as f64));
        let killed_at = now_us();
        remote.kill();
        let gone = remote.wait_gone();
        gone_notice.push(gone.as_secs_f64() * 1000.0);
        if remote.child.is_none() {
            // XPC's own notice, and how late it comes.
            let deadline = Instant::now() + Duration::from_secs(2);
            while LAST_INTERRUPT_US.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_micros(200));
            }
            let at = LAST_INTERRUPT_US.load(Ordering::SeqCst);
            if at == 0 {
                println!("xpc_notice none within 2 s");
            } else {
                println!("xpc_notice_ms {:.2}", at.saturating_sub(killed_at) as f64 / 1000.0);
            }
        }
        drop(remote);
    }
    println!("cold_hello_ms first {:.2} p50 {:.2} p95 {:.2} n {}", hello[0], percentile(&mut hello.clone(), 0.5), percentile(&mut hello.clone(), 0.95), hello.len());
    println!("gone_after_kill_ms p50 {:.2} p95 {:.2}", percentile(&mut gone_notice, 0.5), percentile(&mut gone_notice, 0.95));
    println!("idle_footprint_mib p50 {:.2}", percentile(&mut footprint, 0.5));
}

fn roundtrip(how: &How, path: &str, pages: u32) {
    let control_page = SharedMemory::create(control::LEN).unwrap();
    let (mut remote, _) = Remote::start(how, 4096 * MIB);
    let total = remote.open(path, &control_page);
    let mut samples = Vec::new();
    for page in 0..pages.min(total) {
        let started = Instant::now();
        remote.draw(page, 2.0, None).expect("a page failed");
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    println!("roundtrip_ms p50 {:.2} p95 {:.2} n {}", percentile(&mut samples, 0.5), percentile(&mut samples, 0.95), samples.len());
    println!("peak_mib {:.1}", remote.peak().unwrap_or(0) as f64 / MIB as f64);
}

fn bomb(how: &How, path: &str) {
    let control_page = SharedMemory::create(control::LEN).unwrap();
    let all = Instant::now();
    for (step, limit) in [("512", 512 * MIB), ("1024", 1024 * MIB)] {
        let started = Instant::now();
        let (mut remote, _) = Remote::start(how, limit);
        remote.open(path, &control_page);
        let drew = remote.draw(0, 2.0, Some(limit));
        // Sampled at the moment of the kill, in case the record goes with the process.
        let before = remote.peak();
        let gone = remote.wait_gone();
        let after = remote.peak();
        println!(
            "bomb step {step} MiB: killed {} in {:.0} ms, gone {:.2} ms later, peak just after kill {:?} MiB, peak readable once gone {:?} MiB",
            drew.is_none(),
            started.elapsed().as_secs_f64() * 1000.0,
            gone.as_secs_f64() * 1000.0,
            before.map(|peak| peak / MIB),
            after.map(|peak| peak / MIB)
        );
    }
    println!("bomb_all_ms {:.0}", all.elapsed().as_secs_f64() * 1000.0);
}

/// How the next worker starts after one retired by leaving on its own, which is what retirement and idle exit do. No kill here.
fn retire(how: &How, count: usize) {
    let mut samples = Vec::new();
    for _ in 0..count {
        let (remote, took) = Remote::start_idle(how, 512 * MIB, 100);
        samples.push(took.as_secs_f64() * 1000.0);
        remote.wait_gone();
        drop(remote);
    }
    println!("after_clean_exit_hello_ms {samples:.1?}");
}

fn after_long(how: &How, seconds: u64) {
    let (remote, _) = Remote::start(how, 512 * MIB);
    std::thread::sleep(Duration::from_secs(seconds));
    remote.kill();
    remote.wait_gone();
    drop(remote);
    let (_next, took) = Remote::start(how, 512 * MIB);
    println!("hello_after_a_{seconds}s_run_ms {:.1}", took.as_secs_f64() * 1000.0);
}

fn instances() {
    let (a, b) = (connect(), connect());
    let (pid_a, pid_b) = (pid_from_ping(a), pid_from_ping(b));
    println!("instances: first connection pid {pid_a}, second connection pid {pid_b}, same process: {}", pid_a == pid_b);
    // SAFETY: both are live, and this is their last use.
    unsafe {
        println!("xpc_connection_get_pid: {} {}", xpc_connection_get_pid(a), xpc_connection_get_pid(b));
        xpc_connection_cancel(a);
        xpc_connection_cancel(b);
    }
}

fn probe(args: &[String]) {
    let connection = connect();
    let joined = CString::new(args.join("\u{1f}")).unwrap();
    // SAFETY: a new dictionary on a live connection, and the reply's string is copied before release.
    unsafe {
        let message = xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0);
        xpc_dictionary_set_string(message, c"probe".as_ptr(), joined.as_ptr());
        let reply = xpc_connection_send_message_with_reply_sync(connection, message);
        xpc_release(message);
        let text = xpc_dictionary_get_string(reply, c"report".as_ptr());
        if text.is_null() {
            println!("no report: the service couldn't be reached");
        } else {
            print!("{}", CStr::from_ptr(text).to_string_lossy());
        }
        println!("service pid {}", xpc_dictionary_get_int64(reply, c"pid".as_ptr()));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    now_us();
    let mut rest = args.as_slice();
    let how = match rest {
        [flag, worker, pdfium, tail @ ..] if flag == "--plain" => {
            rest = tail;
            How::Plain { worker: worker.into(), pdfium: pdfium.into() }
        }
        [flag, pdfium, tail @ ..] if flag == "--xpc" => {
            rest = tail;
            How::Xpc { pdfium: pdfium.into() }
        }
        _ => panic!("--plain <worker> <pdfium> | --xpc <pdfium>"),
    };
    match rest {
        [mode] if mode == "instances" => instances(),
        [mode, count] if mode == "cold" => cold(&how, count.parse().unwrap()),
        [mode, path, pages] if mode == "roundtrip" => roundtrip(&how, path, pages.parse().unwrap()),
        [mode, path] if mode == "bomb" => bomb(&how, path),
        [mode, seconds] if mode == "after_long" => after_long(&how, seconds.parse().unwrap()),
        [mode, count] if mode == "retire" => retire(&how, count.parse().unwrap()),
        [mode, tail @ ..] if mode == "probe" => probe(tail),
        _ => panic!("instances | cold <n> | roundtrip <pdf> <pages> | bomb <pdf> | probe <args...>"),
    }
}
