//! Shared by this crate's test binaries.
#![allow(dead_code)]

use recto_render_service::{DocRenderer, HeavySlot, Policy, ServiceConfig, Surface};
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

/// A renderer of the file with the real worker, opened in its two steps so a failure says which one it was. `DocRenderer::new` starts no worker: `open_file` opens the file and `with_file` makes the control block's shared memory, and both answer a bare `Unavailable` (the io error isn't in `RenderFailure`, which is `Copy`). It is not asked again: `Unavailable` isn't busy, and a person's file that can't be opened or a Mac that can't give shared memory is a fault to see. On a failed open the kit tries the file again itself to name the io error it gets now, which may not be the one the first open had.
pub fn renderer(path: &Path, policy: Policy) -> DocRenderer {
    let file = DocRenderer::open_file(path).unwrap_or_else(|failure| {
        panic!(
            "step 1, opening {}: {failure:?}; a plain open now says {:?}",
            path.display(),
            std::fs::File::open(path).map(|_| ())
        )
    });
    DocRenderer::with_file(file, path, None, real_config(), policy)
        .unwrap_or_else(|failure| panic!("step 2, the control block's shared memory for {}: {failure:?}", path.display()))
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
