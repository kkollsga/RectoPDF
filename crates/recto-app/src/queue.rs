//! **The session thread's queue:** what it's asked to do, in the order it was asked, with a parked thread at rest so an open app takes no wake-ups for it. Many reasons to
//! look are one look, notes of pages drawn are the only calls that can be dropped, and while a hand-off or a quit writes what comes back, nothing a person does is taken (the
//! freeze, decided per call with no default by [`CoreCall::is_user_action`]).
//!
//! The calls are the window's way of asking for things today. Step R2.3 has the window send the same `recto_agent::Command`s the CLI does, through one executor, so most of
//! the person's calls here become a command and the rest stay as the system's and the workers' news.

use crate::lock;
use recto_agent::Event;
use recto_control::record_store::Loaded;
use recto_core::{DocFacts, OpenError};
use recto_platform::FileStamp;
use recto_session::TabId;
use recto_session::record::FileStatus;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How many calls can wait for the session thread before notes of pages drawn stop being added.
pub const MAX_QUEUED_CALLS: usize = 4096;
/// What the session thread is asked to do, in the order the user asked.
#[derive(Debug)]
pub enum CoreCall {
    Reconcile,
    /// A password typed for a protected file has been tried: what it opened, or that it didn't.
    Unlocked(crate::passwords::Ask, Result<DocFacts, OpenError>, Option<FileStamp>, Arc<recto_core::Password>),
    /// The app's come to the front: the files showing are looked at, as another app may have changed them.
    LookAtShown,
    /// Those tabs go dormant.
    SleepTabs(Vec<TabId>, crate::sleeper::Why),
    /// The system's short of memory, and whether it's critical.
    MemoryPressure(bool),
    /// A tab woken from its sleep file has another file there now: what a look found, and the stamp it looked against. A result about a stamp the tab has moved
    /// on from since, as a save moves it, says nothing.
    WokenFileIsOther(TabId, FileStatus, Option<FileStamp>),
    /// The worker says how long a tab's own file is, and which version.
    FileLength(TabId, u32, u64),
    /// A worker's found a tab's file replaced, so it's looked at.
    LookAgain(TabId),
    Page(u32, Option<&'static str>, TabId),
    /// A file read off the session thread, for a tab being woken or a file being opened.
    Read(Box<FileRead>),
    /// Everything asked before this has been done.
    Flush(Arc<Flushed>),
    /// The state record has been read, on a thread that could wait for it.
    Restored(Box<Loaded>),
    /// Patience is over for reading the state, or the file a restored tab is on.
    RestorePatienceOver,
    /// What a save off this thread did, for watchers.
    Events(Vec<Event>),
    /// Generated contents were made, or failed: tabs waiting for them look.
    GeneratedFinished,
    /// What a look off this thread found of a tab's generated contents, for the version of the file it looked at.
    GeneratedView(TabId, recto_session::GeneratedView, Option<FileStamp>),
    /// The Find field, its list, and ⌘G.
    Find(crate::find::FindCall),
    /// **A command for the person** (step R2.3): the one an agent would send, run by the one executor on this thread, as theirs.
    Run(Box<PersonCommand>),
    /// A late try at the record's lock is back from its own thread ([`crate::core::keeping`]).
    TryBack(crate::core::keeping::TryBack),
    /// A late try's saved session, read on its thread, with the appearance the window had as the try was kept; or `None`, the thread ended without it (a
    /// panic). Taken in only while it's still restoring.
    LateRead(Option<Box<(Loaded, recto_session::record::Appearance)>>),
}

/// A person's command, as it's queued: what it is, where it came from, and what was read for it, if it's running again once a file's in.
#[derive(Debug)]
pub struct PersonCommand {
    pub command: recto_agent::Command,
    pub origin: Origin,
    /// A file it's opening, read off the session thread, for it to take up as it runs again.
    pub fetched: Option<(PathBuf, recto_agent::later::Fetched)>,
    /// What it comes to by what the session holds when it runs, not when it was asked: the panel's Rotate, every page or, in edit mode, the page being read.
    pub when_run: Option<WhenRun>,
}

/// **Makes a command what it is by the session as it is when it runs,** just before: a person's on the session thread, an agent's on its own. What it reads
/// it names in the command (the tab, not "the active one"), so whatever comes between this and the run, the command is about what was read; and for an
/// actor who may not read a tab, it's refused before anything of that tab is read.
pub type WhenRun =
    fn(&recto_session::Session<recto_agent::DrawableLoader>, recto_agent::Actor, recto_agent::Command) -> Result<recto_agent::Command, recto_agent::AgentError>;

impl PersonCommand {
    /// What the person just did.
    #[must_use]
    pub fn input(command: recto_agent::Command) -> Box<Self> {
        Box::new(Self {
            command,
            origin: Origin::Input,
            fetched: None,
            when_run: None,
        })
    }

    /// The end of something under way: a command running again once its read is in, with what was read, or the close after a save.
    #[must_use]
    pub fn finishing(command: recto_agent::Command, fetched: Option<(PathBuf, recto_agent::later::Fetched)>) -> Self {
        Self {
            command,
            origin: Origin::Finishing,
            fetched,
            when_run: None,
        }
    }
}

/// Where a person's command came from, which says whether a hand-off's freeze holds it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// What the person just did: held back, as everything a person does is.
    Input,
    /// The end of something already under way, as a command running again once its file's been read: let through, or it would wait for good.
    Finishing,
}

impl CoreCall {
    /// **Whether a hand-off's freeze holds it back** (review C8): what a person does, which past the hand-off's last look wouldn't come back, as what's on disk is what does. Every variant decides here, with no default, so a new one can't slip through by being forgotten.
    ///
    /// What isn't held back is the end of something already under way, or the system's or a worker's news. Dropping one of those would leave something waiting for good, or the session not answering, if the freeze lifts again: a quit that stays, or a hand-off that fails.
    #[must_use]
    pub fn is_user_action(&self) -> bool {
        match self {
            Self::Find(call) => call.is_user_action(),
            Self::Run(run) => run.origin == Origin::Input,
            // A password tried on its own thread: dropped, its prompt would wait for good. Its file's read as a `Read` is.
            Self::Unlocked(..)
            | Self::Reconcile
            | Self::LookAtShown
            | Self::SleepTabs(..)
            | Self::MemoryPressure(_)
            | Self::WokenFileIsOther(..)
            | Self::FileLength(..)
            | Self::LookAgain(_)
            | Self::Page(..)
            | Self::Read(_)
            | Self::Flush(_)
            | Self::Restored(_)
            | Self::RestorePatienceOver
            | Self::Events(_)
            | Self::GeneratedFinished
            | Self::GeneratedView(..)
            | Self::TryBack(_)
            | Self::LateRead(_) => false,
        }
    }
}

/// A marker through the session thread's queue, for QA to know what it asked for has happened.
#[derive(Debug, Default)]
pub struct Flushed {
    done: Mutex<bool>,
    ready: Condvar,
}

impl Flushed {
    pub fn finished(&self) {
        *lock(&self.done) = true;
        self.ready.notify_all();
    }

    /// Waits for it, up to a limit, and says whether it came.
    pub fn wait(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut done = lock(&self.done);
        while !*done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            done = self.ready.wait_timeout(done, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }
}

/// The session thread's queue. A parked thread at rest, so an open app takes no wake-ups for it.
#[derive(Default)]
pub struct CoreQueue {
    calls: Mutex<VecDeque<CoreCall>>,
    ready: Condvar,
    reconcile_queued: AtomicBool,
    /// A hand-off or a quit is writing what comes back, so what a person does from here wouldn't: taps, opens and the rest aren't taken.
    pub frozen: AtomicBool,
    /// The session thread is to end once it's through what's queued. Only a headless app's quit asks this, to start again on the same disk in the same process; the app's
    /// own session thread never ends, as the process does.
    ended: AtomicBool,
}

impl CoreQueue {
    pub fn push(&self, call: CoreCall) {
        // Many reasons to look can wait for one look.
        if matches!(call, CoreCall::Reconcile) && self.reconcile_queued.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.frozen.load(Ordering::SeqCst) && call.is_user_action() {
            return;
        }
        let mut calls = lock(&self.calls);
        if let CoreCall::Page(page, failure, tab) = &call {
            // Only notes of what was drawn can go, as they're told on to watchers and no more. Many tiles of a page are one note, which is most of them, so this is hard to reach. With the session thread held up for some reason this stops them growing without end. Anything a person did always gets in.
            if calls.len() >= MAX_QUEUED_CALLS
                || matches!(calls.back(), Some(CoreCall::Page(last_page, last_failure, last_tab)) if last_page == page && last_failure == failure && last_tab == tab)
            {
                return;
            }
        }
        calls.push_back(call);
        drop(calls);
        self.ready.notify_one();
    }

    /// Blocks until the session thread has got through everything already asked of it. For QA, on a job thread. A file on a dead volume could hold this up, so it has a limit.
    pub fn flush(&self, limit: Duration) -> bool {
        let marker = Arc::new(Flushed::default());
        self.push(CoreCall::Flush(Arc::clone(&marker)));
        marker.wait(limit)
    }

    /// The session thread ends once it's through what's queued: see [`Self::ended`].
    pub fn end(&self) {
        self.ended.store(true, Ordering::SeqCst);
        self.nudge();
    }

    /// **The session thread ends now, with nothing more taken,** as a process that's killed takes nothing it was asked. Only a headless app's kill asks this.
    pub fn abandon(&self) {
        // A flush another thread waits on among them is never marked done: it waits out its own limit, as nothing more is done.
        drop(std::mem::take(&mut *lock(&self.calls)));
        self.reconcile_queued.store(false, Ordering::SeqCst);
        self.end();
    }

    /// Whether the session thread is to end.
    pub fn has_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// Wakes whoever's waiting on this, to look at its deadline again.
    pub fn nudge(&self) {
        let _calls = lock(&self.calls);
        self.ready.notify_one();
    }

    /// The next call, or `None` when `deadline` says something's due, or the thread is to end: one wait for both, so the state's one deadline costs no thread or timer of its own. `deadline` is asked again on every wake, and no more than that.
    pub fn next_or_due(&self, deadline: impl Fn() -> Option<Duration>) -> Option<CoreCall> {
        let mut calls = lock(&self.calls);
        loop {
            if let Some(call) = calls.pop_front() {
                if matches!(call, CoreCall::Reconcile) {
                    self.reconcile_queued.store(false, Ordering::SeqCst);
                }
                return Some(call);
            }
            if self.has_ended() {
                return None;
            }
            calls = match deadline() {
                None => self.ready.wait(calls).unwrap_or_else(PoisonError::into_inner),
                Some(wait) if wait.is_zero() => return None,
                Some(wait) => self.ready.wait_timeout(calls, wait).unwrap_or_else(PoisonError::into_inner).0,
            };
        }
    }

    #[cfg(test)]
    fn next(&self) -> CoreCall {
        loop {
            if let Some(call) = self.next_or_due(|| None) {
                return call;
            }
        }
    }
}

/// What a file's being read for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Reading {
    Tab(TabId),
    File(PathBuf),
    /// A restored active tab: what's become of its file, first, then its facts.
    Restored(TabId),
}

/// A file read off the session thread, on its way back to it.
#[derive(Debug)]
pub struct FileRead {
    pub what: Reading,
    pub loaded: Result<DocFacts, OpenError>,
    pub stamp: Option<FileStamp>,
    /// What a look found, for a restored tab.
    pub status: Option<FileStatus>,
    /// The history it was left with, for a file being opened.
    pub history: Option<recto_session::TakenHistory>,
    /// It was read with the password the person typed for it, from the keyring.
    pub unlocked: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A person's open of a file, as the window queues it.
    fn open(path: &str) -> CoreCall {
        CoreCall::Run(PersonCommand::input(
            recto_agent::parse(&format!(r#"{{"cmd":"open","path":"{path}"}}"#)).expect("a command"),
        ))
    }

    /// A person's tap on a tab's row, as the window queues it.
    fn tap(tab: u32) -> CoreCall {
        CoreCall::Run(PersonCommand::input(
            recto_agent::parse(&format!(r##"{{"cmd":"tap","tab":"#{tab}"}}"##)).expect("a command"),
        ))
    }

    /// Failure: notes of tiles drawn fill the session thread's queue, and past the cap what a person did is thrown away: a tap, an open, the appearance. A look dropped when it's full leaves "a look is queued" set, so none is ever queued again. Only those notes can go, and repeats of one don't take a place.
    #[test]
    fn a_full_queue_loses_only_notes_of_pages_drawn() {
        let queue = CoreQueue::default();
        for page in 0..(MAX_QUEUED_CALLS as u32 + 100) {
            queue.push(CoreCall::Page(page, None, TabId(1)));
        }
        assert_eq!(lock(&queue.calls).len(), MAX_QUEUED_CALLS, "it stops at the cap");
        queue.push(tap(2));
        queue.push(open("/d/a.pdf"));
        queue.push(CoreCall::Run(PersonCommand::input(
            recto_agent::parse(r#"{"cmd":"view","sidebar":"off"}"#).expect("a command"),
        )));
        queue.push(CoreCall::Reconcile);
        queue.push(CoreCall::Reconcile);
        queue.push(CoreCall::Flush(Arc::new(Flushed::default())));
        assert_eq!(lock(&queue.calls).len(), MAX_QUEUED_CALLS + 5, "all of those are in, one look among them");
        // Drained, the look it had queued is seen, and another can be asked for.
        while !lock(&queue.calls).is_empty() {
            queue.next();
        }
        assert!(!queue.reconcile_queued.load(Ordering::SeqCst));
        queue.push(CoreCall::Reconcile);
        assert_eq!(lock(&queue.calls).len(), 1);
    }

    /// Failure: a test's kill takes what the app was asked as a killed process never would (a tap, an open, a look), so a kill test passes on work that a real
    /// kill loses. Abandoned, the queue gives the session thread nothing more and ends it, and what was waiting is gone.
    #[test]
    fn an_abandoned_queue_takes_nothing_more() {
        let queue = CoreQueue::default();
        queue.push(tap(1));
        queue.push(open("/d/a.pdf"));
        queue.push(CoreCall::Reconcile);
        queue.abandon();
        assert!(queue.has_ended());
        assert!(queue.next_or_due(|| None).is_none(), "nothing taken");
        assert!(lock(&queue.calls).is_empty());
    }

    /// Failure (review T4): past a hand-off's last look, a tap or an open is taken, and it's lost as what's on disk is what comes back. They aren't taken while it's frozen, and what isn't a person's doing still gets through, or the session would stop answering.
    #[test]
    fn a_frozen_queue_takes_nothing_a_person_does() {
        let queue = CoreQueue::default();
        queue.frozen.store(true, Ordering::SeqCst);
        queue.push(tap(1));
        queue.push(CoreCall::Run(PersonCommand::input(
            recto_agent::parse(r##"{"cmd":"goto","tab":"#1","page":2}"##).expect("a command"),
        )));
        queue.push(open("/d/a.pdf"));
        queue.push(CoreCall::Run(PersonCommand::input(
            recto_agent::parse(r#"{"cmd":"view","paper":"dark"}"#).expect("a command"),
        )));
        assert_eq!(lock(&queue.calls).len(), 0);
        queue.push(CoreCall::Reconcile);
        queue.push(CoreCall::Flush(Arc::new(Flushed::default())));
        assert_eq!(lock(&queue.calls).len(), 2);
        queue.frozen.store(false, Ordering::SeqCst);
        queue.push(tap(1));
        assert_eq!(lock(&queue.calls).len(), 3);
    }

    /// One of every call, and the find calls one of each, with its place in the list. `place` names every variant with no default, so a new one doesn't compile here until it has a place, and the test below fails until it's sampled.
    fn every_call() -> Vec<CoreCall> {
        let tab = TabId(1);
        let stamp = recto_platform::FileStamp {
            size: 1,
            mtime_ns: 1,
            device: 1,
            inode: 1,
        };
        let ask = crate::passwords::Ask {
            id: 1,
            target: crate::passwords::Target::File,
            path: PathBuf::from("/d/a.pdf"),
            wrong: 0,
            trying: true,
        };
        let read = FileRead {
            what: Reading::Tab(tab),
            loaded: Err(recto_core::OpenError::NotFound),
            stamp: None,
            status: None,
            history: None,
            unlocked: false,
        };
        let hit = recto_agent::find::Hit {
            page: 0,
            start: 0,
            end: 1,
            before: "".into(),
            matched: "".into(),
            after: "".into(),
            hidden: false,
        };
        vec![
            CoreCall::Reconcile,
            CoreCall::Unlocked(
                ask,
                Err(recto_core::OpenError::NotFound),
                Some(stamp),
                Arc::new(recto_core::Password::new("pw").unwrap()),
            ),
            CoreCall::LookAtShown,
            CoreCall::SleepTabs(vec![tab], crate::sleeper::Why::Asked),
            CoreCall::MemoryPressure(false),
            CoreCall::WokenFileIsOther(tab, FileStatus::Same(stamp), Some(stamp)),
            CoreCall::FileLength(tab, 1, 1),
            CoreCall::LookAgain(tab),
            CoreCall::Page(1, None, tab),
            CoreCall::Read(Box::new(read)),
            CoreCall::Flush(Arc::new(Flushed::default())),
            CoreCall::Restored(Box::new(Loaded::Nothing)),
            CoreCall::RestorePatienceOver,
            CoreCall::Events(Vec::new()),
            CoreCall::GeneratedFinished,
            CoreCall::GeneratedView(tab, recto_session::GeneratedView::Making, None),
            CoreCall::TryBack(crate::core::keeping::TryBack::for_test()),
            CoreCall::LateRead(None),
            CoreCall::Find(crate::find::FindCall::News),
            CoreCall::Find(crate::find::FindCall::Effect(recto_agent::Effect::Reveal { tab, hit })),
            CoreCall::Run(PersonCommand::input(recto_agent::parse(r##"{"cmd":"tap","tab":"#1"}"##).expect("a command"))),
            CoreCall::Run(Box::new(PersonCommand::finishing(
                recto_agent::parse(r##"{"cmd":"tap","tab":"#1"}"##).expect("a command"),
                None,
            ))),
        ]
    }

    /// Each variant's place, and each find call's: `(place, how many places in all)`.
    fn place(call: &CoreCall) -> (usize, usize) {
        let place = match call {
            CoreCall::Reconcile => 0,
            CoreCall::Unlocked(..) => 1,
            CoreCall::LookAtShown => 2,
            CoreCall::SleepTabs(..) => 3,
            CoreCall::MemoryPressure(_) => 4,
            CoreCall::WokenFileIsOther(..) => 5,
            CoreCall::FileLength(..) => 6,
            CoreCall::LookAgain(_) => 7,
            CoreCall::Page(..) => 8,
            CoreCall::Read(_) => 9,
            CoreCall::Flush(_) => 10,
            CoreCall::Restored(_) => 11,
            CoreCall::RestorePatienceOver => 12,
            CoreCall::Events(_) => 13,
            CoreCall::GeneratedFinished => 14,
            CoreCall::GeneratedView(..) => 15,
            CoreCall::Find(find_call) => match find_call {
                crate::find::FindCall::News => 16,
                crate::find::FindCall::Effect(_) => 17,
            },
            CoreCall::Run(run) => match run.origin {
                Origin::Input => 18,
                Origin::Finishing => 19,
            },
            CoreCall::TryBack(_) => 20,
            CoreCall::LateRead(_) => 21,
        };
        (place, 22)
    }

    /// Failure (review C8): a call a person makes isn't on the hand-off freeze's list, so it's taken while a hand-off writes what comes back, and lost: closing a tab, the close prompt's answer, the contents' note and a section's chevron were. Every variant is sampled, and those a person makes, named here apart from the code that decides, are exactly the ones held back. The rest get through.
    #[test]
    fn every_person_originated_call_is_frozen_during_a_hand_off() {
        let calls = every_call();
        let mut places: Vec<usize> = calls.iter().map(|call| place(call).0).collect();
        places.sort_unstable();
        assert_eq!(
            places,
            (0..place(&CoreCall::Reconcile).1).collect::<Vec<_>>(),
            "one sample of every call, and of every find call"
        );
        let a_persons = |call: &CoreCall| {
            matches!(call, CoreCall::Run(run) if run.origin == Origin::Input) || matches!(call, CoreCall::Find(crate::find::FindCall::Effect(_)))
        };
        for call in calls {
            let queue = CoreQueue::default();
            queue.frozen.store(true, Ordering::SeqCst);
            let persons = a_persons(&call);
            let name = format!("{call:?}");
            queue.push(call);
            assert_eq!(
                lock(&queue.calls).is_empty(),
                persons,
                "{name}: a person's call is held back during a hand-off, and nothing else is"
            );
        }
    }

    /// Failure: each tile of a page is a note, so an ordinary page of tiles uses a dozen places, and the queue's cap is reached by reading alone. In a row, one note for one page is enough.
    #[test]
    fn repeated_notes_of_a_page_take_one_place() {
        let queue = CoreQueue::default();
        for _ in 0..100 {
            queue.push(CoreCall::Page(3, None, TabId(1)));
        }
        assert_eq!(lock(&queue.calls).len(), 1);
        queue.push(CoreCall::Page(3, Some("page_failed"), TabId(1)));
        queue.push(CoreCall::Page(4, None, TabId(1)));
        assert_eq!(lock(&queue.calls).len(), 3);
    }
}
