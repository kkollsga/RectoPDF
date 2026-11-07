//! What the server asks of whoever holds the session. `recto serve` uses [`SessionHost`], and the Mac app wraps one, sharing its session.

use crate::indexing::{Indexer, Notifier};
use crate::record_store::{Flavor, Loaded, RecordStore, StoreError, notice_for};
use crate::state::StatePaths;
use recto_agent::index::Indexing;
use recto_agent::{AgentError, Command, Condition, ConditionState, DrawableLoader, Event, Level, SessionAccess, acted_tab, check_condition, exit, with_tab_candidates};
use recto_session::{DocLoader, Session, TabId};
use serde_json::{Value, json};
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
    /// Why this host won't be stopped over the socket, whatever's in its tabs.
    fn stop_refusal(&self) -> Option<AgentError> {
        None
    }
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
    /// A session with no window keeps a record of its own, written only when it's told to.
    record: Option<RecordStore>,
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
        Self { session: Mutex::new(Session::new(loader)), tabs: AtomicU32::new(0), indexer: None, record: None }
    }

    /// With documents indexed on demand, under the state folder's corpus folder.
    #[must_use]
    pub fn with_indexing(mut self, paths: &StatePaths) -> Self {
        self.indexer = Some(Indexer::new(&paths.corpus));
        self
    }

    /// With `session save` and `session restore`, to a record of its own.
    #[must_use]
    pub fn with_own_record(mut self, paths: &StatePaths) -> Self {
        self.record = Some(RecordStore::open(paths, Flavor::Windowless));
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

impl<L: DocLoader + Clone> SessionHost<L> {
    /// The record's commands, for a session with no window: nothing writes until it's told to, and each save is the whole of it. The app has its own, as it keeps its record as it goes.
    fn run_record_command(&self, level: Level, command: &Command) -> Option<Result<Value, AgentError>> {
        if !matches!(command, Command::SessionShow {} | Command::SessionSave {} | Command::SessionRestore {} | Command::SessionCheck {}) {
            return None;
        }
        // The level was checked by the server against the row. Checked here too, as `execute` would.
        if !recto_agent::allowed(level, command.access()) {
            return Some(Err(recto_agent::refusal(recto_agent::table::display_name(command.tag()), level, command.access())));
        }
        let Some(store) = &self.record else { return Some(Err(no_record())) };
        Some(match command {
            Command::SessionShow {} => Ok(self.with_session(|session| recto_agent::session_show_json(session, level, kept_json(store, None)))),
            Command::SessionCheck {} => Ok(json!({ "safe": true, "busy": [], "seconds_since_input": null })),
            Command::SessionSave {} => self.save_to(store),
            _ => self.restore_from(store, level),
        })
    }

    fn save_to(&self, store: &RecordStore) -> Result<Value, AgentError> {
        let started = std::time::Instant::now();
        let (record, checkpoints, lists, edited) = self.with_session(|session| {
            let checkpoints = session.checkpoints();
            let lists = session.lists_by_journal();
            let edited = session.tabs().iter().filter(|tab| session.page_edits(tab.id).is_some()).count();
            (session.to_record(), checkpoints, lists, edited)
        });
        for (name, header, list) in &checkpoints {
            store.write_checkpoint(name, header, list).map_err(store_error)?;
        }
        store.next_write_whole();
        store.write_record(&record, &recto_session::record::PlaceDelta::default(), true).map_err(store_error)?;
        store.verify(&record, &lists).map_err(store_error)?;
        let (record_bytes, journal_bytes) = store.sizes();
        Ok(json!({ "saved": true, "tabs": record.tabs.len(), "edited_tabs": edited, "record_bytes": record_bytes, "journal_bytes": journal_bytes, "wrote_bytes": store.wrote_bytes(), "total_ms": started.elapsed().as_millis() as u64 }))
    }

    fn restore_from(&self, store: &RecordStore, level: Level) -> Result<Value, AgentError> {
        if level < Level::Full && self.with_session(|session| session.holds_owner_edits()) {
            return Err(recto_agent::restore_would_lose_owners_edits());
        }
        let loaded = store.load();
        let notice = notice_for(&loaded);
        let Loaded::Restored(restored) = loaded else {
            return Ok(json!({ "restored": false, "count": self.tabs.load(Ordering::Relaxed), "notice": notice }));
        };
        self.with_session(|session| {
            session.restore(&restored.record, restored.journals).map_err(|error| AgentError::new(error.code(), "The record can't be used.", exit::FAILED))?;
            // The active tab's facts are what a session with no window would otherwise have to find out at its next look.
            session.wake_active();
            Ok(json!({ "restored": true, "count": session.tabs().len(), "active": session.active().map(|id| id.0), "notice": notice }))
        })
    }
}

/// What's known about a record, for `session show`. Numbers, and Rust's own words.
pub fn kept_json(store: &RecordStore, notice: Option<&str>) -> Value {
    let (record_bytes, journal_bytes) = store.sizes();
    let (writes, appends) = store.writes();
    json!({ "kept": record_bytes > 0, "holds_the_record": store.holds_the_record(), "beside_newer": store.beside_newer(), "record_bytes": record_bytes, "journal_bytes": journal_bytes, "writes": writes, "appends": appends, "wrote_bytes": store.wrote_bytes(), "notice": notice })
}

/// A store's failure, for whoever asked.
pub fn store_error(error: StoreError) -> AgentError {
    AgentError::new(error.code, error.message, exit::FAILED)
}

fn no_record() -> AgentError {
    AgentError::new("no_record", "This session doesn't keep a state record.", exit::FAILED)
}

impl<L: DocLoader + Clone + Send + 'static> Host for SessionHost<L> {
    fn run(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        if let Some(answer) = self.run_record_command(level, command) {
            return answer;
        }
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
