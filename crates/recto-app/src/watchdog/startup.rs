//! A fixed first-launch trace on the UI watchdog's clock. Host checkpoints never wait.
//!
//! Only the first launch-begin starts it; host-ready freezes it even if the point cannot
//! be kept. Reopening a window cannot reset that launch. No document data is retained.

use super::{WINDOW_OF, WINDOW_WATCH, Watchdog};
use recto_platform::thread::own_cpu_us;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError, TryLockError};

pub const CAP: usize = 16;
const NO_CPU: u64 = u64::MAX;
const NOT_BEGUN: u8 = 0;
const CAPTURING: u8 = 1;
// New points stop here; completion is published only after the final point or its loss.
const FINISHING: u8 = 2;
const COMPLETE: u8 = 3;
const NAMES: [&str; 15] = [
    "launch_begin",
    "menu_ready",
    "services_ready",
    "restore_queued",
    "restore_begin",
    "appearance_ready",
    "window_begin",
    "window_body",
    "native_window_ready",
    "page_ready",
    "sidebar_ready",
    "window_layout_ready",
    "window_constructed",
    "window_opened",
    "host_ready",
];

/// Validated once at the host boundary; no host-provided words are retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Phase(u8);

impl Phase {
    const BEGIN: Self = Self(0);
    const READY: Self = Self(14);

    fn decode(id: u8) -> Option<Self> {
        (usize::from(id) < NAMES.len()).then_some(Self(id))
    }

    fn name(self) -> &'static str {
        NAMES[usize::from(self.0)]
    }
}

#[derive(Clone, Copy)]
struct Point {
    phase: Phase,
    at_us: u64,
    cpu_us: u64,
}

impl Point {
    const EMPTY: Self = Self {
        phase: Phase::BEGIN,
        at_us: 0,
        cpu_us: NO_CPU,
    };
}

struct Kept {
    points: [Point; CAP],
    len: usize,
    last_cpu: u64,
}

/// One bounded payload per app, never per tab, with no periodic work.
pub(super) struct Startup {
    state: AtomicU8,
    lost: AtomicU64,
    kept: Mutex<Kept>,
}

impl Default for Startup {
    fn default() -> Self {
        Self {
            state: AtomicU8::new(NOT_BEGUN),
            lost: AtomicU64::new(0),
            kept: Mutex::new(Kept {
                points: [Point::EMPTY; CAP],
                len: 0,
                last_cpu: NO_CPU,
            }),
        }
    }
}

impl Startup {
    fn note(&self, phase: Phase, sample: impl FnOnce() -> (u64, Option<u64>)) {
        self.note_observed(phase, sample, || {});
    }

    // The observer makes the publication boundary testable without scheduling or altering state.
    fn note_observed(&self, phase: Phase, sample: impl FnOnce() -> (u64, Option<u64>), before_lock: impl FnOnce()) {
        let accepted = match phase {
            Phase::BEGIN => self.state.compare_exchange(NOT_BEGUN, CAPTURING, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            Phase::READY => self.state.compare_exchange(CAPTURING, FINISHING, Ordering::AcqRel, Ordering::Acquire).is_ok(),
            _ => self.state.load(Ordering::Acquire) == CAPTURING,
        };
        if !accepted {
            return;
        }
        before_lock();
        self.keep(phase, sample);
        if phase == Phase::READY {
            self.state.store(COMPLETE, Ordering::Release);
        }
    }

    fn keep(&self, phase: Phase, sample: impl FnOnce() -> (u64, Option<u64>)) {
        let mut kept = match self.kept.try_lock() {
            Ok(kept) => kept,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.lost.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        if kept.len == CAP {
            self.lost.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let (at_us, cpu) = sample();
        let cpu = cpu.filter(|cpu| *cpu != NO_CPU && (kept.last_cpu == NO_CPU || *cpu >= kept.last_cpu));
        if let Some(cpu) = cpu {
            kept.last_cpu = cpu;
        }
        let index = kept.len;
        kept.points[index] = Point {
            phase,
            at_us,
            cpu_us: cpu.unwrap_or(NO_CPU),
        };
        kept.len += 1;
    }

    fn json(&self) -> Value {
        self.json_observed(|| {})
    }

    // A later writer can finish after the fixed payload has been copied, before formatting.
    fn json_observed(&self, after_copy: impl FnOnce()) -> Value {
        let (state, lost, points, len) = {
            let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
            // A completion acquire observes its accounted point/loss before this payload is copied.
            let state = self.state.load(Ordering::Acquire);
            let lost = self.lost.load(Ordering::Relaxed);
            (state, lost, kept.points, kept.len)
        };
        after_copy();
        let points: Vec<Value> = points[..len]
            .iter()
            .map(|point| {
                json!({
                    "phase": point.phase.name(), "at_us": point.at_us,
                    "cpu_us": (point.cpu_us != NO_CPU).then_some(point.cpu_us),
                })
            })
            .collect();
        json!({ "clock": "watchdog", "cap": CAP, "complete": state == COMPLETE,
            "lost": lost, "record_bytes": std::mem::size_of::<Self>(), "points": points })
    }
}

/// A host observation only: unknown IDs and threads without the window's binding do nothing.
/// The deferred sample runs only for an accepted point with room and an immediately available lock.
pub fn checkpoint(id: u8) {
    let Some(phase) = Phase::decode(id) else { return };
    let _ = WINDOW_WATCH.try_with(|watch| {
        let Ok(watch) = watch.try_borrow() else { return };
        if let Some(shared) = watch.as_ref().filter(|shared| WINDOW_OF.get() == shared.id) {
            shared
                .short
                .startup
                .note(phase, || (u64::try_from(shared.epoch.elapsed().as_micros()).unwrap_or(u64::MAX), own_cpu_us()));
        }
    });
}

impl Watchdog {
    /// Only the QA reader allocates a report; checkpoints keep the fixed payload.
    #[must_use]
    pub fn startup_json(&self) -> Value {
        self.0.short.startup.json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(trace: &Startup, id: u8, at: u64, cpu: Option<u64>) {
        if let Some(phase) = Phase::decode(id) {
            trace.note(phase, || (at, cpu));
        }
    }

    /// Failure: phase16+1 grows memory or cannot freeze a full trace, so future work overwrites the startup failure.
    #[test]
    fn a_full_startup_trace_stays_bounded_and_still_freezes() {
        let trace = Startup::default();
        note(&trace, 0, 1, Some(1));
        for at in 2..=CAP as u64 {
            note(&trace, 1, at, Some(at));
        }
        note(&trace, 14, 99, Some(99));
        let report = trace.json();
        assert_eq!(report["points"].as_array().unwrap().len(), CAP);
        assert_eq!(report["lost"], 1);
        assert_eq!(report["complete"], true);
        note(&trace, 0, 100, Some(100));
        note(&trace, 1, 101, Some(101));
        assert_eq!(trace.json(), report);
    }

    /// Failure: a later callback fabricates a missing launch, or an unknown phase is accepted or a repeated begin resets the original launch.
    #[test]
    fn only_the_first_real_begin_can_start_a_trace() {
        let trace = Startup::default();
        for id in [1, 14, 15, u8::MAX] {
            note(&trace, id, 1, None);
        }
        assert!(trace.json()["points"].as_array().unwrap().is_empty());
        assert_eq!(trace.json()["complete"], false);
        note(&trace, 0, 2, Some(10));
        let begun = trace.json();
        for id in [15, u8::MAX] {
            note(&trace, id, 9, Some(20));
        }
        assert_eq!(trace.json(), begun);
        note(&trace, 0, 3, Some(20));
        note(&trace, 14, 4, Some(30));
        let report = trace.json();
        assert_eq!(report["points"].as_array().unwrap().len(), 2);
        assert_eq!(report["points"][0]["at_us"], 2);
        assert_eq!(report["points"][1]["phase"], "host_ready");
        assert_eq!(report["complete"], true);
        assert_eq!(report["lost"], 0);
    }

    /// Failure: a UI checkpoint waits for a QA reader, or a dropped completion leaves the trace active forever.
    #[test]
    fn contention_drops_the_point_without_waiting_and_completion_still_freezes() {
        let trace = std::sync::Arc::new(Startup::default());
        note(&trace, 0, 1, Some(1));
        let held = trace.kept.lock().unwrap();
        let other = std::sync::Arc::clone(&trace);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut sampled = false;
            other.note(Phase::READY, || {
                sampled = true;
                (2, Some(2))
            });
            done_tx.send(sampled).unwrap();
        });
        // The rule is completion while the reader still holds its lock. This bound only retires a broken blocking implementation.
        let completed_while_held = done_rx.recv_timeout(std::time::Duration::from_secs(10));
        drop(held);
        worker.join().unwrap();
        assert!(completed_while_held.is_ok(), "checkpoint waited for the reader to release its lock");
        assert!(!completed_while_held.unwrap(), "a contended point sampled instead of returning");
        assert_eq!(trace.state.load(Ordering::Acquire), COMPLETE);
        assert_eq!(trace.lost.load(Ordering::Relaxed), 1);
        let report = trace.json();
        assert_eq!(report["points"].as_array().unwrap().len(), 1);
        trace.note(Phase(1), || panic!("a sealed trace must not sample"));
        assert_eq!(trace.json(), report);
    }

    /// Failure: a failed or backwards CPU observation is shown as valid CPU work, or loses later valid absolute readings.
    #[test]
    fn unknown_and_backwards_cpu_observations_stay_unknown() {
        let trace = Startup::default();
        for (id, cpu) in [(0, Some(100)), (1, None), (2, Some(90)), (3, Some(110)), (14, Some(NO_CPU))] {
            note(&trace, id, u64::from(id), cpu);
        }
        let points = trace.json()["points"].as_array().unwrap().clone();
        assert_eq!(
            points.iter().map(|point| point["cpu_us"].clone()).collect::<Vec<_>>(),
            vec![json!(100), Value::Null, Value::Null, json!(110), Value::Null]
        );
    }

    /// Failure: startup instrumentation on another thread changes the window's launch, or changes the existing hang counters or kept stretches.
    #[test]
    fn startup_checkpoints_keep_the_window_binding_and_existing_watchdog_semantics() {
        let dog = Watchdog::new();
        // Bind this test's window without starting a real watcher or depending on scheduling.
        WINDOW_OF.set(dog.0.id);
        WINDOW_WATCH.with(|watch| *watch.borrow_mut() = Some(std::sync::Arc::clone(&dog.0)));
        dog.0.busy_since.store(123, Ordering::Release);
        let before = dog.report();
        let short = dog.slow_stretches().1;
        checkpoint(0);
        let other = dog.clone();
        std::thread::spawn(move || {
            checkpoint(14);
            assert_eq!(other.startup_json()["complete"], false);
        })
        .join()
        .unwrap();
        checkpoint(u8::MAX);
        assert_eq!(dog.startup_json()["complete"], false);
        checkpoint(14);
        assert_eq!(dog.startup_json()["complete"], true);
        assert_eq!(dog.startup_json()["points"].as_array().unwrap().len(), 2);
        assert_eq!(dog.report(), before);
        assert_eq!(dog.slow_stretches().1, short);
        assert_eq!(dog.0.busy_since.load(Ordering::Acquire), 123);
    }

    fn assert_completion_has_evidence(report: &Value) {
        if report["complete"] == true && report["lost"] == 0 {
            assert!(
                report["points"].as_array().unwrap().iter().any(|point| point["phase"] == "host_ready"),
                "completion claims no loss but its snapshot has no completion point: {report}"
            );
        }
    }

    /// Failure: a report copies old points, then reads completion from a newer version and claims that incomplete evidence is complete with no loss.
    #[test]
    fn a_completion_after_payload_copy_cannot_certify_the_older_snapshot() {
        let trace = Startup::default();
        note(&trace, 0, 1, Some(1));
        let report = trace.json_observed(|| note(&trace, 14, 2, Some(2)));
        assert_completion_has_evidence(&report);
        let final_report = trace.json();
        assert_eq!(final_report["complete"], true);
        assert_completion_has_evidence(&final_report);
    }

    /// Failure: the completion flag is published before either its point or its loss, so a concurrent report falsely certifies incomplete evidence.
    #[test]
    fn completion_is_not_visible_before_its_point_or_loss_is_accounted() {
        let trace = Startup::default();
        note(&trace, 0, 1, Some(1));
        let mut during = None;
        trace.note_observed(Phase::READY, || (2, Some(2)), || during = Some(trace.json()));
        assert_completion_has_evidence(&during.expect("a report in the publication gap"));
        let final_report = trace.json();
        assert_eq!(final_report["complete"], true);
        assert_completion_has_evidence(&final_report);
    }

    /// Failure: startup observation grows a per-app value without reporting its whole compiler size.
    #[test]
    fn startup_trace_whole_sizes_are_measured() {
        println!(
            "Point={} Kept={} Startup={} Short={} Shared={}",
            std::mem::size_of::<Point>(),
            std::mem::size_of::<Kept>(),
            std::mem::size_of::<Startup>(),
            std::mem::size_of::<super::super::Short>(),
            std::mem::size_of::<super::super::Shared>()
        );
        assert_eq!(std::mem::size_of::<Point>(), 24);
        assert_eq!(std::mem::size_of::<Kept>(), 400);
        assert_eq!(std::mem::size_of::<Startup>(), 432);
        assert_eq!(std::mem::size_of::<super::super::Shared>(), 1520);
    }
}
