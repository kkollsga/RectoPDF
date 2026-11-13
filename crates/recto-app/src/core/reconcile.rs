//! **Bringing the window up to date:** what the session thread publishes after each pass, and the steps it takes on the way.

use super::*;

/// A change to how pages are shown, as `recto view` makes it: announced only if something changed (R2.3, accident A2), as `view`'s is.
pub fn set_view(inner: &Inner, events: &mut Vec<Event>, change: impl FnOnce(&mut ViewSettings)) {
    let (was, view) = inner.host.session.with_session(|session| {
        let was = session.view();
        let mut view = was;
        change(&mut view);
        session.set_view(view);
        (was, view)
    });
    if view != was {
        events.push(recto_agent::view_changed(view));
    }
}

/// The session's word for a layout, as the page view has it.
pub fn reader_spread(spread: record::Spread) -> recto_reader::layout::Spread {
    match spread {
        record::Spread::Single => recto_reader::layout::Spread::Single,
        record::Spread::TwoUp => recto_reader::layout::Spread::TwoUp,
        record::Spread::TwoUpCover => recto_reader::layout::Spread::TwoUpCover,
    }
}

/// What a page view was last told about the tab in its pane.
#[derive(Default)]
pub struct PaneSynced {
    pub tab: Option<TabId>,
    pub page: u32,
    pub pages: u32,
    pub map: PageMap,
    pub warm: Vec<u32>,
    pub spread: Option<record::Spread>,
}

/// **Follows a link a click landed on,** by Rust's rule: a page of this document at once, as a jump; a web or email address only once the sheet's been answered; Back and Forward; and anything else never, with a notice saying why.
pub fn follow_link(inner: &Arc<Inner>, pane: usize, engine: &Engine, hit: recto_reader::links::LinkHit) {
    if inner.core.frozen.load(Ordering::SeqCst) {
        return;
    }
    let Some(tab) = inner.panes.tab_of(pane) else { return };
    match hit.target {
        recto_core::nav::LinkTarget::Page { page, within } => match engine.position_in_same_file(hit.page, page) {
            Some(position) => inner.link_to_page(tab, position, within),
            None => notice(inner, "The page this link goes to isn't in the document as it's been edited."),
        },
        recto_core::nav::LinkTarget::History { back } => inner.step_for_person(back),
        recto_core::nav::LinkTarget::Web(address) => {
            inner.tools.prompts.ask_link(&address);
            inner.core.push(CoreCall::Reconcile);
        }
        recto_core::nav::LinkTarget::Refused(why) => notice(inner, why.message()),
        recto_core::nav::LinkTarget::Nowhere => notice(inner, "This link goes nowhere in this document."),
    }
}

/// A line by the page that goes by itself, in Rust's words.
pub fn notice(inner: &Inner, words: &str) {
    *lock(&inner.window.action_notice) = Some(words.to_string());
    inner.core.push(CoreCall::Reconcile);
}

/// **Shows a tab in a pane,** or brings what's shown up to date: another tab, another list of pages, a layout, a page an agent sent it to, or the warm tabs. A tab a pane's page view hasn't held starts from its sleep file's pictures, as any wake does.
pub fn show_in_pane(inner: &Inner, engine: &Engine, synced: &mut PaneSynced, shown: &PaneSnapshot) {
    let id = shown.id;
    // Its file's being looked at: nothing goes on the page until that's known.
    let holding = *lock(&inner.life.holding_tab) == Some(id) && synced.tab.is_none();
    if holding {
        return;
    }
    if synced.tab != Some(id) || synced.pages != shown.pages || synced.map != shown.map {
        inner.queue_those_leaving();
        // Whichever way it's been asked for, the first frame's what its sleep file has, if it's got one and nothing's held of this tab.
        if !engine.holds(id.0) {
            engine.note_wake(id.0, Instant::now());
            inner.give_sleep_view(engine, id);
        }
        engine.activate(
            id.0,
            shown.pages,
            shown.page,
            shown.map.clone(),
            &shown.warm,
            shown.saved,
            reader_spread(shown.spread),
        );
        synced.spread = Some(shown.spread);
        // What was selected comes back on the tab it was on, once.
        let selection = {
            let mut app = lock(&inner.window.app);
            if app.selection_resume.as_ref().is_some_and(|(tab, _)| *tab == id) {
                app.selection_resume.take().map(|(_, selection)| selection)
            } else {
                None
            }
        };
        if let Some(selection) = selection {
            engine.restore_selection((selection.from_page, selection.from_char), (selection.to_page, selection.to_char));
        }
        synced.tab = Some(id);
        synced.pages = shown.pages;
        synced.map = shown.map.clone();
        synced.page = shown.page;
        synced.warm = shown.warm.clone();
        return;
    }
    if synced.spread != Some(shown.spread) {
        engine.set_spread(reader_spread(shown.spread));
        synced.spread = Some(shown.spread);
    }
    // A jump lands where it says, on its page and as far down it, even on the page the reader's on already, as Back to a place higher up the same page does.
    if let Some(stop) = shown.jump {
        engine.jump_within(stop.page, f64::from(stop.within));
        synced.page = shown.page;
    } else if synced.page != shown.page {
        engine.jump_to(shown.page);
        synced.page = shown.page;
    }
    // A warm tab closed or gone cold: what it held goes now, and not at the next switch.
    if synced.warm != shown.warm {
        engine.set_warm(&shown.warm);
        synced.warm = shown.warm.clone();
    }
}

/// Brings everything into line with the session, on the session thread. The place in a tab can move two ways, by the reader or by an agent, and each is passed to the other side.
pub fn reconcile(inner: &Inner, synced: &mut Synced, events: &mut Vec<Event>) {
    let nothing_held = lock(&inner.life.holding_tab).is_none();
    // A tab about to be shown in a pane that wasn't: its file's looked at first, so nothing of a version that's been replaced goes up, held or not.
    let panes_now = inner.host.session.with_session(|session| session.panes());
    for (pane, id) in panes_now.iter().enumerate() {
        if synced.panes.get(pane).and_then(|shown| shown.tab) != Some(*id) && nothing_held {
            sleeper::look_before_showing(inner, *id);
        }
    }
    let engines = inner.panes.engines(&inner.engine);
    let moved: Vec<(usize, u32)> = engines
        .iter()
        .enumerate()
        .filter_map(|(pane, engine)| Some((pane, engine.take_moved_page()?)))
        .collect();
    // What the page views hold of tabs they're about to let go of goes in the session first, as a cold tab holds nothing there.
    inner.take_places();
    let shown_before: Vec<Option<TabId>> = synced.panes.iter().map(|pane| pane.tab).collect();
    let held = {
        let published = lock(&inner.published);
        crate::Held {
            tabs: Arc::clone(&published.tabs),
            contents: Arc::clone(&published.contents),
            sidebar: Arc::clone(&published.sidebar),
            recent_key: published.recent_key,
        }
    };
    // One look at the session for the whole pass, so what's projected is of one moment: the places the page views moved first, then everything the window shows.
    let mut facts = inner.host.session.with_session(|session| {
        let panes = session.panes();
        for (pane, page) in &moved {
            if let (Some(id), Some(Some(shown))) = (panes.get(*pane).copied(), shown_before.get(*pane))
                && *shown == id
                && session.go_to(id, *page).is_ok()
            {
                let page = session.tab(id).map_or(*page, |tab| tab.page);
                if let Some(pane) = synced.panes.get_mut(*pane) {
                    pane.page = page;
                }
                events.push(recto_agent::page_changed(session, id, page + 1));
            }
        }
        let mut facts = crate::read(session, &held);
        for snapshot in &mut facts.panes {
            snapshot.jump = session.take_jump(snapshot.id);
        }
        facts
    });
    // The paper first, so what's asked for next is drawn right.
    let look = crate::paper_look(facts.chosen_paper, inner.window.dark.load(Ordering::SeqCst));
    let paper = look.kind;
    if synced.paper != Some(paper) {
        synced.paper = Some(paper);
        for engine in inner.panes.engines(&inner.engine) {
            engine.set_paper(paper);
        }
    }
    let active = facts.active;
    let comparison = facts.comparison.take();
    let pane_pages = std::mem::take(&mut facts.pane_pages);
    let shown_tab = facts.reading.shown_tab();
    // A notice about one document doesn't stay over another: a save's note, seen on the next tab, would read as being about that one.
    if inner.window.notice_tab.swap(shown_tab, Ordering::SeqCst) != shown_tab {
        lock(&inner.window.action_notice).take();
        snapshot_tool::tab_came_forward(inner);
    }
    let save = save_state_now(inner, &facts.reading);
    let problem = facts.reading_pane().and_then(|shown| shown.problem.clone());
    // The contents follow the tab being read, whichever pane it's in.
    let holding_now = *lock(&inner.life.holding_tab);
    if synced.active != active && active.is_some() && holding_now != active {
        let mut follow = lock(&inner.window.contents_follow);
        follow.new_contents();
        let mut app = lock(&inner.window.app);
        match app.contents_resume.take() {
            Some((tab, scroll)) if Some(tab) == active => follow.resume_at(scroll.y, scroll.following),
            _ => {}
        }
        synced.active = active;
    }
    // A page view for each pane, and no more: one that's gone ends with what it held.
    let count = facts.panes.len().max(1);
    if synced.count != count {
        let window_open = !lock(&inner.window.app).window_closed;
        inner.panes.set_count(&inner.engine, count, paper, window_open);
        synced.count = count;
    }
    synced.panes.resize_with(count, PaneSynced::default);
    if facts.panes.is_empty() && synced.panes[0].tab.take().is_some() {
        inner.engine.deactivate();
    }
    let mut panes_changed = false;
    for (pane, snapshot) in facts.panes.iter().enumerate() {
        let Some(engine) = inner.panes.engine(&inner.engine, pane) else { continue };
        panes_changed |= synced.panes[pane].tab != Some(snapshot.id);
        show_in_pane(inner, &engine, &mut synced.panes[pane], snapshot);
    }
    let shown_tabs: Vec<TabId> = facts.panes.iter().map(|shown| shown.id).collect();
    let reading_pane = active.and_then(|id| shown_tabs.iter().position(|shown| *shown == id)).unwrap_or(0);
    let comparing = comparison.is_some();
    inner.panes.lay_out(shown_tabs, reading_pane, comparison, pane_pages);
    if panes_changed && comparing {
        inner.panes.even_out_zoom(&inner.engine);
    }
    // A file whose renderer found it needs a password the keyring has none for, as one moved to another volume or replaced while it was open, needs it again: its tab says so, and is asked about as any is.
    for engine in inner.panes.engines(&inner.engine) {
        for path in engine.take_keyless() {
            inner.host.session.with_session(|session| {
                if let Some(id) = session.tab_for_path(&path) {
                    session.needs_password_again(id);
                }
            });
        }
    }
    // A protected tab showing with no password given is asked about, unless the person said Cancel and hasn't gone to it again, or its tries have run out.
    let (wants, active_tab) = inner.host.session.with_session(|session| {
        let active = session.active();
        (
            active.filter(|id| session.needs_password(*id)).and_then(|id| Some((id, session.path_of(id)?))),
            active,
        )
    });
    let asked = lock(&inner.tools.prompts.passwords).tabs_asked();
    let closed: Vec<TabId> = if asked.is_empty() {
        Vec::new()
    } else {
        inner
            .host
            .session
            .with_session(|session| asked.into_iter().filter(|id| session.tab(*id).is_none()).collect())
    };
    let password_prompt = {
        let mut asks = lock(&inner.tools.prompts.passwords);
        asks.showing_tab(active_tab, &closed);
        if let Some((id, path)) = wants
            && !asks.declined(id)
        {
            asks.ask(passwords::Target::Tab(id), &path);
        }
        asks.prompt()
    };
    // A password goes with its tab: one for a file no tab has open now is let go, and overwritten.
    if !inner.keys.is_empty() {
        let open: Vec<(u64, u64)> = inner.host.session.with_session(|session| {
            session
                .tabs()
                .iter()
                .filter_map(|tab| session.own_stamp(tab.id))
                .map(|stamp| (stamp.device, stamp.inode))
                .collect()
        });
        inner.keys.keep_only(&open);
    }
    let reading_engine = inner.panes.reading_engine(&inner.engine);
    let note = crate::NoteSources {
        open_failure: lock(&inner.window.open_failure).clone(),
        save_failed: inner.saver.has_failed(),
        problem,
        session: lock(&inner.host.notice).as_deref().and_then(record::notices::words).map(str::to_string),
        compare: facts.compare_note.take(),
        engine: reading_engine.note(),
    }
    .note();
    let selection_notice = crate::SelectionSources {
        copy: reading_engine.selection_notice().map(|notice| crate::CopyNotice {
            text: notice.text,
            offers_copy_anyway: notice.offers_copy_anyway,
            fades: notice.fades,
        }),
        printing: inner.tools.printing.selection_notice(),
        tool: inner.tools.tool.selection_notice(),
        action: lock(&inner.window.action_notice).clone(),
        update: inner.updates.notice().map(|notice| (notice.text, notice.fades)),
    }
    .notice();
    let engine_notes: Vec<Option<String>> = if comparing {
        (0..facts.panes.len())
            .map(|pane| inner.panes.engine(&inner.engine, pane).and_then(|engine| engine.note()))
            .collect()
    } else {
        Vec::new()
    };
    let model = crate::reader_view(
        &facts,
        crate::Surfaces {
            paper: look,
            note,
            selection_notice,
            pane_views: crate::pane_views(&facts.panes, active, comparing, &engine_notes),
            holding: lock(&inner.life.holding_tab).is_some(),
            first_pane_empty: synced.panes.first().is_none_or(|pane| pane.tab.is_none()),
            sidebar_width: lock(&inner.window.app).sidebar_width,
            save: crate::SaveFacts {
                off: save.off,
                note: save.note,
                tooltip: save.tooltip,
            },
            print_ready: inner.tools.printing.has_ready(),
        },
    );
    // The sheets that ask something are the bridge's, and ride beside the model. These three assignments go when the saving, linking and passwords modules move to recto-app.
    let mut reader = ReaderView::from(model);
    reader.save_problem = inner.tools.save_ui.problem();
    reader.link_prompt = lock(&inner.tools.prompts.link).clone();
    reader.password_prompt = password_prompt;
    let pane_tocs: Vec<Option<Arc<recto_core::Toc>>> = facts.panes.iter().map(|shown| shown.toc.clone()).collect();
    let crate::SidebarParts {
        view: sidebar,
        tabs,
        contents,
        rail_filled,
    } = facts.sidebar;
    let mut published = lock(&inner.published);
    published.pane_tocs = pane_tocs;
    // Open Recent is built again only when its list has moved, or the tabs have, which say which of its files are open: not at every reconcile.
    published.recent_key = Some(facts.recent_key);
    let recent_changed = facts.recent.is_some_and(|recent| {
        let changed = *published.recent != recent;
        if changed {
            published.recent = Arc::new(recent);
        }
        changed
    });
    let changed = recent_changed
        || *published.sidebar != sidebar
        || published.reader != reader
        || published.rail_filled != rail_filled
        || !Arc::ptr_eq(&published.contents, &contents);
    if changed {
        published.tabs = tabs;
        published.sidebar = Arc::new(sidebar);
        published.reader = reader;
        published.contents = contents;
        published.rail_filled = rail_filled;
    }
    drop(published);
    if changed {
        inner.listener.model_changed();
    }
    // What's left alone comes due from here, and what's off screen may be over its budget.
    inner.check_woken_files();
    inner.rearm_dormancy();
    inner.look_at_budget();
    // Whatever just happened may be something to keep. Nothing's written unless it really differs when its time comes.
    let (retired, journal_work) = inner
        .host
        .session
        .with_session(|session| (session.has_retired_journals(), session.has_journal_work()));
    if retired {
        inner.saver.write_soon();
    } else {
        if journal_work {
            inner.saver.edit_made();
        }
        inner.saver.view_changed();
    }
}

/// **What the window's Save is for the tab being read, decided by Rust.** A signed file's (SPEC decision 27): an edit a Save over it would break the signature
/// with turns Save off, and Save As says its copy won't be signed. Asked only with edits to save, off this thread, and the window's brought up to date again once
/// it's known. A file nothing can save (opened with its password, or whose readers disagree) has Save and Save As off.
fn save_state_now(inner: &Inner, reading: &crate::model::ReadingFacts) -> crate::saving::SaveState {
    let core_queue = Arc::clone(&inner.core);
    let save_ui = &inner.tools.save_ui;
    let signature = if reading.edited {
        reading
            .file
            .as_ref()
            .and_then(|(id, path, stamp)| save_ui.signature_facts(*id, path, *stamp, move || core_queue.push(CoreCall::Reconcile)))
            .unwrap_or_default()
    } else {
        recto_core::save::SignatureFacts::default()
    };
    let core_queue = Arc::clone(&inner.core);
    save_ui.save_state(
        signature,
        reading.needs_rewrite,
        reading.protected,
        reading
            .file
            .as_ref()
            .filter(|_| reading.edited)
            .map(|(_, path, stamp)| (path.as_path(), *stamp)),
        move || core_queue.push(CoreCall::Reconcile),
    )
}
