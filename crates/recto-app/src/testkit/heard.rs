//! **What the app announces, heard in a test** (step R2.3): a test's core has no socket, so who'd hear a person's events, `recto watch`, isn't there. These
//! hear them as it would, and an agent's command with the events it made, so a person's action and the agent's command it is can be compared.

use super::headless::HeadlessApp;
use recto_agent::{AgentError, Level};
use recto_control::host::Host;
use serde_json::Value;

/// What a test has asked to hear: what the session thread announces, and the commands the window sent for the person. Each `None` until asked for.
#[derive(Default)]
pub struct Overheard {
    pub events: Option<Vec<recto_agent::Event>>,
    pub sent: Option<Vec<recto_agent::Command>>,
}

/// The session thread held by [`HeadlessApp::hold_session_thread`]: dropped, it goes on.
pub struct SessionThreadHeld<'a>(#[allow(dead_code)] std::sync::MutexGuard<'a, Overheard>);

impl HeadlessApp {
    /// **Starts keeping the commands the window sends for the person** from now on: every one the one executor runs as theirs, as it's run.
    /// [`Self::sent`] takes them.
    pub fn keep_sent(&self) {
        crate::lock(&self.core().serving.heard).sent = Some(Vec::new());
    }

    /// The person's commands sent since [`Self::keep_sent`] or the last time this was asked, forgetting them.
    #[must_use]
    pub fn sent(&self) -> Vec<recto_agent::Command> {
        crate::lock(&self.core().serving.heard).sent.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// [`Self::agent`], with the events its command made, which the socket's server announces to watchers itself.
    ///
    /// # Errors
    ///
    /// As [`Self::agent`].
    pub fn agent_heard(&self, level: Level, line: &str) -> (Result<Value, AgentError>, Vec<recto_agent::Event>) {
        let command = match recto_agent::parse(line) {
            Ok(command) => command,
            Err(error) => return (Err(error), Vec::new()),
        };
        if !recto_agent::allowed(level, command.access()) {
            return (
                Err(recto_agent::refusal(recto_agent::table::display_name(command.tag()), level, command.access())),
                Vec::new(),
            );
        }
        let mut events = Vec::new();
        let result = self.core().host.run(recto_agent::Actor::Agent(level), &command, &mut events);
        (result, events)
    }

    /// **The session thread held between two of its calls,** until what this returns is dropped: what's queued for it from here, as an effect a command
    /// asked of the window, or the look at the session every command ends with, waits and is taken in order once it's let go. Meanwhile an agent's command
    /// still runs, on the caller's thread, as one on the socket does while the session thread is busy. So a test can say "the agent's next command comes
    /// before the window has done what its last one asked" and get it every time.
    ///
    /// It holds what every call on the session thread ends with a look at (what's overheard, [`super::heard::Overheard`]), queues a call of its own ahead
    /// of anything asked from here, and returns once that call's done all but that last look, so the thread is stopped there and takes nothing queued after.
    /// Before, it returned as soon as the call was queued, and the look at the session that call ends with ran whenever the thread got to it: on a slow
    /// machine, in the middle of the commands the test made "before the window", which it then sent on as if they'd come after (CI run 43411305187).
    /// Nothing that needs what's overheard ([`Self::heard_events`], [`Self::agent_heard`], [`Self::keep_sent`]) may be asked until it's let go.
    ///
    /// # Panics
    ///
    /// If the session thread doesn't get to the held call within [`super::headless::WAIT_LIMIT`].
    #[must_use]
    pub fn hold_session_thread(&self) -> SessionThreadHeld<'_> {
        let overheard = crate::lock(&self.core().serving.heard);
        // A flush's marker is let go after its call's look at the session and before its look at what's overheard, where the thread then waits.
        let parked = std::sync::Arc::new(crate::queue::Flushed::default());
        self.core().core.push(crate::queue::CoreCall::Flush(std::sync::Arc::clone(&parked)));
        assert!(
            parked.wait(super::headless::WAIT_LIMIT),
            "the session thread didn't get to the held call in {:?}",
            super::headless::WAIT_LIMIT
        );
        SessionThreadHeld(overheard)
    }

    /// **Starts hearing what the session thread announces to watchers** from now on, as `recto watch` would on the app's socket, which a test's core has
    /// none of. [`Self::heard_events`] takes it.
    pub fn hear_events(&self) {
        crate::lock(&self.core().serving.heard).events = Some(Vec::new());
    }

    /// What the session thread announced since [`Self::hear_events`] or the last time this was asked, forgetting it.
    #[must_use]
    pub fn heard_events(&self) -> Vec<recto_agent::Event> {
        crate::lock(&self.core().serving.heard).events.as_mut().map(std::mem::take).unwrap_or_default()
    }
}

/// **What a test asked to happen while a command takes in the views' places** ([`crate::core::Inner::take_places`]), on the thread that asked only, and
/// once each: between the read of the views and the take-in under the session's lock ([`HeadlessApp::when_the_next_take_places_has_read`]), and after the
/// take-in, before the command goes on ([`HeadlessApp::look_after_the_next_take_places`]).
#[derive(Default)]
pub struct OnTakingPlaces {
    read: Then,
    taken: Then,
}

type Then = std::sync::Mutex<Option<(std::thread::ThreadId, Box<dyn FnOnce() + Send>)>>;

impl OnTakingPlaces {
    /// The views have been read, and nothing's taken in yet.
    pub(crate) fn read(&self) {
        run_if_asked_here(&self.read);
    }

    /// What was read has been taken in.
    pub(crate) fn taken(&self) {
        run_if_asked_here(&self.taken);
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

impl HeadlessApp {
    /// **[`Self::hold_session_thread`], held from a thread of its own,** so it can be let go from inside a command the test runs, which the test's own
    /// hold, a lock on its thread, can't be. Asked, like that one, with nothing queued for the session thread: a call queued ahead of the hold's own stops at
    /// its look at what's overheard, which the hold has, and never gets to the held call.
    ///
    /// # Panics
    ///
    /// If the session thread doesn't get to the held call within [`super::headless::WAIT_LIMIT`].
    #[must_use]
    pub fn hold_session_thread_aside(&self) -> SessionThreadHeldAside {
        let inner = std::sync::Arc::clone(self.core());
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (go, go_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::Builder::new()
            .name("recto-test-hold".into())
            .spawn(move || {
                let overheard = crate::lock(&inner.serving.heard);
                let parked = std::sync::Arc::new(crate::queue::Flushed::default());
                inner.core.push(crate::queue::CoreCall::Flush(std::sync::Arc::clone(&parked)));
                let _ = held_tx.send(parked.wait(super::headless::WAIT_LIMIT));
                // Held until the sender's let go of.
                let _ = go_rx.recv();
                drop(overheard);
            })
            .expect("the hold's thread");
        assert_eq!(
            held_rx.recv().ok(),
            Some(true),
            "the session thread didn't get to the held call in {:?}",
            super::headless::WAIT_LIMIT
        );
        SessionThreadHeldAside {
            go: Some(go),
            holder: Some(holder),
        }
    }

    /// **The session thread's look comes between the next command's take-in of the views' places and the rest of it:** once this thread's next command
    /// has taken them in, `held` is let go and the command waits for the one look at the session queued meanwhile to send the jump waiting in the tab being
    /// read to the page view, and no further, before it goes on to its jump or its step. The ordering an agent's command on the socket can meet any time. A
    /// second look, as a flush's own, would take in the view's word that it's been sent there, and hide the ordering.
    ///
    /// # Panics
    ///
    /// In the command, if no jump waits on the page showing, or the page view isn't sent it within [`super::headless::WAIT_LIMIT`].
    pub fn look_after_the_next_take_places(&self, held: SessionThreadHeldAside) {
        let inner = std::sync::Arc::clone(self.core());
        let then = move || {
            // Where the session has sent the reader, the jump waiting: what the page view has once the look has sent it.
            let sent = inner
                .host
                .session
                .with_session(|s| s.active().and_then(|id| s.here(id)))
                .expect("a tab being read");
            let engine = inner.panes.reading_engine(&inner.engine);
            assert_ne!(
                engine.reading_anchor().map(|(page, within)| (page, (within * 100.0).round())),
                Some((sent.page, (f64::from(sent.within) * 100.0).round())),
                "a jump waits, not yet sent"
            );
            held.let_go();
            let began = std::time::Instant::now();
            while !engine
                .reading_anchor()
                .is_some_and(|(page, within)| page == sent.page && (within - f64::from(sent.within)).abs() < 0.005)
            {
                assert!(began.elapsed() < super::headless::WAIT_LIMIT, "the look didn't send the page view to {sent:?}");
                std::thread::sleep(std::time::Duration::from_millis(2)); // ratchet: poll
            }
        };
        *crate::lock(&self.core().serving.on_taking_places.taken) = Some((std::thread::current().id(), Box::new(then)));
    }

    /// **`then` comes between the next command's read of the views' places and its take-in of what it read,** on this thread: what the person and the
    /// session thread do meanwhile is newer than what the command read.
    pub fn when_the_next_take_places_has_read(&self, then: impl FnOnce() + Send + 'static) {
        *crate::lock(&self.core().serving.on_taking_places.read) = Some((std::thread::current().id(), Box::new(then)));
    }
}
