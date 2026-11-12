//! **Fakes of what the app core is given by its host,** for tests here and in other crates (the `testkit` feature): a clock that moves only when told, and, as the seams grow, a
//! listener that counts what it heard. The headless app of step R1.3 is built from these.

use crate::clock::Clock;
use std::sync::atomic::{AtomicU64, Ordering};

/// A clock that stands still until a test moves it.
#[derive(Debug, Default)]
pub struct FakeClock {
    now: AtomicU64,
}

impl FakeClock {
    #[must_use]
    pub fn at(ms: u64) -> Self {
        Self { now: AtomicU64::new(ms) }
    }

    /// Moves it on by `ms`, saturating.
    pub fn advance(&self, ms: u64) {
        let _ = self.now.try_update(Ordering::SeqCst, Ordering::SeqCst, |now| Some(now.saturating_add(ms)));
    }

    /// Sets it to `ms`. A test that sets it back is testing a clock that goes back, which the real one never does.
    pub fn set(&self, ms: u64) {
        self.now.store(ms, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    /// Failure: a crate enables `recto-app`'s `testkit` feature outside `[dev-dependencies]`, so the fakes (a clock that never moves, a listener that hears nothing) are built into the app or the CLI, where nothing should be able to reach them.
    #[test]
    fn only_tests_enable_the_testkit() {
        let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut looked = 0;
        for entry in std::fs::read_dir(&crates).expect("the crates folder") {
            let manifest = entry.expect("an entry").path().join("Cargo.toml");
            let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
            looked += 1;
            let mut section = String::new();
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('[') {
                    section = line.to_string();
                } else if line.starts_with("recto-app") && line.contains("testkit") {
                    assert!(section.ends_with("dev-dependencies]"), "{} enables testkit in {section}", manifest.display());
                }
            }
        }
        assert!(looked > 10, "read only {looked} manifests");
    }
}
