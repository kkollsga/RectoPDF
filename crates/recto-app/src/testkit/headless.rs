//! **The app with no window** (step R1.3): the same core the window runs on ([`Inner::start`]), started on fakes of what the host gives it, so a test of what the app
//! decides runs in this process, in milliseconds, instead of launching `RectoPDF.app`.
//!
//! What's real: the session thread, the state's reading and writing on the real [`RecordStore`] in a temp folder, files read on their own threads with the real
//! loader, the page views, the saver's writer, a quit's flush. What's a fake: the clock ([`FakeClock`]), the listener ([`FakeListener`]), the window (nothing draws,
//! and what the host would call when its window opens and closes is called by the kit, in the host's order). What's not there unless asked: the control socket (an
//! agent's command goes through the same [`Host::run`] the socket's server calls), the search index and generated contents.
//!
//! **How a test stays deterministic.** There's no fixed sleep anywhere here. Every wait is for a condition, bounded, and fails naming what it was still waiting for:
//! [`HeadlessApp::settle`] waits for the session thread to have taken everything queued and every file being read, then for the threads the product starts for a
//! password, a look at files, a write of the record and Find to have finished, and takes what they queued. Time the app's rules go by moves only by
//! [`HeadlessApp::advance`]. What's left to real time is what's real in the product too: a flush's limit on a slow disk, the sleep files' lock.

use super::update_bed::UpdateBed;
use super::{FakeClock, FakeListener, Heard};
use crate::clock::Clock as _;
use crate::core::updates::UpdateSeams;
use crate::core::{Inner, Start};
use crate::launch::LaunchArgs;
use crate::listener::QuitAnswer;
use crate::queue::CoreCall;
use crate::reader_view::ReaderView;
use recto_agent::{AgentError, Level};
use recto_control::StatePaths;
use recto_control::host::Host;
use recto_platform::slow::Seam;
use recto_testpdf::TempDir;
use recto_update::plan::Plan;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// The longest any one wait in the kit is given before the test fails with what it saw. Far past anything the product takes on a loaded machine, and short enough
/// that a hang fails the test rather than the run.
pub const WAIT_LIMIT: Duration = Duration::from_secs(20);

/// How often a bounded wait looks again.
const LOOK_EVERY: Duration = Duration::from_millis(2);

/// **A file's path outside every folder a quiet run's test may use** (its state and the system's temp folders), for a test of what's refused there: in the
/// build's own `target` folder. `None` when the checkout itself is in a temp folder, where nothing near it is outside; the test then says it skipped that
/// part rather than calling a test's own file someone else's.
pub fn outside_the_tests_folders(name: &str) -> Option<PathBuf> {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").canonicalize().ok()?;
    if recto_control::quiet::temp_folders().iter().any(|temp| target.starts_with(temp)) {
        eprintln!("this checkout is in a temp folder: the part about a file outside the test's folders is skipped");
        return None;
    }
    Some(target.join(format!("{}-{name}", std::process::id())))
}

/// **A state folder and a folder for the test's files, kept between runs of the app,** as a disk is between launches. [`HeadlessApp::quit`] gives it back, to
/// [`Disk::start`] again on.
pub struct Disk {
    root: TempDir,
    paths: StatePaths,
    /// The time of day the next launch on this disk starts at, in milliseconds since 1970: what the updater's remembered times go by. It moves on with each run's
    /// clock, as a Mac's does between launches.
    wall_ms: u64,
}

impl Disk {
    /// A fresh one, in the system's temp folder: nothing in the state, no files.
    #[must_use]
    pub fn new() -> Self {
        let root = TempDir::new("recto-headless-");
        let state = root.join("state");
        // Private to this user, as a state folder must be, or the record's lock isn't taken and the app runs as one that isn't keeping it.
        #[cfg(unix)]
        let made = std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700).create(&state);
        #[cfg(not(unix))]
        let made = std::fs::create_dir(&state);
        made.expect("a state folder");
        std::fs::create_dir(root.join("files")).expect("a folder for the test's files");
        let paths = StatePaths::overridden(&state).expect("a temp state folder is allowed");
        Self {
            root,
            paths,
            wall_ms: recto_update::wall_ms(),
        }
    }

    /// The state folder's paths: the record, the backups, the socket.
    #[must_use]
    pub fn paths(&self) -> &StatePaths {
        &self.paths
    }

    /// Where the test's own files go: outside the state folder, as a person's documents are.
    #[must_use]
    pub fn files(&self) -> PathBuf {
        self.root.join("files")
    }

    /// Writes a file of the test's, by name, and says where it is.
    pub fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.files().join(name);
        std::fs::write(&path, bytes).expect("writing a test's file");
        path
    }

    /// A PDF of `pages` pages, each with a line of text naming it, by name.
    pub fn pdf(&self, name: &str, pages: usize) -> PathBuf {
        let pages: Vec<recto_testpdf::Page> = (1..=pages)
            .map(|page| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("{name} page {page}")])))
            .collect();
        self.file(name, &recto_testpdf::pages(&pages))
    }

    /// The app, started on this disk as a launch would be, with what [`Builder`] gives by default.
    #[must_use]
    pub fn start(self) -> HeadlessApp {
        Builder::default().start_on(self)
    }

    /// A builder, to start the app on this disk with something other than the defaults.
    #[must_use]
    pub fn builder(self) -> DiskBuilder {
        DiskBuilder {
            disk: self,
            builder: Builder::default(),
        }
    }
}

impl Default for Disk {
    fn default() -> Self {
        Self::new()
    }
}

/// What the app starts with, other than the disk.
#[derive(Clone, Debug, Default)]
pub struct Builder {
    indexing: bool,
    clock_ms: u64,
    open: Vec<PathBuf>,
    update: Option<UpdateBed>,
    state_late: bool,
    held: Vec<Seam>,
    leaving_wait_ms: u64,
    another_keeps: bool,
}

impl Builder {
    /// **A pushed-out tab's sleep file waits this long** on the fake clock ([`crate::sleeper::LEAVING_WAIT_MS`] in the app). None by default, so a test of
    /// anything else has its sleep files written as it settles, and doesn't wait out a cool-down its clock won't move through.
    #[must_use]
    pub fn leaving_wait(mut self, ms: u64) -> Self {
        self.leaving_wait_ms = ms;
        self
    }

    /// With the search index and generated contents, as the app has them. Off by default: they start threads of their own that a test of anything else needn't wait for.
    #[must_use]
    pub fn indexing(mut self, on: bool) -> Self {
        self.indexing = on;
        self
    }

    /// Where the fake clock starts, in milliseconds.
    #[must_use]
    pub fn clock_at(mut self, ms: u64) -> Self {
        self.clock_ms = ms;
        self
    }

    /// Files named at launch, as a double-click on a PDF with the app not running gives them.
    #[must_use]
    pub fn open(mut self, path: impl Into<PathBuf>) -> Self {
        self.open.push(path.into());
        self
    }

    /// **As an installed copy that updates itself** (step R1.4): told, as the host tells it, which version it is and where its bundle is (the disk's
    /// [`Disk::install_app`]), on a test build's source, with this bed's releases and installer and the disk's time of day for the updater's clock.
    #[must_use]
    pub fn update(mut self, bed: &UpdateBed) -> Self {
        self.update = Some(bed.clone());
        self
    }

    /// **The state read late,** as off a slow disk: the host's second of patience is over before it's read, so the window goes up without it (or, for a launch an
    /// update may have opened, waits on), and it comes in after.
    #[must_use]
    pub fn state_late(mut self) -> Self {
        self.state_late = true;
        self
    }

    /// **Another copy keeps the record at the launch** (a store the test holds): the launch is let start without its lock, which a launch otherwise fails on, and
    /// its trying again for it is what's tested ([`crate::lifecycle::keeping`]).
    #[must_use]
    pub fn another_keeps(mut self) -> Self {
        self.another_keeps = true;
        self
    }

    /// **These seams held from the launch on** ([`recto_platform::slow::gate`]): what arrives at each waits for [`HeadlessApp::let_through`], in the order it came,
    /// with no sleep and no timing. With [`Seam::State`] held the launch goes on without the state, as past its patience, and [`HeadlessApp::let_state_in`] brings
    /// it in.
    #[must_use]
    pub fn hold(mut self, seams: &[Seam]) -> Self {
        self.held.extend_from_slice(seams);
        self
    }

    /// On a fresh disk.
    #[must_use]
    pub fn start(self) -> HeadlessApp {
        self.start_on(Disk::new())
    }

    /// **A launch, as the host makes it, with no window:** the core started, the state read on its thread and waited for, the window "opened" as the host does when
    /// the state's in, then `ready`, and everything that set going settled.
    fn start_on(self, disk: Disk) -> HeadlessApp {
        recto_platform::slow::gate::hold(&self.held);
        let clock = Arc::new(FakeClock::at(self.clock_ms));
        let listener = Arc::new(FakeListener::default());
        let (update_source, update_seams) = match &self.update {
            Some(bed) => (UpdateBed::source(), bed.seams(disk.wall_ms, &clock)),
            // A test's source: no network, whatever the environment says.
            None => (recto_update::source::resolve(false, true, None, None), UpdateSeams::default()),
        };
        let inner = Inner::start_with(
            Start {
                listener: Arc::clone(&listener) as Arc<dyn crate::Listener>,
                clock: Arc::clone(&clock) as Arc<dyn crate::clock::Clock>,
                paths: disk.paths.clone(),
                args: LaunchArgs::default(),
                update_source,
                socket: false,
                indexing: self.indexing,
            },
            update_seams,
        )
        .expect("the core starts");
        inner.sleeper.set_leaving_wait(self.leaving_wait_ms);
        // As the app does unless another copy has it: a test whose app isn't keeping the record would test another path, without saying so.
        assert!(
            self.another_keeps || inner.saver.store.holds_the_record(),
            "the headless app didn't get the record's lock in {}: {}",
            disk.paths.root.display(),
            inner.saver.store.why_not_keeping().unwrap_or_else(|| "no reason was kept".to_owned())
        );
        if self.update.is_some() {
            // What the host says at launch, before the state's read: this copy's version and bundle, from its own Info.plist.
            let app = disk.app();
            let version = disk
                .app_version()
                .unwrap_or_else(|| panic!("no app installed at {}: Disk::install_app first", app.display()));
            let release = recto_update::version::parse(&version).map(|parsed| {
                let (a, b, c) = parsed.release;
                format!("{a}.{b}.{c}")
            });
            let development = recto_update::version::is_development(&version).then(|| version.clone());
            inner.note_update_facts(release.unwrap_or(version), development, app.display().to_string(), "15.0".to_owned());
        }
        let app = HeadlessApp {
            inner: Some(inner),
            clock_at_start: clock.now_ms(),
            clock,
            listener,
            disk: Some(disk),
            host: std::sync::Mutex::new(HostPlay::default()),
            open: self.open.clone(),
            view: std::sync::Mutex::new(super::scrolled::ScrolledView::default()),
        };
        let state_held = self.held.contains(&Seam::State);
        // The state read late, as a slow disk reads it: the host's second of patience is over before the reading even starts, and what the core says to that is
        // what the host hears first.
        if self.state_late || state_held {
            app.clock.advance(u64::try_from(crate::core::RESTORE_PATIENCE.as_millis()).unwrap_or(u64::MAX));
            app.core().restore_patience_over();
            assert!(app.core().core.flush(WAIT_LIMIT), "the patience's call taken");
            app.host_hears();
        }
        // `applicationDidFinishLaunching`: the state's read on a thread of its own. The host's own timer isn't armed: a test that wants the window up without the
        // state says so ([`Builder::state_late`]).
        app.core().begin_restore();
        if self.held.contains(&Seam::Launch) {
            // What this copy is, looked at before the state's read: held there, the state's read waits behind it.
            app.wait_until("the launch's look to wait at its seam", || {
                recto_platform::slow::gate::waiting(Seam::Launch) > 0
            });
            app.settle();
            return app;
        }
        if state_held {
            app.wait_until("the state's read to wait at its seam", || recto_platform::slow::gate::waiting(Seam::State) > 0);
            app.settle();
            return app;
        }
        app.state_taken_in();
        app
    }
}

/// **What the host does on `restore_ready`,** as `App.swift`'s `restoreReady` and `restoredLate` do it, by the same calls: the first time, the window's opened or
/// not and the launch goes on; a later time, the state's come in after the window went up without it (or a late try at the record's lock has restored the saved
/// session), and the window's made again from it if Rust's rule says so now ([`crate::core::Lifecycle::may_remake_window`]), with the appearance taken either way.
#[derive(Default)]
pub(crate) struct HostPlay {
    heard: usize,
    launch_done: bool,
    /// How many times the window was made again from the state: at most once a launch.
    pub(crate) remade: usize,
}

impl HostPlay {
    /// Takes in every `restore_ready` heard since the last look.
    fn hears(&mut self, app: &HeadlessApp, open: &[PathBuf]) {
        let heard = app.listener.count(&Heard::RestoreReady);
        while self.heard < heard {
            self.heard += 1;
            let inner = app.core();
            if !self.launch_done {
                self.launch_done = true;
                let with_state = inner.state_read();
                if inner.window_should_open() {
                    inner.window_opened();
                } else {
                    inner.window_closed();
                }
                if with_state {
                    inner.shown_with_state();
                }
                inner.ready(open);
            } else if inner.state_read() && inner.life.may_remake_window() {
                self.remade += 1;
                // The window made again from the state: the old one goes without counting as the person closing it (`windowDidClose` while replacing), and the
                // new one opens.
                if !crate::lock(&inner.window.app).window_closed {
                    inner.window_replaced();
                    inner.window_opened();
                }
                inner.shown_with_state();
            } else if inner.state_read() {
                inner.shown_with_state();
            }
        }
    }
}

/// [`Builder`] on a disk of the test's.
pub struct DiskBuilder {
    disk: Disk,
    builder: Builder,
}

impl DiskBuilder {
    #[must_use]
    pub fn indexing(mut self, on: bool) -> Self {
        self.builder = self.builder.indexing(on);
        self
    }

    #[must_use]
    pub fn open(mut self, path: impl Into<PathBuf>) -> Self {
        self.builder = self.builder.open(path);
        self
    }

    #[must_use]
    pub fn update(mut self, bed: &UpdateBed) -> Self {
        self.builder = self.builder.update(bed);
        self
    }

    #[must_use]
    pub fn state_late(mut self) -> Self {
        self.builder = self.builder.state_late();
        self
    }

    #[must_use]
    pub fn hold(mut self, seams: &[Seam]) -> Self {
        self.builder = self.builder.hold(seams);
        self
    }

    #[must_use]
    pub fn leaving_wait(mut self, ms: u64) -> Self {
        self.builder = self.builder.leaving_wait(ms);
        self
    }

    #[must_use]
    pub fn another_keeps(mut self) -> Self {
        self.builder = self.builder.another_keeps();
        self
    }

    #[must_use]
    pub fn start(self) -> HeadlessApp {
        self.builder.start_on(self.disk)
    }
}

/// **The running app, with no window.** Dropped without [`Self::quit`], it ends as a killed app does: nothing more is written.
pub struct HeadlessApp {
    inner: Option<Arc<Inner>>,
    clock: Arc<FakeClock>,
    /// Where the clock was at the launch, so the disk's time of day moves on by what this run's clock did.
    clock_at_start: u64,
    listener: Arc<FakeListener>,
    disk: Option<Disk>,
    /// What the host has made of what it's heard: `App.swift`'s part of a launch ([`HostPlay`]).
    pub(crate) host: std::sync::Mutex<HostPlay>,
    /// Files named at launch.
    open: Vec<PathBuf>,
    /// The page being read's view as the host has it, by [`Self::person_scrolls`]: where it's scrolled to and the last scroll it was sent that it's
    /// arrived at; and the last scroll it was sent, from the layer changes taken, `(y, seq)`.
    pub(super) view: std::sync::Mutex<super::scrolled::ScrolledView>,
}

impl HeadlessApp {
    /// What the host does with every `restore_ready` heard since it last looked.
    fn host_hears(&self) {
        let mut host = crate::lock(&self.host);
        host.hears(self, &self.open);
    }

    /// The state read and taken in, the host's part of that done, and everything it set going settled.
    fn state_taken_in(&self) {
        self.wait_until("the state to be read and taken in", || self.core().state_read());
        assert!(self.core().core.flush(WAIT_LIMIT), "what came of the state taken");
        self.host_hears();
        self.settle();
    }

    /// **The state comes in now,** held at its seam since the launch ([`Builder::hold`]): read, taken in, the window made again from it as the host makes it (once
    /// the window's remake is let through, if that's held too), and settled.
    pub fn let_state_in(&self) {
        recto_platform::slow::gate::let_through(Seam::State, 1);
        self.state_taken_in();
    }

    /// The next `n` arrivals at a held seam go, in the order they came; a call to the host kept there is made now, and the host does with it what it would.
    pub fn let_through(&self, seam: Seam, n: u64) {
        recto_platform::slow::gate::let_through(seam, n);
        self.host_hears();
    }

    /// How many are waiting at a held seam now.
    #[must_use]
    pub fn waiting(&self, seam: Seam) -> u64 {
        recto_platform::slow::gate::waiting(seam)
    }

    /// On a fresh disk, with the defaults.
    #[must_use]
    pub fn start() -> Self {
        Builder::default().start()
    }

    #[must_use]
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// The core, as `AppCore` holds it: what a test calls for anything the window calls, by the same method.
    ///
    /// # Panics
    ///
    /// Only once the app has quit, which takes it by value, so never.
    #[must_use]
    pub fn core(&self) -> &Arc<Inner> {
        self.inner.as_ref().expect("the app is running")
    }

    /// The disk it runs on.
    ///
    /// # Panics
    ///
    /// Never while it runs.
    #[must_use]
    pub fn disk(&self) -> &Disk {
        self.disk.as_ref().expect("the app is running")
    }

    /// **A call on the session thread's queue,** for what isn't a person's action: the system's and the workers' news. What the window does is [`Self::act`].
    /// Doesn't wait: [`Self::settle`] does.
    pub fn send(&self, call: CoreCall) {
        self.core().core.push(call);
    }

    /// **What the person does at the window,** by the one way the window says it ([`crate::core::controls::UiAction`]). Doesn't wait: [`Self::settle`] does.
    pub fn act(&self, action: crate::core::controls::UiAction) -> crate::core::controls::Acted {
        self.core().act(action)
    }

    /// **A person's command,** as a line of JSON the way `recto` sends it, run as the window runs the person's commands. Doesn't wait: [`Self::settle`] does.
    pub fn person(&self, line: &str) {
        crate::core::person::submit(self.core(), recto_agent::parse(line).expect("a command"));
    }

    /// Opens a file as the window's Open does, and settles.
    pub fn open(&self, path: &Path) {
        self.act(crate::core::controls::UiAction::Open {
            path: path.display().to_string(),
        });
        self.settle();
    }

    /// **An agent's command, at `level`,** as a line of JSON the way `recto` sends it, through the same refusal and [`Host::run`] as the socket's server. Waits for what
    /// it answers, and doesn't settle.
    ///
    /// # Errors
    ///
    /// The command's own refusal or failure, as the agent would get it.
    pub fn agent(&self, level: Level, line: &str) -> Result<Value, AgentError> {
        let command = recto_agent::parse(line)?;
        if !recto_agent::allowed(level, command.access()) {
            return Err(recto_agent::refusal(recto_agent::table::display_name(command.tag()), level, command.access()));
        }
        let mut events = Vec::new();
        self.core().host.run(recto_agent::Actor::Agent(level), &command, &mut events)
    }

    /// **`recto <words>`, at `level`,** as the CLI turns the words into its line and sends it: [`Self::agent`] on that line.
    ///
    /// # Errors
    ///
    /// The words' usage error, or the command's own refusal or failure.
    pub fn recto(&self, level: Level, words: &[&str]) -> Result<Value, AgentError> {
        let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
        let line = recto_agent::table::parse_cli(&words)?.wire();
        self.agent(level, &line)
    }

    /// **Waits for the app to be at rest:** everything queued for the session thread taken, every file being read in, every password being tried and every look at
    /// files done, the record's writer through, and the page views' layer changes taken as the host takes them. Bounded, and never a fixed sleep.
    ///
    /// # Panics
    ///
    /// Past [`WAIT_LIMIT`], naming what was still under way.
    pub fn settle(&self) {
        let began = Instant::now();
        loop {
            let left = WAIT_LIMIT.saturating_sub(began.elapsed());
            assert!(
                self.core().core.flush(left),
                "settle: the session thread didn't take what was queued, with the files being read, in {WAIT_LIMIT:?}; still under way: {:?}",
                self.under_way()
            );
            self.take_ops();
            let busy = self.under_way();
            if busy.is_empty() {
                // What finished just now queued what came of it, and that's taken too.
                assert!(
                    self.core().core.flush(left),
                    "settle: the session thread didn't take the last of what was queued"
                );
                return;
            }
            assert!(began.elapsed() < WAIT_LIMIT, "settle: still under way after {WAIT_LIMIT:?}: {busy:?}");
            std::thread::sleep(LOOK_EVERY); // ratchet: poll
        }
    }

    /// What's still going on off the session thread, in words, for a wait's failure.
    fn under_way(&self) -> Vec<String> {
        let inner = self.core();
        let mut busy = Vec::new();
        if crate::lock(&inner.tools.prompts.passwords).trying() {
            busy.push("a password being tried".to_string());
        }
        let reads = inner.life.reads_running.load(Ordering::SeqCst);
        if reads > 0 {
            busy.push(format!("{reads} file(s) being read"));
        }
        let persons = inner.life.persons_running.load(Ordering::SeqCst);
        if persons > 0 {
            busy.push(format!("{persons} of the person's command(s) on a thread of their own"));
        }
        // The window's Save and Save As run on a thread of their own, and what they did is announced once they're through.
        let saves = inner.tools.save_ui.running.load(Ordering::SeqCst);
        if saves > 0 {
            busy.push(format!("{saves} of the window's save(s)"));
        }
        if inner.life.others_looking.load(Ordering::SeqCst) > 0 {
            busy.push("the other tabs' files being looked at".to_string());
        }
        let looks = inner.sleeper.looks_running();
        if looks > 0 {
            busy.push(format!("{looks} look(s) at a woken tab's file"));
        }
        if inner.saver.is_writing() {
            busy.push("the record being written".to_string());
        }
        // Its thread goes once it's said how its last query ended, which the session thread takes as Find's news.
        if inner.host.session.finder().has_thread() {
            busy.push("Find looking".to_string());
        }
        busy
    }

    /// The page views' layer changes, taken and applied as the host's UI thread does on `ops_ready`, so what's drawn and let go goes as it would.
    fn take_ops(&self) {
        let inner = self.core();
        let reading = inner.panes.reading_engine(&inner.engine);
        for engine in inner.panes.engines(&inner.engine) {
            let ops = engine.drain();
            if engine.same_as(&reading) {
                self.note_scrolls_sent(&ops);
            }
            engine.applied();
        }
    }

    /// Moves the app's clock on, and has the session thread look at its deadlines again. Doesn't settle. The time since the person last touched the app and the
    /// updater's clock move with it.
    pub fn advance(&self, ms: u64) {
        self.clock.advance(ms);
        self.core().core.nudge();
        self.core().sleeper.poke_leaving();
    }

    /// The person touched the app now, by the clock, as the host's event monitor says.
    pub fn touch(&self) {
        self.core().note_input();
    }

    /// The updater's one timer went off, as the host's does when [`Inner::update_timer_fired`] is due.
    pub fn update_tick(&self) {
        self.core().update_timer_fired();
    }

    /// `recto update status`'s answer, as `update status` gives it.
    #[must_use]
    pub fn update_status(&self) -> Value {
        self.core().update_report()
    }

    /// Waits for `update status` to say `status`.
    ///
    /// # Panics
    ///
    /// Past [`WAIT_LIMIT`], with the status it last said.
    pub fn wait_for_status(&self, status: &str) -> Value {
        self.wait_until(&format!("update status {status}"), || self.update_status()["status"] == status);
        self.update_status()
    }

    /// **The hand-off's end, as the process sees it:** the app was told to quit for an update ([`crate::Listener::hand_off_quit`]), and it's gone, as a process
    /// ending is, with nothing more written. Says the plan the installer was started on, beside the disk, for [`Disk::install`] and then [`Disk::start`].
    ///
    /// # Panics
    ///
    /// If the app isn't told to quit for an update in [`WAIT_LIMIT`], or no installer was started on a plan.
    #[must_use]
    pub fn handed_off(mut self, bed: &UpdateBed) -> (Plan, Disk) {
        self.wait_until("the app told to quit for an update", || self.listener.count(&Heard::HandOffQuit) > 0);
        let plan = bed.installer.plans().pop().expect("an installer started on a plan");
        (plan, self.end())
    }

    /// The reader's model, as the window pulls it.
    #[must_use]
    pub fn reader(&self) -> ReaderView {
        crate::lock(&self.core().published).reader.clone()
    }

    /// The sidebar's model but its rows, as the window pulls it.
    #[must_use]
    pub fn sidebar(&self) -> crate::SidebarView {
        (*crate::lock(&self.core().published).sidebar).clone()
    }

    /// The sidebar's tab rows.
    #[must_use]
    pub fn tabs(&self) -> Vec<crate::SidebarRow> {
        self.core().sidebar_tabs()
    }

    /// `qa state`'s `session`: the record as it would be written now, and each tab's edits in full.
    #[must_use]
    pub fn session_json(&self) -> Value {
        self.core().state_json()
    }

    /// `qa state`'s `record`: how the record's writing and the sleep files have gone.
    #[must_use]
    pub fn record_json(&self) -> Value {
        self.core().record_report()
    }

    /// Everything the host has heard so far, in order.
    #[must_use]
    pub fn heard(&self) -> Vec<Heard> {
        self.listener.heard()
    }

    /// The listener, to count or take what it heard.
    #[must_use]
    pub fn listener(&self) -> &FakeListener {
        &self.listener
    }

    /// The fake clock.
    #[must_use]
    pub fn clock(&self) -> &FakeClock {
        &self.clock
    }

    /// Waits for `what` to be true, looking every few milliseconds.
    ///
    /// # Panics
    ///
    /// Past [`WAIT_LIMIT`], naming `what` and what's still under way.
    pub fn wait_until(&self, what: &str, done: impl Fn() -> bool) {
        self.wait_for(what, || if done() { Ok(()) } else { Err(String::new()) });
    }

    /// Waits for `what`, as [`Self::wait_until`] does, with `done` saying what's still missing, which a failure says too.
    ///
    /// # Panics
    ///
    /// Past [`WAIT_LIMIT`], naming `what`, what was still missing at the last look, and what's still under way.
    pub fn wait_for(&self, what: &str, done: impl Fn() -> Result<(), String>) {
        let began = Instant::now();
        loop {
            let missing = match done() {
                Ok(()) => return,
                Err(missing) if missing.is_empty() => String::new(),
                Err(missing) => format!(", missing: {missing}"),
            };
            assert!(
                began.elapsed() < WAIT_LIMIT,
                "waited {WAIT_LIMIT:?} for {what}{missing}; under way: {:?}; heard: {:?}",
                self.under_way(),
                self.listener.heard().len()
            );
            std::thread::sleep(LOOK_EVERY); // ratchet: poll
        }
    }

    /// **A quit, as the menu asks for one:** everything written on the quit's thread, the answer heard, the socket ended, and the core let go of as the process would
    /// be ended, so its disk can be started on again in this process. Says what the quit answered, beside the disk. A quiet run's host goes whatever the answer, and
    /// so does this: [`Self::ask_to_quit`] is for a test of the question.
    ///
    /// # Panics
    ///
    /// If no answer comes in [`WAIT_LIMIT`], or something keeps the core past it.
    #[must_use]
    pub fn quit(self) -> (QuitAnswer, Disk) {
        let answer = self.ask_to_quit();
        (answer, self.quit_anyway())
    }

    /// **A quit asked for, and its answer,** with the app left running as the host's question leaves it: go, or the words of Quit Anyway or Stay.
    ///
    /// # Panics
    ///
    /// If no answer comes in [`WAIT_LIMIT`].
    #[must_use]
    pub fn ask_to_quit(&self) -> QuitAnswer {
        let answers = || {
            self.listener
                .heard()
                .into_iter()
                .filter_map(|heard| match heard {
                    Heard::QuitAnswer(answer) => Some(answer),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let before = answers().len();
        self.core().quit_requested();
        self.wait_until("the quit's answer", || answers().len() > before);
        answers().pop().expect("an answer")
    }

    /// The quit's question answered with Stay, as the host's alert says it.
    pub fn stay(&self) {
        self.core().quit_stayed();
    }

    /// The host's way out once a quit's answered (go, or Quit Anyway): `applicationWillTerminate`, which a quiet run reaches whatever the answer, and the process
    /// gone.
    #[must_use]
    pub fn quit_anyway(mut self) -> Disk {
        self.core().shutdown();
        self.end()
    }

    /// **The app killed:** nothing it was asked is taken, nothing more is written, nothing's drained or flushed, and the disk is as the last write left it.
    #[must_use]
    pub fn kill(mut self) -> Disk {
        self.end()
    }

    /// **Lets go of the core, as a process ending does,** whether by a quit's way out or a kill: from this moment nothing more is written, whatever's still
    /// running; nothing queued for the session thread is taken; every held seam opens, so what waited there goes on to find it can't write; the page views end;
    /// and every thread that held the core, or the updater, lets go of it.
    fn end(&mut self) -> Disk {
        if let Some(inner) = self.inner.take() {
            inner.saver.store.let_go();
            inner.core.abandon();
            recto_platform::slow::gate::open_all();
            for engine in inner.panes.engines(&inner.engine) {
                engine.shut_down();
            }
            let began = Instant::now();
            while Arc::strong_count(&inner) > 1 {
                assert!(
                    began.elapsed() < WAIT_LIMIT,
                    "the core is still held by {} other(s) {WAIT_LIMIT:?} after it was ended",
                    Arc::strong_count(&inner) - 1
                );
                std::thread::sleep(LOOK_EVERY); // ratchet: poll
            }
            // And the updater's threads end with it: an install that had handed off was waiting for its process to go, on a clock that stands still here,
            // and looks at whether its app has ended instead.
            let updater = Arc::downgrade(&inner.updates);
            drop(inner);
            let began = Instant::now();
            while updater.strong_count() > 0 {
                assert!(
                    began.elapsed() < WAIT_LIMIT,
                    "the updater is still held by {} {WAIT_LIMIT:?} after its app ended: an update's thread that didn't end with it",
                    updater.strong_count()
                );
                std::thread::sleep(LOOK_EVERY); // ratchet: poll
            }
        }
        let mut disk = self.disk.take().expect("the disk");
        // The time of day moved on with this run's clock, from where it was at its launch.
        disk.wall_ms = disk.wall_ms.saturating_add(self.clock.now_ms().saturating_sub(self.clock_at_start));
        disk
    }
}

impl Drop for HeadlessApp {
    fn drop(&mut self) {
        // A test that failed is unwinding through here: its failure is the one to see, so nothing here adds a second.
        if self.inner.is_some() && !std::thread::panicking() {
            drop(self.end());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::HeadlessApp;

    /// Failure (`late_lock_order`'s belt test, once in a 47-test run at a load of about 30, 2026-11-14: "Keeps" read where a writer would have found
    /// "NotKept"): [`HeadlessApp::settle`] returned with a write asked for (`write_soon`) that no writer had started, and the test read the state before any
    /// writer had looked. Settle's flush is a call on the session thread's queue, and that thread takes a queued call before it looks at its deadlines, so the
    /// flush came back with the write due and its writer not yet started, and nothing else was under way. Forced: the session thread held between two calls
    /// while the write is asked for, where settle's flush finds it; what settle counts as under way must name the write.
    #[test]
    fn a_write_asked_for_and_not_yet_started_keeps_settle_waiting() {
        let app = HeadlessApp::start();
        let held = app.hold_session_thread();
        app.core().saver.write_soon();
        let busy = app.under_way();
        drop(held);
        app.settle();
        assert!(
            busy.iter().any(|what| what == "the record being written"),
            "settle would have returned: {busy:?}"
        );
    }
}
