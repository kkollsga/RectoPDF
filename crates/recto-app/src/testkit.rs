//! **Fakes of what the app core is given by its host,** for tests here and in other crates (the `testkit` feature): a clock that moves only when told, a listener that records
//! what it heard, and a page view's hooks that hear nothing. The headless app of step R1.3 is built from these.

use crate::clock::Clock;
use crate::listener::{Listener, QuitAnswer};
use std::sync::Mutex;
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

/// One call the host heard, by its name in [`Listener`], with what it carried.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Heard {
    ModelChanged,
    OpsReady,
    QaRequest { id: u64, verb: String, value: String },
    SessionEnded,
    CopyReady,
    RestoreReady,
    QuitAnswer(QuitAnswer),
    HandOffQuit,
}

/// What a check run inside each call is given: the call.
type Check = Box<dyn Fn(Heard) + Send + Sync>;

/// A listener that records every call in order, and can run a check inside each one, at the moment it's made: what the core holds then, for one, which [`Listener`]'s promises
/// are about. Recording takes a short lock of its own, after the check.
#[derive(Default)]
pub struct FakeListener {
    heard: Mutex<Vec<Heard>>,
    check: Option<Check>,
}

impl std::fmt::Debug for FakeListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeListener").field("heard", &self.heard()).finish_non_exhaustive()
    }
}

impl FakeListener {
    /// One that runs `check` inside every call, before recording it. A check that panics fails the test on the core's thread, where the call was made.
    #[must_use]
    pub fn checking(check: impl Fn(Heard) + Send + Sync + 'static) -> Self {
        Self {
            heard: Mutex::default(),
            check: Some(Box::new(check)),
        }
    }

    /// Every call so far, in order.
    #[must_use]
    pub fn heard(&self) -> Vec<Heard> {
        crate::lock(&self.heard).clone()
    }

    /// How many of one call.
    #[must_use]
    pub fn count(&self, what: &Heard) -> usize {
        crate::lock(&self.heard).iter().filter(|heard| *heard == what).count()
    }

    /// Every call so far, forgetting them, so a test can ask what one step alone made.
    pub fn take(&self) -> Vec<Heard> {
        std::mem::take(&mut *crate::lock(&self.heard))
    }

    fn hear(&self, what: Heard) {
        if let Some(check) = &self.check {
            check(what.clone());
        }
        crate::lock(&self.heard).push(what);
    }
}

impl Listener for FakeListener {
    fn model_changed(&self) {
        self.hear(Heard::ModelChanged);
    }

    fn ops_ready(&self) {
        self.hear(Heard::OpsReady);
    }

    fn qa_request(&self, id: u64, verb: String, value: String) {
        self.hear(Heard::QaRequest { id, verb, value });
    }

    fn session_ended(&self) {
        self.hear(Heard::SessionEnded);
    }

    fn copy_ready(&self) {
        self.hear(Heard::CopyReady);
    }

    fn restore_ready(&self) {
        self.hear(Heard::RestoreReady);
    }

    fn quit_answer(&self, answer: QuitAnswer) {
        self.hear(Heard::QuitAnswer(answer));
    }

    fn hand_off_quit(&self) {
        self.hear(Heard::HandOffQuit);
    }
}

/// A page view's hooks that hear nothing, for a page view made in a test.
#[derive(Clone, Copy, Debug, Default)]
pub struct QuietEngine;

impl recto_reader::engine::EngineEvents for QuietEngine {
    fn ops_ready(&self) {}
    fn note_changed(&self) {}
    fn page_moved(&self) {}
    fn page_drawn(&self, _tab: u32, _page: u32, _failure: Option<&'static str>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the fake listener loses a call, records them out of order, or `take` leaves them for the next step, so a test of what one step tells the host reads another's.
    #[test]
    fn the_fake_listener_records_in_order_and_takes() {
        let seen = std::sync::Arc::new(AtomicU64::new(0));
        let counted = std::sync::Arc::clone(&seen);
        let listener = FakeListener::checking(move |_| {
            counted.fetch_add(1, Ordering::SeqCst);
        });
        listener.model_changed();
        listener.restore_ready();
        listener.model_changed();
        listener.quit_answer(QuitAnswer::go());
        assert_eq!(listener.count(&Heard::ModelChanged), 2);
        assert_eq!(seen.load(Ordering::SeqCst), 4, "the check runs in each call");
        assert_eq!(
            listener.take(),
            vec![
                Heard::ModelChanged,
                Heard::RestoreReady,
                Heard::ModelChanged,
                Heard::QuitAnswer(QuitAnswer::go())
            ]
        );
        assert!(listener.heard().is_empty());
    }

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
