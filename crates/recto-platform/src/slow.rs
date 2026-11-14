//! **A slow mode, for tests: every waiting point made late on purpose** (guide, "Code that's right only when it's fast"). On 2026-11-11 `main` went red on CI three pushes running, each time on a path only a slower machine took: a state read after the window's second of patience, a worker's answer after a deadline, a window laid out only when asked. Each was found one push at a time. A quiet run of a test build with `RECTO_QA_SLOW` set takes every one of those paths at once, here.
//!
//! **Where it waits.** Each [`Seam`] is one waiting point, and each has a one-line call where it is: [`pause`] on a background thread that's about to hand on what it waited for, or [`later`] for a call to the host, which arrives late on a thread of this module's and blocks nobody. A [`pause`] on the UI thread is refused and counted, so a seam put in the wrong place can't hang the app (AGENTS rule 3): `ui_refusals` in the report says so.
//!
//! **When.** Never in a release build: [`RELEASE`] is compiled in, as `recto_update::RELEASE_BUILD` is, and [`arm`] does nothing there whatever the environment says. Elsewhere only in a quiet run, which [`arm`] is told about by the app, once, at its start: a normal launch of a test build reads none of this, and neither does `recto`. Unarmed, each seam is one load of a `OnceLock`, and one of an atomic for a test's [`gate`].
//!
//! **The switch.** `RECTO_QA_SLOW` is a list, read left to right, with commas between:
//! - a number of milliseconds, for every seam;
//! - `ci`, the profile of a machine like CI's on a bad day: see [`Seam::ci_ms`];
//! - `<seam>=<ms>`, for one seam, by [`Seam::name`];
//! - `jitter=<seed>`: each delay is drawn from half to one and a half times its own, from that seed, so a run can be repeated exactly.
//!
//! **A hold.** `RECTO_QA_SLOW_HOLD=<seam>:<path>` holds that seam until the file at the path is there, not for a time, so a test that has to act while the app waits there can't run out of time on a slow machine (CI, 2026-11-12: a 10 s wait at the launch was over before the test acted). `<seam>:window` holds it until the app says its window is up ([`window_up`]), for what a test can't look at while it's held: the state, as the socket answers only once that's in, held until the window has gone up without it however fast its patience runs out (CI, 2026-11-13: a state 1.05 s late against a 1 s patience came in first on a starved runner). A seam's delay, if it has one, comes after its hold. A hang is all its bound of [`HOLD_LIMIT`] is for.
//!
//! `RECTO_QA_RESTORE_DELAY_MS` and `RECTO_QA_REMAKE_DELAY_MS`, the first two seams' own switches from before this one, still work, and win over the list for their seam. What can't be read refuses the launch, so a slow run can't quietly be a fast one.

use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

pub mod gate;

/// Compiled as a release by `app/release-zip.sh`. A release build is never slow, whatever's in its environment.
pub const RELEASE: bool = crate::RELEASE_BUILD;
/// The switch: see the module's notes.
pub const SWITCH: &str = "RECTO_QA_SLOW";
/// A seam held until a file is there: `<seam>:<path>`.
pub const HOLD_SWITCH: &str = "RECTO_QA_SLOW_HOLD";
/// The most a hold waits, for a test that never lets go: past any test's own time, short of a run's.
pub const HOLD_LIMIT: Duration = Duration::from_secs(120);
/// The state read's own switch, from before [`SWITCH`].
pub const STATE_SWITCH: &str = "RECTO_QA_RESTORE_DELAY_MS";
/// The window remade from a late state, its own switch from before [`SWITCH`].
pub const REMAKE_SWITCH: &str = "RECTO_QA_REMAKE_DELAY_MS";

/// One waiting point.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Seam {
    /// The saved state, read on its own thread before the window can be made from it.
    State,
    /// The window that went up without the state, made again from it once it's come.
    Remake,
    /// A renderer worker's answer to a job, before the caller has it.
    Worker,
    /// The headings helper's generated contents, before the session hears of them.
    Headings,
    /// The search indexer's finished build, before the session hears of it.
    Index,
    /// The control socket: bound at the start, and answering only this much later.
    Socket,
    /// The page view's word to the host that there are layers to draw: tiles, pages and their pictures.
    Draw,
    /// A tab's sleep file, read as it wakes.
    SleepRead,
    /// A tab's sleep file, written as it goes dormant or the app goes.
    SleepWrite,
    /// The session thread, once the state's been taken in and the host told: late for what it does next, as a starved machine has it while the UI thread makes the window.
    Session,
    /// The launch, looked at on the restore thread before the state's read: what this copy is, its bundle and version, known late, as on a starved machine where the thread starts late.
    Launch,
    /// A writer of the record with what it writes gathered and the record about to be written (the saver's pass and its flush): the moment between deciding
    /// what's written and writing it, which a test holds to put something between them, and a busy disk makes late.
    RecordWrite,
    /// A late try at the record's lock, made on its own thread and not yet told to the session thread: the moment between the lock taken and the app deciding
    /// whether to keep it, which a test holds to put a quit or the person's change between them, and a sleeping disk makes late.
    RecordLock,
    /// The updater told of the first frame, on the thread that tells it, before it looks at whether this version has come up: where a test ends the app under
    /// it, and a starved machine starts that thread late.
    FirstFrame,
    /// The session thread woken for one of its deadlines that's due now (the record's write, a tab's dormancy, a try at the record's lock, the launch's
    /// patience), before it acts on it: late, as a starved machine schedules the thread late after its timer; and held by a test, so "due now and not yet
    /// done" can be looked at every time.
    Due,
}

impl Seam {
    pub const ALL: [Self; 15] = [
        Self::State,
        Self::Remake,
        Self::Worker,
        Self::Headings,
        Self::Index,
        Self::Socket,
        Self::Draw,
        Self::SleepRead,
        Self::SleepWrite,
        Self::Session,
        Self::Launch,
        Self::RecordWrite,
        Self::RecordLock,
        Self::FirstFrame,
        Self::Due,
    ];

    /// Its name in the switch and the report.
    pub const fn name(self) -> &'static str {
        match self {
            Self::State => "state",
            Self::Remake => "remake",
            Self::Worker => "worker",
            Self::Headings => "headings",
            Self::Index => "index",
            Self::Socket => "socket",
            Self::Draw => "draw",
            Self::SleepRead => "sleep_read",
            Self::SleepWrite => "sleep_write",
            Self::Session => "session",
            Self::Launch => "launch",
            Self::RecordWrite => "record_write",
            Self::RecordLock => "record_lock",
            Self::FirstFrame => "first_frame",
            Self::Due => "due",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|seam| seam.name() == name)
    }

    const fn index(self) -> usize {
        self as usize
    }

    /// **The `ci` profile:** about how late each comes on a machine like CI's under load, or past it, as each was seen on 2026-11-11 or is the deadline it has to beat. The state past the window's second of patience (1.5 s, as the late-state stage had it: 1.2 s here and the launch's 300 ms before it on the same thread), the window remade 300 ms after it, the session thread 300 ms late after it, the launch looked at 300 ms late, a worker's answer at 60 ms where a tile here takes a few, the headings and the index past what a test's first look would give them, the socket's first answer past a client's first try, a sleep file read and written as by a disk that's busy, and the updater told of the first frame a little late.
    pub const fn ci_ms(self) -> u64 {
        match self {
            Self::State => 1200,
            Self::Remake => 300,
            Self::Worker => 60,
            Self::Headings => 1500,
            Self::Index => 800,
            Self::Socket => 700,
            Self::Draw => 40,
            Self::SleepRead => 150,
            Self::SleepWrite => 300,
            Self::Session => 300,
            Self::Launch => 300,
            Self::RecordWrite => 50,
            Self::RecordLock => 50,
            // The thread that tells the updater of the first frame started late: the installer's patience for it is minutes, so a little is enough.
            Self::FirstFrame => 100,
            // A thread woken by its timer and scheduled late: a little, as a deadline comes due a few times a minute at most.
            Self::Due => 20,
        }
    }

    /// The most it may be made to wait, whatever's asked, so a typo can't hold a test for an hour.
    const fn cap(self) -> Duration {
        match self {
            Self::State => Duration::from_secs(30),
            _ => Duration::from_secs(10),
        }
    }
}

/// How late each seam comes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    ms: [u64; Seam::ALL.len()],
    jitter: Option<u64>,
}

impl Profile {
    /// From the switch and the two older ones, as the environment has them. `None` with nothing slow; an error that says what couldn't be read.
    pub fn read(switch: Option<&str>, state: Option<&str>, remake: Option<&str>) -> Result<Option<Self>, String> {
        let mut profile = Self::default();
        for item in switch.unwrap_or("").split(',').map(str::trim).filter(|item| !item.is_empty()) {
            let ms = |value: &str| value.parse::<u64>().map_err(|_| format!("{SWITCH}: `{value}` isn't a number of milliseconds."));
            match item.split_once('=') {
                None if item == "ci" => profile.ms = Seam::ALL.map(Seam::ci_ms),
                None => profile.ms = [ms(item)?; Seam::ALL.len()],
                Some(("jitter", seed)) => profile.jitter = Some(seed.parse().map_err(|_| format!("{SWITCH}: jitter's seed `{seed}` isn't a number."))?),
                Some((name, value)) => {
                    let seam = Seam::from_name(name).ok_or_else(|| format!("{SWITCH}: `{name}` isn't a seam. They're {}.", names()))?;
                    profile.ms[seam.index()] = ms(value)?;
                }
            }
        }
        for (seam, value, switch) in [(Seam::State, state, STATE_SWITCH), (Seam::Remake, remake, REMAKE_SWITCH)] {
            if let Some(value) = value {
                profile.ms[seam.index()] = value.parse().map_err(|_| format!("{switch}: `{value}` isn't a number of milliseconds."))?;
            }
        }
        Ok(profile.ms.iter().any(|ms| *ms > 0).then_some(profile))
    }

    /// **The switch that gives this profile again:** each slow seam's delay by name, and the jitter's seed, as [`Profile::read`] reads them. What a failure prints to be run again as it ran (PLAN R0.7b).
    pub fn switch(&self) -> String {
        let mut items: Vec<String> = Seam::ALL
            .iter()
            .filter(|seam| self.ms(**seam) > 0)
            .map(|seam| format!("{}={}", seam.name(), self.ms(*seam)))
            .collect();
        if let Some(seed) = self.jitter {
            items.push(format!("jitter={seed}"));
        }
        items.join(",")
    }

    /// The jitter's seed, if there is one.
    pub fn seed(&self) -> Option<u64> {
        self.jitter
    }

    /// Its own delay for a seam, before any jitter.
    pub fn ms(&self, seam: Seam) -> u64 {
        self.ms[seam.index()]
    }

    /// The `n`th wait at a seam: its delay, drawn about it if there's jitter, capped. `None` for a seam that isn't slow.
    pub fn delay(&self, seam: Seam, n: u64) -> Option<Duration> {
        let ms = self.ms(seam);
        if ms == 0 {
            return None;
        }
        let ms = match self.jitter {
            // Half to one and a half times, from the seed, the seam and the count: the same run gives the same delays.
            Some(seed) => ms / 2 + mix(seed ^ ((seam.index() as u64) << 56) ^ n) % (ms + 1),
            None => ms,
        };
        Some(Duration::from_millis(ms).min(seam.cap()))
    }
}

/// A seam held until a file's there, or the window's up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hold {
    pub seam: Seam,
    pub until: Until,
}

/// What a hold waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Until {
    /// A file at this path, made by the test.
    File(std::path::PathBuf),
    /// The app's word that its window is up ([`window_up`]): `window` in the switch.
    Window,
}

/// A hold's word for [`Until::Window`].
pub const UNTIL_WINDOW: &str = "window";

/// Whether the app has said its window is up.
static WINDOW_UP: AtomicBool = AtomicBool::new(false);

/// **The app's window is up,** with the state or without it: a hold [`Until::Window`] lets go. One relaxed store of an atomic, from the UI thread or anywhere, whether or not anything's slow.
pub fn window_up() {
    WINDOW_UP.store(true, Ordering::Relaxed);
}

impl Hold {
    /// From [`HOLD_SWITCH`]'s value, `<seam>:<path>` or `<seam>:window`: `None` with nothing asked, and an error that says what couldn't be read.
    pub fn read(switch: Option<&str>) -> Result<Option<Self>, String> {
        let Some(value) = switch.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let (name, path) = value
            .split_once(':')
            .ok_or_else(|| format!("{HOLD_SWITCH}: `{value}` isn't `<seam>:<path>` or `<seam>:{UNTIL_WINDOW}`."))?;
        let seam = Seam::from_name(name).ok_or_else(|| format!("{HOLD_SWITCH}: `{name}` isn't a seam. They're {}.", names()))?;
        if path == UNTIL_WINDOW {
            return Ok(Some(Self { seam, until: Until::Window }));
        }
        let file = std::path::PathBuf::from(path);
        if !file.is_absolute() {
            return Err(format!("{HOLD_SWITCH}: `{path}` isn't a whole path or `{UNTIL_WINDOW}`."));
        }
        Ok(Some(Self {
            seam,
            until: Until::File(file),
        }))
    }

    /// Whether what it waits for has come.
    fn come(&self) -> bool {
        match &self.until {
            Until::File(file) => file.exists(),
            Until::Window => WINDOW_UP.load(Ordering::Relaxed),
        }
    }

    /// Waits until its file's there or the window's up, or the limit. How long it waited.
    fn wait(&self) -> Duration {
        let began = Instant::now();
        while !self.come() && began.elapsed() < HOLD_LIMIT {
            std::thread::sleep(Duration::from_millis(20)); // ratchet: poll
        }
        began.elapsed()
    }
}

fn names() -> String {
    Seam::ALL.map(Seam::name).join(", ")
}

/// SplitMix64: a fixed, good spread from a counter.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// What one seam's done so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeamReport {
    pub seam: &'static str,
    pub ms: u64,
    pub times: u64,
    pub waited_ms: u64,
}

/// What the slow mode's done, for `qa state`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub seams: Vec<SeamReport>,
    pub jitter: Option<u64>,
    /// Pauses asked for on the UI thread, which were refused: a seam in the wrong place.
    pub ui_refusals: u64,
}

/// A profile in force, with what it's done.
struct Armed {
    profile: Profile,
    hold: Option<Hold>,
    ui: Option<ThreadId>,
    times: [AtomicU64; Seam::ALL.len()],
    waited_ms: [AtomicU64; Seam::ALL.len()],
    ui_refusals: AtomicU64,
}

impl Armed {
    fn new(profile: Profile, hold: Option<Hold>, ui: Option<ThreadId>) -> Self {
        Self {
            profile,
            hold,
            ui,
            times: Default::default(),
            waited_ms: Default::default(),
            ui_refusals: AtomicU64::new(0),
        }
    }

    /// The next delay at a seam, counted as waited.
    fn next(&self, seam: Seam) -> Option<Duration> {
        if self.profile.ms(seam) == 0 {
            return None;
        }
        let n = self.times[seam.index()].fetch_add(1, Ordering::Relaxed);
        let delay = self.profile.delay(seam, n)?;
        self.waited_ms[seam.index()].fetch_add(delay.as_millis() as u64, Ordering::Relaxed);
        Some(delay)
    }

    fn on_ui(&self) -> bool {
        self.ui.is_some_and(|ui| ui == std::thread::current().id())
    }

    fn report(&self) -> Report {
        Report {
            seams: Seam::ALL
                .iter()
                .map(|seam| SeamReport {
                    seam: seam.name(),
                    ms: self.profile.ms(*seam),
                    times: self.times[seam.index()].load(Ordering::Relaxed),
                    waited_ms: self.waited_ms[seam.index()].load(Ordering::Relaxed),
                })
                .collect(),
            jitter: self.profile.jitter,
            ui_refusals: self.ui_refusals.load(Ordering::Relaxed),
        }
    }
}

static ARMED: OnceLock<Option<Armed>> = OnceLock::new();

/// **Reads the switch, once, at a launch.** Only a quiet run of a build that isn't a release is ever slow. Called on the UI thread, which it remembers, so a [`pause`] there is refused. A second call changes nothing. An error is the switch's words, for the launch to refuse with.
pub fn arm(quiet: bool) -> Result<(), String> {
    if RELEASE || !quiet || ARMED.get().is_some() {
        return Ok(());
    }
    let var = |name| std::env::var(name).ok();
    let profile = Profile::read(var(SWITCH).as_deref(), var(STATE_SWITCH).as_deref(), var(REMAKE_SWITCH).as_deref())?;
    let hold = Hold::read(var(HOLD_SWITCH).as_deref())?;
    let armed = (profile.is_some() || hold.is_some()).then(|| Armed::new(profile.unwrap_or_default(), hold, Some(std::thread::current().id())));
    let _ = ARMED.set(armed);
    Ok(())
}

fn armed() -> Option<&'static Armed> {
    ARMED.get()?.as_ref()
}

/// Whether this process is slow, for what decides by it.
pub fn is_on() -> bool {
    armed().is_some() || gate::any()
}

/// The next delay at a seam, for a caller that waits in its own way. `None` unless armed and that seam's slow.
pub fn delay(seam: Seam) -> Option<Duration> {
    armed()?.next(seam)
}

/// **Waits here, at a seam,** on the background thread that's about to hand on what it waited for. Refused on the UI thread.
pub fn pause(seam: Seam) {
    if gate::pause(seam) {
        return;
    }
    let Some(armed) = armed() else { return };
    if armed.on_ui() {
        armed.ui_refusals.fetch_add(1, Ordering::Relaxed);
        return;
    }
    if let Some(hold) = armed.hold.as_ref().filter(|hold| hold.seam == seam) {
        let waited = hold.wait();
        armed.times[seam.index()].fetch_add(1, Ordering::Relaxed);
        armed.waited_ms[seam.index()].fetch_add(waited.as_millis() as u64, Ordering::Relaxed);
    }
    if let Some(delay) = armed.next(seam) {
        std::thread::sleep(delay);
    }
}

/// **Runs `call` once the seam's delay is up,** on this module's one thread, so the call arrives late and nobody waits for it: a call to the host, from wherever. At once, here, when the seam isn't slow. Calls at the same seam arrive in the order they were made, as their delays don't shrink unless there's jitter.
pub fn later(seam: Seam, call: impl FnOnce() + Send + 'static) {
    let call: Call = match gate::later(seam, Box::new(call)) {
        Ok(()) => return,
        Err(call) => call,
    };
    match delay(seam) {
        Some(delay) => deliveries().push(Instant::now() + delay, call),
        None => call(),
    }
}

/// What the slow mode's done, if it's on.
pub fn report() -> Option<Report> {
    armed().map(Armed::report)
}

/// What a quiet run's relaunch by an update carries over, so the new version is as slow.
pub fn relaunch_env() -> Vec<(String, String)> {
    if RELEASE || !is_on() {
        return Vec::new();
    }
    [SWITCH, STATE_SWITCH, REMAKE_SWITCH, HOLD_SWITCH]
        .into_iter()
        .filter_map(|name| Some((name.to_owned(), std::env::var(name).ok()?)))
        .collect()
}

type Call = Box<dyn FnOnce() + Send>;

/// A call that's due, soonest first, then in the order made.
struct Due {
    at: Instant,
    order: u64,
    call: Call,
}

impl PartialEq for Due {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.order) == (other.at, other.order)
    }
}
impl Eq for Due {}
impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Due {
    // A max-heap, so the soonest is the greatest.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (other.at, other.order).cmp(&(self.at, self.order))
    }
}

/// Calls waiting to arrive, and the one thread that makes them. It's there only while there are some, and goes [`DELIVERY_IDLE`] after the last, so a test counting threads once things are quiet sees none of it.
#[derive(Default)]
struct Deliveries {
    due: Mutex<(BinaryHeap<Due>, u64)>,
    ready: Condvar,
    running: AtomicBool,
}

const DELIVERY_IDLE: Duration = Duration::from_millis(500);

/// **The delivery thread's own end, however it comes** (review 1): a call that panics takes the thread with it, and with `running` left set no other was ever started, so nothing late arrived again. Ended by a panic, it says it's gone, and starts another for what's still waiting.
struct Running(&'static Deliveries);

impl Drop for Running {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let deliveries = self.0;
        let waiting = !deliveries.due.lock().unwrap_or_else(PoisonError::into_inner).0.is_empty();
        if waiting {
            deliveries.start();
        } else {
            deliveries.running.store(false, Ordering::SeqCst);
        }
    }
}

fn deliveries() -> &'static Deliveries {
    static DELIVERIES: OnceLock<Deliveries> = OnceLock::new();
    DELIVERIES.get_or_init(Deliveries::default)
}

impl Deliveries {
    fn push(&'static self, at: Instant, call: Call) {
        {
            let mut due = self.due.lock().unwrap_or_else(PoisonError::into_inner);
            let order = due.1;
            due.1 += 1;
            due.0.push(Due { at, order, call });
        }
        self.ready.notify_one();
        if !self.running.swap(true, Ordering::SeqCst) {
            self.start();
        }
    }

    /// The thread, with `running` already set for it.
    fn start(&'static self) {
        let spawned = std::thread::Builder::new().name("recto-qa-slow".into()).stack_size(256 * 1024).spawn(move || {
            let _guard = Running(self);
            self.deliver();
        });
        if spawned.is_err() {
            self.running.store(false, Ordering::SeqCst);
            // No thread: everything waiting goes now, rather than never.
            let waiting = std::mem::take(&mut self.due.lock().unwrap_or_else(PoisonError::into_inner).0);
            for due in waiting.into_sorted_vec().into_iter().rev() {
                (due.call)();
            }
        }
    }

    fn deliver(&self) {
        let mut due = self.due.lock().unwrap_or_else(PoisonError::into_inner);
        let mut idle_since = Instant::now();
        loop {
            let now = Instant::now();
            match due.0.peek().map(|next| next.at) {
                Some(at) if at <= now => {
                    let next = due.0.pop().map(|next| next.call);
                    drop(due);
                    if let Some(call) = next {
                        call();
                    }
                    due = self.due.lock().unwrap_or_else(PoisonError::into_inner);
                    idle_since = Instant::now();
                }
                Some(at) => due = self.ready.wait_timeout(due, at - now).unwrap_or_else(PoisonError::into_inner).0,
                None if now.duration_since(idle_since) >= DELIVERY_IDLE => {
                    // Under the lock, so a push either sees this thread running or starts another.
                    self.running.store(false, Ordering::SeqCst);
                    return;
                }
                None => due = self.ready.wait_timeout(due, DELIVERY_IDLE).unwrap_or_else(PoisonError::into_inner).0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc::channel;

    /// Failure: the switch reads something other than what it says, and a "slow" run is a fast one, or slow where it wasn't asked.
    #[test]
    fn the_switch_says_how_late_each_seam_is() {
        assert_eq!(Profile::read(None, None, None), Ok(None), "nothing asked is nothing slow");
        assert_eq!(Profile::read(Some(""), None, None), Ok(None));
        assert_eq!(Profile::read(Some("0"), None, None), Ok(None), "all at nothing is nothing slow");
        let all = Profile::read(Some("250"), None, None).unwrap().unwrap();
        assert!(Seam::ALL.iter().all(|seam| all.ms(*seam) == 250));
        let ci = Profile::read(Some("ci"), None, None).unwrap().unwrap();
        assert!(
            Seam::ALL.iter().all(|seam| ci.ms(*seam) == seam.ci_ms() && ci.ms(*seam) > 0),
            "the ci profile slows every seam"
        );
        let mixed = Profile::read(Some("ci, worker=5,draw=0"), None, None).unwrap().unwrap();
        assert_eq!(
            (mixed.ms(Seam::Worker), mixed.ms(Seam::Draw), mixed.ms(Seam::State)),
            (5, 0, Seam::State.ci_ms()),
            "read left to right"
        );
        assert_eq!(mixed.delay(Seam::Draw, 0), None, "a seam at nothing doesn't wait");
    }

    /// Failure: a hold reads something other than what it says, so a test that has to act while the app waits at a seam acts after it's gone on, or a typo holds the wrong seam or none. And a hold doesn't let go once its file is there, or once the window's up (so a state held until then is never read), or lets go before.
    #[test]
    fn a_hold_says_which_seam_and_lets_go_when_what_it_waits_for_comes() {
        assert_eq!(Hold::read(None), Ok(None));
        assert_eq!(Hold::read(Some(" ")), Ok(None));
        assert_eq!(
            Hold::read(Some("launch:/t/go")),
            Ok(Some(Hold {
                seam: Seam::Launch,
                until: Until::File("/t/go".into())
            }))
        );
        assert_eq!(
            Hold::read(Some("state:window")),
            Ok(Some(Hold {
                seam: Seam::State,
                until: Until::Window
            }))
        );
        for bad in ["launch", "lunch:/t/go", "launch:go", "launch:windows"] {
            assert!(Hold::read(Some(bad)).is_err(), "{bad}");
        }
        let dir = std::env::temp_dir().join(format!("recto-hold-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let go = dir.join("go");
        let hold = Hold {
            seam: Seam::Launch,
            until: Until::File(go.clone()),
        };
        let (done, waited) = channel();
        std::thread::spawn(move || done.send(hold.wait()).unwrap());
        assert!(waited.recv_timeout(Duration::from_millis(200)).is_err(), "held while its file isn't there");
        std::fs::write(&go, "").unwrap();
        assert!(waited.recv_timeout(Duration::from_secs(10)).is_ok(), "let go once it is");
        let _ = std::fs::remove_dir_all(&dir);
        let hold = Hold {
            seam: Seam::State,
            until: Until::Window,
        };
        let (done, waited) = channel();
        std::thread::spawn(move || done.send(hold.wait()).unwrap());
        assert!(waited.recv_timeout(Duration::from_millis(200)).is_err(), "held while the window isn't up");
        window_up();
        assert!(waited.recv_timeout(Duration::from_secs(10)).is_ok(), "let go once it is");
    }

    /// Failure: the two older switches stop working, and the restore tests that use them run fast without saying so.
    #[test]
    fn the_older_switches_still_work_and_win_for_their_seam() {
        let state = Profile::read(None, Some("1500"), None).unwrap().unwrap();
        assert_eq!(state.ms(Seam::State), 1500);
        assert!(
            Seam::ALL.iter().filter(|seam| **seam != Seam::State).all(|seam| state.ms(*seam) == 0),
            "only the state's slow"
        );
        let both = Profile::read(Some("ci"), Some("2500"), Some("1000")).unwrap().unwrap();
        assert_eq!(
            (both.ms(Seam::State), both.ms(Seam::Remake), both.ms(Seam::Worker)),
            (2500, 1000, Seam::Worker.ci_ms())
        );
    }

    /// Failure: a switch with a typo in it is taken as nothing, and a run meant to be slow passes as a fast one.
    #[test]
    fn a_switch_that_cant_be_read_says_so() {
        for bad in ["fast", "wrker=5", "worker=soon", "jitter=x", "-1"] {
            let error = Profile::read(Some(bad), None, None).unwrap_err();
            assert!(error.contains(SWITCH), "{bad}: {error}");
        }
        assert!(
            Profile::read(Some("wrker=5"), None, None).unwrap_err().contains("session"),
            "it lists the seams"
        );
        assert!(Profile::read(None, Some("soon"), None).unwrap_err().contains(STATE_SWITCH));
    }

    /// Failure: jitter isn't repeatable, so a failure it found can't be had again, or it strays past the bounds it promises, or past a seam's cap.
    #[test]
    fn jitter_is_repeatable_and_bounded() {
        let profile = Profile::read(Some("200,jitter=7"), None, None).unwrap().unwrap();
        let again = Profile::read(Some("200,jitter=7"), None, None).unwrap().unwrap();
        let other = Profile::read(Some("200,jitter=8"), None, None).unwrap().unwrap();
        let draws: Vec<_> = (0..64).map(|n| profile.delay(Seam::Worker, n).unwrap()).collect();
        assert_eq!(
            draws,
            (0..64).map(|n| again.delay(Seam::Worker, n).unwrap()).collect::<Vec<_>>(),
            "the same seed, the same delays"
        );
        assert_ne!(
            draws,
            (0..64).map(|n| other.delay(Seam::Worker, n).unwrap()).collect::<Vec<_>>(),
            "another seed, others"
        );
        assert!(draws.iter().all(|delay| (100..=300).contains(&delay.as_millis())), "{draws:?}");
        assert!(draws.windows(2).any(|pair| pair[0] != pair[1]), "it does vary");
        let huge = Profile::read(Some("99999999"), None, None).unwrap().unwrap();
        assert_eq!(huge.delay(Seam::State, 0), Some(Duration::from_secs(30)));
        assert_eq!(huge.delay(Seam::Worker, 0), Some(Duration::from_secs(10)));
    }

    /// Failure: a failure's rerun line (PLAN R0.7b) doesn't give back the run it came from: the switch a profile writes reads as another profile, or with another seed, so the same arrivals in the same order can't be had again.
    #[test]
    fn a_profile_written_back_as_a_switch_gives_the_same_delays() {
        for asked in ["ci", "ci,jitter=12345", "worker=7,state=0,draw=3,jitter=0", "250"] {
            let profile = Profile::read(Some(asked), None, None).unwrap().unwrap();
            let again = Profile::read(Some(&profile.switch()), None, None).unwrap().unwrap();
            assert_eq!(again, profile, "{asked} as {}", profile.switch());
            for seam in Seam::ALL {
                let draws = |p: &Profile| (0..16).map(|n| p.delay(seam, n)).collect::<Vec<_>>();
                assert_eq!(draws(&again), draws(&profile), "{asked}: {}", seam.name());
            }
        }
        let seeded = Profile::read(Some("ci,jitter=99"), None, None).unwrap().unwrap();
        assert_eq!(seeded.seed(), Some(99));
        assert!(seeded.switch().ends_with(",jitter=99"), "{}", seeded.switch());
        assert_eq!(Profile::read(Some("ci"), None, None).unwrap().unwrap().seed(), None);
    }

    /// Failure: a seam's pause put on the UI thread hangs the app for its delay (AGENTS rule 3), where it should be refused and counted.
    #[test]
    fn a_pause_on_the_ui_thread_is_refused_and_counted() {
        let armed = Armed::new(
            Profile::read(Some("5000"), None, None).unwrap().unwrap(),
            None,
            Some(std::thread::current().id()),
        );
        assert!(armed.on_ui());
        let elsewhere = Arc::new(Armed::new(armed.profile.clone(), None, armed.ui));
        let other = Arc::clone(&elsewhere);
        assert!(!std::thread::spawn(move || other.on_ui()).join().unwrap(), "another thread isn't the UI's");
        assert_eq!(armed.next(Seam::Worker), Some(Duration::from_secs(5)));
        assert_eq!(armed.report().seams[Seam::Worker.index()].times, 1);
        assert_eq!(armed.report().seams[Seam::Worker.index()].waited_ms, 5000);
    }

    /// Failure: a late call to the host is lost, made twice, or made out of order, so a draw the page view asked for never comes, or an older one comes after a newer one.
    #[test]
    fn late_calls_arrive_once_and_in_order() {
        let (send, receive) = channel();
        let start = Instant::now();
        for n in 0..20u32 {
            let send = send.clone();
            deliveries().push(start + Duration::from_millis(30), Box::new(move || send.send(n).unwrap()));
        }
        let got: Vec<u32> = (0..20).map(|_| receive.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        assert_eq!(got, (0..20).collect::<Vec<_>>());
        assert!(start.elapsed() >= Duration::from_millis(30), "not before they're due");
        assert!(receive.recv_timeout(Duration::from_millis(50)).is_err(), "once each");
    }

    /// Failure (review 1): a late call that panics takes the delivery thread with it, `running` stays set, and no late call arrives again, as no other thread is started. The ones after it arrive, and so does one made later. And the delivery thread goes once nothing's waiting, so a quiet slow run doesn't carry a thread a fast one doesn't, and a test that counts threads at rest fails only in slow mode; and comes again for the next.
    #[test]
    fn a_call_that_panics_doesnt_stop_the_ones_after_it() {
        let (send, receive) = channel();
        let start = Instant::now();
        deliveries().push(start + Duration::from_millis(10), Box::new(|| panic!("a call that panics, on purpose")));
        let after = send.clone();
        deliveries().push(start + Duration::from_millis(20), Box::new(move || after.send(1).unwrap()));
        assert_eq!(receive.recv_timeout(Duration::from_secs(10)), Ok(1), "the one after it");
        let deadline = Instant::now() + Duration::from_secs(10);
        while deliveries().running.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "the thread never went");
            std::thread::sleep(Duration::from_millis(10));
        }
        deliveries().push(Instant::now(), Box::new(move || send.send(2).unwrap()));
        assert_eq!(receive.recv_timeout(Duration::from_secs(10)), Ok(2), "and one made later");
    }

    /// Failure: something in a process that isn't a quiet run, as `recto` or a normal launch of a test build, is made slow.
    #[test]
    fn nothing_is_slow_unless_armed() {
        assert_eq!(arm(false), Ok(()));
        assert!(!is_on());
        assert_eq!(delay(Seam::Worker), None);
        let start = Instant::now();
        pause(Seam::State);
        let (send, receive) = channel();
        later(Seam::Draw, move || send.send(()).unwrap());
        assert!(receive.try_recv().is_ok(), "unarmed, a late call is made at once, here");
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(report(), None);
        assert!(relaunch_env().is_empty());
    }
}
