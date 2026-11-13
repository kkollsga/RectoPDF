//! **The window's edits, as the session thread does them:** the history cleared, a rotation, Undo and Redo.

use super::*;

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
