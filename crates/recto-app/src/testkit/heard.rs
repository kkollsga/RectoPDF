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
