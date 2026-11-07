//! The real worker, for signing with the App Sandbox entitlement, which also lets go of its Mach send rights once PDFium has loaded and before any hostile byte arrives. Everything else
//! is the worker as shipped. `build.rs` makes the body from `crates/recto-render-service/src/bin/worker.rs`.

include!(concat!(env!("OUT_DIR"), "/worker_body.rs"));

#[path = "../mach_drop.rs"]
mod mach_drop;

/// Why a worker refused to go on: the sandbox isn't on, or it held no rights to drop, so something isn't as this code understood it. A build signed wrong must not draw a hostile file.
const NO_WALL: i32 = 78;

fn spike_args() -> Vec<std::ffi::OsString> {
    std::env::args_os().skip(1).collect()
}

fn spike_channel() -> std::io::Result<Channel> {
    Channel::inherited()
}

fn spike_after_load() {
    // The test is the wall itself, not a variable that says so. `/Users` is listable by any plain process here and refused inside the sandbox.
    if std::fs::read_dir("/Users").is_ok() {
        std::process::exit(NO_WALL);
    }
    if mach_drop::drop_send_rights().0 == 0 {
        std::process::exit(NO_WALL);
    }
}

fn main() {
    worker_main();
}
