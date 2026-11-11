//! **The updater, as the app has it.** Small state and no threads at rest: the host sets one timer for when [`Updater::next_wake`] says, and everything else comes from a menu item, a command or that timer. A check or an install gets a thread for as long as it runs.
//!
//! Every word and rule's here, and the host only draws. What it needs of the app, it asks through [`Host`], which is all a test needs to stand in for.

use crate::fetch::{self, FetchError};
use crate::plan::{Files, Outcome, OutcomeKind, Plan, TestLaunch, beside_app, helper_alive, log, new_app_in, previous_app_in};
use crate::rules::{self, AutoLook, AutoStep, Latest, MAX_LATEST_BYTES, is_newer, parse_latest};
use crate::saved::{LastUpdate, Probation, Saved};
use crate::source::{Mode, Source};
use crate::stage::{self, Stage, StageError};
use crate::{APP_NAME, HELPER};
use recto_platform::local;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

pub mod builds;

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
/// Launches of a new version that don't come up. The launch after this many goes back to the kept one, as a launch that fails can't do anything about it itself.
const UNSTARTED_LIMIT: u32 = 3;
/// Times an install gets as far as quitting the app and reopening it with nothing to show for it. Past this it's not tried by itself again, as each one's the user's app going away and coming back for nothing.
const QUITS_FOR_NOTHING: u32 = 3;
/// How long the record as it was before an update is kept, and how long a new version that's come up is watched for runs that end early. A new version that comes up having misread the record, with fewer tabs than it was handed, goes back at once with this copy put back; one that misreads it in some way that keeps its tabs has thrown away nothing that can't be given back by hand, for this long. The next update replaces it. A clone on APFS, so it costs no disk until the record changes.
pub const KEEP_RECORD_COPY_MS: u64 = 7 * 24 * 3600 * 1000;
/// A run of a new version that ends sooner than this after its launch, by anything but a quit someone asked for, has ended early: a crash, or a hang someone had to end. Once a run's lasted this long, the count starts again.
pub const EARLY_END_MS: u64 = 60_000;
/// Runs in a row of a new version that end early. The launch after this many goes back to the kept version, leaving the record as it is, as it holds whatever the user's done since: the crash loop that no first frame can catch.
pub const EARLY_ENDS_LIMIT: u32 = 3;
/// How long the installer waits in a test's run for a new app to come up, as its broken ones are the point.
const TEST_START_LIMIT_MS: u64 = 30_000;

/// What the app is to the updater.
pub trait Host: Send + Sync {
    /// What's in flight, in words for the user, and how long since they touched the app. Cheap.
    fn look(&self) -> Look;
    /// Writes and reads back the state, freezing the app. Blocks on the disk.
    fn hand_off(&self, limit: Duration) -> Result<HandedOff, HandOffRefused>;
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
    /// Whether this copy of the app keeps the state record. A second copy on the same state can't hand off, so it has no business downloading or tidying what the first has staged.
    fn keeps_the_record(&self) -> bool {
        true
    }
    /// Whether a development build an agent handed over may still go in: agent access is still Full, as it was when the build was handed over. Read now, as the level can drop with no word to anyone. Cheap: one small file.
    fn may_take_a_build(&self) -> bool {
        true
    }
    /// Whether this version can draw a page at all: our own tiny PDF, through the real renderer. Blocks for as long as a worker takes to start, so only ever on a thread of its own, and only when a new version's to be proven.
    fn can_draw(&self) -> bool {
        true
    }
    /// How many tabs the record held as this launch read it, or nothing if it hasn't read it yet. Counted in the record, not the session: entries a restore leaves out on purpose, as a second tab on the same file, aren't a misread, and nor is a tab closed since.
    fn tabs_restored(&self) -> Option<u32> {
        None
    }
    /// What this launch was asked to open, by its arguments or the system, before it read the state. What a launch that goes back passes on to the version it goes back to.
    fn documents_asked_for(&self) -> Vec<PathBuf> {
        Vec::new()
    }
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

/// What a good hand-off wrote and read back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HandedOff {
    /// The tabs in the record as written and read back: what a new version that reads fewer has misread.
    pub tabs: Option<u32>,
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
    Waiting {
        version: String,
        for_what: String,
    },
    Installing(String),
    Failed {
        version: String,
        why: String,
    },
    Updated(String),
    WentBack {
        version: String,
        back_on: String,
    },
    /// A development build's been handed over and checked, and goes in once the app hasn't been used for 5 minutes.
    BuildReady(String),
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
            Self::BuildReady(_) => "build_ready",
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
    /// Another copy of the app has the state, and it's the one that can.
    AnotherCopy,
    /// A development build handed over, and *Allow development builds* is off.
    DevBuildsOff,
    /// What was handed over isn't a `RectoPDF-<version>.zip`.
    NotABuild,
    /// It's the version that's running.
    SameVersion,
    /// This copy is signed by the team, and takes no build signed ad hoc: the first goes in by `tools/dev-build.sh --first-install --replace-release`.
    TeamSignedCopy,
    /// It was checked, and it isn't one to install: the code and our own words.
    Refused {
        code: &'static str,
        why: String,
    },
}

struct St {
    status: Status,
    saved: Saved,
    launched_ms: u64,
    facts: Option<Facts>,
    /// Another copy of the app keeps this state folder, so this one writes nothing and installs nothing.
    second_copy: bool,
    notice: Option<Notice>,
    /// When an automatic install next looks. Set when one's on offer. Moves only then and when it's looked: nothing's done per input.
    install_look_at: Option<u64>,
    /// What this launch came from, if it's an update that's still to reach its first frame.
    arrived: Option<Plan>,
    /// This launch was opened by an update: arriving, or the version before back after one that didn't work or wasn't installed. It stays so after the first frame.
    reopened_by_update: bool,
    /// The record from before the update, to take if what the version that didn't work left can't be read here. Only at the launch that comes back.
    fall_back: Option<PathBuf>,
    first_frame_done: bool,
    /// The first frame came before what's remembered had been read, off a slow disk. It's dealt with as soon as it has.
    first_frame_early: Option<u64>,
    /// A development build handed over and checked, waiting beside the app for its moment.
    build: Option<builds::Build>,
    /// What's under way was chosen from the menu, so its answer is for someone looking at the app: with no window to say it in, the host shows it as an alert.
    from_menu: bool,
    /// That answer, for an alert, until the host's taken it.
    alert: Option<String>,
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
    /// The installer opened this launch, as it went back or didn't install.
    reopened: bool,
    fall_back: Option<PathBuf>,
    changed: bool,
    plan_gone: bool,
}

/// Why a launch is going back by itself, with no installer about to see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Back {
    /// It never came up, and the record's put back as it was before the update.
    NeverCameUp,
    /// It came up and then kept ending early, and the record stays as it is.
    KeptEnding,
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
        let st = St {
            status: Status::Idle,
            saved: Saved::default(),
            launched_ms: crate::wall_ms(),
            facts: None,
            second_copy: false,
            notice: None,
            install_look_at: None,
            arrived: None,
            reopened_by_update: false,
            fall_back: None,
            first_frame_done: false,
            first_frame_early: None,
            build: None,
            from_menu: false,
            alert: None,
        };
        Arc::new(Self {
            files: Files::in_state(root),
            source,
            host,
            st: Mutex::new(st),
            writing: Mutex::new(()),
            clock_ahead_ms: AtomicI64::new(0),
            stopping: AtomicBool::new(false),
        })
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
        let (saved, second_copy) = {
            let st = lock(&self.st);
            (st.saved.clone(), st.second_copy)
        };
        if !second_copy {
            saved.write(&self.files.saved);
        }
    }

    /// Whether the switch is on, by what was chosen or by what this build starts with.
    #[must_use]
    pub fn switch_on(&self) -> bool {
        lock(&self.st).saved.auto_update.unwrap_or_else(|| self.source.auto_default())
    }

    /// The switch was flipped. It holds at once, and an offer waiting is looked at again. It goes to disk on a thread of its own, as this may well be the UI thread.
    pub fn choose_switch(self: &Arc<Self>, on: bool) {
        {
            let mut st = lock(&self.st);
            st.saved.auto_update = Some(on);
            st.install_look_at = (on && self.offered_here(&st).is_some()).then(|| self.now_ms());
        }
        self.save_soon();
        self.host.changed();
    }

    /// Writes what's remembered on a thread of its own, as this may be the UI thread.
    fn save_soon(self: &Arc<Self>) {
        let me = Arc::clone(self);
        let written = std::thread::Builder::new()
            .name("recto-update-save".into())
            .stack_size(128 * 1024)
            .spawn(move || me.save());
        if written.is_err() {
            self.save();
        }
    }

    fn offered_here(&self, st: &St) -> Option<String> {
        if !self.source.may_replace_itself() || st.second_copy {
            return None;
        }
        let current = st.facts.as_ref()?.version.as_str();
        st.saved
            .offered
            .as_ref()
            .filter(|offered| is_newer(offered, current) || builds::is_way_back(&st.saved, offered, current))
            .cloned()
    }

    /// What the host knows of this copy. Given once, before anything else asks. Reads what's remembered, says what became of an install that's just ended, and tidies up. All of it files, so never on the UI thread, and none of it holding what the UI thread asks through.
    pub fn launched(&self, mut facts: Facts) -> AtLaunch {
        let now = self.now_ms();
        facts.app = facts.app.canonicalize().unwrap_or(facts.app);
        let mut saved = Saved::read(&self.files.saved);
        let mut dirty_at_launch = false;
        let current = facts.version.clone();
        let app = facts.app.clone();
        // What's in the state folder and beside the app is the first copy's. This one reads and leaves it.
        let second_copy = !self.host.keeps_the_record();
        // Kept or staged by a build before these had names of their own, and not while that build's installer may still be looking for them by the old ones.
        if !second_copy
            && !helper_alive(&self.files)
            && let Some(beside) = beside_app(&facts.app)
        {
            crate::plan::move_to_new_names(&beside);
        }
        let settled = if second_copy {
            Settled::default()
        } else {
            self.settle_last_install(&mut saved, &current, &app, now)
        };
        let may_replace = self.source.may_replace_itself() && !second_copy;
        // A release running is where the way back from a development build was going.
        if saved.back_to_release && !crate::version::is_development(&current) {
            saved.back_to_release = false;
            dirty_at_launch = true;
        }
        let offered = saved
            .offered
            .clone()
            .filter(|offered| is_newer(offered, &current) || builds::is_way_back(&saved, offered, &current));
        // What's staged and no longer on offer is the size of the app, for nothing. Only with no installer at work.
        if !second_copy && settled.plan_gone && !helper_alive(&self.files) {
            self.clear_leftovers(&app, &current, offered.is_some());
            self.drop_an_old_record_copy();
        }
        let status = settled.status.unwrap_or_else(|| match &offered {
            Some(offered) if may_replace => Status::Available(offered.clone()),
            Some(offered) => Status::AvailableElsewhere(offered.clone()),
            None if saved.last_check_ms.is_some() => Status::UpToDate,
            None => Status::Idle,
        });
        // A new version counts towards going back until it's reached a first frame. Once it has, a launch writes nothing here.
        let mut going_back = false;
        let mut dirty = settled.changed || dirty_at_launch;
        let beside = beside_app(&app).unwrap_or_default();
        if may_replace && saved.proven.as_deref() != Some(current.as_str()) && previous_app_in(&beside).is_dir() {
            saved.unstarted = saved.unstarted.saturating_add(1);
            dirty = true;
            going_back =
                saved.unstarted > UNSTARTED_LIMIT && !helper_alive(&self.files) && self.start_going_back(&mut saved, &current, &app, now, Back::NeverCameUp);
        }
        if may_replace && !going_back && saved.probation.is_some() {
            dirty = true;
            going_back = self.probation_at_launch(&mut saved, &current, &app, now);
        }
        let early = {
            let mut st = lock(&self.st);
            st.launched_ms = now;
            st.saved = saved;
            st.facts = Some(facts);
            st.second_copy = second_copy;
            st.status = status;
            st.notice = settled.notice;
            st.reopened_by_update = settled.arrived.is_some() || settled.reopened;
            st.fall_back = settled.fall_back;
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

    /// What a run that ended early left: a part-downloaded zip, a part-unpacked app, and a staged one that's no use now. Looks at a few names, and a normal launch finds none of them. **Never the only good copy:** an app there that's older than this one is the swap's other half, from when keeping it didn't work, and it's kept now, if nothing else is.
    fn clear_leftovers(&self, app: &Path, current: &str, still_on_offer: bool) {
        if let Ok(entries) = std::fs::read_dir(&self.files.work) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with(".zip") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let Some(beside) = beside_app(app) else { return };
        let _ = std::fs::remove_dir_all(beside.join("unpacking"));
        let left = new_app_in(&beside);
        if left.is_dir() && stage::app_version(&left).is_some_and(|version| is_newer(current, &version)) {
            keep_if_nothing_is(&beside, &left);
        }
        if !still_on_offer || !beside.join("staged.json").exists() {
            stage::clear_staged(app);
        }
    }

    /// What's kept of the record from before the last update goes once it's a week old. One look at one file's time.
    fn drop_an_old_record_copy(&self) {
        let whole = self.files.before().join(".whole");
        let age = std::fs::metadata(&whole)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|at| at.elapsed().ok());
        if age.is_some_and(|age| age.as_millis() > u128::from(KEEP_RECORD_COPY_MS)) {
            let _ = std::fs::remove_dir_all(self.files.before());
        }
    }

    /// The state's record folder.
    #[must_use]
    pub fn session_folder(&self) -> PathBuf {
        self.files.session()
    }

    /// This copy's bundle, once the host's said.
    #[must_use]
    pub fn app(&self) -> Option<PathBuf> {
        lock(&self.st).facts.as_ref().map(|facts| facts.app.clone())
    }

    /// Whether this launch was opened by an update, so what comes back is exactly what was left, down to a window that was closed, in the Dock or hidden: an update arriving, or the version before back after one that didn't work, or that wasn't installed and the installer reopened this one. Any other launch was asked for by someone, and shows a window, as an app does.
    #[must_use]
    pub fn reopened_by_update(&self) -> bool {
        lock(&self.st).reopened_by_update
    }

    /// Whether this launch may be one an update opened, before the state's been read: what it was found to be once the launch has been looked at, and until then, whether an install left its plan or what became of it. A stat or two, for the window's patience to go by.
    #[must_use]
    pub fn may_be_reopened_by_update(&self) -> bool {
        {
            let st = lock(&self.st);
            if st.facts.is_some() {
                return st.reopened_by_update;
            }
        }
        self.files.plan().exists() || self.files.outcome().exists()
    }

    /// The record from before the update, if this launch is the version before coming back from one that kept ending, and it's there: for when what that one left can't be read here. Taken once.
    #[must_use]
    pub fn take_record_to_fall_back_on(&self) -> Option<PathBuf> {
        lock(&self.st).fall_back.take()
    }

    /// The record from before the update was put back, as what the version that didn't work left couldn't be read: the notice says so.
    pub fn fell_back(&self) {
        let mut st = lock(&self.st);
        if let Status::WentBack { version, back_on } = &st.status {
            let text = format!(
                "{APP_NAME} {version} kept quitting unexpectedly, and what it left couldn't be read, so {back_on} came back with your session from before the update."
            );
            st.notice = Some(Notice { text, fades: true });
        }
        drop(st);
        self.log("what the version that didn't work left couldn't be read, so the record from before the update was put back");
    }

    /// What the last install left behind. The installer tidies up when it finishes: this is for when it didn't get to.
    fn settle_last_install(&self, saved: &mut Saved, current: &str, app: &Path, now: u64) -> Settled {
        let mut settled = Settled::default();
        let Some(beside) = beside_app(app) else { return settled };
        if let Some(outcome) = Outcome::read(&self.files.outcome()) {
            let _ = std::fs::remove_file(self.files.outcome());
            let _ = std::fs::remove_file(self.files.plan());
            let _ = std::fs::remove_file(self.files.helper_pid());
            // Gone back with it put back, the copy's for nothing. Stuck on a version that didn't come up, it's the state before it, and stays. Gone back with the record as it was, it's what to fall back on if the previous version can't read that, and it goes after its week as ever.
            let kept_ending = outcome.kind == OutcomeKind::WentBack && outcome.why == "kept_ending";
            if kept_ending {
                settled.fall_back = self.files.before().join(".whole").exists().then(|| self.files.before());
            } else if outcome.kind != OutcomeKind::CouldntGoBack {
                let _ = std::fs::remove_dir_all(self.files.before());
            }
            settled.reopened = match outcome.kind {
                OutcomeKind::WentBack => true,
                OutcomeKind::NotInstalled => installer_reopened(&outcome.why),
                OutcomeKind::CouldntGoBack => false,
            };
            settled.changed = true;
            settled.plan_gone = true;
            match outcome.kind {
                OutcomeKind::WentBack => {
                    saved.note_failed_version(&outcome.to);
                    saved.install_failures = 0;
                    settled.status = Some(Status::WentBack {
                        version: outcome.to.clone(),
                        back_on: current.to_owned(),
                    });
                    settled.notice = Some(Notice {
                        text: went_back_words(&outcome.why, &outcome.to, current),
                        fades: true,
                    });
                    self.log(&format!("{} didn't work ({}), and {current} came back", outcome.to, clean_code(&outcome.why)));
                }
                OutcomeKind::CouldntGoBack => {
                    saved.note_failed_version(&outcome.to);
                    let why = "it didn't come up, and there was no earlier version to go back to";
                    settled.status = Some(Status::Failed {
                        version: outcome.to.clone(),
                        why: why.to_owned(),
                    });
                    settled.notice = Some(Notice {
                        text: format!("{APP_NAME} {} is what's installed, and {why}.", outcome.to),
                        fades: true,
                    });
                    self.log(&format!("{} didn't come up, and couldn't go back: {}", outcome.to, clean_code(&outcome.why)));
                }
                OutcomeKind::NotInstalled => {
                    saved.install_failures = saved.install_failures.saturating_add(1);
                    saved.quits_for_nothing = saved.quits_for_nothing.saturating_add(1);
                    saved.last_install_ms = Some(now);
                    let (why, lasting) = why_not_installed(&outcome.why);
                    // Whatever it was, the app went away and came back for nothing, and the user may have seen that. Say so, and don't do it for ever.
                    if lasting || saved.quits_for_nothing >= QUITS_FOR_NOTHING {
                        saved.note_failed_version(&outcome.to);
                    }
                    settled.status = Some(Status::Failed {
                        version: outcome.to.clone(),
                        why: why.to_owned(),
                    });
                    settled.notice = Some(Notice {
                        text: format!("Couldn't update to {}: {why}. {APP_NAME} is unchanged.", outcome.to),
                        fades: true,
                    });
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
            settled.notice = Some(Notice {
                text: builds::arrived_words(&plan),
                fades: true,
            });
            saved.offered = None;
            saved.install_failures = 0;
            saved.quits_for_nothing = 0;
            saved.failures = 0;
            if !alive {
                self.tidy_after_swap(&plan, &beside);
            }
            settled.arrived = Some(plan);
        } else if alive {
            // Reopened by someone in the moment it was gone, with the installer still about. It stands down when it sees this, and there's nothing here to wait for: this copy goes on as normal, and the next launch hears how it ended.
            self.log("the app was reopened while an installer was at work");
        } else {
            // The installer was stopped before it swapped, so this is the old app, and there's no harm done.
            saved.install_failures = saved.install_failures.saturating_add(1);
            saved.quits_for_nothing = saved.quits_for_nothing.saturating_add(1);
            saved.last_install_ms = Some(now);
            if saved.quits_for_nothing >= QUITS_FOR_NOTHING {
                saved.note_failed_version(&plan.to);
            }
            settled.status = Some(Status::Failed {
                version: plan.to.clone(),
                why: "the installer stopped before it finished".to_owned(),
            });
            settled.notice = Some(Notice {
                text: format!(
                    "Couldn't update to {}: the installer stopped before it finished. {APP_NAME} is unchanged.",
                    plan.to
                ),
                fades: true,
            });
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
            keep_if_nothing_is(beside, &left);
            // It couldn't be moved, so where it is it's the only way back, and it stays.
            if stage::app_version(&previous_app_in(beside)).is_none() {
                let _ = std::fs::remove_file(self.files.plan());
                let _ = std::fs::remove_file(self.files.helper_pid());
                return;
            }
        }
        let _ = std::fs::remove_dir_all(beside.join("new"));
        let _ = std::fs::remove_file(beside.join("staged.json"));
        // The record's copy stays until this version's come up: it's what going back would put back.
        let _ = std::fs::remove_file(self.files.plan());
        let _ = std::fs::remove_file(self.files.helper_pid());
    }

    /// A launch of a version that's come up after an update and is being watched. The last run's counted if it ended early, and enough of those in a row go back. Otherwise this run is the one being watched. Says whether it's going back.
    fn probation_at_launch(&self, saved: &mut Saved, current: &str, app: &Path, now: u64) -> bool {
        let Some(probation) = saved.probation.as_mut() else { return false };
        // Another version's here now, by hand or by an update, or it's been a week: there's nothing to watch.
        if probation.version != current || now.saturating_sub(probation.since_ms) > KEEP_RECORD_COPY_MS {
            saved.probation = None;
            return false;
        }
        if probation.running_since_ms.take().is_some() {
            probation.early_ends = probation.early_ends.saturating_add(1);
            self.log(&format!("{current} ended early, {} in a row", probation.early_ends));
        }
        if probation.early_ends >= EARLY_ENDS_LIMIT && !helper_alive(&self.files) && self.start_going_back(saved, current, app, now, Back::KeptEnding) {
            return true;
        }
        if let Some(probation) = saved.probation.as_mut() {
            probation.running_since_ms = Some(now);
        }
        false
    }

    /// The run being watched has lasted long enough, by the timer: the count of early ends starts again. Says whether that's changed what's remembered.
    fn probation_lasted(&self, now: u64) -> bool {
        let mut st = lock(&self.st);
        let current = st.facts.as_ref().map(|facts| facts.version.clone());
        match st.saved.probation.as_mut() {
            Some(probation)
                if Some(&probation.version) == current.as_ref() && probation.running_since_ms.is_some_and(|at| now >= at.saturating_add(EARLY_END_MS)) =>
            {
                probation.running_since_ms = None;
                probation.early_ends = 0;
                true
            }
            _ => false,
        }
    }

    /// When the run being watched will have lasted long enough, if one is.
    fn probation_wake(st: &St) -> Option<u64> {
        let probation = st.saved.probation.as_ref()?;
        let watched = st.facts.as_ref().is_some_and(|facts| facts.version == probation.version);
        watched.then_some(probation.running_since_ms?).map(|at| at.saturating_add(EARLY_END_MS))
    }

    /// The app's quitting because someone asked it to, with everything written: this run didn't end early, whenever it ends. Writes at once, as the process is about to go, so it's for the quit's own thread.
    pub fn quitting_as_asked(&self) {
        let changed = {
            let mut st = lock(&self.st);
            let current = st.facts.as_ref().map(|facts| facts.version.clone());
            match st.saved.probation.as_mut() {
                Some(probation) if Some(&probation.version) == current.as_ref() && probation.running_since_ms.is_some() => {
                    probation.running_since_ms = None;
                    probation.early_ends = 0;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.save();
        }
    }

    /// The quit didn't go ahead after all, as someone chose to stay: this run's watched again.
    pub fn quit_not_taken(&self) {
        let now = self.now_ms();
        let changed = {
            let mut st = lock(&self.st);
            let current = st.facts.as_ref().map(|facts| facts.version.clone());
            match st.saved.probation.as_mut() {
                Some(probation) if Some(&probation.version) == current.as_ref() && probation.running_since_ms.is_none() => {
                    probation.running_since_ms = Some(now);
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.save();
        }
    }

    /// This version keeps failing, before its first frame or after it, and an older one's kept: starts the installer to go back to it.
    fn start_going_back(&self, saved: &mut Saved, current: &str, app: &Path, now: u64, why: Back) -> bool {
        let Some(beside) = beside_app(app) else { return false };
        // Older, or after a development build was handed over, any other version: never this one.
        let Some(previous) = stage::app_version(&previous_app_in(&beside)).filter(|previous| previous != current && crate::version::parse(previous).is_some())
        else {
            return false;
        };
        // Nobody's there to say: only a release of the team's, or a build the owner allowed, is put back by itself.
        if !self.may_go_back_to(&previous_app_in(&beside), saved, app) {
            self.log(&format!("{current} keeps failing, and the kept {previous} isn't one to go back to by itself"));
            return false;
        }
        let plan = Plan {
            id: crate::plan::new_id(),
            cdhash: String::new(),
            team_required: false,
            any_version: !is_newer(current, &previous),
            development_build: false,
            team_signed: false,
            from: previous,
            to: current.to_owned(),
            pid: std::process::id(),
            app: app.to_path_buf(),
            beside,
            root: self.files.root.clone(),
            quit_at_ms: now,
            // Someone launched this one, or the system did for them: what comes back comes forward, with what they asked it to open.
            background: false,
            quit_limit_ms: crate::plan::QUIT_LIMIT_MS,
            start_limit_ms: crate::plan::START_LIMIT_MS,
            handed_off_tabs: None,
            open: self.host.documents_asked_for(),
            keep_record: why == Back::KeptEnding,
            test: test_launch(&self.source, self.host.relaunch()),
        };
        self.ready_folder();
        if plan.write(&self.files.plan()).is_err() {
            return false;
        }
        saved.unstarted = 0;
        saved.probation = None;
        saved.write(&self.files.saved);
        self.log(&match why {
            Back::NeverCameUp => format!("{current} didn't reach a first frame {UNSTARTED_LIMIT} times, going back"),
            Back::KeptEnding => format!("{current} ended early {EARLY_ENDS_LIMIT} times in a row, going back with the record as it is"),
        });
        local::spawn_detached(
            &app.join(HELPER),
            &["back".to_owned(), self.files.plan().display().to_string()],
            &[],
            &self.files.log,
        )
        .is_ok()
    }

    /// The first correct frame's on the screen. **What "come up" means, which is what the installer waits for and what can send a new version back:** the state's been read and a first frame shown, and this version has drawn a page through the real renderer: our own tiny PDF, not the user's. Not the user's, as one of theirs being moved, damaged or on a dead volume says nothing of this version. And not the first frame alone, which a timer can force and a preview from a sleep file can fill without the renderer ever running. So a version whose renderer can't load, which would otherwise look fine, shows no pages for everyone, and be the one nobody can get back from, goes back. It's asked only of a version that's new here, so a normal launch costs nothing, and this waits on a worker, so it has a thread of its own.
    pub fn first_frame(&self, shown_at_ms: u64) {
        let (version, arrived, to_prove, second_copy, kept) = {
            let mut st = lock(&self.st);
            if st.first_frame_done {
                return;
            }
            let Some(facts) = st.facts.as_ref() else {
                st.first_frame_early = Some(shown_at_ms);
                return;
            };
            let version = facts.version.clone();
            let beside = beside_app(&facts.app).unwrap_or_default();
            st.first_frame_done = true;
            let arrived = st.arrived.take();
            let new_here = self.source.may_replace_itself() && st.saved.proven.as_deref() != Some(version.as_str());
            let kept = previous_app_in(&beside).is_dir();
            (version, arrived.clone(), new_here && (arrived.is_some() || kept), st.second_copy, kept)
        };
        if second_copy {
            return;
        }
        if to_prove {
            self.ready_folder();
        }
        if to_prove && !self.host.can_draw() {
            self.log(&format!("{version} can't draw a page"));
            // Without the word from here the installer would find out only by waiting. With no installer, the launches that don't come up are what count.
            if arrived.is_some() {
                self.ready_folder();
                let _ = local::write_private_file(&self.files.launch_failed(), version.as_bytes());
            }
            return;
        }
        let mut watched = false;
        let changed = {
            let mut st = lock(&self.st);
            let mut changed = st.saved.unstarted != 0;
            st.saved.unstarted = 0;
            if self.source.may_replace_itself() && st.saved.proven.as_deref() != Some(version.as_str()) {
                st.saved.proven = Some(version.clone());
                changed = true;
                // Come up for the first time, with the version before it kept: watched now for runs that end early, starting with this one.
                if kept {
                    watched = true;
                    st.saved.probation = Some(Probation {
                        version: version.clone(),
                        since_ms: shown_at_ms,
                        running_since_ms: Some(st.launched_ms),
                        early_ends: 0,
                    });
                }
            }
            if let Some(plan) = &arrived {
                st.saved.last_update = Some(LastUpdate {
                    from: plan.from.clone(),
                    to: plan.to.clone(),
                    gap_ms: shown_at_ms.saturating_sub(plan.quit_at_ms),
                    at_ms: shown_at_ms,
                });
                changed = true;
            }
            changed
        };
        if let Some(plan) = arrived {
            self.ready_folder();
            let line = crate::plan::launch_ok_line(&version, std::process::id(), shown_at_ms, self.host.tabs_restored());
            let _ = local::write_private_file(&self.files.launch_ok(), line.as_bytes());
            self.log(&format!(
                "updated from {} to {}: {} ms from quit to first frame",
                plan.from,
                plan.to,
                shown_at_ms.saturating_sub(plan.quit_at_ms)
            ));
        }
        if changed {
            self.save();
        }
        // The timer comes for when this first run's lasted, which the host arms once it's told.
        if watched {
            self.host.changed();
        }
    }

    /// The menu item. Rust's words, whichever way things stand. An offer stays on it through whatever else has happened: a check that failed last night, or an install that didn't work, which by hand can always be tried again.
    #[must_use]
    pub fn menu_item(&self) -> MenuItem {
        let st = lock(&self.st);
        match &st.status {
            Status::Preparing(version) | Status::Waiting { version, .. } | Status::Installing(version) => {
                return MenuItem {
                    title: format!("Updating to {version}…"),
                    enabled: false,
                };
            }
            Status::Checking => {
                return MenuItem {
                    title: "Check for Updates…".to_owned(),
                    enabled: false,
                };
            }
            _ => {}
        }
        if let Some(build) = &st.build {
            return MenuItem {
                title: format!("Update to {}…", build.version),
                enabled: true,
            };
        }
        match self.offered_here(&st) {
            Some(version) => MenuItem {
                title: format!("Update to {version}…"),
                enabled: true,
            },
            None => MenuItem {
                title: "Check for Updates…".to_owned(),
                enabled: true,
            },
        }
    }

    /// The menu item was chosen. Whichever it says: it starts on a thread of its own, and the UI's not held.
    pub fn menu_chosen(self: &Arc<Self>) {
        let (on_offer, build) = {
            let mut st = lock(&self.st);
            // Only what this starts: something already under way was someone else's to ask for.
            if st.status.is_under_way() {
                return;
            }
            st.from_menu = true;
            (self.offered_here(&st).is_some(), st.build.is_some())
        };
        // A development build waiting goes at once, by hand.
        let started = if build {
            self.build_now(Asked::ByHand);
            true
        } else if on_offer {
            self.install(Asked::ByHand).is_ok()
        } else {
            self.check_in_background(Asked::ByHand)
        };
        if !started {
            lock(&self.st).from_menu = false;
        }
    }

    /// What's been said of something chosen from the menu, to show as an alert when there's no window to say it in. Taken once.
    #[must_use]
    pub fn take_alert(&self) -> Option<String> {
        lock(&self.st).alert.take()
    }

    /// The answer to what's under way, said by hand: in the margin, and as an alert too if it was chosen from the menu.
    fn answer_by_hand(st: &mut St, text: String) {
        if std::mem::take(&mut st.from_menu) {
            st.alert = Some(text.clone());
        }
        st.notice = Some(Notice { text, fades: true });
    }

    /// When the host's timer should next go off, by this clock. One timer, for this and nothing else, and none if there's nothing to wake for. It changes only when something's happened, not with time or input.
    #[must_use]
    pub fn next_wake(&self) -> Option<u64> {
        let st = lock(&self.st);
        if st.facts.is_none() || st.status.is_under_way() || st.second_copy {
            return None;
        }
        // While a build waits, the check waits for it, so it doesn't set the timer either: a check that's due and can't go would wake it every second.
        let check = (self.source.checks_by_itself() && st.build.is_none()).then(|| rules::next_check_at(&st.saved, st.launched_ms, self.now_ms()));
        [check, st.install_look_at, Self::probation_wake(&st)].into_iter().flatten().min()
    }

    /// The timer's gone off. Does what's due and leaves the next time to [`Self::next_wake`]: a timer that goes off early or late does no harm.
    pub fn timer_fired(self: &Arc<Self>) {
        let now = self.now_ms();
        if self.probation_lasted(now) {
            self.save_soon();
        }
        let (check_due, install_due) = {
            let st = lock(&self.st);
            if st.status.is_under_way() || st.facts.is_none() {
                return;
            }
            // A build that's waiting goes first, and the check comes after: it's what's to be said till then.
            let check_due = self.source.checks_by_itself() && st.build.is_none() && rules::next_check_at(&st.saved, st.launched_ms, now) <= now;
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
        if lock(&self.st).build.is_some() {
            self.look_at_build(now);
            return;
        }
        let look = self.host.look();
        let since_input_ms = self.since_input(&look);
        let step = {
            let mut st = lock(&self.st);
            let offered = self.offered_here(&st);
            // The way back from a development build goes by itself, whichever way the switch is: it's what turning development builds off asked for.
            let on = st.saved.auto_update.unwrap_or_else(|| self.source.auto_default()) || st.saved.back_to_release;
            let step = rules::auto_step(AutoLook {
                build_may: self.source.may_replace_itself(),
                switch_on: on,
                offered: offered.is_some(),
                version_failed: offered.as_ref().is_some_and(|version| st.saved.failed.contains(version)),
                since_input_ms,
                busy: !look.busy.is_empty(),
                back_off_ms: st.saved.install_back_off(now),
            });
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
    /// Says whether it started: one's already under way otherwise.
    pub fn check_in_background(self: &Arc<Self>, asked: Asked) -> bool {
        if !self.begin_check() {
            return false;
        }
        let me = Arc::clone(self);
        let started = std::thread::Builder::new()
            .name("recto-update-check".into())
            .stack_size(CHECK_STACK)
            .spawn(move || me.finish_check(asked));
        if started.is_err() {
            self.finish_check_with(asked, Err(FetchError::Failed("no thread could be started")));
        }
        true
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
        // A build that's waiting is what's to be said until it goes, and a check would only cover it up.
        if st.status.is_under_way() || st.facts.is_none() || st.build.is_some() {
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
                if latest.on_offer(&current, &system) || (st.saved.back_to_release && latest.on_offer_back(&current, &system)) {
                    st.saved.offered = Some(latest.version.clone());
                    if self.source.may_replace_itself() {
                        st.status = Status::Available(latest.version.clone());
                        // A newer one than last time has its own chance, whatever became of that.
                        st.install_look_at = Some(now);
                        format!("{APP_NAME} {0} is available. Choose Update to {0}… in the {APP_NAME} menu.", latest.version)
                    } else {
                        st.status = Status::AvailableElsewhere(latest.version.clone());
                        format!("{APP_NAME} {} is available. This is a local build, so it won't replace itself.", latest.version)
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
                // What's already known doesn't go away because a check couldn't get through: an offline night leaves the offer on the menu.
                st.status = match self.offered_here(&st) {
                    Some(version) => Status::Available(version),
                    None => Status::CheckFailed(why.clone()),
                };
                failure = Some(why.clone());
                format!("Couldn't check for updates: {why}.")
            }
        };
        if asked == Asked::ByHand {
            Self::answer_by_hand(&mut st, words);
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
            if st.status.is_under_way() || st.build.is_some() {
                return Err(InstallRefused::Underway);
            }
            if st.second_copy {
                return Err(InstallRefused::AnotherCopy);
            }
            let Some(version) = self.offered_here(&st) else {
                return Err(InstallRefused::NothingOnOffer);
            };
            st.status = Status::Preparing(version.clone());
            st.saved.last_install_ms = Some(self.now_ms());
            if asked == Asked::ByHand {
                st.notice = Some(Notice {
                    text: format!("Downloading {APP_NAME} {version}…"),
                    fades: false,
                });
            }
            version
        };
        self.host.changed();
        let me = Arc::clone(self);
        let thread_version = version.clone();
        let started = std::thread::Builder::new()
            .name("recto-update-install".into())
            .stack_size(CHECK_STACK)
            .spawn(move || me.run_install(asked, &thread_version));
        if started.is_err() {
            self.failed(asked, &version, "no thread could be started", false);
        }
        Ok(version)
    }

    fn failed(&self, asked: Asked, version: &str, why: &str, lasting: bool) {
        let now = self.now_ms();
        let mut st = lock(&self.st);
        st.status = Status::Failed {
            version: version.to_owned(),
            why: why.to_owned(),
        };
        st.saved.install_failures = st.saved.install_failures.saturating_add(1);
        st.saved.last_install_ms = Some(now);
        if lasting {
            st.saved.note_failed_version(version);
        }
        // A build that didn't go is handed over again if it's wanted: nothing's tried again by itself.
        let build_gone = st.build.as_ref().is_some_and(|build| build.version == version);
        if build_gone {
            st.build = None;
        }
        // A failure that may pass gets another chance after a while, by the back-off.
        st.install_look_at = (!lasting && self.offered_here(&st).is_some()).then(|| now.saturating_add(st.saved.install_back_off(now)));
        st.notice = None;
        if asked == Asked::ByHand {
            Self::answer_by_hand(&mut st, format!("Couldn't update to {version}: {why}. {APP_NAME} is unchanged."));
        }
        drop(st);
        if build_gone && let Some(app) = self.app() {
            stage::clear_staged(&app);
        }
        self.save();
        self.log(&format!("the update to {version} failed: {why}"));
        self.host.changed();
    }

    fn back_on_offer(&self, version: &str, look_at: Option<u64>) {
        let mut st = lock(&self.st);
        st.status = if st.build.as_ref().is_some_and(|build| build.version == version) {
            Status::BuildReady(version.to_owned())
        } else {
            Status::Available(version.to_owned())
        };
        st.install_look_at = look_at;
        st.notice = None;
        st.from_menu = false;
        drop(st);
        self.host.changed();
    }

    /// Whether an automatic install is still right to go ahead: the user may have come back during a download.
    fn still_idle(&self, look: &Look, idle_ms: u64) -> bool {
        self.since_input(look) >= idle_ms
    }

    fn run_install(self: &Arc<Self>, asked: Asked, version: &str) {
        let Some(facts) = lock(&self.st).facts.clone() else { return };
        self.ready_folder();
        self.log(&format!("updating to {version}"));
        // Read again, so one that's been withdrawn doesn't go in, and the checksum's today's.
        let way_back = lock(&self.st).saved.back_to_release;
        let latest = match self.fetch_latest(&facts.version) {
            Ok(latest) if latest.on_offer(&facts.version, &facts.system) || (way_back && latest.on_offer_back(&facts.version, &facts.system)) => latest,
            Ok(_) | Err(FetchError::NotFound) => {
                let mut st = lock(&self.st);
                st.saved.offered = None;
                st.status = Status::UpToDate;
                st.install_look_at = None;
                st.notice = None;
                if asked == Asked::ByHand {
                    Self::answer_by_hand(
                        &mut st,
                        format!("{version} isn't on offer any more. You're up to date: {APP_NAME} {}.", facts.version),
                    );
                }
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
        let stage_in = Stage {
            source: &self.source,
            latest: &latest,
            current: &facts.version,
            app: &facts.app,
            files: &self.files,
            user_agent: &user_agent,
        };
        let log_file = self.files.log.clone();
        let staged = match stage::prepare(&stage_in, &mut |step| log(&log_file, &format!("{version}: {step}"))) {
            Ok(staged) => staged,
            Err(StageError { code, why, lasting }) => {
                self.log(&format!("{version} was refused: {code}"));
                if lasting {
                    stage::clear_staged(&facts.app);
                }
                self.failed(asked, version, &why, lasting);
                return;
            }
        };
        let to = Going {
            version,
            cdhash: &staged.cdhash,
            idle_ms: rules::IDLE_BEFORE_INSTALL_MS,
            any_version: way_back && !is_newer(version, &facts.version),
            development_build: false,
            team_signed: true,
        };
        self.wait_and_hand_off(asked, &facts, &to);
    }

    /// The download's good. What's in flight finishes first, and the user's told if they're waiting on it. Then the hand-off and the quit: from here, what's written is what comes back.
    fn wait_and_hand_off(self: &Arc<Self>, asked: Asked, facts: &Facts, to: &Going<'_>) {
        let version = to.version;
        let began = Instant::now();
        loop {
            // The user's quitting by other means, and this doesn't turn that into a reopening.
            if self.stopping.load(Ordering::SeqCst) {
                self.back_on_offer(version, None);
                return;
            }
            // A build was handed over at Full: if access has dropped since, it doesn't go in, as no one at the level that asked for it is there to.
            if to.development_build && !self.host.may_take_a_build() {
                self.failed(
                    asked,
                    version,
                    "agent access isn't Full any more, and a development build is installed only at Full",
                    false,
                );
                return;
            }
            let look = self.host.look();
            if asked == Asked::ByItself && !self.still_idle(&look, to.idle_ms) {
                // They've come back. It's all ready for the next quiet half hour.
                let wait = to.idle_ms.saturating_sub(self.since_input(&look));
                self.back_on_offer(version, Some(self.now_ms().saturating_add(wait)));
                return;
            }
            if look.busy.is_empty() {
                match self.host.hand_off(HAND_OFF_LIMIT) {
                    Ok(handed) => {
                        self.quit_after_hand_off(asked, facts, to, handed);
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
                    st.status = Status::Waiting {
                        version: version.to_owned(),
                        for_what: for_what.clone(),
                    };
                    st.notice = Some(Notice {
                        text: format!("Updating to {version} as soon as this is done: {for_what}"),
                        fades: false,
                    });
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
    fn quit_after_hand_off(self: &Arc<Self>, asked: Asked, facts: &Facts, to: &Going<'_>, handed: HandedOff) {
        let (version, cdhash) = (to.version, to.cdhash);
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
        if asked == Asked::ByItself && !self.still_idle(&look, to.idle_ms) {
            self.host.cancelled();
            let wait = to.idle_ms.saturating_sub(self.since_input(&look));
            self.back_on_offer(version, Some(self.now_ms().saturating_add(wait)));
            return;
        }
        let Some(beside) = beside_app(&facts.app) else {
            give_up("this copy isn't in a folder");
            return;
        };
        let team_required = matches!(self.source.trust, crate::source::Trust::DeveloperId { .. });
        let start_limit_ms = if self.source.mode == Mode::Test {
            TEST_START_LIMIT_MS
        } else {
            crate::plan::START_LIMIT_MS
        };
        let plan = Plan {
            id: crate::plan::new_id(),
            cdhash: cdhash.to_owned(),
            team_required,
            any_version: to.any_version,
            development_build: to.development_build,
            team_signed: to.team_signed,
            from: facts.version.clone(),
            to: version.to_owned(),
            pid: std::process::id(),
            app: facts.app.clone(),
            beside,
            root: self.files.root.clone(),
            quit_at_ms: crate::wall_ms(),
            background: !self.host.frontmost(),
            quit_limit_ms: crate::plan::QUIT_LIMIT_MS,
            start_limit_ms,
            handed_off_tabs: handed.tabs,
            open: Vec::new(),
            keep_record: false,
            test: test_launch(&self.source, self.host.relaunch()),
        };
        let written = plan.write(&self.files.plan());
        if written.is_err() {
            give_up("the update's plan couldn't be written");
            return;
        }
        {
            let mut st = lock(&self.st);
            st.status = Status::Installing(version.to_owned());
            st.notice = None;
            st.from_menu = false;
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
            Status::UpToDate | Status::NoReleases => st.saved.last_check_ms.map_or_else(
                || format!("{APP_NAME} {version} is up to date"),
                |at| format!("{APP_NAME} {version} is up to date · checked {}", ago(at, now)),
            ),
            Status::Available(offered) => format!("{APP_NAME} {offered} is available"),
            Status::AvailableElsewhere(offered) => format!("{APP_NAME} {offered} is available · a local build doesn't replace itself"),
            Status::CheckFailed(why) => format!("Couldn't check for updates: {why}"),
            Status::Preparing(offered) => format!("Downloading {APP_NAME} {offered}…"),
            Status::Waiting { version: offered, for_what } => format!("Updating to {offered} as soon as this is done: {for_what}"),
            Status::Installing(offered) => format!("Installing {APP_NAME} {offered}…"),
            Status::Failed { version: offered, why } => format!("Update to {offered} failed: {why}"),
            Status::Updated(offered) => format!("Updated to {offered}"),
            Status::WentBack { version: offered, back_on } => format!("{offered} didn't start, so {back_on} came back"),
            Status::BuildReady(build) => format!("Development build {build} is ready, and goes in once {APP_NAME} hasn't been used for 5 minutes"),
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
        let kept = app
            .as_deref()
            .and_then(beside_app)
            .map(|beside| previous_app_in(&beside))
            .filter(|previous| previous.is_dir())
            .and_then(|previous| stage::app_version(&previous));
        let st = lock(&self.st);
        json!({
            "version": st.facts.as_ref().map(|facts| facts.version.clone()),
            "build": self.source.mode_name(),
            "status": st.status.code(),
            "words": line,
            "menu": { "title": menu.title, "enabled": menu.enabled },
            "offered": st.saved.offered,
            "auto_update": on,
            "second_copy": st.second_copy,
            "failed_versions": st.saved.failed,
            "last_check_ms": st.saved.last_check_ms,
            "last_update": st.saved.last_update.as_ref().map(|update| json!({ "from": update.from, "to": update.to, "gap_ms": update.gap_ms, "at_ms": update.at_ms })),
            "next_wake_in_ms": next_wake.map(|at| at.saturating_sub(now)),
            "install_look_in_ms": st.install_look_at.map(|at| at.saturating_sub(now)),
            "kept": kept,
            "dev_builds": st.saved.dev_builds.unwrap_or(false),
            "back_to_release": st.saved.back_to_release,
            "build_waiting": st.build.as_ref().map(|build| build.version.clone()),
            "build_team": st.build.as_ref().and_then(|build| build.team.clone()),
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

/// What an install is going to, once it's been checked: a release downloaded, or a development build handed over.
struct Going<'a> {
    version: &'a str,
    cdhash: &'a str,
    /// How long the app has to have gone unused, for an install that goes by itself: a release's 30 minutes, or a development build's 5.
    idle_ms: u64,
    /// Any other version may replace this one, older included ([`Plan::any_version`]).
    any_version: bool,
    /// A development build handed over ([`Plan::development_build`]), and whether a team signed it.
    development_build: bool,
    team_signed: bool,
}

/// What's said once a version that didn't work has gone back, by why.
fn went_back_words(why: &str, version: &str, current: &str) -> String {
    match why {
        "kept_ending" => {
            format!("{APP_NAME} {version} kept quitting unexpectedly, so {current} came back, with your tabs and edits as {version} last had them.")
        }
        "misread" => format!("{APP_NAME} {version} didn't read your last session properly, so {current} came back, just as you left it."),
        _ => format!("{APP_NAME} {version} didn't start, so {current} came back, just as you left it."),
    }
}

/// Whether the installer opened the app again after not installing, for this reason. For the others the old app was never quit, or someone else opened it.
fn installer_reopened(code: &str) -> bool {
    matches!(code, "swap_failed" | "not_permitted" | "changed_since_checked" | "new_app_missing")
}

/// What each of the installer's reasons comes to, and whether it's the sort of thing that'll be the same tomorrow.
fn why_not_installed(code: &str) -> (&'static str, bool) {
    match code {
        "swap_failed" => ("the app couldn't be replaced where it is", true),
        // Not lasting: once it's allowed, it works.
        "not_permitted" => (
            "macOS didn't allow RectoPDF to change itself. Allow it in System Settings › Privacy & Security › App Management, then update again",
            false,
        ),
        "changed_since_checked" => ("what was downloaded changed after it was checked", true),
        "old_app_stayed" => ("the old app didn't quit in time", false),
        "app_reopened" => ("the app was opened again while it was going", false),
        "another_installer" => ("another update was still at work", false),
        _ => ("it couldn't be put in place", false),
    }
}

/// Moves an old app to where it's kept, if there's nothing kept that's any use.
fn keep_if_nothing_is(beside: &Path, old_app: &Path) {
    let keep = previous_app_in(beside);
    if stage::app_version(&keep).is_some() {
        return;
    }
    if let Some(folder) = keep.parent() {
        if std::fs::remove_dir_all(folder).is_err() {
            let _ = std::fs::remove_file(folder);
        }
        let _ = std::fs::create_dir_all(folder);
        let _ = std::fs::rename(old_app, &keep);
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
    use crate::stage::tests::{fake_app, fake_app_at, latest_json, temp_dir, zip_of};
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
        /// What the time since input is on each look, for a user who comes back at a given moment. Past the list, `since_input_ms`.
        looks: Mutex<std::collections::VecDeque<u64>>,
        cant_draw: AtomicBool,
        draws: Mutex<u32>,
        second_copy: AtomicBool,
    }

    impl Host for Fake {
        fn look(&self) -> Look {
            let scripted = lock(&self.looks).pop_front();
            Look {
                busy: lock(&self.busy).clone(),
                since_input_ms: scripted.unwrap_or(*lock(&self.since_input_ms)),
            }
        }
        fn hand_off(&self, _limit: Duration) -> Result<HandedOff, HandOffRefused> {
            self.hand_offs.fetch_add(1, Ordering::SeqCst);
            match lock(&self.refuse_with).clone() {
                Some(refused) => Err(refused),
                None => Ok(HandedOff { tabs: Some(2) }),
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
        fn keeps_the_record(&self) -> bool {
            !self.second_copy.load(Ordering::SeqCst)
        }
        fn can_draw(&self) -> bool {
            *lock(&self.draws) += 1;
            !self.cant_draw.load(Ordering::SeqCst)
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
            let app = fake_app(&install, "0.1.0");
            let server = Server::start();
            let fake = Arc::new(Fake::default());
            let updater = Updater::new(&root, server.source(), Arc::clone(&fake) as Arc<dyn Host>);
            assert_eq!(
                updater.launched(Facts {
                    version: "0.1.0".into(),
                    app: app.clone(),
                    system: "15.0".into()
                }),
                AtLaunch::GoOn
            );
            Self {
                dir,
                server,
                fake,
                updater,
                app,
            }
        }

        fn publish(&self, version: &str) {
            let built = self.dir.join(format!("r-{version}"));
            std::fs::create_dir_all(&built).unwrap();
            let app = fake_app(&built, version);
            let (bytes, sha) = zip_of(&app, &built.join("r.zip"));
            self.server
                .set(&format!("/releases/download/v{version}/RectoPDF-{version}.zip"), Answer::Body(bytes));
            self.server
                .set("/releases/latest/download/latest.json", Answer::Body(latest_json(version, &sha)));
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
        assert_eq!(
            bed.updater.menu_item(),
            MenuItem {
                title: "Check for Updates…".into(),
                enabled: true
            }
        );
        assert_eq!(bed.updater.check_here(Asked::ByHand), Status::NoReleases);
        assert!(bed.updater.notice().unwrap().text.contains("up to date"), "{:?}", bed.updater.notice());
        assert_eq!(bed.updater.menu_item().title, "Check for Updates…");
        bed.publish("0.1.0");
        assert_eq!(bed.updater.check_here(Asked::ByHand), Status::UpToDate);
        assert!(bed.updater.notice().unwrap().text.starts_with("You're up to date"));
        bed.publish("0.2.0");
        bed.found("0.2.0");
        assert_eq!(
            bed.updater.menu_item(),
            MenuItem {
                title: "Update to 0.2.0…".into(),
                enabled: true
            }
        );
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
        again.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert_eq!(again.menu_item().title, "Update to 0.2.0…");
        // An offer the same as this version, once it's installed, isn't one.
        let after = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        after.launched(Facts {
            version: "0.2.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert_eq!(after.menu_item().title, "Check for Updates…");
    }

    /// Failure: a check chosen from the menu with the window closed says how it went only in Settings, which may not be open, so the user sees nothing (PLAN, "A check by hand with the window closed"). What's chosen from the menu leaves its answer for an alert, once; an agent's check, which has its answer in the command's, and a check by itself leave none. So does an install from the menu that fails.
    #[test]
    fn what_the_menu_starts_leaves_its_answer_for_an_alert() {
        let bed = Bed::new("alert");
        bed.publish("0.1.0");
        bed.updater.menu_chosen();
        bed.wait_for("the check", |bed| !lock(&bed.updater.st).status.is_under_way());
        let alert = bed.updater.take_alert().expect("the menu's check leaves its answer");
        assert!(alert.starts_with("You're up to date"), "{alert}");
        assert_eq!(bed.updater.take_alert(), None, "taken once");
        assert_eq!(bed.updater.check_here(Asked::ByHand), Status::UpToDate);
        assert!(bed.updater.notice().is_some(), "an agent's check still says so in the margin");
        assert_eq!(bed.updater.take_alert(), None, "and puts up nothing");
        assert!(bed.updater.check_in_background(Asked::ByItself));
        bed.wait_for("the check by itself", |bed| !lock(&bed.updater.st).status.is_under_way());
        assert_eq!(bed.updater.take_alert(), None);
        // An install from the menu that can't go ahead says why.
        bed.publish("0.2.0");
        bed.found("0.2.0");
        bed.server.set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Status(500));
        bed.server
            .set("/releases/latest/download/latest.json", Answer::Body(latest_json("0.2.0", &"a".repeat(64))));
        bed.updater.menu_chosen();
        bed.wait_for("the install to fail", |bed| matches!(bed.status(), Status::Failed { .. }));
        let alert = bed.updater.take_alert().expect("the menu's install leaves its answer");
        assert!(alert.starts_with("Couldn't update to 0.2.0"), "{alert}");
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
        assert_eq!(
            bed.updater.install(Asked::ByHand),
            Err(InstallRefused::NothingOnOffer),
            "a downgrade can't be asked for"
        );
    }

    /// Failure: a development build replaces itself, or checks by itself. The owner's rule: development builds check only when asked and never replace themselves. It also answers a check by hand, against a server it's been told about only here.
    #[test]
    fn a_development_build_doesnt_replace_itself() {
        let dir = temp_dir("dev");
        let root = dir.join("state");
        local::ensure_private_dir(&root).unwrap();
        let fake = Arc::new(Fake::default());
        let source = Source {
            mode: Mode::Development,
            repo: "http://127.0.0.1:9".into(),
            trust: crate::source::Trust::AnySignature,
            network: false,
            test_env: Vec::new(),
        };
        let updater = Updater::new(&root, source, Arc::clone(&fake) as Arc<dyn Host>);
        let app = fake_app(&dir, "0.1.0");
        updater.launched(Facts {
            version: "0.1.0".into(),
            app,
            system: "15.0".into(),
        });
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
        assert_eq!(
            bed.updater.menu_item(),
            MenuItem {
                title: "Updating to 0.2.0…".into(),
                enabled: false
            }
        );
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
        assert_eq!(
            bed.updater.menu_item().title,
            "Update to 0.2.0…",
            "it can be asked again, and it's still on offer"
        );
        assert_eq!(
            lock(&bed.updater.st).saved.failed,
            Vec::<String>::new(),
            "this may pass, so it's not held against the version"
        );
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
        bed.server
            .set("/releases/download/v0.2.0/RectoPDF-0.2.0.zip", Answer::Body(b"not the file".to_vec()));
        bed.found("0.2.0");
        *lock(&bed.fake.since_input_ms) = rules::IDLE_BEFORE_INSTALL_MS;
        bed.updater.install(Asked::ByItself).unwrap();
        bed.wait_for("the failure", |bed| matches!(bed.status(), Status::Failed { .. }));
        assert_eq!(lock(&bed.updater.st).saved.failed, vec!["0.2.0".to_string()]);
        assert_eq!(bed.updater.notice(), None, "by itself, it says nothing");
        assert_eq!(lock(&bed.updater.st).install_look_at, None, "not looked at again");
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "the app never hears of it");
    }

    /// Failure: the 30-minute rule is broken: an install goes while the user's in the app, or doesn't go when they've been away. With a clock moved on rather than half an hour's sleep, by the same call the timer makes. Someone coming back later than the timer's look is the two tests after this.
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
        assert!(
            (rules::IDLE_BEFORE_INSTALL_MS - 300_000..=rules::IDLE_BEFORE_INSTALL_MS - 59_000).contains(&wait),
            "{wait}"
        );
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0);
        assert!(bed.updater.set_test_clock(i64::try_from(wait).unwrap() + 1_000), "a test's clock");
        // It's been touched again since, so the clock moving on doesn't count from the first touch: what counts is the time since the last.
        *lock(&bed.fake.since_input_ms) = 0;
        bed.updater.set_test_clock(i64::try_from(wait).unwrap() + 1_000);
        bed.updater.timer_fired();
        assert_eq!(
            bed.fake.hand_offs.load(Ordering::SeqCst),
            0,
            "there was input, so it's not been thirty quiet minutes"
        );
        // Now nobody's touched it for the whole half hour.
        *lock(&bed.fake.since_input_ms) = rules::IDLE_BEFORE_INSTALL_MS;
        bed.updater.set_test_clock(i64::try_from(rules::IDLE_BEFORE_INSTALL_MS).unwrap() * 2);
        bed.updater.timer_fired();
        bed.wait_for("the quit", |bed| bed.fake.quits.load(Ordering::SeqCst) == 1);
        assert!(
            lock(&bed.fake.plan_at_quit).clone().unwrap().background,
            "it was not frontmost, so it does not take the focus"
        );
    }

    /// Failure: an automatic install stays waiting at the same place when the switch goes off, or carries on while the user's come back. The switch stops it at once, whichever way it's set for the daily check.
    #[test]
    fn the_switch_stops_an_automatic_install_and_not_the_check() {
        let bed = Bed::new("switch");
        bed.publish("0.2.0");
        bed.updater.check_here(Asked::ByItself);
        bed.updater.choose_switch(false);
        *lock(&bed.fake.since_input_ms) = rules::IDLE_BEFORE_INSTALL_MS * 2;
        bed.updater.timer_fired();
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "off means it waits for the menu");
        assert_eq!(bed.updater.menu_item().title, "Update to 0.2.0…");
        assert!(bed.updater.next_wake().is_some(), "the daily check runs whichever way the switch is set");
        bed.updater.choose_switch(true);
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
            let saved = Saved {
                offered: offered.map(str::to_owned),
                ..Saved::default()
            };
            saved.write(&files.saved);
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            updater.launched(Facts {
                version: "0.1.0".into(),
                app: bed.app.clone(),
                system: "15.0".into(),
            });
        };
        again(Some("0.2.0"));
        assert!(!files.zip("0.2.0").exists() && !beside.join("unpacking").exists());
        assert!(new_app_in(&beside).exists(), "still on offer, so it's worth keeping");
        again(None);
        assert!(!new_app_in(&beside).exists() && !beside.join("staged.json").exists(), "and now it isn't");
    }

    /// Failure: a user who comes back while a release downloads has the app quit under them. The review found this test's forerunner claimed to cover it, and nothing changed the input during the download. The first look says it's been half an hour, as the timer saw. The next, when the slow part's done, says they've just touched it.
    #[test]
    fn someone_coming_back_during_the_download_stops_it() {
        let bed = Bed::new("back-download");
        bed.publish("0.2.0");
        bed.updater.check_here(Asked::ByItself);
        *lock(&bed.fake.since_input_ms) = 1_000;
        lock(&bed.fake.looks).extend([rules::IDLE_BEFORE_INSTALL_MS]);
        lock(&bed.updater.st).install_look_at = Some(bed.updater.now_ms());
        bed.updater.timer_fired();
        bed.wait_for("standing down", |bed| {
            matches!(bed.status(), Status::Available(_)) && lock(&bed.updater.st).install_look_at.is_some()
        });
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "not even a hand-off");
        assert!(
            lock(&bed.updater.st).install_look_at.unwrap() > bed.updater.now_ms() + 25 * 60_000,
            "and the next look is when it could be half an hour"
        );
        assert!(new_app_in(&beside_app(&bed.app).unwrap()).is_dir(), "it's all ready for then");
    }

    /// Failure: a user who touches the app in the moment before it froze has it quit under them, and it stays frozen. This is the look after the freeze, which nothing tested. What's in between is real: a real download, and the hand-off answered.
    #[test]
    fn someone_coming_back_as_it_freezes_unfreezes_it() {
        let bed = Bed::new("back-freeze");
        bed.publish("0.2.0");
        bed.updater.check_here(Asked::ByItself);
        let idle = rules::IDLE_BEFORE_INSTALL_MS;
        *lock(&bed.fake.since_input_ms) = 1_000;
        // The timer, then the look before the hand-off, then the one after it.
        lock(&bed.fake.looks).extend([idle, idle, 1_000]);
        lock(&bed.updater.st).install_look_at = Some(bed.updater.now_ms());
        bed.updater.timer_fired();
        bed.wait_for("the cancel", |bed| bed.fake.cancels.load(Ordering::SeqCst) == 1);
        assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 1);
        assert_eq!(bed.fake.quits.load(Ordering::SeqCst), 0);
        assert_eq!(lock(&bed.fake.installers).len(), 0, "no installer was started");
        bed.wait_for("the offer again", |bed| matches!(bed.status(), Status::Available(_)));
    }

    /// Failure: an offline night takes *Update to X…* off the menu until a check next gets through.
    #[test]
    fn a_failed_check_leaves_the_offer_on_the_menu() {
        let bed = Bed::new("offline");
        bed.publish("0.2.0");
        bed.found("0.2.0");
        bed.server.set("/releases/latest/download/latest.json", Answer::Status(503));
        assert_eq!(bed.updater.check_here(Asked::ByHand), Status::Available("0.2.0".into()));
        assert_eq!(bed.updater.menu_item().title, "Update to 0.2.0…");
        assert!(bed.updater.notice().unwrap().text.starts_with("Couldn't check"), "it says the check failed");
        assert!(lock(&bed.updater.st).saved.failures == 1, "and backs off");
    }

    /// Failure: the user's choice of off is lost with a record, or turned back on by a launch, so an app they told not to replaces itself anyway. The switch is in this updater's own file.
    #[test]
    fn the_switch_is_kept_in_the_updaters_own_file() {
        let bed = Bed::new("switch-file");
        assert!(bed.updater.switch_on(), "on, by default, in this build");
        bed.updater.choose_switch(false);
        let saved = bed.dir.join("state/updates.json");
        bed.wait_for("the write", |_| Saved::read(&saved).auto_update == Some(false));
        let again = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        again.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert!(!again.switch_on(), "a launch knows it, whatever's in the record");
        // And a damaged file errs towards not replacing anything.
        local::write_private_file(&saved, b"not json").unwrap();
        let damaged = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        damaged.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert!(!damaged.switch_on());
    }

    /// Failure: a new version that can't draw counts as having come up, because a first frame showed, so every copy moves to a release that shows no pages. **`first_frame` says what "come up" means.** It proves itself with a drawing, says so to the installer at once if it can't, and counts towards going back if there's no installer to hear. A normal launch asks nothing of the renderer.
    #[test]
    fn a_version_that_cant_draw_hasnt_come_up() {
        let bed = Bed::new("draw");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        let plan = Plan {
            id: "1-1".into(),
            cdhash: "a".repeat(40),
            team_required: false,
            any_version: false,
            development_build: false,
            team_signed: false,
            from: "0.1.0".into(),
            to: "0.2.0".into(),
            pid: 1,
            app: bed.app.clone(),
            beside: beside_app(&bed.app).unwrap(),
            root: root.clone(),
            quit_at_ms: 5,
            background: false,
            quit_limit_ms: 1,
            start_limit_ms: 1,
            handed_off_tabs: None,
            open: Vec::new(),
            keep_record: false,
            test: None,
        };
        local::ensure_private_dir(&files.work).unwrap();
        plan.write(&files.plan()).unwrap();
        bed.fake.cant_draw.store(true, Ordering::SeqCst);
        let launch = |version: &str| {
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            updater.launched(Facts {
                version: version.into(),
                app: bed.app.clone(),
                system: "15.0".into(),
            });
            updater
        };
        let updater = launch("0.2.0");
        updater.first_frame(100);
        assert!(files.launch_failed().exists(), "the installer hears");
        assert!(!files.launch_ok().exists(), "and doesn't hear otherwise");
        assert!(lock(&updater.st).saved.proven.is_none());
        // Mended: it comes up, and proven, it's not asked again.
        bed.fake.cant_draw.store(false, Ordering::SeqCst);
        std::fs::remove_file(files.launch_failed()).unwrap();
        plan.write(&files.plan()).unwrap();
        let updater = launch("0.2.0");
        updater.first_frame(100);
        assert!(files.launch_ok().exists());
        let asked = *lock(&bed.fake.draws);
        let updater = launch("0.2.0");
        updater.first_frame(100);
        assert_eq!(*lock(&bed.fake.draws), asked, "a normal launch asks nothing");
        assert_eq!(lock(&updater.st).saved.proven.as_deref(), Some("0.2.0"));
    }

    /// Failure: a new version that comes up and then crashes in a loop stays, as nothing after its first frame was watched, and the user can't use the app. Each run that ends within [`EARLY_END_MS`] of its launch, other than by a quit someone asked for, counts, and the launch after [`EARLY_ENDS_LIMIT`] in a row goes back to the kept version with the record as it is. A run that lasts, or a quit, starts the count again, and it's watched only for as long as the record from before is kept.
    #[test]
    fn a_new_version_that_keeps_ending_goes_back() {
        let bed = Bed::new("ending");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        let beside = beside_app(&bed.app).unwrap();
        fake_app_at(&previous_app_in(&beside), "0.1.0");
        let arrive = || {
            Plan {
                id: "1-1".into(),
                cdhash: "a".repeat(40),
                team_required: false,
                any_version: false,
                development_build: false,
                team_signed: false,
                from: "0.1.0".into(),
                to: "0.2.0".into(),
                pid: 1,
                app: bed.app.clone(),
                beside: beside.clone(),
                root: root.clone(),
                quit_at_ms: 5,
                background: false,
                quit_limit_ms: 1,
                start_limit_ms: 1,
                handed_off_tabs: None,
                open: Vec::new(),
                keep_record: false,
                test: None,
            }
            .write(&files.plan())
            .unwrap();
        };
        let launch = || {
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            let at = updater.launched(Facts {
                version: "0.2.0".into(),
                app: bed.app.clone(),
                system: "15.0".into(),
            });
            updater.first_frame(updater.now_ms());
            (updater, at)
        };
        let early_ends = |updater: &Updater| lock(&updater.st).saved.probation.as_ref().map(|probation| probation.early_ends);
        local::ensure_private_dir(&files.work).unwrap();
        arrive();
        let (updater, _) = launch();
        assert_eq!(early_ends(&updater), Some(0), "watched from its first frame");
        let wake = updater.next_wake().unwrap();
        assert!(wake <= updater.now_ms() + EARLY_END_MS, "the timer comes for when this run's lasted");
        for ended in 1..EARLY_ENDS_LIMIT {
            let (updater, at) = launch();
            assert_eq!((at, early_ends(&updater)), (AtLaunch::GoOn, Some(ended)), "each run that ended early counts");
        }
        let (_, at) = launch();
        assert_eq!(at, AtLaunch::Exit, "the launch after {EARLY_ENDS_LIMIT} goes back");
        let plan = Plan::read(&files.plan()).unwrap();
        assert!(plan.keep_record, "with the record as it is");
        assert_eq!((plan.from.as_str(), plan.to.as_str()), ("0.1.0", "0.2.0"));
        assert_eq!(Saved::read(&files.saved).probation, None);

        // Ends that aren't in a row don't add up: a run that lasts, by the timer, and a quit someone asked for.
        let _ = std::fs::remove_file(files.plan());
        Saved::default().write(&files.saved);
        arrive();
        let _ = launch();
        let (updater, _) = launch();
        assert_eq!(early_ends(&updater), Some(1));
        // Checked just now, so the timer starts no check, whose ending would write this copy's memory over the next launches'.
        lock(&updater.st).saved.last_check_ms = Some(updater.now_ms() + EARLY_END_MS);
        updater.set_test_clock(i64::try_from(EARLY_END_MS).unwrap() + 1_000);
        updater.timer_fired();
        assert_eq!(early_ends(&updater), Some(0), "it lasted");
        // Written on a thread of its own, as the timer's on the UI thread.
        let began = Instant::now();
        while Saved::read(&files.saved).probation.is_none_or(|probation| probation.running_since_ms.is_some()) {
            assert!(began.elapsed() < Duration::from_secs(10), "it's written");
            std::thread::sleep(Duration::from_millis(10));
        }

        let (updater, _) = launch();
        assert_eq!(early_ends(&updater), Some(0), "so its end wasn't early");
        updater.quitting_as_asked();
        let (updater, at) = launch();
        assert_eq!((at, early_ends(&updater)), (AtLaunch::GoOn, Some(0)), "nor is a quit");

        // A week on, it isn't watched.
        let mut saved = Saved::read(&files.saved);
        saved.probation.as_mut().unwrap().since_ms = 0;
        saved.write(&files.saved);
        let (updater, _) = launch();
        assert_eq!(early_ends(&updater), None);
    }

    /// Failure (review 1): going back from a version that kept ending deletes the record from before the update, the only good copy if what that version left can't be read by the one before. It's kept, offered once to fall back on, and goes after its week as ever; going back for any other reason, with it already put back, still clears it. And what comes back after a version that didn't work, or that the installer reopened without installing, is what was left, out of sight included, as an update's arrival is; a launch after the old app never quit isn't one of those.
    #[test]
    fn coming_back_from_a_version_that_kept_ending_keeps_the_record_from_before() {
        let bed = Bed::new("back-kept");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();
        let come_back = |why: &str, kind: OutcomeKind| {
            std::fs::create_dir_all(files.before()).unwrap();
            std::fs::write(files.before().join(".whole"), "").unwrap();
            Outcome {
                kind,
                from: "0.1.0".into(),
                to: "0.2.0".into(),
                why: why.into(),
            }
            .write(&files.outcome())
            .unwrap();
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            updater.launched(Facts {
                version: "0.1.0".into(),
                app: bed.app.clone(),
                system: "15.0".into(),
            });
            updater
        };
        let updater = come_back("kept_ending", OutcomeKind::WentBack);
        assert!(files.before().join(".whole").exists(), "kept");
        assert_eq!(updater.take_record_to_fall_back_on(), Some(files.before()), "to fall back on");
        assert_eq!(updater.take_record_to_fall_back_on(), None, "once");
        assert!(updater.reopened_by_update());
        let notice = updater.notice().unwrap().text;
        assert!(
            notice.contains("as RectoPDF 0.2.0 last had them") || notice.contains("as 0.2.0 last had them"),
            "{notice}"
        );
        updater.fell_back();
        assert!(updater.notice().unwrap().text.contains("from before the update"));

        let updater = come_back("didnt_start", OutcomeKind::WentBack);
        assert!(!files.before().exists(), "put back already, so it's for nothing");
        assert_eq!(updater.take_record_to_fall_back_on(), None);
        assert!(updater.reopened_by_update(), "the installer opened it");
        assert!(
            come_back("swap_failed", OutcomeKind::NotInstalled).reopened_by_update(),
            "reopened without installing"
        );
        assert!(
            !come_back("old_app_stayed", OutcomeKind::NotInstalled).reopened_by_update(),
            "the old app never went, so this is someone's own launch"
        );
    }

    /// Failure (review 1): the first update from a build that kept its copies as `RectoPDF.app` can't go back, as this one looks for `RectoPDF.kept`. A launch moves them to the new names, unless something's already there, and not while an installer of that build may be looking for the old ones.
    #[test]
    fn copies_under_the_old_names_are_moved_to_the_new() {
        let bed = Bed::new("names");
        let beside = beside_app(&bed.app).unwrap();
        let old_kept = beside.join("previous").join(crate::APP_BUNDLE);
        let old_staged = beside.join("new").join(crate::APP_BUNDLE);
        fake_app_at(&old_kept, "0.0.9");
        fake_app_at(&old_staged, "0.2.0");
        crate::plan::move_to_new_names(&beside);
        assert_eq!(stage::app_version(&previous_app_in(&beside)).as_deref(), Some("0.0.9"));
        assert_eq!(stage::app_version(&new_app_in(&beside)).as_deref(), Some("0.2.0"));
        assert!(!old_kept.exists() && !old_staged.exists());
        // Something under the new name stays as it is, and so does what's under the old.
        fake_app_at(&old_kept, "0.0.8");
        crate::plan::move_to_new_names(&beside);
        assert_eq!(stage::app_version(&previous_app_in(&beside)).as_deref(), Some("0.0.9"));
        assert!(old_kept.exists());
        // And at a launch.
        std::fs::remove_dir_all(previous_app_in(&beside)).unwrap();
        let root = bed.dir.join("state");
        let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        updater.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert_eq!(stage::app_version(&previous_app_in(&beside)).as_deref(), Some("0.0.8"), "moved at a launch");
    }

    /// Failure (review 1): the first run of a version being watched never gets its minute's timer, as nothing told the host the time had changed, so a run that lasted is counted as an early end. And a quit someone chose not to go ahead with leaves the run unwatched.
    #[test]
    fn a_watched_run_has_its_timer_and_a_quit_not_taken_is_watched_again() {
        let bed = Bed::new("watched-timer");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        let beside = beside_app(&bed.app).unwrap();
        fake_app_at(&previous_app_in(&beside), "0.1.0");
        local::ensure_private_dir(&files.work).unwrap();
        Plan {
            id: "1-1".into(),
            cdhash: "a".repeat(40),
            team_required: false,
            any_version: false,
            development_build: false,
            team_signed: false,
            from: "0.1.0".into(),
            to: "0.2.0".into(),
            pid: 1,
            app: bed.app.clone(),
            beside: beside.clone(),
            root: root.clone(),
            quit_at_ms: 5,
            background: false,
            quit_limit_ms: 1,
            start_limit_ms: 1,
            handed_off_tabs: None,
            open: Vec::new(),
            keep_record: false,
            test: None,
        }
        .write(&files.plan())
        .unwrap();
        let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        updater.launched(Facts {
            version: "0.2.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        let before = bed.fake.changes.load(Ordering::SeqCst);
        updater.first_frame(updater.now_ms());
        assert!(bed.fake.changes.load(Ordering::SeqCst) > before, "the host's told, and arms the timer");
        assert!(updater.next_wake().unwrap() <= updater.now_ms() + EARLY_END_MS);
        updater.quitting_as_asked();
        assert_eq!(Saved::read(&files.saved).probation.unwrap().running_since_ms, None);
        updater.quit_not_taken();
        assert!(Saved::read(&files.saved).probation.unwrap().running_since_ms.is_some(), "watched again");
    }

    /// Failure (full check under load, 2026-11-11): the window's patience can't tell a launch an update opened before the launch has been looked at, so it went up without the state and a closed window opened. Before then, an install's plan or outcome says it may be; after, what the launch was found to be.
    #[test]
    fn whether_an_update_opened_this_launch_is_known_before_it_is_looked_at() {
        let dir = temp_dir("maybe-reopened");
        let root = dir.join("state");
        local::ensure_private_dir(&root).unwrap();
        let files = Files::in_state(&root);
        let fresh = || {
            Updater::new(
                &root,
                crate::source::resolve(false, true, None, None),
                Arc::new(Fake::default()) as Arc<dyn Host>,
            )
        };
        assert!(!fresh().may_be_reopened_by_update(), "nothing of an install about");
        local::ensure_private_dir(&files.work).unwrap();
        std::fs::write(files.plan(), "{}").unwrap();
        assert!(fresh().may_be_reopened_by_update(), "a plan left");
        std::fs::remove_file(files.plan()).unwrap();
        std::fs::write(files.outcome(), "{}").unwrap();
        assert!(fresh().may_be_reopened_by_update(), "an outcome left");
        std::fs::remove_file(files.outcome()).unwrap();
        let updater = fresh();
        let app = fake_app(&dir.join("install"), "0.1.0");
        updater.launched(Facts {
            version: "0.1.0".into(),
            app,
            system: "15.0".into(),
        });
        assert!(!updater.may_be_reopened_by_update(), "looked at, and it isn't");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Failure: a second copy of the app downloads a release it can't install, as its hand-off will be refused, or takes away what the first has staged. It reads, and says, and leaves it all alone, and it wakes for nothing.
    #[test]
    fn a_second_copy_leaves_it_all_to_the_first() {
        let bed = Bed::new("second");
        let root = bed.dir.join("state");
        let beside = beside_app(&bed.app).unwrap();
        bed.publish("0.2.0");
        bed.found("0.2.0");
        std::fs::create_dir_all(new_app_in(&beside)).unwrap();
        std::fs::write(beside.join("staged.json"), "0.2.0 abc").unwrap();
        let before = std::fs::read(root.join("updates.json")).unwrap();
        bed.fake.second_copy.store(true, Ordering::SeqCst);
        let second = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        second.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert_eq!(second.next_wake(), None, "it wakes for nothing");
        assert_eq!(second.install(Asked::ByHand), Err(InstallRefused::AnotherCopy));
        assert!(
            new_app_in(&beside).exists() && beside.join("staged.json").exists(),
            "the first copy's staging stays"
        );
        second.first_frame(10);
        assert_eq!(std::fs::read(root.join("updates.json")).unwrap(), before, "it writes nothing of the first's");
        assert_eq!(second.menu_item().title, "Check for Updates…");
    }

    /// Failure: an app that can't be replaced quits and reopens for ever, a day apart, saying nothing, which the review proved. It's told once it's back, and that's held against the version. Anything else that quit for nothing gets three goes, and then the same.
    #[test]
    fn quitting_for_nothing_says_so_and_stops() {
        let bed = Bed::new("nothing");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();
        let launch_with = |why: &str| {
            Outcome {
                kind: OutcomeKind::NotInstalled,
                from: "0.1.0".into(),
                to: "0.2.0".into(),
                why: why.into(),
            }
            .write(&files.outcome())
            .unwrap();
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            updater.launched(Facts {
                version: "0.1.0".into(),
                app: bed.app.clone(),
                system: "15.0".into(),
            });
            updater
        };
        let updater = launch_with("swap_failed");
        let notice = updater.notice().unwrap().text;
        assert!(notice.contains("couldn't be replaced") && notice.contains("unchanged"), "{notice}");
        assert_eq!(lock(&updater.st).saved.failed, vec!["0.2.0".to_string()], "not by itself again");
        lock(&updater.st).saved.failed.clear();
        let cleared = lock(&updater.st).saved.clone();
        Saved {
            install_failures: 0,
            quits_for_nothing: 0,
            ..cleared
        }
        .write(&files.saved);
        for tries in 1..=QUITS_FOR_NOTHING {
            let updater = launch_with("old_app_stayed");
            assert!(updater.notice().is_some(), "said each time");
            assert_eq!(
                lock(&updater.st).saved.failed.contains(&"0.2.0".to_string()),
                tries == QUITS_FOR_NOTHING,
                "{tries}"
            );
        }
    }

    /// Failure: a new version that comes up and then misreads the record, starting empty or wrong, has already thrown away the record from before it, which was deleted at its first frame. It's kept for a week, and then it goes.
    #[test]
    fn the_record_from_before_an_update_is_kept_for_a_while() {
        let bed = Bed::new("before");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();
        std::fs::create_dir_all(files.before()).unwrap();
        std::fs::write(files.before().join(".whole"), "").unwrap();
        std::fs::write(files.before().join("record.a.json"), "the state before").unwrap();
        let plan = Plan {
            id: "1-1".into(),
            cdhash: "a".repeat(40),
            team_required: false,
            any_version: false,
            development_build: false,
            team_signed: false,
            from: "0.1.0".into(),
            to: "0.2.0".into(),
            pid: 1,
            app: bed.app.clone(),
            beside: beside_app(&bed.app).unwrap(),
            root: root.clone(),
            quit_at_ms: 5,
            background: false,
            quit_limit_ms: 1,
            start_limit_ms: 1,
            handed_off_tabs: None,
            open: Vec::new(),
            keep_record: false,
            test: None,
        };
        plan.write(&files.plan()).unwrap();
        let launch = |version: &str| {
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            updater.launched(Facts {
                version: version.into(),
                app: bed.app.clone(),
                system: "15.0".into(),
            });
            updater
        };
        launch("0.2.0").first_frame(100);
        assert!(files.before().join("record.a.json").exists(), "come up, and kept");
        launch("0.2.0").first_frame(100);
        assert!(files.before().join("record.a.json").exists(), "and at the next launch");
        let week_and_more = std::time::SystemTime::now() - Duration::from_millis(KEEP_RECORD_COPY_MS + 60_000);
        std::fs::File::options()
            .write(true)
            .open(files.before().join(".whole"))
            .unwrap()
            .set_modified(week_and_more)
            .unwrap();
        launch("0.2.0");
        assert!(!files.before().exists(), "a week on, it goes");
    }

    /// Failure: a launch tidying up deletes the only good copy, as `new/` after a keep that failed, or says the version that's here came back. All that's old in there is kept; a version it couldn't go back from says just that.
    #[test]
    fn the_only_good_copy_is_never_tidied_away() {
        let bed = Bed::new("only");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        let beside = beside_app(&bed.app).unwrap();
        local::ensure_private_dir(&files.work).unwrap();
        let in_new = new_app_in(&beside);
        fake_app_at(&in_new, "0.0.5");
        let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        updater.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert_eq!(stage::app_version(&previous_app_in(&beside)).as_deref(), Some("0.0.5"), "moved to be kept");
        Outcome {
            kind: OutcomeKind::CouldntGoBack,
            from: "0.0.5".into(),
            to: "0.1.0".into(),
            why: "nothing_to_go_back_to".into(),
        }
        .write(&files.outcome())
        .unwrap();
        let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        updater.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        let notice = updater.notice().unwrap().text;
        assert!(notice.contains("no earlier version") && !notice.contains("came back"), "{notice}");
    }

    /// Failure: a check leaves a thread behind, which would be a cost at rest for ever after. It's what the report's thread numbers can't assert under load, as the system's own pool comes and goes, so it's asked here, of this process alone.
    #[test]
    fn a_check_leaves_no_thread_behind() {
        let bed = Bed::new("threads");
        bed.publish("0.1.0");
        let threads = || local::process_counters(std::process::id()).unwrap().threads;
        // The server's own connection threads end with each request, so a first request settles those.
        bed.updater.check_here(Asked::ByHand);
        std::thread::sleep(Duration::from_millis(100));
        let before = threads();
        for _ in 0..3 {
            bed.updater.check_in_background(Asked::ByItself);
            bed.wait_for("the check", |bed| !lock(&bed.updater.st).status.is_under_way());
        }
        let began = Instant::now();
        while threads() > before {
            assert!(began.elapsed() < Duration::from_secs(10), "{} threads, and {before} before", threads());
            std::thread::sleep(Duration::from_millis(10));
        }
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
        let plan = Plan {
            id: "1-1".into(),
            cdhash: "a".repeat(40),
            team_required: false,
            any_version: false,
            development_build: false,
            team_signed: false,
            from: "0.1.0".into(),
            to: "0.2.0".into(),
            pid: 1,
            app: bed.app.clone(),
            beside: beside_app(&bed.app).unwrap(),
            root: root.clone(),
            quit_at_ms: 5,
            background: false,
            quit_limit_ms: 1,
            start_limit_ms: 1,
            handed_off_tabs: None,
            open: Vec::new(),
            keep_record: false,
            test: None,
        };
        let launch = |version: &str| {
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            updater.launched(Facts {
                version: version.into(),
                app: bed.app.clone(),
                system: "15.0".into(),
            });
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
        fake_app_at(&plan.new_app(), "0.1.0");
        let updater = launch("0.2.0");
        assert_eq!(lock(&updater.st).status, Status::Updated("0.2.0".into()));
        assert_eq!(
            stage::app_version(&plan.previous_app()).as_deref(),
            Some("0.1.0"),
            "kept, as the installer would have"
        );
        assert!(!plan.new_app().exists() && !files.plan().exists());
        updater.first_frame(1_005);
        assert_eq!(*lock(&bed.fake.draws), 1, "a version that's arrived has to show it can draw");
        let report = updater.report();
        assert_eq!(report["last_update"]["gap_ms"], 1_000);
        assert_eq!(std::fs::read_to_string(files.launch_ok()).unwrap().split_whitespace().next(), Some("0.2.0"));

        // A version that didn't start: said once, remembered, and not tried by itself again.
        Outcome {
            kind: OutcomeKind::WentBack,
            from: "0.1.0".into(),
            to: "0.3.0".into(),
            why: "didnt_start".into(),
        }
        .write(&files.outcome())
        .unwrap();
        let updater = launch("0.1.0");
        assert_eq!(
            lock(&updater.st).status,
            Status::WentBack {
                version: "0.3.0".into(),
                back_on: "0.1.0".into()
            }
        );
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
