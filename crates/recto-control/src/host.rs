//! What the server asks of whoever holds the session. `recto serve` uses [`SessionHost`], and the Mac app wraps one, sharing its session.

use crate::finding::Finder;
use crate::generating::Generator;
use crate::indexing::{Indexer, Notifier};
use crate::record_store::{Flavor, RecordStore, StoreError};
use crate::state::StatePaths;
use recto_agent::commands::record::Records;
use recto_agent::index::Indexing;
use recto_agent::saving::SaveSupport;
use recto_agent::{
    Actor, AgentError, Command, Condition, ConditionState, DrawableLoader, Event, SessionAccess, acted_tab, check_condition, exit, with_tab_candidates,
};

mod own_record;
#[cfg(test)]
mod route_tests;
use recto_session::{DocLoader, Session, TabId};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// The session, as the server sees it. Called from job threads, never the thread watching the socket.
pub trait Host: Send + Sync + 'static {
    /// Runs a command for whoever asked: an agent at its level over the socket, or the person at the window.
    fn run(&self, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError>;
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
    generator: Option<Generator>,
    /// Find, in the background: the app's sidebar and `recto find`.
    finder: Finder,
    /// A session with no window keeps a record of its own, written only when it's told to.
    record: Option<RecordStore>,
    /// What a save needs beyond the session: where backups go, and the app's journals. Set once.
    saving: std::sync::OnceLock<Arc<dyn SaveSupport + Send>>,
    /// The person's passwords, for their own Find: set by the app, and none for a session with no window.
    find_keys: std::sync::OnceLock<recto_agent::find::KeyLookup>,
    /// The app's state record, which it keeps as it goes, for the record's commands. Set once, by the app.
    records: std::sync::OnceLock<Arc<dyn Records + Send + Sync>>,
    /// Where what a command asks of the window goes, set once by the app: none for a session with no window.
    effects: std::sync::OnceLock<Effects>,
    /// What takes in where the window's page views are, before a jump: set once by the app, none for a session with no window.
    places: std::sync::OnceLock<Places>,
}

/// What carries a command's [`recto_agent::Effect`] to the window.
pub type Effects = Arc<dyn Fn(recto_agent::Effect) + Send + Sync>;

/// What takes in where the window's page views have each tab: [`recto_agent::SessionAccess::take_places`].
pub type Places = Arc<dyn Fn() + Send + Sync>;

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
    /// The app's record, or this session's own, or none.
    records: Option<&'a (dyn Records + Send + Sync)>,
    effects: Option<&'a Effects>,
    places: Option<&'a Places>,
    /// How what a command needs is read, on a host that mustn't wait on a disk: the window's session thread. None everywhere else.
    reads_later: Option<&'a dyn recto_agent::later::ReadLater>,
    /// Whether the answer is read: not on the window's session thread, which draws what the session publishes.
    answer_read: bool,
    /// The person's passwords, for their own Find.
    find_keys: Option<&'a recto_agent::find::KeyLookup>,
    own_record: Option<own_record::OwnRecord<'a, L>>,
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
    fn records(&self) -> Result<&dyn Records, AgentError> {
        match (self.records, &self.own_record) {
            (Some(records), _) => Ok(records as &dyn Records),
            (None, Some(own)) => Ok(own as &dyn Records),
            (None, None) => Err(recto_agent::commands::record::no_record()),
        }
    }
    fn take_places(&self) {
        if let Some(places) = self.places {
            places();
        }
    }
    fn effect(&self, effect: recto_agent::Effect) {
        if let Some(effects) = self.effects {
            effects(effect);
        }
    }
    fn reads_later(&self) -> Option<&dyn recto_agent::later::ReadLater> {
        self.reads_later
    }
    fn answer_read(&self) -> bool {
        self.answer_read
    }
    fn find_keys(&self, actor: Actor) -> Option<recto_agent::find::KeyLookup> {
        self.find_keys.filter(|_| actor.sees_protected()).cloned()
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
            records: std::sync::OnceLock::new(),
            effects: std::sync::OnceLock::new(),
            places: std::sync::OnceLock::new(),
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
        recto_agent::find::choose(&self.access(), &self.finder, index, recto_session::Author::Owner)
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

    /// The state record whoever holds this session keeps, for the record's commands. The first one set stays, and it's used in place of one of this session's own.
    pub fn set_records(&self, records: Arc<dyn Records + Send + Sync>) {
        let _ = self.records.set(records);
    }

    /// Where what a command asks of the window goes. The first one set stays.
    pub fn set_effects(&self, effects: Effects) {
        let _ = self.effects.set(effects);
    }

    /// Where the window's page views are is taken in from here before a jump, once, by the app.
    pub fn set_places(&self, places: Places) {
        let _ = self.places.set(places);
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
            records: self.records.get().map(|records| &**records),
            effects: self.effects.get(),
            places: self.places.get(),
            reads_later: None,
            answer_read: true,
            find_keys: self.find_keys.get(),
            own_record: self.record.as_ref().map(|store| own_record::OwnRecord { session: &self.session, store }),
        }
    }

    /// **A command on a thread that mustn't wait on a disk,** the window's session thread: what it needs from a file is read by `reads`, off this thread, and it
    /// runs again once that's in ([`recto_agent::later`]). Its answer isn't read, as the window draws what the session publishes, so a command that would build
    /// a costly one doesn't (`SessionAccess::answer_read`). Otherwise as [`Host::run`].
    pub fn run_reading_later(
        &self,
        actor: Actor,
        command: &Command,
        events: &mut Vec<Event>,
        reads: &dyn recto_agent::later::ReadLater,
    ) -> Result<Value, AgentError> {
        self.run_on(
            &Hosted {
                reads_later: Some(reads),
                answer_read: false,
                ..self.access()
            },
            actor,
            command,
            events,
        )
    }

    /// A command on the session, by whichever way it's reached, and what a close takes with it.
    fn run_on(&self, access: &Hosted<'_, L>, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        // A tab that closes takes its generated contents' job with it, if one's running or waiting.
        let closing = matches!(command, Command::Move(recto_agent::commands::Move::Close(_)))
            .then(|| {
                let session = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                acted_tab(&session, command).and_then(|id| session.path_of(id))
            })
            .flatten();
        let result = recto_agent::execute(access, actor, command, events);
        if let (Some(path), Ok(_), Some(generator)) = (&closing, &result, &self.generator) {
            generator.cancel_path(path);
        }
        let tabs = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tabs().len();
        self.tabs.store(u32::try_from(tabs).unwrap_or(u32::MAX), Ordering::Relaxed);
        result
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

impl<L: DocLoader + Clone + Send + 'static> Host for SessionHost<L> {
    fn run(&self, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        self.run_on(&self.access(), actor, command, events)
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
