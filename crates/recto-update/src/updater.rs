//! **The updater, as the app has it.** Small state and no threads at rest: the host sets one timer for when [`Updater::next_wake`] says, and everything else comes from a menu item, a command or that timer. A check or an install gets a thread for as long as it runs.
//!
//! Every word and rule's here, and the host only draws. What it needs of the app, it asks through [`Host`], which is all a test needs to stand in for.

use crate::fetch::{self, FetchError};
use crate::plan::{Files, Outcome, OutcomeKind, Plan, TestLaunch, beside_app, helper_alive, log, new_app_in, previous_app_in};
use crate::rules::{self, AutoLook, AutoStep, Latest, MAX_LATEST_BYTES, is_newer, parse_latest};
use crate::saved::{LastUpdate, Saved};
use crate::source::{Mode, Source};
use crate::stage::{self, Stage, StageError};
use crate::{APP_NAME, HELPER};
use recto_platform::local;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long a hand-off gets to write and read back the state.
const HAND_OFF_LIMIT: Duration = Duration::from_secs(5);
/// How often an install that's been asked for looks again at what it's waiting for.
const WAIT_STEP: Duration = Duration::from_millis(100);
/// How long an install that's been asked for will wait for something in flight, before saying it gave up.
const WAIT_LIMIT: Duration = Duration::from_secs(10 * 60);
/// How long the app gives itself to quit, before it says that didn't happen and carries on.
const QUIT_PATIENCE: Duration = Duration::from_secs(15);
/// What a check's thread gets: room for the parser's own limit of 128 levels on a hostile `latest.json`, at a debug build's frame sizes. It's only address space until it's used.
const CHECK_STACK: usize = 1024 * 1024;
/// Launches of a new version that don't reach their first frame, before going back to the kept one.
const UNSTARTED_LIMIT: u32 = 3;

/// What the app is to the updater.
pub trait Host: Send + Sync {
    /// What's in flight, in words for the user, and how long since they touched the app. Cheap.
    fn look(&self) -> Look;
    /// Writes and reads back the state, freezing the app. Blocks on the disk.
    fn hand_off(&self, limit: Duration) -> Result<(), HandOffRefused>;
    /// Quits, after a good hand-off.
    fn quit_now(&self) -> bool;
    /// The hand-off isn't going ahead: the app must take what the user does again.
    fn cancelled(&self);
    /// Something's changed that the host draws: the menu, the settings, the timer.
    fn changed(&self);
    /// Whether the app's the frontmost one.
    fn frontmost(&self) -> bool;
    /// What a test's run reopens with. Nothing, for a real one.
    fn relaunch(&self) -> TestLaunch;
    /// Starts the installer. A test stands in for this.
    fn start_installer(&self, helper: &Path, plan: &Path, log: &Path) -> std::io::Result<u32> {
        local::spawn_detached(helper, &["install".to_owned(), plan.display().to_string()], &[], log)
    }
}

/// How it stands for an install.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub busy: Vec<String>,
    pub since_input_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandOffRefused {
    Busy(Vec<String>),
    NotSaved(String),
}

/// What's known about this copy, from the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub version: String,
    pub app: PathBuf,
    pub system: String,
}

/// Who asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// The user or an agent, who wants to hear how it went.
    ByHand,
    /// The timer. Failures stay out of the way.
    ByItself,
}

/// How things stand. What the menu, the status line and `recto update status` say comes from this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    NoReleases,
    Available(String),
    /// Found, and this build won't replace itself.
    AvailableElsewhere(String),
    CheckFailed(String),
    Preparing(String),
    Waiting { version: String, for_what: String },
    Installing(String),
    Failed { version: String, why: String },
    Updated(String),
    WentBack { version: String, back_on: String },
}

impl Status {
    /// Its name, for an agent. These don't change.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Checking => "checking",
            Self::UpToDate => "up_to_date",
            Self::NoReleases => "no_releases",
            Self::Available(_) => "available",
            Self::AvailableElsewhere(_) => "available_not_installed",
            Self::CheckFailed(_) => "check_failed",
            Self::Preparing(_) => "preparing",
            Self::Waiting { .. } => "waiting",
            Self::Installing(_) => "installing",
            Self::Failed { .. } => "failed",
            Self::Updated(_) => "updated",
            Self::WentBack { .. } => "went_back",
        }
    }

    fn is_under_way(&self) -> bool {
        matches!(self, Self::Checking | Self::Preparing(_) | Self::Waiting { .. } | Self::Installing(_))
    }
}

/// A line for the reader, in the app's margin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub fades: bool,
}

/// One menu item, as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub title: String,
    pub enabled: bool,
}

/// What a command's asked to install came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallRefused {
    NotInThisBuild,
    NothingOnOffer,
    Underway,
}

struct St {
    status: Status,
    saved: Saved,
    launched_ms: u64,
    facts: Option<Facts>,
    /// What the user chose in Settings, or none: the build's default.
    switch: Option<bool>,
    notice: Option<Notice>,
    /// When an automatic install next looks. Set when one's on offer. Moves only then and when it's looked: nothing's done per input.
    install_look_at: Option<u64>,
    /// What this launch came from, if it's an update that's still to reach its first frame.
    arrived: Option<Plan>,
    first_frame_done: bool,
    /// The first frame came before what's remembered had been read, off a slow disk. It's dealt with as soon as it has.
    first_frame_early: Option<u64>,
}

pub struct Updater {
    files: Files,
    source: Source,
    host: Arc<dyn Host>,
    st: Mutex<St>,
    /// Held while `updates.json` is written, and never with `st`, so a slow disk can't hold up the UI thread asking about the menu.
    writing: Mutex<()>,
    /// A test's clock: how far ahead of the real one it's set. Nothing else sets this, and a release can't.
    clock_ahead_ms: AtomicI64,
    /// Stops an install thread waiting on something, when the app's going anyway.
    stopping: AtomicBool,
}

/// What became of the last install, as a launch finds it.
#[derive(Default)]
struct Settled {
    status: Option<Status>,
    notice: Option<Notice>,
    arrived: Option<Plan>,
    changed: bool,
    plan_gone: bool,
}

/// What a launch should do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtLaunch {
    GoOn,
    /// The installer's been started to go back. This process ends now.
    Exit,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// "2 hours ago", inside a sentence.
fn ago(at_ms: u64, now_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(at_ms) / 60_000;
    match minutes {
        0 => "just now".to_owned(),
        1 => "a minute ago".to_owned(),
        2..=59 => format!("{minutes} minutes ago"),
        60..=119 => "an hour ago".to_owned(),
        120..=1439 => format!("{} hours ago", minutes / 60),
        1440..=2879 => "yesterday".to_owned(),
        _ => format!("{} days ago", minutes / 1440),
    }
}

impl Updater {
    #[must_use]
    pub fn new(root: &Path, source: Source, host: Arc<dyn Host>) -> Arc<Self> {
        let st = St { status: Status::Idle, saved: Saved::default(), launched_ms: crate::wall_ms(), facts: None, switch: None, notice: None, install_look_at: None, arrived: None, first_frame_done: false, first_frame_early: None };
        Arc::new(Self { files: Files::in_state(root), source, host, st: Mutex::new(st), writing: Mutex::new(()), clock_ahead_ms: AtomicI64::new(0), stopping: AtomicBool::new(false) })
    }

    /// Now, by this updater's clock.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        crate::wall_ms().saturating_add_signed(self.clock_ahead_ms.load(Ordering::SeqCst))
    }

    /// Moves a test's clock on. Says whether it did: only a test run's clock moves.
    pub fn set_test_clock(&self, ahead_ms: i64) -> bool {
        if self.source.mode != Mode::Test {
            return false;
        }
        self.clock_ahead_ms.store(ahead_ms, Ordering::SeqCst);
        true
    }

    fn since_input(&self, look: &Look) -> u64 {
        look.since_input_ms.saturating_add_signed(self.clock_ahead_ms.load(Ordering::SeqCst))
    }

    #[must_use]
    pub fn source(&self) -> &Source {
        &self.source
    }

    fn log(&self, line: &str) {
        log(&self.files.log, line);
    }

    fn ready_folder(&self) {
        let _ = local::ensure_private_dir(&self.files.work);
    }

    /// Writes what's remembered, whatever it is by the time the last write's done, off the lock the UI thread asks through.
    fn save(&self) {
        let _writing = lock(&self.writing);
        let saved = lock(&self.st).saved.clone();
        saved.write(&self.files.saved);
    }

    /// Whether the switch is on, by what was chosen or by what this build starts with.
    #[must_use]
    pub fn switch_on(&self) -> bool {
        lock(&self.st).switch.unwrap_or_else(|| self.source.auto_default())
    }

    /// The switch's position, from the record or from Settings. An offer waiting is looked at again.
    pub fn set_switch(&self, chosen: Option<bool>) {
        let mut st = lock(&self.st);
        st.switch = chosen;
        let on = chosen.unwrap_or_else(|| self.source.auto_default());
        st.install_look_at = (on && self.offered_here(&st).is_some()).then(|| self.now_ms());
        drop(st);
        self.host.changed();
    }

    #[must_use]
    pub fn switch_choice(&self) -> Option<bool> {
        lock(&self.st).switch
    }

    fn offered_here(&self, st: &St) -> Option<String> {
        if !self.source.may_replace_itself() {
            return None;
        }
        let current = st.facts.as_ref()?.version.as_str();
        st.saved.offered.as_ref().filter(|offered| is_newer(offered, current)).cloned()
    }

    /// What the host knows of this copy. Given once, before anything else asks. Reads what's remembered, says what became of an install that's just ended, and tidies up. All of it files, so never on the UI thread, and none of it holding what the UI thread asks through.
    pub fn launched(&self, facts: Facts) -> AtLaunch {
        let now = self.now_ms();
        let mut saved = Saved::read(&self.files.saved);
        let current = facts.version.clone();
        let app = facts.app.clone();
        let settled = self.settle_last_install(&mut saved, &current, &app, now);
        let may_replace = self.source.may_replace_itself();
        let offered = saved.offered.clone().filter(|offered| is_newer(offered, &current));
        // What's staged and no longer on offer is the size of the app, for nothing. Only with no installer at work.
        if settled.plan_gone && !helper_alive(&self.files) {
            self.clear_leftovers(&app, offered.is_some());
        }
        let status = settled.status.unwrap_or_else(|| match &offered {
            Some(offered) if may_replace => Status::Available(offered.clone()),
            Some(offered) => Status::AvailableElsewhere(offered.clone()),
            None if saved.last_check_ms.is_some() => Status::UpToDate,
            None => Status::Idle,
        });
        // A new version counts towards going back until it's reached a first frame. Once it has, a launch writes nothing here.
        let mut going_back = false;
        let mut dirty = settled.changed;
        let beside = beside_app(&app).unwrap_or_default();
        if may_replace && saved.proven.as_deref() != Some(current.as_str()) && previous_app_in(&beside).is_dir() {
            saved.unstarted = saved.unstarted.saturating_add(1);
            dirty = true;
            going_back = saved.unstarted > UNSTARTED_LIMIT && !helper_alive(&self.files) && self.start_going_back(&mut saved, &current, &app, now);
        }
        let early = {
            let mut st = lock(&self.st);
            st.launched_ms = now;
            st.saved = saved;
            st.facts = Some(facts);
            st.status = status;
            st.notice = settled.notice;
            st.arrived = settled.arrived;
            if may_replace && offered.is_some() {
                st.install_look_at = Some(now.saturating_add(rules::MIN_GAP_MS));
            }
            st.first_frame_early.take()
        };
        if dirty {
            self.save();
        }
        if going_back {
            return AtLaunch::Exit;
        }
        if let Some(shown_at_ms) = early {
            self.first_frame(shown_at_ms);
        }
        self.host.changed();
        AtLaunch::GoOn
    }

    /// What a run that ended early left: a part-downloaded zip, a part-unpacked app, and a staged one that's no use now. Looks at a few names, and a normal launch finds none of them.
    fn clear_leftovers(&self, app: &Path, still_on_offer: bool) {
        if let Ok(entries) = std::fs::read_dir(&self.files.work) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with(".zip") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let Some(beside) = beside_app(app) else { return };
        let _ = std::fs::remove_dir_all(beside.join("unpacking"));
        if !still_on_offer || !beside.join("staged.json").exists() {
            stage::clear_staged(app);
        }
    }

    /// Whether this launch is an update arriving, so what comes back is exactly what was left, down to a window that was closed. Any other launch opens a window, as an app does.
    #[must_use]
    pub fn arrived_by_update(&self) -> bool {
        lock(&self.st).arrived.is_some()
    }

    /// What the last install left behind. The installer tidies up when it finishes: this is for when it didn't get to.
    fn settle_last_install(&self, saved: &mut Saved, current: &str, app: &Path, now: u64) -> Settled {
        let mut settled = Settled::default();
        let Some(beside) = beside_app(app) else { return settled };
        if let Some(outcome) = Outcome::read(&self.files.outcome()) {
            let _ = std::fs::remove_file(self.files.outcome());
            let _ = std::fs::remove_file(self.files.plan());
            let _ = std::fs::remove_file(self.files.helper_pid());
            let _ = std::fs::remove_dir_all(self.files.before());
            settled.changed = true;
            settled.plan_gone = true;
            match outcome.kind {
                OutcomeKind::WentBack => {
                    saved.note_failed_version(&outcome.to);
                    saved.install_failures = 0;
                    settled.status = Some(Status::WentBack { version: outcome.to.clone(), back_on: current.to_owned() });
                    settled.notice = Some(Notice { text: format!("{APP_NAME} {} didn't start, so {current} came back, just as you left it.", outcome.to), fades: true });
                    self.log(&format!("{} didn't start, and {current} came back", outcome.to));
                }
                OutcomeKind::NotInstalled => {
                    saved.install_failures = saved.install_failures.saturating_add(1);
                    saved.last_install_ms = Some(now);
                    settled.status = Some(Status::Failed { version: outcome.to.clone(), why: "it couldn't be put in place".to_owned() });
                    self.log(&format!("{} wasn't installed: {}", outcome.to, clean_code(&outcome.why)));
                }
            }
            return settled;
        }
        let Some(plan) = Plan::read(&self.files.plan()) else {
            settled.plan_gone = true;
            return settled;
        };
        settled.changed = true;
        let alive = helper_alive(&self.files);
        if plan.to == current {
            settled.status = Some(Status::Updated(plan.to.clone()));
            settled.notice = Some(Notice { text: format!("Updated to {}.", plan.to), fades: true });
            saved.offered = None;
            saved.install_failures = 0;
            saved.failures = 0;
            if !alive {
                self.tidy_after_swap(&plan, &beside);
            }
            settled.arrived = Some(plan);
        } else if alive {
            settled.status = Some(Status::Installing(plan.to.clone()));
        } else {
            // The installer was stopped before it swapped, so this is the old app, and there's no harm done.
            saved.install_failures = saved.install_failures.saturating_add(1);
            saved.last_install_ms = Some(now);
            settled.status = Some(Status::Failed { version: plan.to.clone(), why: "the installer stopped before it finished".to_owned() });
            self.log(&format!("the install of {} stopped before it finished, and {current} is unchanged", plan.to));
            let _ = std::fs::remove_file(self.files.plan());
            let _ = std::fs::remove_file(self.files.helper_pid());
            let _ = std::fs::remove_dir_all(self.files.before());
            stage::clear_staged(app);
        }
        settled
    }

    /// The new version's here and the installer isn't: moves the old app to where it's kept, if it stopped short of that, and clears up.
    fn tidy_after_swap(&self, plan: &Plan, beside: &Path) {
        let left = new_app_in(beside);
        if left.is_dir() && stage::app_version(&left).as_deref() == Some(plan.from.as_str()) {
            let keep = previous_app_in(beside);
            if let Some(folder) = keep.parent() {
                let _ = std::fs::remove_dir_all(folder);
                let _ = std::fs::create_dir_all(folder);
                let _ = std::fs::rename(&left, &keep);
            }
        }
        let _ = std::fs::remove_dir_all(beside.join("new"));
        let _ = std::fs::remove_file(beside.join("staged.json"));
        // The record's copy stays until this version's reached a first frame: it's what going back would put back.
        let _ = std::fs::remove_file(self.files.plan());
        let _ = std::fs::remove_file(self.files.helper_pid());
    }

    /// This version keeps failing before its first frame and an older one's kept: starts the installer to go back to it.
    fn start_going_back(&self, saved: &mut Saved, current: &str, app: &Path, now: u64) -> bool {
        let Some(beside) = beside_app(app) else { return false };
        let Some(previous) = stage::app_version(&previous_app_in(&beside)).filter(|previous| is_newer(current, previous)) else { return false };
        let plan = Plan { from: previous, to: current.to_owned(), pid: std::process::id(), app: app.to_path_buf(), beside, root: self.files.root.clone(), quit_at_ms: now, background: true, quit_limit_ms: crate::plan::QUIT_LIMIT_MS, start_limit_ms: crate::plan::START_LIMIT_MS, test: test_launch(&self.source, self.host.relaunch()) };
        self.ready_folder();
        if plan.write(&self.files.plan()).is_err() {
            return false;
        }
        saved.unstarted = 0;
        saved.write(&self.files.saved);
        self.log(&format!("{current} didn't reach a first frame {UNSTARTED_LIMIT} times, going back"));
        local::spawn_detached(&app.join(HELPER), &["back".to_owned(), self.files.plan().display().to_string()], &[], &self.files.log).is_ok()
    }

    /// The first correct frame's on the screen. The installer's waiting to hear this, and what it costs at a normal launch is nothing.
    pub fn first_frame(&self, shown_at_ms: u64) {
        let (version, arrived, newly_proven, changed) = {
            let mut st = lock(&self.st);
            if st.first_frame_done {
                return;
            }
            let Some(version) = st.facts.as_ref().map(|facts| facts.version.clone()) else {
                st.first_frame_early = Some(shown_at_ms);
                return;
            };
            st.first_frame_done = true;
            let mut changed = st.saved.unstarted != 0;
            st.saved.unstarted = 0;
            let newly_proven = st.saved.proven.as_deref() != Some(version.as_str()) && self.source.may_replace_itself();
            if newly_proven {
                st.saved.proven = Some(version.clone());
                changed = true;
            }
            let arrived = st.arrived.take();
            if let Some(plan) = &arrived {
                st.saved.last_update = Some(LastUpdate { from: plan.from.clone(), to: plan.to.clone(), gap_ms: shown_at_ms.saturating_sub(plan.quit_at_ms), at_ms: shown_at_ms });
                changed = true;
            }
            (version, arrived, newly_proven, changed)
        };
        // It's come up, so there's nothing the record's copy would be for. The installer clears its own if it's still around.
        if newly_proven && !helper_alive(&self.files) {
            let _ = std::fs::remove_dir_all(self.files.before());
        }
        if let Some(plan) = arrived {
            self.ready_folder();
            let _ = local::write_private_file(&self.files.launch_ok(), format!("{version} {} {shown_at_ms}", std::process::id()).as_bytes());
            self.log(&format!("updated from {} to {}: {} ms from quit to first frame", plan.from, plan.to, shown_at_ms.saturating_sub(plan.quit_at_ms)));
        }
        if changed {
            self.save();
        }
    }

    /// The menu item. Rust's words, whichever way things stand.
    #[must_use]
    pub fn menu_item(&self) -> MenuItem {
        let st = lock(&self.st);
        match &st.status {
            Status::Available(version) if self.source.may_replace_itself() => MenuItem { title: format!("Update to {version}…"), enabled: true },
            Status::Preparing(version) | Status::Waiting { version, .. } | Status::Installing(version) => MenuItem { title: format!("Updating to {version}…"), enabled: false },
            Status::Checking => MenuItem { title: "Check for Updates…".to_owned(), enabled: false },
            _ => MenuItem { title: "Check for Updates…".to_owned(), enabled: true },
        }
    }

    /// The menu item was chosen. Whichever it says: it starts on a thread of its own, and the UI's not held.
    pub fn menu_chosen(self: &Arc<Self>) {
        let available = matches!(lock(&self.st).status, Status::Available(_));
        if available {
            let _ = self.install(Asked::ByHand);
        } else {
            self.check_in_background(Asked::ByHand);
        }
    }

    /// When the host's timer should next go off, by this clock. One timer, for this and nothing else, and none if there's nothing to wake for. It changes only when something's happened, not with time or input.
    #[must_use]
    pub fn next_wake(&self) -> Option<u64> {
        let st = lock(&self.st);
        if st.facts.is_none() || st.status.is_under_way() {
            return None;
        }
        let check = self.source.checks_by_itself().then(|| rules::next_check_at(&st.saved, st.launched_ms, self.now_ms()));
        match (check, st.install_look_at) {
            (Some(check), Some(install)) => Some(check.min(install)),
            (check, install) => check.or(install),
        }
    }

    /// The timer's gone off. Does what's due and leaves the next time to [`Self::next_wake`]: a timer that goes off early or late does no harm.
    pub fn timer_fired(self: &Arc<Self>) {
        let now = self.now_ms();
        let (check_due, install_due) = {
            let st = lock(&self.st);
            if st.status.is_under_way() || st.facts.is_none() {
                return;
            }
            let check_due = self.source.checks_by_itself() && rules::next_check_at(&st.saved, st.launched_ms, now) <= now;
            (check_due, st.install_look_at.is_some_and(|at| at <= now))
        };
        if check_due {
            self.check_in_background(Asked::ByItself);
        } else if install_due {
            self.look_at_installing(now);
        }
        self.host.changed();
    }

    /// The 30-minute rule, now.
    fn look_at_installing(self: &Arc<Self>, now: u64) {
        let look = self.host.look();
        let since_input_ms = self.since_input(&look);
        let step = {
            let mut st = lock(&self.st);
            let offered = self.offered_here(&st);
            let on = st.switch.unwrap_or_else(|| self.source.auto_default());
            let step = rules::auto_step(AutoLook { build_may: self.source.may_replace_itself(), switch_on: on, offered: offered.is_some(), version_failed: offered.as_ref().is_some_and(|version| st.saved.failed.contains(version)), since_input_ms, busy: !look.busy.is_empty(), back_off_ms: st.saved.install_back_off(now) });
            st.install_look_at = match step {
                AutoStep::Never | AutoStep::Go => None,
                AutoStep::Wait(ms) => Some(now.saturating_add(ms)),
            };
            step
        };
        if step == AutoStep::Go {
            let _ = self.install(Asked::ByItself);
        }
    }

    /// A check, on a thread of its own.
    pub fn check_in_background(self: &Arc<Self>, asked: Asked) {
        if !self.begin_check() {
            return;
        }
        let me = Arc::clone(self);
        let started = std::thread::Builder::new().name("recto-update-check".into()).stack_size(CHECK_STACK).spawn(move || me.finish_check(asked));
        if started.is_err() {
            self.finish_check_with(asked, Err(FetchError::Failed("no thread could be started")));
        }
    }

    /// A check, here. For a command on its own job thread, which is off the UI thread already.
    pub fn check_here(self: &Arc<Self>, asked: Asked) -> Status {
        if self.begin_check() {
            self.finish_check(asked);
        }
        lock(&self.st).status.clone()
    }

    fn begin_check(&self) -> bool {
        let mut st = lock(&self.st);
        if st.status.is_under_way() || st.facts.is_none() {
            return false;
        }
        st.status = Status::Checking;
        drop(st);
        self.host.changed();
        true
    }

    fn fetch_latest(&self, version: &str) -> Result<Latest, FetchError> {
        if !self.source.network {
            return Err(FetchError::Failed("a test run reaches no update server"));
        }
        let user_agent = format!("{APP_NAME}/{version}");
        let body = fetch::get_small(&self.source, &self.source.latest_url(), &user_agent, MAX_LATEST_BYTES, fetch::CHECK_LIMIT)?;
        parse_latest(&body).ok_or(FetchError::Failed("the release's description can't be read"))
    }

    fn finish_check(self: &Arc<Self>, asked: Asked) {
        let version = lock(&self.st).facts.as_ref().map(|facts| facts.version.clone()).unwrap_or_default();
        let result = self.fetch_latest(&version);
        self.finish_check_with(asked, result);
    }

    fn finish_check_with(self: &Arc<Self>, asked: Asked, result: Result<Latest, FetchError>) {
        let now = self.now_ms();
        let mut st = lock(&self.st);
        let (current, system) = st.facts.as_ref().map(|facts| (facts.version.clone(), facts.system.clone())).unwrap_or_default();
        st.saved.last_attempt_ms = Some(now);
        let mut failure = None;
        let words = match result {
            Ok(latest) => {
                st.saved.last_check_ms = Some(now);
                st.saved.failures = 0;
                if latest.on_offer(&current, &system) {
                    st.saved.offered = Some(latest.version.clone());
                    if self.source.may_replace_itself() {
                        st.status = Status::Available(latest.version.clone());
                        // A newer one than last time has its own chance, whatever became of that.
                        st.install_look_at = Some(now);
                        format!("{APP_NAME} {0} is available. Choose Update to {0}… in the {APP_NAME} menu.", latest.version)
                    } else {
                        st.status = Status::AvailableElsewhere(latest.version.clone());
                        format!("{APP_NAME} {} is available. This is a development build, so it won't replace itself.", latest.version)
                    }
                } else {
                    st.saved.offered = None;
                    st.install_look_at = None;
                    st.status = Status::UpToDate;
                    format!("You're up to date: {APP_NAME} {current}.")
                }
            }
            Err(FetchError::NotFound) => {
                st.saved.last_check_ms = Some(now);
                st.saved.failures = 0;
                st.saved.offered = None;
                st.install_look_at = None;
                st.status = Status::NoReleases;
                format!("You're up to date: {APP_NAME} {current}. There are no releases yet.")
            }
            Err(error) => {
                st.saved.failures = st.saved.failures.saturating_add(1);
                let why = error.words();
                st.status = Status::CheckFailed(why.clone());
                failure = Some(why.clone());
                format!("Couldn't check for updates: {why}.")
            }
        };
        if asked == Asked::ByHand {
            st.notice = Some(Notice { text: words, fades: true });
        }
        drop(st);
        if let Some(why) = failure {
            self.log(&format!("a check failed: {why}"));
        }
        self.save();
        self.host.changed();
    }

    /// Starts an install, on a thread of its own. It fetches the description again, downloads, checks and unpacks, all with the app in use; then waits for what's in flight, hands off and quits.
    pub fn install(self: &Arc<Self>, asked: Asked) -> Result<String, InstallRefused> {
        if !self.source.may_replace_itself() {
            return Err(InstallRefused::NotInThisBuild);
        }
        self.stopping.store(false, Ordering::SeqCst);
        let version = {
            let mut st = lock(&self.st);
            if st.status.is_under_way() {
                return Err(InstallRefused::Underway);
            }
            let Some(version) = self.offered_here(&st) else { return Err(InstallRefused::NothingOnOffer) };
            st.status = Status::Preparing(version.clone());
            st.saved.last_install_ms = Some(self.now_ms());
            if asked == Asked::ByHand {
                st.notice = Some(Notice { text: format!("Downloading {APP_NAME} {version}…"), fades: false });
            }
            version
        };
        self.host.changed();
        let me = Arc::clone(self);
        let thread_version = version.clone();
        let started = std::thread::Builder::new().name("recto-update-install".into()).stack_size(CHECK_STACK).spawn(move || me.run_install(asked, &thread_version));
        if started.is_err() {
            self.failed(asked, &version, "no thread could be started", false);
        }
        Ok(version)
    }

    fn failed(&self, asked: Asked, version: &str, why: &str, lasting: bool) {
        let now = self.now_ms();
        let mut st = lock(&self.st);
        st.status = Status::Failed { version: version.to_owned(), why: why.to_owned() };
        st.saved.install_failures = st.saved.install_failures.saturating_add(1);
        st.saved.last_install_ms = Some(now);
        if lasting {
            st.saved.note_failed_version(version);
        }
        // A failure that may pass gets another chance after a while, by the back-off.
        st.install_look_at = (!lasting && self.offered_here(&st).is_some()).then(|| now.saturating_add(st.saved.install_back_off(now)));
        st.notice = (asked == Asked::ByHand).then(|| Notice { text: format!("Couldn't update to {version}: {why}. {APP_NAME} is unchanged."), fades: true });
        drop(st);
        self.save();
        self.log(&format!("the update to {version} failed: {why}"));
        self.host.changed();
    }

    fn back_on_offer(&self, version: &str, look_at: Option<u64>) {
        let mut st = lock(&self.st);
        st.status = Status::Available(version.to_owned());
        st.install_look_at = look_at;
        st.notice = None;
        drop(st);
        self.host.changed();
    }

    /// Whether an automatic install is still right to go ahead: the user may have come back during a download.
    fn still_idle(&self, look: &Look) -> bool {
        self.since_input(look) >= rules::IDLE_BEFORE_INSTALL_MS
    }

    fn run_install(self: &Arc<Self>, asked: Asked, version: &str) {
        let Some(facts) = lock(&self.st).facts.clone() else { return };
        self.ready_folder();
        self.log(&format!("updating to {version}"));
        // Read again, so one that's been withdrawn doesn't go in, and the checksum's today's.
        let latest = match self.fetch_latest(&facts.version) {
            Ok(latest) if latest.on_offer(&facts.version, &facts.system) => latest,
            Ok(_) | Err(FetchError::NotFound) => {
                let mut st = lock(&self.st);
                st.saved.offered = None;
                st.status = Status::UpToDate;
                st.install_look_at = None;
                st.notice = (asked == Asked::ByHand).then(|| Notice { text: format!("{version} isn't on offer any more. You're up to date: {APP_NAME} {}.", facts.version), fades: true });
                drop(st);
                self.save();
                self.host.changed();
                return;
            }
            Err(error) => {
                self.failed(asked, version, &format!("the release couldn't be read: {}", error.words()), false);
                return;
            }
        };
        let version = latest.version.as_str();
        let user_agent = format!("{APP_NAME}/{}", facts.version);
        let stage_in = Stage { source: &self.source, latest: &latest, current: &facts.version, app: &facts.app, files: &self.files, user_agent: &user_agent };
        let log_file = self.files.log.clone();
        let prepared = stage::prepare(&stage_in, &mut |step| log(&log_file, &format!("{version}: {step}")));
        if let Err(StageError { code, why, lasting }) = prepared {
            self.log(&format!("{version} was refused: {code}"));
            if lasting {
                stage::clear_staged(&facts.app);
            }
            self.failed(asked, version, &why, lasting);
            return;
        }
        self.wait_and_hand_off(asked, version, &facts);
    }

    /// The download's good. What's in flight finishes first, and the user's told if they're waiting on it. Then the hand-off and the quit: from here, what's written is what comes back.
    fn wait_and_hand_off(self: &Arc<Self>, asked: Asked, version: &str, facts: &Facts) {
        let began = Instant::now();
        loop {
            // The user's quitting by other means, and this doesn't turn that into a reopening.
            if self.stopping.load(Ordering::SeqCst) {
                self.back_on_offer(version, None);
                return;
            }
            let look = self.host.look();
            if asked == Asked::ByItself && !self.still_idle(&look) {
                // They've come back. It's all ready for the next quiet half hour.
                let wait = rules::IDLE_BEFORE_INSTALL_MS.saturating_sub(self.since_input(&look));
                self.back_on_offer(version, Some(self.now_ms().saturating_add(wait)));
                return;
            }
            if look.busy.is_empty() {
                match self.host.hand_off(HAND_OFF_LIMIT) {
                    Ok(()) => {
                        self.quit_after_hand_off(asked, version, facts);
                        return;
                    }
                    Err(HandOffRefused::NotSaved(why)) => {
                        // Nothing's been lost, and the app's taking everything again: the update just doesn't go ahead.
                        self.log(&format!("{version}: the state couldn't be written and read back"));
                        self.failed(asked, version, &format!("the app's state couldn't be saved first: {why}"), false);
                        return;
                    }
                    Err(HandOffRefused::Busy(_)) => {}
                }
            }
            if asked == Asked::ByItself {
                self.back_on_offer(version, Some(self.now_ms().saturating_add(rules::BUSY_RETRY_MS)));
                return;
            }
            let for_what = look.busy.first().cloned().unwrap_or_default();
            {
                let mut st = lock(&self.st);
                let changed = !matches!(&st.status, Status::Waiting { for_what: known, .. } if *known == for_what);
                if changed {
                    st.status = Status::Waiting { version: version.to_owned(), for_what: for_what.clone() };
                    st.notice = Some(Notice { text: format!("Updating to {version} as soon as this is done: {for_what}"), fades: false });
                    drop(st);
                    self.host.changed();
                }
            }
            if began.elapsed() >= WAIT_LIMIT {
                self.failed(asked, version, &format!("it waited too long: {for_what}"), false);
                return;
            }
            std::thread::sleep(WAIT_STEP);
        }
    }

    /// Past a good hand-off the app's frozen, so every way out from here has to free it.
    fn quit_after_hand_off(self: &Arc<Self>, asked: Asked, version: &str, facts: &Facts) {
        let give_up = |why: &str| {
            let _ = std::fs::remove_file(self.files.plan());
            self.host.cancelled();
            self.failed(asked, version, why, false);
        };
        // A quit that came in the meantime goes on as it was going, frozen and with nothing started. If it doesn't go through, it's that which takes everything again.
        if self.stopping.load(Ordering::SeqCst) {
            self.back_on_offer(version, None);
            return;
        }
        let look = self.host.look();
        // The user may have touched it in the moment before it froze.
        if asked == Asked::ByItself && !self.still_idle(&look) {
            self.host.cancelled();
            let wait = rules::IDLE_BEFORE_INSTALL_MS.saturating_sub(self.since_input(&look));
            self.back_on_offer(version, Some(self.now_ms().saturating_add(wait)));
            return;
        }
        let Some(beside) = beside_app(&facts.app) else {
            give_up("this copy isn't in a folder");
            return;
        };
        let plan = Plan { from: facts.version.clone(), to: version.to_owned(), pid: std::process::id(), app: facts.app.clone(), beside, root: self.files.root.clone(), quit_at_ms: crate::wall_ms(), background: !self.host.frontmost(), quit_limit_ms: crate::plan::QUIT_LIMIT_MS, start_limit_ms: crate::plan::START_LIMIT_MS, test: test_launch(&self.source, self.host.relaunch()) };
        let written = plan.write(&self.files.plan());
        if written.is_err() {
            give_up("the update's plan couldn't be written");
            return;
        }
        {
            let mut st = lock(&self.st);
            st.status = Status::Installing(version.to_owned());
            st.notice = None;
        }
        let helper = facts.app.join(HELPER);
        if self.host.start_installer(&helper, &self.files.plan(), &self.files.log).is_err() {
            give_up("the installer couldn't be started");
            return;
        }
        self.log(&format!("{version}: handed off, quitting"));
        if !self.host.quit_now() {
            give_up("the app couldn't be asked to quit");
            return;
        }
        // Past here this process should be gone. If it isn't, the plan goes, so the installer stands down, and the app's given back.
        std::thread::sleep(QUIT_PATIENCE);
        give_up("the app didn't quit");
    }

    /// The line in Settings and `update status`.
    #[must_use]
    pub fn status_line(&self) -> String {
        let now = self.now_ms();
        let st = lock(&self.st);
        let version = st.facts.as_ref().map_or_else(String::new, |facts| facts.version.clone());
        match &st.status {
            Status::Idle => format!("{APP_NAME} {version} · not checked yet"),
            Status::Checking => "Checking for updates…".to_owned(),
            Status::UpToDate | Status::NoReleases => st.saved.last_check_ms.map_or_else(|| format!("{APP_NAME} {version} is up to date"), |at| format!("{APP_NAME} {version} is up to date · checked {}", ago(at, now))),
            Status::Available(offered) => format!("{APP_NAME} {offered} is available"),
            Status::AvailableElsewhere(offered) => format!("{APP_NAME} {offered} is available · a development build doesn't replace itself"),
            Status::CheckFailed(why) => format!("Couldn't check for updates: {why}"),
            Status::Preparing(offered) => format!("Downloading {APP_NAME} {offered}…"),
            Status::Waiting { version: offered, for_what } => format!("Updating to {offered} as soon as this is done: {for_what}"),
            Status::Installing(offered) => format!("Installing {APP_NAME} {offered}…"),
            Status::Failed { version: offered, why } => format!("Update to {offered} failed: {why}"),
            Status::Updated(offered) => format!("Updated to {offered}"),
            Status::WentBack { version: offered, back_on } => format!("{offered} didn't start, so {back_on} came back"),
        }
    }

    /// What's to be said in the app's margin now, if anything.
    #[must_use]
    pub fn notice(&self) -> Option<Notice> {
        lock(&self.st).notice.clone()
    }

    /// It's been read, or it faded.
    pub fn dismiss_notice(&self) {
        lock(&self.st).notice = None;
    }

    /// For `recto update status`, `qa state` and tests.
    #[must_use]
    pub fn report(&self) -> Value {
        let now = self.now_ms();
        let line = self.status_line();
        let menu = self.menu_item();
        let next_wake = self.next_wake();
        let on = self.switch_on();
        // Runs a tool, so before the lock.
        let app = lock(&self.st).facts.as_ref().map(|facts| facts.app.clone());
        let kept = app.as_deref().and_then(beside_app).map(|beside| previous_app_in(&beside)).filter(|previous| previous.is_dir()).and_then(|previous| stage::app_version(&previous));
        let st = lock(&self.st);
        json!({
            "version": st.facts.as_ref().map(|facts| facts.version.clone()),
            "build": self.source.mode_name(),
            "status": st.status.code(),
            "words": line,
            "menu": { "title": menu.title, "enabled": menu.enabled },
            "offered": st.saved.offered,
            "auto_update": on,
            "failed_versions": st.saved.failed,
            "last_check_ms": st.saved.last_check_ms,
            "last_update": st.saved.last_update.as_ref().map(|update| json!({ "from": update.from, "to": update.to, "gap_ms": update.gap_ms, "at_ms": update.at_ms })),
            "next_wake_in_ms": next_wake.map(|at| at.saturating_sub(now)),
            "install_look_in_ms": st.install_look_at.map(|at| at.saturating_sub(now)),
            "kept": kept,
            "log": self.files.log,
        })
    }

    /// The user's quitting, so an install under way stands down: quitting doesn't turn into a reopening.
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
    }

    /// The quit didn't happen after all.
    pub fn stop_over(&self) {
        self.stopping.store(false, Ordering::SeqCst);
    }
}

/// A test's relaunch, never a release's: the source says which it is.
fn test_launch(source: &Source, mut launch: TestLaunch) -> Option<TestLaunch> {
    if source.mode != Mode::Test {
        return None;
    }
    launch.env.extend(source.test_env.iter().cloned());
    Some(launch)
}

/// What's in an outcome file is a code, and this keeps it one on its way to the log.
fn clean_code(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(40).collect()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::fetch::tests::{Answer, Server};
    use crate::stage::tests::{fake_app, latest_json, temp_dir, zip_of};
    use std::sync::atomic::AtomicU32;

    #[derive(Default)]
    struct Fake {
        busy: Mutex<Vec<String>>,
        since_input_ms: Mutex<u64>,
        refuse_with: Mutex<Option<HandOffRefused>>,
        hand_offs: AtomicU32,
        quits: AtomicU32,
        cancels: AtomicU32,
        installers: Mutex<Vec<PathBuf>>,
        installer_fails: AtomicBool,
        changes: AtomicU32,
        frontmost: AtomicBool,
        plan_at_quit: Mutex<Option<Plan>>,
    }

    impl Host for Fake {
        fn look(&self) -> Look {
            Look { busy: lock(&self.busy).clone(), since_input_ms: *lock(&self.since_input_ms) }
        }
        fn hand_off(&self, _limit: Duration) -> Result<(), HandOffRefused> {
            self.hand_offs.fetch_add(1, Ordering::SeqCst);
            match lock(&self.refuse_with).clone() {
                Some(refused) => Err(refused),
                None => Ok(()),
            }
        }
        fn quit_now(&self) -> bool {
            self.quits.fetch_add(1, Ordering::SeqCst);
            true
        }
        fn cancelled(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
        }
        fn changed(&self) {
            self.changes.fetch_add(1, Ordering::SeqCst);
        }
        fn frontmost(&self) -> bool {
            self.frontmost.load(Ordering::SeqCst)
        }
        fn relaunch(&self) -> TestLaunch {
            TestLaunch::default()
        }
        fn start_installer(&self, helper: &Path, plan: &Path, _log: &Path) -> std::io::Result<u32> {
            *lock(&self.plan_at_quit) = Plan::read(plan);
            lock(&self.installers).push(helper.to_path_buf());
            if self.installer_fails.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("no"));
            }
            Ok(1)
        }
    }

    struct Bed {
        dir: PathBuf,
        server: Server,
        fake: Arc<Fake>,
        updater: Arc<Updater>,
        app: PathBuf,
    }

    impl Bed {
        fn new(name: &str) -> Self {
            let dir = temp_dir(&format!("u-{name}"));
            let root = dir.join("state");
            local::ensure_private_dir(&root).unwrap();
            let install = dir.join("install");
            std::fs::create_dir_all(&install).unwrap();
            let app = fake_app(&install, "0.1.0", "exit 0");
            let server = Server::start();
            let fake = Arc::new(Fake::default());
            let updater = Updater::new(&root, server.source(), Arc::clone(&fake) as Arc<dyn Host>);
            assert_eq!(updater.launched(Facts { version: "0.1.0".into(), app: app.clone(), system: "15.0".into() }), AtLaunch::GoOn);
            Self { dir, server, fake, updater, app }
        }

        fn publish(&self, version: &str) {
            let built = self.dir.join(format!("r-{version}"));
            std::fs::create_dir_all(&built).unwrap();
            let app = fake_app(&built, version, "exit 0");
            let (bytes, sha) = zip_of(&app, &built.join("r.zip"));
            self.server.set(&format!("/releases/download/v{version}/RectoPDF-{version}.zip"), Answer::Body(bytes));
            self.server.set("/releases/latest/download/latest.json", Answer::Body(latest_json(version, &sha)));
        }

        fn wait_for(&self, what: &str, done: impl Fn(&Self) -> bool) {
            let began = Instant::now();
            while !done(self) {
                assert!(began.elapsed() < Duration::from_secs(30), "waited for {what}: {}", self.updater.report());
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn status(&self) -> Status {
            lock(&self.updater.st).status.clone()
        }

        fn found(&self, version: &str) {
            let status = self.updater.check_here(Asked::ByHand);
            assert_eq!(status, Status::Available(version.to_owned()));
        }
    }

    impl Drop for Bed {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Failure: the menu doesn't turn from a check into an offer as the owner asked, a check by hand doesn't say how it went, or what a check finds isn't there at the next launch.
    #[test]
    fn the_menu_goes_from_check_to_update_and_a_check_by_hand_says_so() {
        let bed = Bed::new("menu");
        assert_eq!(bed.updater.menu_item(), MenuItem { title: "Check for Updates…".into(), enabled: true });
        assert_eq!(bed.updater.check_here(Asked::ByHand), Status::NoReleases);
        assert!(bed.updater.notice().unwrap().text.contains("up to date"), "{:?}", bed.updater.notice());
        assert_eq!(bed.updater.menu_item().title, "Check for Updates…");
        bed.publish("0.1.0");
        assert_eq!(bed.updater.check_here(Asked::ByHand), Status::UpToDate);
        assert!(bed.updater.notice().unwrap().text.starts_with("You're up to date"));
        bed.publish("0.2.0");
        bed.found("0.2.0");
        assert_eq!(bed.updater.menu_item(), MenuItem { title: "Update to 0.2.0…".into(), enabled: true });
        assert!(bed.updater.notice().unwrap().text.contains("0.2.0 is available"));
        // Withdrawn since: an install looks again before it does anything, and takes it off the menu.
        bed.server.set("/releases/latest/download/latest.json", Answer::Status(404));
        bed.updater.install(Asked::ByHand).unwrap();
        bed.wait_for("the second look", |bed| !lock(&bed.updater.st).status.is_under_way());
        assert_eq!(bed.status(), Status::UpToDate);
        assert_eq!(bed.updater.menu_item().title, "Check for Updates…");
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "a withdrawn release never gets near a quit");
        bed.publish("0.2.0");
        bed.found("0.2.0");
        // And again at the next launch, with no check.
        let again = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        again.launched(Facts { version: "0.1.0".into(), app: bed.app.clone(), system: "15.0".into() });
        assert_eq!(again.menu_item().title, "Update to 0.2.0…");
        // An offer the same as this version, once it's installed, isn't one.
        let after = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        after.launched(Facts { version: "0.2.0".into(), app: bed.app.clone(), system: "15.0".into() });
        assert_eq!(after.menu_item().title, "Check for Updates…");
    }

    /// Failure: a check that goes wrong raises an alarm at the user by itself, leaves the menu stuck, or the next one comes at once. Each of a garbled answer, a downgrade and a dead server.
    #[test]
    fn a_failed_check_stays_out_of_the_way_and_backs_off() {
        let bed = Bed::new("failed");
        bed.server.set("/releases/latest/download/latest.json", Answer::Body(b"garbled".to_vec()));
        bed.updater.check_in_background(Asked::ByItself);
        bed.wait_for("the check", |bed| !lock(&bed.updater.st).status.is_under_way());
        assert!(matches!(bed.status(), Status::CheckFailed(_)));
        assert_eq!(bed.updater.notice(), None, "by itself, it says nothing");
        let saved = lock(&bed.updater.st).saved.clone();
        assert_eq!(saved.failures, 1);
        let next = bed.updater.next_wake().unwrap();
        assert!(next >= bed.updater.now_ms() + 3_500_000, "an hour later, not now");
        // A release older than this one isn't on offer, and isn't a failure.
        bed.publish("0.0.9");
        assert_eq!(bed.updater.check_here(Asked::ByHand), Status::UpToDate);
        assert_eq!(lock(&bed.updater.st).saved.failures, 0);
        assert_eq!(bed.updater.install(Asked::ByHand), Err(InstallRefused::NothingOnOffer), "a downgrade can't be asked for");
    }

    /// Failure: a development build replaces itself, or checks by itself. The owner's rule: development builds check only when asked and never replace themselves. It also answers a check by hand, against a server it's been told about only here.
    #[test]
    fn a_development_build_doesnt_replace_itself() {
        let dir = temp_dir("dev");
        let root = dir.join("state");
        local::ensure_private_dir(&root).unwrap();
        let fake = Arc::new(Fake::default());
        let source = Source { mode: Mode::Development, repo: "http://127.0.0.1:9".into(), trust: crate::source::Trust::AnySignature, network: false, test_env: Vec::new() };
        let updater = Updater::new(&root, source, Arc::clone(&fake) as Arc<dyn Host>);
        let app = fake_app(&dir, "0.1.0", "exit 0");
        updater.launched(Facts { version: "0.1.0".into(), app, system: "15.0".into() });
        assert_eq!(updater.next_wake(), None, "no timer at all");
        assert!(!updater.switch_on(), "off to begin with");
        lock(&updater.st).saved.offered = Some("0.2.0".into());
        lock(&updater.st).status = Status::AvailableElsewhere("0.2.0".into());
        assert_eq!(updater.menu_item().title, "Check for Updates…");
        assert_eq!(updater.install(Asked::ByHand), Err(InstallRefused::NotInThisBuild));
        assert_eq!(fake.hand_offs.load(Ordering::SeqCst), 0);
        // A quiet run with no test server reaches no real one.
        assert!(matches!(updater.check_here(Asked::ByHand), Status::CheckFailed(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: an update chosen by hand asks about unsaved edits, or doesn't wait for what's in flight, or goes without the download being verified. The order the owner asked for: everything slow first, then a hand-off and a quit, and the plan says where it's going.
    #[test]
    fn an_update_by_hand_goes_at_once_after_the_slow_part() {
        let bed = Bed::new("hand");
        bed.publish("0.2.0");
        bed.found("0.2.0");
        bed.fake.frontmost.store(true, Ordering::SeqCst);
        assert_eq!(bed.updater.install(Asked::ByHand), Ok("0.2.0".to_owned()));
        assert_eq!(bed.updater.install(Asked::ByHand), Err(InstallRefused::Underway), "one at a time");
        bed.wait_for("the quit", |bed| bed.fake.quits.load(Ordering::SeqCst) == 1);
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 1);
        let plan = lock(&bed.fake.plan_at_quit).clone().unwrap();
        assert_eq!((plan.from.as_str(), plan.to.as_str(), plan.pid), ("0.1.0", "0.2.0", std::process::id()));
        assert!(!plan.background, "it was frontmost, so it comes back frontmost");
        assert!(plan.new_app().is_dir(), "already there, verified");
        assert_eq!(stage::app_version(&plan.new_app()).as_deref(), Some("0.2.0"));
        assert_eq!(bed.fake.cancels.load(Ordering::SeqCst), 0);
    }

    /// Failure: the app quits over a sheet or a running command instead of waiting, doesn't tell the user what it's waiting for, or forgets to go on when it's done.
    #[test]
    fn a_busy_app_waits_and_says_so() {
        let bed = Bed::new("busy");
        bed.publish("0.2.0");
        bed.found("0.2.0");
        lock(&bed.fake.busy).push("A sheet or panel is open over the window.".into());
        bed.updater.install(Asked::ByHand).unwrap();
        bed.wait_for("the wait", |bed| matches!(bed.status(), Status::Waiting { .. }));
        let notice = bed.updater.notice().unwrap();
        assert!(notice.text.contains("as soon as this is done") && notice.text.contains("sheet"), "{notice:?}");
        assert!(!notice.fades, "it stays while it's so");
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
        assert_eq!(bed.updater.menu_item(), MenuItem { title: "Updating to 0.2.0…".into(), enabled: false });
        lock(&bed.fake.busy).clear();
        bed.wait_for("the quit", |bed| bed.fake.quits.load(Ordering::SeqCst) == 1);
    }

    /// Failure: a hand-off that's refused, or one that's good and then goes wrong, leaves the app frozen or says nothing. Without `hand_off_cancelled` on every way out past a good hand-off, the app would stay frozen.
    #[test]
    fn a_stopped_hand_off_never_leaves_the_app_frozen() {
        let bed = Bed::new("cancel");
        bed.publish("0.2.0");
        bed.found("0.2.0");
        // A refusal: the state couldn't be written. The app's frozen only inside the hand-off, which undid it.
        *lock(&bed.fake.refuse_with) = Some(HandOffRefused::NotSaved("the disk is full".into()));
        bed.updater.install(Asked::ByHand).unwrap();
        bed.wait_for("the failure", |bed| matches!(bed.status(), Status::Failed { .. }));
        assert_eq!(bed.fake.quits.load(Ordering::SeqCst), 0, "no quit, or the state would be lost");
        assert!(bed.updater.notice().unwrap().text.contains("is unchanged"));
        assert_eq!(bed.updater.menu_item().title, "Check for Updates…", "it can be asked again");
        assert_eq!(lock(&bed.updater.st).saved.failed, Vec::<String>::new(), "this may pass, so it's not held against the version");
        // A good hand-off and no installer: this one has to be undone.
        *lock(&bed.fake.refuse_with) = None;
        bed.fake.installer_fails.store(true, Ordering::SeqCst);
        bed.found("0.2.0");
        bed.updater.install(Asked::ByHand).unwrap();
        bed.wait_for("the cancel", |bed| bed.fake.cancels.load(Ordering::SeqCst) == 1);
        bed.wait_for("the failure", |bed| matches!(bed.status(), Status::Failed { .. }));
        assert_eq!(bed.fake.quits.load(Ordering::SeqCst), 0);
        assert!(!bed.updater.files.plan().exists(), "the plan goes with it");
    }

    /// Failure: a release that's bad is tried over and over by itself, or tried by hand and held against the user. A lasting refusal is remembered for this version, and a newer one gets its chance.
    #[test]
    fn a_bad_release_isnt_tried_again_by_itself() {
        let bed = Bed::new("bad");
        bed.publish("0.2.0");
        bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Body(b"not the file".to_vec()));
        bed.found("0.2.0");
        *lock(&bed.fake.since_input_ms) = rules::IDLE_BEFORE_INSTALL_MS;
        bed.updater.install(Asked::ByItself).unwrap();
        bed.wait_for("the failure", |bed| matches!(bed.status(), Status::Failed { .. }));
        assert_eq!(lock(&bed.updater.st).saved.failed, vec!["0.2.0".to_string()]);
        assert_eq!(bed.updater.notice(), None, "by itself, it says nothing");
        assert_eq!(lock(&bed.updater.st).install_look_at, None, "not looked at again");
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "the app never hears of it");
    }

    /// Failure: the 30-minute rule is broken: an install goes while the user's in the app, or doesn't go when they've been away, or it goes when they've come back during the download. With a clock moved on rather than half an hour's sleep, by the same call the timer makes.
    #[test]
    fn an_automatic_install_goes_only_after_thirty_quiet_minutes() {
        let bed = Bed::new("thirty");
        bed.publish("0.2.0");
        bed.updater.check_here(Asked::ByItself);
        assert_eq!(bed.status(), Status::Available("0.2.0".into()));
        assert_eq!(bed.updater.notice(), None);
        // Just touched: it waits as long as it has to, and the timer's set for then, not for every minute.
        *lock(&bed.fake.since_input_ms) = 60_000;
        bed.updater.timer_fired();
        let wait = lock(&bed.updater.st).install_look_at.unwrap().saturating_sub(bed.updater.now_ms());
        assert!((rules::IDLE_BEFORE_INSTALL_MS - 300_000..=rules::IDLE_BEFORE_INSTALL_MS - 59_000).contains(&wait), "{wait}");
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
        assert!(bed.updater.set_test_clock(i64::try_from(wait).unwrap() + 1_000), "a test's clock");
        // It's been touched again since, so the clock moving on doesn't count from the first touch: what counts is the time since the last.
        *lock(&bed.fake.since_input_ms) = 0;
        bed.updater.set_test_clock(i64::try_from(wait).unwrap() + 1_000);
        bed.updater.timer_fired();
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "there was input, so it's not been thirty quiet minutes");
        // Now nobody's touched it for the whole half hour.
        *lock(&bed.fake.since_input_ms) = rules::IDLE_BEFORE_INSTALL_MS;
        bed.updater.set_test_clock(i64::try_from(rules::IDLE_BEFORE_INSTALL_MS).unwrap() * 2);
        bed.updater.timer_fired();
        bed.wait_for("the quit", |bed| bed.fake.quits.load(Ordering::SeqCst) == 1);
        assert!(lock(&bed.fake.plan_at_quit).clone().unwrap().background, "it was not frontmost, so it does not take the focus");
    }

    /// Failure: an automatic install stays waiting at the same place when the switch goes off, or carries on while the user's come back. The switch stops it at once, whichever way it's set for the daily check.
    #[test]
    fn the_switch_stops_an_automatic_install_and_not_the_check() {
        let bed = Bed::new("switch");
        bed.publish("0.2.0");
        bed.updater.check_here(Asked::ByItself);
        bed.updater.set_switch(Some(false));
        *lock(&bed.fake.since_input_ms) = rules::IDLE_BEFORE_INSTALL_MS * 2;
        bed.updater.timer_fired();
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "off means it waits for the menu");
        assert_eq!(bed.updater.menu_item().title, "Update to 0.2.0…");
        assert!(bed.updater.next_wake().is_some(), "the daily check runs whichever way the switch is set");
        bed.updater.set_switch(Some(true));
        bed.updater.timer_fired();
        bed.wait_for("the quit", |bed| bed.fake.quits.load(Ordering::SeqCst) == 1);
    }

    /// Failure: quitting while an update waits for something turns into the app reopening by itself as a new version, when the user asked it to go away. An install under way stands down, and it can be asked for again.
    #[test]
    fn a_quit_while_an_update_waits_is_a_quit() {
        let bed = Bed::new("quit");
        bed.publish("0.2.0");
        bed.found("0.2.0");
        lock(&bed.fake.busy).push("A sheet or panel is open over the window.".into());
        bed.updater.install(Asked::ByHand).unwrap();
        bed.wait_for("the wait", |bed| matches!(bed.status(), Status::Waiting { .. }));
        bed.updater.stop();
        lock(&bed.fake.busy).clear();
        bed.wait_for("standing down", |bed| matches!(bed.status(), Status::Available(_)));
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
        assert_eq!(bed.fake.quits.load(Ordering::SeqCst), 0);
        assert_eq!(bed.updater.menu_item().title, "Update to 0.2.0…");
        bed.updater.stop_over();
        bed.updater.install(Asked::ByHand).unwrap();
        bed.wait_for("the quit", |bed| bed.fake.quits.load(Ordering::SeqCst) == 1);
    }

    /// Failure: what a run that ended early left stays beside the app for good, at the size of the app. A part-download and a staged app that's no longer on offer both go at the next launch, and a good staged one stays.
    #[test]
    fn a_launch_clears_what_a_run_left_lying_about() {
        let bed = Bed::new("litter");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        let beside = beside_app(&bed.app).unwrap();
        local::ensure_private_dir(&files.work).unwrap();
        std::fs::write(files.zip("0.2.0"), "part of a download").unwrap();
        std::fs::create_dir_all(beside.join("unpacking")).unwrap();
        std::fs::create_dir_all(new_app_in(&beside)).unwrap();
        std::fs::write(beside.join("staged.json"), "0.2.0 abc").unwrap();
        let again = |offered: Option<&str>| {
            let saved = Saved { offered: offered.map(str::to_owned), ..Saved::default() };
            saved.write(&files.saved);
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            updater.launched(Facts { version: "0.1.0".into(), app: bed.app.clone(), system: "15.0".into() });
        };
        again(Some("0.2.0"));
        assert!(!files.zip("0.2.0").exists() && !beside.join("unpacking").exists());
        assert!(new_app_in(&beside).exists(), "still on offer, so it's worth keeping");
        again(None);
        assert!(!new_app_in(&beside).exists() && !beside.join("staged.json").exists(), "and now it isn't");
    }

    /// Failure: a real run's clock can be moved, which would let anything with the socket cut the wait short. Only a test run's moves.
    #[test]
    fn only_a_test_runs_clock_moves() {
        let dir = temp_dir("clock");
        let root = dir.join("state");
        local::ensure_private_dir(&root).unwrap();
        let source = crate::source::resolve(true, false, None, None);
        let updater = Updater::new(&root, source, Arc::new(Fake::default()) as Arc<dyn Host>);
        assert!(!updater.set_test_clock(999_999_999));
        assert!(updater.now_ms() <= crate::wall_ms() + 1_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: what a stopped installer leaves isn't put right at the next launch, or a launch reads one as a failure when the new version's come up. All the ends a killed installer can leave, by its files, in a launch of either version.
    #[test]
    fn a_launch_puts_right_what_a_stopped_installer_left() {
        let bed = Bed::new("left");
        let root = bed.dir.join("state");
        let plan = Plan { from: "0.1.0".into(), to: "0.2.0".into(), pid: 1, app: bed.app.clone(), beside: beside_app(&bed.app).unwrap(), root: root.clone(), quit_at_ms: 5, background: false, quit_limit_ms: 1, start_limit_ms: 1, test: None };
        let launch = |version: &str| {
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            updater.launched(Facts { version: version.into(), app: bed.app.clone(), system: "15.0".into() });
            updater
        };
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();

        // Stopped before the swap: this is still the old version, and it says what happened.
        plan.write(&files.plan()).unwrap();
        std::fs::create_dir_all(plan.new_app()).unwrap();
        let updater = launch("0.1.0");
        assert!(matches!(lock(&updater.st).status, Status::Failed { .. }));
        assert!(!files.plan().exists() && !plan.new_app().exists(), "nothing's left to confuse the next one");

        // Stopped right after the swap: the new version's here, with the old one where the swap left it.
        plan.write(&files.plan()).unwrap();
        std::fs::create_dir_all(plan.new_app().parent().unwrap()).unwrap();
        fake_app(plan.new_app().parent().unwrap(), "0.1.0", "exit 0");
        let updater = launch("0.2.0");
        assert_eq!(lock(&updater.st).status, Status::Updated("0.2.0".into()));
        assert_eq!(stage::app_version(&plan.previous_app()).as_deref(), Some("0.1.0"), "kept, as the installer would have");
        assert!(!plan.new_app().exists() && !files.plan().exists());
        updater.first_frame(1_005);
        let report = updater.report();
        assert_eq!(report["last_update"]["gap_ms"], 1_000);
        assert_eq!(std::fs::read_to_string(files.launch_ok()).unwrap().split_whitespace().next(), Some("0.2.0"));

        // A version that didn't start: said once, remembered, and not tried by itself again.
        Outcome { kind: OutcomeKind::WentBack, from: "0.1.0".into(), to: "0.3.0".into(), why: "didnt_start".into() }.write(&files.outcome()).unwrap();
        let updater = launch("0.1.0");
        assert_eq!(lock(&updater.st).status, Status::WentBack { version: "0.3.0".into(), back_on: "0.1.0".into() });
        assert!(updater.notice().unwrap().text.contains("came back"));
        assert!(lock(&updater.st).saved.failed.contains(&"0.3.0".to_string()));
        assert!(!files.outcome().exists());
    }

    /// Failure: the timer keeps firing with nothing to do, or there's a wake-up per input, so an idle app costs something at rest. It's one time, which moves only when something happens.
    #[test]
    fn at_rest_there_is_one_time_a_day_and_input_doesnt_move_it() {
        let bed = Bed::new("rest");
        let launched = lock(&bed.updater.st).launched_ms;
        assert_eq!(bed.updater.next_wake(), Some(launched + rules::MIN_GAP_MS), "a minute after launch");
        bed.publish("0.1.0");
        bed.updater.check_here(Asked::ByItself);
        let next = bed.updater.next_wake().unwrap();
        assert!(next >= bed.updater.now_ms() + rules::CHECK_EVERY_MS - 10_000, "a day on");
        for since in [0, 1_000, 5_000] {
            *lock(&bed.fake.since_input_ms) = since;
            assert_eq!(bed.updater.next_wake(), Some(next), "input changes nothing");
        }
    }
}
