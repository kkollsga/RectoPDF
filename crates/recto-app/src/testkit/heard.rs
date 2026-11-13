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
