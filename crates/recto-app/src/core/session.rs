//! **The session thread:** what it does with each call, the state's coming in, and the files being read off it.

use super::*;

/// Puts the session as what was read says, and lets everything that was waiting for that go on: the window, the socket, files named at launch.
pub fn apply_restored(inner: &Arc<Inner>, loaded: Loaded, synced: &mut Synced, reads: &mut FileReads, deferred: &mut Vec<PathBuf>) {
    let source: Arc<dyn HistorySource> = Arc::clone(&inner.saver.store) as Arc<dyn HistorySource>;
    inner.host.session.with_session(|session| {
        session.set_history_source(source);
        session.set_sleep_files(true);
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
    host.session.with_session(|session| session.set_journaling(holds_the_record));
    let first_time = !inner.life.restored.load(Ordering::SeqCst);
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
                    app.appearance = saved.appearance;
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
                }
                // A code this build doesn't know has no words, and goes.
                *lock(&host.notice) = saved.notice.clone().filter(|code| record::notices::words(code).is_some());
                holding = host
                    .session
                    .with_session(|session| session.active().and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id)))));
            }
        }
        Loaded::Nothing if first_time => migrate_older_settings(inner),
        _ => {}
    }
    if let Some(words) = notice {
        *lock(&host.notice) = Some(words.to_string());
    }
    if let Some((id, path, stamp)) = holding {
        // With a sleep file, the first frame has what it needs. Nothing waits for the PDF, and it's looked at on the way past.
        if inner.host.session.with_session(|session| session.wake_from_sleep_file(id)) {
            inner.host.session.with_session(|session| session.wake_active());
            inner.engine.note_wake(id.0, Instant::now());
        } else {
            *lock(&inner.life.holding_tab) = Some(id);
            reads.wanted_tab = Some(id);
            reads.start_restored(inner, id, path, stamp);
        }
        look_at_other_files(inner, id);
    }
    inner.saver.start();
    inner.life.restored.store(true, Ordering::SeqCst);
    if !first_time {
        return;
    }
    start_serving(inner);
    for path in deferred.drain(..) {
        inner.core.push(CoreCall::Open(path));
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
        session.set_view(ViewSettings {
            paper: paper.or(session.view().paper),
            agent_icon_hidden: hidden,
            ..session.view()
        })
    });
    inner.saver.write_soon();
    let _ = std::fs::remove_file(paths.root.join(PAPER_FILE));
    let _ = std::fs::remove_file(paths.root.join(AGENT_ICON_FILE));
}

/// One pass by one thread over every other tab's file, once after a restore. A stat each, or a lookup by number if that wasn't it. It's gone as soon as it's been round, with nothing kept for a file that's as it was, and it can sit in a file on a dead volume as long as that likes.
pub fn look_at_other_files(inner: &Arc<Inner>, active: TabId) {
    let inner = Arc::clone(inner);
    let _ = std::thread::Builder::new().name("recto-look".into()).stack_size(256 * 1024).spawn(move || {
        let files = inner.host.session.with_session(|session| session.files_to_look_at());
        let mut changed = false;
        for (id, path, stamp) in files {
            if id == active {
                continue;
            }
            let status = look_at_file(&path, stamp);
            let is_as_it_was = matches!(&status, FileStatus::Same(now) if Some(*now) == stamp) || matches!(status, FileStatus::NotLooked);
            if !is_as_it_was {
                inner.host.session.with_session(|session| session.apply_file_status(id, status));
                changed = true;
            }
        }
        if changed {
            inner.core.push(CoreCall::Reconcile);
        }
    });
}

/// Runs the socket from here on, now the session's as it was: nothing asked of it could be about an empty one, or open over what's coming back.
pub fn start_serving(inner: &Arc<Inner>) {
    let waiting = lock(&inner.serving.waiting_server).take();
    let ending = Arc::clone(&inner.listener);
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
        ending.session_ended();
    });
}

pub fn session_thread(inner: &Arc<Inner>) {
    let mut synced = Synced::default();
    let mut reads = FileReads::default();
    let mut deferred: Vec<PathBuf> = Vec::new();
    loop {
        // Parked for the queue and the state's one deadline, and nothing else.
        let Some(call) = inner.core.next_or_due(|| match (inner.saver.wait_for_due(), inner.sleeper.wait_for_due()) {
            (Some(one), Some(other)) => Some(one.min(other)),
            (one, other) => one.or(other),
        }) else {
            if inner.saver.wait_for_due().is_some_and(|wait| wait.is_zero()) {
                inner.saver.spawn_writer();
            }
            if inner.sleeper.wait_for_due().is_some_and(|wait| wait.is_zero())
                && let Some(due) = inner.dormancy_due()
            {
                inner.go_dormant(&due, sleeper::Why::LeftAlone);
                // With what's gone, the page view lets go of it too, and the next deadline's worked out.
                inner.core.push(CoreCall::Reconcile);
            }
            continue;
        };
        let mut events = Vec::new();
        let host = &inner.host;
        match call {
            CoreCall::Reconcile => {}
            CoreCall::LookAtShown => {
                if let Some(inner) = inner.saver.inner() {
                    for id in host.session.with_session(|session| session.panes()) {
                        sleeper::check_woken_file(&inner, id);
                    }
                }
            }
            CoreCall::Unlocked(ask, loaded, stamp, password) => match loaded {
                Ok(facts) => {
                    // Kept by which file it is, so a move keeps it and a new file in its place doesn't get it. A file with no such numbers can't have its password kept for drawing it, so it's said rather than asked again for ever.
                    let identity = stamp.map(|stamp| (stamp.device, stamp.inode)).filter(|identity| *identity != (0, 0));
                    match identity {
                        None => {
                            lock(&inner.tools.prompts.passwords).failed(ask.id);
                            *lock(&inner.window.open_failure) = Some(passwords::CANT_KEEP.to_string());
                        }
                        Some(identity) => {
                            lock(&inner.tools.prompts.passwords).opened(ask.id);
                            inner.keys.set(identity, password);
                            for engine in inner.panes.engines(&inner.engine) {
                                engine.key_given(&ask.path);
                            }
                            match ask.target {
                                passwords::Target::File => match host.session.with_session(|session| session.open_unlocked(&ask.path, facts, stamp)) {
                                    Ok(id) => events.push(host.session.with_session(|session| recto_agent::event::tab_opened(session, id))),
                                    Err(error) => *lock(&inner.window.open_failure) = Some(error.message().to_string()),
                                },
                                passwords::Target::Tab(id) => host.session.with_session(|session| session.unlock(id, facts, stamp)),
                            }
                        }
                    }
                }
                Err(OpenError::PasswordNeeded) => {
                    if lock(&inner.tools.prompts.passwords).wrong(ask.id) == passwords::Wrong::Stopped {
                        *lock(&inner.window.action_notice) = Some(passwords::stopped_asking());
                    }
                }
                Err(error) => {
                    lock(&inner.tools.prompts.passwords).failed(ask.id);
                    *lock(&inner.window.open_failure) = Some(error.message().to_string());
                }
            },
            CoreCall::ClearRecent => {
                host.session.with_session(|session| session.clear_recent());
                inner.saver.write_soon();
            }
            CoreCall::Restored(loaded) => {
                let first_time = !inner.life.restored.load(Ordering::SeqCst);
                apply_restored(inner, *loaded, &mut synced, &mut reads, &mut deferred);
                // What every call ends with, and before the host's told, as it makes the window from what's published now: Find's field among it, or a field that had the keyboard comes back without it (CI, run 43062872948).
                after_call(inner, &mut synced, &mut reads, events);
                if first_time {
                    // A test's slow window: the window that went up without the state is made again from it this much later, so what's done in between can be seen to wait for it.
                    let listener = Arc::clone(&inner.listener);
                    if inner.life.shown_without_state.load(Ordering::SeqCst) {
                        recto_platform::slow::later(recto_platform::slow::Seam::Remake, move || listener.restore_ready());
                    } else {
                        listener.restore_ready();
                    }
                }
                recto_platform::slow::pause(recto_platform::slow::Seam::Session);
                continue;
            }
            CoreCall::RestorePatienceOver => {
                if !inner.life.restored.load(Ordering::SeqCst) {
                    match updates::restore_patience(inner.updates.may_be_reopened_by_update(), inner.life.started.elapsed()) {
                        // The state hasn't come. The window goes up without it, and when it does come it's taken in.
                        updates::Patience::ShowNow => updates::show_without_state(&inner.life.shown_without_state, &*inner.listener),
                        updates::Patience::WaitFor(left) => updates::look_again_after(inner, left),
                    }
                }
                // A tab's file not answering: on to what's on the record. The page says it isn't answering.
                if lock(&inner.life.holding_tab).take().is_some() {
                    host.session.with_session(|session| session.wake_active());
                }
            }
            CoreCall::Open(path) if !inner.life.restored.load(Ordering::SeqCst) => {
                deferred.push(path);
                continue;
            }
            CoreCall::Tap(id) => {
                inner.engine.note_wake(id.0, Instant::now());
                // A cold tab's file is read on a thread of its own, as an `open` is, so a file on a dead volume holds up neither this thread nor anything that asks about tabs, and a later tap goes ahead.
                match host.session.with_session(|session| session.wake_for_tap(id)) {
                    Some((tab, path)) => {
                        reads.wanted_tab = Some(tab);
                        reads.start(inner, Reading::Tab(tab), path);
                    }
                    None => {
                        reads.wanted_tab = None;
                        tap_now(inner, id, &mut events);
                    }
                }
            }
            CoreCall::Jump(id, page, within) => {
                // Where the reader is, as the page view has it now, is what Back comes back to. `jump` clamps: a contents entry can't send the reader off the end.
                inner.take_places();
                if let Ok(event) = host
                    .session
                    .with_session(|session| session.jump(id, page, within).map(|()| recto_agent::page_now(session, id)))
                {
                    events.push(event);
                }
            }
            CoreCall::JumpTyped(id, typed) => {
                inner.take_places();
                let went = host.session.with_session(|session| {
                    let page = session.page_for_typed(id, &typed)?;
                    session.jump(id, page, 0.0).ok()?;
                    Some(recto_agent::page_now(session, id))
                });
                match went {
                    Some(event) => events.push(event),
                    None => *lock(&inner.window.action_notice) = Some(linking::no_such_page(&typed)),
                }
            }
            CoreCall::Step(back) => {
                inner.take_places();
                let moved = host.session.with_session(|session| {
                    let id = session.active()?;
                    let moved = if back { session.go_back(id) } else { session.go_forward(id) };
                    moved.ok().filter(|moved| *moved).map(|_| recto_agent::page_now(session, id))
                });
                if let Some(event) = moved {
                    events.push(event);
                }
            }
            CoreCall::Open(path) => {
                *lock(&inner.window.open_failure) = None;
                // Opening a file again is what starts its password tries afresh (SPEC).
                let (tab, own) = host.session.with_session(|session| {
                    let tab = session.tab_for_path(&path);
                    (tab, tab.and_then(|id| session.path_of(id)))
                });
                lock(&inner.tools.prompts.passwords).opened_again(&[Some(path.clone()), own].into_iter().flatten().collect::<Vec<_>>(), tab);
                let cold = host
                    .session
                    .with_session(|session| session.tab_for_path(&path).and_then(|id| session.wake_for_tap(id)));
                if let Some((tab, file)) = cold {
                    reads.wanted_tab = Some(tab);
                    reads.start(inner, Reading::Tab(tab), file);
                } else if host.session.with_session(|session| session.go_to_open_file(&path)).is_some() {
                    reads.wanted_tab = None;
                } else {
                    // A link or a moved name is found once it's been looked at, on the thread reading it.
                    reads.wanted_tab = None;
                    reads.start(inner, Reading::File(path.clone()), path);
                }
            }
            CoreCall::Read(read) => reads.finished(inner, *read, &mut events),
            CoreCall::SetPaper(paper) => set_view(inner, &mut events, |view| view.paper = Some(paper)),
            CoreCall::SleepTabs(tabs, why) => {
                inner.go_dormant(&tabs, why);
            }
            CoreCall::MemoryPressure(critical) => inner.memory_pressure(critical),
            CoreCall::WokenFileIsOther(id, status) => {
                host.session.with_session(|session| {
                    session.apply_file_status(id, status);
                    session.forget_read(id);
                });
                let reread = host.session.with_session(|session| {
                    session
                        .active()
                        .filter(|active| *active == id)
                        .and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id))))
                });
                if let Some((id, path, stamp)) = reread {
                    reads.wanted_tab = Some(id);
                    reads.start_restored(inner, id, path, stamp);
                }
            }
            CoreCall::LookAgain(id) => {
                if let Some(inner) = inner.saver.inner() {
                    sleeper::check_woken_file(&inner, id);
                }
            }
            CoreCall::FileLength(id, pages, version) => {
                // What a sleep file said of it is wrong, so it's read after all.
                let wrong = host.session.with_session(|session| session.file_length_is(id, pages, version));
                let reread = host.session.with_session(|session| {
                    session
                        .active()
                        .filter(|active| wrong && *active == id)
                        .and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id))))
                });
                if let Some((id, path, stamp)) = reread {
                    reads.wanted_tab = Some(id);
                    reads.start_restored(inner, id, path, stamp);
                }
            }
            CoreCall::Events(list) => events.extend(list),
            CoreCall::CloseTab(id, discard) => close_tab(host, id, discard, &mut events),
            CoreCall::CloseAnswered(id, choice) => match choice {
                crate::saving::CloseChoice::Save => {
                    if let Some(inner) = inner.saver.inner() {
                        saving::start_save(&inner, id, None, true);
                    }
                }
                crate::saving::CloseChoice::DontSave => close_tab(host, id, true, &mut events),
                crate::saving::CloseChoice::Cancel | crate::saving::CloseChoice::SaveAs => {}
            },
            CoreCall::CloseAfterSave(id) => {
                // An edit that came in between the save and now isn't in the file: the tab stays, and says so, rather than lose it (review).
                let closed = host.session.with_session(|session| {
                    session.close_after_save(id).inspect(|closed| {
                        if let Some(closed) = closed {
                            recto_agent::event::closed(session, *closed, &mut events);
                        }
                    })
                });
                if let Ok(None) = closed {
                    *lock(&inner.window.action_notice) = Some(saving::SAVED_NOT_CLOSED.to_string());
                }
            }
            CoreCall::ClearEditHistory => inner.host.session.with_session(|session| {
                if let Some(id) = session.active() {
                    session.clear_history(id, recto_session::ClearReason::UserAsked);
                }
            }),
            CoreCall::ToggleAgentIcon => set_view(inner, &mut events, |view| view.agent_icon_hidden = !view.agent_icon_hidden),
            CoreCall::ContentsAction(id, action) => generated_contents::act(inner, id, action),
            CoreCall::GeneratedFinished => generated_contents::look_again(inner),
            CoreCall::GeneratedView(id, view, version) => host.session.with_session(|session| session.set_generated_view_for(id, view, version)),
            CoreCall::ToggleSection(id, entry) => host.session.with_session(|session| session.toggle_contents_section(id, entry)),
            CoreCall::Find(call) => find::act(inner, call, &mut events),
            CoreCall::SetEditMode(on) => host.session.with_session(|session| session.set_edit_mode(on)),
            CoreCall::SetSidebarHidden(hidden) => set_view(inner, &mut events, |view| view.sidebar_hidden = hidden),
            CoreCall::Rotate(turns) => {
                let refused = host.session.with_session(|session| match session.active() {
                    None => Some(recto_session::SessionError::NoSuchTab),
                    Some(id) => match session.rotate_from_panel(id, turns) {
                        Ok(()) => {
                            events.push(recto_agent::edits_changed(session, id));
                            None
                        }
                        Err(error) => Some(error),
                    },
                });
                // It says why, by the page, and goes by itself.
                *lock(&inner.window.action_notice) = refused.map(|error| format!("Couldn't rotate: {}", error.message()));
            }
            CoreCall::UndoOrRedo(undo) => host.session.with_session(|session| {
                if let Some(id) = session.active()
                    && (if undo {
                        session.undo_edit_as(id, recto_session::Author::Owner)
                    } else {
                        session.redo_edit_as(id, recto_session::Author::Owner)
                    })
                    .unwrap_or(false)
                {
                    events.push(recto_agent::edits_changed(session, id));
                }
            }),
            CoreCall::CompareClick(id) => {
                // The tab joining, or the pane read next if the one read leaves, is read first with the session free, as a tap's is.
                let cold = host.session.with_session(|session| session.wake_for_compare_click(id));
                match cold {
                    Some((tab, path)) => {
                        reads.wanted_tab = Some(tab);
                        reads.start(inner, Reading::Compare(id, tab), path);
                    }
                    None => compare_click_now(inner, id, &mut events),
                }
            }
            CoreCall::FocusPane(id) => {
                let cold = host.session.with_session(|session| {
                    session
                        .comparison()
                        .is_some_and(|comparison| comparison.contains(id))
                        .then(|| session.wake_for_tap(id))
                        .flatten()
                });
                match cold {
                    Some((tab, path)) => {
                        reads.wanted_tab = Some(tab);
                        reads.start(inner, Reading::Pane(tab), path);
                    }
                    None => {
                        events.extend(
                            host.session
                                .with_session(|session| session.focus_pane(id).then(|| recto_agent::event::tab_activated(session, id))),
                        );
                    }
                }
            }
            CoreCall::SetSynced(on) => {
                events.extend(host.session.with_session(|session| {
                    session
                        .set_compare_synced(on)
                        .filter(|changed| *changed)
                        .map(|_| recto_agent::event::comparison_changed(session))
                }));
            }
            CoreCall::SetSpread(tab, spread) => host.session.with_session(|session| {
                if let Some(id) = tab.filter(|tab| session.tab(*tab).is_some()).or(session.active())
                    && session.set_spread(id, spread)
                {
                    events.push(Event::LayoutChanged {
                        tab: id,
                        layout: spread.name(),
                    });
                }
            }),
            // What was asked before this includes any file being read for it: let go at the end of this call if nothing's being read, or once the last read's done.
            CoreCall::Flush(marker) => reads.flushes.push(marker),
            CoreCall::Page(page, failure, tab) => {
                host.session.with_session(|session| session.note_drawn(DrawNote { tab, page, failure }));
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
    generated_contents::look_if_chosen(inner);
    find::keep_up(inner);
    reconcile(inner, synced, &mut events);
    if reads.running.is_empty() {
        for marker in reads.flushes.drain(..) {
            marker.finished();
        }
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

    pub fn finished(&mut self, inner: &Arc<Inner>, read: FileRead, events: &mut Vec<Event>) {
        self.running.remove(&read.what);
        inner.life.reads_running.fetch_sub(1, Ordering::SeqCst);
        let host = &inner.host;
        // Read with a password: the tab's marked before anything read takes its place, so no agent can see it.
        if read.unlocked
            && let Reading::Restored(tab) | Reading::Tab(tab) | Reading::Compare(_, tab) | Reading::Pane(tab) = &read.what
        {
            host.session.with_session(|session| session.note_password_used(*tab));
        }
        match read.what {
            Reading::Restored(tab) => {
                if let Some(status) = read.status {
                    host.session.with_session(|session| session.apply_file_status(tab, status));
                }
                host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                // The tab it's for may have been left already.
                if lock(&inner.life.holding_tab).take().is_some() || self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                    host.session.with_session(|session| {
                        if session.active() == Some(tab) {
                            session.wake_active()
                        }
                    });
                }
            }
            Reading::Tab(tab) => {
                // Only if it's still what's wanted. If not, there's no telling how long till it's used, and it'll be read fresh when it's asked for.
                if self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                    host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                    tap_now(inner, tab, events);
                }
            }
            Reading::Compare(clicked, tab) => {
                host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                if self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                }
                compare_click_now(inner, clicked, events);
            }
            Reading::Pane(tab) => {
                host.session.with_session(|session| session.give_woken(tab, read.loaded, read.stamp));
                if self.wanted_tab == Some(tab) {
                    self.wanted_tab = None;
                }
                events.extend(
                    host.session
                        .with_session(|session| session.focus_pane(tab).then(|| recto_agent::event::tab_activated(session, tab))),
                );
            }
            Reading::File(path) => match read.loaded.map_err(SessionError::Open).and_then(|facts| {
                host.session.with_session(|session| {
                    if read.unlocked {
                        session.open_unlocked(&path, facts, read.stamp)
                    } else {
                        session.open_with_facts(&path, facts, read.stamp, read.history)
                    }
                })
            }) {
                Ok(id) => {
                    events.push(host.session.with_session(|session| recto_agent::event::tab_opened(session, id)));
                }
                // A protected file asks for its password, on the system's sheet, rather than only saying it needs one. After any other file asking, each with its own answer.
                Err(SessionError::Open(OpenError::PasswordNeeded)) => {
                    lock(&inner.tools.prompts.passwords).ask(passwords::Target::File, &path);
                }
                Err(error) => *lock(&inner.window.open_failure) = Some(error.message().to_string()),
            },
        }
    }
}
