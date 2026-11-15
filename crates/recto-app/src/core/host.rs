//! **The session the socket and the app share:** the host every command runs on, and what the page views tell the app.

use super::*;
use recto_agent::Code;
use recto_agent::commands::Move;
use recto_agent::commands::view::{Palette, View};
use recto_agent::{Actor, Runs};

#[cfg(test)]
mod route_tests;

/// What a command asked past a hand-off's or a quit's freeze is refused with.
pub(super) const HELD_BACK: &str = "The app is being updated, so this wasn't done.";

/// The session the socket and the app share.
pub struct AppHost {
    pub engine: Engine,
    pub session: SessionHost<DrawableLoader>,
    pub core: Arc<CoreQueue>,
    pub qa: Option<Arc<QaBridge>>,
    pub saver: Arc<Saver>,
    /// Commands running now, which an update waits for. Not the ones that only ask about the state.
    pub running: AtomicU32,
    /// Those of them anything of which can be in what a quit writes, which a quit waits for ([`super::command_kinds::changes`]).
    pub changing: AtomicU32,
    /// Those of them an agent's, which a hand-off names as an agent's: counted as they run, not worked out from the person's, which are counted from
    /// when they're asked, so neither hides the other (the phase's closing read, 2026-11-15).
    pub agents: AtomicU32,
    /// An update is under way, past the last look: what's written is what comes back, so nothing more is done.
    pub handing_off: AtomicBool,
    /// What a read of the record told the reader, while they've not moved on.
    pub notice: Mutex<Option<String>>,
    /// Who's watching over the socket, once it runs: the session's news goes on to them. See [`AppHost::listen`].
    pub watchers: Mutex<Option<Notifier>>,
}

/// **A command counted running until this is dropped** ([`AppHost::counted`]), as it returns or as its thread unwinds.
struct Counted<'a> {
    host: &'a AppHost,
    changes: bool,
    agents: bool,
}

impl<'a> Counted<'a> {
    fn start(host: &'a AppHost, changes: bool, agents: bool) -> Self {
        host.running.fetch_add(1, Ordering::SeqCst);
        if changes {
            host.changing.fetch_add(1, Ordering::SeqCst);
        }
        if agents {
            host.agents.fetch_add(1, Ordering::SeqCst);
        }
        Self { host, changes, agents }
    }
}

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        if self.agents {
            self.host.agents.fetch_sub(1, Ordering::SeqCst);
        }
        self.host.command_ended(self.changes);
    }
}

impl AppHost {
    /// The host, with nothing running, nothing said and no watchers yet.
    pub(super) fn new(engine: Engine, session: SessionHost<DrawableLoader>, core: Arc<CoreQueue>, qa: Option<Arc<QaBridge>>, saver: Arc<Saver>) -> Self {
        Self {
            engine,
            session,
            core,
            qa,
            saver,
            running: AtomicU32::new(0),
            changing: AtomicU32::new(0),
            agents: AtomicU32::new(0),
            handing_off: AtomicBool::new(false),
            notice: Mutex::new(None),
            watchers: Mutex::new(None),
        }
    }

    /// **A command run as `origin` says it was asked:** one asked before a freeze whose turn in the person's lane has come goes through it
    /// ([`crate::queue::Origin::held_back_by_a_freeze`]), as on the session thread's queue; anything else asked now is refused past it.
    pub fn run_as(&self, actor: Actor, command: &Command, events: &mut Vec<Event>, origin: crate::queue::Origin) -> Result<Value, AgentError> {
        self.counted(actor, command, origin, || self.run_counted(actor, command, events))
    }

    /// **A command of the person's at the window, as `origin` says it was asked:** as [`Self::run_as`] with [`Actor::Person`], counted the same, by the
    /// person's own door, which alone carries their passwords to their own Find and Print ([`SessionHost::run_for_person`]). The server holds this host as
    /// a [`Host`], whose `run` is [`Self::run_as`]: it can't reach this.
    pub fn run_for_person(&self, command: &Command, events: &mut Vec<Event>, origin: crate::queue::Origin) -> Result<Value, AgentError> {
        self.counted(Actor::Person, command, origin, || match command.route() {
            Runs::App => self.run_app(Actor::Person, command, events),
            Runs::Record => self.session.run(Actor::Person, command, events),
            Runs::Session | Runs::Server => {
                let result = self.session.run_for_person(command, events);
                self.core.push(CoreCall::Reconcile);
                result
            }
        })
    }

    /// `run`, counted for a hand-off and a quit, and refused past a freeze or while keeping the record settles, as [`Self::run_as`] says.
    fn counted(
        &self,
        actor: Actor,
        command: &Command,
        origin: crate::queue::Origin,
        run: impl FnOnce() -> Result<Value, AgentError>,
    ) -> Result<Value, AgentError> {
        let asks_only = command.only_asks();
        // While keeping the record is settling, an agent's command that isn't a look is refused before it's counted, so none is ever under way then
        // (`keeping::answers_agents`).
        if actor.is_agent() && !super::keeping::answers_agents(command) && self.saver.inner().is_some_and(|inner| inner.life.keeping().refuses_agents_changes())
        {
            return Err(
                AgentError::new(Code::BUSY, "RectoPDF is still starting, so this wasn't done.", recto_agent::exit::BUSY).then("Try again in a moment."),
            );
        }
        // Counted first, and then looked: a hand-off that's set this flag and sees no count can't have this one slip past both looks. Past the last look, a change would be lost: what's been written is what comes back.
        let changes = !asks_only && super::command_kinds::changes(command);
        // Let go as this returns, however: a command that panicked was counted running for good.
        let _counted = (!asks_only).then(|| Counted::start(self, changes, actor.is_agent()));
        if !asks_only && self.handing_off.load(Ordering::SeqCst) && origin.held_back_by_a_freeze() {
            return Err(AgentError::new(Code::BUSY, HELD_BACK, recto_agent::exit::BUSY).then("Try again in a moment, when it's back."));
        }
        #[cfg(test)]
        self.core.holds.at(command.tag());
        run()
    }

    /// **News from outside a command** (the page-identity thread's): to the watchers, by the one outlet that judges what of it may go to an agent
    /// ([`recto_agent::withheld`]). The window hears of it by the reconcile its sender asks for.
    pub(crate) fn tell(&self, events: Vec<Event>) {
        if let Some(notify) = lock(&self.watchers).clone() {
            notify(events);
        }
    }

    /// **The session's news reaches the app from the start, and watchers once the socket runs** (slow mode, 2026-11-11: with the socket's first answer late, Find came back with its hits and not the hit gone to). The app heard of Find's hits and of generated contents only once the socket's server ran and attached, so what finished before that, or all of it with the socket another session's, was heard by nobody. Given once, as the session takes it once.
    pub fn listen(self: &Arc<Self>) {
        let core = Arc::clone(&self.core);
        let host = Arc::downgrade(self);
        self.session.attach(Arc::new(move |events: Vec<Event>| {
            if events.iter().any(|event| event.name() == "contents_generated") {
                core.push(CoreCall::GeneratedFinished);
            }
            // The Finder's news, for the sidebar's list and a step waiting for hits.
            if events.iter().any(|event| matches!(event.name(), "find_changed" | "find_finished")) {
                core.push(CoreCall::Find(find::FindCall::News));
            }
            if let Some(notify) = host.upgrade().and_then(|host| lock(&host.watchers).clone()) {
                notify(events);
            }
        }));
    }
}

impl Host for AppHost {
    /// A command asked now, an agent's or the person's: a hand-off's or a quit's freeze holds it back.
    fn run(&self, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        self.run_as(actor, command, events, crate::queue::Origin::Input)
    }

    fn check(&self, condition: Condition, tab: &recto_agent::TabRef, page: Option<u32>) -> ConditionState {
        self.session.check(condition, tab, page)
    }

    fn tab_count(&self) -> u32 {
        self.session.tab_count()
    }

    /// Quitting is the person's. A quiet run is someone's test, and stops when it's told to.
    fn stop_refusal(&self) -> Option<AgentError> {
        self.qa.is_none().then(recto_agent::only_the_user_quits)
    }

    /// Nothing's lost by this session ending: its edits are in the state record, if it's the one keeping it and writing it (`Keeper::may_write`: not while a
    /// late try is back with the lock and not yet decided on, when the person may have started working in a window that won't be remembered).
    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        let writes = self
            .saver
            .inner()
            .map_or_else(|| self.saver.store.holds_the_record(), |inner| inner.on_disk.keeper().may_write());
        if writes { Vec::new() } else { self.session.unsaved_tabs() }
    }

    fn acted_tab(&self, command: &Command) -> Option<TabId> {
        self.session.acted_tab(command)
    }

    fn for_watchers(&self, events: &mut Vec<Event>) {
        self.session.for_watchers(events);
    }

    fn attach(&self, notify: Notifier) {
        // The session's been told since the start, by `listen`: from now its news goes on to watchers too.
        *lock(&self.watchers) = Some(notify);
    }

    fn shut_down(&self) {
        self.session.shut_down();
    }

    fn access_is_off(&self) {
        self.session.access_is_off();
    }
}

impl AppHost {
    /// **A command counted in [`Self::running`] has ended,** and in [`Self::changing`] if it `changes`: said to what waits for what's running
    /// ([`super::running::ended`]).
    pub fn command_ended(&self, changes: bool) {
        self.running.fetch_sub(1, Ordering::SeqCst);
        if changes {
            self.changing.fetch_sub(1, Ordering::SeqCst);
        }
        super::running::ended(&self.core);
    }

    /// Where a command goes, by its declared route: the app's own to [`Self::run_app`], the rest to the session.
    pub fn run_counted(&self, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        match command.route() {
            Runs::App => self.run_app(actor, command, events),
            // The record's commands are the session's, with the app's record: they change nothing the window needs to look at again.
            Runs::Record => self.session.run(actor, command, events),
            Runs::Session | Runs::Server => self.run_session(actor, command, events),
        }
    }

    /// A command the session answers, and what the window does after it.
    fn run_session(&self, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        let result = self.session.run(actor, command, events);
        self.core.push(CoreCall::Reconcile);
        result
    }

    /// The app's own commands: QA, the zoom, the palette and updates. Their level's checked here, as `execute` checks the session's: the server checked it too, but this host is also reached from inside the app.
    fn run_app(&self, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        if !actor.may(command.access()) {
            return Err(recto_agent::refusal(
                recto_agent::table::display_name(command.tag()),
                actor.level(),
                command.access(),
            ));
        }
        // What the app's own commands name (a build's zip, a QA verb's file) is held to a quiet run's folders as the session's are.
        if let Some(folders) = self.session.confinement() {
            recto_agent::quiet_files::check_paths(&folders, &recto_agent::quiet_files::named_paths(command))?;
        }
        match command {
            Command::Qa(asked) => match &self.qa {
                Some(qa) => qa.run(asked),
                None => Err(recto_agent::qa_needs_app()),
            },
            Command::Move(Move::View(asked @ View { zoom: Some(_), .. })) => self.zoom(actor, asked, events),
            Command::Move(Move::Palette(Palette { query, run, limit })) => self.run_palette(actor, query, run, limit, events),
            Command::Updates(_) => {
                let inner = self.saver.inner().ok_or_else(recto_agent::update_needs_app)?;
                super::updates::command(&inner.updates, &inner.life.update_facts, command)
            }
            // Declared the app's with nothing here to answer it: the session says it needs the app, and the guard on this routing says that's wrong.
            _ => self.run_session(actor, command, events),
        }
    }

    /// Zoom is about what's on the screen, so it's the page views': [`super::zoom::run`], as the person's is. The rest of the command is the session's.
    fn zoom(&self, actor: Actor, asked: &View, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        // A word that isn't a zoom is refused before anything else is done.
        if asked.zoom.as_deref().and_then(menus::zoom_from_word).is_none() {
            return super::zoom::run(self, asked);
        }
        let rest = Command::from(View { zoom: None, ..asked.clone() });
        let mut answer = self.session.run(actor, &rest, events)?;
        let zoomed = super::zoom::run(self, asked)?;
        answer["zoom"] = zoomed["zoom"].clone();
        if !zoomed["zoom_level"].is_null() {
            answer["zoom_level"] = zoomed["zoom_level"].clone();
        }
        self.core.push(CoreCall::Reconcile);
        Ok(answer)
    }

    /// **`recto palette`:** what the query finds, as the window's palette ranks it, and with `--run` the first row run at the agent's level, as its own command where it has one.
    fn run_palette(&self, actor: Actor, query: &Option<String>, run: &bool, limit: &Option<u32>, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        let inner = self
            .saver
            .inner()
            .ok_or_else(|| AgentError::new(Code::BUSY, "The app is going away.", recto_agent::exit::BUSY))?;
        // Nothing of a tab opened with a password, whose name may have been read with it, and files opened lately only at `recto recent`'s level.
        // In one hold of the session: the tabs left out, and Back and Forward as the gate has them for this actor, so whether they're offered says
        // nothing of a withheld place (D-2).
        let (hidden_tabs, hidden_commands) = self.session.with_session(|session| {
            let hidden_tabs = session
                .tabs()
                .iter()
                .filter(|tab| session.tabs().withheld_from_agents(tab.id))
                .map(|tab| tab.id.0)
                .collect();
            (hidden_tabs, recto_agent::place::trail_rows_hidden(session, actor))
        });
        let whom = crate::palette::ForWhom {
            hidden_tabs,
            recent: recto_agent::table::by_tag("recent").is_some_and(|spec| actor.may(spec.access)),
            hidden_commands,
        };
        let rows = palette::found_for(&inner, query.as_deref().unwrap_or(""), &whom);
        // A tab's name is the file's, so it's marked as the document's words.
        let row_json = |row: &crate::palette::PaletteRow| {
            if row.from_file {
                json!({ "id": row.id, "detail": row.detail, "shortcut": row.shortcut, "untrusted": { "title": row.title } })
            } else {
                json!({ "id": row.id, "title": row.title, "detail": row.detail, "shortcut": row.shortcut })
            }
        };
        let mut answer = json!({ "query": query, "items": rows.iter().take(limit.unwrap_or(20).clamp(1, 50) as usize).map(row_json).collect::<Vec<_>>() });
        if *run {
            let first = rows.first().ok_or_else(|| {
                AgentError::new(Code::NOTHING_FOUND, "Nothing matches that, so nothing was run.", recto_agent::exit::NOT_FOUND)
                    .then("`recto palette` lists what there is.")
            })?;
            // A tab's or a recent file's row is the agent's own command; a menu id's is the one table's (`menu_rows`).
            let id = first.id.as_str();
            let host = match palette::as_command(id, &inner.recent_files()) {
                Some(direct) => {
                    inner.tools.palette().close();
                    self.run_counted(actor, &direct, events)?;
                    HostAction::None
                }
                None => self.run_menu_id(actor, &inner, id, Some(&first.title), events)?,
            };
            let ran = if first.from_file {
                json!({ "id": first.id, "untrusted": { "title": first.title }, "host": palette::host_word(host) })
            } else {
                json!({ "id": first.id, "title": first.title, "host": palette::host_word(host) })
            };
            answer["ran"] = ran;
            self.core.push(CoreCall::Reconcile);
        }
        Ok(answer)
    }
}

impl AppHost {
    /// **An agent's palette run of a menu id:** what it comes to as an agent's own command runs as that, at the agent's level, with that command's checks,
    /// and as the agent's step, resolved by the session as it is now where the row goes by it. What's the window's alone is done only where the row lets an
    /// agent have it, at the level it says (Full, or `copy`'s), by its way, which sends nothing as anyone; the person's own is refused at every level.
    ///
    /// `matched` is the title the agent's query matched, as the palette listed it: the run names what it matched, and is refused if the palette's title for
    /// the id is no longer that, as what it would run has changed under it (*Check for Updates…* become *Update to X…* as an offer lands). `None` for a run
    /// of an id whatever the palette lists, as a test makes it.
    pub(crate) fn run_menu_id(
        &self,
        actor: Actor,
        inner: &Arc<Inner>,
        id: &str,
        matched: Option<&str>,
        events: &mut Vec<Event>,
    ) -> Result<HostAction, AgentError> {
        let ran = super::menu_rows::for_agent(inner, id, |when_run, command| {
            self.session.with_session(|session| when_run(session, actor, command))
        })?;
        if matches!(ran, super::menu_rows::ForAgent::Command(_) | super::menu_rows::ForAgent::Window { .. })
            && let Some(matched) = matched
            && palette::title_now(inner, id).as_deref() != Some(matched)
        {
            return Err(AgentError::new(
                Code::CHANGED,
                format!("“{matched}” changed between being found and being run, so nothing was run."),
                recto_agent::exit::FAILED,
            )
            .then("`recto palette` lists what there is now."));
        }
        match ran {
            super::menu_rows::ForAgent::Command(direct) => {
                inner.tools.palette().close();
                self.run_counted(actor, &direct, events)?;
                Ok(HostAction::None)
            }
            // The window's alone, which the row lets an agent have: its way, at the level that needs, sending nothing as anyone. Past a hand-off's last
            // look, what's done here wouldn't come back, so nothing is, as the menu's way has it.
            super::menu_rows::ForAgent::Window { needs, does } => {
                if !actor.may(needs) {
                    return Err(recto_agent::refusal(&format!("palette --run for {id}"), actor.level(), needs)
                        .then("Run that command itself, if it has one, at the level it needs."));
                }
                inner.tools.palette().close();
                if inner.core.frozen.load(Ordering::SeqCst) || !super::updates::takes_input_now(inner.life.window_interactive(), id) {
                    return Ok(HostAction::None);
                }
                Ok(does())
            }
            super::menu_rows::ForAgent::PersonsOwn { why, instead } => Err(AgentError::new(
                Code::PERSONS_OWN,
                format!("`{id}` is the person's own in the window ({why}), so it isn't run for an agent."),
                recto_agent::exit::FAILED,
            )
            .then(match instead {
                Some(command) => format!("{command} is an agent's own way to it, at the level it needs."),
                None => "There's no agent's command for it: `recto help` lists what there is.".to_string(),
            })),
            super::menu_rows::ForAgent::NothingNow => Err(AgentError::new(
                Code::NOTHING_TO_DO,
                format!("`{id}` has nothing to do just now, so nothing was run."),
                recto_agent::exit::FAILED,
            )
            .then("`recto palette` lists what there is now.")),
            super::menu_rows::ForAgent::Unknown => Err(AgentError::new(
                Code::NOTHING_FOUND,
                format!("`{id}` isn't anything the window has, so nothing was run."),
                recto_agent::exit::NOT_FOUND,
            )
            .then("`recto palette` lists what there is.")),
        }
    }
}

/// Files being read at once off the session thread. Each is a thread that a dead volume can hold for as long as it likes, so there's a limit, and past it a tab or file doesn't open and says so.
pub const MAX_FILE_READS: usize = 4;

pub struct EngineHooks {
    pub core: Arc<CoreQueue>,
    pub listener: Arc<dyn crate::Listener>,
}

impl EngineEvents for EngineHooks {
    fn ops_ready(&self) {
        if recto_platform::slow::is_on() {
            let listener = Arc::clone(&self.listener);
            recto_platform::slow::later(recto_platform::slow::Seam::Draw, move || listener.ops_ready());
        } else {
            self.listener.ops_ready();
        }
    }

    fn note_changed(&self) {
        self.core.push(CoreCall::Reconcile);
    }

    fn page_moved(&self) {
        self.core.push(CoreCall::Reconcile);
    }

    /// Pane 0's page view's draws, numbered 0; the other panes' come by [`Self::page_drawn_by`] with theirs (`panes::PaneEvents`).
    fn page_drawn(&self, tab: u32, page: u32, failure: Option<&'static str>, asked: u64) {
        self.page_drawn_by(0, tab, page, failure, asked);
    }

    fn page_drawn_by(&self, view: u8, tab: u32, page: u32, failure: Option<&'static str>, asked: u64) {
        self.core.push(CoreCall::Page(page, failure, TabId(tab), asked, view));
    }

    fn file_length(&self, tab: u32, pages: u32, version: u64) {
        self.core.push(CoreCall::FileLength(TabId(tab), pages, version));
    }

    fn file_replaced(&self, tab: u32) {
        self.core.push(CoreCall::LookAgain(TabId(tab)));
    }

    fn copy_ready(&self) {
        self.listener.copy_ready();
    }
}

#[cfg(test)]
mod tests {
    /// **Failure (found with the branch's review, F3): a command that panics is counted running for good,** as its count was let go only after it
    /// returned, so every settle, `qa wait-settled` and a quit's wait for what's running waited out its limit after it. A command made to panic as it's
    /// run, on a thread of its own: its count goes as it ends.
    #[test]
    fn a_command_that_panics_isnt_counted_running() {
        let app = crate::testkit::headless::HeadlessApp::start();
        let inner = std::sync::Arc::clone(app.core());
        app.panic_next("text");
        let ran = std::thread::spawn(move || {
            let command = recto_agent::parse(r#"{"cmd":"text","page":1}"#).expect("a command");
            let _ = inner.host.run_as(
                recto_agent::Actor::Agent(recto_agent::Level::Full),
                &command,
                &mut Vec::new(),
                crate::queue::Origin::Input,
            );
        });
        assert!(ran.join().is_err(), "it panicked");
        assert_eq!(app.core().host.running.load(std::sync::atomic::Ordering::SeqCst), 0, "counted running");
        assert_eq!(app.core().host.changing.load(std::sync::atomic::Ordering::SeqCst), 0, "counted changing");
    }
}
