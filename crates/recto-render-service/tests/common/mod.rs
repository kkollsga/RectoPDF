//! Shared by this crate's test binaries.
#![allow(dead_code)]

use recto_render_service::{DocRenderer, Policy, ServiceConfig, Surface};
use std::path::Path;
use std::time::{Duration, Instant};

/// The real worker. Failing here means `tools/pdfium.sh` hasn't run: a plain failure, because a skipped renderer test looks like a pass.
pub fn real_config() -> ServiceConfig {
    ServiceConfig::for_worker(env!("CARGO_BIN_EXE_recto-render-service").into()).unwrap_or_else(|failure| panic!("{}. Run tools/pdfium.sh", failure.message()))
}

pub fn renderer(path: &Path, policy: Policy) -> DocRenderer {
    DocRenderer::new(path, None, real_config(), policy).unwrap()
}

pub fn tile_surface() -> Surface {
    Surface::create(512 * 512 * 4).unwrap()
}

/// Waits for a state, not a time. The bound only turns a hang into a failure.
pub fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < Duration::from_secs(30), "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Kills another process the way an outside failure would. No `libc` here: this crate has no `unsafe` of its own.
pub fn kill_process(pid: u32) {
    let _ = std::process::Command::new("/bin/kill").arg("-9").arg(pid.to_string()).status();
}
