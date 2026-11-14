//! The updater, joined to the app (SPEC decision 24). All the rules and words are `recto-update`'s and this one's: the host sets one timer for when it's told, draws a menu item and a small window, and passes on what's chosen.

use super::ScreenInfo;
use super::{CoreCall, Inner, lock};
use recto_agent::Code;
use recto_agent::{AgentError, Command, exit};
use recto_control::record_store::Loaded;
use recto_platform::isolate::SharedMemory;
use recto_render_service::{DocRenderer, Policy, ServiceConfig, TileKind, TileRect, TileRequest, TileResult};
use recto_session::record::SettingsWindow;
use recto_update::plan::TestLaunch;
use recto_update::source::Mode;
use recto_update::updater::{Asked, Facts, HandOffRefused, HandedOff, Host, InstallRefused, Look};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

/// The switch's id, in the window, in QA and nowhere else.
pub const AUTO_UPDATE_ROW: &str = "update.automatic";
/// *Allow development builds*.
pub const DEV_BUILDS_ROW: &str = "update.dev_builds";
/// The button that installs a waiting update now.
pub const UPDATE_NOW_ROW: &str = "update.now";

/// **What starts the installer:** a process of its own, detached, that outlives this one. A test's records the plan instead (step R1.4).
pub trait Installer: Send + Sync {
    /// Starts it on the plan, logging to `log`. Says its process number.
    ///
    /// # Errors
    ///
    /// It couldn't be started.
    fn start(&self, helper: &Path, plan: &Path, log: &Path) -> std::io::Result<u32>;
}

/// The real installer, `recto-updater install <plan>`, as the updater has always started it.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealInstaller;

impl Installer for RealInstaller {
    fn start(&self, helper: &Path, plan: &Path, log: &Path) -> std::io::Result<u32> {
        recto_platform::local::spawn_detached(helper, &["install".to_owned(), plan.display().to_string()], &[], log)
    }
}

/// What an update is made with that a test gives otherwise: the updater's wall clock and releases, and what starts the installer. The real ones by default.
#[derive(Clone)]
pub struct UpdateSeams {
    pub updater: recto_update::updater::Seams,
    pub installer: Arc<dyn Installer>,
}

impl Default for UpdateSeams {
    fn default() -> Self {
        Self {
            updater: recto_update::updater::Seams::default(),
            installer: Arc::new(RealInstaller),
        }
    }
}

/// How the updater reaches the app. Holds only a way back to it: it's made first, as the updater's a part of what it reaches.
pub struct Hooks {
    inner: OnceLock<Weak<Inner>>,
    installer: Arc<dyn Installer>,
}

impl Hooks {
    pub fn new(installer: Arc<dyn Installer>) -> Arc<Self> {
        Arc::new(Self {
            inner: OnceLock::new(),
            installer,
        })
    }

    pub fn attach(&self, inner: Weak<Inner>) {
        let _ = self.inner.set(inner);
    }

    fn inner(&self) -> Option<Arc<Inner>> {
        self.inner.get().and_then(Weak::upgrade)
    }
}

impl Host for Hooks {
    fn look(&self) -> Look {
        let Some(inner) = self.inner() else {
            return Look {
                busy: vec!["The app is starting.".to_owned()],
                since_input_ms: 0,
            };
        };
        let check = inner.hand_off_check();
        Look {
            busy: check.busy.into_iter().map(|item| item.message).collect(),
            since_input_ms: (check.seconds_since_input * 1000.0) as u64,
        }
    }

    fn hand_off(&self, limit: Duration) -> Result<HandedOff, HandOffRefused> {
        let inner = self.inner().ok_or_else(|| HandOffRefused::NotSaved("the app is going away".to_owned()))?;
        // The tabs in the record as written and read back, which is what the new version reads.
        inner
            .hand_off(limit)
            .map(|report| HandedOff {
                tabs: u32::try_from(report.tabs).ok(),
            })
            .map_err(|error| match error {
                super::HandOffError::Busy { items } => HandOffRefused::Busy(items.into_iter().map(|item| item.message).collect()),
                super::HandOffError::NotSaved { message, .. } => HandOffRefused::NotSaved(message),
            })
    }

    fn quit_now(&self) -> bool {
        self.inner().is_some_and(|inner| inner.hand_off_quit_now())
    }

    /// Attached once and let go of since: the headless kit's app has ended, as a real one ends only with its process.
    fn gone(&self) -> bool {
        self.inner.get().is_some_and(|inner| inner.strong_count() == 0)
    }

    fn cancelled(&self) {
        if let Some(inner) = self.inner() {
            inner.hand_off_cancelled();
        }
    }

    fn changed(&self) {
        if let Some(inner) = self.inner() {
            inner.core.push(CoreCall::Reconcile);
            inner.listener.model_changed();
        }
    }

    fn frontmost(&self) -> bool {
        self.inner().is_some_and(|inner| lock(&inner.window.app).focus.app_frontmost)
    }

    /// A quiet run reopens quiet, on its own state folder. Only the updater's test mode ever looks at this.
    fn relaunch(&self) -> TestLaunch {
        let Some(inner) = self.inner() else { return TestLaunch::default() };
        let root = inner.serving.paths.as_ref().map(|paths| paths.root.display().to_string()).unwrap_or_default();
        let mut args = if inner.qa.is_some() { vec!["--qa".to_owned()] } else { Vec::new() };
        // The new run ends with the same test as this one.
        if inner.qa.is_some() {
            args.extend(super::qa::owner_args());
        }
        let mut env = vec![("RECTO_STATE_DIR".to_owned(), root)];
        if inner.qa.is_some() {
            env.extend(recto_platform::slow::relaunch_env());
        }
        TestLaunch { args, env, direct: false }
    }

    /// Whether this copy keeps the record as the updater's told of the launch ([`LaunchFacts::tell`]: after the launch's one wait for the lock, or held back until
    /// whether it's kept has settled): it holds the lock and claims the record ([`crate::lifecycle::keeping::Keeping::claims`]), so a lock a try or a late read
    /// still holds once keeping's settled not to (the third read's 4a and 4c) isn't taken as keeping it. Never asking for the lock here, so a launch told it's a
    /// second copy can't take it after.
    fn keeps_the_record(&self) -> bool {
        self.inner()
            .is_some_and(|inner| inner.life.keeping().claims() && inner.saver.store.holds_the_record())
    }

    /// A build goes in only while agent access is Full, as it was when an agent handed it over: read from the level's file now.
    fn may_take_a_build(&self) -> bool {
        let Some(inner) = self.inner() else { return false };
        inner
            .serving
            .paths
            .as_ref()
            .is_some_and(|paths| recto_control::levels::current(paths) == recto_agent::Level::Full)
    }

    fn can_draw(&self) -> bool {
        let Some(inner) = self.inner() else { return false };
        let Some(paths) = inner.serving.paths.as_ref() else { return false };
        draws_a_page(&paths.root.join("update"))
    }

    /// The record as read, once it has been: a first frame that came before that, off a slow disk, says nothing.
    fn tabs_restored(&self) -> Option<u32> {
        let read = TABS_READ.load(Ordering::SeqCst);
        (read != NOT_READ).then(|| u32::try_from(read).unwrap_or(u32::MAX))
    }

    fn documents_asked_for(&self) -> Vec<PathBuf> {
        lock(&ASKED_TO_OPEN).clone()
    }

    fn start_installer(&self, helper: &Path, plan: &Path, log: &Path) -> std::io::Result<u32> {
        self.installer.start(helper, plan, log)
    }
}

/// **What the host knows of this copy, for the updater, until the updater's told of the launch** ([`Self::tell`]): told at once when the record's lock came at
/// the launch, as always; held back when it didn't, until whether this copy keeps the record has settled ([`crate::lifecycle::keeping`]), with a first frame shown
/// meanwhile, so the updater hears of the launch once, as it turned out, and a copy that keeps the record late says it came up with the tabs it read then.
///
/// **Its rules are its own:** the facts are told once (taken as they're told); what's held can't be left held once keeping has settled ([`Self::untold`] says
/// whether it is); and a first frame shown while held is kept until the launch has been told and the tabs read. Its one lock is held for a copy or a swap of a few
/// fields, never across telling the updater, which reads and writes files.
pub struct LaunchFacts(Mutex<Launch>);

#[derive(Default)]
struct Launch {
    facts: Option<Facts>,
    /// Held back: the record's lock didn't come at the launch. Until the updater's been told and has heard of the first frame.
    held: bool,
    /// The first frame, shown while they were held, by the updater's wall clock. Kept here and not in the updater's own early frame
    /// ([`recto_update::updater::Updater::first_frame`] keeps one that comes before it's told of the launch, and tells it as it's told): for a late keep the updater's
    /// told of the launch before the saved session is read, so its own would be told then, with no tabs read yet, and the installer's look at whether the new
    /// version misread the record couldn't be made. This one's told after the read is in ([`LaunchFacts::first_frame_after_launch`]), with the tabs it came up with.
    first_frame_at: Option<u64>,
}

impl LaunchFacts {
    /// At launch: nothing known, nothing held.
    pub(super) fn new() -> Self {
        Self(Mutex::new(Launch::default()))
    }

    /// What this copy of the app is, from the host, before the state's read.
    pub fn note(&self, version: String, dev_version: Option<String>, bundle_path: String, system_version: String) {
        // Resolved where the rest of what's asked of the disk is, on the thread that reads the state.
        lock(&self.0).facts = Some(Facts {
            version: recto_update::version::running(&version, dev_version.as_deref()),
            app: PathBuf::from(bundle_path),
            system: system_version,
        });
    }

    /// Whether this launch reads nothing of the record: a test's broken release that misreads it ([`misreads_the_record`]).
    pub(super) fn misreads(&self) -> bool {
        misreads_the_record(lock(&self.0).facts.as_ref())
    }

    /// Held back for the record's lock: nothing of an update begins meanwhile.
    pub(super) fn held(&self) -> bool {
        lock(&self.0).held
    }

    /// **Held back and not yet told:** what can't be once keeping has settled ([`crate::lifecycle::keeping::Keeping::settled`]), as settling is where they're
    /// taken to be told.
    #[must_use]
    pub(super) fn untold(&self) -> bool {
        let launch = lock(&self.0);
        launch.held && launch.facts.is_some()
    }

    /// **The updater told of the launch, or held back** (at the launch, on the thread that reads the state, before it reads): `keeps` is the answer of the
    /// launch's one wait for the record's lock ([`super::launching::read_state`]), as an app that's just quit can still be letting go of it, and a quick quit and
    /// reopen isn't a second copy. Kept: told now ([`Self::tell`]), so a version that keeps failing goes back here, and not by reading on. Not kept: what's known
    /// is held back and nothing about an update can begin (the updater has no version to go by: a check or an install is answered that it's still starting, and
    /// its timer isn't set), until keeping has settled, where it's told ([`super::keeping`]): by the late try that got the lock, before the saved session is
    /// read, as a launch is; or as a second copy once it's said the window won't be remembered ([`tell_held`]). A quit or a hand-off before then tells it
    /// nothing, and nothing of the updater's is written.
    pub(super) fn hold_or_tell(&self, keeps: bool, updates: &recto_update::updater::Updater) -> recto_update::updater::AtLaunch {
        if !keeps {
            let mut launch = lock(&self.0);
            if launch.facts.is_some() {
                launch.held = true;
            }
            return recto_update::updater::AtLaunch::GoOn;
        }
        self.tell(updates)
    }

    /// **The updater told of the launch** ([`recto_update::updater::Updater::launched`]) with what the host knows, once: the facts are taken, so a second
    /// telling has nothing to tell. Whether this copy keeps the record is the store's answer as it stands ([`Hooks::keeps_the_record`]). On a thread that can wait.
    pub(super) fn tell(&self, updates: &recto_update::updater::Updater) -> recto_update::updater::AtLaunch {
        let Some(facts) = lock(&self.0).facts.take() else {
            return recto_update::updater::AtLaunch::GoOn;
        };
        updates.launched(facts)
    }

    /// The updater's been told of a launch that was held back, and the tabs are read: a first frame shown meanwhile is told now. On a thread that can wait.
    fn first_frame_after_launch(&self, updates: &recto_update::updater::Updater) {
        let held = {
            let mut launch = lock(&self.0);
            launch.held = false;
            launch.first_frame_at.take()
        };
        if let Some(at) = held {
            tell_first_frame(updates, at);
        }
    }

    /// The first frame's up, or there's no window to show: what "come up" means is the updater's, on the thread this is called on, unless the launch is held
    /// back, when it's kept for then.
    pub fn first_frame_shown(&self, updates: &recto_update::updater::Updater) {
        let at = updates.wall_ms();
        {
            let mut launch = lock(&self.0);
            if launch.held {
                launch.first_frame_at.get_or_insert(at);
                return;
            }
        }
        tell_first_frame(updates, at);
    }

    /// What's held for the updater, taken in one look: the facts if they're still untold, and nothing at all for a launch that wasn't held back.
    fn take_held(&self) -> Option<Option<Facts>> {
        let (facts, held) = {
            let mut launch = lock(&self.0);
            (launch.facts.take(), launch.held)
        };
        if facts.is_none() && !held {
            return None;
        }
        Some(facts)
    }
}

/// **The updater told of a launch that was held back,** now that keeping has settled. What's held is taken here, so it can't be left held once settled; it's
/// told on a thread of its own, as that touches files: of the launch, if it hadn't been (a second copy, or a copy that came to keep the record by another route
/// than a late try, whose read tells it), as what this copy claims then ([`crate::lifecycle::keeping::Keeping::claims`], whatever lock a try or a read still
/// holds), and then of a first frame shown meanwhile, with the tabs that were read. Nothing at all for a launch that wasn't held back.
///
/// The thread holds the life (where what's held is) and the updater, not the app: if the app has gone by the time the updater asks whether this copy keeps the
/// record, the updater's way back to it ([`Hooks`]' weak reference) finds nothing and it reads "doesn't keep the record", so a tell outliving its app writes only
/// what a second copy does (`update_guards::an_app_ended_while_the_updater_is_told_writes_nothing_a_kept_copy_would`).
pub(super) fn tell_held(life: &Arc<super::Lifecycle>, updates: &Arc<recto_update::updater::Updater>) {
    let Some(facts) = life.update_facts.take_held() else {
        return;
    };
    let facts = Arc::new(Mutex::new(facts));
    let (telling, by, theirs) = (Arc::clone(life), Arc::clone(updates), Arc::clone(&facts));
    let told = std::thread::Builder::new()
        .name("recto-update-frame".into())
        .stack_size(512 * 1024)
        .spawn(move || tell(&telling.update_facts, &by, lock(&theirs).take()));
    // With no thread to be had, what's held isn't left held for the rest of the run: told here, as the one thing that can still tell it.
    if told.is_err() {
        tell(&life.update_facts, updates, lock(&facts).take());
    }
}

/// Told, on the thread that tells it.
fn tell(launch: &LaunchFacts, updates: &recto_update::updater::Updater, facts: Option<Facts>) {
    if let Some(facts) = facts
        && updates.launched(facts) == recto_update::updater::AtLaunch::Exit
    {
        std::process::exit(0);
    }
    launch.first_frame_after_launch(updates);
}

/// The updater's told of the first frame. Then a test's broken release ends here, as a new version that crashes once it's up would.
fn tell_first_frame(updates: &recto_update::updater::Updater, at: u64) {
    updates.first_frame(at);
    let Some(app) = updates.app() else { return };
    if recto_update::test_breakage(&app, recto_update::TestBreakage::EndsAfterFirstFrame) {
        std::thread::sleep(Duration::from_millis(300));
        if recto_update::test_breakage(&app, recto_update::TestBreakage::DamagesTheRecordAsItEnds) {
            for slot in ["record.a.json", "record.b.json"] {
                let _ = std::fs::write(updates.session_folder().join(slot), "damaged by the version that kept ending");
            }
        }
        std::process::exit(70);
    }
}

const NOT_READ: u64 = u64::MAX;
/// The tabs in the record as this launch read it: none for a record that wasn't there or couldn't be read.
static TABS_READ: AtomicU64 = AtomicU64::new(NOT_READ);
/// What this launch was asked to open before the state was read, at most [`recto_update::plan::MAX_OPEN`].
static ASKED_TO_OPEN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The record's been read: how many tabs it held, for the installer's look at whether this version misread it. Not read at all, for the lock, says nothing.
pub fn note_tabs_read(loaded: &Loaded) {
    let tabs = match loaded {
        Loaded::Restored(restored) => restored.record.tabs.len() as u64,
        Loaded::OtherInstance => return,
        _ => 0,
    };
    TABS_READ.store(tabs.min(NOT_READ - 1), Ordering::SeqCst);
}

/// Documents asked for before the state was read, by the launch's arguments or the system.
pub fn note_asked_to_open(paths: &[PathBuf]) {
    let mut asked = lock(&ASKED_TO_OPEN);
    for path in paths {
        if asked.len() < recto_update::plan::MAX_OPEN && path.is_absolute() && !asked.contains(path) {
            asked.push(path.clone());
        }
    }
}

/// **Back from a version that kept ending,** the record's as it left it, as that holds whatever was done since. If this version can't read it, damaged or of a newer kind, the record from before the update is put back, where this version reads, and read instead. Only at that one launch.
pub fn fall_back_if_unreadable(updates: &recto_update::updater::Updater, store: &recto_control::record_store::RecordStore, loaded: Loaded) -> Loaded {
    let Some(before) = updates.take_record_to_fall_back_on() else {
        return loaded;
    };
    let into = match loaded {
        // Damaged, whether or not anything could be brought back from its journals beside it.
        Loaded::Damaged => store.main_folder(),
        Loaded::Restored(restored) if restored.notice == Some(recto_session::record::notices::DAMAGED) => store.main_folder(),
        // A newer kind is never written over: this version's own folder beside it is where it reads.
        Loaded::Newer => store.own_folder(),
        other => return other,
    };
    recto_update::install::restore_record(&before, &into);
    updates.fell_back();
    store.load()
}

/// How long a launch an update opened waits for the state before it shows anything without it, from the launch. Whether its window is closed, in the Dock or hidden, and where it goes, are in the state, and nobody's watching it come up, so it waits; a launch someone asked for waits a second, as a person is looking for it.
pub const UPDATE_RESTORE_PATIENCE: Duration = Duration::from_secs(20);

/// What the window's patience for the state comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Patience {
    ShowNow,
    WaitFor(Duration),
}

/// **The window's patience for the state** (a full check under load: a window closed before an update came back open, and one in the Dock came back shown, as the state was read after the second's patience and the window went up without it). A launch an update may have opened waits for the state up to [`UPDATE_RESTORE_PATIENCE`]; any other goes up now.
pub fn restore_patience(may_be_reopened_by_update: bool, since_launch: Duration) -> Patience {
    if !may_be_reopened_by_update || since_launch >= UPDATE_RESTORE_PATIENCE {
        return Patience::ShowNow;
    }
    Patience::WaitFor(UPDATE_RESTORE_PATIENCE - since_launch)
}

/// How long a window up without the state waits for it before it takes input anyway. Past this, a state that comes in isn't made into the window, as the person may be using it.
pub const INPUT_PATIENCE: Duration = Duration::from_secs(10);

/// **A window that went up without the state takes no input until it's made from it** (CI, main at faa9180: ⌘F in the first second gave the field the keyboard on a window that the state, read late, then made again, and the new one came up with the page focused). Refusing what's done in that moment loses nothing; carrying it over would have to know every way a window can be changed, and lose what it didn't. Not for ever: past [`INPUT_PATIENCE`] it takes input, and a state that comes later isn't made into it.
pub fn takes_input(shown_with_state: bool, since_launch: Duration) -> bool {
    shown_with_state || since_launch >= INPUT_PATIENCE
}

/// Whether a menu item or key does anything now. What's the app's and not the window's, as Quit, Settings and the update, always does.
pub fn takes_input_now(window_interactive: bool, id: &str) -> bool {
    window_interactive || id.starts_with("app.") || matches!(id, "file.settings" | "file.open")
}

/// What a restore that comes after the host has decided keeps: what the host did, not what was saved. Before it has, what was saved, for it to decide on.
pub fn out_of_sight_when_restored(saved: bool, decided: Option<bool>) -> bool {
    decided.unwrap_or(saved)
}

/// What this launch reads: the record, or nothing at all for a test's broken release that misreads it.
fn misreads_the_record(facts: Option<&Facts>) -> bool {
    facts.is_some_and(|facts| recto_update::test_breakage(&facts.app, recto_update::TestBreakage::MisreadsTheRecord))
}

/// **What the window does with how the person's update went, when they chose it from the menu** (R2.3, C8): the menu's *Update to X…* whose offer has gone
/// by the time it runs is answered as the menu always did, by the person's `update check`, a follow-up of the window's, still marked as the menu's; and what
/// the menu asked for that didn't start, or a check that's said what it found, puts nothing more up as an alert. Settings' button is never the menu's, and
/// nothing follows it.
pub fn answered(inner: &Arc<Inner>, ran: &super::person::Ran<'_>) {
    use recto_agent::commands::Updates;
    use recto_agent::commands::updates::{UpdateCheck, UpdateInstall};
    let Command::Updates(asked) = ran.command else { return };
    if !inner.updates.menu().marked() {
        return;
    }
    match (asked, ran.refused()) {
        (Updates::UpdateInstall(UpdateInstall { zip: None, now: true }), Some(error))
            if matches!(error.code, Code::NOTHING_ON_OFFER | Code::DEVELOPMENT_BUILD) =>
        {
            super::person::submit(inner, Command::from(UpdateCheck {}));
        }
        (Updates::UpdateCheck(_), _) | (_, Some(_)) => inner.updates.menu().unmark(),
        _ => {}
    }
}

/// **The updates' menu ids** ([`super::menu_rows`]): *Update to X…* is `update install --now` and *Check for Updates…* `update check`, by what the updater
/// offers now; Settings opens its window, whose switches are `UiAction::SettingChosen`.
pub fn row<'a>(inner: &'a Arc<Inner>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    use super::menu_rows::Row;
    use recto_agent::commands::updates::{UpdateCheck, UpdateInstall};
    match (asked.action.as_ref()?, asked.id) {
        (crate::menus::Action::Update, _) => {
            let command = if inner.updates.menu_installs() {
                Command::from(UpdateInstall { zip: None, now: true })
            } else {
                Command::from(UpdateCheck {})
            };
            Some(Row::own_way(
                command.clone(),
                "the person's is said in an alert too when there's no document window to say it in (the updater's mark), on a thread of its own. With something under way the item can't be chosen; a choice that races it sends nothing, so the mark isn't put on someone else's, and that is the window's alone: the command does nothing then for anyone (a check under way isn't started again, and an install is refused `update_under_way`)",
                move || {
                    if inner.updates.menu().mark() {
                        super::person::submit(inner, command);
                    }
                    super::HostAction::None
                },
            ))
        }
        (crate::menus::Action::Host(host), "app.settings" | "file.settings") => Some(Row::host(
            "opens the Settings window for the person; what's chosen there is `SettingChosen`",
            super::menu_rows::Agents::ItsOwn("`recto settings`"),
            *host,
        )),
        _ => None,
    }
}

/// On the way out, at a quit or as a quiet run ends with what started it: a quiet run's copy is a test's or a script's, and the system registered it when it was started, so it's forgotten again and the one install stays the only RectoPDF in Open With. Never the one install itself. Returns at once: the tool outlives the process. Which copy is this process's own executable's bundle, known from the start: what the host tells the updater comes later, and a run can end first (CI, 2026-11-12). `quiet`: this is a quiet run (the app has QA).
pub fn forget_a_quiet_copy(quiet: bool, updates: &recto_update::updater::Updater) {
    let own = std::env::current_exe()
        .ok()
        .and_then(|program| program.canonicalize().ok())
        .and_then(|program| recto_update::rules::bundle_of(&program));
    if let Some(app) = own.or_else(|| updates.app())
        && recto_update::rules::forgets_itself(quiet, &app)
    {
        recto_platform::registry::forget_soon(&app);
    }
}

/// **A switch or the button in Settings, as the person chooses it** (R2.3, C8): *Update automatically* is `settings --auto-update on|off`, *Allow
/// development builds* `update dev-builds --set on|off`, and *Update Now* `update install --now`, the commands an agent sends, run for the person by the
/// one executor, off the UI thread ([`super::controls`]' `act` sends it). What any of them comes to is in the status, as before. Nothing for an id that isn't one.
pub fn setting_command(id: &str, on: bool) -> Option<Command> {
    use recto_agent::commands::updates::{Settings, UpdateDevBuilds, UpdateInstall};
    let word = || Some(recto_agent::commands::words::OnOff::from_bool(on));
    let command = match id {
        AUTO_UPDATE_ROW => Command::from(Settings { auto_update: word() }),
        DEV_BUILDS_ROW => Command::from(UpdateDevBuilds { set: word() }),
        UPDATE_NOW_ROW => Command::from(UpdateInstall { zip: None, now: true }),
        _ => return None,
    };
    Some(command)
}

/// Whether Settings has this switch or button, for QA's `settings` verb.
pub fn has_setting(id: &str) -> bool {
    matches!(id, AUTO_UPDATE_ROW | DEV_BUILDS_ROW | UPDATE_NOW_ROW)
}

impl Inner {
    /// What `recto update` and `recto settings` come to. The level's been checked.
    pub fn update_command(&self, command: &Command) -> Result<Value, AgentError> {
        match command {
            Command::Updates(recto_agent::commands::Updates::UpdateStatus(_)) => Ok(self.updates.report()),
            // While the launch is held back for the record's lock (`LaunchFacts`), nothing of an update begins, by anyone's asking.
            Command::Updates(recto_agent::commands::Updates::UpdateCheck(_) | recto_agent::commands::Updates::UpdateInstall(_))
                if self.life.update_facts.held() =>
            {
                Err(refused_words(&InstallRefused::Starting))
            }
            Command::Updates(recto_agent::commands::Updates::UpdateCheck(_)) => {
                self.updates.check_here(Asked::ByHand);
                Ok(self.updates.report())
            }
            Command::Updates(recto_agent::commands::Updates::UpdateInstall(recto_agent::commands::updates::UpdateInstall { zip: Some(zip), now })) => {
                self.install_build(zip, *now)
            }
            Command::Updates(recto_agent::commands::Updates::UpdateDevBuilds(recto_agent::commands::updates::UpdateDevBuilds { set })) => {
                if let Some(set) = set {
                    self.set_dev_builds(set.is_on());
                }
                Ok(self.dev_builds_answer())
            }
            // The five idle minutes are the owner's rule: the plain command only says what a waiting build waits for. `--now`, the menu and Settings' button are the one call that goes.
            Command::Updates(recto_agent::commands::Updates::UpdateInstall(recto_agent::commands::updates::UpdateInstall { zip: None, now: false }))
                if self.updates.waiting_build_words().is_some() =>
            {
                let (version, words) = self.updates.waiting_build_words().unwrap_or_default();
                Ok(json!({ "started": false, "version": version, "words": words }))
            }
            Command::Updates(recto_agent::commands::Updates::UpdateInstall(recto_agent::commands::updates::UpdateInstall { zip: None, .. })) => {
                match self.updates.update_now() {
                    Ok(version) => Ok(
                        json!({ "started": true, "version": version, "words": format!("Updating to {version}. RectoPDF downloads and checks it first, then quits and reopens as it was. `recto update status` says how it's going.") }),
                    ),
                    Err(InstallRefused::NotInThisBuild) => {
                        Err(
                            AgentError::new(Code::DEVELOPMENT_BUILD, "A development build doesn't replace itself.", exit::FAILED)
                                .then("`recto update check` still says whether there's a new version."),
                        )
                    }
                    Err(InstallRefused::NothingOnOffer) => {
                        Err(AgentError::new(Code::NOTHING_ON_OFFER, "There's no new version on offer.", exit::FAILED)
                            .then("`recto update check` looks for one."))
                    }
                    Err(InstallRefused::Underway) => Err(
                        AgentError::new(Code::UPDATE_UNDER_WAY, "An update or a check is already under way.", exit::BUSY)
                            .then("`recto update status` says how it's going."),
                    ),
                    Err(refused) => Err(refused_words(&refused)),
                }
            }
            Command::Updates(recto_agent::commands::Updates::Settings(recto_agent::commands::updates::Settings { auto_update })) => {
                if let Some(on) = auto_update {
                    self.set_auto_update(on.is_on());
                }
                Ok(self.settings_answer())
            }
            _ => Err(recto_agent::update_needs_app()),
        }
    }

    /// `recto update install <zip>`: a development build handed over. Checked here, on the command's own thread, and then it waits for its moment.
    fn install_build(&self, zip: &Path, now: bool) -> Result<Value, AgentError> {
        let version = self.updates.install_build(zip, now).map_err(|refused| refused_words(&refused))?;
        let report = self.updates.report();
        let waits_ms = report["install_look_in_ms"].as_u64();
        let team = report["build_team"].as_str().map(str::to_owned);
        let signed = match &team {
            Some(team) => format!("It's signed by RectoPDF's developer (team {team})."),
            None => "It's signed on this Mac, not by RectoPDF's developer, so macOS may ask again for access to folders.".to_owned(),
        };
        let words = if now || waits_ms.is_none() {
            format!("Installing development build {version} now. RectoPDF quits and reopens as it was. {signed} `recto update status` says how it's going.")
        } else {
            format!(
                "Development build {version} is checked, and goes in once RectoPDF hasn't been used for 5 minutes, then reopens as it was. Choose Update to {version}… or add `--now` to go at once. {signed} `recto update status` says how it's going."
            )
        };
        Ok(json!({ "started": true, "version": version, "waits_ms": waits_ms, "team": team, "words": words }))
    }

    fn set_dev_builds(&self, on: bool) {
        self.updates.choose_dev_builds(on);
        self.core.push(CoreCall::Reconcile);
    }

    fn dev_builds_answer(&self) -> Value {
        let on = self.updates.dev_builds_on();
        let back = self.updates.report()["back_to_release"] == true;
        let words = match (on, back) {
            (true, _) => "Allow development builds: on. `recto update install <zip>` installs one, once RectoPDF hasn't been used for 5 minutes.".to_owned(),
            (false, true) => "Allow development builds: off. This is a development build, so the latest release goes in by itself once there is one and RectoPDF hasn't been touched for 30 minutes.".to_owned(),
            (false, false) => "Allow development builds: off. RectoPDF installs only releases.".to_owned(),
        };
        json!({ "dev_builds": on, "back_to_release": back, "words": words })
    }

    fn settings_answer(&self) -> Value {
        let on = self.updates.switch_on();
        let words = if self.updates.source().mode == Mode::Development {
            format!(
                "Update automatically: {}. A local build checks only when asked, and doesn't replace itself.",
                if on { "on" } else { "off" }
            )
        } else {
            format!("Update automatically: {}.", if on { "on" } else { "off" })
        };
        json!({ "auto_update": on, "words": words })
    }

    /// The switch, however it's changed. Written out soon, with the rest of the state.
    fn set_auto_update(&self, on: bool) {
        self.updates.choose_switch(on);
        self.core.push(CoreCall::Reconcile);
    }

    /// With the window open, the margin says it, and nothing more is put up.
    pub fn take_update_alert(&self) -> Option<UpdateAlert> {
        let words = self.updates.menu().take_alert()?;
        let app = lock(&self.window.app);
        let in_dock = app.window.as_ref().is_some_and(|window| window.minimised);
        alert_for(words, !app.window_closed && !in_dock)
    }

    pub fn settings_model(&self) -> SettingsModel {
        let development = self.updates.source().mode == Mode::Development;
        let hint = self.updates.auto_update_hint(crate::privacy::CHECK_EITHER_WAY);
        let mut rows = vec![
                SettingRow {
                    id: AUTO_UPDATE_ROW.to_owned(),
                    title: "Update automatically".to_owned(),
                    hint,
                    on: self.updates.switch_on(),
                    enabled: !development,
                },
                SettingRow {
                    id: DEV_BUILDS_ROW.to_owned(),
                    title: "Allow development builds".to_owned(),
                    hint: "Lets a development build replace RectoPDF, once you haven't used it for 5 minutes. Off, only releases are installed, and a development build goes back to the latest release.".to_owned(),
                    on: self.updates.dev_builds_on(),
                    enabled: !development,
                },
        ];
        // Rust's rule: only on a development build, or while it's on.
        rows.retain(|row| row.id != DEV_BUILDS_ROW || self.updates.shows_dev_builds_switch());
        SettingsModel {
            title: "Settings".to_owned(),
            rows,
            action: self.updates.settings_action().map(|action| SettingsAction {
                id: UPDATE_NOW_ROW.to_owned(),
                title: action.title,
                hint: action.hint,
                button: action.button,
                enabled: action.enabled,
            }),
            status: self.updates.status_line(),
            privacy_title: crate::privacy::STATEMENT_TITLE.to_owned(),
            privacy_url: crate::privacy::statement_url(),
        }
    }

    /// The hand-off isn't going ahead.
    pub fn hand_off_cancelled(&self) {
        self.carry_out(self.life.step(crate::lifecycle::Event::HandOffUndone));
    }
}

/// A page of one colour, as small as a PDF gets. Ours, so it says something about this version and nothing about whatever the user had open.
fn probe_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 64 64] /Contents 4 0 R >>".to_owned(),
        {
            let content = "1 0 0 rg 0 0 64 64 re f";
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
        },
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut places = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        places.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let table = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for place in places {
        pdf.extend_from_slice(format!("{place:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{table}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

/// Whether this version can draw: the page above, through a real worker with the real library, and what comes back has to be that colour. What a new version has to show before it counts as having come up, as nothing else says a renderer that can't load is broken until someone's looking at a blank page.
pub fn draws_a_page(folder: &Path) -> bool {
    let file = folder.join("draws.pdf");
    if std::fs::create_dir_all(folder).is_err() || std::fs::write(&file, probe_pdf()).is_err() {
        return false;
    }
    let drew = (|| {
        let config = ServiceConfig::discover().ok()?;
        let mut document = DocRenderer::new(&file, None, config, Policy::default()).ok()?;
        let surface = SharedMemory::create(64 * 64 * 4).ok()?;
        let request = TileRequest {
            page: 0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            scale: 1.0,
            stride: 64 * 4,
            generation: 0,
        };
        match document.render_tile(&request, &surface).ok()? {
            TileResult::Done(tile) => Some(matches!(tile.kind, TileKind::Blank { rgb } if (rgb >> 16) & 0xFF > 0xC0 && rgb & 0xFFFF < 0x4040)),
            TileResult::Cancelled { .. } => None,
        }
    })();
    let _ = std::fs::remove_file(&file);
    drew == Some(true)
}

/// What a refusal to install comes to, for an agent: a code that doesn't change, our words, and what to do next.
fn refused_words(refused: &InstallRefused) -> AgentError {
    let said = refused.said();
    let exit = match said.kind {
        recto_update::updater::SaidKind::Usage => exit::USAGE,
        recto_update::updater::SaidKind::Busy => exit::BUSY,
        recto_update::updater::SaidKind::Failed => exit::FAILED,
    };
    AgentError::new(Code::lower(said.code), said.words, exit).then(said.next)
}

/// Whether a launch comes up behind what the user's doing: an update arriving, with the app not having been frontmost. Any other launch was asked for by someone, and comes forward as usual.
pub fn opens_in_background(arrived_by_update: bool, was_frontmost: bool) -> bool {
    arrived_by_update && !was_frontmost
}

/// What a menu's update says, as an alert: only with no window showing, closed or in the Dock, where the margin would have said it.
pub fn alert_for(words: String, window_showing: bool) -> Option<UpdateAlert> {
    (!window_showing).then(|| UpdateAlert {
        message: words,
        button: "OK".to_owned(),
    })
}

/// Whether something out of sight stays so: a window in the Dock, or the app hidden. At a launch an update opened, exactly as it was, whether it arrived, went back or wasn't installed, as what the user didn't watch mustn't put anything in front of them. Any other launch was asked for by someone, who's looking for the app.
pub fn stays_out_of_sight(was: bool, reopened_by_update: bool) -> bool {
    was && reopened_by_update
}

/// Going into full screen is another Space, and moves the user there. So not by itself: not until they come to it.
pub fn full_screen_on_showing(was_full_screen: bool, in_background: bool) -> (bool, bool) {
    (was_full_screen && !in_background, was_full_screen && in_background)
}

/// Where the Settings window goes: where it was, if that's on a screen there is now.
pub fn settings_place(saved: Option<SettingsWindow>, screens: &[ScreenInfo]) -> Option<Vec<f64>> {
    let saved = saved?;
    screens
        .iter()
        .any(|screen| {
            saved.x >= screen.visible_x
                && saved.x < screen.visible_x + screen.visible_width
                && saved.y >= screen.visible_y
                && saved.y < screen.visible_y + screen.visible_height
        })
        .then(|| vec![saved.x, saved.y])
}

/// What the host's one timer should do. No timer at all if there's nothing to wake for, and never less than a second, so a time already past can't make it spin.
pub fn timer_for(next_wake: Option<u64>, now_ms: u64) -> Option<UpdateTimer> {
    next_wake.map(|at_ms| UpdateTimer {
        at_ms,
        in_seconds: (at_ms.saturating_sub(now_ms) as f64 / 1000.0).max(1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a silent update puts a window over what the user's doing, or moves them to another Space by going into full screen. **Only as far as it can be tested with no screen:** that Rust says behind and not yet, for an update arriving in the background and nothing else. What AppKit then does stays unproven until a signed release on a real desktop.
    #[test]
    fn a_background_update_stays_behind_and_out_of_full_screen() {
        assert!(opens_in_background(true, false));
        assert!(!opens_in_background(true, true), "it was frontmost, so it comes back frontmost");
        assert!(
            !opens_in_background(false, false),
            "a launch someone asked for comes forward, whatever it was before"
        );
        assert_eq!(full_screen_on_showing(true, true), (false, true), "not yet");
        assert_eq!(full_screen_on_showing(true, false), (true, false), "at once, as normal");
        assert_eq!(full_screen_on_showing(false, true), (false, false));
    }

    /// Failure (owner, 2026-11-07: "It's crucial that app state is perfectly recovered before and after update"): a window the user put in the Dock, or an app they hid, comes back on the screen across an update they didn't watch. Or the other way, a launch someone asked for keeps the app out of sight. Out of sight stays so across an update and only then.
    #[test]
    fn out_of_sight_stays_so_across_an_update_and_only_then() {
        assert!(stays_out_of_sight(true, true));
        assert!(!stays_out_of_sight(true, false), "a launch someone asked for shows it");
        assert!(!stays_out_of_sight(false, true));
        assert!(!stays_out_of_sight(false, false));
    }

    /// Failure: a check chosen from the menu with the window closed says how it went nowhere the user can see. With no window it's an alert; with one, the margin says it and nothing's put over the page.
    #[test]
    fn a_menus_answer_is_an_alert_only_with_no_window() {
        assert!(alert_for("x".into(), false).is_some(), "closed or in the Dock");
        let alert = alert_for("You're up to date: RectoPDF 0.1.0.".into(), false).unwrap();
        assert_eq!(alert.message, "You're up to date: RectoPDF 0.1.0.");
        assert!(!alert.button.is_empty());
        assert_eq!(alert_for("x".into(), true), None);
    }

    /// Failure (review 1): a restore that comes after the host has shown the app without it, slow disk and all, writes the saved "hidden" or "in the Dock" back into the record while the app's on the screen, so the next update hides it. Once the host has decided, what it did stands.
    #[test]
    fn a_late_restore_keeps_what_the_host_did() {
        assert!(!out_of_sight_when_restored(true, Some(false)), "shown already, so shown");
        assert!(out_of_sight_when_restored(true, None), "not decided yet: what was saved, to decide on");
        assert!(out_of_sight_when_restored(false, Some(true)));
    }

    /// Failure (full check under load, 2026-11-11): the state read later than the window's second of patience, a window closed before an update came back open and one in the Dock came back shown, as the window went up without the state. A launch an update may have opened waits for it, up to a cap; any other launch doesn't.
    #[test]
    fn a_launch_an_update_opened_waits_for_the_state() {
        let second = Duration::from_secs(1);
        assert_eq!(restore_patience(false, second), Patience::ShowNow, "someone's looking for it");
        assert_eq!(restore_patience(true, second), Patience::WaitFor(UPDATE_RESTORE_PATIENCE - second));
        assert_eq!(restore_patience(true, UPDATE_RESTORE_PATIENCE), Patience::ShowNow, "not for ever");
    }

    /// Failure (CI, main at faa9180): what was done to a window that went up without the state, ⌘F and its field taking the keyboard, was lost when the state came in and made the window again. Until it's made from the state it takes nothing but the app's own items, and not for ever.
    #[test]
    fn a_window_up_without_the_state_takes_no_input_until_it_is_made_from_it() {
        let second = Duration::from_secs(1);
        assert!(!takes_input(false, second));
        assert!(takes_input(true, second));
        assert!(takes_input(false, INPUT_PATIENCE), "not for ever");
        assert!(!takes_input_now(false, "find.open"));
        assert!(takes_input_now(false, "app.quit") && takes_input_now(false, "file.settings"), "the app's own");
        assert!(takes_input_now(true, "find.open"));
    }

    /// Failure: Settings comes back where there's no screen any more, and can't be reached.
    #[test]
    fn settings_comes_back_only_on_a_screen_there_is() {
        let screen = ScreenInfo {
            id: 1,
            name: "a".into(),
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 800.0,
            visible_x: 0.0,
            visible_y: 0.0,
            visible_width: 1000.0,
            visible_height: 780.0,
        };
        let on = SettingsWindow { x: 100.0, y: 100.0 };
        assert_eq!(settings_place(Some(on), std::slice::from_ref(&screen)), Some(vec![100.0, 100.0]));
        assert_eq!(
            settings_place(Some(SettingsWindow { x: 2000.0, y: 100.0 }), std::slice::from_ref(&screen)),
            None
        );
        assert_eq!(settings_place(Some(on), &[]), None);
        assert_eq!(settings_place(None, &[screen]), None);
    }

    /// Failure: the page a new version's asked to draw isn't one PDFium reads as a page of that colour, so every update would be sent back, or it says so whatever's drawn. Only the worker's own build isn't here, so the real drawing's in `restore`'s tests of the real app.
    #[test]
    fn the_page_it_draws_is_a_whole_pdf() {
        let pdf = probe_pdf();
        assert!(pdf.starts_with(b"%PDF-1.4"));
        let text = String::from_utf8(pdf).unwrap();
        let table = text.rfind("xref\n0 5").unwrap();
        assert!(text.contains(&format!("startxref\n{table}\n")));
        assert!(text.contains("1 0 0 rg 0 0 64 64 re f"));
    }

    /// An updater on the update bed, which has heard of its own app's launch, for a [`LaunchFacts`] of a test's own to tell; and what's noted of a copy.
    fn on_the_bed() -> (crate::testkit::headless::HeadlessApp, crate::testkit::update_bed::UpdateBed, LaunchFacts) {
        use crate::testkit::update_bed::{OLD, UpdateBed};
        let disk = crate::testkit::headless::Disk::new();
        disk.install_app(OLD);
        let bed = UpdateBed::new();
        let app = disk.builder().update(&bed).start();
        let launch = LaunchFacts::new();
        launch.note(OLD.to_owned(), None, app.disk().app().display().to_string(), "15.0".to_owned());
        // The updater's look at a launch counted from here, and never waited at.
        recto_platform::slow::gate::hold(&[recto_platform::slow::Seam::Launch]);
        recto_platform::slow::gate::let_through(recto_platform::slow::Seam::Launch, 1_000);
        (app, bed, launch)
    }

    fn told() -> u64 {
        recto_platform::slow::gate::arrived(recto_platform::slow::Seam::Launch)
    }

    /// Failure (the record's-lock retry's rules, now the part's): a launch that didn't get the lock tells the updater at once, or never, or twice; or a
    /// launch that did is held back. Not kept: held and untold, nothing told; told once when asked, and a second telling has nothing to tell; then not
    /// untold, so settling can't leave it held. Kept: told at once, and nothing held.
    #[test]
    fn a_launch_held_back_is_told_once_and_only_when_asked() {
        let (app, _bed, launch) = on_the_bed();
        let updates = &app.core().updates;
        assert_eq!(launch.hold_or_tell(false, updates), recto_update::updater::AtLaunch::GoOn);
        assert!(launch.held() && launch.untold(), "held back, untold");
        assert_eq!(told(), 0, "nothing told while held");
        assert_eq!(launch.tell(updates), recto_update::updater::AtLaunch::GoOn);
        assert_eq!(launch.tell(updates), recto_update::updater::AtLaunch::GoOn);
        assert_eq!(told(), 1, "told once");
        assert!(!launch.untold(), "told, so settling leaves nothing held");

        let kept = LaunchFacts::new();
        kept.note(
            crate::testkit::update_bed::OLD.to_owned(),
            None,
            app.disk().app().display().to_string(),
            "15.0".to_owned(),
        );
        assert_eq!(kept.hold_or_tell(true, updates), recto_update::updater::AtLaunch::GoOn);
        assert_eq!(told(), 2, "kept: told at once");
        assert!(!kept.held() && kept.take_held().is_none(), "and nothing held");
    }

    /// Failure (the record's-lock retry's rules, now the part's): a first frame shown while the launch is held back is told before the launch (so a late keep
    /// says it came up with no tabs read), or lost; or the updater, once told of a held launch, never hears of the first frame. Kept while held, the first
    /// time only; told, and the launch no longer held, once the held facts are told on the tell's way.
    #[test]
    fn a_first_frame_while_held_is_kept_until_the_launch_is_told() {
        let (app, _bed, launch) = on_the_bed();
        let updates = &app.core().updates;
        launch.hold_or_tell(false, updates);
        launch.first_frame_shown(updates);
        let first = lock(&launch.0).first_frame_at;
        assert!(first.is_some(), "kept while held");
        launch.first_frame_shown(updates);
        assert_eq!(lock(&launch.0).first_frame_at, first, "the first frame's time, not a later one");
        let facts = launch.take_held().expect("held back");
        assert!(facts.is_some(), "with the facts untold");
        tell(&launch, updates, facts);
        assert_eq!(told(), 1);
        assert!(
            !launch.held() && lock(&launch.0).first_frame_at.is_none(),
            "no longer held, and the first frame told"
        );
        assert!(launch.take_held().is_none(), "nothing left to tell");
    }

    /// Failure: a time that's passed arms a timer that fires at once and again and again, or a time far off comes out as anything but itself. At rest it's one timer a day.
    #[test]
    fn the_timer_never_spins() {
        assert_eq!(timer_for(None, 5), None);
        assert_eq!(timer_for(Some(5), 10_000).unwrap().in_seconds, 1.0);
        assert_eq!(timer_for(Some(5_000), 5_000).unwrap().in_seconds, 1.0);
        let day = timer_for(Some(86_400_000), 0).unwrap();
        assert_eq!(day.in_seconds, 86_400.0);
        assert_eq!(day.at_ms, 86_400_000);
    }
}

/// When the host's one timer should go off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpdateTimer {
    /// What identifies this time: the host arms the timer again only when this changes.
    pub at_ms: u64,
    pub in_seconds: f64,
}

/// One switch of the Settings window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingRow {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub on: bool,
    pub enabled: bool,
}

/// An update waiting or under way, with the button that installs it now: Rust's words ([`recto_update::updater::UpdateAction`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsAction {
    pub id: String,
    pub title: String,
    pub hint: String,
    pub button: String,
    pub enabled: bool,
}

/// The Settings window, as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsModel {
    pub title: String,
    pub rows: Vec<SettingRow>,
    /// A waiting update and its button, above the switches. None when there's nothing to install.
    pub action: Option<SettingsAction>,
    /// How updates stand now.
    pub status: String,
    /// A link to the privacy statement: its words, and where it opens.
    pub privacy_title: String,
    pub privacy_url: String,
}

/// What an update chosen from the menu came to, for an app-modal alert. Rust's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateAlert {
    pub message: String,
    pub button: String,
}
