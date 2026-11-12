//! **The window's edits and closes, as the session thread does them:** a close answered or after a save, the history cleared, a rotation, Undo and Redo.

use super::*;

/// `CoreCall::CloseAnswered`, on the session thread.
pub fn close_answered(inner: &Arc<Inner>, id: TabId, choice: crate::saving::CloseChoice, events: &mut Vec<Event>) {
    let host = &inner.host;
    match choice {
        crate::saving::CloseChoice::Save => {
            if let Some(inner) = inner.saver.inner() {
                saving::start_save(&inner, id, None, true);
            }
        }
        crate::saving::CloseChoice::DontSave => close_tab(host, id, true, events),
        crate::saving::CloseChoice::Cancel | crate::saving::CloseChoice::SaveAs => {}
    }
}

/// `CoreCall::CloseAfterSave`, on the session thread.
pub fn close_after_save(inner: &Arc<Inner>, id: TabId, events: &mut Vec<Event>) {
    let host = &inner.host;
    // An edit that came in between the save and now isn't in the file: the tab stays, and says so, rather than lose it (review).
    let closed = host.session.with_session(|session| {
        session.close_after_save(id).inspect(|closed| {
            if let Some(closed) = closed {
                recto_agent::event::closed(session, *closed, events);
            }
        })
    });
    if let Ok(None) = closed {
        *lock(&inner.window.action_notice) = Some(saving::SAVED_NOT_CLOSED.to_string());
    }
}

/// `CoreCall::ClearEditHistory`, on the session thread.
pub fn clear_edit_history(inner: &Arc<Inner>) {
    inner.host.session.with_session(|session| {
        if let Some(id) = session.active() {
            session.clear_history(id, recto_session::ClearReason::UserAsked);
        }
    });
}

/// `CoreCall::Rotate`, on the session thread.
pub fn rotate(inner: &Arc<Inner>, turns: i32, events: &mut Vec<Event>) {
    let host = &inner.host;
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

/// `CoreCall::UndoOrRedo`, on the session thread.
pub fn undo_or_redo(inner: &Arc<Inner>, undo: bool, events: &mut Vec<Event>) {
    let host = &inner.host;
    host.session.with_session(|session| {
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
    });
}
