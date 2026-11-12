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

/// **The window's side of going places:** a link's sheet answered, a typed page and a swipe, each going on the session thread.
impl Inner {
    /// The sheet asking about a web or email address was answered, by its id: with Open, the address, exactly as the sheet showed it; with Cancel, nothing.
    pub fn link_prompt_answered(&self, id: u64, open: bool) -> Option<String> {
        let mut asked = lock(&self.tools.prompts.link);
        // An answer is to the sheet that was shown: a later link's asking is left for its own answer.
        if asked.as_ref().is_none_or(|prompt| prompt.id != id) {
            return None;
        }
        let prompt = asked.take();
        drop(asked);
        self.core.push(CoreCall::Reconcile);
        prompt.filter(|_| open).map(|prompt| prompt.address)
    }

    /// Return in the go-to field: a label the file uses, or a page number. Says whether it was something to look up.
    pub fn page_typed(&self, text: &str) -> bool {
        let typed = text.trim();
        if typed.is_empty() || typed.len() > linking::MAX_TYPED_PAGE || self.core.frozen.load(Ordering::SeqCst) {
            return false;
        }
        let reading = self.panes.reading();
        let Some(tab) = self.panes.tab_of(reading) else { return false };
        self.core.push(CoreCall::JumpTyped(tab, typed.to_string()));
        true
    }

    /// A swipe on the page, as the system reports it: Back or Forward, as in Safari.
    pub fn swiped(&self, delta_x: f64) {
        if let Some(back) = linking::swipe_goes_back(delta_x) {
            self.core.push(CoreCall::Step(back));
        }
    }
}
