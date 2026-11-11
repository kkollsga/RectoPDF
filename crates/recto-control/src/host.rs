//! What the server asks of whoever holds the session. `recto serve` uses [`SessionHost`], and the Mac app wraps one, sharing its session.

use crate::finding::Finder;
use crate::generating::Generator;
use crate::indexing::{Indexer, Notifier};
use crate::record_store::{Flavor, Loaded, RecordStore, StoreError, notice_for};
use crate::state::StatePaths;
use recto_agent::index::Indexing;
use recto_agent::saving::SaveSupport;
use recto_agent::{
    AgentError, Command, Condition, ConditionState, DrawableLoader, Event, Level, SessionAccess, acted_tab, check_condition, exit, with_tab_candidates,
};
use recto_session::{DocLoader, Session, TabId};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

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
    /// The owner's standing consent to this one command, whatever the level: a switch of theirs that says it may come from anyone on this Mac who can reach the socket, which is only this user. Must never block: this is the socket's own thread.
    fn consents_to(&self, _command: &Command) -> bool {
        false
    }
}

/// A session in this process.
pub struct SessionHost<L = DrawableLoader> {
    session: Mutex<Session<L>>,
    tabs: AtomicU32,
    indexer: Option<Indexer>,
    generator: Option<Generator>,
    /// Find, in the background: the app's sidebar and `recto find`.
    finder: Finder,
    /// A session with no window keeps a record of its own, written only when it's told to.
    record: Option<RecordStore>,
    /// What a save needs beyond the session: where backups go, and the app's journals. Set once.
    saving: std::sync::OnceLock<Arc<dyn SaveSupport + Send>>,
    /// The person's passwords, for their own Find: set by the app, and none for a session with no window.
    find_keys: std::sync::OnceLock<recto_agent::find::KeyLookup>,
}

/// A session with no window keeps backups and no journals as it goes, so its saves need only the folder.
pub struct BackupsOnly(pub PathBuf);

impl SaveSupport for BackupsOnly {
    fn backups(&self) -> Option<PathBuf> {
        Some(self.0.clone())
    }
}

/// Where the one backup of each original goes: `.noindex`, so Spotlight leaves the copies alone, and out of Time Machine.
pub fn backups_folder(paths: &StatePaths) -> PathBuf {
    paths.root.join("backups.noindex")
}

/// The session and its indexer, the way a command reaches them.
struct Hosted<'a, L> {
    session: &'a Mutex<Session<L>>,
    indexer: Option<&'a Indexer>,
    saving: Option<&'a (dyn SaveSupport + Send)>,
    generator: Option<&'a Generator>,
    finder: &'a Finder,
}

impl<L: DocLoader + Clone> SessionAccess for Hosted<'_, L> {
    type Loader = L;
    fn with<R>(&self, step: impl FnOnce(&mut Session<L>) -> R) -> R {
        self.session.with(step)
    }
    fn indexing(&self) -> Option<&dyn Indexing> {
        self.indexer.map(|indexer| indexer as &dyn Indexing)
    }
    fn saving(&self) -> Option<&dyn SaveSupport> {
        self.saving.map(|saving| saving as &dyn SaveSupport)
    }
    fn generating(&self) -> Option<&dyn recto_agent::generated::Generating> {
        self.generator.map(|generator| generator as &dyn recto_agent::generated::Generating)
    }
    fn finding(&self) -> Option<&dyn recto_agent::find::Finding> {
        Some(self.finder as &dyn recto_agent::find::Finding)
    }
}

impl Default for SessionHost<DrawableLoader> {
    fn default() -> Self {
        Self::new(DrawableLoader)
    }
}

impl<L: DocLoader + Clone> SessionHost<L> {
    pub fn new(loader: L) -> Self {
        Self {
            session: Mutex::new(Session::new(loader)),
            tabs: AtomicU32::new(0),
            indexer: None,
            generator: None,
            finder: Finder::new(None),
            record: None,
            saving: std::sync::OnceLock::new(),
            find_keys: std::sync::OnceLock::new(),
        }
    }

    /// With documents indexed on demand, under the state folder's corpus folder.
    #[must_use]
    pub fn with_indexing(mut self, paths: &StatePaths) -> Self {
        let indexer = Indexer::new(&paths.corpus);
        self.finder = Finder::new(Some(indexer.clone()));
        self.indexer = Some(indexer);
        self
    }

    /// Find, for the app's own sidebar.
    pub fn finder(&self) -> &Finder {
        &self.finder
    }

    /// What the Find field does, by the same steps as `recto find`: a query, which runs in the background.
    pub fn find_begin(&self, query: &str) -> Result<u64, AgentError> {
        recto_agent::find::begin_keyed(&self.access(), query, self.find_keys.get().cloned())
    }

    /// The person's passwords, for their own Find in a document they opened with one, set once by the app: see [`recto_agent::find::FindPlan::with_keys`]. An agent's `find` never gets them.
    pub fn set_find_keys(&self, keys: recto_agent::find::KeyLookup) {
        let _ = self.find_keys.set(keys);
    }

    /// Those passwords, for a plan the app makes itself.
    pub fn find_keys(&self) -> Option<recto_agent::find::KeyLookup> {
        self.find_keys.get().cloned()
    }

    /// What's in the Find field that the hits held aren't of, if anything.
    pub fn find_stale_query(&self) -> Option<String> {
        recto_agent::find::stale_query(&self.access(), &self.finder)
    }

    /// ⌘G and ⇧⌘G, by the same step as `recto find --next`, or Return and ⇧Return in the field, the person's jump.
    pub fn find_step(&self, forward: bool, going: recto_agent::find::Going) -> Option<(u32, recto_agent::find::Hit)> {
        recto_agent::find::step(&self.access(), &self.finder, forward, going)
    }

    /// A click on a hit's row.
    pub fn find_choose(&self, index: usize) -> Option<(u32, recto_agent::find::Hit)> {
        recto_agent::find::choose(&self.access(), &self.finder, index)
    }

    /// Escape in the field, by the same step as `recto find --close`.
    pub fn find_close(&self) {
        recto_agent::find::close(&self.access());
    }

    /// With the one backup of each original kept on its first save, in the state folder. Old backups go now, as a session starts, never on a timer.
    #[must_use]
    pub fn with_backups(self, paths: &StatePaths) -> Self {
        let folder = backups_folder(paths);
        recto_core::save::prune_backups(&folder, std::time::SystemTime::now());
        self.set_saving(Arc::new(BackupsOnly(folder)));
        self
    }

    /// What saves need of whoever holds this session. The first one set stays.
    pub fn set_saving(&self, support: Arc<dyn SaveSupport + Send>) {
        let _ = self.saving.set(support);
    }

    /// With generated contents made on demand, kept in the state folder's contents folder.
    #[must_use]
    pub fn with_generating(mut self, paths: &StatePaths) -> Self {
        self.generator = Some(Generator::new(&paths.generated));
        self
    }

    /// What makes generated contents, for the app's own sidebar.
    pub fn generator(&self) -> Option<&Generator> {
        self.generator.as_ref()
    }

    /// With `session save` and `session restore`, to a record of its own.
    #[must_use]
    pub fn with_own_record(mut self, paths: &StatePaths) -> Self {
        self.record = Some(RecordStore::open(paths, Flavor::Windowless));
        self
    }

    fn access(&self) -> Hosted<'_, L> {
        Hosted {
            session: &self.session,
            indexer: self.indexer.as_ref(),
            generator: self.generator.as_ref(),
            finder: &self.finder,
            saving: self.saving.get().map(|saving| &**saving),
        }
    }

    /// The session itself, unless something's got it now. For a thread that mustn't wait for a read under the session's lock, as the UI thread mustn't.
    pub fn try_with_session<R>(&self, step: impl FnOnce(&mut Session<L>) -> R) -> Option<R> {
        let mut session = match self.session.try_lock() {
            Ok(session) => session,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        Some(step(&mut session))
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
        if !matches!(
            command,
            Command::SessionSleep { .. } | Command::SessionShow {} | Command::SessionSave {} | Command::SessionRestore {} | Command::SessionCheck {}
        ) {
            return None;
        }
        // The level was checked by the server against the row. Checked here too, as `execute` would.
        if !recto_agent::allowed(level, command.access()) {
            return Some(Err(recto_agent::refusal(
                recto_agent::table::display_name(command.tag()),
                level,
                command.access(),
            )));
        }
        let Some(store) = &self.record else { return Some(Err(no_record())) };
        Some(match command {
            Command::SessionShow {} => Ok(self.with_session(|session| recto_agent::session_show_json(session, level, kept_json(store, None)))),
            Command::SessionCheck {} => Ok(json!({ "safe": true, "busy": [], "seconds_since_input": null })),
            Command::SessionSleep { tab } => self.sleep_here(tab),
            Command::SessionSave {} => self.save_to(store),
            _ => self.restore_from(store, level),
        })
    }

    /// A session with no window has no pictures for a tab, and keeps no journals as it goes, so all it can let go of is what it's read.
    fn sleep_here(&self, tab: &str) -> Result<Value, AgentError> {
        self.with_session(|session| {
            let id = recto_agent::select_tab(session, tab)?;
            let dormant = session.go_dormant(id);
            Ok(json!({ "tab": id.0, "dormant": dormant, "list_asleep": session.list_is_asleep(id) }))
        })
    }

    fn save_to(&self, store: &RecordStore) -> Result<Value, AgentError> {
        let started = std::time::Instant::now();
        let (record, checkpoints, lists, edited, retired) = self.with_session(|session| {
            // What's been discarded goes, or it would come back at the next restore.
            let retired = session.journal_work().retired;
            let checkpoints = session.checkpoints();
            let lists = session.lists_by_journal();
            let edited = session.tabs().iter().filter(|tab| session.page_edits(tab.id).is_some()).count();
            (session.to_record(), checkpoints, lists, edited, retired)
        });
        for name in &retired {
            store.remove_journal(name);
        }
        for (name, header, list) in &checkpoints {
            store.write_checkpoint(name, header, list).map_err(store_error)?;
        }
        store.next_write_whole();
        store
            .write_record(&record, &recto_session::record::PlaceDelta::default(), true)
            .map_err(store_error)?;
        store.verify(&record, &lists).map_err(store_error)?;
        let (record_bytes, journal_bytes) = store.sizes();
        Ok(
            json!({ "saved": true, "tabs": record.tabs.len(), "edited_tabs": edited, "record_bytes": record_bytes, "journal_bytes": journal_bytes, "wrote_bytes": store.wrote_bytes(), "total_ms": started.elapsed().as_millis() as u64 }),
        )
    }

    fn restore_from(&self, store: &RecordStore, level: Level) -> Result<Value, AgentError> {
        if level < Level::Full && self.with_session(|session| session.holds_owner_edits()) {
            return Err(recto_agent::restore_would_lose_owners_edits());
        }
        let loaded = store.load();
        let notice = notice_for(&loaded).and_then(recto_session::record::notices::words);
        let Loaded::Restored(restored) = loaded else {
            return Ok(json!({ "restored": false, "count": self.tabs.load(Ordering::Relaxed), "notice": notice }));
        };
        self.with_session(|session| {
            session
                .restore(&restored.record, restored.journals)
                .map_err(|error| AgentError::new(error.code(), "The record can't be used.", exit::FAILED))?;
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
        // A tab that closes takes its generated contents' job with it, if one's running or waiting.
        let closing = matches!(command, Command::Close { .. })
            .then(|| self.acted_tab(command).and_then(|id| self.with_session(|session| session.path_of(id))))
            .flatten();
        let result = recto_agent::execute(&self.access(), level, command, events);
        if let (Some(path), Ok(_), Some(generator)) = (&closing, &result, &self.generator) {
            generator.cancel_path(path);
        }
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
        self.finder.attach(Arc::clone(&notify));
        if let Some(generator) = &self.generator {
            generator.attach(Arc::clone(&notify));
        }
        if let Some(indexer) = &self.indexer {
            indexer.attach(notify);
        }
    }

    fn shut_down(&self) {
        self.finder.shut_down();
        if let Some(indexer) = &self.indexer {
            indexer.shut_down();
        }
        if let Some(generator) = &self.generator {
            generator.shut_down();
        }
    }

    fn access_is_off(&self) {
        if let Some(indexer) = &self.indexer {
            indexer.cancel(None);
        }
        // What agents asked for stops. What the reader asked for in the sidebar does too, and they can ask again: the helper can't tell whose ask it was.
        if let Some(generator) = &self.generator {
            recto_agent::generated::Generating::cancel(generator, None);
        }
    }

    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        let session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Undone steps count: they can still be got back, and stopping would lose them.
        session
            .tabs()
            .iter()
            .filter(|tab| session.is_edited(tab.id) || session.page_edits(tab.id).is_some_and(recto_session::edit::PageEdits::can_redo))
            .map(|tab| (tab.id, tab.name.to_string()))
            .collect()
    }
}

/// The refusal for stopping with unsaved edits.
pub(crate) fn unsaved_refusal(tabs: &[(TabId, String)]) -> AgentError {
    let mut error = AgentError::new(
        "unsaved_changes",
        "The session wasn't stopped: these tabs have unsaved edits, which would be lost.",
        exit::FAILED,
    );
    with_tab_candidates(&mut error, tabs.iter().map(|(id, name)| (*id, name.as_str())));
    error
}
