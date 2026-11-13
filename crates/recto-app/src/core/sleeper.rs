//! **Tabs going dormant and waking, SPEC decision 25.** Owner, 2026-11-07: "It's fine to keep in memory for a while, and then as it goes dormant we store to disk." And: "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way that's quick to recover without any delay."
//!
//! **One deadline, and no thread.** A tab left alone for [`DORMANT_AFTER_SECONDS`] goes dormant. There's nothing per tab: the session thread's one wait has this as well as the state's deadline, and it's the soonest of the few tabs that hold anything. Coming due, it does what's asked and works out the next. At rest with nothing warm there's no deadline at all.
//!
//! **Going dormant, once:** the tab's sleep file's written if there's anything new to say, its journal's compacted and read back, and its contents, list, worker and tiles go. A byte budget across the documents off screen and memory pressure make that happen sooner, oldest first. A tab being worked on by an agent or saved doesn't go, whatever's asking: `Session::hold` and `saving` see to that.

use super::{CoreCall, Inner, lock};
use crate::clock::Clock;
use recto_platform::surface::PixelSurface;
use recto_reader::layout::PageBox;
use recto_reader::paper::PaperKind;
use recto_reader::{SleepMaterial, SleepShot, SleepView};
use recto_session::record::{FileStatus, look_at_file};
use recto_session::sleep::{SleepFile, SleepPicture};
use recto_session::{SleepFacts, TabId};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
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
    pub fn set_hour_cap(&self, bytes: u64) {
        let _ = self.step(Event::SetHourCap(bytes));
    }

    /// What agents' sleep files may come to of it, for a test of the share at the share and one past it.
    pub fn set_agent_hour_cap(&self, bytes: u64) {
        let _ = self.step(Event::SetAgentHourCap(bytes));
    }

    /// **A sleep file asked for by `by`,** with a ticket of its own to take what came of it back by ([`Self::take_asked_words`]).
    pub fn asking(&self, by: recto_agent::Actor) -> app::Asked {
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
    pub fn take_asked_words(&self, ticket: u64, on_screen: bool) -> Option<String> {
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
    pub fn retire(&self, names: &[String]) {
        self.leaving.retire(names);
    }

    /// How long a pushed-out tab's sleep file waits, for tests.
    pub fn set_leaving_wait(&self, ms: u64) {
        self.leaving.set_wait(ms);
    }

    /// The writer looks at the queue again, as the clock's moved on (a test's clock, which no timeout follows).
    pub fn poke_leaving(&self) {
        self.leaving.poke();
    }

    /// **Everything waiting is written now, without waiting for the cool-down** (memory pressure, a quit, a hand-off), with a writer started for it by `start`
    /// if there's none, as after one panicked. Not the UI thread's: that's [`Self::hurry_leaving_soon`].
    pub fn hurry_leaving(&self, start: impl FnOnce()) {
        if self.leaving.hurry() {
            start();
        }
    }

    /// **Everything waiting is written now, and waited for until `until`** (a quit's, a hand-off's), a writer started for it by `start` if there's none: past
    /// it, or with no writer to be had, what's left is written after or not at all, which costs those tabs only their quick way back. Says whether it all went.
    pub fn drain_leaving(&self, until: Instant, start: impl FnOnce()) -> bool {
        self.hurry_leaving(start);
        self.leaving.drain(until)
    }

    /// **A hurry going to the background asked for, carried out:** what waits in the cool-down is due now, under the queue's lock, and a writer's started by
    /// `start` if there's none (as after one panicked). Called by the saver's pass and at the end of a flush, never on the UI thread.
    pub fn hurry_leaving_if_asked(&self, start: impl FnOnce()) {
        if self.background_hurry.swap(false, Ordering::SeqCst) {
            self.hurry_leaving(start);
        }
    }

    /// The next sleep writer panics as it starts, for a test of what's left after one.
    #[cfg(test)]
    pub fn panic_next_leaving_writer(&self) {
        self.leaving.panic_next_writer();
    }

    /// **The app's gone to the background:** what's waiting is to be written now, by the saver's pass the caller asks for next (`Saver::write_soon`), which
    /// carries it out off this thread ([`Self::hurry_leaving_if_asked`]). The UI thread's: one atomic store, no lock, no word to the writer.
    pub fn hurry_leaving_soon(&self) {
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

impl Inner {
    /// Works out the next deadline, after anything that can have changed it. A look at the few tabs that hold anything.
    ///
    /// **Real time:** how long until the next is due is the session's own clock (`Dormant::next_in`, as every `touch` goes by it); the
    /// deadline it comes to is on the app's clock.
    pub fn rearm_dormancy(&self) {
        let after = self.sleeper.after_seconds();
        let in_seconds = self.host.session.with_session(|session| session.dormant().next_in(after));
        let _ = self.sleeper.step(Event::Rearm { in_seconds });
    }

    /// It's come due, on the session thread. Whatever's due goes dormant, unless a file's being read, when it looks again shortly.
    /// **Real time:** which are due is the session's, by its own clock (`seconds_now`).
    pub fn dormancy_due(&self) -> Option<Vec<TabId>> {
        let busy = self.life.reads_running.load(Ordering::SeqCst) > 0 || self.core.frozen.load(Ordering::SeqCst);
        self.sleeper.step(Event::DueNow { busy })?;
        let after = self.sleeper.after_seconds();
        Some(self.host.session.with_session(|session| session.dormant().due(after)))
    }

    /// **Those tabs go dormant,** on the session thread, those that may: whatever's asking, not the one on screen or one being worked on or saved. Whatever hasn't gone to its journal goes first, or a list couldn't.
    pub fn go_dormant(&self, tabs: &[TabId], why: Why) -> usize {
        let tabs: Vec<(TabId, bool)> = self
            .host
            .session
            .with_session(|session| tabs.iter().map(|id| (*id, session.dormant().may_go(*id))).collect());
        let frozen = self.core.frozen.load(Ordering::SeqCst);
        let Some(Out::GoDormant { tabs, in_place }) = self.sleeper.step(Event::GoDormant { why, tabs, frozen }) else {
            return 0;
        };
        self.saver.write_journals_now(self);
        let mut gone = 0;
        for &id in &tabs {
            // Being asked is an agent's or `recto session sleep`'s, which looks at the file straight after, and waits behind the one being written at most. The rest are the session's own, which never waits on a disk: queued, as those leaving the warm ones are (review 1 of `build/one-sleep-writer`). Being asked counts against the hour too: only a flush doesn't wait (review).
            if let (true, Why::Asked(asked)) = (in_place, why) {
                let outcome = self.write_sleep_for(id, By::Asked(asked.by));
                self.sleeper.answer(asked, outcome);
            } else {
                self.queue_sleep_for(id);
            }
            let list = self.host.session.with_session(|session| session.dormant().list_to_sleep(id));
            // Compacted, and read back. If that doesn't say it's all there, the list stays in memory, and it's looked at again next time it's left alone.
            let settled = list.is_none_or(|(name, header, base)| self.saver.store.settle_list(&name, &header, &base));
            // If it's not gone, it's looked at again next time it's been left alone, and not till then.
            let went = self.host.session.with_session(|session| {
                let went = settled && session.sleeper().go_dormant(id);
                if !went {
                    session.sleeper().touch(id);
                }
                went
            });
            gone += usize::from(went);
        }
        let _ = self.sleeper.step(Event::WentDormant { why, gone });
        gone
    }

    /// Writes a tab's sleep file from what's known of it now, if that's anything new, waiting behind the one being written: as being asked has it, or a flush.
    /// Says what came of it, if anything was decided.
    pub fn write_sleep_for(&self, id: TabId, by: By) -> Option<Outcome> {
        self.write_sleep_now_for(id, by, None)
    }

    /// **The sleep files a flush writes** (a quit's, a hand-off's, `session save`'s), the state already written. **The tab on screen's first**, the next launch's
    /// first frame, waiting for another being written only until `until` and counted as skipped past it (a quit waited its 5 s behind sleep files queued for a
    /// slow disk and went without the state, slow mode 2026-11-11); **then what's waiting in the cool-down**, hurried and waited for within what's left of the
    /// time. The other way round, on a slow disk the queued files used the limit and the one file the next launch needs was skipped (review, 2026-11-13).
    /// **Asked for by an agent** (`session save`), the cool-down is left alone (what waits stays waiting) and the tab on screen's file is held to the agent's
    /// share of the hour: an agent's save in a loop cut every cool-down short on demand. What came of that write is kept under the agent's ticket, for its
    /// answer to say if its share cut it short.
    pub fn sleep_at_flush(&self, asked: Option<app::Asked>, until: Instant) {
        let agent = asked.filter(|asked| matches!(asked.by, recto_agent::Actor::Agent(_)));
        if let Some(id) = self.host.session.with_session(|session| session.tabs().active()) {
            let outcome = self.write_sleep_now_for(id, agent.map_or(By::Flush, |asked| By::Asked(asked.by)), Some(until));
            if let Some(asked) = agent {
                self.sleeper.answer(asked, outcome);
            }
        }
        if agent.is_none() {
            self.sleeper.drain_leaving(until, || self.start_sleep_writer());
        }
    }

    fn write_sleep_now_for(&self, id: TabId, by: By, until: Option<Instant>) -> Option<Outcome> {
        let (facts, now) = self
            .host
            .session
            .with_session(|session| Some((session.dormant().sleep_facts(id)?, pages_now(session, id)?)))?;
        let material = self.panes.engine_for_tab(&self.engine, id).waking().sleep_material(id.0, &now)?;
        // What's queued for it is older than this.
        self.sleeper.leaving.forget(id);
        self.write_with_material(facts, material, by, until)
    }

    /// A sleep file from what's known and what's on hand. With no pictures to hand, as with a closed window or what the system's taken, what's there already stands, if it's about the same list. Past the limits, a tab that's only moving about keeps what it has, and one with nothing yet has its facts, which is small and says what the PDF would.
    fn write_with_material(&self, facts: SleepFacts, material: SleepMaterial, by: By, until: Option<Instant>) -> Option<Outcome> {
        // Past `until` it's counted as skipped, by the machine.
        let _one_at_a_time = self.sleeper.take_writing(until)?;
        self.write_holding_lock(facts, material, by)
    }

    /// The same, with the one-at-a-time lock held: what it carries is the machine's to say ([`Write`]), and an agent's is measured against its share before
    /// it's written. Says what came of it.
    fn write_holding_lock(&self, mut facts: SleepFacts, material: SleepMaterial, by: By) -> Option<Outcome> {
        let material = crate::sleep_writer::allowed_material(&mut facts, material);
        let SleepFacts {
            id,
            path,
            sleep,
            pictures_allowed,
        } = facts;
        let there = self.saver.store.read_sleep(sleep.file, &path);
        let new_dark = material.pictures.first().map(|picture| picture.paper == PaperKind::Dark);
        let write = self.sleeper.step(Event::Write {
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
        // An agent's file, measured as it would be written: written only if it fits what's left of agents' share, or else its facts alone if those fit
        // and the tab has no file, or nothing. Packed twice so (the store packs again), for agents' files only, which their share keeps to a few dozen an hour.
        while by.agent() {
            let bytes = recto_session::sleep::encode_sleep(&sleep).len() as u64;
            let pictures = !sleep.pictures.is_empty();
            match self.sleeper.step(Event::AgentSize {
                bytes,
                pictures,
                there: had_file,
            }) {
                None => break,
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
        let bytes = self.saver.store.write_sleep(&path, &sleep);
        let _ = self.sleeper.step(Event::Wrote { id, bytes });
        Some(Outcome { write, cut_for_share })
    }

    /// **Tabs pushed out of the warm ones, their sleep files queued,** with their pictures copied just before the page view lets go of them: a quick copy of each, of those whose time's come, queued as it's made, so what's held is the queue's cap and the one being copied. Packing a photograph takes long enough to hold up whatever's asked for next, so the one writer does that ([`crate::sleep_queue`]). Each waits [`crate::sleeper::LEAVING_WAIT_MS`] first (the cool-down on the write). A flush hurries what's waiting and waits for it for what's left of its time, then writes the tab on screen's file.
    /// `shown` is the tab being shown: what's queued for it was copied as it left, so it's taken back, not written, and said for its first frame
    /// ([`Self::give_sleep_view`]); it's queued again when it next leaves.
    pub fn queue_those_leaving(&self, shown: TabId) -> Option<(SleepFacts, SleepMaterial)> {
        let back = self.sleeper.leaving.take_back(shown);
        let left = self.host.session.with_session(|session| session.sleeper().take_left_warm());
        let now = self.sleeper.millis();
        let mut start = false;
        for (facts, pushed_by) in left {
            let Some(now_pages) = self.host.session.with_session(|session| pages_now(session, facts.id)) else {
                continue;
            };
            if let Some(material) = self
                .panes
                .engine_for_tab(&self.engine, facts.id)
                .waking()
                .sleep_material(facts.id.0, &now_pages)
            {
                start |= self.sleeper.leaving.push(facts, material, Leaves::PushedOut(pushed_by), now);
            }
        }
        if start {
            self.start_sleep_writer();
        }
        back
    }

    /// A tab going dormant by itself: its sleep file, from what's on hand now, queued for the one writer.
    fn queue_sleep_for(&self, id: TabId) {
        let Some((facts, now)) = self
            .host
            .session
            .with_session(|session| Some((session.dormant().sleep_facts(id)?, pages_now(session, id)?)))
        else {
            return;
        };
        let Some(material) = self.panes.engine_for_tab(&self.engine, id).waking().sleep_material(id.0, &now) else {
            return;
        };
        if self.sleeper.leaving.push(facts, material, Leaves::Dormant, self.sleeper.millis()) {
            self.start_sleep_writer();
        }
    }

    /// The queue's said there's no writer: one's started.
    pub(crate) fn start_sleep_writer(&self) {
        let Some(inner) = self.saver.inner() else {
            self.sleeper.leaving.writer_gone();
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("recto-sleep".into())
            .stack_size(512 * 1024)
            .spawn(move || inner.write_leaving());
        if spawned.is_err() {
            self.sleeper.leaving.writer_gone();
        }
    }

    /// **The queue's one writer** ([`crate::sleep_queue::Leaving::serve`]): oldest first, until there's none, and then it ends. A write someone's waiting on goes
    /// first. One that something newer's been written for since it was taken isn't written: that's asked with the one-at-a-time lock held, so the newer either
    /// came before or comes after.
    fn write_leaving(&self) {
        let leaving = &self.sleeper.leaving;
        leaving.serve(
            || self.sleeper.millis(),
            |facts, material, caused_by| {
                self.sleeper.let_others_go_first();
                let _one_at_a_time = lock(&self.sleeper.writing);
                if leaving.still_wanted() {
                    let (stamp, path) = (facts.sleep.file, facts.path.clone());
                    let _ = self.write_holding_lock(facts, material, By::Queue(caused_by));
                    // Retired as it was written: the saver may have removed the file before this landed, so it goes now.
                    if leaving.retired_in_flight() {
                        self.saver.store.remove_sleep(stamp, &path);
                    }
                }
            },
        );
    }

    /// Tabs just woken from their sleep files: nothing's asked of the PDF on the way, as it may be on a disk that's gone away. Each is looked at on a thread of its own, and if it's turned out to be something else, it's read after all.
    pub fn check_woken_files(&self) {
        let woken = self.host.session.with_session(|session| session.dormancy_mut().take_woken_from_file());
        if woken.is_empty() {
            return;
        }
        let _ = self.sleeper.step(Event::Woken(woken.len()));
        if let Some(inner) = self.saver.inner() {
            for id in woken {
                check_woken_file(&inner, id);
            }
        }
    }

    /// What a tab's first frame shows, for the page view: from the file just read, or its sleep file if nobody's read it, whichever way this tab's been asked for. Only what's about the document as it is now. Each picture says what paper it's for, and only goes on that. On the session thread, with no lock on the page view while the surfaces are made.
    /// `back` is what was taken back from the queue for it, a tab shown again inside its wait: the page view let go of its pictures as it left, and these are
    /// them, held to the same checks as a sleep file's (this version of its file, these pages).
    pub fn give_sleep_view(&self, engine: &recto_reader::Engine, id: TabId, back: Option<(SleepFacts, SleepMaterial)>) {
        let read = self.host.session.with_session(|session| session.dormancy_mut().take_sleep_read(id));
        // What was waiting first: it's what the tab left with last, newer than any file on disk for it, which may be of where it was the time before. Held to
        // the check a sleep file's pictures are.
        let back = back.map(|(mut facts, material)| {
            let material = crate::sleep_writer::allowed_material(&mut facts, material);
            with_material(facts.sleep, material)
        });
        let handed_back = back.is_some();
        let sleep = back.or(read).or_else(|| {
            let (stamp, path) = self.host.session.with_session(|session| session.dormant().sleep_to_read_awake(id))?;
            self.saver.store.read_sleep(stamp, &path)
        });
        let Some(sleep) = sleep else { return };
        // Read for a file that's since been found to be another: nothing in it's about this one.
        if self
            .host
            .session
            .with_session(|session| session.tabs().own_stamp(id))
            .is_none_or(|now| !(now.same_contents(sleep.file) && now.inode == sleep.file.inode))
        {
            return;
        }
        let (pages, mark) = self
            .host
            .session
            .with_session(|session| (session.pages().count(id), session.lists().list_mark(id)));
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

    /// The documents off screen against their budget. At most once a second, and only with something off screen, which most of the time there's little of.
    pub fn look_at_budget(&self) {
        let (warm, active) = self
            .host
            .session
            .with_session(|session| (session.dormant().oldest_first(), session.tabs().active()));
        // A change of tab is when a document comes to be off screen, so that's looked at straight away.
        if self
            .sleeper
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
                    self.panes.engine_for_tab(&self.engine, *id).workers().bytes(id.0)
                        + self
                            .host
                            .session
                            .with_session(|session| session.lists().get(*id).map_or(0, |list| list.history_bytes() as u64)),
                )
            })
            .collect();
        if let Some(Out::SleepTabs(over, why)) = self.sleeper.step(Event::Costs(costs)) {
            self.core.push(CoreCall::SleepTabs(over, why));
        }
    }

    /// The system's memory pressure, from the host's dispatch source (and `qa memory-pressure system-warning`): acted on, or in a quiet run only counted ([`crate::pressure::heard`]). A quiet run is the one with a QA bridge.
    pub fn system_pressure(&self, critical: bool) {
        let quiet = self.qa.is_some();
        if let Some(Out::Pressure(critical)) = self.sleeper.step(Event::SystemPressure { critical, quiet }) {
            self.core.push(CoreCall::MemoryPressure(critical));
        }
    }

    /// The system's asked for memory back: the same as a tab's being left alone, for all that can go, and now. Counted first, so a test can see it came.
    pub fn memory_pressure(&self, critical: bool) {
        let Some(Out::AllGo(critical)) = self.sleeper.step(Event::Pressure { critical }) else {
            return;
        };
        self.sleeper.hurry_leaving(|| self.start_sleep_writer());
        let tabs = self.host.session.with_session(|session| session.dormant().oldest_first());
        self.go_dormant(&tabs, Why::Pressure);
        for engine in self.panes.engines(&self.engine) {
            engine.memory_pressure(critical);
        }
    }
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

/// **Looks at a tab's file before it's shown, for as long as a frame can wait for it.** Another file there means what's held of it is of something else, so the tab's read again, and nothing drawn of the old one is shown: the page view's told its version's changed. Past the time, which is a disk that's gone away, it's shown as it was, and the look goes on on its own thread. A stat, which is microseconds.
pub fn look_before_showing(inner: &Inner, id: TabId) {
    let Some((path, stamp, says_something)) = look_facts(inner, id) else {
        return;
    };
    // One look per tab, and so many in all: with a disk that's stopped answering, showing tab after tab adds no threads past them.
    if !inner.sleeper.looks.start(id) {
        return;
    }
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let core = std::sync::Arc::clone(&inner.core);
    let slots = std::sync::Arc::clone(&inner.sleeper.looks);
    let looking = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
        let status = look_at_file(&path, stamp);
        // A look asked for meanwhile may be about a change made after this one looked: the files showing are looked at again.
        if slots.finish(id) {
            core.push(CoreCall::LookAtShown);
        }
        let other = look_is_news(&status, stamp, says_something);
        // Too late for the frame: it's said the usual way.
        if send.try_send((other, status.clone())).is_err() && other {
            core.push(CoreCall::WokenFileIsOther(id, status, stamp));
        }
    });
    if looking.is_err() {
        inner.sleeper.looks.finish(id);
        return;
    }
    // Only what's new: a tab whose file is already known to be another, shown again, is neither forgotten nor read again.
    if let Ok((true, status)) = receive.recv_timeout(LOOK_PATIENCE) {
        take_shown_look(inner, id, stamp, status);
    }
}

/// What a look at a tab's file starts from: its path, its stamp, and whether the tab says something of its file now (a change under its edits, a file gone).
fn look_facts(inner: &Inner, id: TabId) -> Option<(std::path::PathBuf, Option<recto_platform::FileStamp>, bool)> {
    inner
        .host
        .session
        .with_session(|session| Some((session.tabs().path_of(id)?, session.tabs().own_stamp(id), session.notices().said(id).is_some())))
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
pub(crate) fn take_shown_look(inner: &Inner, id: TabId, stamp: Option<recto_platform::FileStamp>, status: FileStatus) -> bool {
    if !inner
        .host
        .session
        .with_session(|session| session.tabs().own_stamp(id) == stamp && session.apply_file_status(id, status))
    {
        return false;
    }
    inner.host.session.with_session(|session| session.sleeper().forget_read(id));
    // The read, against the stamp as it is now that the news is in.
    let now = inner.host.session.with_session(|session| session.tabs().own_stamp(id));
    inner.core.push(CoreCall::WokenFileIsOther(id, FileStatus::NotLooked, now));
    true
}

/// How long a tab being shown waits for a look at its file.
const LOOK_PATIENCE: Duration = Duration::from_millis(30);

/// One thread looks at the one file a tab was woken from its sleep file on, or that's showing when the app comes to the front, and ends. If it's something else now, the session's told, and it's read after all. A tab with a look under way already, or past [`MAX_LOOKS`] in all, isn't looked at now: see [`LookSlots`].
pub fn check_woken_file(inner: &std::sync::Arc<Inner>, id: TabId) {
    let Some((path, stamp, says_something)) = look_facts(inner, id) else {
        return;
    };
    if !inner.sleeper.looks.start(id) {
        return;
    }
    let (slots, core) = (std::sync::Arc::clone(&inner.sleeper.looks), std::sync::Arc::clone(&inner.core));
    let spawned = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
        let status = look_at_file(&path, stamp);
        if look_is_news(&status, stamp, says_something) {
            core.push(CoreCall::WokenFileIsOther(id, status, stamp));
        }
        // A look asked for meanwhile may be about a change made after this one looked: the files showing are looked at again.
        if slots.finish(id) {
            core.push(CoreCall::LookAtShown);
        }
    });
    if spawned.is_err() {
        inner.sleeper.looks.finish(id);
    }
}

/// **What the session says a tab's pages are now,** as its page view would be given them: what a sleep file's pictures are checked against, so that none is of a page that's been deleted, moved or turned since it was drawn.
fn pages_now<L: recto_session::DocLoader>(session: &recto_session::Session<L>, id: TabId) -> Option<recto_reader::tiles::PageMap> {
    crate::model::pane_snapshot(session, id, false, &[]).map(|shown| shown.map)
}

#[cfg(test)]
mod tests {
    use super::*;

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
