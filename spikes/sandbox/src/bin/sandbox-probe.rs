//! See `probe_core.rs`.

#[path = "../mach_drop.rs"]
mod mach_drop;
#[path = "../probe_core.rs"]
mod probe_core;

fn main() {
    probe_core::run(std::env::args().skip(1).collect());
}
