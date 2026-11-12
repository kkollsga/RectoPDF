//! **Going to a page:** a contents entry or a link, a typed page, and Back and Forward, as the session thread does them.

use super::*;

/// `CoreCall::Jump`, on the session thread.
pub fn jump(inner: &Arc<Inner>, id: TabId, page: u32, within: f64, events: &mut Vec<Event>) {
    let host = &inner.host;
    // Where the reader is, as the page view has it now, is what Back comes back to. `jump` clamps: a contents entry can't send the reader off the end.
    inner.take_places();
    if let Ok(event) = host
        .session
        .with_session(|session| session.jump(id, page, within).map(|()| recto_agent::page_now(session, id)))
    {
        events.push(event);
    }
}

/// `CoreCall::JumpTyped`, on the session thread.
pub fn jump_typed(inner: &Arc<Inner>, id: TabId, typed: String, events: &mut Vec<Event>) {
    let host = &inner.host;
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

/// `CoreCall::Step`, on the session thread.
pub fn step(inner: &Arc<Inner>, back: bool, events: &mut Vec<Event>) {
    let host = &inner.host;
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
