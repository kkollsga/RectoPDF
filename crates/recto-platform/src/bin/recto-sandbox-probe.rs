//! For tests only, never in the app: stands in for a compromised renderer. Signed into the renderer helper's own layout by `app/render-helper.sh`, in place of the worker, it puts up the
//! worker's own walls with the worker's own calls, then tries what a compromised renderer would try, and says what happened. One line per attempt: `name<TAB>allowed|denied<TAB>detail`.
//! It reports no file's contents, and removes what it writes. `tests/walls.rs` runs it, and runs it with no sandbox too, so a wall can't pass because the attempt was broken.
//!
//! Ported from the sandbox spike's probe (`spikes/sandbox/src/probe_core.rs`), keeping the attempts that bear on the shipped arrangement.

#[cfg(target_os = "macos")]
fn main() {
    mac::run(std::env::args().skip(1).collect());
}

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
mod mac {
    use recto_platform::wall;
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    unsafe extern "C" {
        static bootstrap_port: u32;
        static mach_task_self_: u32;
        fn bootstrap_look_up(bp: u32, name: *const c_char, port: *mut u32) -> c_int;
        fn mach_port_kernel_object(task: u32, name: u32, object_type: *mut u32, address: *mut u64) -> c_int;
        fn mach_port_kobject(task: u32, name: u32, object_type: *mut u32, address: *mut u64) -> c_int;
        fn mach_port_names(task: u32, names: *mut *mut u32, names_count: *mut u32, types: *mut *mut u32, types_count: *mut u32) -> c_int;
        fn mach_ports_lookup(task: u32, ports: *mut *mut u32, count: *mut u32) -> c_int;
        fn task_get_special_port(task: u32, which: c_int, port: *mut u32) -> c_int;
        fn thread_get_special_port(thread: u32, which: c_int, port: *mut u32) -> c_int;
        fn host_get_special_port(host: u32, node: c_int, which: c_int, port: *mut u32) -> c_int;
        fn mach_host_self() -> u32;
        fn mach_thread_self() -> u32;
        fn sandbox_check(pid: c_int, operation: *const c_char, filter: c_int, ...) -> c_int;
        fn task_get_exception_ports(task: u32, mask: u32, masks: *mut u32, count: *mut u32, handlers: *mut u32, behaviors: *mut i32, flavors: *mut i32) -> c_int;
        fn thread_get_exception_ports(thread: u32, mask: u32, masks: *mut u32, count: *mut u32, handlers: *mut u32, behaviors: *mut i32, flavors: *mut i32) -> c_int;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(allocator: *const c_void, buffer: *const u8, length: isize, is_directory: u8) -> *const c_void;
    }

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSOpenCFURLRef(url: *const c_void, launched: *mut *const c_void) -> c_int;
    }

    fn report(name: &str, allowed: bool, detail: impl std::fmt::Display) {
        println!("{name}\t{}\t{detail}", if allowed { "allowed" } else { "denied" });
        let _ = std::io::stdout().flush();
    }

    fn errno_text() -> String {
        std::io::Error::last_os_error().to_string()
    }

    fn read_file(label: &str, path: &str) {
        let name = format!("read:{label}");
        match std::fs::File::open(path) {
            Ok(mut file) => match file.read(&mut [0u8; 1]) {
                Ok(_) => report(&name, true, "opened and read"),
                Err(error) => report(&name, false, format!("opened, read failed: {error}")),
            },
            Err(error) => report(&name, false, error),
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

    fn inet(port: u16) -> libc::sockaddr_in {
        // SAFETY: all zeros is a valid `sockaddr_in`, and the fields that matter are set below.
        let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        address.sin_len = size_of::<libc::sockaddr_in>() as u8;
        address.sin_family = libc::AF_INET as u8;
        address.sin_port = port.to_be();
        address.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, 1]);
        address
    }

    /// A TCP connection to the test's listener, a UDP bind, and the system's DNS socket: each a way to the network.
    fn network(loopback_port: u16) {
        // SAFETY: plain socket calls on values made here. Every descriptor goes into an `OwnedFd`, and a failed one (-1) is never wrapped.
        unsafe {
            let tcp = libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0);
            if tcp >= 0 {
                let tcp = OwnedFd::from_raw_fd(tcp);
                let address = inet(loopback_port);
                let rc = libc::connect(tcp.as_raw_fd(), (&raw const address).cast(), size_of::<libc::sockaddr_in>() as u32);
                report("net:connect_loopback", rc == 0, if rc == 0 { "connected".to_string() } else { errno_text() });
            } else {
                report("net:connect_loopback", false, format!("no socket: {}", errno_text()));
            }
            let udp = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
            if udp >= 0 {
                let udp = OwnedFd::from_raw_fd(udp);
                let address = inet(0);
                let rc = libc::bind(udp.as_raw_fd(), (&raw const address).cast(), size_of::<libc::sockaddr_in>() as u32);
                report("net:bind_loopback", rc == 0, if rc == 0 { "bound".to_string() } else { errno_text() });
            } else {
                report("net:bind_loopback", false, format!("no socket: {}", errno_text()));
            }
            let unix = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
            if unix >= 0 {
                let unix = OwnedFd::from_raw_fd(unix);
                let mut address: libc::sockaddr_un = std::mem::zeroed();
                address.sun_family = libc::AF_UNIX as u8;
                let path = b"/var/run/mDNSResponder\0";
                std::ptr::copy_nonoverlapping(path.as_ptr().cast::<c_char>(), address.sun_path.as_mut_ptr(), path.len());
                let rc = libc::connect(unix.as_raw_fd(), (&raw const address).cast(), size_of::<libc::sockaddr_un>() as u32);
                report("net:connect_dns_socket", rc == 0, if rc == 0 { "connected".to_string() } else { errno_text() });
            } else {
                report("net:connect_dns_socket", false, format!("no socket: {}", errno_text()));
            }
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

    fn signal_parent() {
        // SAFETY: no pointers, and signal 0 delivers nothing.
        let rc = unsafe { libc::kill(libc::getppid(), 0) };
        report("signal:parent", rc == 0, if rc == 0 { "permitted".to_string() } else { errno_text() });
    }

    /// The worker's own walls, by the worker's own calls, in its order: the limits first, then the sandbox test and the Mach drop.
    fn walls() {
        recto_platform::isolate::harden_this_process();
        match wall::put_up() {
            Ok(dropped) => report("walls", true, format!("{} send rights dropped, {} left as dead names, {} kept", dropped.dropped, dropped.dead_names, dropped.kept)),
            Err(why) => report("walls", false, format!("{why:?}")),
        }
    }

    /// What the kernel says a right names, by both calls the product asks: `mach_port_kobject` and the older `mach_port_kernel_object`, each as `(rc, type)`. macOS versions differ in
    /// which answers and how: on macOS 15 CI, `mach_port_kernel_object` didn't give launchd's right the "unknown" type it gives on 27.
    fn described(task: u32, port: u32) -> ((c_int, u32), (c_int, u32)) {
        let (mut first, mut second, mut address) = (0u32, 0u32, 0u64);
        // SAFETY: all out-pointers are live, and these only read about a name this process holds.
        let rcs = unsafe { (mach_port_kobject(task, port, &raw mut first, &raw mut address), mach_port_kernel_object(task, port, &raw mut second, &raw mut address)) };
        ((rcs.0, first), (rcs.1, second))
    }

    /// Positively a kernel object by either call: a type that is neither none (0) nor unknown (all ones). Such a right reaches no process.
    fn is_kernel_object(((rc_a, type_a), (rc_b, type_b)): ((c_int, u32), (c_int, u32))) -> bool {
        let kernel = |rc: c_int, kind: u32| rc == 0 && kind != 0 && kind != u32::MAX;
        kernel(rc_a, type_a) || kernel(rc_b, type_b)
    }

    fn description_text(port: u32, ((rc_a, type_a), (rc_b, type_b)): ((c_int, u32), (c_int, u32))) -> String {
        format!("port {port:#x}, kobject rc {rc_a} type {type_a}, kernel_object rc {rc_b} type {type_b}")
    }

    /// Every send right this process holds, as the kernel describes it: what the product's drop decides from. One line each, `right:<name>`, allowed if it isn't positively a kernel
    /// object, so it's one the drop lets go of unless the kernel won't describe it.
    fn rights() {
        // SAFETY: reading a variable the runtime set before `main`.
        let task = unsafe { mach_task_self_ };
        let (mut names, mut types): (*mut u32, *mut u32) = (std::ptr::null_mut(), std::ptr::null_mut());
        let (mut name_count, mut type_count) = (0u32, 0u32);
        // SAFETY: all four out-pointers are live; the arrays are only read, and this process ends soon after.
        if unsafe { mach_port_names(task, &raw mut names, &raw mut name_count, &raw mut types, &raw mut type_count) } != 0 {
            report("rights", false, "mach_port_names refused");
            return;
        }
        // SAFETY: the kernel returned that many of each.
        let (names, types) = unsafe { (std::slice::from_raw_parts(names, name_count as usize), std::slice::from_raw_parts(types, type_count as usize)) };
        for (name, kind) in names.iter().zip(types) {
            if kind & (1 << 16) != 0 {
                let description = described(task, *name);
                report(&format!("right:{name:#x}"), !is_kernel_object(description), description_text(*name, description));
            }
        }
    }

    /// Every way this process knows of to ask the kernel for a right to something: what a compromised worker would try after the drop. `regain:<slot>` is allowed if the right isn't
    /// positively a kernel object. Whether it reaches launchd doesn't rest on that description, which differs between macOS versions: every special or registered right that isn't
    /// positively a kernel object gets a real bootstrap lookup through it, `regain_launchd:<slot>`. An exception port goes to a crash handler that may never answer, so it gets no
    /// lookup, only its description, and so do the host's slots, which name system daemons.
    fn regain() {
        // SAFETY: reading a variable the runtime set before `main`.
        let task = unsafe { mach_task_self_ };
        let look_through = |what: &str, port: u32| {
            let description = described(task, port);
            let kernel = is_kernel_object(description);
            report(&format!("regain:{what}"), !kernel, description_text(port, description));
            if kernel {
                report(&format!("regain_launchd:{what}"), false, "a kernel object, which reaches no process: not tried");
                return;
            }
            let mut service = 0u32;
            // SAFETY: the name is NUL-terminated and `service` is live. Asks for a port, and sends the service nothing.
            let rc = unsafe { bootstrap_look_up(port, c"com.apple.coreservices.launchservicesd".as_ptr(), &raw mut service) };
            report(&format!("regain_launchd:{what}"), rc == 0, format!("a bootstrap lookup through it returned {rc}"));
        };
        let describe = |what: String, port: u32| look_through(&what, port);
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
        // SAFETY: no preconditions.
        let host = unsafe { mach_host_self() };
        for which in 1..32 {
            let mut port = 0u32;
            // SAFETY: `port` is live. Reads only.
            // The host's slots name system daemons, which may never answer a stray lookup: described, not looked through.
            if unsafe { host_get_special_port(host, -1, which, &raw mut port) } == 0 && port != 0 {
                let description = described(task, port);
                report(&format!("regain:host_special_{which}"), !is_kernel_object(description), description_text(port, description));
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
        // SAFETY: no preconditions.
        let thread = unsafe { mach_thread_self() };
        for (what, target) in [("task", task), ("thread", thread)] {
            let (mut masks, mut count) = ([0u32; 32], 32u32);
            let (mut handlers, mut behaviors, mut flavors) = ([0u32; 32], [0i32; 32], [0i32; 32]);
            // SAFETY: the arrays hold the 32 entries `count` says, and the calls only read.
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
                        let description = described(task, *handler);
                        report(&format!("regain:{what}_exception_{i}"), !is_kernel_object(description), description_text(*handler, description));
                    }
                }
            }
        }
    }

    /// What this process can tell about its own sandbox: the kernel's answer, and the variables the App Sandbox sets.
    fn state() {
        // SAFETY: a null operation with no filter only asks whether there's a sandbox.
        let checked = unsafe { sandbox_check(libc::getpid(), std::ptr::null(), 0) };
        report("state:sandbox_check", checked == 1, checked);
        let container = std::env::var("APP_SANDBOX_CONTAINER_ID").unwrap_or_default();
        report("state:container_id", !container.is_empty(), container);
        report("state:home", true, std::env::var("HOME").unwrap_or_default());
        report("state:env_count", true, std::env::vars_os().count());
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
            // Said and flushed before each attempt, so a process the system kills shows which attempt it was killed in.
            println!("trying\t{}", flag.trim_start_matches('-'));
            let _ = std::io::stdout().flush();
            match flag.as_str() {
                "--walls" => walls(),
                "--state" => state(),
                "--rights" => rights(),
                "--harden" => recto_platform::isolate::harden_this_process(),
                "--regain" => regain(),
                "--read" => {
                    let (label, path) = split(value());
                    read_file(&label, &path);
                }
                "--list" => {
                    let (label, path) = split(value());
                    list_dir(&label, &path);
                }
                "--write" => {
                    let (label, path) = split(value());
                    write_file(&label, &path);
                }
                "--net" => network(value().parse().expect("a port")),
                "--spawn" => spawn(&value()),
                "--fork" => fork_attempt(),
                "--mach" => mach_lookups(&value()),
                "--ls-open" => ls_open(&value()),
                "--signal-parent" => signal_parent(),
                other => panic!("unknown flag {other}"),
            }
        }
    }
}
