//! The sidebar's side of generated contents: how a tab comes to show them. What a tap on the note does is `contents show` and `contents stop`, the commands an
//! agent sends, run for the person (R2.3, C5f). Everything that looks at a file happens on a one-shot thread, never on the session thread or the UI's, since a file on a dead volume can hold a look for as long as it likes. What comes back goes through the session thread's queue.
//!
//! It's given the session host and a [`Push`]: the one way an answer gets back to the session thread, which `recto-ffi` makes over its queue. A push is cloned only for a look's
//! thread, which owns its copy.

use recto_agent::DrawableLoader;
use recto_agent::generated;
use recto_control::host::SessionHost;
use recto_platform::FileStamp;
use recto_session::{GeneratedView, TabId};

/// Hands a tab's generated contents view to the session thread, with the version of the file it's about (`None`: whichever). Must return at once: it's called on a look's
/// thread and on the session thread.
pub trait Push: Fn(TabId, GeneratedView, Option<FileStamp>) + Clone + Send + 'static {}

impl<P: Fn(TabId, GeneratedView, Option<FileStamp>) + Clone + Send + 'static> Push for P {}

/// Generated contents were made somewhere, or failed: each tab waiting for some looks at where its own stand.
pub fn look_again(session: &SessionHost<DrawableLoader>, push: &impl Push) {
    let waiting = session.with_session(|session| session.generated_making());
    for id in waiting {
        look(session, push, id, false);
    }
}

/// A tab that's shown generated contents before, across a restart or after they'd left memory, finds them again. Only the active tab, once: what it finds is noted, so this costs a comparison at every other turn.
pub fn look_if_chosen(session: &SessionHost<DrawableLoader>, push: &impl Push) {
    let wanted = session.with_session(|session| {
        let id = session.active()?;
        let chosen = session.tab(id)?.generated_contents && session.generated_view(id).is_none() && session.contents_verdict(id).is_some();
        if chosen {
            session.set_generated_view(id, GeneratedView::NotKept);
        }
        chosen.then_some(id)
    });
    if let Some(id) = wanted {
        look(session, push, id, false);
    }
}

/// Looks at where one tab's generated contents stand, on a thread of its own, and asks for them to be made if `ask`.
fn look(session: &SessionHost<DrawableLoader>, push: &impl Push, id: TabId, ask: bool) {
    let Some(generator) = session.generator().cloned() else {
        push(id, GeneratedView::Failed("Generated contents aren't available here."), None);
        return;
    };
    // Which version of the file this look is about: an answer that comes back after the file changed is about the old one, and is dropped.
    let Some((path, pages, version)) = session.with_session(|session| Some((session.path_of(id)?, session.tab(id)?.page_count, session.own_stamp(id)))) else {
        return;
    };
    let answer = push.clone();
    let spawned = std::thread::Builder::new()
        .name("recto-contents-look".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            if let Some(view) = generated::where_they_stand(&generator, &path, pages, ask) {
                answer(id, view, version);
            }
        });
    if spawned.is_err() {
        push(id, GeneratedView::Failed("They couldn't be asked for just now. Try again."), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Failure: looking again when nothing's waiting asks the helper or pushes something anyway, on every Finished the generator sends.
    #[test]
    fn looking_again_with_nothing_waiting_does_nothing() {
        let session = SessionHost::default();
        let pushed = Arc::new(Mutex::new(0_usize));
        let got = Arc::clone(&pushed);
        let push = move |_: TabId, _: GeneratedView, _: Option<FileStamp>| *crate::lock(&got) += 1;
        look_again(&session, &push);
        look_if_chosen(&session, &push);
        assert_eq!(*crate::lock(&pushed), 0);
    }
}
