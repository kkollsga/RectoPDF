//! **Tabs going dormant and waking, SPEC decision 25.** Owner, 2026-11-07: "It's fine to keep in memory for a while, and then as it goes dormant we store to disk." And: "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way that's quick to recover without any delay."
//!
//! **One deadline, and no thread.** A tab left alone for [`DORMANT_AFTER_SECONDS`] goes dormant. There's nothing per tab: the session thread's one wait has this as well as the state's deadline, and it's the soonest of the few tabs that hold anything. Coming due, it does what's asked and works out the next. At rest with nothing warm there's no deadline at all.
//!
//! **Going dormant, once:** the tab's sleep file's written if there's anything new to say, its journal's compacted and read back, and its contents, list, worker and tiles go. A byte budget across the documents off screen and memory pressure make that happen sooner, oldest first. A tab being worked on by an agent or saved doesn't go, whatever's asking: `Session::hold` and `saving` see to that.

use super::keeper::{Keeper, KeepsRecord};
use super::saver::Saver;
use super::{CoreCall, Inner, lock};
use crate::clock::Clock;
use crate::panes::Panes;
use crate::queue::CoreQueue;
use recto_agent::DrawableLoader;
use recto_control::host::SessionHost;
use recto_control::record_store::RecordStore;
use recto_platform::surface::PixelSurface;
use recto_reader::Engine;
use recto_reader::layout::PageBox;
use recto_reader::paper::PaperKind;
use recto_reader::{SleepMaterial, SleepShot, SleepView};
use recto_session::record::{FileStatus, look_at_file};
use recto_session::sleep::{SleepFile, SleepPicture};
use recto_session::{SleepFacts, TabId};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::sleep_queue::Leaves;
use crate::sleep_writer::{By, Event, Out, Outcome, SleepWriter, Write};
use crate::sleeper as app;
pub use crate::sleeper::{Asked, Why};
pub use crate::sleeper::{DORMANT_AFTER_SECONDS, LIVE_BUDGET_BYTES};
/// How often a flush looks again for the sleep files' lock, while it may wait for it.
const WRITING_LOOK_EVERY: Duration = Duration::from_millis(10);

pub struct Sleeper {
    /// What the machine's decisions go by: when dormancy is due, the write gap and the hourly cap, the budget's look. The host's clock, a test's fake one. The
    /// waits for the sleep files' lock (a flush's `until` and the writer's hold-back) are real time and go by `Instant`, as they wait on another thread's disk.
    /// Which tabs are due is the session's own clock (`Session::seconds_now`), as it always was: see [`crate::sleep_writer`].
    clock: Arc<dyn Clock>,
    /// When the soonest is due, in milliseconds of `clock` plus one, and zero for nothing: the machine's [`Out::Due`], kept where the session thread's wait reads
    /// it with the queue's lock held, as an atomic.
    due: AtomicU64,
    /// **The sleep writer's state, as one machine** ([`SleepWriter`]). Held only inside [`Self::step`] and the queries beside it: never across carrying out an
    /// `Out`, a session call or the disk. Taken inside [`Self::writing`] (a write's decision and its count), never the other way round.
    machine: Mutex<SleepWriter>,
    /// One sleep file being written at a time, so a flush has what's on its way.
    writing: Mutex<()>,
    /// Sleep files written off the session's time, waiting for their one writer, and what waits on them: [`crate::sleep_queue::Leaving`].
    leaving: crate::sleep_queue::Leaving,
    /// Looks at files under way, each on a thread of its own: one per tab at most, and [`MAX_LOOKS`] in all.
    looks: std::sync::Arc<LookSlots>,
    /// **The app's gone to the background and what waits in the cool-down is to be written now:** set on the UI thread, which takes no lock and says no word
    /// ([`Self::hurry_leaving_soon`]), and taken by the saver's pass that going to the background asks for, off the UI thread, which hurries the queue under its
    /// lock and starts a writer if there's none ([`Self::hurry_leaving_if_asked`]).
    background_hurry: std::sync::atomic::AtomicBool,
    /// **What came of sleep files agents asked for, each kept under its asker's ticket** till the asker takes it ([`Self::take_asked_words`]): only writes cut
    /// short by agents' share, the one thing an asker is told of, and at most [`MAX_ASKED_ANSWERS`], the oldest going first (an asker that gave up waiting).
    answers: Mutex<Vec<(u64, Outcome)>>,
    /// The last ticket handed out.
    tickets: AtomicU64,
}

/// Answers kept for agents that asked for a sleep file and haven't taken them yet. Each asker takes its own at once, so a few cover agents asking together.
const MAX_ASKED_ANSWERS: usize = 8;

/// Looks at files that can be under way at once. A look is a stat, microseconds, unless the file's on a volume that's stopped answering, where it may never end: then this many threads are all that's ever held for it, however many times the app comes to the front.
pub const MAX_LOOKS: usize = 8;

/// **Which tabs have a look at their file under way.** A tab already being looked at doesn't get a second thread, and past [`MAX_LOOKS`] no more start, so coming back to the app over and over with a hung disk adds no threads (review O3). A look asked for while one's under way isn't lost: it's noted, and when the look under way ends it's asked for again, as the one under way may have looked before the file changed.
#[derive(Debug, Default)]
pub struct LookSlots {
    /// Each tab being looked at, and whether another look was asked for meanwhile.
    running: Mutex<Vec<(TabId, bool)>>,
}

impl LookSlots {
    /// Takes a slot for a look at this tab's file, if it hasn't one and one's free. If it has one, another look is noted for when that ends.
    pub fn start(&self, id: TabId) -> bool {
        let mut running = lock(&self.running);
        if let Some(entry) = running.iter_mut().find(|(tab, _)| *tab == id) {
            entry.1 = true;
            return false;
        }
        if running.len() >= MAX_LOOKS {
            return false;
        }
        running.push((id, false));
        true
    }

    /// **The look's done, and what it found is queued:** if another look was asked for meanwhile, the files showing are looked at again, as the one under way
    /// may have looked before the file changed. Asked for with the slot still held, so the look counts as under way ([`Self::running`]) until everything it
    /// comes to is queued for the session thread, which is what the app's settled goes by ([`crate::settled`]): it was let go first, and a look counted done
    /// with its word not yet queued could be read as at rest.
    pub fn finish_and_look_again(&self, id: TabId, core: &CoreQueue) {
        let mut running = lock(&self.running);
        if running.iter().any(|(tab, again)| *tab == id && *again) {
            core.push(CoreCall::LookAtShown);
        }
        running.retain(|(tab, _)| *tab != id);
    }

    /// The look's done, however it ended: whether another was asked for meanwhile, which the caller asks for again.
    pub fn finish(&self, id: TabId) -> bool {
        let mut running = lock(&self.running);
        let again = running.iter().any(|(tab, again)| *tab == id && *again);
        running.retain(|(tab, _)| *tab != id);
        again
    }

    pub fn running(&self) -> usize {
        lock(&self.running).len()
    }
}

impl LookSlots {
    /// **Looks at a tab's file before it's shown, for as long as a frame can wait for it.** Another file there means what's held of it is of something else, so the tab's read again, and nothing drawn of the old one is shown: the page view's told its version's changed. Past the time, which is a disk that's gone away, it's shown as it was, and the look goes on on its own thread. A stat, which is microseconds.
    pub(super) fn look_before_showing(self: &Arc<Self>, session: &SessionHost<DrawableLoader>, core: &Arc<CoreQueue>, id: TabId) {
        let Some((path, stamp, says_something)) = look_facts(session, id) else {
            return;
        };
        // One look per tab, and so many in all: with a disk that's stopped answering, showing tab after tab adds no threads past them.
        if !self.start(id) {
            return;
        }
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let core_for_look = std::sync::Arc::clone(core);
        let slots = std::sync::Arc::clone(self);
        // Its way back to the session thread is counted from here till it has said what it found, the usual way if the frame has stopped waiting: what
        // the app's settled goes by ([`crate::settled`]). Its slot goes before it answers the frame, as the next look at this tab may be asked for at once.
        core.way_back_taken();
        let looking = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
            let status = look_at_file(&path, stamp);
            // A look asked for meanwhile may be about a change made after this one looked: the files showing are looked at again.
            if slots.finish(id) {
                core_for_look.push(CoreCall::LookAtShown);
            }
            let other = look_is_news(&status, stamp, says_something);
            // Too late for the frame: it's said the usual way.
            if send.try_send((other, status.clone())).is_err() && other {
                core_for_look.push(CoreCall::WokenFileIsOther(id, status, stamp));
            }
            #[cfg(test)]
            after_answering();
            core_for_look.way_back_let_go();
        });
        if looking.is_err() {
            core.way_back_let_go();
            self.finish(id);
            return;
        }
        // Only what's new: a tab whose file is already known to be another, shown again, is neither forgotten nor read again.
        if let Ok((true, status)) = receive.recv_timeout(LOOK_PATIENCE) {
            take_shown_look(session, core, id, stamp, status);
        }
    }

    /// One thread looks at the one file a tab was woken from its sleep file on, or that's showing when the app comes to the front, and ends. If it's something else now, the session's told, and it's read after all. A tab with a look under way already, or past [`MAX_LOOKS`] in all, isn't looked at now: see [`LookSlots`].
    pub(super) fn check_woken_file(self: &Arc<Self>, session: &SessionHost<DrawableLoader>, core: &Arc<CoreQueue>, id: TabId) {
        let Some((path, stamp, says_something)) = look_facts(session, id) else {
            return;
        };
        if !self.start(id) {
            return;
        }
        let (slots, core) = (std::sync::Arc::clone(self), std::sync::Arc::clone(core));
        let spawned = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
            let status = look_at_file(&path, stamp);
            if look_is_news(&status, stamp, says_something) {
                core.push(CoreCall::WokenFileIsOther(id, status, stamp));
            }
            slots.finish_and_look_again(id, &core);
        });
        if spawned.is_err() {
            self.finish(id);
        }
    }
}

impl Sleeper {
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            due: AtomicU64::new(0),
            machine: Mutex::new(SleepWriter::default()),
            writing: Mutex::new(()),
            leaving: crate::sleep_queue::Leaving::default(),
            looks: std::sync::Arc::new(LookSlots::default()),
            background_hurry: std::sync::atomic::AtomicBool::new(false),
            answers: Mutex::new(Vec::new()),
            tickets: AtomicU64::new(0),
        }
    }

    fn millis(&self) -> u64 {
        self.clock.now_ms() + 1
    }

    /// **What happens to the sleep writer,** now by the clock. The lock's let go of before this returns: the caller carries out what it comes to. A new
    /// deadline is kept here, as it's only stored.
    fn step(&self, event: Event) -> Option<Out> {
        let now = self.millis();
        let out = lock(&self.machine).step(event, now);
        if let Some(Out::Due(due)) = out {
            self.due.store(due, Ordering::SeqCst);
            return None;
        }
        out
    }

    /// How long until something's due, for the session thread's wait, or `None`. An atomic: it's asked with the queue's lock held.
    pub fn wait_for_due(&self) -> Option<Duration> {
        app::wait_for(self.due.load(Ordering::SeqCst), self.millis()).map(Duration::from_millis)
    }

    pub fn set_after(&self, seconds: u32) {
        let _ = self.step(Event::SetAfter(seconds));
    }

    /// How long a tab that's only moving about waits for its next sleep file. None at all takes the hourly limit with it.
    pub fn set_write_gap(&self, seconds: u32) {
        let _ = self.step(Event::SetWriteGap(seconds));
    }

    /// What sleep files may come to in an hour, for a test of the cap at the cap and one past it.
    #[cfg(test)]
    pub(super) fn set_hour_cap(&self, bytes: u64) {
        let _ = self.step(Event::SetHourCap(bytes));
    }

    /// What agents' sleep files may come to of it, for a test of the share at the share and one past it.
    #[cfg(test)]
    pub(super) fn set_agent_hour_cap(&self, bytes: u64) {
        let _ = self.step(Event::SetAgentHourCap(bytes));
    }

    /// **A sleep file asked for by `by`,** with a ticket of its own to take what came of it back by ([`Self::take_asked_words`]).
    pub(super) fn asking(&self, by: recto_agent::Actor) -> app::Asked {
        app::Asked {
            by,
            ticket: self.tickets.fetch_add(1, Ordering::SeqCst) + 1,
        }
    }

    /// What came of the sleep file `asked` was for, kept for its asker if there's anything to tell: a write cut short by agents' share.
    fn answer(&self, asked: app::Asked, outcome: Option<Outcome>) {
        let Some(outcome) = outcome.filter(|outcome| asked.ticket != 0 && outcome.cut_for_share) else {
            return;
        };
        let mut answers = lock(&self.answers);
        if answers.len() >= MAX_ASKED_ANSWERS {
            answers.remove(0);
        }
        answers.push((asked.ticket, outcome));
    }

    /// **What an agent is told of the sleep file it asked for under `ticket`**, in Rust's words, from that write's own outcome: put off, or its facts written
    /// without pictures, as agents' share of the hour is used, and when it comes back; nothing if it wasn't cut short. `on_screen`: the tab on screen's, at
    /// the agent's `session save`.
    pub(super) fn take_asked_words(&self, ticket: u64, on_screen: bool) -> Option<String> {
        let outcome = {
            let mut answers = lock(&self.answers);
            let at = answers.iter().position(|(kept, _)| *kept == ticket)?;
            answers.remove(at).1
        };
        lock(&self.machine).share_words(outcome, on_screen, self.millis())
    }

    fn after_seconds(&self) -> u32 {
        lock(&self.machine).after_seconds()
    }

    /// These sleep files are about to be removed, as retired: nothing queued for them is written after ([`crate::sleep_queue::LeavingQueue::retire`]).
    pub(super) fn retire(&self, names: &[String]) {
        self.leaving.retire(names);
    }

    /// How long a pushed-out tab's sleep file waits, for tests.
    #[cfg(any(test, feature = "testkit"))]
    pub(crate) fn set_leaving_wait(&self, ms: u64) {
        self.leaving.set_wait(ms);
    }

    /// The writer looks at the queue again, as the clock's moved on (a test's clock, which no timeout follows).
    #[cfg(any(test, feature = "testkit"))]
    pub(crate) fn poke_leaving(&self) {
        self.leaving.poke();
    }

    /// **Everything waiting is written now, without waiting for the cool-down** (memory pressure, a quit, a hand-off), with a writer started for it by `start`
    /// if there's none, as after one panicked. Not the UI thread's: that's [`Self::hurry_leaving_soon`].
    fn hurry_leaving(&self, start: StartsWriter<'_>) {
        if self.leaving.hurry() {
            start.start();
        }
    }

    /// **Everything waiting is written now, and waited for until `until`** (a quit's, a hand-off's), a writer started for it by `start` if there's none: past
    /// it, or with no writer to be had, what's left is written after or not at all, which costs those tabs only their quick way back. Says whether it all went.
    fn drain_leaving(&self, until: Instant, start: StartsWriter<'_>) -> bool {
        self.hurry_leaving(start);
        self.leaving.drain(until)
    }

    /// **A hurry going to the background asked for, carried out:** what waits in the cool-down is due now, under the queue's lock, and a writer's started by
    /// `start` if there's none (as after one panicked). Called by the saver's pass and at the end of a flush, never on the UI thread.
    pub(super) fn hurry_leaving_if_asked(&self, start: StartsWriter<'_>) {
        if self.background_hurry.swap(false, Ordering::SeqCst) {
            self.hurry_leaving(start);
        }
    }

    /// The next sleep writer panics as it starts, for a test of what's left after one.
    #[cfg(test)]
    pub(super) fn panic_next_leaving_writer(&self) {
        self.leaving.panic_next_writer();
    }

    /// **The app's gone to the background:** what's waiting is to be written now, by the saver's pass the caller asks for next (`Saver::write_soon`), which
    /// carries it out off this thread ([`Self::hurry_leaving_if_asked`]). The UI thread's: one atomic store, no lock, no word to the writer.
    pub(super) fn hurry_leaving_soon(&self) {
        self.background_hurry.store(true, Ordering::SeqCst);
    }

    /// The one-at-a-time lock on writing sleep files, for a write someone's waiting on: waited for, or until `until` and no further (counted as skipped past it),
    /// and said to the machine meanwhile so the queue's writer lets it go first. A flush's thread or being asked's, never the UI's.
    /// `until` is real time, not the clock: a wait on another thread's disk.
    fn take_writing(&self, until: Option<Instant>) -> Option<std::sync::MutexGuard<'_, ()>> {
        let _ = self.step(Event::LockWanted);
        let taken = match until {
            None => Some(lock(&self.writing)),
            Some(until) => loop {
                match self.writing.try_lock() {
                    Ok(guard) => break Some(guard),
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => break Some(poisoned.into_inner()),
                    Err(std::sync::TryLockError::WouldBlock) if Instant::now() >= until => break None,
                    Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(WRITING_LOOK_EVERY),
                }
            },
        };
        let _ = self.step(Event::LockWaited { got: taken.is_some() });
        taken
    }

    /// Before the queue's writer takes the lock for its next, a write someone's waiting on goes first: a flush looks only every [`WRITING_LOOK_EVERY`], and a lock isn't fair, so the writer, going from one to the next, would otherwise have it again first, every time. Bounded, as a flush has a limit of its own.
    fn let_others_go_first(&self) {
        // Real time, not the clock: how long the writer has held back, a wait on other threads.
        let began = Instant::now();
        while lock(&self.machine).holds_back(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX)) {
            std::thread::sleep(WRITING_LOOK_EVERY);
        }
    }

    /// Looks at woken tabs' files under way now.
    pub fn looks_running(&self) -> usize {
        self.looks.running()
    }

    /// A look at a tab's file on a thread of its own, if it hasn't one under way and one's free: [`LookSlots::check_woken_file`].
    pub(super) fn check_woken_file(&self, session: &SessionHost<DrawableLoader>, core: &Arc<CoreQueue>, id: TabId) {
        self.looks.check_woken_file(session, core, id);
    }

    /// A look at a tab's file before it's shown, for as long as a frame can wait for it: [`LookSlots::look_before_showing`].
    pub(super) fn look_before_showing(&self, session: &SessionHost<DrawableLoader>, core: &Arc<CoreQueue>, id: TabId) {
        self.looks.look_before_showing(session, core, id);
    }

    pub fn set_budget(&self, bytes: u64) {
        let _ = self.step(Event::SetBudget(bytes));
    }

    pub fn report(&self) -> Value {
        let (counts, after_seconds, budget_bytes, agent_bytes) = {
            let machine = lock(&self.machine);
            (
                machine.counts(),
                machine.after_seconds(),
                machine.budget_bytes(),
                machine.agent_bytes_this_hour(),
            )
        };
        json!({
            "after_seconds": after_seconds,
            "budget_bytes": budget_bytes,
            "due_in_ms": self.wait_for_due().map(|wait| u64::try_from(wait.as_millis()).unwrap_or(u64::MAX)),
            "gone_dormant": { "left_alone": counts.left_alone, "budget": counts.budget, "pressure": counts.pressure, "asked": counts.asked },
            "sleep_files_written": counts.sleep_files,
            "writes_put_off": counts.writes_put_off,
            "writes_for_paper": counts.writes_for_paper,
            "writes_for_place": counts.writes_for_place,
            "writes_skipped_busy": counts.writes_skipped_busy,
            "agent_share_used": counts.agent_share_used,
            "agent_bytes_this_hour": agent_bytes,
            "sleep_bytes_written": counts.sleep_bytes,
            "woken_from_file": counts.woken_from_file,
            "pressure_events": counts.pressure_events,
            "system_pressure": { "warning": counts.system_warning, "critical": counts.system_critical },
            "looks_running": self.looks.running(),
            "leaving": self.leaving.report(),
        })
    }
}

impl Sleeper {
    /// Writes a tab's sleep file from what's known of it now, if that's anything new, waiting behind the one being written: as being asked has it (`until`
    /// none), or a flush (only until `until`, counted as skipped past it). Says what came of it, if anything was decided.
    pub(super) fn write_sleep_for(
        &self,
        on_hand: &OnHand<'_>,
        store: &RecordStore,
        keeps: &KeepsRecord<'_>,
        id: TabId,
        by: By,
        until: Option<Instant>,
    ) -> Option<Outcome> {
        let (facts, material) = on_hand.facts_and_material(id)?;
        // What's queued for it is older than this.
        self.leaving.forget(id);
        self.write_with_material(store, keeps, facts, material, by, until)
    }

    /// **The sleep files a flush writes** (a quit's, a hand-off's, `session save`'s), the state already written. **The tab on screen's first**, the next launch's
    /// first frame, waiting for another being written only until `until` and counted as skipped past it (a quit waited its 5 s behind sleep files queued for a
    /// slow disk and went without the state, slow mode 2026-11-11); **then what's waiting in the cool-down**, hurried and waited for within what's left of the
    /// time, a writer started for it by `start_writer` if there's none. The other way round, on a slow disk the queued files used the limit and the one file the
    /// next launch needs was skipped (review, 2026-11-13).
    /// **Asked for by an agent** (`session save`), the cool-down is left alone (what waits stays waiting) and the tab on screen's file is held to the agent's
    /// share of the hour: an agent's save in a loop cut every cool-down short on demand. What came of that write is kept under the agent's ticket, for its
    /// answer to say if its share cut it short.
    pub(super) fn sleep_at_flush(
        &self,
        on_hand: &OnHand<'_>,
        store: &RecordStore,
        keeps: &KeepsRecord<'_>,
        asked: Option<app::Asked>,
        until: Instant,
        start_writer: StartsWriter<'_>,
    ) {
        let agent = asked.filter(|asked| matches!(asked.by, recto_agent::Actor::Agent(_)));
        if let Some(id) = on_hand.session.with_session(|session| session.tabs().active()) {
            let outcome = self.write_sleep_for(on_hand, store, keeps, id, agent.map_or(By::Flush, |asked| By::Asked(asked.by)), Some(until));
            if let Some(asked) = agent {
                self.answer(asked, outcome);
            }
        }
        if agent.is_none() {
            self.drain_leaving(until, start_writer);
        }
    }

    /// **Tabs pushed out of the warm ones, their sleep files queued,** with their pictures copied just before the page view lets go of them: a quick copy of each, of those whose time's come, queued as it's made, so what's held is the queue's cap and the one being copied. Packing a photograph takes long enough to hold up whatever's asked for next, so the one writer does that ([`crate::sleep_queue`]). Each waits [`crate::sleeper::LEAVING_WAIT_MS`] first (the cool-down on the write). A flush hurries what's waiting and waits for it for what's left of its time, then writes the tab on screen's file.
    /// `shown` is the tab being shown: what's queued for it was copied as it left, so it's taken back, not written, and said for its first frame
    /// ([`give_sleep_view`]); it's queued again when it next leaves.
    /// A writer's started by `start_writer` if the queue says there's none.
    pub(super) fn queue_those_leaving(&self, on_hand: &OnHand<'_>, shown: TabId, start_writer: StartsWriter<'_>) -> Option<(SleepFacts, SleepMaterial)> {
        let back = self.leaving.take_back(shown);
        let left = on_hand.session.with_session(|session| session.sleeper().take_left_warm());
        let now = self.millis();
        let mut start = false;
        for (facts, pushed_by) in left {
            if let Some(material) = on_hand.material(facts.id) {
                start |= self.leaving.push(facts, material, Leaves::PushedOut(pushed_by), now);
            }
        }
        if start {
            start_writer.start();
        }
        back
    }

    /// A tab going dormant by itself: its sleep file, from what's on hand now, queued for the one writer, started by `start_writer` if there's none.
    pub(super) fn queue_sleep_for(&self, on_hand: &OnHand<'_>, id: TabId, start_writer: StartsWriter<'_>) {
        let Some((facts, material)) = on_hand.facts_and_material(id) else {
            return;
        };
        if self.leaving.push(facts, material, Leaves::Dormant, self.millis()) {
            start_writer.start();
        }
    }

    /// The documents off screen against their budget. At most once a second, and only with something off screen, which most of the time there's little of.
    /// Tabs it says are over go to the session thread through `core`.
    fn look_at_budget(&self, on_hand: &OnHand<'_>, core: &CoreQueue) {
        let (warm, active) = on_hand
            .session
            .with_session(|session| (session.dormant().oldest_first(), session.tabs().active()));
        // A change of tab is when a document comes to be off screen, so that's looked at straight away.
        if self
            .step(Event::LookAtBudget {
                active,
                off_screen: !warm.is_empty(),
            })
            .is_none()
        {
            return;
        }
        let costs: Vec<(TabId, u64)> = warm
            .iter()
            .map(|id| {
                (
                    *id,
                    on_hand.engine_for(*id).workers().bytes(id.0)
                        + on_hand
                            .session
                            .with_session(|session| session.lists().get(*id).map_or(0, |list| list.history_bytes() as u64)),
                )
            })
            .collect();
        if let Some(Out::SleepTabs(over, why)) = self.step(Event::Costs(costs)) {
            core.push(CoreCall::SleepTabs(over, why));
        }
    }
}

impl Sleeper {
    /// **What the sleeper does at the end of every call on the session thread:** the tabs just woken from their sleep files are looked at, the next
    /// deadline's worked out, and what's off screen is looked at against its budget, in that order. The rule it holds is that a call ends with all three, so
    /// none is left out where the session thread's tail is changed. The order is the one the tail always had, and it's kept for a reason: the looks at the
    /// woken files go first, as each starts a thread and the soonest a file found to be another is read again is best; the deadline's then worked out from
    /// everything this call touched or woke; the budget goes last, as what it decides is sent to sleep by a call of its own after this one. None of the three
    /// reads what another wrote in this call, so the order is not a correctness rule. It is pinned, with what the sleeper writes, by the golden in
    /// `headless/sleep_counts.rs`.
    pub(super) fn after_a_call(&self, on_hand: &OnHand<'_>, core: &Arc<CoreQueue>) {
        self.check_woken_files(on_hand.session, core);
        self.rearm(on_hand.session);
        self.look_at_budget(on_hand, core);
    }

    /// Works out the next deadline, after anything that can have changed it. A look at the few tabs that hold anything.
    ///
    /// **Real time:** how long until the next is due is the session's own clock (`Dormant::next_in`, as every `touch` goes by it); the
    /// deadline it comes to is on the app's clock.
    fn rearm(&self, session: &SessionHost<DrawableLoader>) {
        let after = self.after_seconds();
        let in_seconds = session.with_session(|session| session.dormant().next_in(after));
        let _ = self.step(Event::Rearm { in_seconds });
    }

    /// It's come due, on the session thread. Whatever's due goes dormant, unless the app's `busy` (a file's being read, or a hand-off has it frozen), when it
    /// looks again shortly.
    /// **Real time:** which are due is the session's, by its own clock (`seconds_now`).
    pub(super) fn dormancy_due(&self, session: &SessionHost<DrawableLoader>, busy: bool) -> Option<Vec<TabId>> {
        self.step(Event::DueNow { busy })?;
        let after = self.after_seconds();
        Some(session.with_session(|session| session.dormant().due(after)))
    }

    /// Tabs just woken from their sleep files: nothing's asked of the PDF on the way, as it may be on a disk that's gone away. Each is looked at on a thread of its own, and if it's turned out to be something else, it's read after all.
    fn check_woken_files(&self, session: &SessionHost<DrawableLoader>, core: &Arc<CoreQueue>) {
        let woken = session.with_session(|session| session.dormancy_mut().take_woken_from_file());
        if woken.is_empty() {
            return;
        }
        let _ = self.step(Event::Woken(woken.len()));
        for id in woken {
            self.looks.check_woken_file(session, core, id);
        }
    }
}

/// **The session and the page views, borrowed from the app for the sleeper's calls,** which read them and own neither: made for a call, or once for the
/// session thread's run inside [`GoingDormant`]. It's for three things:
/// - **what a tab's sleep file is made from now** ([`Self::facts_and_material`], [`Self::material`]): what the session says of it, and what its page view
///   holds, taken against the pages the session says the tab has now ([`pages_now`]), so that no picture in it is of a page deleted, moved or turned since it
///   was drawn. The one place that rule is carried out, for a tab going dormant, one leaving the warm ones and one written in place;
/// - **which page view holds a tab** ([`Self::engine_for`]), for what the budget's look costs it;
/// - **the session**, for the looks at woken tabs' files and the next deadline at the end of a call ([`Sleeper::after_a_call`]).
///
/// **Nothing else of the app goes in it.** The record's store, its keeping ([`KeepsRecord`]), the writer's start ([`StartsWriter`]), the core queue and the
/// clock are each passed on their own to the calls that need them, so what a call reaches is in its signature; added here, they'd make this the app by another
/// name, a context every call gets whole. Nor any state of its own: the assertion below holds it at its three borrows.
pub struct OnHand<'a> {
    session: &'a SessionHost<DrawableLoader>,
    panes: &'a Panes,
    engine: &'a Engine,
}

/// Built for the sleeper's calls that read the session or the page views, so it holds its three borrows and nothing else. Failure caught: a field added,
/// of the app's or its own.
const _: () = assert!(std::mem::size_of::<OnHand<'static>>() == 3 * std::mem::size_of::<usize>());

impl<'a> OnHand<'a> {
    /// From the app's session, its panes and its first page view.
    pub(super) fn new(session: &'a SessionHost<DrawableLoader>, panes: &'a Panes, engine: &'a Engine) -> Self {
        Self { session, panes, engine }
    }

    /// The page view holding a tab: the pane's showing it, or pane 0's, which holds the warm tabs.
    fn engine_for(&self, id: TabId) -> Engine {
        self.panes.engine_for_tab(self.engine, id)
    }

    /// What the session says of a tab's sleep file and its pages now, in one look, and what its page view has of those pages.
    fn facts_and_material(&self, id: TabId) -> Option<(SleepFacts, SleepMaterial)> {
        let (facts, now) = self
            .session
            .with_session(|session| Some((session.dormant().sleep_facts(id)?, pages_now(session, id)?)))?;
        let material = self.engine_for(id).waking().sleep_material(id.0, &now)?;
        Some((facts, material))
    }

    /// What a tab's page view has of its pages as the session says they are now, for facts already in hand (a tab leaving the warm ones).
    fn material(&self, id: TabId) -> Option<SleepMaterial> {
        let now_pages = self.session.with_session(|session| pages_now(session, id))?;
        self.engine_for(id).waking().sleep_material(id.0, &now_pages)
    }
}

/// **What a tab's going dormant reaches beyond the sleeper,** borrowed for the session thread's run, as [`OnHand`] is for one call: the session and the page
/// views (`on_hand`), what's kept on disk (the record's keeper: its store, its rule of who may write; and the writer's start), the saver (whose gate the
/// journals are written under, and which writes them) and the core's freeze (nothing goes dormant during a hand-off or a quit). **Nothing else goes in it,**
/// and it holds no state of its own: the assertion below holds it at its six borrows. Made only by [`GoingDormant::new`].
pub(super) struct GoingDormant<'a> {
    on_hand: OnHand<'a>,
    on_disk: &'a OnDisk,
    saver: &'a Saver,
    frozen: &'a AtomicBool,
}

/// Failure caught: a field added, of the app's or its own.
const _: () = assert!(std::mem::size_of::<GoingDormant<'static>>() == 6 * std::mem::size_of::<usize>());

impl<'a> GoingDormant<'a> {
    /// From the session and the page views, what's kept on disk, the saver and the core's freeze.
    pub(super) fn new(on_hand: OnHand<'a>, on_disk: &'a OnDisk, saver: &'a Saver, frozen: &'a AtomicBool) -> Self {
        Self {
            on_hand,
            on_disk,
            saver,
            frozen,
        }
    }
}

impl Sleeper {
    /// **Those tabs go dormant,** on the session thread, those that may: whatever's asking, not the one on screen or one being worked on or saved. Whatever hasn't gone to its journal goes first, or a list couldn't.
    ///
    /// The sleeper's since R3.7 S5, which gave the record's keeping a part of its own ([`Keeper`]): what it reaches beyond the sleeper is in `at`.
    pub(super) fn go_dormant(&self, at: &GoingDormant<'_>, tabs: &[TabId], why: Why) -> usize {
        let on_hand = &at.on_hand;
        let tabs: Vec<(TabId, bool)> = on_hand
            .session
            .with_session(|session| tabs.iter().map(|id| (*id, session.dormant().may_go(*id))).collect());
        let frozen = at.frozen.load(Ordering::SeqCst);
        let Some(Out::GoDormant { tabs, in_place }) = self.step(Event::GoDormant { why, tabs, frozen }) else {
            return 0;
        };
        at.saver.write_journals_now(at.on_disk.keeper(), self);
        let mut gone = 0;
        for &id in &tabs {
            // Being asked is an agent's or `recto session sleep`'s, which looks at the file straight after, and waits behind the one being written at most. The rest are the session's own, which never waits on a disk: queued, as those leaving the warm ones are (review 1 of `build/one-sleep-writer`). Being asked counts against the hour too: only a flush doesn't wait (review).
            if let (true, Why::Asked(asked)) = (in_place, why) {
                let outcome = self.write_sleep_for(on_hand, at.on_disk.keeper().store(), &at.on_disk.keeper().asks(), id, By::Asked(asked.by), None);
                self.answer(asked, outcome);
            } else {
                self.queue_sleep_for(on_hand, id, at.on_disk.starts_writer());
            }
            let list = on_hand.session.with_session(|session| session.dormant().list_to_sleep(id));
            // Compacted, and read back. If that doesn't say it's all there, the list stays in memory, and it's looked at again next time it's left alone.
            let settled = list.is_none_or(|(name, header, base)| at.on_disk.keeper().store().settle_list(&name, &header, &base));
            // If it's not gone, it's looked at again next time it's been left alone, and not till then.
            let went = on_hand.session.with_session(|session| {
                let went = settled && session.sleeper().go_dormant(id);
                if !went {
                    session.sleeper().touch(id);
                }
                went
            });
            gone += usize::from(went);
        }
        let _ = self.step(Event::WentDormant { why, gone });
        gone
    }

    /// The system's asked for memory back: the same as a tab's being left alone, for all that can go, and now. Counted first, so a test can see it came.
    /// Every page view is told too.
    pub(super) fn memory_pressure(&self, at: &GoingDormant<'_>, critical: bool) {
        let Some(Out::AllGo(critical)) = self.step(Event::Pressure { critical }) else {
            return;
        };
        self.hurry_leaving(at.on_disk.starts_writer());
        let tabs = at.on_hand.session.with_session(|session| session.dormant().oldest_first());
        self.go_dormant(at, &tabs, Why::Pressure);
        for engine in at.on_hand.panes.engines(at.on_hand.engine) {
            engine.memory_pressure(critical);
        }
    }
}

/// **What's kept on disk, as the app holds it:** the record's keeper ([`Keeper`]: the saved state, its lock and who may write it) and the sleeper (the sleep
/// files and the tabs going dormant), each behind an `Arc` the threads that write them hold in the app's place (the launch's read and a late try hold the
/// keeper; the sleep files' writer holds both). A plain holder (R3.7 S5): private fields, an accessor for each, and the one constructor of the
/// writer's start, which needs both; no logic, lock or state of its own. **Why the two are paired:** the writer's start needs both, so its one constructor
/// lives where both are, and one field for both keeps Inner within its budget of fields.
pub struct OnDisk {
    keeper: Arc<Keeper>,
    sleeper: Arc<Sleeper>,
}

impl OnDisk {
    /// The two, as the app starts.
    pub(super) fn new(keeper: Arc<Keeper>, sleeper: Arc<Sleeper>) -> Self {
        Self { keeper, sleeper }
    }

    /// The record's keeper.
    pub(crate) fn keeper(&self) -> &Arc<Keeper> {
        &self.keeper
    }

    /// The sleeper.
    pub(crate) fn sleeper(&self) -> &Arc<Sleeper> {
        &self.sleeper
    }

    /// What starts the queue's writer when the queue says there's none: the one [`StartsWriter`] there is, for the sleeper's calls that may need one. The
    /// writer's thread holds the sleeper and the record's keeper, whose rule it asks before each write ([`Keeper::asks`]), and not the app.
    pub(super) fn starts_writer(&self) -> StartsWriter<'_> {
        StartsWriter {
            sleeper: &self.sleeper,
            keeper: &self.keeper,
        }
    }
}

/// **The keeper and the sleeper, held weakly,** for the headless kit's end to wait for the threads that hold them in the app's place ([`OnDisk::downgrade`]).
#[cfg(any(test, feature = "testkit"))]
pub(crate) struct LetGo {
    keeper: std::sync::Weak<Keeper>,
    sleeper: std::sync::Weak<Sleeper>,
}

#[cfg(any(test, feature = "testkit"))]
impl OnDisk {
    /// Both, held weakly.
    pub(crate) fn downgrade(&self) -> LetGo {
        LetGo {
            keeper: Arc::downgrade(&self.keeper),
            sleeper: Arc::downgrade(&self.sleeper),
        }
    }
}

#[cfg(any(test, feature = "testkit"))]
impl LetGo {
    /// Waits, up to `limit`, until nothing holds either: every thread that held them in the app's place has ended. Fails naming what's still held.
    pub(crate) fn wait_until_let_go(&self, limit: Duration) {
        let began = Instant::now();
        while self.keeper.strong_count() + self.sleeper.strong_count() > 0 {
            assert!(
                began.elapsed() < limit,
                "{limit:?} after the app ended, the record's keeper is still held by {} and the sleeper by {}: a thread of the record's that didn't end with it",
                self.keeper.strong_count(),
                self.sleeper.strong_count()
            );
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        }
    }
}

/// **The one way the sleep queue's writer is started,** taken by every call of the [`Sleeper`]'s that may find the queue wanting one. Not a closure: the queue
/// sets its `writer` flag as it says "start one", and a caller that passed something that started nothing left the flag set with no thread, so a hurry
/// never started one and a quit's drain waited out its whole limit. Its fields are private and it's built only by [`OnDisk::starts_writer`], so what
/// it runs is always the writer's start. Two borrows, nothing more: no allocation, lock or indirection per call.
#[derive(Clone, Copy)]
pub(super) struct StartsWriter<'a> {
    sleeper: &'a Arc<Sleeper>,
    keeper: &'a Arc<Keeper>,
}

impl StartsWriter<'_> {
    /// The queue's said there's no writer: one's started, on a thread holding the sleeper and the record's keeper (not the app). With no thread to be had,
    /// the queue's told there's none after all, so the next that wants one starts it.
    fn start(self) {
        let (sleeper, keeper) = (Arc::clone(self.sleeper), Arc::clone(self.keeper));
        let spawned = std::thread::Builder::new()
            .name("recto-sleep".into())
            .stack_size(512 * 1024)
            .spawn(move || sleeper.write_leaving(keeper.store(), &keeper.asks()));
        if spawned.is_err() {
            self.sleeper.leaving.writer_gone();
        }
    }
}

impl Sleeper {
    /// A sleep file from what's known and what's on hand. With no pictures to hand, as with a closed window or what the system's taken, what's there already stands, if it's about the same list. Past the limits, a tab that's only moving about keeps what it has, and one with nothing yet has its facts, which is small and says what the PDF would.
    fn write_with_material(
        &self,
        store: &RecordStore,
        keeps: &KeepsRecord<'_>,
        facts: SleepFacts,
        material: SleepMaterial,
        by: By,
        until: Option<Instant>,
    ) -> Option<Outcome> {
        // Past `until` it's counted as skipped, by the machine.
        let _one_at_a_time = self.take_writing(until)?;
        self.write_holding_lock(store, keeps, facts, material, by)
    }

    /// The same, with the one-at-a-time lock held: what it carries is the machine's to say ([`Write`]), and an agent's is measured against its share before
    /// it's written. Says what came of it.
    ///
    /// **Only a copy that keeps the record writes one** ([`Keeper::may_write`], the rule every writer of the record goes by): the queue's writer and an
    /// asked sleep aren't behind the saver's gate, and the store's own refusal goes by its lock, which a copy can hold while it doesn't keep the record (a late
    /// keep's read, or its read past its patience). So nothing's written then, and nothing's said to have been.
    fn write_holding_lock(&self, store: &RecordStore, keeps: &KeepsRecord<'_>, mut facts: SleepFacts, material: SleepMaterial, by: By) -> Option<Outcome> {
        if !keeps.may_write() {
            return None;
        }
        let material = crate::sleep_writer::allowed_material(&mut facts, material);
        let SleepFacts {
            id,
            path,
            sleep,
            pictures_allowed,
        } = facts;
        let there = store.read_sleep(sleep.file, &path);
        let new_dark = material.pictures.first().map(|picture| picture.paper == PaperKind::Dark);
        let write = self.step(Event::Write {
            id,
            by,
            there: there.is_some(),
            pictures_allowed,
            there_for_other_paper: crate::sleep_writer::there_for_other_paper(there.as_ref(), new_dark),
            there_for_other_place: crate::sleep_writer::there_for_other_place(there.as_ref(), &sleep, &material),
        });
        let (mut write, mut cut_for_share) = match write {
            Some(Out::Write(write)) => (write, false),
            Some(Out::ShareCut(write)) => (write, true),
            _ => return None,
        };
        if write == Write::PutOff {
            return Some(Outcome { write, cut_for_share });
        }
        let had_file = there.is_some();
        let in_view = material.in_view.clone();
        let mut sleep = with_material(sleep, material);
        if write == Write::FactsOnly {
            sleep.pictures.clear();
        } else {
            sleep = crate::sleep_writer::with_older_pictures(sleep, there, &in_view);
        }
        // An agent's file, packed and measured as it's written: written only if it fits what's left of agents' share, or else its facts alone if those
        // fit and the tab has no file, or nothing. The bytes measured are the bytes handed to the store, so the share counts what reaches the drive and
        // nothing's packed twice (a photograph's page takes milliseconds, on the session thread for an agent's `session sleep`).
        let mut measured = None;
        while by.agent() {
            let packed = recto_session::sleep::encode_sleep(&sleep);
            let pictures = !sleep.pictures.is_empty();
            match self.step(Event::AgentSize {
                bytes: packed.len() as u64,
                pictures,
                there: had_file,
            }) {
                None => {
                    measured = Some(packed);
                    break;
                }
                Some(Out::ShareCut(Write::FactsOnly)) if pictures => {
                    (write, cut_for_share) = (Write::FactsOnly, true);
                    sleep.pictures.clear();
                }
                _ => {
                    return Some(Outcome {
                        write: Write::PutOff,
                        cut_for_share: true,
                    });
                }
            }
        }
        let bytes = match &measured {
            Some(packed) => store.write_sleep_bytes(&path, sleep.file, packed),
            None => store.write_sleep(&path, &sleep),
        };
        debug_assert!(
            measured.as_ref().is_none_or(|packed| bytes == 0 || bytes == packed.len() as u64),
            "an agent's file is written as measured"
        );
        let _ = self.step(Event::Wrote { id, bytes });
        Some(Outcome { write, cut_for_share })
    }

    /// **The queue's one writer** ([`crate::sleep_queue::Leaving::serve`]): oldest first, until there's none, and then it ends. A write someone's waiting on goes
    /// first. One that something newer's been written for since it was taken isn't written: that's asked with the one-at-a-time lock held, so the newer either
    /// came before or comes after.
    fn write_leaving(&self, store: &RecordStore, keeps: &KeepsRecord<'_>) {
        let leaving = &self.leaving;
        leaving.serve(
            || self.millis(),
            |facts, material, caused_by| {
                self.let_others_go_first();
                let _one_at_a_time = lock(&self.writing);
                if leaving.still_wanted() {
                    let (stamp, path) = (facts.sleep.file, facts.path.clone());
                    let wrote = self
                        .write_holding_lock(store, keeps, facts, material, By::Queue(caused_by))
                        .is_some_and(|outcome| outcome.write != Write::PutOff);
                    // Retired as it was written: the saver may have removed the file before this landed, so it goes now. Only one this wrote.
                    if wrote && leaving.retired_in_flight() {
                        store.remove_sleep(stamp, &path);
                    }
                }
            },
        );
    }
}

impl Inner {
    /// The system's memory pressure, from the host's dispatch source (and `qa memory-pressure system-warning`): acted on, or in a quiet run only counted ([`crate::pressure::heard`]). A quiet run is the one with a QA bridge.
    ///
    /// **Stays on the app** (R3.7 S6): it's the bridge's entry point (`recto-ffi` calls it), and reads the QA bridge and the core queue beside the sleeper;
    /// it goes when the bridge's entry points are next touched.
    pub fn system_pressure(&self, critical: bool) {
        let quiet = self.qa.is_some();
        if let Some(Out::Pressure(critical)) = self.on_disk.sleeper().step(Event::SystemPressure { critical, quiet }) {
            self.core.push(CoreCall::MemoryPressure(critical));
        }
    }
}

/// What a tab's first frame shows, for the page view: from the file just read, or its sleep file if nobody's read it, whichever way this tab's been asked for. Only what's about the document as it is now. Each picture says what paper it's for, and only goes on that. On the session thread, with no lock on the page view while the surfaces are made.
/// `back` is what was taken back from the queue for it, a tab shown again inside its wait: the page view let go of its pictures as it left, and these are
/// them, held to the same checks as a sleep file's (this version of its file, these pages).
pub(super) fn give_sleep_view(
    session: &SessionHost<DrawableLoader>,
    store: &RecordStore,
    engine: &recto_reader::Engine,
    id: TabId,
    back: Option<(SleepFacts, SleepMaterial)>,
) {
    let read = session.with_session(|session| session.dormancy_mut().take_sleep_read(id));
    // What was waiting first: it's what the tab left with last, newer than any file on disk for it, which may be of where it was the time before. Held to
    // the check a sleep file's pictures are.
    let back = back.map(|(mut facts, material)| {
        let material = crate::sleep_writer::allowed_material(&mut facts, material);
        with_material(facts.sleep, material)
    });
    let handed_back = back.is_some();
    let sleep = back.or(read).or_else(|| {
        let (stamp, path) = session.with_session(|session| session.dormant().sleep_to_read_awake(id))?;
        store.read_sleep(stamp, &path)
    });
    let Some(sleep) = sleep else { return };
    // Read for a file that's since been found to be another: nothing in it's about this one.
    if session
        .with_session(|session| session.tabs().own_stamp(id))
        .is_none_or(|now| !(now.same_contents(sleep.file) && now.inode == sleep.file.inode))
    {
        return;
    }
    let (pages, mark) = session.with_session(|session| (session.pages().count(id), session.lists().list_mark(id)));
    if sleep.pages != pages || sleep.list_seq != mark {
        return;
    }
    let page_box = |(width, height): (f32, f32)| PageBox { width, height };
    let mut pictures = Vec::with_capacity(sleep.pictures.len());
    for picture in &sleep.pictures {
        if let Some(surface) = surface_of(picture) {
            pictures.push(SleepShot {
                page: picture.page,
                scale: picture.scale,
                paper: if picture.dark { PaperKind::Dark } else { PaperKind::White },
                surface,
            });
        }
    }
    engine.waking().give_sleep(
        id.0,
        SleepView {
            version: sleep.version,
            usual: sleep.usual.map(page_box),
            sizes: sleep.sizes.iter().map(|(page, width, height)| (*page, page_box((*width, *height)))).collect(),
            pictures,
            handed_back,
        },
    );
}

fn with_material(mut sleep: SleepFile, material: SleepMaterial) -> SleepFile {
    sleep.version = material.version;
    sleep.usual = material.usual.map(|size| (size.width, size.height));
    sleep.sizes = material.sizes.into_iter().map(|(page, size)| (page, size.width, size.height)).collect();
    sleep.pictures = material
        .pictures
        .into_iter()
        .map(|picture| SleepPicture {
            page: picture.page,
            scale: picture.scale,
            width: picture.width,
            height: picture.height,
            dark: picture.paper == PaperKind::Dark,
            bgra: picture.bgra,
        })
        .collect();
    sleep
}

/// A surface of its own for a picture. Rows of a surface can be padded.
fn surface_of(picture: &SleepPicture) -> Option<PixelSurface> {
    let surface = PixelSurface::create(picture.width, picture.height).ok()?;
    let stride = surface.stride() as usize;
    let row = picture.width as usize * 4;
    surface
        .with_pixels(|target| {
            for (line, pixels) in target.chunks_exact_mut(stride).zip(picture.bgra.chunks_exact(row)) {
                line[..row].copy_from_slice(pixels);
            }
        })
        .ok()?;
    Some(surface)
}

pub use crate::version_of;

/// What a look at a tab's file starts from: its path, its stamp, and whether the tab says something of its file now (a change under its edits, a file gone).
fn look_facts(session: &SessionHost<DrawableLoader>, id: TabId) -> Option<(std::path::PathBuf, Option<recto_platform::FileStamp>, bool)> {
    session.with_session(|session| Some((session.tabs().path_of(id)?, session.tabs().own_stamp(id), session.notices().said(id).is_some())))
}

/// **Whether a look at a tab's file, made against `stamp`, is for the session to hear.** Not looked at is nothing. The file found as `stamp` says is nothing
/// too, unless the tab says something of its file (`says_something`): then the version its edits were made on may be back, or a file gone may be back, which
/// only the session can tell, so it's told. Found 2026-11-13 in a quiet run: that version written back over the file in place (`cp`, then `touch -r`) has the
/// same number, size and time as the stamp the edits keep, so every look called it "as it was", and the notice and the refused Save stayed.
pub(crate) fn look_is_news(status: &FileStatus, stamp: Option<recto_platform::FileStamp>, says_something: bool) -> bool {
    match status {
        FileStatus::NotLooked => false,
        FileStatus::Same(now) if Some(*now) == stamp => says_something,
        _ => true,
    }
}

/// What a look at a tab being shown found, made against `stamp`, taken in in time for its frame: on news, what's held is forgotten and the file read again.
/// A save meanwhile moves the stamp, and a look against the old one is about nothing now. Says whether it was news.
pub(crate) fn take_shown_look(
    session: &SessionHost<DrawableLoader>,
    core: &CoreQueue,
    id: TabId,
    stamp: Option<recto_platform::FileStamp>,
    status: FileStatus,
) -> bool {
    if !session.with_session(|session| session.tabs().own_stamp(id) == stamp && session.apply_file_status(id, status)) {
        return false;
    }
    session.with_session(|session| session.sleeper().forget_read(id));
    // The read, against the stamp as it is now that the news is in.
    let now = session.with_session(|session| session.tabs().own_stamp(id));
    core.push(CoreCall::WokenFileIsOther(id, FileStatus::NotLooked, now));
    true
}

/// How long a tab being shown waits for a look at its file.
const LOOK_PATIENCE: Duration = Duration::from_millis(30);

/// A test's: the next look at a shown tab's file says it has answered, then waits to be let go, so what it still holds then can be looked at.
#[cfg(test)]
static AFTER_ANSWER: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> = Mutex::new(None);

/// Where a look at a shown tab's file waits, once it has answered, while a test holds it there ([`AFTER_ANSWER`]).
#[cfg(test)]
fn after_answering() {
    if let Some((answered, go)) = lock(&AFTER_ANSWER).take() {
        let _ = answered.send(());
        let _ = go.recv_timeout(crate::testkit::headless::WAIT_LIMIT);
    }
}

/// **What the session says a tab's pages are now,** as its page view would be given them: what a sleep file's pictures are checked against, so that none is of a page that's been deleted, moved or turned since it was drawn.
fn pages_now<L: recto_session::DocLoader>(session: &recto_session::Session<L>, id: TabId) -> Option<recto_reader::tiles::PageMap> {
    crate::model::pane_snapshot(session, id, false, &[]).map(|shown| shown.map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (the settled branch, 2026-11-14: e0fa6288, found by timing the long-session app test): the look at a shown tab's file answered the frame and
    /// then gave its slot back, so a tab shown again at once found a look still running, got none of its own, and was marked to be looked at again, a round
    /// of looks more each time. Held just after it has answered: its slot is already free, so the next show of that tab starts its own look.
    #[test]
    fn a_tab_shown_again_at_once_gets_a_look_of_its_own() {
        use crate::testkit::headless::{HeadlessApp, WAIT_LIMIT};
        let app = HeadlessApp::start();
        let doc = app.disk().pdf("a.pdf", 2);
        app.open(&doc);
        let id = app.core().host.session.with_session(|session| session.tabs().active()).expect("a tab");
        let (answered, held) = std::sync::mpsc::channel();
        let (go, waiting) = std::sync::mpsc::channel();
        *lock(&AFTER_ANSWER) = Some((answered, waiting));
        let sleeper = app.core().on_disk.sleeper();
        sleeper.look_before_showing(&app.core().host.session, &app.core().core, id);
        held.recv_timeout(WAIT_LIMIT).expect("the look answered");
        let holding = sleeper.looks_running();
        drop(go);
        app.settle();
        assert_eq!(
            holding, 0,
            "the look held its slot after answering: the tab shown again would get no look of its own"
        );
    }

    /// Failure (the sleep branch's second review, 2026-11-13): what an agent was told of its `session sleep` came from a counter all agents share, compared
    /// across a flush, so another agent's put-off gave this agent a note about a sleep that was written. Each asker takes back only what its own ticket's
    /// write came to, once, and nothing for a write its share didn't cut short; past the cap the oldest unclaimed goes.
    #[test]
    fn each_asker_is_told_only_of_its_own_sleep_file() {
        let sleeper = Sleeper::new(Arc::new(crate::clock::SystemClock::new()));
        let agent = recto_agent::Actor::Agent(recto_agent::Level::Control);
        let (mine, theirs) = (sleeper.asking(agent), sleeper.asking(agent));
        assert_ne!(mine.ticket, theirs.ticket);
        let cut = Outcome {
            write: Write::PutOff,
            cut_for_share: true,
        };
        sleeper.answer(theirs, Some(cut));
        sleeper.answer(
            mine,
            Some(Outcome {
                write: Write::WithPictures,
                cut_for_share: false,
            }),
        );
        assert!(
            sleeper.take_asked_words(mine.ticket, false).is_none(),
            "another agent's put-off told to this one"
        );
        assert!(
            sleeper
                .take_asked_words(theirs.ticket, false)
                .is_some_and(|words| words.contains("wasn't written"))
        );
        assert!(sleeper.take_asked_words(theirs.ticket, false).is_none(), "taken once");
        for _ in 0..=MAX_ASKED_ANSWERS {
            sleeper.answer(sleeper.asking(agent), Some(cut));
        }
        assert_eq!(lock(&sleeper.answers).len(), MAX_ASKED_ANSWERS, "unclaimed answers are capped");
    }

    /// Failure: coming back to the app with a file on a disk that's stopped answering starts another look thread per tab each time, without end (review O3): a tab already being looked at must not get a second thread, and no more than `MAX_LOOKS` at once. And a look asked for while one is under way is lost, so a file changed after that one looked isn't noticed when the app comes to the front (seen once in the opening shots, under load).
    #[test]
    fn looks_are_one_per_tab_and_capped() {
        let slots = LookSlots::default();
        assert!(slots.start(TabId(1)));
        assert!(!slots.start(TabId(1)), "one look per tab");
        for id in 2..=(MAX_LOOKS as u32 + 4) {
            slots.start(TabId(id));
        }
        assert_eq!(slots.running(), MAX_LOOKS, "capped in all");
        assert!(slots.finish(TabId(1)), "the look asked for meanwhile is asked for again");
        assert!(slots.start(TabId(1)), "a slot comes back when a look ends");
        assert!(!slots.finish(TabId(1)), "nothing was asked for meanwhile this time");
    }
}
