//! What the server asks of whoever holds the session. `recto serve` uses [`SessionHost`], and the Mac app will use one that shares its own session.

use crate::indexing::{Indexer, Notifier};
use crate::state::StatePaths;
use recto_agent::index::Indexing;
use recto_agent::{AgentError, Command, Condition, ConditionState, DrawableLoader, Event, Level, SessionAccess, acted_tab, check_condition, exit, with_tab_candidates};
use recto_session::{DocLoader, Session, TabId};
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
    /// The tabs with unsaved edits, which `stop` won't throw away.
    fn unsaved_tabs(&self) -> Vec<(TabId, String)>;
    /// The tab a command is about to act on, for the log.
    fn acted_tab(&self, _command: &Command) -> Option<TabId> {
        None
    }
    /// How to tell the session something happened outside any command. Given once, before anything runs.
    fn attach(&self, _notify: Notifier) {}
    /// The session is ending. Anything running in the background for it stops.
    fn shut_down(&self) {}
    /// Agent access is Off now. Whatever agents started that's still working on documents stops. Must never block: this is the socket's own thread.
    fn access_is_off(&self) {}
}

/// A session in this process.
pub struct SessionHost<L = DrawableLoader> {
    session: Mutex<Session<L>>,
    tabs: AtomicU32,
    indexer: Option<Indexer>,
}

/// The session and its indexer, the way a command reaches them.
struct Hosted<'a, L> {
    session: &'a Mutex<Session<L>>,
    indexer: Option<&'a Indexer>,
}

impl<L: DocLoader + Clone> SessionAccess for Hosted<'_, L> {
    type Loader = L;
    fn with<R>(&self, step: impl FnOnce(&mut Session<L>) -> R) -> R {
        self.session.with(step)
    }
    fn indexing(&self) -> Option<&dyn Indexing> {
        self.indexer.map(|indexer| indexer as &dyn Indexing)
    }
}

impl Default for SessionHost<DrawableLoader> {
    fn default() -> Self {
        Self::new(DrawableLoader)
    }
}

impl<L: DocLoader + Clone> SessionHost<L> {
    pub fn new(loader: L) -> Self {
        Self { session: Mutex::new(Session::new(loader)), tabs: AtomicU32::new(0), indexer: None }
    }

    /// With documents indexed on demand, under the state folder's corpus folder.
    #[must_use]
    pub fn with_indexing(mut self, paths: &StatePaths) -> Self {
        self.indexer = Some(Indexer::new(&paths.corpus));
        self
    }

    fn access(&self) -> Hosted<'_, L> {
        Hosted { session: &self.session, indexer: self.indexer.as_ref() }
    }

    /// The session itself, for whoever holds it: the app's own edits go through here, as the owner's. Nothing over the socket reaches this.
    pub fn with_session<R>(&self, step: impl FnOnce(&mut Session<L>) -> R) -> R {
        let (result, tabs) = {
            let mut session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let result = step(&mut session);
            (result, session.tabs().len())
        };
        self.tabs.store(u32::try_from(tabs).unwrap_or(u32::MAX), Ordering::Relaxed);
        result
    }
}

impl<L: DocLoader + Clone + Send + 'static> Host for SessionHost<L> {
    fn run(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        let result = recto_agent::execute(&self.access(), level, command, events);
        let tabs = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tabs().len();
        self.tabs.store(u32::try_from(tabs).unwrap_or(u32::MAX), Ordering::Relaxed);
        result
    }

    fn check(&self, condition: Condition, tab: &str, page: Option<u32>) -> ConditionState {
        check_condition(&self.access(), condition, tab, page)
    }

    fn tab_count(&self) -> u32 {
        self.tabs.load(Ordering::Relaxed)
    }

    fn acted_tab(&self, command: &Command) -> Option<TabId> {
        acted_tab(&self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner), command)
    }

    fn attach(&self, notify: Notifier) {
        if let Some(indexer) = &self.indexer {
            indexer.attach(notify);
        }
    }

    fn shut_down(&self) {
        if let Some(indexer) = &self.indexer {
            indexer.shut_down();
        }
    }

    fn access_is_off(&self) {
        if let Some(indexer) = &self.indexer {
            indexer.cancel(None);
        }
    }

    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        let session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Undone steps count: they can still be got back, and stopping would lose them.
        session.tabs().iter().filter(|tab| session.is_edited(tab.id) || session.page_edits(tab.id).is_some_and(recto_session::edit::PageEdits::can_redo)).map(|tab| (tab.id, tab.name.to_string())).collect()
    }
}

/// The refusal for stopping with unsaved edits.
pub(crate) fn unsaved_refusal(tabs: &[(TabId, String)]) -> AgentError {
    let mut error = AgentError::new("unsaved_changes", "The session wasn't stopped: these tabs have unsaved edits, which would be lost.", exit::FAILED);
    with_tab_candidates(&mut error, tabs.iter().map(|(id, name)| (*id, name.as_str())));
    error
}
