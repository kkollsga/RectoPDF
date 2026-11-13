//! QA commands, on their way to the UI thread. They run on a job thread of the server, so waiting here holds up nobody. A QA command calls what the UI calls: the host's answer says what happened.

mod as_person;
mod levers;
mod values;

use super::{CoreQueue, Inner, lock};
use crate::watchdog::{FAIL_MS, NOTE_MS, Watchdog};
use recto_agent::commands::Cmd;
use recto_agent::commands::Qa;
use recto_agent::commands::qa::{QaDrive, QaRead, verbs};
use recto_agent::{AgentError, exit};
use recto_control::state::is_in_real_home;
use recto_reader::Engine;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};
use values::{bad, check_drive_value, cleaned, parse_page_place, refuse_shot_path};

/// How long a quiet run lasts at most, if it isn't told.
pub const QUIET_RUN_LIMIT: Duration = Duration::from_secs(600);

/// What a test's slow mode has done in this run, for `qa state`: each seam's delay, how often it's been waited at and for how long in all. Null when it's off.
fn slow_report() -> Value {
    recto_platform::slow::report().map_or(Value::Null, |report| {
        let seams: serde_json::Map<String, Value> = report
            .seams
            .iter()
            .map(|seam| (seam.seam.to_owned(), json!({ "ms": seam.ms, "times": seam.times, "waited_ms": seam.waited_ms })))
            .collect();
        json!({ "seams": seams, "jitter": report.jitter, "ui_refusals": report.ui_refusals })
    })
}

/// A test's outside change to a quiet run's activation policy: how often the run turns itself `accessory`, as something outside the app once did (a full check, 2026-11-11, and `open -g` of a copy with the same identifier, twice in about 145 reports), from `RECTO_QA_POLICY_NUDGE_MS`. Nothing outside a quiet run, and never in a release build.
///
/// `<ms>,in-put-back` also lands one change in the guard's first put-back of each, between its set and its read, as Launch Services' change from an `open` did (three quiet runs side by side, 2026-11-11: a set from `accessory` said it took, and the read after it said `regular`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nudge {
    pub every: Duration,
    pub in_put_back: bool,
}

pub fn policy_nudge(quiet: bool) -> Option<Nudge> {
    nudge_for(recto_platform::RELEASE_BUILD, quiet, || std::env::var("RECTO_QA_POLICY_NUDGE_MS").ok())
}

/// [`policy_nudge`] for a build that is or isn't a release, with the switch read only if it's needed. A release never takes it, whatever its
/// environment says, as the slow mode doesn't (`recto_platform::slow::arm`): `RELEASE_BUILD` is compiled in.
fn nudge_for(release: bool, quiet: bool, switch: impl FnOnce() -> Option<String>) -> Option<Nudge> {
    if release || !quiet {
        return None;
    }
    let switch = switch()?;
    let (ms, rest) = switch.split_once(',').map_or((switch.as_str(), None), |(ms, rest)| (ms, Some(rest)));
    let ms: u64 = ms.trim().parse().ok()?;
    let in_put_back = match rest.map(str::trim) {
        None => false,
        Some("in-put-back") => true,
        Some(_) => return None,
    };
    Some(Nudge {
        every: Duration::from_millis(ms.clamp(5, 10_000)),
        in_put_back,
    })
}

/// The process this quiet run ends with, if it was told one: what a relaunch by an update passes on.
static OWNER: OnceLock<u32> = OnceLock::new();

/// What a quiet run's relaunch is given, so the new run ends with the same test.
pub fn owner_args() -> Vec<String> {
    OWNER.get().map(|pid| vec!["--qa-owner-pid".to_owned(), pid.to_string()]).unwrap_or_default()
}

/// **A quiet run ends with what started it** (PLAN: three ran on, orphaned, for up to their limit after their worktrees were gone). One thread, asleep in one `kevent` with no wake-ups, until the soonest of: the test or script it was started for ends (`--qa-owner-pid`, from `RECTO_QA_OWNER_PID` or `recto qa start --owner`), its temp state folder goes, its own program is deleted, as a worktree's removal or a rebuild does, or its time limit is up. A run started by hand with no owner, as an agent does one command at a time, lasts until one of the others. Its copy forgets itself with Launch Services before it exits, as a quit does, or it stays a second RectoPDF in Open With (CI run 43195226598: two runs of the built app ended so by `a_quiet_run_ends_with_what_started_it` left it registered).
pub fn end_with_starter(limit: Duration, owner: Option<u32>, state: Option<std::path::PathBuf>, ending: Weak<Inner>) {
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
        if let Some(inner) = ending.upgrade() {
            inner.forget_a_quiet_copy();
        }
        std::process::exit(124);
    });
}

fn updates_takes_input(inner: &Inner) -> bool {
    inner.life.takes_input()
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

pub struct QaBridge {
    listener: Arc<dyn crate::Listener>,
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

/// Clears the verb the watchdog notes, as the ask that set it returns by any way.
struct ClearVerb<'a>(&'a Watchdog);

impl Drop for ClearVerb<'_> {
    fn drop(&mut self) {
        self.0.note_verb(None);
    }
}

impl QaBridge {
    pub fn new(listener: Arc<dyn crate::Listener>, engine: Engine, watchdog: Watchdog, core: Arc<CoreQueue>) -> Self {
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

    pub fn attach(&self, inner: Weak<Inner>) {
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

    pub fn answer(&self, id: u64, ok: bool, json: String) {
        let mut answers = lock(&self.answers);
        if let Some(slot) = answers.get_mut(&id) {
            *slot = Some(if ok { Ok(json) } else { Err(json) });
            self.answered.notify_all();
        }
    }

    pub fn note_step(&self, micros: u64, cpu_micros: u64, contents_y: f64, current_visible: bool) {
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
        // Only what the table says the host answers goes to it: a verb it doesn't would come back as no QA command from the host, after a wait.
        if !verbs::host_answers(verb) {
            return Err(bad("That isn't a QA command the window answers."));
        }
        // What acts on the window waits for it to take input, as a person's click would have to: a window up without the state is made again from it, and what was done to it would go (CI, 2026-11-11). Looking doesn't wait.
        if !matches!(verb, "state" | "animations" | "copied") {
            let began = Instant::now();
            while !self.inner().is_none_or(|inner| updates_takes_input(&inner)) && began.elapsed() < UI_ANSWER_WAIT {
                std::thread::sleep(ANIMATION_POLL);
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.answers).insert(id, None);
        // The hang watchdog records which verb a stall was in; cleared when this returns.
        self.watchdog.note_verb(Some(verb));
        let _noted = ClearVerb(&self.watchdog);
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

    pub fn run(&self, command: &Qa) -> Result<Value, AgentError> {
        match command {
            Qa::QaRead(QaRead { verb, out, timeout_ms }) => match verb.as_str() {
                "state" => self.state(),
                "settled" => self.settled(timeout_ms.map_or(DEFAULT_SETTLE_WAIT, Duration::from_millis)),
                "shot" | "shot_settings" | "shot_menu" | "shot_menus" => self.shot(verb, out.as_deref().ok_or_else(|| bad("A shot needs `--out`."))?),
                "copied" => self.ask("copied", ""),
                _ => Err(bad("That isn't a QA command.")),
            },
            Qa::QaDrive(QaDrive { verb, value, timeout_ms }) => self.drive(verb, value, *timeout_ms),
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
        // The table says what a client may drive: a verb with a `qa_drive` row. Nothing else gets further, here or at the host.
        if verbs::row_of(verb).is_none_or(|row| row.tag != QaDrive::TAG) {
            return Err(bad("That isn't a QA command."));
        }
        refused_in_a_release(recto_platform::RELEASE_BUILD, verb)?;
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
            "handoff" => return self.handoff(),
            // A person's command, as the window sends it, for what only a person may do. Checked there: its level is its row's, Full.
            // What Save As's sheet sends when it's answered with this path, the window's own answer: the system's sheet can't be typed into.
            "save_as" => {
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                if !value.starts_with('/') {
                    return Err(bad("It's the whole path of a new file."));
                }
                let tab = lock(&inner.published).reader.shown_tab;
                if tab == 0 {
                    return Err(AgentError::new("no_active_tab", "No tab is open.", exit::NOT_FOUND));
                }
                as_person::check_tests_files(&inner, &[Path::new(value)])?;
                inner.act(crate::core::controls::UiAction::SaveAs { tab, path: value.to_owned() });
                self.core.flush(ANSWER_AFTER_CLICK);
                return self.ask("state", "");
            }
            "as_person" | "owner_edit" => {
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                let words = if verb == "owner_edit" {
                    as_person::owner_edit_words(value)?
                } else {
                    value.to_owned()
                };
                as_person::run(&inner, &words)?;
                self.core.flush(ANSWER_AFTER_CLICK);
                return self.ask("state", "");
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
                if !matches!(value, "open" | "close" | "privacy") && !inner.has_setting(value) {
                    return Err(bad(format!("`{}` isn't something `settings` takes.", cleaned(value))));
                }
                self.ask(verb, value)?;
                self.core.flush(ANSWER_AFTER_CLICK);
                return self.ask("state", "");
            }
            "appearance" | "window" | "hover" | "click_row" | "compare_click" | "pane_focus" | "pane_scroll" | "pane_context_menu" | "resize"
            | "contents_scroll" | "contents_pointer" | "corner_pointer" | "reduce_motion" | "reveal" | "scroller_style" | "menu" | "panel" | "button"
            | "zoom_type" | "page_type" | "find_type" | "find_key" | "find_click" | "swipe" | "context_menu" | "key_action" | "full_screen" | "minimise"
            | "hide_app" | "focus" | "frontmost" | "sheet" | "sheet_answer" | "sidebar_width" | "accessibility" => check_drive_value(verb, value)?,
            "palette_type" | "palette_key" => {
                check_drive_value(verb, value)?;
                if !self.inner().is_some_and(|inner| inner.tools.palette.is_open()) {
                    return Err(bad("The palette isn't open. `recto qa menu palette.open` opens it."));
                }
            }
            // A test's password, for a test's own file: the sheet showing must be asking about a file in the quiet run's state folder or a temp folder, so this can't type into a sheet for one of the person's own (review O6).
            "password" => {
                check_drive_value(verb, value)?;
                let inner = self.inner().ok_or_else(|| bad("The app is going away."))?;
                password_sheet_is_a_tests(&inner)?;
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
                // A file in a compound value the table can't see as one, so it's put through the session's one rule here (2026-11-13).
                if let Some(folders) = self
                    .inner()
                    .and_then(|inner| inner.host.session.with_session(|session| session.confinement().folders().cloned()))
                {
                    recto_agent::quiet_files::check_paths(&folders, &[out.to_path_buf()])?;
                }
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

    /// `qa handoff`: what an update does with the state before it quits, measured, then the quit, after the answer has had time to get out.
    fn handoff(&self) -> Result<Value, AgentError> {
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
        Ok(
            json!({ "handed_off": true, "tabs": report.tabs, "edited_tabs": report.edited_tabs, "record_bytes": report.record_bytes, "journal_bytes": report.journal_bytes, "wrote_bytes": report.wrote_bytes, "total_us": report.total_us, "force_us": report.force_us }),
        )
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
            "hangs": { "over_1s": over_note, "over_2s": over_fail, "worst_ms": worst, "note_ms": NOTE_MS, "fail_ms": FAIL_MS, "unbracketed_blocks": self.watchdog.unbracketed(), "worst": self.watchdog.worst_stall().map_or(Value::Null, |stall| json!({ "wall_ms": stall.wall_ms, "cpu_ms": stall.cpu_ms, "window_ms": stall.window_ms, "state": stall.state, "waiting_looks": stall.waiting_looks, "running_looks": stall.running_looks, "verb": stall.verb, "ended": stall.ended })) },
            "printing": self.inner().map_or(Value::Null, |inner| json!({ "busy": inner.tools.printing.busy(), "notice": inner.tools.printing.notice(), "pid": std::process::id() })),
            "snapshot": self.inner().map_or(Value::Null, |inner| json!({ "on": inner.tools.tool.is_on(), "drawing": inner.tools.tool.is_drawing(), "busy": inner.tools.tool.busy(), "notice": inner.tools.tool.notice().map(|notice| notice.text) })),
            "slow": slow_report(),
        }))
    }

    /// Settled is the session thread done with what it was asked, every page view drawn, and no host animation running that changes what's shown: the sidebar contents' fade and slide, their drift, a name about to be shown, a zoom about to go sharp. Each of those ends by itself, and one ending can start drawing again (a zoom going sharp), so all of it is asked again until it holds at once, within the one limit.
    fn settled(&self, limit: Duration) -> Result<Value, AgentError> {
        let started = Instant::now();
        let left = || limit.saturating_sub(started.elapsed());
        let mut animations_waited = Duration::ZERO;
        // Before anything, what's shown has to have been made from the state, which may be read late and taken in by making the window again (CI, 2026-11-11): a report before then is of a window that's about to go.
        while !self.inner().is_none_or(|inner| inner.life.state_read() && inner.life.shown_with_state()) {
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
            let snapshot_busy = self.inner().is_some_and(|inner| inner.tools.tool.busy() || inner.tools.printing.busy());
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

/// **`qa password` types only into a test's own sheet:** the sheet showing must be asking about a file in the quiet run's state folder or a temp folder, so
/// it can't type into one for the person's own (review O6).
pub(crate) fn password_sheet_is_a_tests(inner: &Inner) -> Result<(), AgentError> {
    let path = lock(&inner.tools.prompts.passwords)
        .showing()
        .map(|ask| ask.path.clone())
        .ok_or_else(|| bad("No password sheet is open."))?;
    as_person::check_tests_files(inner, &[path.as_path()])
}

/// **What acts as the person is a test's alone, never a release's** (decided in R2.4's review, D1): `qa as-person`, its `owner-edit` alias and `qa
/// save-as` are refused in a release build, as a test's update clock is, by the build's own flag. `app/release-zip.sh` checks it on the signed app.
fn refused_in_a_release(release: bool, verb: &str) -> Result<(), AgentError> {
    if release && matches!(verb, "as_person" | "owner_edit" | "save_as") {
        return Err(AgentError::new(
            "qa_refused",
            "A release build doesn't act as the person for a test: this QA verb is only in a development build.",
            exit::FAILED,
        ));
    }
    Ok(())
}

fn stuck() -> AgentError {
    AgentError::new("ui_not_answering", "The app's UI thread didn't answer in 30 s.", exit::TIMED_OUT)
        .then("That's a hang, and a bug. `recto qa state` may show it once it's back.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::headless::{HeadlessApp, outside_the_tests_folders};

    /// A QA bridge on a headless app, for what's refused before the host is asked: no host answers it, so a verb that got as far as the host would wait
    /// out [`UI_ANSWER_WAIT`] and fail as stuck.
    fn bridge(app: &HeadlessApp) -> QaBridge {
        let inner = app.core();
        let bridge = QaBridge::new(
            Arc::new(crate::testkit::FakeListener::default()),
            inner.engine.clone(),
            Watchdog::new(),
            Arc::clone(&inner.core),
        );
        bridge.attach(Arc::downgrade(inner));
        bridge
    }

    fn drive(bridge: &QaBridge, verb: &str, value: &str) -> Result<Value, AgentError> {
        bridge.run(&Qa::QaDrive(QaDrive {
            verb: verb.to_owned(),
            value: value.to_owned(),
            timeout_ms: None,
        }))
    }

    /// Failure (review of R2.4, S2): `qa save-as` acts as the person, at Control and with no file wall, where the same through `qa as-person` needs Full
    /// and the wall: a save to a path outside the test's folders, or with a file that isn't a test's open, goes through. Through the verb, as a client
    /// sends it.
    #[test]
    fn save_as_is_at_full_behind_the_wall() {
        let line = r#"{"cmd":"qa_drive","verb":"save_as","value":"/tmp/x.pdf"}"#;
        assert_eq!(recto_agent::parse(line).unwrap().access(), recto_agent::Access::At(recto_agent::Level::Full));
        let app = HeadlessApp::start();
        let qa = bridge(&app);
        let own = app.disk().pdf("own.pdf", 2);
        app.open(&own);
        let Some(outside) = outside_the_tests_folders("recto-save-as.pdf") else {
            return;
        };
        let refused = drive(&qa, "save_as", &outside.to_string_lossy()).expect_err("a save to a path outside");
        assert!(refused.message.contains("a test's own file"), "{refused:?}");
        // A file that isn't a test's doesn't open in a quiet run at all (the session's wall, 2026-11-13), so "with one open" is a second line nothing
        // reaches now.
        std::fs::copy(&own, &outside).unwrap();
        app.open(&outside);
        assert_eq!(app.tabs().len(), 1, "a file that isn't a test's opened");
        let _ = std::fs::remove_file(&outside);
    }

    /// **The arguments whose value carries a file the table can't see as one**, each put through the one rule by what reads it: `qa print-to-pdf`'s by
    /// its verb, `qa compare-budget`'s list by `recto_control::quiet::check_here`. The guard below fails an argument that names a whole path and is neither
    /// a file to the table nor here.
    const FILE_IN_A_VALUE: &[(&str, &str)] = &[("qa compare-budget", "files"), ("qa print-to-pdf", "value")];

    /// Failure (2026-11-13): a QA verb's value names a file the table declares as text, so the session's wall never sees it, and a quiet run writes a
    /// print, or opens a file, outside its folders. Every argument whose help names a whole path is a file to the table or on [`FILE_IN_A_VALUE`], and each
    /// of those is refused outside, through the verb as a client sends it, writing nothing.
    #[test]
    fn every_file_in_a_qa_value_is_held_to_the_run() {
        use recto_agent::table::{Kind, TABLE};
        let named: Vec<(&str, &str)> = TABLE
            .iter()
            .flat_map(|spec| spec.args.iter().map(move |arg| (spec, arg)))
            .filter(|(_, arg)| arg.kind != Kind::Path && arg.help.contains("whole path"))
            .map(|(spec, arg)| (spec.name, arg.key))
            .collect();
        assert_eq!(named, FILE_IN_A_VALUE, "a whole path that isn't a file to the table");
        let app = HeadlessApp::start();
        let qa = bridge(&app);
        app.open(&app.disk().pdf("own.pdf", 2));
        let Some(outside) = outside_the_tests_folders("recto-print.pdf") else {
            return;
        };
        let refused = drive(&qa, "print_to_pdf", &outside.to_string_lossy()).expect_err("a print outside the run's folders");
        assert_eq!(refused.code, "outside_quiet_run", "{refused:?}");
        let refused = drive(&qa, "print_to_pdf", &format!("{}|1|1", outside.display())).expect_err("with a range");
        assert_eq!(refused.code, "outside_quiet_run", "{refused:?}");
        assert!(!outside.exists(), "a print was written outside");
        let spec = TABLE.iter().find(|spec| spec.name == "qa compare-budget").unwrap();
        let mut fields = serde_json::Map::new();
        let own = app.disk().pdf("other.pdf", 1);
        fields.insert("files".into(), json!(format!("{},{}", own.display(), outside.display())));
        let line = recto_agent::table::Invocation {
            spec,
            fields,
            json: true,
            help: false,
            rest: Vec::new(),
        };
        assert_eq!(
            recto_control::quiet::check_here(app.disk().paths(), &line).unwrap_err().code,
            "outside_quiet_run"
        );
    }

    /// Failure (review of R2.4, item 7): `qa password`, sent as a client sends it, types into the sheet for a file that isn't a test's: the check is a
    /// helper that the verb's own path doesn't call.
    #[test]
    fn qa_password_through_the_verb_types_only_into_a_tests_sheet() {
        let app = HeadlessApp::start();
        let qa = bridge(&app);
        let Some(outside) = outside_the_tests_folders("recto-password.pdf") else {
            return;
        };
        std::fs::write(&outside, crate::core::headless::files::locked_with("open sesame", 1)).unwrap();
        // It doesn't open in a quiet run at all (the session's wall, 2026-11-13), so there's no sheet for the verb to type into.
        app.open(&outside);
        assert!(
            app.reader().password_prompt.is_none() && app.tabs().is_empty(),
            "a file that isn't a test's opened"
        );
        assert!(drive(&qa, "password", "open sesame").is_err(), "typed with no sheet of a test's");
        let _ = std::fs::remove_file(&outside);
    }

    /// Failure (review of R2.4, D1): a release build acts as the person for whoever starts it with `--qa`. `as_person`, `owner_edit` and `save_as` are refused
    /// there, and nothing else is.
    #[test]
    fn a_release_doesnt_act_as_the_person() {
        for verb in ["as_person", "owner_edit", "save_as"] {
            assert_eq!(refused_in_a_release(true, verb).unwrap_err().code, "qa_refused", "{verb}");
            assert!(refused_in_a_release(false, verb).is_ok(), "{verb} in a development build");
        }
        assert!(refused_in_a_release(true, "state").is_ok());
        assert!(refused_in_a_release(true, "zoom").is_ok());
    }

    /// Failure: a release build takes a test's hook from its environment: `RECTO_QA_POLICY_NUDGE_MS` turned a release's quiet run `accessory` every
    /// few milliseconds, where the slow mode beside it is off in a release whatever the environment says. Not read at all there, and only in a quiet run.
    #[test]
    fn a_release_never_takes_the_policy_nudge() {
        let read = std::cell::Cell::new(false);
        let switch = || {
            read.set(true);
            Some("15".to_owned())
        };
        assert_eq!(nudge_for(true, true, switch), None, "a release's quiet run took the nudge");
        assert!(!read.get(), "a release read the switch");
        assert_eq!(
            nudge_for(false, false, || Some("15".to_owned())),
            None,
            "a launch that isn't quiet took the nudge"
        );
        assert_eq!(
            nudge_for(false, true, || Some("15".to_owned())).map(|nudge| (nudge.every, nudge.in_put_back)),
            Some((Duration::from_millis(15), false))
        );
        assert_eq!(
            nudge_for(false, true, || Some("15,in-put-back".to_owned())).map(|nudge| nudge.in_put_back),
            Some(true),
            "the change in the put-back"
        );
        assert_eq!(
            nudge_for(false, true, || Some("15,sometimes".to_owned())),
            None,
            "a switch it can't read is off"
        );
        assert_eq!(
            nudge_for(true, true, || Some("15,in-put-back".to_owned())),
            None,
            "a release took the change in the put-back"
        );
        assert_eq!(
            nudge_for(false, true, || Some("1".to_owned())).map(|nudge| nudge.every),
            Some(Duration::from_millis(5)),
            "held to its floor"
        );
        assert_eq!(nudge_for(false, true, || Some("soon".to_owned())), None);
    }

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
}
