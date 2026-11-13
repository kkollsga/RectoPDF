//! **Side by side, as the session thread does it:** a tab joining or leaving, the pane being read and synced scroll. Each pane's layout is `view`'s.

use super::*;

/// `CoreCall::CompareClick`, on the session thread.
pub fn compare_click(inner: &Arc<Inner>, reads: &mut FileReads, id: TabId, events: &mut Vec<Event>) {
    let host = &inner.host;
    // The tab joining, or the pane read next if the one read leaves, is read first with the session free, as a tap's is.
    let cold = host.session.with_session(|session| session.wake_for_compare_click(id));
    match cold {
        Some((tab, path)) => {
            reads.wanted_tab = Some(tab);
            reads.start(inner, Reading::Compare(id, tab), path);
        }
        None => compare_click_now(inner, id, events),
    }
}

/// `CoreCall::FocusPane`, on the session thread.
pub fn focus_pane(inner: &Arc<Inner>, reads: &mut FileReads, id: TabId, events: &mut Vec<Event>) {
    let host = &inner.host;
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

/// `CoreCall::SetSynced`, on the session thread.
pub fn set_synced(inner: &Arc<Inner>, on: bool, events: &mut Vec<Event>) {
    let host = &inner.host;
    events.extend(host.session.with_session(|session| {
        session
            .set_compare_synced(on)
            .filter(|changed| *changed)
            .map(|_| recto_agent::event::comparison_changed(session))
    }));
}
