//! **The sidebar's Find: its records and the calls.** What it shows, what each call does and when the host is told are `crate::find`'s; this hands it the app's parts it
//! reaches, as a [`FindHandle`], and converts what comes back.

use super::Inner;
use crate::find::{self as app, FindHandle};
pub use crate::find::{FIELD_HEIGHT, FindCall, ROW_HEIGHT, STATUS_HEIGHT, heights};
pub use crate::find::{FindRow, FindRowKind, FindView};
use recto_agent::Event;
use serde_json::Value;

/// What Find reaches of the app, for one call.
fn handle<R>(inner: &Inner, step: impl FnOnce(&FindHandle<'_, dyn crate::Listener>) -> R) -> R {
    let set_view = |events: &mut Vec<Event>, change: fn(&mut recto_session::ViewSettings)| super::set_view(inner, events, change);
    step(&FindHandle {
        session: &inner.host.session,
        field: &inner.tools.find_field,
        frozen: &inner.core.frozen,
        panes: &inner.panes,
        engine: &inner.engine,
        listener: &*inner.listener,
        set_view: &set_view,
    })
}

/// **What a command asks of the window, carried out on the session thread,** whoever asked: Find's hit selected and brought into view.
pub fn effects(core: std::sync::Arc<crate::queue::CoreQueue>) -> recto_control::host::Effects {
    std::sync::Arc::new(move |effect| match effect {
        recto_agent::Effect::Reveal { tab, hit } => core.push(crate::queue::CoreCall::Find(FindCall::Reveal(tab.0, hit))),
        // Only the person's step is kept, and the window's own Find keeps it itself until its steps are the person's `find`.
        recto_agent::Effect::FindStepPending { .. } => {}
    })
}

pub fn is_open(inner: &Inner) -> bool {
    app::is_open(&inner.tools.find_field)
}

pub fn view(inner: &Inner) -> FindView {
    handle(inner, |find| find.view())
}

pub fn rows(inner: &Inner, first: u32, count: u32) -> Vec<FindRow> {
    handle(inner, |find| find.rows(first, count)).into_iter().collect()
}

pub fn act(inner: &Inner, call: FindCall, events: &mut Vec<Event>) {
    handle(inner, |find| find.act(call, events));
}

pub fn keep_up(inner: &Inner) {
    handle(inner, |find| find.keep_up());
}

pub fn report(inner: &Inner) -> Value {
    handle(inner, |find| find.report())
}
