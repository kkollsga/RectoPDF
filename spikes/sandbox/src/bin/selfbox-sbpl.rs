//! The real worker, restricted by `sandbox_init` with a deny-by-default profile, once PDFium has loaded and before any hostile byte arrives. Everything else about the process is
//! the worker as shipped: same parent, same channel, same limits. `build.rs` makes the body from `crates/recto-render-service/src/bin/worker.rs`.
//!
//! `sandbox_init` is deprecated, and text as its profile is outside its documented contract ("flags must be SANDBOX_NAMED"). See the write-up before copying this.

include!(concat!(env!("OUT_DIR"), "/worker_body.rs"));

unsafe extern "C" {
    fn sandbox_init(profile: *const std::ffi::c_char, flags: u64, error: *mut *mut std::ffi::c_char) -> std::ffi::c_int;
}

/// Why a worker refused to go on. Never runs a hostile file unsandboxed.
const NO_SANDBOX: i32 = 78;

fn spike_args() -> Vec<std::ffi::OsString> {
    std::env::args_os().skip(1).collect()
}

fn spike_channel() -> std::io::Result<Channel> {
    Channel::inherited()
}

fn spike_after_load() {
    let profile = concat!(include_str!("../../profile.sb"), "\0");
    let mut error = std::ptr::null_mut();
    // SAFETY: the profile is NUL-terminated and `error` is live for the call.
    if unsafe { sandbox_init(profile.as_ptr().cast(), 0, &raw mut error) } != 0 {
        std::process::exit(NO_SANDBOX);
    }
}

fn main() {
    worker_main();
}
