//! QA commands, on their way to the UI thread. They run on a job thread of the server, so waiting here holds up nobody. A QA command calls what the UI calls: the host's answer says what happened.

use crate::watchdog::{FAIL_MS, NOTE_MS, Watchdog};
use crate::{AppListener, CoreQueue, lock};
use recto_agent::{AgentError, Command, exit};
use recto_control::state::is_in_real_home;
use recto_reader::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long the UI thread may take with a QA request before the answer is that it's stuck.
const UI_ANSWER_WAIT: Duration = Duration::from_secs(30);
/// How long a drive verb waits for its effect before saying how things stand. A click on a tab whose file isn't answering is still a click that's been made, and `qa wait-settled` is for waiting on as long as it takes.
const ANSWER_AFTER_CLICK: Duration = Duration::from_secs(3);
const DEFAULT_SETTLE_WAIT: Duration = Duration::from_secs(30);
/// How long a sweep may be asked to run.
const MAX_SWEEP: Duration = Duration::from_secs(60);
/// A step at 120 Hz, the rate the budget talks about.
const SWEEP_STEP: Duration = Duration::from_micros(8333);

/// One scroll step of a sweep, as the UI thread saw it.
#[derive(Clone, Copy)]
struct Step {
    micros: u32,
    at: Instant,
    contents_y: f64,
    current_visible: bool,
}

/// One move of the contents, while it's being found.
#[derive(Clone, Copy)]
struct Motion {
    start: f64,
    end: f64,
    from: f64,
    to: f64,
    peak: f64,
    largest: f64,
}

/// What the contents did during a sweep, found from where they were on each step: their moves, each one's length and peak speed, and how long they rested in between. Properties, not a curve.
fn contents_moves(steps: &[(f64, f64)]) -> Value {
    // Steps this close together with no movement between them are one move's slow part, and not a rest: the scroll is rounded to a device pixel, so a slow end can hold still for a few.
    const SAME_MOVE_GAP: f64 = 0.2;
    const SPEED_OVER: f64 = 0.025;
    let mut back = 0;
    // The moves themselves, up to a limit: a sweep can't have more that anyone would read one by one.
    const MOVES_LISTED: usize = 64;
    let finish = |motion: Motion, moves: &mut Vec<(f64, f64)>, listed: &mut Vec<Value>| {
        moves.push((motion.end - motion.start, motion.peak));
        if listed.len() < MOVES_LISTED {
            listed.push(json!({ "at_s": motion.start, "ms": (motion.end - motion.start) * 1000.0, "points": motion.to - motion.from, "peak": motion.peak, "largest_step": motion.largest }));
        }
    };
    let (mut moves, mut rests): (Vec<(f64, f64)>, Vec<f64>) = (Vec::new(), Vec::new());
    let mut listed: Vec<Value> = Vec::new();
    let mut current: Option<Motion> = None;
    let mut last_moved_at = steps.first().map_or(0.0, |step| step.0);
    let mut largest_step = 0.0f64;
    for pair in steps.windows(2) {
        let ((then, from), (now, to)) = (pair[0], pair[1]);
        largest_step = largest_step.max((to - from).abs());
        if to == from {
            continue;
        }
        // Over a little time, not one step: steps aren't evenly spaced, and two close together would read as a speed nothing reached.
        while now - steps[back + 1].0 >= SPEED_OVER {
            back += 1;
        }
        let speed = (to - steps[back].1).abs() / (now - steps[back].0).max(SPEED_OVER);
        current = match current {
            Some(motion) if then - motion.end <= SAME_MOVE_GAP => Some(Motion { end: now, to, peak: motion.peak.max(speed), largest: motion.largest.max((to - from).abs()), ..motion }),
            finished => {
                if let Some(motion) = finished {
                    rests.push(then - motion.end);
                    finish(motion, &mut moves, &mut listed);
                } else {
                    rests.push(then - last_moved_at);
                }
                Some(Motion { start: then, end: now, from, to, peak: speed, largest: (to - from).abs() })
            }
        };
        last_moved_at = now;
    }
    if let Some(motion) = current {
        finish(motion, &mut moves, &mut listed);
    }
    let sorted = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values
    };
    let lengths = sorted(moves.iter().map(|one| one.0).collect());
    let peaks = sorted(moves.iter().map(|one| one.1).collect());
    let rests = sorted(rests);
    let middle = |values: &[f64]| values.get(values.len() / 2).copied().unwrap_or(0.0);
    json!({
        "moves": moves.len(),
        "move_ms": { "median": middle(&lengths) * 1000.0, "longest": lengths.last().copied().unwrap_or(0.0) * 1000.0 },
        "peak_points_per_s": { "median": middle(&peaks), "highest": peaks.last().copied().unwrap_or(0.0) },
        "rest_s": { "shortest": rests.first().copied().unwrap_or(0.0), "median": middle(&rests) },
        "largest_step_points": largest_step,
        "listed": listed,
    })
}

pub(crate) struct QaBridge {
    listener: Arc<dyn AppListener>,
    engine: Engine,
    watchdog: Watchdog,
    core: Arc<CoreQueue>,
    next_id: AtomicU64,
    answers: Mutex<HashMap<u64, Option<Result<String, String>>>>,
    answered: Condvar,
    steps: Mutex<Vec<Step>>,
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

    pub(crate) fn note_step(&self, micros: u64, contents_y: f64, current_visible: bool) {
        self.step_waiting.store(false, Ordering::SeqCst);
        let mut steps = lock(&self.steps);
        // A minute at 120 Hz at most, and each is 4 bytes.
        if steps.len() < 16_384 {
            steps.push(Step { micros: u32::try_from(micros).unwrap_or(u32::MAX), at: Instant::now(), contents_y, current_visible });
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
                "shot" => self.shot(out.as_deref().ok_or_else(|| bad("`qa shot` needs `--out`."))?),
                _ => Err(bad("That isn't a QA command.")),
            },
            Command::QaDrive { verb, value, timeout_ms } => self.drive(verb, value, *timeout_ms),
            _ => Err(bad("That isn't a QA command.")),
        }
    }

    /// The host draws it into a temp file of ours, and it's published here, by the rule `render` keeps to: nothing already there is replaced. A QA run can be started from Off, and this is at Read-only.
    fn shot(&self, out: &Path) -> Result<Value, AgentError> {
        refuse_shot_path(out, is_in_real_home)?;
        let temp = std::env::temp_dir().join(format!("recto-shot-{}-{}.png", std::process::id(), self.next_id.fetch_add(1, Ordering::SeqCst)));
        let answer = self.ask("shot", &temp.to_string_lossy());
        let published = answer.and_then(|mut answer| {
            recto_agent::publish_picture(&temp, out)?;
            answer["out"] = json!(out);
            Ok(answer)
        });
        let _ = std::fs::remove_file(&temp);
        published
    }

    fn drive(&self, verb: &str, value: &str, timeout_ms: Option<u64>) -> Result<Value, AgentError> {
        let number = |low: f64, high: f64| -> Result<f64, AgentError> {
            value.parse::<f64>().ok().filter(|number| number.is_finite() && *number >= low && *number <= high).ok_or_else(|| bad(format!("`{}` isn't a number from {low} to {high}.", cleaned(value))))
        };
        match verb {
            "appearance" | "window" | "hover" | "click_row" | "resize" | "contents_scroll" | "contents_pointer" | "reduce_motion" | "reveal" | "scroller_style" => check_drive_value(verb, value)?,
            "entry_points" => return self.ask(verb, value),
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
        self.core.flush(ANSWER_AFTER_CLICK);
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
        let recorded = lock(&self.steps).clone();
        let mut steps: Vec<u32> = recorded.iter().map(|step| step.micros).collect();
        steps.sort_unstable();
        // What the contents did, step by step, while the page scrolled.
        let began = recorded.first().map_or_else(Instant::now, |step| step.at);
        let positions: Vec<(f64, f64)> = recorded.iter().map(|step| (step.at.duration_since(began).as_secs_f64(), step.contents_y)).collect();
        let mut contents = contents_moves(&positions);
        contents["steps_with_place_in_view"] = json!(recorded.iter().filter(|step| step.current_visible).count());
        let at = |share: f64| -> u32 { steps.get(((steps.len() as f64 - 1.0) * share).round() as usize).copied().unwrap_or(0) };
        let after = self.engine.report().0;
        Ok(json!({
            "steps": steps.len(),
            "contents": contents,
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

/// What each QA verb takes. Here, so the host is only ever told what it can do, and has no rules of its own about it.
fn check_drive_value(verb: &str, value: &str) -> Result<(), AgentError> {
    let one_of = |words: &[&str]| words.contains(&value);
    let row = || value.parse::<u32>().is_ok();
    let fits = match verb {
        "appearance" => one_of(&["light", "dark"]),
        "window" => one_of(&["open", "close"]),
        "contents_pointer" => one_of(&["in", "out"]),
        "reduce_motion" => one_of(&["on", "off"]),
        "scroller_style" => one_of(&["legacy", "overlay"]),
        "hover" | "reveal" => value == "none" || row(),
        "click_row" | "contents_scroll" => row(),
        "resize" => value.split_once('x').and_then(|(width, height)| Some((width.parse::<f64>().ok()?, height.parse::<f64>().ok()?))).is_some_and(|(width, height)| (400.0..=8000.0).contains(&width) && (300.0..=8000.0).contains(&height)),
        _ => false,
    };
    if fits {
        return Ok(());
    }
    Err(bad(format!("`{}` isn't something `{}` takes.", cleaned(value), verb.replace('_', "-"))))
}

/// What a `qa shot` may write: a new `.png`, outside the real home.
fn refuse_shot_path(out: &Path, in_real_home: impl Fn(&Path) -> bool) -> Result<(), AgentError> {
    if !out.is_absolute() {
        return Err(bad("That path isn't whole. A session has no folder of its own to start from."));
    }
    if !out.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("png")) {
        return Err(bad("A shot is a new `.png` file."));
    }
    if in_real_home(out) {
        return Err(AgentError::new("path_refused", "A shot goes outside your home folder, somewhere temporary.", exit::FAILED));
    }
    // `symlink_metadata`, so a link is something being there, wherever it points or if it points nowhere.
    if std::fs::symlink_metadata(out).is_ok() {
        return Err(recto_agent::output_exists());
    }
    Ok(())
}

fn bad(words: impl Into<String>) -> AgentError {
    AgentError::new("usage", words.into(), exit::USAGE)
}

fn cleaned(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the recording calls one move many, or a rest a move, so the numbers reported say something other than what happened. A made-up path: rest, a move, rest, a move that holds still for a moment at a rounded pixel, rest.
    #[test]
    fn moves_are_found_from_where_the_contents_were() {
        let mut path = Vec::new();
        let mut time = 0.0;
        let mut add = |y: f64, seconds: f64| {
            let mut left = seconds;
            while left > 0.0 {
                path.push((time, y));
                time += 0.01;
                left -= 0.01;
            }
        };
        add(0.0, 2.0);
        for step in 1..=20 {
            add(f64::from(step) * 10.0, 0.01);
        }
        add(200.0, 3.0);
        for step in 1..=10 {
            add(200.0 + f64::from(step) * 5.0, 0.01);
        }
        add(250.0, 0.03);
        for step in 1..=10 {
            add(250.0 + f64::from(step) * 5.0, 0.01);
        }
        add(300.0, 1.0);
        let found = contents_moves(&path);
        assert_eq!(found["moves"], 2, "{found}");
        assert!(found["rest_s"]["shortest"].as_f64().unwrap() > 1.5, "{found}");
        assert!(found["move_ms"]["longest"].as_f64().unwrap() >= 200.0, "{found}");
        assert!(found["largest_step_points"].as_f64().unwrap() <= 10.0 + 0.001, "{found}");
        assert!(found["peak_points_per_s"]["highest"].as_f64().unwrap() >= 900.0, "{found}");
        assert_eq!(contents_moves(&[(0.0, 5.0), (1.0, 5.0)])["moves"], 0, "still");
    }

    /// Failure: a shot lands in the real home, by a path or through a link to it, or takes a document's name. Holds the rule, with what counts as the home given.
    #[test]
    fn a_shot_goes_only_where_it_may() {
        let nowhere = |_: &Path| false;
        let dir = std::env::temp_dir();
        assert_eq!(refuse_shot_path(Path::new("a.png"), nowhere).unwrap_err().code, "usage", "not whole");
        assert_eq!(refuse_shot_path(&dir.join("a.pdf"), nowhere).unwrap_err().code, "usage", "a document's name");
        assert_eq!(refuse_shot_path(&dir.join("a"), nowhere).unwrap_err().code, "usage");
        assert_eq!(refuse_shot_path(&dir.join("a.png"), |_| true).unwrap_err().code, "path_refused");
        assert!(refuse_shot_path(&dir.join("recto-no-such-shot.PNG"), nowhere).is_ok());
    }

    /// Failure: a word the host can't do reports success: `qa appearance zzz` said it was done and went light, since the rule was the host's. They're checked here and every one is refused.
    #[test]
    fn a_verb_only_takes_what_it_can_do() {
        for (verb, good, wrong) in [("appearance", "dark", "zzz"), ("window", "close", "zzz"), ("contents_pointer", "in", "zzz"), ("reduce_motion", "on", "yes"), ("hover", "none", "-1"), ("reveal", "3", "x"), ("click_row", "0", "none"), ("contents_scroll", "9", "1.5"), ("resize", "800x600", "10x10")] {
            assert!(check_drive_value(verb, good).is_ok(), "{verb} {good}");
            assert_eq!(check_drive_value(verb, wrong).unwrap_err().code, "usage", "{verb} {wrong}");
        }
        assert!(check_drive_value("resize", "800").is_err());
    }
}
