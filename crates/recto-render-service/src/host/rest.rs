//! When a resting worker is let go (`Policy::heavy_rest`), in a module of its own with the numbers it was chosen from.

use std::time::Duration;

/// **When a worker is let go at rest,** where any other waits the 30 s of `Policy::idle_exit`: one past `Policy::retire_bytes` leaves once nothing has been done for `after`, and
/// one past `over_bytes` but not that, a page that a person reads on from, only after `gently_after`: a turn after a pause costs a new worker, an open and a page load, which on the
/// shaded NASA page (p175, resting at 54 MiB) is 265 ms against 51 with the worker kept (2026-11-16). `over_bytes` is not `retire_bytes`: that line decides what a worker may
/// hold *between pages while it is being used*, and a worker let go there costs a restart per page.
#[derive(Clone, Copy, Debug)]
pub struct HeavyRest {
    pub over_bytes: u64,
    pub after: Duration,
    pub gently_after: Duration,
}

impl Default for HeavyRest {
    fn default() -> Self {
        Self {
            // After 3 s: long enough to bridge a pause in scrolling, and a reload costs less than holding a gigabyte.
            // 2026-11-16, release app, quiet, what a worker rests at: 9 to 10 MiB after a text page (`rfc9110`), 15 after a colour page (NASA `ntrs20240002628` p50), 16 at most after
            // four sweeps of 6 pages a second through `rfc9110`, and 34 to 42 after the same through the NASA file (three runs); the shaded page (NASA `ntrs20220005969`
            // p175) rests at 53.6 to 54.0 (six runs) and the USGS map at 301. 48 MiB is above every ordinary rest and under the shaded page's. It was `retire_bytes` (64), above
            // that page's 54, so it was kept for the 30 s of `idle_exit`. `docs/engineering/app-memory.md`.
            over_bytes: 48 * 1024 * 1024,
            after: Duration::from_secs(3),
            // Between 48 and 64 MiB a worker is let go only after a pause that is not a reader's: 12 s, over a turn costing 265 ms against 51 (shaded p175 and p178, 5 runs each).
            gently_after: Duration::from_secs(12),
        }
    }
}
