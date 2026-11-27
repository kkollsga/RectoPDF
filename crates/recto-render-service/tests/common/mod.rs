//! Shared by this crate's test binaries.
#![allow(dead_code)]

use recto_render_service::{DocRenderer, HeavySlot, MAX_BUSY, Policy, RenderFailure, ServiceConfig, Surface};
use std::path::Path;
use std::time::{Duration, Instant};

/// The real worker. Failing here means `tools/pdfium.sh` hasn't run: a plain failure, because a skipped renderer test looks like a pass. `RECTO_TEST_WORKER` names another build of
/// it, such as the one in a built app's sandboxed helper, to run this whole suite against: `RECTO_TEST_WORKER=app/build.noindex/RectoPDF.app/Contents/Helpers/RectoRender.app/Contents/MacOS/recto-render-service`.
pub fn real_config() -> ServiceConfig {
    let worker = std::env::var_os("RECTO_TEST_WORKER").unwrap_or_else(|| env!("CARGO_BIN_EXE_recto-render-service").into());
    let mut config = ServiceConfig::for_worker(worker.into()).unwrap_or_else(|failure| panic!("{}. Run tools/pdfium.sh", failure.message()));
    // Each test's heavy worker is its own, so tests running side by side in one process can't change each other's results.
    config.heavy_slot = HeavySlot::new();
    config
}

/// A renderer of the file with the real worker. `DocRenderer::new` starts no worker: it opens the file and makes the control block's shared memory, and on a Mac loaded with other tests either can answer `Unavailable` once (2026-11-26/27, `tests/common/mod.rs` `.unwrap()` in a `like-ci` run, no reason kept). The product asks again at `open_retry_pause`, and so does this: at most [`MAX_BUSY`] tries (the one bound the service has for asking again), then it panics with the last answer, so a file that really can't be opened still fails the test.
pub fn renderer(path: &Path, policy: Policy) -> DocRenderer {
    opened_again(|| DocRenderer::new(path, None, real_config(), policy.clone()))
}

/// Asks `open` again while it says [`RenderFailure::Unavailable`] (a try that couldn't start, which remembers nothing), up to [`MAX_BUSY`] tries.
pub fn opened_again(mut open: impl FnMut() -> Result<DocRenderer, RenderFailure>) -> DocRenderer {
    let mut last = RenderFailure::Unavailable;
    for _ in 0..MAX_BUSY {
        match open() {
            Ok(doc) => return doc,
            Err(RenderFailure::Unavailable) => last = RenderFailure::Unavailable,
            Err(other) => panic!("the renderer wasn't opened: {other:?}"),
        }
    }
    panic!("the renderer wasn't opened in {MAX_BUSY} tries: {last:?}");
}

pub fn tile_surface() -> Surface {
    Surface::create(512 * 512 * 4).unwrap()
}

/// Waits for a state, not a time. The bound only turns a hang into a failure.
pub fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < Duration::from_secs(30), "gave up waiting for {what}");
        std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
    }
}

/// Kills another process the way an outside failure would. No `libc` here: this crate has no `unsafe` of its own.
pub fn kill_process(pid: u32) {
    let _ = std::process::Command::new("/bin/kill").arg("-9").arg(pid.to_string()).status();
}
