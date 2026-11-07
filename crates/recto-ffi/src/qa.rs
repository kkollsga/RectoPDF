//! QA commands, on their way to the UI thread. They run on a job thread of the server, so waiting here holds up nobody. A QA command calls what the UI calls: the host's answer says what happened.

use crate::watchdog::{FAIL_MS, NOTE_MS, Watchdog};
use crate::{AppListener, CoreQueue, lock};
use recto_agent::{AgentError, Command, exit};
use recto_reader::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long the UI thread may take with a QA request before the answer is that it's stuck.
const UI_ANSWER_WAIT: Duration = Duration::from_secs(30);
const DEFAULT_SETTLE_WAIT: Duration = Duration::from_secs(30);
/// How long a sweep may be asked to run.
const MAX_SWEEP: Duration = Duration::from_secs(60);
/// A step at 120 Hz, the rate the budget talks about.
const SWEEP_STEP: Duration = Duration::from_micros(8333);

pub(crate) struct QaBridge {
    listener: Arc<dyn AppListener>,
    engine: Engine,
    watchdog: Watchdog,
    core: Arc<CoreQueue>,
    next_id: AtomicU64,
    answers: Mutex<HashMap<u64, Option<Result<String, String>>>>,
    answered: Condvar,
    steps: Mutex<Vec<u32>>,
    step_waiting: AtomicBool,
}

impl QaBridge {
    pub(crate) fn new(listener: Arc<dyn AppListener>, engine: Engine, watchdog: Watchdog, core: Arc<CoreQueue>) -> Self {
        Self { listener, engine, watchdog, core, next_id: AtomicU64::new(1), answers: Mutex::new(HashMap::new()), answered: Condvar::new(), steps: Mutex::new(Vec::new()), step_waiting: AtomicBool::new(false) }
    }

    pub(crate) fn answer(&self, id: u64, ok: bool, json: String) {
        let mut answers = lock(&self.answers);
        if let Some(slot) = answers.get_mut(&id) {
            *slot = Some(if ok { Ok(json) } else { Err(json) });
            self.answered.notify_all();
        }
    }

    pub(crate) fn note_step(&self, micros: u64) {
        self.step_waiting.store(false, Ordering::SeqCst);
        let mut steps = lock(&self.steps);
        // A minute at 120 Hz at most, and each is 4 bytes.
        if steps.len() < 16_384 {
            steps.push(u32::try_from(micros).unwrap_or(u32::MAX));
        }
    }

    /// Asks the UI thread and waits for it. No answer in time means the UI thread is stuck, which is its own kind of failure.
    fn ask(&self, verb: &str, value: &str) -> Result<Value, AgentError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.answers).insert(id, None);
        self.listener.qa_request(id, verb.to_string(), value.to_string());
        let deadline = Instant::now() + UI_ANSWER_WAIT;
        let mut answers = lock(&self.answers);
        loop {
            match answers.get_mut(&id).map(Option::take) {
                Some(Some(answer)) => {
                    answers.remove(&id);
                    return match answer {
                        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or(Value::Null)),
                        Err(words) => Err(AgentError::new("qa_failed", words, exit::FAILED)),
                    };
                }
                Some(None) => {}
                None => return Err(stuck()),
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                answers.remove(&id);
                return Err(stuck());
            }
            answers = self.answered.wait_timeout(answers, left).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    pub(crate) fn run(&self, command: &Command) -> Result<Value, AgentError> {
        match command {
            Command::QaRead { verb, out, timeout_ms } => match verb.as_str() {
                "state" => self.state(),
                "settled" => self.settled(timeout_ms.map_or(DEFAULT_SETTLE_WAIT, Duration::from_millis)),
                "shot" => {
                    let out = out.as_deref().ok_or_else(|| bad("`qa shot` needs `--out`."))?;
                    if !out.is_absolute() {
                        return Err(bad("That path isn't whole. A session has no folder of its own to start from."));
                    }
                    self.ask("shot", &out.to_string_lossy())
                }
                _ => Err(bad("That isn't a QA command.")),
            },
            Command::QaDrive { verb, value, timeout_ms } => self.drive(verb, value, *timeout_ms),
            _ => Err(bad("That isn't a QA command.")),
        }
    }

    fn drive(&self, verb: &str, value: &str, timeout_ms: Option<u64>) -> Result<Value, AgentError> {
        let number = |low: f64, high: f64| -> Result<f64, AgentError> {
            value.parse::<f64>().ok().filter(|number| number.is_finite() && *number >= low && *number <= high).ok_or_else(|| bad(format!("`{}` isn't a number from {low} to {high}.", recto_core_clean(value))))
        };
        match verb {
            "appearance" | "window" | "hover" | "click_row" | "resize" => {}
            "scroll" => {
                number(-1e9, 1e9)?;
            }
            "zoom" => {
                number(recto_reader::tiles::MIN_ZOOM, recto_reader::tiles::MAX_ZOOM)?;
            }
            "stall" => {
                number(0.0, 10_000.0)?;
            }
            "sweep" => return self.sweep(number(0.1, 1000.0)?, Duration::from_millis(timeout_ms.unwrap_or(5000)).min(MAX_SWEEP)),
            _ => return Err(bad("That isn't a QA command.")),
        }
        self.ask(verb, value)?;
        // A click goes to the session thread and comes back as a model. The answer is what's true once that's happened, so nobody has to sleep and hope.
        self.core.flush(UI_ANSWER_WAIT);
        self.ask("state", "")
    }

    fn state(&self) -> Result<Value, AgentError> {
        let app = self.ask("state", "")?;
        let (worker_bytes, worker_peak) = self.engine.workers_footprint();
        let own = recto_platform::footprint_bytes().unwrap_or(0);
        let counters = recto_platform::local::process_counters(std::process::id());
        let (over_note, over_fail, worst) = self.watchdog.report();
        Ok(json!({
            "app": app,
            "reader": self.engine.report().0,
            "memory": {
                "footprint_bytes": own,
                "worker_bytes": worker_bytes,
                "worker_peak_bytes": worker_peak,
                "tree_bytes": own + worker_bytes,
                "threads": counters.map(|counters| counters.threads),
                "open_handles": recto_platform::open_handle_count(),
            },
            "hangs": { "over_1s": over_note, "over_2s": over_fail, "worst_ms": worst, "note_ms": NOTE_MS, "fail_ms": FAIL_MS },
        }))
    }

    fn settled(&self, limit: Duration) -> Result<Value, AgentError> {
        let started = Instant::now();
        // What's been asked of the session thread comes first: an `open` it hasn't reached yet isn't settled, it's not started.
        if self.core.flush(limit) && self.engine.wait_settled(limit.saturating_sub(started.elapsed())) {
            Ok(json!({ "settled": true, "waited_ms": started.elapsed().as_millis() as u64 }))
        } else {
            Err(AgentError::new("timed_out", "The page view didn't settle in time.", exit::TIMED_OUT).then("`recto qa state` shows what it's still waiting for."))
        }
    }

    /// Scrolls at a steady speed by what a scroll does, measuring as it goes. The UI thread's work, since a quiet run has no frames reaching a screen.
    fn sweep(&self, pages_per_second: f64, length: Duration) -> Result<Value, AgentError> {
        lock(&self.steps).clear();
        let before = self.engine.report().0;
        let pages = before["document_height"].as_f64().unwrap_or(0.0) / before["pages"].as_f64().unwrap_or(1.0).max(1.0);
        let points_per_step = pages * pages_per_second * SWEEP_STEP.as_secs_f64();
        let started = Instant::now();
        let mut next = started;
        let (mut sent, mut skipped) = (0u64, 0u64);
        let (mut peak_own, mut peak_worker, mut peak_tree) = (0u64, 0u64, 0u64);
        let mut sampled = started;
        let mut owed = 0.0;
        while started.elapsed() < length {
            owed += points_per_step;
            if self.step_waiting.swap(true, Ordering::SeqCst) {
                skipped += 1;
            } else {
                sent += 1;
                self.listener.qa_request(0, "scroll_step".to_string(), owed.to_string());
                owed = 0.0;
            }
            if sampled.elapsed() >= Duration::from_millis(20) {
                sampled = Instant::now();
                let own = recto_platform::footprint_bytes().unwrap_or(0);
                let (worker, _) = self.engine.workers_footprint();
                peak_own = peak_own.max(own);
                peak_worker = peak_worker.max(worker);
                peak_tree = peak_tree.max(own + worker);
            }
            next += SWEEP_STEP;
            let wait = next.saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        let stopped = Instant::now();
        // A final viewport that isn't moving, so whatever's still wanted gets drawn.
        let settled = self.engine.wait_settled(Duration::from_secs(20));
        let sharp_after_ms = stopped.elapsed().as_secs_f64() * 1000.0;
        let mut steps = lock(&self.steps).clone();
        steps.sort_unstable();
        let at = |share: f64| -> u32 { steps.get(((steps.len() as f64 - 1.0) * share).round() as usize).copied().unwrap_or(0) };
        let after = self.engine.report().0;
        Ok(json!({
            "steps": steps.len(),
            "sent": sent,
            "skipped_because_the_last_was_still_running": skipped,
            "step_us": { "p50": at(0.5), "p99": at(0.99), "worst": steps.last().copied().unwrap_or(0) },
            "blank_seen_steps": after["counts"]["blank_seen_steps"].as_u64().unwrap_or(0) - before["counts"]["blank_seen_steps"].as_u64().unwrap_or(0),
            "blank_steps": after["counts"]["blank_steps"].as_u64().unwrap_or(0) - before["counts"]["blank_steps"].as_u64().unwrap_or(0),
            "cancelled": after["counts"]["cancelled"].as_u64().unwrap_or(0) - before["counts"]["cancelled"].as_u64().unwrap_or(0),
            "peak_footprint_bytes": peak_own,
            "peak_worker_bytes": peak_worker,
            "peak_tree_bytes": peak_tree,
            "settled": settled,
            "sharp_after_ms": sharp_after_ms,
            "page": after["page"],
        }))
    }
}

fn stuck() -> AgentError {
    AgentError::new("ui_not_answering", "The app's UI thread didn't answer in 30 s.", exit::TIMED_OUT).then("That's a hang, and a bug. `recto qa state` may show it once it's back.")
}

fn bad(words: impl Into<String>) -> AgentError {
    AgentError::new("usage", words.into(), exit::USAGE)
}

fn recto_core_clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(64).collect()
}
