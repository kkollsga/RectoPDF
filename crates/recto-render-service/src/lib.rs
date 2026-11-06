//! The host's side of a renderer worker: starts it, gives it one document, watches it from outside, validates what it says, and kills it.
//! No window, no AppKit, no PDFium in this process.
//!
//! **Threads.** None. A call blocks its caller, which must not be the UI thread, and watches the worker in a poll loop while a job is in
//! flight. Nothing runs and nothing wakes between jobs. A cold tab has no [`DocRenderer`], so it costs nothing here.
//!
//! **Limits.** Enforced from this side, because the worker may be compromised, and macOS offers no OS limit to lean on: `setrlimit`
//! refuses `RLIMIT_AS`, `RLIMIT_DATA` and `RLIMIT_RSS` with `EINVAL` (measured, macOS 27).
//! - Hard memory limit: the worker's footprint is read every 5 ms during a job and it's killed when past the limit.
//! - Page deadline: past it, the worker is killed.
//! - Retirement: after a job, a footprint past 64 MiB ends the worker, because only exit returns the memory (spike §6).
//! - Cancel by generation: one shared atomic. The worker polls it between objects; if it hasn't returned within the grace, it's killed.
//! - Idle exit: the worker leaves 30 s after its last job, and the next job starts another.
//!
//! The worker also watches itself during a job, as a second line for when the host has died and nobody else is watching.

mod budget;
mod host;
pub mod transport;

pub use budget::{BudgetOptions, BudgetReport, PageSample, STRIP_MAX_BYTES, for_each_strip, measure_budget};
pub use host::{CancelHandle, DocRenderer, KillReason, PageCharacters, Policy, RenderFailure, RenderStats, ServiceConfig, Surface, Tile, TileRequest, TileResult};
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
}

/// Layout of the control page: one word the host writes and the worker reads, with no system call on either side.
pub mod control {
    /// A job whose generation is below this word is stale.
    pub const CANCEL_BEFORE_OFFSET: usize = 0;
    pub const LEN: usize = 4096;
}
