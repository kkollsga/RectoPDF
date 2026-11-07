//! Stands in for a compromised renderer: from inside whatever sandbox it was started in, it tries what a compromised one would try and says what happened. One line per
//! attempt: `name<TAB>allowed|denied<TAB>detail`. It reports no file's contents, and everything it writes it removes. The `sandbox-probe` binary and the XPC service both run this.
//!
//! `tests/walls.rs` runs it with no sandbox as a control, so a wall can't pass just because the attempt was broken.
#![allow(dead_code)]

use std::ffi::{CString, c_char, c_int, c_void};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

unsafe extern "C" {
    static bootstrap_port: u32;
    fn bootstrap_look_up(bp: u32, name: *const c_char, port: *mut u32) -> c_int;
    fn sandbox_init(profile: *const c_char, flags: u64, error: *mut *mut c_char) -> c_int;
    fn task_set_special_port(task: u32, which: c_int, port: u32) -> c_int;
    static mach_task_self_: u32;
    fn mach_port_deallocate(task: u32, name: u32) -> c_int;
    fn mach_port_kernel_object(task: u32, name: u32, object_type: *mut u32, address: *mut u64) -> c_int;
    fn mach_ports_lookup(task: u32, ports: *mut *mut u32, count: *mut u32) -> c_int;
    fn task_get_special_port(task: u32, which: c_int, port: *mut u32) -> c_int;
    fn thread_get_special_port(thread: u32, which: c_int, port: *mut u32) -> c_int;
    fn host_get_special_port(host: u32, node: c_int, which: c_int, port: *mut u32) -> c_int;
    fn mach_host_self() -> u32;
    fn mach_thread_self() -> u32;
    fn task_get_exception_ports(task: u32, mask: u32, masks: *mut u32, count: *mut u32, handlers: *mut u32, behaviors: *mut i32, flavors: *mut i32) -> c_int;
    fn thread_get_exception_ports(thread: u32, mask: u32, masks: *mut u32, count: *mut u32, handlers: *mut u32, behaviors: *mut i32, flavors: *mut i32) -> c_int;
    fn host_get_exception_ports(host: u32, mask: u32, masks: *mut u32, count: *mut u32, handlers: *mut u32, behaviors: *mut i32, flavors: *mut i32) -> c_int;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: u32, matching: *mut c_void) -> u32;
    fn IOServiceOpen(service: u32, task: u32, kind: u32, connection: *mut u32) -> c_int;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(allocator: *const c_void, buffer: *const u8, length: isize, is_directory: u8) -> *const c_void;
    fn CFURLCreateWithBytes(allocator: *const c_void, bytes: *const u8, length: isize, encoding: u32, base: *const c_void) -> *const c_void;
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSOpenCFURLRef(url: *const c_void, launched: *mut *const c_void) -> c_int;
}

const SANDBOX_NAMED: u64 = 1;
const TASK_BOOTSTRAP_PORT: c_int = 4;

/// What's been reported, for a caller with no standard output.
static REPORT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn report(name: &str, allowed: bool, detail: impl std::fmt::Display) {
    let line = format!("{name}\t{}\t{detail}\n", if allowed { "allowed" } else { "denied" });
    print!("{line}");
    let _ = std::io::stdout().flush();
    REPORT.lock().unwrap().push_str(&line);
}

/// Everything reported so far.
pub fn take_report() -> String {
    std::mem::take(&mut *REPORT.lock().unwrap())
}

fn errno_text() -> String {
    std::io::Error::last_os_error().to_string()
}

fn read_file(label: &str, path: &str) {
    match std::fs::File::open(path) {
        Ok(mut file) => {
            let mut byte = [0u8; 1];
            match file.read(&mut byte) {
                Ok(_) => report(&format!("read:{label}"), true, "opened and read"),
                Err(error) => report(&format!("read:{label}"), false, format!("opened, read failed: {error}")),
            }
        }
        Err(error) => report(&format!("read:{label}"), false, error),
    }
}

fn stat_file(label: &str, path: &str) {
    match std::fs::metadata(path) {
        Ok(_) => report(&format!("stat:{label}"), true, "metadata read"),
        Err(error) => report(&format!("stat:{label}"), false, error),
    }
}

fn list_dir(label: &str, path: &str) {
    match std::fs::read_dir(path) {
        Ok(entries) => report(&format!("list:{label}"), true, format!("{} entries", entries.count())),
        Err(error) => report(&format!("list:{label}"), false, error),
    }
}

fn write_file(label: &str, path: &str) {
    let name = format!("write:{label}");
    match std::fs::OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(mut file) => {
            let wrote = file.write(b"probe");
            let _ = std::fs::remove_file(path);
            match wrote {
                Ok(count) if count > 0 => report(&name, true, "created and wrote"),
                Ok(_) => report(&name, false, "created, wrote nothing"),
                Err(error) => report(&name, false, format!("created, write failed: {error}")),
            }
        }
        Err(error) => report(&name, false, error),
    }
}

fn network(loopback_port: Option<u16>) {
    // SAFETY: plain socket calls on values made here. Every descriptor goes into an `OwnedFd`.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
        if fd < 0 {
            report("net:socket", false, errno_text());
            return;
        }
        let owned = OwnedFd::from_raw_fd(fd);
        report("net:socket", true, "a descriptor, which isn't yet a connection");
        if let Some(port) = loopback_port {
            let mut address: libc::sockaddr_in = std::mem::zeroed();
            address.sin_len = size_of::<libc::sockaddr_in>() as u8;
            address.sin_family = libc::AF_INET as u8;
            address.sin_port = port.to_be();
            address.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
            let rc = libc::connect(owned.as_raw_fd(), (&raw const address).cast(), size_of::<libc::sockaddr_in>() as u32);
            report("net:connect_loopback", rc == 0, if rc == 0 { "connected".to_string() } else { errno_text() });
        }
        let udp = OwnedFd::from_raw_fd(libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0));
        let mut address: libc::sockaddr_in = std::mem::zeroed();
        address.sin_len = size_of::<libc::sockaddr_in>() as u8;
        address.sin_family = libc::AF_INET as u8;
        address.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
        let rc = libc::bind(udp.as_raw_fd(), (&raw const address).cast(), size_of::<libc::sockaddr_in>() as u32);
        report("net:bind_loopback", rc == 0, if rc == 0 { "bound".to_string() } else { errno_text() });
        let unix = OwnedFd::from_raw_fd(libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0));
        let mut address: libc::sockaddr_un = std::mem::zeroed();
        address.sun_family = libc::AF_UNIX as u8;
        let path = b"/var/run/mDNSResponder\0";
        std::ptr::copy_nonoverlapping(path.as_ptr().cast::<c_char>(), address.sun_path.as_mut_ptr(), path.len());
        let rc = libc::connect(unix.as_raw_fd(), (&raw const address).cast(), size_of::<libc::sockaddr_un>() as u32);
        report("net:connect_dns_socket", rc == 0, if rc == 0 { "connected".to_string() } else { errno_text() });
    }
}

fn spawn(path: &str) {
    let c_path = CString::new(path).unwrap();
    let argv = [c_path.as_ptr().cast_mut(), std::ptr::null_mut()];
    let envp = [std::ptr::null_mut::<c_char>()];
    let mut pid = 0;
    // SAFETY: the path and arrays are NUL-terminated and outlive the call.
    let rc = unsafe { libc::posix_spawn(&raw mut pid, c_path.as_ptr(), std::ptr::null(), std::ptr::null(), argv.as_ptr(), envp.as_ptr()) };
    if rc == 0 {
        let mut status = 0;
        // SAFETY: `pid` is this process's new child and `status` is live.
        unsafe { libc::waitpid(pid, &raw mut status, 0) };
        report("spawn", true, format!("{path} ran, status {status}"));
    } else {
        report("spawn", false, format!("{path}: {}", std::io::Error::from_raw_os_error(rc)));
    }
}

fn fork_attempt() {
    // SAFETY: the child does nothing but `_exit`.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: no preconditions.
        unsafe { libc::_exit(0) };
    }
    if pid < 0 {
        report("fork", false, errno_text());
    } else {
        let mut status = 0;
        // SAFETY: `pid` is this process's new child and `status` is live.
        unsafe { libc::waitpid(pid, &raw mut status, 0) };
        report("fork", true, "a child ran");
    }
}

fn mach_lookups(names: &str) {
    for name in names.split(',').filter(|name| !name.is_empty()) {
        let c_name = CString::new(name).unwrap();
        let mut port = 0u32;
        // SAFETY: the name is NUL-terminated and `port` is live. Only looks the service up, and sends it nothing.
        let rc = unsafe { bootstrap_look_up(bootstrap_port, c_name.as_ptr(), &raw mut port) };
        // 1100 is BOOTSTRAP_NOT_PRIVILEGED, what a sandbox denial returns. 1102 is BOOTSTRAP_UNKNOWN_SERVICE: reached launchd, nobody by that name.
        report(&format!("mach:{name}"), rc == 0, format!("bootstrap_look_up returned {rc}"));
    }
}

fn ls_open(path: &str) {
    // SAFETY: the buffer and length describe `path`. The URL is leaked, as this process is about to end.
    let status = unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(std::ptr::null(), path.as_ptr(), path.len() as isize, 1);
        LSOpenCFURLRef(url, std::ptr::null_mut())
    };
    report("launchservices:open", status == 0, format!("LSOpenCFURLRef returned {status}"));
}

/// The same request by URL, which is how `https://` reaches a browser. Needs no read access to any file.
fn ls_open_url(url: &str) {
    // SAFETY: the buffer and length describe `url`. The URL is leaked, as this process is about to end.
    let status = unsafe {
        let url = CFURLCreateWithBytes(std::ptr::null(), url.as_ptr(), url.len() as isize, 0x0800_0100, std::ptr::null());
        LSOpenCFURLRef(url, std::ptr::null_mut())
    };
    report("launchservices:open_url", status == 0, format!("LSOpenCFURLRef returned {status}"));
}

/// What `kill` says about another process. Signal 0 checks permission and delivers nothing.
fn signal_parent() {
    // SAFETY: no pointers, and signal 0 delivers nothing.
    let rc = unsafe { libc::kill(libc::getppid(), 0) };
    report("signal:parent", rc == 0, if rc == 0 { "permitted".to_string() } else { errno_text() });
}

fn self_memory() {
    match recto_platform::isolate::process_memory(std::process::id()) {
        Some(memory) => report("self_footprint", true, format!("{} MiB", memory.footprint_bytes >> 20)),
        None => report("self_footprint", false, "unreadable, so the worker's own watchdog would never fire"),
    }
}

fn self_sandbox(spec: &str) {
    let (profile, flags) = match spec.split_once(':') {
        Some(("named", name)) => (name.to_string(), SANDBOX_NAMED),
        Some(("sbpl", file)) => (std::fs::read_to_string(file).expect("the profile file"), 0),
        _ => panic!("named:<name> or sbpl:<file>"),
    };
    let c_profile = CString::new(profile).unwrap();
    let mut error = std::ptr::null_mut();
    // SAFETY: the profile is NUL-terminated and `error` is live. The API is deprecated; this is the spike finding out what it still does.
    let rc = unsafe { sandbox_init(c_profile.as_ptr(), flags, &raw mut error) };
    let text = if error.is_null() {
        String::new()
    } else {
        // SAFETY: the call set it to a NUL-terminated string.
        unsafe { std::ffi::CStr::from_ptr(error) }.to_string_lossy().replace('\n', " ")
    };
    report("sandbox_init", rc == 0, format!("rc {rc} {text}"));
}

/// Throws away this task's send right to launchd, which is what every Mach service lookup starts from.
fn drop_bootstrap() {
    // SAFETY: only this process's own ports, with null as the new value.
    let (a, b) = unsafe {
        let a = task_set_special_port(mach_task_self_, TASK_BOOTSTRAP_PORT, 0);
        let b = mach_port_deallocate(mach_task_self_, bootstrap_port);
        (a, b)
    };
    report("drop_bootstrap", a == 0 && b == 0, format!("task_set_special_port {a}, mach_port_deallocate {b}"));
}

fn drop_mach() {
    let (dropped, kept) = crate::mach_drop::drop_send_rights();
    report("drop_mach", dropped > 0, format!("{dropped} send rights dropped, {kept} kernel objects kept"));
}

/// Every way this process knows of to ask the kernel for a right to something, after dropping them all: what it would try next. A right that comes back as a user port is a way out.
fn regain() {
    let task = unsafe { mach_task_self_ };
    let describe = |what: String, port: u32| {
        let (mut object_type, mut address) = (0u32, 0u64);
        // SAFETY: both out-pointers are live.
        let rc = unsafe { mach_port_kernel_object(task, port, &raw mut object_type, &raw mut address) };
        let user_port = rc == 0 && object_type == u32::MAX;
        // The real test of a right that came back: is it launchd? A bootstrap lookup through it succeeds only if so.
        let mut service = 0u32;
        // SAFETY: the name is NUL-terminated and `service` is live. Asks for a port, and sends the service nothing.
        let looked_up = user_port && unsafe { bootstrap_look_up(port, c"com.apple.coreservices.launchservicesd".as_ptr(), &raw mut service) } == 0;
        report(&format!("regain:{what}"), user_port, format!("port {port:#x}, kernel object type {object_type} (rc {rc})"));
        if user_port {
            report(&format!("regain_launchd:{what}"), looked_up, "a bootstrap lookup through it");
        }
    };
    for which in 0..16 {
        let mut port = 0u32;
        // SAFETY: `port` is live, and this only reads this task's own special port.
        if unsafe { task_get_special_port(task, which, &raw mut port) } == 0 && port != 0 {
            describe(format!("task_special_{which}"), port);
        }
    }
    for which in 0..16 {
        let mut port = 0u32;
        // SAFETY: `port` is live, and this only reads this thread's own special port.
        if unsafe { thread_get_special_port(mach_thread_self(), which, &raw mut port) } == 0 && port != 0 {
            describe(format!("thread_special_{which}"), port);
        }
    }
    let host = unsafe { mach_host_self() };
    for which in 1..32 {
        let mut port = 0u32;
        // SAFETY: `port` is live. Reads only.
        if unsafe { host_get_special_port(host, -1, which, &raw mut port) } == 0 && port != 0 {
            describe(format!("host_special_{which}"), port);
        }
    }
    let mut ports: *mut u32 = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: both out-pointers are live.
    if unsafe { mach_ports_lookup(task, &raw mut ports, &raw mut count) } == 0 {
        for i in 0..count as usize {
            // SAFETY: the kernel returned `count` of them.
            let port = unsafe { *ports.add(i) };
            if port != 0 {
                describe(format!("registered_{i}"), port);
            }
        }
    }
    let exception_getters: [(&str, u32); 2] = [("task", task), ("thread", unsafe { mach_thread_self() })];
    for (what, target) in exception_getters {
        let (mut masks, mut count) = ([0u32; 32], 32u32);
        let (mut handlers, mut behaviors, mut flavors) = ([0u32; 32], [0i32; 32], [0i32; 32]);
        // SAFETY: the arrays match `count`, and the call only reads.
        let rc = unsafe {
            if what == "task" {
                task_get_exception_ports(target, 0x3ffe, masks.as_mut_ptr(), &raw mut count, handlers.as_mut_ptr(), behaviors.as_mut_ptr(), flavors.as_mut_ptr())
            } else {
                thread_get_exception_ports(target, 0x3ffe, masks.as_mut_ptr(), &raw mut count, handlers.as_mut_ptr(), behaviors.as_mut_ptr(), flavors.as_mut_ptr())
            }
        };
        if rc == 0 {
            for (i, handler) in handlers.iter().take(count as usize).enumerate() {
                if *handler != 0 {
                    describe(format!("{what}_exception_{i}"), *handler);
                }
            }
        }
    }
    let (mut masks, mut count) = ([0u32; 32], 32u32);
    let (mut handlers, mut behaviors, mut flavors) = ([0u32; 32], [0i32; 32], [0i32; 32]);
    // SAFETY: the arrays match `count`, and the call only reads.
    let rc = unsafe { host_get_exception_ports(host, 0x3ffe, masks.as_mut_ptr(), &raw mut count, handlers.as_mut_ptr(), behaviors.as_mut_ptr(), flavors.as_mut_ptr()) };
    report("regain:host_exception_call", rc == 0, format!("returned {rc}"));
    if rc == 0 {
        for (i, handler) in handlers.iter().take(count as usize).enumerate() {
            if *handler != 0 {
                describe(format!("host_exception_{i}"), *handler);
            }
        }
    }
}

/// A kernel driver's front door, which a sandbox can leave open. Opening is all this does.
fn iokit(name: &str) {
    let c_name = CString::new(name).unwrap();
    // SAFETY: the name is NUL-terminated, the matching dictionary is consumed by the call, and `connection` is live.
    unsafe {
        let service = IOServiceGetMatchingService(0, IOServiceMatching(c_name.as_ptr()));
        if service == 0 {
            report(&format!("iokit:{name}"), false, "no such service, or matching refused");
            return;
        }
        let mut connection = 0u32;
        let rc = IOServiceOpen(service, mach_task_self_, 0, &raw mut connection);
        report(&format!("iokit:{name}"), rc == 0, format!("IOServiceOpen returned {rc:#x}"));
    }
}

/// Runs the attempts the arguments name, in order.
pub fn run(arguments: Vec<String>) {
    // A write past `RLIMIT_FSIZE` should show as a failed write here, not end the probe.
    // SAFETY: setting a signal to ignored has no preconditions.
    unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
    let mut args = arguments.into_iter();
    while let Some(flag) = args.next() {
        let mut value = || args.next().expect("a value");
        let split = |text: String| -> (String, String) {
            let (label, path) = text.split_once('=').expect("label=path");
            (label.to_string(), path.to_string())
        };
        match flag.as_str() {
            "--harden" => recto_platform::isolate::harden_this_process(),
            "--self" => self_sandbox(&value()),
            "--drop-bootstrap" => drop_bootstrap(),
            "--drop-mach" => drop_mach(),
            "--regain" => regain(),
            "--iokit" => iokit(&value()),
            "--read" => {
                let (label, path) = split(value());
                read_file(&label, &path);
            }
            "--stat" => {
                let (label, path) = split(value());
                stat_file(&label, &path);
            }
            "--list" => {
                let (label, path) = split(value());
                list_dir(&label, &path);
            }
            "--write" => {
                let (label, path) = split(value());
                write_file(&label, &path);
            }
            "--net" => network(value().parse().ok()),
            "--spawn" => spawn(&value()),
            "--fork" => fork_attempt(),
            "--mach" => mach_lookups(&value()),
            "--ls-open" => ls_open(&value()),
            "--ls-open-url" => ls_open_url(&value()),
            "--signal-parent" => signal_parent(),
            "--self-memory" => self_memory(),
            "--pdfium" => {
                let path = value();
                match recto_platform::isolate::Library::open(std::path::Path::new(&path)) {
                    Ok(_) => report("dlopen:pdfium", true, "loaded, initialisers ran"),
                    Err(_) => {
                        // SAFETY: `dlerror` returns a NUL-terminated string or null.
                        let why = unsafe {
                            let text = libc::dlerror();
                            if text.is_null() { String::new() } else { std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned() }
                        };
                        report("dlopen:pdfium", false, why);
                    }
                }
            }
            // Last, as it replaces this process. A sandbox stays on across exec, and `RLIMIT_NPROC` doesn't count it, so this has its own line.
            "--exec" => {
                let parts: Vec<CString> = value().split(',').map(|part| CString::new(part).unwrap()).collect();
                report("exec:attempting", true, "replacing this process");
                let mut argv: Vec<*const c_char> = parts.iter().map(|part| part.as_ptr()).collect();
                argv.push(std::ptr::null());
                // SAFETY: the path and array are NUL-terminated and outlive the call, which doesn't return if it works.
                // The binary named should prove it ran by itself, as this code doesn't get to report that.
                unsafe { libc::execv(parts[0].as_ptr(), argv.as_ptr()) };
                report("exec", false, errno_text());
            }
            other => panic!("unknown flag {other}"),
        }
    }
}
