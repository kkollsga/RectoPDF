//! **The session thread:** what it does with each call, the state's coming in, and the files being read off it.

use super::*;

/// Puts the session as what was read says, and lets everything that was waiting for that go on: the window, the socket, files named at launch.
/// Says what the app's life comes to, for the caller to carry out once the host can be told.
pub fn apply_restored(
    inner: &Arc<Inner>,
    loaded: Loaded,
    synced: &mut Synced,
    reads: &mut FileReads,
    deferred: &mut Vec<CoreCall>,
    appearance_then: Option<record::Appearance>,
) -> Option<crate::lifecycle::Out> {
    let source: Arc<dyn HistorySource> = Arc::clone(&inner.saver.store) as Arc<dyn HistorySource>;
    inner.host.session.with_session(|session| {
        session.journals_mut().set_history(source);
        session.dormancy_mut().set_sleep_files(true);
    });
    // This goes for `session restore` too: nothing drawn stays from before.
    if synced.panes.iter_mut().any(|pane| pane.tab.take().is_some()) {
        inner.engine.deactivate();
        inner.panes.set_count(
            &inner.engine,
            1,
            synced.paper.unwrap_or(PaperKind::White),
            !lock(&inner.window.app).window_closed,
        );
        synced.count = 1;
        synced.panes.truncate(1);
    }
    let mut notice = notice_for(&loaded);
    let host = &inner.host;
    let mut holding = None;
    let holds_the_record = inner.saver.store.holds_the_record();
    host.session.with_session(|session| session.journals_mut().set_journaling(holds_the_record));
    let first_time = !inner.life.state_read();
    let other_instance = matches!(loaded, Loaded::OtherInstance);
    // A late keep's restore: the window keeps its own until it's made again from what was saved (`keeping::window_as_it_is`).
    let real_window = inner.life.keeping().restoring().then(|| lock(&inner.window.app).clone());
    match loaded {
        Loaded::Restored(restored) => {
            let restored = *restored;
            let application = host.session.with_session(|session| session.restore(&restored.record, restored.journals));
            // It's been through every check, so this isn't expected, and it isn't to be passed over.
            if application.is_err() {
                notice = Some(record::notices::DAMAGED);
            }
            if application.is_ok() {
                let saved = &restored.record.app;
                if let Some(paper_set) = saved.sidebar_width {
                    lock(&inner.window.app).sidebar_width = record::sidebar_width(paper_set);
                }
                inner.window.panel_open.store(saved.panel_open, Ordering::SeqCst);
                {
                    let mut app = lock(&inner.window.app);
                    app.settings_window = saved.settings_window;
                    app.window_closed = updates::out_of_sight_when_restored(saved.window_closed, app.window_decided.then_some(app.window_closed));
                    // Once the host's shown or hidden things without the state, what it did is how things are, and the record says that, not what was saved.
                    app.app_hidden = updates::out_of_sight_when_restored(saved.app_hidden, app.hidden_decided.then_some(app.app_hidden));
                    let minimised_now = app.minimised_decided.then(|| app.window.as_ref().is_some_and(|window| window.minimised));
                    app.window = saved.window.clone();
                    if let Some(window) = app.window.as_mut() {
                        window.minimised = updates::out_of_sight_when_restored(window.minimised, minimised_now);
                    }
                    // A window that's up and taking input isn't made again from a state that comes now, so what it shows stays, its appearance with it: the host
                    // applies the state's appearance as it hears it (`restoredLate`), and the record says what the window shows. A late keep's window goes by
                    // `keeping::window_as_it_is`.
                    let shown_as_it_is = app.window_decided && real_window.is_none() && inner.life.takes_input();
                    if !shown_as_it_is {
                        app.appearance = saved.appearance;
                    }
                    app.focus = saved.focus;
                    let active = restored.record.active.map(TabId);
                    app.contents_resume = active.zip(saved.contents_scroll);
                    app.selection_resume = active.and_then(|id| {
                        restored
                            .record
                            .tabs
                            .iter()
                            .find(|tab| tab.id == id.0)?
                            .selection
                            .map(|selection| (id, selection))
                    });
                    app.contents_y = saved.contents_scroll.map(|scroll| scroll.y);
                    // Taken in with a window already up without it: the window made again from it goes by what it said.
                    if app.window_decided {
                        let state = AppState {
                            from_state: None,
                            ..app.clone()
                        };
                        app.from_state = Some(Box::new(state));
                    }
                    if let Some(real) = &real_window {
                        keeping::window_as_it_is(&mut app, real, appearance_then);
                    }
                }
                // A code this build doesn't know has no words, and goes.
                *lock(&host.notice) = saved.notice.clone().filter(|code| record::notices::words(code).is_some());
                holding = host.session.with_session(|session| {
                    session
                        .tabs()
                        .active()
                        .and_then(|id| Some((id, session.tabs().path_of(id)?, session.tabs().own_stamp(id))))
                });
            }
        }
        Loaded::Nothing if first_time => migrate_older_settings(inner),
        _ => {}
    }
    let (say_notice, remake) = keeping::taken_in(inner, other_instance, first_time, reads);
    if !say_notice {
        notice = None;
    }
    if let Some(words) = notice {
        *lock(&host.notice) = Some(words.to_string());
    }
    if let Some((id, path, stamp)) = holding {
        // With a sleep file, the first frame has what it needs. Nothing waits for the PDF, and it's looked at on the way past.
        if inner.host.session.with_session(|session| session.sleeper().wake_from_sleep_file(id)) {
            inner.host.session.with_session(|session| session.wake_active());
            inner.engine.waking().note_wake(id.0, Instant::now());
        } else {
            *lock(&inner.life.holding_tab) = Some(id);
            reads.wanted_tab = Some(id);
            reads.start_restored(inner, id, path, stamp);
        }
        look_at_other_files(inner, id);
    }
    inner.saver.start();
    let ready = inner.life.step(crate::lifecycle::Event::StateRead).or(remake);
    if first_time {
        start_serving(inner);
    }
    // What waited for the state: at the launch, and what the person did while a late keep's restore was read.
    release(inner, deferred);
    ready
}

/// What the person did while a late keep's restore was read, let go once it's no longer restoring without the restore coming in (its patience ran out, or its
/// thread ended): it runs on the window as it is. Not before the launch's state is read, when what waits is an open, which waits for that.
fn release_held(inner: &Arc<Inner>, deferred: &mut Vec<CoreCall>) {
    if deferred.is_empty() || !inner.life.state_read() || inner.life.keeping().restoring() {
        return;
    }
    release(inner, deferred);
}

/// **What waited, let go:** each call back on the queue, in the order it came; a person's command that runs on a thread of its own sent there again.
fn release(inner: &Arc<Inner>, deferred: &mut Vec<CoreCall>) {
    for call in deferred.drain(..) {
        match call {
            CoreCall::Run(run) if person::on_a_thread_of_its_own(&run) => person::submit(inner, run.command),
            call => inner.core.push(call),
        }
    }
}

/// **Whether a call waits, and holds it if it does:** an open, for the launch's state, as what's opened goes over what comes back; and anything the person
/// does, for a late keep's restore, so what they ask for last lands on the restored session (a command, Find's step on the page, and a command that runs on a
/// thread of its own, held when it was asked for: see `person::submit`). A command of the last kind that's no longer held goes to its own thread.
fn hold(inner: &Arc<Inner>, call: CoreCall, deferred: &mut Vec<CoreCall>) -> Option<CoreCall> {
    if waits(&call, inner.life.state_read(), inner.life.keeping().restoring()) {
        deferred.push(call);
        return None;
    }
    match call {
        CoreCall::Run(run) if person::on_a_thread_of_its_own(&run) => {
            person::submit(inner, run.command);
            None
        }
        call => Some(call),
    }
}

/// Whether a call waits now ([`hold`]), by whether the launch's state has been read and whether a late keep's saved session is being read.
fn waits(call: &CoreCall, state_read: bool, restoring: bool) -> bool {
    match call {
        CoreCall::Run(run) => (!state_read && opening::is_open(&run.command)) || restoring,
        CoreCall::Find(find) => find.is_user_action() && restoring,
        _ => false,
    }
}

/// The two small files that held the paper and the agent icon before there was a record. Read once, and they go when the record's written.
pub fn migrate_older_settings(inner: &Inner) {
    let Some(paths) = &inner.serving.paths else { return };
    let paper = recto_platform::local::read_private_file(&paths.root.join(PAPER_FILE), 16)
        .ok()
        .flatten()
        .and_then(|bytes| std::str::from_utf8(&bytes).ok().and_then(Paper::from_name));
    let hidden = paths.root.join(AGENT_ICON_FILE).exists();
    if paper.is_none() && !hidden {
        return;
    }
    inner.host.session.with_session(|session| {
        let view = session.viewing().view();
        session.viewing_mut().set_view(ViewSettings {
            paper: paper.or(view.paper),
            agent_icon_hidden: hidden,
            ..view
        });
    });
    inner.saver.write_soon();
    let _ = std::fs::remove_file(paths.root.join(PAPER_FILE));
    let _ = std::fs::remove_file(paths.root.join(AGENT_ICON_FILE));
}

/// One pass by one thread over every other tab's file, once after a restore. A stat each, or a lookup by number if that wasn't it. It's gone as soon as it's been round, with nothing kept for a file that's as it was, and it can sit in a file on a dead volume as long as that likes.
pub fn look_at_other_files(inner: &Arc<Inner>, active: TabId) {
    // Counted, so a test can wait for it to have been round, and not for a time.
    inner.life.others_looking.fetch_add(1, Ordering::SeqCst);
    let looking = inner;
    let inner = Arc::clone(inner);
    let spawned = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
        let files = inner.host.session.with_session(|session| session.files_to_look_at());
        let mut changed = false;
        for (id, path, stamp, says_something) in files {
            if id == active {
                continue;
            }
            // A file as it was, of a tab that says nothing of it, is nothing new: no lock taken for it.
            let status = look_at_file(&path, stamp);
            if sleeper::look_is_news(&status, stamp, says_something) {
                changed |= take_other_look(&inner, id, stamp, status);
            }
        }
        if changed {
            inner.core.push(CoreCall::Reconcile);
        }
        inner.life.others_looking.fetch_sub(1, Ordering::SeqCst);
    });
    if spawned.is_err() {
        looking.life.others_looking.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What a look at a tab not showing found, made against `stamp`, taken in: whether it was news. Only something other than what was known is told, and a save
/// meanwhile moves the stamp, so a look against the old one is about nothing now.
pub(crate) fn take_other_look(inner: &Inner, id: TabId, stamp: Option<recto_platform::FileStamp>, status: FileStatus) -> bool {
    inner.host.session.with_session(|session| {
        sleeper::look_is_news(&status, stamp, session.notices().said(id).is_some())
            && session.tabs().own_stamp(id) == stamp
            && session.apply_file_status(id, status)
    })
}

/// Runs the socket from here on, now the session's as it was: nothing asked of it could be about an empty one, or open over what's coming back.
pub fn start_serving(inner: &Arc<Inner>) {
    if !inner.serving.socket {
        return;
    }
    let waiting = lock(&inner.serving.waiting_server).take();
    let (ending, life) = (Arc::clone(&inner.listener), Arc::clone(&inner.life));
    let served = Arc::clone(&inner.host);
    let inner = Arc::clone(inner);
    // Blocked in `poll` with nothing happening, as `recto serve` is.
    let _ = std::thread::Builder::new().name("recto-control".into()).stack_size(512 * 1024).spawn(move || {
        // The socket wasn't free at the start. If that's the app this one takes over from, on its way out, it soon will be, and an update mustn't lose the socket to that. If it's another session it stays that one's, and this thread goes.
        let began = Instant::now();
        let mut server = waiting;
        while server.is_none() && began.elapsed() < SOCKET_PATIENCE {
            std::thread::sleep(Duration::from_millis(20));
            server = inner.serving.paths.as_ref().and_then(|paths| Server::bind(paths).ok());
            if let Some(bound) = &server {
                *lock(&inner.serving.stopper) = bound.stopper().ok();
                *lock(&inner.serving.announcer) = bound.announcer().ok();
            }
        }
        let Some(server) = server else { return };
        drop(inner);
        recto_platform::slow::pause(recto_platform::slow::Seam::Socket);
        let _ = server.run(served, Limits::default());
        // Told once, by the machine, however the server was stopped.
        if life.step(crate::lifecycle::Event::SocketEnded) == Some(crate::lifecycle::Out::SessionEnded) {
            ending.session_ended();
        }
    });
}

pub fn session_thread(inner: &Arc<Inner>) {
    let mut synced = Synced::default();
    let mut reads = FileReads::default();
    let mut deferred: Vec<CoreCall> = Vec::new();
    loop {
        // Parked for the queue and the state's one deadline, and nothing else.
        let Some(call) = inner.core.next_or_due(|| {
            [
                inner.saver.wait_for_due(),
                inner.sleeper.wait_for_due(),
                inner.life.due(),
                inner.life.keeping_due(),
            ]
            .into_iter()
            .flatten()
            .min()
        }) else {
            if inner.core.has_ended() {
                return;
            }
            recto_platform::slow::pause(recto_platform::slow::Seam::Due);
            // A launch an update may have opened has waited for the state as long as it does.
            if inner.life.due().is_some_and(|wait| wait.is_zero()) {
                restore_patience_over(inner);
            }
            keeping::when_due(inner, &reads);
            release_held(inner, &mut deferred);
            if inner.saver.wait_for_due().is_some_and(|wait| wait.is_zero()) {
                inner.saver.spawn_writer();
            }
            if inner.sleeper.wait_for_due().is_some_and(|wait| wait.is_zero())
                && let Some(due) = inner.sleeper.dormancy_due(
                    &inner.host.session,
                    inner.life.reads_running.load(Ordering::SeqCst) > 0 || inner.core.frozen.load(Ordering::SeqCst),
                )
            {
                inner.go_dormant(&due, sleeper::Why::LeftAlone);
                // With what's gone, the page view lets go of it too, and the next deadline's worked out.
                inner.core.push(CoreCall::Reconcile);
            }
            continue;
        };
        let Some(call) = hold(inner, call, &mut deferred) else { continue };
        let mut events = Vec::new();
        let host = &inner.host;
        match call {
            CoreCall::Reconcile => {}
            CoreCall::LookAtShown => look_at_shown(inner),
            CoreCall::Unlocked(ask, loaded, stamp, password) => unlocked(inner, ask, loaded, stamp, password, &mut events),
            CoreCall::Restored(loaded) => {
                let ready = apply_restored(inner, *loaded, &mut synced, &mut reads, &mut deferred, None);
                // What every call ends with, and before the host's told, as it makes the window from what's published now: Find's field among it, or a field that had the keyboard comes back without it (CI, run 43062872948).
                after_call(inner, &mut synced, &mut reads, events);
                // The first time only: the host's told, late or not.
                inner.carry_out(ready);
                recto_platform::slow::pause(recto_platform::slow::Seam::Session);
                continue;
            }
            // A late try's saved session: taken in as the state is at a launch, while it's still restoring, and dropped after its patience.
            CoreCall::LateRead(read) => {
                let Some(loaded) = keeping::late_read(inner, read) else {
                    release_held(inner, &mut deferred);
                    after_call(inner, &mut synced, &mut reads, events);
                    continue;
                };
                let ready = apply_restored(inner, loaded.0, &mut synced, &mut reads, &mut deferred, Some(loaded.1));
                after_call(inner, &mut synced, &mut reads, events);
                inner.carry_out(ready);
                continue;
            }
            CoreCall::RestorePatienceOver => restore_patience_over(inner),
            CoreCall::Read(read) => {
                // A person's command waiting for this read runs again now, in this call, as the window's own tap and open did.
                if let Some(run) = reads.finished(inner, *read) {
                    let _ = person::run(inner, &mut reads, run, &mut events);
                }
            }
            CoreCall::SleepTabs(tabs, why) => {
                inner.go_dormant(&tabs, why);
            }
            CoreCall::MemoryPressure(critical) => inner.memory_pressure(critical),
            CoreCall::WokenFileIsOther(id, status, against) => woken_file_is_other(inner, &mut reads, id, status, against),
            CoreCall::LookAgain(id) => inner.sleeper.check_woken_file(&inner.host.session, &inner.core, id),
            CoreCall::FileLength(id, pages, version) => file_length(inner, &mut reads, id, pages, version),
            CoreCall::Events(list) => events.extend(list),
            CoreCall::GeneratedFinished => generated_contents::look_again(inner),
            CoreCall::GeneratedView(id, view, version) => host.session.with_session(|session| session.set_generated_view_for(id, view, version)),
            CoreCall::Find(call) => find::act(inner, call, &mut events),
            CoreCall::TryBack(back) => keeping::try_back(inner, &reads, back),
            CoreCall::Run(run) => {
                let _ = person::run(inner, &mut reads, *run, &mut events);
            }
            // What was asked before this includes any file being read for it: let go at the end of this call if nothing's being read, or once the last read's done.
            CoreCall::Flush(marker) => reads.flushes.push(marker),
            CoreCall::Page(page, failure, tab) => {
                host.session
                    .with_session(|session| session.mail_mut().note_drawn(DrawNote { tab, page, failure }));
                events.push(match failure {
                    None => Event::PageDrawn { tab, page: page + 1 },
                    Some(code) => Event::PageFailed { tab, page: page + 1, code },
                });
                // Nothing the sidebar or the reader shows comes from this. Going round for it would rebuild the model per tile drawn.
                if let Some(announcer) = lock(&inner.serving.announcer).as_ref() {
                    announcer.announce(events);
                }
                continue;
            }
        }
        after_call(inner, &mut synced, &mut reads, events);
    }
}

/// **What every call on the session thread ends with,** the state's coming in too (review 1: it went on without the events it made being announced, or a flush that waited on a read being told it was done): generated contents and Find kept up with the session, the host's view made from it, flushes with nothing left to read let go, and what happened announced.
pub fn after_call(inner: &Arc<Inner>, synced: &mut Synced, reads: &mut FileReads, mut events: Vec<Event>) {
    keeping::look_after_call(inner, reads);
    generated_contents::look_if_chosen(inner);
    find::keep_up(inner);
    reconcile(inner, synced, &mut events);
    if reads.running.is_empty() {
        for marker in reads.flushes.drain(..) {
            marker.finished();
        }
    }
    #[cfg(any(test, feature = "testkit"))]
    if let Some(heard) = lock(&inner.serving.heard).events.as_mut() {
        heard.extend(events.iter().cloned());
    }
    if !events.is_empty()
        && let Some(announcer) = lock(&inner.serving.announcer).as_ref()
    {
        announcer.announce(events);
    }
}

/// Reads a file's facts off the session thread: with the password the person typed for it, if the keyring has one for that file, so a protected tab that went cold and wakes doesn't ask again; and otherwise as any file is. Whether a password was used.
pub fn read_facts(keys: &recto_reader::engine::Keyring, path: &std::path::Path) -> (Result<DocFacts, OpenError>, bool) {
    match keys.for_path(path) {
        Some(key) => (recto_core::inspect_with_password(path, &key), true),
        None => (DrawableLoader.load(path), false),
    }
}

/// Reads of files under way off the session thread. A file on a dead volume holds up the one thread reading it, and neither this one nor anyone else's tap.
#[derive(Default)]
pub struct FileReads {
    pub running: HashSet<Reading>,
    /// The tab the reader last asked for that's being read. Reading it isn't asking again: they may have gone elsewhere by the time it's done.
    pub wanted_tab: Option<TabId>,
    pub flushes: Vec<Arc<Flushed>>,
    /// The last file being opened, so several opened together keep their order unless one isn't answering.
    pub last_open: Option<Arc<Flushed>>,
    /// The person's command each read is for, which runs again once it's in: the last asked for each.
    pub then: HashMap<Reading, Box<recto_agent::Command>>,
}

/// What's said when there are more files not answering than can be waited on.
pub const TOO_MANY_READS: &str = "Several files aren't answering, so this one wasn't opened. Their disk may have gone to sleep or been disconnected.";

/// How long a file being opened waits for the one opened before it, so tabs come in the order asked for, unless that one isn't answering.
pub const OPEN_ORDER_WAIT: Duration = Duration::from_secs(1);

impl FileReads {
    /// Reads a restored active tab's file: what's become of it first, as it may be somewhere else now. On a thread of its own, as any other is.
    pub fn start_restored(&mut self, inner: &Arc<Inner>, id: TabId, path: PathBuf, stamp: Option<FileStamp>) {
        let what = Reading::Restored(id);
        if !self.running.insert(what.clone()) {
            return;
        }
        inner.life.reads_running.fetch_add(1, Ordering::SeqCst);
        let core = Arc::clone(&inner.core);
        let keys = Arc::clone(&inner.keys);
        let spawned = std::thread::Builder::new().name("recto-read".into()).stack_size(1024 * 1024).spawn(move || {
            let status = look_at_file(&path, stamp);
            let path = if let FileStatus::Moved(moved, _) = &status { moved.clone() } else { path };
            let stamp = recto_platform::file_stamp(&path).ok();
            let (loaded, unlocked) = read_facts(&keys, &path);
            core.push(CoreCall::Read(Box::new(FileRead {
                what,
                loaded,
                stamp,
                status: Some(status),
                history: None,
                unlocked,
            })));
        });
        if spawned.is_err() {
            self.running.remove(&Reading::Restored(id));
            inner.life.reads_running.fetch_sub(1, Ordering::SeqCst);
            *lock(&inner.life.holding_tab) = None;
            inner.host.session.with_session(|session| session.wake_active());
        }
    }

    pub fn start(&mut self, inner: &Arc<Inner>, what: Reading, path: PathBuf) {
        if self.running.contains(&what) {
            return;
        }
        if self.running.len() >= MAX_FILE_READS {
            *lock(&inner.window.open_failure) = Some(TOO_MANY_READS.to_string());
            return;
        }
        let core = Arc::clone(&inner.core);
        let (before, done) = if matches!(what, Reading::File(_)) {
            let done = Arc::new(Flushed::default());
            (self.last_open.replace(Arc::clone(&done)), Some(done))
        } else {
            (None, None)
        };
        let reading = what.clone();
        let saver = Arc::clone(&inner.saver);
        let inner_for_read = Arc::clone(inner);
        let spawned = std::thread::Builder::new().name("recto-read".into()).stack_size(1024 * 1024).spawn(move || {
            while inner_for_read.life.qa_hold_reads.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let stamp = recto_platform::file_stamp(&path).ok();
            let (loaded, unlocked) = read_facts(&inner_for_read.keys, &path);
            // A PDF already open by another name keeps what it has. The session says for sure when this gets there.
            let already =
                matches!(reading, Reading::File(_)) && inner_for_read.host.session.with_session(|session| session.tab_for_file(&path, stamp).is_some());
            // Only after a good read: taking it up marks it as open, and a file that couldn't be read won't have a tab to hold it.
            let history = if matches!(reading, Reading::File(_)) && !already && loaded.is_ok() {
                saver.store.history_of(stamp, &path)
            } else {
                None
            };
            if let Some(before) = before {
                before.wait(OPEN_ORDER_WAIT);
            }
            core.push(CoreCall::Read(Box::new(FileRead {
                what: reading,
                loaded,
                stamp,
                status: None,
                history,
                unlocked,
            })));
            if let Some(done) = done {
                done.finished();
            }
        });
        if spawned.is_ok() {
            self.running.insert(what);
            inner.life.reads_running.fetch_add(1, Ordering::SeqCst);
        } else {
            *lock(&inner.window.open_failure) = Some(TOO_MANY_READS.to_string());
        }
    }

    /// A read that couldn't start (past the cap, which says so itself, or no thread) leaves no command waiting for it, which nothing would ever run.
    pub fn forget_unless_running(&mut self, what: &Reading) {
        if !self.running.contains(what) {
            self.then.remove(what);
        }
    }

    /// Takes a read in, and says the person's command it was for, if one's to run again now.
    pub fn finished(&mut self, inner: &Arc<Inner>, read: FileRead) -> Option<crate::queue::PersonCommand> {
        self.running.remove(&read.what);
        inner.life.reads_running.fetch_sub(1, Ordering::SeqCst);
        let host = &inner.host;
        // Read with a password: the tab's marked before anything read takes its place, so no agent can see it.
        if read.unlocked
            && let Reading::Restored(tab) | Reading::Tab(tab) = &read.what
        {
            host.session.with_session(|session| session.tabs_mut().note_password_used(*tab));
        }
        match read.what {
            Reading::Restored(tab) => {
                if let Some(status) = read.status {
                    host.session.with_session(|session| session.apply_file_status(tab, status));
                }
                host.session.with_session(|session| session.sleeper().give_woken(tab, read.loaded, read.stamp));
                // The tab it's for may have been left already.
                if lock(&inner.life.holding_tab).take().is_some() || self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                    host.session.with_session(|session| {
                        if session.tabs().active() == Some(tab) {
                            session.wake_active()
                        }
                    });
                }
            }
            Reading::Tab(tab) => {
                let then = self.then.remove(&Reading::Tab(tab));
                // Only if it's still what's wanted: the tab last asked for wins, a ⌘-click's as a tap's. If not, there's no telling how long till it's used, and it'll be read fresh when it's asked for.
                if let Some(command) = then
                    && self.wanted_tab == Some(tab)
                {
                    self.wanted_tab = None;
                    host.session.with_session(|session| session.sleeper().give_woken(tab, read.loaded, read.stamp));
                    return Some(person::read_in(*command, None));
                }
            }
            Reading::File(path) => {
                let command = self.then.remove(&Reading::File(path.clone()))?;
                let fetched = recto_agent::later::Fetched {
                    stamp: read.stamp,
                    facts: read.loaded,
                    history: read.history,
                    unlocked: read.unlocked,
                };
                return Some(person::read_in(*command, Some((path, fetched))));
            }
        }
        None
    }
}

/// `CoreCall::RestorePatienceOver`, on the session thread.
pub fn restore_patience_over(inner: &Arc<Inner>) {
    let host = &inner.host;
    // The state hasn't come. The window goes up without it, and when it does come it's taken in; or a launch an update may have opened waits on, to the session
    // thread's own deadline ([`crate::lifecycle::Life::due`]).
    if !inner.life.state_read() {
        let may_be_reopened_by_update = inner.updates.may_be_reopened_by_update();
        inner.carry_out(inner.life.step(crate::lifecycle::Event::PatienceOver { may_be_reopened_by_update }));
    }
    // A tab's file not answering: on to what's on the record. The page says it isn't answering.
    if lock(&inner.life.holding_tab).take().is_some() {
        host.session.with_session(|session| session.wake_active());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (review, 2026-11-13, S5): while a late keep's saved session is read, Find's step on the page (an agent's `find`, shown as the person's) wasn't
    /// held as the person's commands are, so it landed on the window before the restore, and the restore replaced it; or something that isn't the person's
    /// doing (the Finder's news, a read coming in) is held and waits on the restore. What the person does waits while it restores, and only that.
    #[test]
    fn what_the_person_does_waits_for_a_late_keeps_restore_and_nothing_else() {
        let tab = TabId(1);
        let hit = recto_agent::find::Hit {
            page: 0,
            start: 0,
            end: 1,
            before: "".into(),
            matched: "".into(),
            after: "".into(),
            hidden: false,
        };
        let step = CoreCall::Find(crate::find::FindCall::Effect(recto_agent::Effect::Reveal { tab, hit }));
        let run = CoreCall::Run(crate::queue::PersonCommand::input(
            recto_agent::parse(r#"{"cmd":"view","paper":"dark"}"#).expect("a command"),
        ));
        let news = CoreCall::Find(crate::find::FindCall::News);
        for (call, while_restoring) in [(&step, true), (&run, true), (&news, false), (&CoreCall::Reconcile, false)] {
            assert_eq!(waits(call, true, true), while_restoring, "{call:?} while restoring");
            assert!(!waits(call, true, false), "{call:?} once it isn't");
        }
    }
}
