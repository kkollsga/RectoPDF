//! QA commands, on their way to the UI thread. They run on a job thread of the server, so waiting here holds up nobody. A QA command calls what the UI calls: the host's answer says what happened.

use crate::watchdog::{FAIL_MS, NOTE_MS, Watchdog};
use crate::{AppCore, AppListener, CoreQueue, Inner, lock};
use recto_agent::{AgentError, Command, exit};
use recto_control::state::is_in_real_home;
use recto_reader::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

/// How long a quiet run lasts at most, if it isn't told.
pub(crate) const QUIET_RUN_LIMIT: Duration = Duration::from_secs(600);

/// A test's slow disk: how long a quiet run waits before it reads the state, from `RECTO_QA_RESTORE_DELAY_MS`, so a restore can come after the window's gone up without it. Nothing outside a quiet run.
pub(crate) fn restore_delay(quiet: bool) -> Option<Duration> {
    if !quiet {
        return None;
    }
    let ms: u64 = std::env::var("RECTO_QA_RESTORE_DELAY_MS").ok()?.parse().ok()?;
    Some(Duration::from_millis(ms.min(30_000)))
}

/// A test's slow window: how long after the state comes in late the window's made again from it, from `RECTO_QA_REMAKE_DELAY_MS`. Nothing outside a quiet run.
pub(crate) fn remake_delay(quiet: bool) -> Option<Duration> {
    if !quiet {
        return None;
    }
    let ms: u64 = std::env::var("RECTO_QA_REMAKE_DELAY_MS").ok()?.parse().ok()?;
    Some(Duration::from_millis(ms.min(10_000)))
}

/// A test's outside change to a quiet run's activation policy: how often the run turns itself `accessory`, as something outside the app once did (a full check, 2026-11-11, and `open -g` of a copy with the same identifier, twice in about 145 reports), from `RECTO_QA_POLICY_NUDGE_MS`. Nothing outside a quiet run.
pub(crate) fn policy_nudge(quiet: bool) -> Option<Duration> {
    if !quiet {
        return None;
    }
    let ms: u64 = std::env::var("RECTO_QA_POLICY_NUDGE_MS").ok()?.parse().ok()?;
    Some(Duration::from_millis(ms.clamp(5, 10_000)))
}

#[uniffi::export]
impl AppCore {
    /// [`policy_nudge`], in milliseconds, for the host's quiet-run guard.
    pub fn qa_policy_nudge_ms(&self) -> Option<u64> {
        policy_nudge(self.qa_mode).map(|every| every.as_millis() as u64)
    }
}

/// What a quiet run's relaunch by an update carries over of a test's slow disk, so the new version reads its state as late.
pub(crate) fn relaunch_env() -> Vec<(String, String)> {
    std::env::var("RECTO_QA_RESTORE_DELAY_MS")
        .ok()
        .filter(|ms| ms.parse::<u64>().is_ok())
        .map(|ms| ("RECTO_QA_RESTORE_DELAY_MS".to_owned(), ms))
        .into_iter()
        .collect()
}

/// The process this quiet run ends with, if it was told one: what a relaunch by an update passes on.
static OWNER: OnceLock<u32> = OnceLock::new();

/// What a quiet run's relaunch is given, so the new run ends with the same test.
pub(crate) fn owner_args() -> Vec<String> {
    OWNER.get().map(|pid| vec!["--qa-owner-pid".to_owned(), pid.to_string()]).unwrap_or_default()
}

/// **A quiet run ends with what started it** (PLAN: three ran on, orphaned, for up to their limit after their worktrees were gone). One thread, asleep in one `kevent` with no wake-ups, until the soonest of: the test or script it was started for ends (`--qa-owner-pid`, from `RECTO_QA_OWNER_PID` or `recto qa start --owner`), its temp state folder goes, its own program is deleted, as a worktree's removal or a rebuild does, or its time limit is up. A run started by hand with no owner, as an agent does one command at a time, lasts until one of the others.
pub(crate) fn end_with_starter(limit: Duration, owner: Option<u32>, state: Option<std::path::PathBuf>) {
    if let Some(pid) = owner {
        let _ = OWNER.set(pid);
    }
    let program = std::env::current_exe().ok();
    let _ = std::thread::Builder::new().name("recto-qa-limit".into()).stack_size(64 * 1024).spawn(move || {
        let pids: Vec<u32> = owner.into_iter().collect();
        let paths: Vec<&Path> = state.iter().chain(program.iter()).map(std::path::PathBuf::as_path).collect();
        let gone = recto_platform::local::wait_until_gone(&pids, &paths, limit);
        let why = match gone {
            recto_platform::local::Gone::Process(_) => "its test or script ended",
            recto_platform::local::Gone::Path(_) => "its state folder or its program went",
            recto_platform::local::Gone::TimeUp => "its time limit was up",
        };
        eprintln!("recto: this quiet run ended, as {why}.");
        std::process::exit(124);
    });
}

fn updates_takes_input(inner: &Inner) -> bool {
    crate::updates::takes_input(inner.shown_with_state.load(Ordering::SeqCst), inner.started.elapsed())
}

/// How long the UI thread may take with a QA request before the answer is that it's stuck.
const UI_ANSWER_WAIT: Duration = Duration::from_secs(30);
/// How long a shot waits for the page view to settle before it's taken anyway, saying so.
const SHOT_SETTLE: Duration = Duration::from_secs(10);

/// How long a drive verb waits for its effect before saying how things stand. A click on a tab whose file isn't answering is still a click that's been made, and `qa wait-settled` is for waiting on as long as it takes.
const ANSWER_AFTER_CLICK: Duration = Duration::from_secs(3);
const DEFAULT_SETTLE_WAIT: Duration = Duration::from_secs(30);
/// How often `qa wait-settled` asks whether the window's animations have ended: a frame and a bit, so it's back within one of the end, and a 0.15 s fade is asked about some ten times.
const ANIMATION_POLL: Duration = Duration::from_millis(15);
/// How long `qa handoff` gives its answer to get out, before it quits. In what a QA run measures as the gap, and in no real update.
const HANDOFF_ANSWER_WAIT: Duration = Duration::from_millis(50);
/// How long a sweep may be asked to run.
const MAX_SWEEP: Duration = Duration::from_secs(60);
/// A step at 120 Hz, the rate the budget talks about.
const SWEEP_STEP: Duration = Duration::from_micros(8333);

/// One scroll step of a sweep, as the UI thread saw it.
#[derive(Clone, Copy)]
struct Step {
    micros: u32,
    /// Of those, the time the UI thread was on a processor: the rest it was waiting, for a lock or for its turn.
    cpu_micros: u32,
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
            Some(motion) if then - motion.end <= SAME_MOVE_GAP => Some(Motion {
                end: now,
                to,
                peak: motion.peak.max(speed),
                largest: motion.largest.max((to - from).abs()),
                ..motion
            }),
            finished => {
                if let Some(motion) = finished {
                    rests.push(then - motion.end);
                    finish(motion, &mut moves, &mut listed);
                } else {
                    rests.push(then - last_moved_at);
                }
                Some(Motion {
                    start: then,
                    end: now,
                    from,
                    to,
                    peak: speed,
                    largest: (to - from).abs(),
                })
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
    inner: OnceLock<Weak<Inner>>,
}

impl QaBridge {
    pub(crate) fn new(listener: Arc<dyn AppListener>, engine: Engine, watchdog: Watchdog, core: Arc<CoreQueue>) -> Self {
        Self {
            listener,
            engine,
            watchdog,
            core,
            next_id: AtomicU64::new(1),
            answers: Mutex::new(HashMap::new()),
            answered: Condvar::new(),
            steps: Mutex::new(Vec::new()),
            step_waiting: AtomicBool::new(false),
            inner: OnceLock::new(),
        }
    }

    pub(crate) fn attach(&self, inner: Weak<Inner>) {
        let _ = self.inner.set(inner);
    }

    fn inner(&self) -> Option<Arc<Inner>> {
        self.inner.get().and_then(Weak::upgrade)
    }

    /// Every page view: one, or a comparison's panes.
    fn engines(&self) -> Vec<Engine> {
        self.inner()
            .map_or_else(|| vec![self.engine.clone()], |inner| inner.panes.engines(&inner.engine))
    }

    /// The page view of the pane being read, which QA's pointer goes to, as the reader's would.
    fn reading_engine(&self) -> Engine {
        self.inner()
            .map_or_else(|| self.engine.clone(), |inner| inner.panes.reading_engine(&inner.engine))
    }

    /// Live workers that said their walls were up.
    fn walled_workers(&self) -> u32 {
        self.engines().iter().map(|engine| engine.walled_workers()).sum()
    }

    /// The workers of every page view, now and at their peaks.
    fn workers_footprint(&self) -> (u64, u64, usize) {
        self.engines().iter().fold((0, 0, 0), |(now, peak, count), engine| {
            let (one, top) = engine.workers_footprint();
            (now + one, peak + top, count + engine.worker_pids().len())
        })
    }

    pub(crate) fn answer(&self, id: u64, ok: bool, json: String) {
        let mut answers = lock(&self.answers);
        if let Some(slot) = answers.get_mut(&id) {
            *slot = Some(if ok { Ok(json) } else { Err(json) });
            self.answered.notify_all();
        }
    }

    pub(crate) fn note_step(&self, micros: u64, cpu_micros: u64, contents_y: f64, current_visible: bool) {
        self.step_waiting.store(false, Ordering::SeqCst);
        let mut steps = lock(&self.steps);
        // A minute at 120 Hz at most, and each is 4 bytes.
        if steps.len() < 16_384 {
            steps.push(Step {
                micros: u32::try_from(micros).unwrap_or(u32::MAX),
                cpu_micros: u32::try_from(cpu_micros).unwrap_or(u32::MAX),
                at: Instant::now(),
                contents_y,
                current_visible,
            });
        }
    }

    /// Asks the UI thread and waits for it. No answer in time means the UI thread is stuck, which is its own kind of failure.
    fn ask(&self, verb: &str, value: &str) -> Result<Value, AgentError> {
        // What acts on the window waits for it to take input, as a person's click would have to: a window up without the state is made again from it, and what was done to it would go (CI, 2026-11-11). Looking doesn't wait.
        if !matches!(verb, "state" | "animations" | "copied") {
            let began = Instant::now();
            while !self.inner().is_none_or(|inner| updates_takes_input(&inner)) && began.elapsed() < UI_ANSWER_WAIT {
                std::thread::sleep(ANIMATION_POLL);
            }
        }
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
                "shot" | "shot_settings" | "shot_menu" | "shot_menus" => self.shot(verb, out.as_deref().ok_or_else(|| bad("A shot needs `--out`."))?),
                "copied" => self.ask("copied", ""),
                _ => Err(bad("That isn't a QA command.")),
            },
            Command::QaDrive { verb, value, timeout_ms } => self.drive(verb, value, *timeout_ms),
            _ => Err(bad("That isn't a QA command.")),
        }
    }

    /// The host draws it into a temp file of ours, and it's published here, by the rule `render` keeps to: nothing already there is replaced. A QA run can be started from Off, and this is at Read-only.
    fn shot(&self, verb: &str, out: &Path) -> Result<Value, AgentError> {
        refuse_shot_path(out, is_in_real_home)?;
        // A shot of a page mid-draw shows it blurred, and one of the contents mid-slide shows two rows over each other: it waits for settled, as `qa wait-settled` has it, every pane and the window's animations, for as long as a shot may, and says if it didn't (review).
        let started = Instant::now();
        let settled = self.settled(SHOT_SETTLE).is_ok();
        let temp = std::env::temp_dir().join(format!("recto-shot-{}-{}.png", std::process::id(), self.next_id.fetch_add(1, Ordering::SeqCst)));
        let answer = self.ask(verb, &temp.to_string_lossy());
        let published = answer.and_then(|mut answer| {
            recto_agent::publish_picture(&temp, out)?;
            answer["out"] = json!(out);
            answer["settled"] = json!(settled);
            answer["settle_waited_ms"] = json!(started.elapsed().as_millis() as u64);
            Ok(answer)
        });
        let _ = std::fs::remove_file(&temp);
        published
    }

    fn drive(&self, verb: &str, value: &str, timeout_ms: Option<u64>) -> Result<Value, AgentError> {
        let number = |low: f64, high: f64| -> Result<f64, AgentError> {
            value
                .parse::<f64>()
                .ok()
                .filter(|number| number.is_finite() && *number >= low && *number <= high)
                .ok_or_else(|| bad(format!("`{}` isn't a number from {low} to {high}.", cleaned(value))))
        };
        if let Some(inner) = self.inner()
            && let Some(result) = inner.qa_session_verb(verb, value)
        {
            result?;
            self.core.flush(ANSWER_AFTER_CLICK);
            return self.ask("state", "");
        }
        match verb {
            // What ends the app answers first, as nothing can after.
            "quit" => return self.ask(verb, value),
            "handoff" => {
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                let report = inner
                    .hand_off(ANSWER_AFTER_CLICK)
                    .map_err(|error| AgentError::new("hand_off_refused", error.to_string(), exit::FAILED))?;
                // The answer gets out first. An updater has no one to answer, and doesn't wait.
                let quitting = Arc::clone(&inner);
                let _ = std::thread::Builder::new().name("recto-qa-quit".into()).stack_size(64 * 1024).spawn(move || {
                    std::thread::sleep(HANDOFF_ANSWER_WAIT);
                    quitting.hand_off_quit_now();
                });
                return Ok(
                    json!({ "handed_off": true, "tabs": report.tabs, "edited_tabs": report.edited_tabs, "record_bytes": report.record_bytes, "journal_bytes": report.journal_bytes, "wrote_bytes": report.wrote_bytes, "total_us": report.total_us, "force_us": report.force_us }),
                );
            }
            "hand_off_quit" => {
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                return Ok(json!({ "quit": inner.hand_off_quit_now() }));
            }
            // What the timer does, by the same call, and a test's clock. It's the same call whichever way it comes.
            "update_tick" => {
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                inner.update_timer_fired();
                self.core.flush(ANSWER_AFTER_CLICK);
                return self.ask("state", "");
            }
            "update_clock" => {
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                let ahead = value.parse::<i64>().map_err(|_| bad("It's a number of milliseconds."))?;
                if !inner.update_test_clock(ahead) {
                    return Err(bad("Only a test run with its own update server has a clock to move."));
                }
                return self.ask("state", "");
            }
            "settings" => {
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                if !matches!(value, "open" | "close") && !inner.has_setting(value) {
                    return Err(bad(format!("`{}` isn't something `settings` takes.", cleaned(value))));
                }
                self.ask(verb, value)?;
                self.core.flush(ANSWER_AFTER_CLICK);
                return self.ask("state", "");
            }
            "open_file" => {
                // It takes a moment, and may never answer: `qa wait-settled` is for waiting on it. Answering is the point.
                return self.ask(verb, value);
            }
            "appearance" | "window" | "hover" | "click_row" | "compare_click" | "pane_focus" | "pane_scroll" | "pane_context_menu" | "resize"
            | "contents_scroll" | "contents_pointer" | "corner_pointer" | "reduce_motion" | "reveal" | "scroller_style" | "menu" | "panel" | "button"
            | "zoom_type" | "page_type" | "find_type" | "find_key" | "find_click" | "swipe" | "context_menu" | "key_action" | "full_screen" | "minimise"
            | "hide_app" | "focus" | "frontmost" | "sheet" | "sheet_answer" | "sidebar_width" | "accessibility" => check_drive_value(verb, value)?,
            "palette_type" | "palette_key" => {
                check_drive_value(verb, value)?;
                if !self.inner().is_some_and(|inner| inner.palette_open()) {
                    return Err(bad("The palette isn't open. `recto qa menu palette.open` opens it."));
                }
            }
            // A test's password, for a test's own file: the sheet showing must be asking about a file in the quiet run's state folder or a temp folder, so this can't type into a sheet for one of the person's own (review O6).
            "password" => {
                check_drive_value(verb, value)?;
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                let path = lock(&inner.password_asks)
                    .showing()
                    .map(|ask| ask.path.clone())
                    .ok_or_else(|| bad("No password sheet is open."))?;
                if !is_a_tests_file(&path, inner.paths.as_ref().map(|paths| paths.root.as_path())) {
                    return Err(bad(
                        "`qa password` types only into the sheet for a test's own file, in the quiet run's state folder or a temp folder.",
                    ));
                }
            }
            "entry_points" | "services" => return self.ask(verb, value),
            // A paste of one type from another app, as the system asks for it: a promised type is made now, and what's on the clipboard comes back as `qa copied` says it.
            "paste" => {
                if !value.starts_with("public.") || value.len() > 64 {
                    return Err(bad("A paste names a type, as `public.svg-image`."));
                }
                return self.ask("copied", value);
            }
            // A place is said as an agent has it, on a page, and goes in by the document's coordinates, on the way to the window's, so it goes through what a real pointer's position does.
            "mouse_down" | "mouse_drag" | "pointer" => {
                let place = parse_page_place(value)?;
                let (x, y) = self
                    .reading_engine()
                    .document_point(place.page, place.x, place.y)
                    .ok_or_else(|| bad("That page isn't in this document."))?;
                self.ask(verb, &format!("{x},{y},{},{}", place.clicks, u8::from(place.shift)))?;
                self.core.flush(ANSWER_AFTER_CLICK);
                return self.ask("state", "");
            }
            "mouse_up" | "notice_button" | "zoom_escape" | "page_escape" => {}
            // Where the print goes is held to the rules every export keeps, and a file that's there isn't replaced.
            "print_to_pdf" => {
                let (out, range) = value.split_once('|').map_or((value, None), |(out, range)| (out, Some(range)));
                let out = Path::new(out);
                recto_agent::saving::check_export_path(out, &["pdf"], "PDF")?;
                if !out.is_absolute() || std::fs::symlink_metadata(out).is_ok() {
                    return Err(bad("A print goes to a new file, by its whole path."));
                }
                if let Some(range) = range {
                    let (first, last) = range.split_once('|').ok_or_else(|| bad("A range is FIRST|LAST."))?;
                    if first.parse::<u32>().is_err() || last.parse::<u32>().is_err() {
                        return Err(bad("A range is two page numbers."));
                    }
                }
            }
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
        let (worker_bytes, worker_peak, workers) = self.workers_footprint();
        let comparison = self.inner().map_or(Value::Null, |inner| {
            let engines = inner.panes.engines(&inner.engine);
            let panes: Vec<Value> = engines.iter().map(|engine| {
                let report = engine.report().0;
                json!({ "tab": report["tab"], "page": report["page"], "pages": report["pages"], "scroll_y": report["scroll_y"], "magnification": report["magnification"], "fit": report["fit"], "settled": report["settled"], "spread": report["spread"], "thread": engine.has_thread() })
            }).collect();
            let held: u64 = engines.iter().map(|engine| engine.report().0["documents_held"].as_u64().unwrap_or(0)).sum();
            json!({ "panes": panes, "reading": inner.panes.reading(), "documents_held": held })
        });
        let own = recto_platform::footprint_bytes().unwrap_or(0);
        let mine = recto_platform::own_memory().unwrap_or_default();
        let counters = recto_platform::local::process_counters(std::process::id());
        let (over_note, over_fail, worst) = self.watchdog.report();
        Ok(json!({
            "app": app,
            "reader": self.engine.report().0,
            "panes": comparison,
            "session": self.inner().map_or(Value::Null, |inner| inner.state_json()),
            "update": self.inner().map_or(Value::Null, |inner| inner.update_report()),
            "record": self.inner().map_or(Value::Null, |inner| inner.record_report()),
            "memory": {
                "footprint_bytes": own,
                "compressed_bytes": mine.compressed,
                "heap_bytes": mine.heap,
                "workers": workers,
                "workers_walled": self.walled_workers(),
                "worker_bytes": worker_bytes,
                "worker_peak_bytes": worker_peak,
                "tree_bytes": own + worker_bytes,
                "threads": counters.map(|counters| counters.threads),
                "disk_bytes_written": counters.map(|counters| counters.disk_bytes_written),
                "open_handles": recto_platform::open_handle_count(),
            },
            "hangs": { "over_1s": over_note, "over_2s": over_fail, "worst_ms": worst, "note_ms": NOTE_MS, "fail_ms": FAIL_MS },
            "printing": self.inner().map_or(Value::Null, |inner| json!({ "busy": inner.printing.busy(), "notice": inner.printing.notice(), "pid": std::process::id() })),
            "snapshot": self.inner().map_or(Value::Null, |inner| json!({ "on": inner.tool.is_on(), "drawing": inner.tool.is_drawing(), "busy": inner.tool.busy(), "notice": inner.tool.notice().map(|notice| notice.text) })),
        }))
    }

    /// Settled is the session thread done with what it was asked, every page view drawn, and no host animation running that changes what's shown: the sidebar contents' fade and slide, their drift, a name about to be shown, a zoom about to go sharp. Each of those ends by itself, and one ending can start drawing again (a zoom going sharp), so all of it is asked again until it holds at once, within the one limit.
    fn settled(&self, limit: Duration) -> Result<Value, AgentError> {
        let started = Instant::now();
        let left = || limit.saturating_sub(started.elapsed());
        let mut animations_waited = Duration::ZERO;
        // Before anything, what's shown has to have been made from the state, which may be read late and taken in by making the window again (CI, 2026-11-11): a report before then is of a window that's about to go.
        while !self
            .inner()
            .is_none_or(|inner| inner.restored.load(Ordering::SeqCst) && inner.shown_with_state.load(Ordering::SeqCst))
        {
            if left().is_zero() {
                return Err(AgentError::new("timed_out", "The state hasn't been read and shown yet.", exit::TIMED_OUT)
                    .then("A slow disk holds it up: `recto qa state` says what the window shows meanwhile."));
            }
            std::thread::sleep(ANIMATION_POLL.min(left()));
        }
        loop {
            // What's been asked of the session thread comes first: an `open` it hasn't reached yet isn't settled, it's not started.
            if !(self.core.flush(left()) && self.engines().iter().all(|engine| engine.wait_settled(left()))) {
                return Err(AgentError::new("timed_out", "The page view didn't settle in time.", exit::TIMED_OUT)
                    .then("`recto qa state` shows what it's still waiting for."));
            }
            let running = self.ask("animations", "")?;
            // A snapshot on its way to the clipboard is about to change what `qa copied` and the notice say.
            // A snapshot on its way to the clipboard, or a print's pages being drawn or handed to the system, is about to change what's shown.
            let snapshot_busy = self.inner().is_some_and(|inner| inner.tool.busy() || inner.printing.busy());
            if running.as_array().is_none_or(Vec::is_empty) && !snapshot_busy {
                return Ok(
                    json!({ "settled": true, "waited_ms": started.elapsed().as_millis() as u64, "animations_waited_ms": animations_waited.as_millis() as u64 }),
                );
            }
            if left().is_zero() {
                return Err(
                    AgentError::new("timed_out", format!("Something in the window was still moving: {running}."), exit::TIMED_OUT)
                        .then("`recto qa state` shows what's running under `app.animations`."),
                );
            }
            let pause = ANIMATION_POLL.min(left());
            std::thread::sleep(pause);
            animations_waited += pause;
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
                let (worker, _, _) = self.workers_footprint();
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
        let mut cpu: Vec<u32> = recorded.iter().map(|step| step.cpu_micros).collect();
        cpu.sort_unstable();
        // The slowest steps, by how much of each was on a processor: a step that waited is a lock or the Mac being busy, and one that worked is this code.
        let mut slowest: Vec<(u32, u32)> = recorded.iter().map(|step| (step.micros, step.cpu_micros)).collect();
        slowest.sort_unstable_by(|one, two| two.cmp(one));
        slowest.truncate(5);
        // What the contents did, step by step, while the page scrolled.
        let began = recorded.first().map_or_else(Instant::now, |step| step.at);
        let positions: Vec<(f64, f64)> = recorded
            .iter()
            .map(|step| (step.at.duration_since(began).as_secs_f64(), step.contents_y))
            .collect();
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
            "step_cpu_us": { "p50": cpu.get(cpu.len() / 2).copied().unwrap_or(0), "p99": cpu.get(((cpu.len() as f64 - 1.0) * 0.99).round() as usize).copied().unwrap_or(0), "worst": cpu.last().copied().unwrap_or(0) },
            "slowest_steps_us_and_cpu_us": slowest,
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
    AgentError::new("ui_not_answering", "The app's UI thread didn't answer in 30 s.", exit::TIMED_OUT)
        .then("That's a hang, and a bug. `recto qa state` may show it once it's back.")
}

/// Where a QA mouse verb goes: `PAGE:X,Y`, with `:CLICKS` and `:shift` after it for a press. The page is as `recto` counts them.
#[derive(Debug, PartialEq)]
struct PagePlace {
    page: u32,
    x: f64,
    y: f64,
    clicks: u32,
    shift: bool,
}

fn parse_page_place(value: &str) -> Result<PagePlace, AgentError> {
    let wrong = || {
        bad(format!(
            "`{}` isn't a place. It's `PAGE:X,Y`, with `:CLICKS` and `:shift` after it for a press, such as `3:120,88:2`.",
            cleaned(value)
        ))
    };
    let mut parts = value.split(':');
    let page = parts
        .next()
        .and_then(|word| word.parse::<u32>().ok())
        .and_then(|page| page.checked_sub(1))
        .ok_or_else(wrong)?;
    let (x, y) = parts.next().and_then(|word| word.split_once(',')).ok_or_else(wrong)?;
    let (x, y) = (x.parse::<f64>().map_err(|_| wrong())?, y.parse::<f64>().map_err(|_| wrong())?);
    if !x.is_finite() || !y.is_finite() {
        return Err(wrong());
    }
    let (mut clicks, mut shift) = (1, false);
    for part in parts {
        match part {
            "shift" => shift = true,
            count => clicks = count.parse::<u32>().ok().filter(|count| (1..=4).contains(count)).ok_or_else(wrong)?,
        }
    }
    Ok(PagePlace { page, x, y, clicks, shift })
}

/// What each QA verb takes. Here, so the host is only ever told what it can do, and has no rules of its own about it.
/// Whether a file is one a quiet run's test made: in its state folder, or a temp folder. Compared by the folders the paths really are, so a link out of one doesn't count.
fn is_a_tests_file(path: &std::path::Path, state: Option<&std::path::Path>) -> bool {
    let Ok(file) = path.canonicalize() else { return false };
    let roots = [
        state.map(std::path::Path::to_path_buf),
        Some(std::env::temp_dir()),
        Some(std::path::PathBuf::from("/private/tmp")),
    ];
    roots
        .into_iter()
        .flatten()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| file.starts_with(root))
}

fn check_drive_value(verb: &str, value: &str) -> Result<(), AgentError> {
    let one_of = |words: &[&str]| words.contains(&value);
    let row = || value.parse::<u32>().is_ok();
    let fits = match verb {
        "appearance" => one_of(&["light", "dark"]),
        "window" => one_of(&["open", "close"]),
        "contents_pointer" => one_of(&["in", "out"]),
        "corner_pointer" => one_of(&[
            "top-left:in",
            "top-left:out",
            "top-right:in",
            "top-right:out",
            "lower-right:in",
            "lower-right:out",
        ]),
        "reduce_motion" => one_of(&["on", "off"]),
        "accessibility" => crate::sidebar_access::is_qa_value(value),
        "scroller_style" => one_of(&["legacy", "overlay"]),
        "full_screen" | "minimise" | "hide_app" | "frontmost" => one_of(&["on", "off"]),
        "focus" => one_of(&["page", "sidebar", "find"]),
        "sheet" => one_of(&["open", "closed"]),
        "sidebar_width" => value.parse::<f64>().is_ok_and(f64::is_finite),
        // What shows a window is left out: a quiet run shows none.
        // Save As's sheet is a sheet on the window, as the close prompt is, so a quiet run can show it and answer it.
        "menu" => {
            !matches!(value, "file.open" | "app.about" | "app.quit") && (crate::menus::action(value).is_some() || crate::snapshot_tool::is_menu_id(value))
        }
        "panel" => {
            value == "toggle"
                || value == "edit_mode.done"
                || value.starts_with("paper.")
                || value.starts_with("view.")
                || value.starts_with("rotate.")
                || value.starts_with("tools.")
        }
        "button" => one_of(&["sidebar", "zoom", "zoom_reset", "save", "page"]),
        "sheet_answer" => one_of(&["first", "second", "third", "cancel"]),
        "key_action" => one_of(&["undo", "redo"]),
        "context_menu" => crate::menus::context_menu(None, Some(false))
            .iter()
            .any(|item| !item.id.is_empty() && item.id == value),
        // What a person can type in a short field, and no more. Whether it's a level is the field's to find out, as it is for a person.
        "zoom_type" => value.chars().count() <= 32,
        // A test's password, typed as a person would: the app takes it once and keeps it only for the open tab.
        "password" => !value.is_empty() && value.len() <= recto_core::MAX_PASSWORD_BYTES,
        "page_type" => value.len() <= crate::linking::MAX_TYPED_PAGE,
        "swipe" => value.parse::<f64>().is_ok_and(|delta| delta.is_finite() && delta.abs() <= 100.0),
        "hover" | "reveal" => value == "none" || row(),
        "click_row" => value == "note" || value.strip_prefix("fold:").is_some_and(|row| row.parse::<u32>().is_ok()) || row(),
        "contents_scroll" | "compare_click" | "find_click" => row(),
        "palette_type" => value.chars().count() <= 256,
        "palette_key" => one_of(&["up", "down", "return", "escape"]),
        "find_type" => value.len() <= recto_session::record::MAX_FIND_BYTES,
        "find_key" => one_of(&["return", "shift_return", "escape"]),
        "pane_context_menu" => value.split_once(':').is_some_and(|(pane, item)| {
            pane.parse::<usize>().is_ok_and(|pane| pane < recto_session::compare::MAX_PANES)
                && crate::menus::context_menu(None, Some(false))
                    .iter()
                    .any(|entry| !entry.id.is_empty() && entry.id == item)
        }),
        "pane_focus" => value.parse::<usize>().is_ok_and(|pane| pane < recto_session::compare::MAX_PANES),
        "pane_scroll" => value.split_once(':').is_some_and(|(pane, points)| {
            pane.parse::<usize>().is_ok_and(|pane| pane < recto_session::compare::MAX_PANES)
                && points.parse::<f64>().is_ok_and(|points| points.is_finite() && points.abs() <= 1e9)
        }),
        "resize" => value
            .split_once('x')
            .and_then(|(width, height)| Some((width.parse::<f64>().ok()?, height.parse::<f64>().ok()?)))
            .is_some_and(|(width, height)| (400.0..=8000.0).contains(&width) && (300.0..=8000.0).contains(&height)),
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
        return Err(AgentError::new(
            "path_refused",
            "A shot goes outside your home folder, somewhere temporary.",
            exit::FAILED,
        ));
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

    /// Failure: a QA click lands somewhere other than what it says, or a page counted from 0 slips through, or a NaN reaches the host.
    #[test]
    fn a_mouse_verb_says_a_place_or_is_refused() {
        assert_eq!(
            parse_page_place("3:120.5,88").unwrap(),
            PagePlace {
                page: 2,
                x: 120.5,
                y: 88.0,
                clicks: 1,
                shift: false
            }
        );
        assert_eq!(
            parse_page_place("1:0,0:2:shift").unwrap(),
            PagePlace {
                page: 0,
                x: 0.0,
                y: 0.0,
                clicks: 2,
                shift: true
            }
        );
        for wrong in ["", "3", "0:1,1", "3:1", "3:x,1", "3:NaN,1", "3:1,inf", "3:1,1:0", "3:1,1:9", "3:1,1:zzz"] {
            assert_eq!(parse_page_place(wrong).unwrap_err().code, "usage", "{wrong}");
        }
    }

    /// Failure: a word the host can't do reports success: `qa appearance zzz` said it was done and went light, since the rule was the host's. They're checked here and every one is refused.
    #[test]
    fn a_verb_only_takes_what_it_can_do() {
        for (verb, good, wrong) in [
            ("appearance", "dark", "zzz"),
            ("window", "close", "zzz"),
            ("contents_pointer", "in", "zzz"),
            ("reduce_motion", "on", "yes"),
            ("hover", "none", "-1"),
            ("reveal", "3", "x"),
            ("click_row", "0", "none"),
            ("contents_scroll", "9", "1.5"),
            ("resize", "800x600", "10x10"),
        ] {
            assert!(check_drive_value(verb, good).is_ok(), "{verb} {good}");
            assert_eq!(check_drive_value(verb, wrong).unwrap_err().code, "usage", "{verb} {wrong}");
        }
        assert!(check_drive_value("resize", "800").is_err());
    }
}
