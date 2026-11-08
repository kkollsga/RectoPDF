//! The host's side of a renderer worker: starts it, gives it one document, watches it from outside, validates what it says, and kills it.
//! No window, no AppKit, no PDFium in this process.
//!
//! **Threads.** A call blocks its caller, which must not be the UI thread, and watches the worker in a poll loop while a job is in flight. Between jobs
//! there's no polling: one reaper thread, in `recto-platform`, exists while any worker does and sleeps in `kevent`. It wakes when a worker ends, and at
//! most once per `idle_exit + idle_grace` per live worker. A cold tab has no [`DocRenderer`], so it costs nothing here: with no workers, no thread and
//! no handle.
//!
//! **Limits.** Enforced from this side, because the worker may be compromised, and macOS offers no OS limit to lean on: `setrlimit`
//! refuses `RLIMIT_AS`, `RLIMIT_DATA` and `RLIMIT_RSS` with `EINVAL` (measured, macOS 27).
//! - Hard memory limit: the worker's footprint is read every 5 ms during a job and it's killed when past the limit. A page that passes it goes down a sequence: that page
//!   alone at 1 GiB if the Mac has the memory and no other page has the one heavy slot, then a lower-resolution version reported as such, then a plain failure. Each step
//!   is remembered, so a hostile page ends quickly. See [`DocRenderer::render_tile`]. An export waits for the slot instead, and comes out sharp or not at all: see [`DrawPurpose`].
//! - Page deadline: past it, the worker is killed.
//! - Retirement: a footprint past 64 MiB after a job ends the worker before its next job for another page, because only exit returns the
//!   memory (spike §6).
//! - Cancel by generation: one shared atomic. The worker polls it between objects; if it hasn't returned within the grace, it's killed.
//! - Idle exit: the worker leaves 30 s after its last job, and the next job starts another. Past `idle_grace` more, the reaper kills one that didn't.
//!
//! The worker also watches itself during a job, as a second line for when the host has died and nobody else is watching.
//!
//! **What remains between jobs.** An honest worker sits in `poll` and can't grow. A compromised one could, and the host's check comes only at the idle
//! rule's one wake-up, so it could grow unwatched for up to `idle_exit + idle_grace`, 35 s by default. Closing that needs a timer per live worker or
//! the sandbox's memory limit. A timer would break the "no wake-ups when idle" budget, so it waits for the sandbox spike. An exited worker's channel
//! stays open in its `DocRenderer` until the next call on it: one descriptor, on a tab that's hot or warm anyway.

mod budget;
mod host;
pub mod transport;

pub use budget::{
    BUDGET_MAX_TIME, BudgetOptions, BudgetReport, DrawStrategy, OutputCaps, PageDrawn, PageEvent, PageSample, REGION_MIN_IMAGE_PIXELS, STRIP_MAX_BYTES, ViewportOptions, ViewportStep, box_pixels, draw_page_box, draw_whole_page, largest_scale_that_fits, measure_budget,
    measure_viewport, page_device_size, plan_regions,
};
pub use host::{CancelHandle, DocRenderer, DrawPurpose, ExportRefusal, HeavySlot, KillReason, LowRes, LowResWhy, PageCharacters, PageDrawState, Policy, RenderFailure, RenderStats, ServiceConfig, Surface, Target, Tile, TileRequest, TileResult, WallsDown};
pub use host::contents::{ContentsDone, ContentsEvent, ContentsRequest};
pub use recto_proto::{CharRecord, TileKind, TileRect, TILE_EDGE};

/// Exit codes the worker uses to say why it left. Anything else, or a signal, reads as a crash.
pub mod exit_code {
    /// Its 30 seconds were up. Not a failure.
    pub const IDLE: i32 = 0;
    /// Past its own memory limit. The host normally kills first; this is the second line.
    pub const MEMORY: i32 = 71;
    /// Past its own page deadline.
    pub const DEADLINE: i32 = 72;
    /// Its host has gone during a job.
    pub const ORPHANED: i32 = 73;
    /// PDFium wouldn't load.
    pub const NO_LIBRARY: i32 = 74;
    pub const BAD_ARGUMENTS: i32 = 75;
    pub const PROTOCOL: i32 = 76;
    /// It's in an app bundle, or a release, and the App Sandbox isn't on: the helper is signed wrong. It read no document.
    pub const NO_WALL: i32 = 77;
    /// Its walls couldn't go up: macOS gave it no Mach rights to drop. It read no document.
    pub const NO_RIGHTS_TO_DROP: i32 = 78;
    /// Its walls couldn't go up: launchd still answered after the drop. It read no document.
    pub const LAUNCHD_REACHABLE: i32 = 79;
}

/// Where the renderer lives in the app, and when it must be walled in. `docs/ENGINEERING.md`, "Renderer".
pub mod bundle {
    use std::path::{Path, PathBuf};

    /// Compiled as a release by `app/release-zip.sh`, which can't be changed afterwards. A release worker never runs without its walls, wherever it is, and a release host starts only
    /// the helper's worker and refuses one that says its walls aren't up.
    pub const RELEASE: bool = option_env!("RECTO_RELEASE").is_some();

    /// The helper app, in the main app's `Contents/Helpers`. Its own `.app`, because the App Sandbox takes its identity from a bundle's `Info.plist`.
    pub const HELPER_APP: &str = "RectoRender.app";
    /// The worker's file name, in the helper's `Contents/MacOS` and, in development, beside `recto`.
    pub const WORKER: &str = "recto-render-service";

    /// The helper's worker for a program in an app's `Contents/MacOS`: the app itself, `recto` and the other helpers there.
    pub fn helper_worker(macos_dir: &Path) -> Option<PathBuf> {
        Some(macos_dir.parent()?.join("Helpers").join(HELPER_APP).join("Contents/MacOS").join(format!("{WORKER}{}", std::env::consts::EXE_SUFFIX)))
    }

    /// Whether a worker at this path has to put its walls up before it reads a document: in any app bundle, or always in a release. Run from a bundle, it was put there to be sandboxed,
    /// so one that isn't was signed wrong. Development and tests run it bare from `target/`, where it isn't. Not knowing where it is counts as a bundle.
    pub fn walls_required(exe: Option<&Path>, release: bool) -> bool {
        if release {
            return true;
        }
        let Some(exe) = exe else { return true };
        let folders: Vec<_> = exe.ancestors().skip(1).take(3).map(|dir| dir.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()).collect();
        matches!(folders.as_slice(), [macos, contents, app] if macos == "MacOS" && contents == "Contents" && app.ends_with(".app"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Failure: a worker in the shipped bundle skips its wall check, so a helper signed without the sandbox draws hostile files. Or a bare development worker demands a sandbox it can't have.
        #[test]
        fn only_a_bundled_or_released_worker_needs_its_walls() {
            let bundled = Path::new("/Applications/RectoPDF.app/Contents/Helpers/RectoRender.app/Contents/MacOS/recto-render-service");
            assert!(walls_required(Some(bundled), false));
            assert!(walls_required(Some(Path::new("/tmp/Any.app/Contents/MacOS/recto-render-service")), false));
            assert!(!walls_required(Some(Path::new("/repo/target/debug/recto-render-service")), false));
            assert!(!walls_required(Some(Path::new("/repo/Contents/MacOS/recto-render-service")), false), "not in an app");
            assert!(!walls_required(Some(Path::new("/A.app/Contents/recto-render-service")), false), "not in an app's MacOS");
            assert!(walls_required(Some(Path::new("/repo/target/release/recto-render-service")), true), "a release always");
            assert!(walls_required(None, false), "not knowing where it is fails closed");
            assert_eq!(helper_worker(Path::new("/A.app/Contents/MacOS")).unwrap(), bundled_in("/A.app"));
        }

        fn bundled_in(app: &str) -> PathBuf {
            Path::new(app).join("Contents/Helpers/RectoRender.app/Contents/MacOS/recto-render-service")
        }
    }
}

/// Layout of the control page: one word the host writes and the worker reads, with no system call on either side.
pub mod control {
    /// A job whose generation is below this word is stale.
    pub const CANCEL_BEFORE_OFFSET: usize = 0;
    pub const LEN: usize = 4096;
}
