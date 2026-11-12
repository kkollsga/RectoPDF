//! Generated contents in the sidebar: what a tap on the note does and how a tab comes to show them are `recto_app::generated_contents`'s; this is how their answers reach the
//! session thread, through its queue.

use crate::{CoreCall, Inner};
use recto_app::generated_contents as app;
use recto_session::{ContentsAction, TabId};
use std::sync::Arc;

/// The queue, as the one way an answer gets back. Made per call, an `Arc` clone: a look's thread takes its own.
fn push(inner: &Inner) -> impl app::Push {
    let core = Arc::clone(&inner.core);
    move |id, view, version| core.push(CoreCall::GeneratedView(id, view, version))
}

pub(crate) fn act(inner: &Inner, id: TabId, action: ContentsAction) {
    app::act(&inner.host.session, &push(inner), id, action);
}

pub(crate) fn look_again(inner: &Inner) {
    app::look_again(&inner.host.session, &push(inner));
}

pub(crate) fn look_if_chosen(inner: &Inner) {
    app::look_if_chosen(&inner.host.session, &push(inner));
}
