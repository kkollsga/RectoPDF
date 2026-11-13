//! **The session the socket and the app share:** the host every command runs on, and what the page views tell the app.

use super::*;
use recto_agent::commands::Move;
use recto_agent::commands::view::{Palette, View};
use recto_agent::{Actor, Runs};

#[cfg(test)]
mod route_tests;

/// The session the socket and the app share.
pub struct AppHost {
    pub engine: Engine,
    pub session: SessionHost<DrawableLoader>,
    pub core: Arc<CoreQueue>,
    pub qa: Option<Arc<QaBridge>>,
    pub saver: Arc<Saver>,
    /// Commands running now, which an update waits for. Not the ones that only ask about the state.
    pub running: AtomicU32,
    /// An update is under way, past the last look: what's written is what comes back, so nothing more is done.
    pub handing_off: AtomicBool,
    /// What a read of the record told the reader, while they've not moved on.
    pub notice: Mutex<Option<String>>,
    /// Who's watching over the socket, once it runs: the session's news goes on to them. See [`AppHost::listen`].
    pub watchers: Mutex<Option<Notifier>>,
}

impl AppHost {
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
    fn run(&self, actor: Actor, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        let asks_only = command.only_asks();
        // Counted first, and then looked: a hand-off that's set this flag and sees no count can't have this one slip past both looks. Past the last look, a change would be lost: what's been written is what comes back.
        if !asks_only {
            self.running.fetch_add(1, Ordering::SeqCst);
            if self.handing_off.load(Ordering::SeqCst) {
                self.running.fetch_sub(1, Ordering::SeqCst);
                return Err(
                    AgentError::new("busy", "The app is being updated, so this wasn't done.", recto_agent::exit::BUSY)
                        .then("Try again in a moment, when it's back."),
                );
            }
        }
        let result = self.run_counted(actor, command, events);
        if !asks_only {
            self.running.fetch_sub(1, Ordering::SeqCst);
        }
        result
    }

    fn check(&self, condition: Condition, tab: &str, page: Option<u32>) -> ConditionState {
        self.session.check(condition, tab, page)
    }

    fn tab_count(&self) -> u32 {
        self.session.tab_count()
    }

    /// Quitting is the person's. A quiet run is someone's test, and stops when it's told to.
    fn stop_refusal(&self) -> Option<AgentError> {
        self.qa.is_none().then(recto_agent::only_the_user_quits)
    }

    /// Nothing's lost by this session ending: its edits are in the state record, if it's the one keeping it.
    fn unsaved_tabs(&self) -> Vec<(TabId, String)> {
        if self.saver.store.holds_the_record() {
            Vec::new()
        } else {
            self.session.unsaved_tabs()
        }
    }

    fn acted_tab(&self, command: &Command) -> Option<TabId> {
        self.session.acted_tab(command)
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
        if let Some(folders) = self.session.with_session(|session| session.confinement().folders().cloned()) {
            recto_agent::quiet_files::check_paths(&folders, &recto_agent::quiet_files::named_paths(command))?;
        }
        match command {
            Command::Qa(asked) => match &self.qa {
                Some(qa) => qa.run(asked),
                None => Err(recto_agent::qa_needs_app()),
            },
            Command::Move(Move::View(asked @ View { zoom: Some(_), .. })) => self.zoom(actor, asked, events),
            Command::Move(Move::Palette(Palette { query, run, limit })) => self.run_palette(actor, query, run, limit, events),
            Command::Updates(_) => self.saver.inner().ok_or_else(recto_agent::update_needs_app)?.update_command(command),
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
            .ok_or_else(|| AgentError::new("busy", "The app is going away.", recto_agent::exit::BUSY))?;
        // Nothing of a tab opened with a password, whose name may have been read with it, and files opened lately only at `recto recent`'s level.
        let whom = crate::palette::ForWhom {
            hidden_tabs: self.session.with_session(|session| {
                session
                    .tabs()
                    .iter()
                    .filter(|tab| session.withheld_from_agents(tab.id))
                    .map(|tab| tab.id.0)
                    .collect()
            }),
            recent: recto_agent::table::by_tag("recent").is_some_and(|spec| actor.may(spec.access)),
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
                AgentError::new("nothing_found", "Nothing matches that, so nothing was run.", recto_agent::exit::NOT_FOUND)
                    .then("`recto palette` lists what there is.")
            })?;
            let reader = lock(&inner.published).reader.clone();
            // What it comes to as an agent's own command runs as that, at the agent's level, with that command's checks, and as the agent's step. What has none, or writes the clipboard, needs what the direct way needs: Full, or `copy`'s level.
            let host = match palette::as_command(&first.id, &reader, &inner.recent_files()) {
                Some(direct) => {
                    inner.tools.palette.close();
                    self.run_counted(actor, &direct, events)?;
                    HostAction::None
                }
                // A row that's a person's command by the menu's way, with none of the agent's own: the menu's way would run it as the person, past the
                // refusals every agent's command has. Refused, whatever the level.
                None if !crate::palette::THE_WINDOWS_ALONE.contains(&first.id.as_str()) => {
                    let full = recto_agent::table::Access::At(Level::Full);
                    if !actor.may(full) {
                        return Err(recto_agent::refusal(&format!("palette --run for {}", first.id), actor.level(), full)
                            .then("Run that command itself, if it has one, at the level it needs."));
                    }
                    return Err(AgentError::new(
                        "needs_its_command",
                        format!("`{}` is the person's control in the window, and isn't run for an agent.", first.id),
                        recto_agent::exit::FAILED,
                    )
                    .then("Run the command it stands for itself, at the level it needs: `recto help` lists them."));
                }
                None => {
                    let needed = if first.id.starts_with("edit.copy") {
                        recto_agent::table::by_tag("copy").map_or(recto_agent::table::Access::At(Level::Full), |spec| spec.access)
                    } else {
                        recto_agent::table::Access::At(Level::Full)
                    };
                    if !actor.may(needed) {
                        return Err(recto_agent::refusal(&format!("palette --run for {}", first.id), actor.level(), needed)
                            .then("Run that command itself, if it has one, at the level it needs."));
                    }
                    palette::run(&inner, &first.id)
                }
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

    fn page_drawn(&self, tab: u32, page: u32, failure: Option<&'static str>) {
        self.core.push(CoreCall::Page(page, failure, TabId(tab)));
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
