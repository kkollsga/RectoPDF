//! **Where a test can come between the reads of the page views' places and their take-in** ([`crate::core::places`]): a command's
//! ([`crate::core::Inner::take_places`]) and the session thread's look's. A read and its take-in are apart, with no lock on the session between, and what
//! lands between them on another thread (the person's scroll, a look, another command, the saver) is where the rule that the newer read wins is tested.
//! Each seam runs once, on the thread that asked (a command's) or on the session thread (a look's), and does nothing in the app.

use super::headless::{HeadlessApp, WAIT_LIMIT};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

type Then = Mutex<Option<(std::thread::ThreadId, Box<dyn FnOnce() + Send>)>>;

/// **What a test asked of the reads of the views' places and their take-in,** and the looks counted: in [`crate::core::Serving`], test builds only.
#[derive(Default)]
pub struct PlaceSeams {
    /// A command's read is done and nothing of it is taken in.
    command_read: Then,
    /// A command's read is taken in, and the command hasn't gone on.
    command_taken: Then,
    /// The session thread's look has read the views and taken nothing in.
    look_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Looks begun, and looks done (the jumps they sent said to the session).
    looks_begun: AtomicU64,
    looks_done: AtomicU64,
    /// The most looks that may begin; one past it waits at its start until this is lifted.
    looks_allowed: Mutex<Option<u64>>,
    lifted: Condvar,
}

impl PlaceSeams {
    /// A command's read of the views is done.
    pub(crate) fn command_read(&self) {
        run_if_asked_here(&self.command_read);
    }

    /// A command's read is taken in.
    pub(crate) fn command_taken(&self) {
        run_if_asked_here(&self.command_taken);
    }

    /// The session thread's look begins: counted, and held at its start if a test allowed no more.
    pub(crate) fn look_began(&self) {
        let this = self.looks_begun.fetch_add(1, Ordering::SeqCst) + 1;
        let allowed = crate::lock(&self.looks_allowed);
        let (allowed, timeout) = self
            .lifted
            .wait_timeout_while(allowed, WAIT_LIMIT, |allowed| allowed.is_some_and(|most| this > most))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            !timeout.timed_out(),
            "look {this} held past {WAIT_LIMIT:?}: the test didn't let looks go on ({allowed:?})"
        );
    }

    /// The look has read the views.
    pub(crate) fn look_read(&self) {
        let then = crate::lock(&self.look_read).take();
        if let Some(then) = then {
            then();
        }
    }

    /// The look is done, the jumps it sent said to the session.
    pub(crate) fn look_done(&self) {
        self.looks_done.fetch_add(1, Ordering::SeqCst);
    }

    /// How many looks are done.
    pub(crate) fn looks_done(&self) -> u64 {
        self.looks_done.load(Ordering::SeqCst)
    }

    fn allow_looks(&self, most: Option<u64>) {
        *crate::lock(&self.looks_allowed) = most;
        self.lifted.notify_all();
    }
}

fn run_if_asked_here(then: &Then) {
    let here = std::thread::current().id();
    let asked = {
        let mut asked = crate::lock(then);
        if asked.as_ref().is_some_and(|(thread, _)| *thread == here) {
            asked.take()
        } else {
            None
        }
    };
    if let Some((_, then)) = asked {
        then();
    }
}

/// The session thread held by [`HeadlessApp::hold_session_thread_aside`], from a thread of its own: let go when this is let go of or dropped, from
/// whichever thread, so a test can let it go from inside a command.
pub struct SessionThreadHeldAside {
    go: Option<std::sync::mpsc::Sender<()>>,
    holder: Option<std::thread::JoinHandle<()>>,
}

impl SessionThreadHeldAside {
    /// Lets the session thread go on, and returns once it has been let go.
    pub fn let_go(mut self) {
        self.go_now();
    }

    fn go_now(&mut self) {
        drop(self.go.take());
        if let Some(holder) = self.holder.take() {
            let _ = holder.join();
        }
    }
}

impl Drop for SessionThreadHeldAside {
    fn drop(&mut self) {
        self.go_now();
    }
}

/// **The one look [`HeadlessApp::look_after_the_next_take_places`] lets come between a command's take-in and the rest of it:** no other look begins until
/// this is dropped, which the test does once the command is done.
///
/// # Panics
///
/// Dropped with the command never having taken the places in, so the look never came between and the test tested nothing.
#[must_use = "dropped at once, the command's look would come at no time the test chose"]
pub struct OneLookBetween {
    inner: Arc<crate::core::Inner>,
    came: Arc<AtomicBool>,
}

impl Drop for OneLookBetween {
    fn drop(&mut self) {
        let seams = &self.inner.serving.place_seams;
        seams.allow_looks(None);
        // Never run, it still holds the session thread: let go with it.
        drop(crate::lock(&seams.command_taken).take());
        if !self.came.load(Ordering::SeqCst) && !std::thread::panicking() {
            panic!("look_after_the_next_take_places: the next command on this thread never took the views' places in, so no look came between");
        }
    }
}

impl HeadlessApp {
    /// **[`Self::hold_session_thread`], held from a thread of its own,** so it can be let go from inside a command the test runs, which the test's own
    /// hold, a lock on its thread, can't be. Asked, like that one, with nothing queued for the session thread: a call queued ahead of the hold's own stops at
    /// its look at what's overheard, which the hold has, and never gets to the held call.
    ///
    /// # Panics
    ///
    /// If the session thread doesn't get to the held call within [`WAIT_LIMIT`].
    #[must_use]
    pub fn hold_session_thread_aside(&self) -> SessionThreadHeldAside {
        let inner = Arc::clone(self.core());
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (go, go_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::Builder::new()
            .name("recto-test-hold".into())
            .spawn(move || {
                let overheard = crate::lock(&inner.serving.heard);
                let parked = Arc::new(crate::queue::Flushed::default());
                inner.core.push(crate::queue::CoreCall::Flush(Arc::clone(&parked)));
                let _ = held_tx.send(parked.wait(WAIT_LIMIT));
                // Held until the sender's let go of.
                let _ = go_rx.recv();
                drop(overheard);
            })
            .expect("the hold's thread");
        assert_eq!(
            held_rx.recv().ok(),
            Some(true),
            "the session thread didn't get to the held call in {WAIT_LIMIT:?}"
        );
        SessionThreadHeldAside {
            go: Some(go),
            holder: Some(holder),
        }
    }

    /// **The session thread's look comes between the next command's take-in of the views' places and the rest of it, once:** when this thread's next
    /// command has taken them in, `held` is let go and the command waits for exactly one look, the one queued meanwhile, to send the page view the jump
    /// waiting in the tab being read and say so to the session, before it goes on to its jump or its step. No other look begins until what this returns is
    /// dropped: a second, as a stray call's, would take in the view's word after the send and hide the ordering. The ordering an agent's command on the
    /// socket can meet any time.
    ///
    /// # Panics
    ///
    /// In the command, if no jump waits on the page showing, if the look doesn't send it within [`WAIT_LIMIT`], or if any number of looks but one came in
    /// the gap; and when what this returns is dropped, if the command never took the places in.
    pub fn look_after_the_next_take_places(&self, held: SessionThreadHeldAside) -> OneLookBetween {
        let inner = Arc::clone(self.core());
        let came = Arc::new(AtomicBool::new(false));
        // The look queued while held may begin, and none after it.
        let begun = inner.serving.place_seams.looks_begun.load(Ordering::SeqCst);
        inner.serving.place_seams.allow_looks(Some(begun + 1));
        let then = {
            let inner = Arc::clone(&inner);
            let came = Arc::clone(&came);
            move || {
                // Where the session has sent the reader, the jump waiting: what the page view has once the look has sent it.
                let jump = inner
                    .host
                    .session
                    .with_session(|s| s.active().and_then(|id| s.waiting_jump(id)))
                    .expect("a jump waiting on the tab being read");
                let engine = inner.panes.reading_engine(&inner.engine);
                let done = inner.serving.place_seams.looks_done.load(Ordering::SeqCst);
                held.let_go();
                let began = std::time::Instant::now();
                while inner.serving.place_seams.looks_done.load(Ordering::SeqCst) == done {
                    assert!(began.elapsed() < WAIT_LIMIT, "no look came in {WAIT_LIMIT:?}");
                    std::thread::sleep(std::time::Duration::from_millis(2)); // ratchet: poll
                }
                assert_eq!(
                    inner.serving.place_seams.looks_done.load(Ordering::SeqCst),
                    done + 1,
                    "exactly one look in the gap"
                );
                let at = engine.reading_anchor().expect("a place");
                assert!(
                    at.0 == jump.page && (at.1 - f64::from(jump.within)).abs() < 0.005,
                    "the look sent the page view to {jump:?}: it's at {at:?}"
                );
                came.store(true, Ordering::SeqCst);
            }
        };
        *crate::lock(&inner.serving.place_seams.command_taken) = Some((std::thread::current().id(), Box::new(then)));
        OneLookBetween { inner, came }
    }

    /// **`then` comes between the next command's read of the views' places and its take-in of what it read,** on this thread: what the person, the
    /// session thread or another command do meanwhile is newer than what the command read.
    pub fn when_the_next_take_places_has_read(&self, then: impl FnOnce() + Send + 'static) {
        *crate::lock(&self.core().serving.place_seams.command_read) = Some((std::thread::current().id(), Box::new(then)));
    }

    /// **`then` comes between the session thread's next look's read of the views and its take-in,** on the session thread: what lands meanwhile, the
    /// person's scroll or a command run here, is newer than what the look read.
    pub fn when_the_next_look_has_read(&self, then: impl FnOnce() + Send + 'static) {
        *crate::lock(&self.core().serving.place_seams.look_read) = Some(Box::new(then));
    }

    /// How many of the session thread's looks are done: so a test can say none came, as nothing in the window queues one for a scroll within a page.
    #[must_use]
    pub fn looks_done(&self) -> u64 {
        self.core().serving.place_seams.looks_done()
    }
}
