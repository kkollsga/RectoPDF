//! The real worker with no changes, built here so the spike's tests have it without building the main workspace. For comparing against: see `build.rs`.

include!(concat!(env!("OUT_DIR"), "/worker_body.rs"));

fn spike_args() -> Vec<std::ffi::OsString> {
    std::env::args_os().skip(1).collect()
}

fn spike_channel() -> std::io::Result<Channel> {
    Channel::inherited()
}

fn spike_after_load() {}

fn main() {
    worker_main();
}
