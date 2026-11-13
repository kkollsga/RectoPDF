//! Faults a test makes the helper meet, read from the job line in a debug build only. In a release build there is no field to read and no code behind it: `apply` is an empty function there, so a job can't ask for any of this.

use serde_json::Value;

/// What a job asked a test build to go wrong, read once.
pub struct Faults {
    /// Starting a watcher thread fails, as it does under a low thread limit.
    pub spawn_fails: bool,
}

/// Debug builds: `test_stall_exit_ms` makes the C exit handlers slow, as a static destructor waiting on a lock does. `test_hold_mib` takes that much memory and keeps it. `test_spawn_fails` is `Faults::spawn_fails`.
#[cfg(debug_assertions)]
pub fn apply(job: &Value) -> Faults {
    if let Some(ms) = job["test_stall_exit_ms"].as_u64() {
        recto_platform::leave::stall_exit_handlers(ms);
    }
    if let Some(mib) = job["test_hold_mib"].as_u64() {
        // Written to, so it counts toward the footprint, and kept for the life of the process.
        Box::leak(vec![1u8; (mib as usize) << 20].into_boxed_slice());
    }
    Faults {
        spawn_fails: job["test_spawn_fails"].as_bool().unwrap_or(false),
    }
}

#[cfg(not(debug_assertions))]
pub fn apply(_job: &Value) -> Faults {
    Faults { spawn_fails: false }
}

/// Debug builds: `test_busy_ms` keeps the main thread spinning that long before it does the job, so a test can tell a helper that goes from one that finishes first.
#[cfg(debug_assertions)]
pub fn busy(job: &Value) {
    if let Some(ms) = job["test_busy_ms"].as_u64() {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < until {
            std::hint::spin_loop();
        }
    }
}

#[cfg(not(debug_assertions))]
pub fn busy(_job: &Value) {}
