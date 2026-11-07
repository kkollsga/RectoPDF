//! What the server asks of whoever holds the session. `recto serve` uses [`SessionHost`], and the Mac app will use one that shares its own session.

use recto_agent::{AgentError, Command, Condition, ConditionState, DrawableLoader, Event, Level, check_condition, exit};
use recto_session::{DocLoader, Session};
use serde_json::Value;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// The session, as the server sees it. Called from job threads, never the thread watching the socket.
pub trait Host: Send + Sync + 'static {
    /// Runs a command at a level.
    fn run(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError>;
    /// Looks at a condition once.
    fn check(&self, condition: Condition, tab: &str, page: Option<u32>) -> ConditionState;
    /// How many tabs are open. Must never block: `status` answers from the socket's own thread.
    fn tab_count(&self) -> u32;
    /// The names of tabs with unsaved edits, which `stop` won't throw away.
    fn unsaved_tabs(&self) -> Vec<String>;
}

/// A session in this process.
pub struct SessionHost<L = DrawableLoader> {
    session: Mutex<Session<L>>,
    tabs: AtomicU32,
}

impl Default for SessionHost<DrawableLoader> {
    fn default() -> Self {
        Self::new(DrawableLoader)
    }
}

impl<L: DocLoader + Clone> SessionHost<L> {
    pub fn new(loader: L) -> Self {
        Self { session: Mutex::new(Session::new(loader)), tabs: AtomicU32::new(0) }
    }
}

impl<L: DocLoader + Clone + Send + 'static> Host for SessionHost<L> {
    fn run(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        let result = recto_agent::execute(&self.session, level, command, events);
        let tabs = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tabs().len();
        self.tabs.store(u32::try_from(tabs).unwrap_or(u32::MAX), Ordering::Relaxed);
        result
    }

    fn check(&self, condition: Condition, tab: &str, page: Option<u32>) -> ConditionState {
        check_condition(&self.session, condition, tab, page)
    }

    fn tab_count(&self) -> u32 {
        self.tabs.load(Ordering::Relaxed)
    }

    fn unsaved_tabs(&self) -> Vec<String> {
        let session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        session.tabs().iter().filter(|tab| session.is_edited(tab.id)).map(|tab| format!("#{} {}", tab.id.0, tab.name)).collect()
    }
}

/// The refusal for stopping with unsaved edits.
pub(crate) fn unsaved_refusal(tabs: Vec<String>) -> AgentError {
    let mut error = AgentError::new("unsaved_changes", "The session wasn't stopped: these tabs have unsaved edits, which would be lost.", exit::FAILED);
    error.candidates = tabs;
    error
}
