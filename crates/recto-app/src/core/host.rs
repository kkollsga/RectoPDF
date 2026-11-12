//! **The session the socket and the app share:** the host every command runs on, and what the page views tell the app.

use super::*;

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

    pub fn record_command(&self, level: Level, command: &Command) -> Result<Value, AgentError> {
        match command {
            Command::SessionShow {} => {
                let notice = lock(&self.notice).clone();
                let kept = kept_json(&self.saver.store, notice.as_deref().and_then(record::notices::words));
                Ok(self.session.with_session(|session| recto_agent::session_show_json(session, level, kept)))
            }
            Command::SessionSleep { tab } => {
                let id = self.session.with_session(|session| recto_agent::select_tab(session, tab))?;
                self.core.push(CoreCall::SleepTabs(vec![id], sleeper::Why::Asked));
                self.core.flush(Duration::from_secs(5));
                let (warm, list_asleep) = self
                    .session
                    .with_session(|session| (session.warm_tabs().any(|other| other == id), session.list_is_asleep(id)));
                Ok(json!({ "tab": id.0, "dormant": !warm, "list_asleep": list_asleep }))
            }
            Command::SessionSave {} => {
                let report = self.saver.flush(false, Duration::from_secs(5)).map_err(store_error)?;
                Ok(
                    json!({ "saved": true, "tabs": report.tabs, "edited_tabs": report.edited_tabs, "record_bytes": report.record_bytes, "journal_bytes": report.journal_bytes, "wrote_bytes": report.wrote_bytes, "total_ms": report.total_us / 1000, "total_us": report.total_us, "write_us": report.write_us, "force_us": report.force_us, "verify_us": report.verify_us }),
                )
            }
            Command::SessionRestore {} => {
                if level < Level::Full && self.session.with_session(|session| session.holds_owner_edits()) {
                    return Err(recto_agent::restore_would_lose_owners_edits());
                }
                let loaded = self.saver.store.load();
                let notice = notice_for(&loaded).and_then(record::notices::words);
                let restored = matches!(loaded, Loaded::Restored(_));
                self.core.push(CoreCall::Restored(Box::new(loaded)));
                self.core.flush(Duration::from_secs(5));
                let (count, active) = self.session.with_session(|session| (session.tabs().len(), session.active().map(|id| id.0)));
                Ok(json!({ "restored": restored, "count": count, "active": active, "notice": notice }))
            }
            _ => Err(recto_agent::qa_needs_app()),
        }
    }
}

impl Host for AppHost {
    fn run(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        let asks_only = matches!(
            command,
            Command::QaRead { .. }
                | Command::QaDrive { .. }
                | Command::SessionCheck {}
                | Command::SessionShow {}
                | Command::SessionSave {}
                | Command::UpdateStatus {}
                | Command::UpdateDevBuilds { set: None }
                | Command::Settings { auto_update: None }
        );
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
        if matches!(
            command,
            Command::SessionSleep { .. } | Command::SessionShow {} | Command::SessionSave {} | Command::SessionRestore {}
        ) {
            let result = self.record_command(level, command);
            if !asks_only {
                self.running.fetch_sub(1, Ordering::SeqCst);
            }
            return result;
        }
        let result = self.run_counted(level, command, events);
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
    pub fn run_counted(&self, level: Level, command: &Command, events: &mut Vec<Event>) -> Result<Value, AgentError> {
        // The level was checked by the server against the row, and again by `execute`. QA doesn't reach `execute`, which would say it needs an app.
        if matches!(command, Command::QaRead { .. } | Command::QaDrive { .. }) {
            return match &self.qa {
                Some(qa) => qa.run(command),
                None => Err(recto_agent::qa_needs_app()),
            };
        }
        // Zoom is about what's on the screen, so it's the page view's. The rest of the command is the session's.
        if let Command::View {
            paper,
            sidebar,
            agent_icon,
            zoom: Some(word),
            layout,
        } = command
        {
            let zoom = menus::zoom_from_word(word).ok_or_else(|| {
                AgentError::new(
                    "usage",
                    format!(
                        "`{}` isn't a zoom. It's `in`, `out`, `fit`, `actual` or a number.",
                        recto_core::clean_line(word)
                    ),
                    recto_agent::exit::USAGE,
                )
            })?;
            let rest = Command::View {
                paper: paper.clone(),
                sidebar: sidebar.clone(),
                agent_icon: agent_icon.clone(),
                zoom: None,
                layout: layout.clone(),
            };
            let mut answer = self.session.run(level, &rest, events)?;
            let inner = self.saver.inner();
            let reading = inner
                .as_ref()
                .map_or_else(|| self.engine.clone(), |inner| inner.panes.reading_engine(&inner.engine));
            let value = inner
                .as_ref()
                .map_or_else(|| self.engine.zoom(zoom), |inner| inner.panes.zoom_all(&inner.engine, zoom))
                .ok_or_else(|| AgentError::new("no_active_tab", "There's no page to zoom.", recto_agent::exit::NOT_FOUND))?;
            answer["zoom"] = json!(value);
            if let Some(fit) = reading.fit() {
                answer["zoom_level"] = json!(menus::zoom_label(value, fit).text);
            }
            self.core.push(CoreCall::Reconcile);
            return Ok(answer);
        }
        if let Command::Palette { query, run, limit } = command {
            if !recto_agent::allowed(level, command.access()) {
                return Err(recto_agent::refusal(recto_agent::table::display_name(command.tag()), level, command.access()));
            }
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
                recent: recto_agent::table::by_tag("recent").is_some_and(|spec| recto_agent::allowed(level, spec.access)),
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
                        self.run_counted(level, &direct, events)?;
                        HostAction::None
                    }
                    None => {
                        let needed = if first.id.starts_with("edit.copy") {
                            recto_agent::table::by_tag("copy").map_or(recto_agent::table::Access::At(Level::Full), |spec| spec.access)
                        } else {
                            recto_agent::table::Access::At(Level::Full)
                        };
                        if !recto_agent::allowed(level, needed) {
                            return Err(recto_agent::refusal(&format!("palette --run for {}", first.id), level, needed)
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
            return Ok(answer);
        }
        if matches!(
            command,
            Command::UpdateStatus {} | Command::UpdateCheck {} | Command::UpdateInstall { .. } | Command::UpdateDevBuilds { .. } | Command::Settings { .. }
        ) {
            let inner = self.saver.inner().ok_or_else(recto_agent::update_needs_app)?;
            if !recto_agent::allowed(level, command.access()) {
                return Err(recto_agent::refusal(recto_agent::table::display_name(command.tag()), level, command.access()));
            }
            return inner.update_command(command);
        }
        if matches!(command, Command::SessionCheck {}) {
            let check = hand_off_check(self);
            let busy: Vec<Value> = check.busy.iter().map(|item| json!({ "code": item.code, "message": item.message })).collect();
            return Ok(json!({ "safe": check.safe, "busy": busy, "seconds_since_input": check.seconds_since_input }));
        }
        let result = self.session.run(level, command, events);
        // `recto find --next` went to a hit: the page view selects it, as ⌘G's does.
        if let (Command::Find { .. }, Ok(answer)) = (command, &result)
            && let Some(went) = answer.get("went_to").filter(|went| went.is_object())
            && let (Some(tab), Some(page), Some(start), Some(end)) = (went["tab"].as_u64(), went["page"].as_u64(), went["start"].as_u64(), went["end"].as_u64())
        {
            let hit = recto_agent::find::Hit {
                page: (page as u32).saturating_sub(1),
                start: start as u32,
                end: end as u32,
                before: "".into(),
                matched: "".into(),
                after: "".into(),
                hidden: false,
            };
            self.core.push(CoreCall::Find(find::FindCall::Reveal(tab as u32, hit)));
        }
        self.core.push(CoreCall::Reconcile);
        result
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
