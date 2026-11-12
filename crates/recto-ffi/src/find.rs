//! **The sidebar's Find: its records and the calls.** What it shows, what each call does and when the host is told are `recto_app::find`'s; this hands it the app's parts it
//! reaches, as a [`FindHandle`], and converts what comes back.

use crate::Inner;
use recto_agent::Event;
use recto_app::find::{self as app, FindHandle};
pub(crate) use recto_app::find::{FIELD_HEIGHT, FindCall, ROW_HEIGHT, STATUS_HEIGHT, heights};
pub use recto_app::find::{FindRow, FindRowKind, FindView};
use serde_json::Value;

/// Everything the sidebar draws of Find but its rows, which are pulled with [`rows`] as they come on screen.
#[uniffi::remote(Record)]
pub struct FindView {
    pub open: bool,
    /// What's in the field.
    pub text: String,
    pub placeholder: String,
    /// The line under the field, in words. Empty with nothing typed.
    pub status: String,
    /// Goes up with every change to the rows or the line, which is when to pull them.
    pub version: u64,
    pub row_count: u32,
    /// The row of the hit gone to last.
    pub current_row: Option<u32>,
    /// A query's running, which `qa wait-settled` waits for.
    pub running: bool,
    pub close_tooltip: String,
}

#[uniffi::remote(Enum)]
pub enum FindRowKind {
    /// A page, heading its hits.
    Page,
    Hit,
}

/// A row of the hit list. A hit's words are the document's, so they're only ever drawn as text.
#[uniffi::remote(Record)]
pub struct FindRow {
    pub kind: FindRowKind,
    /// A page row's label, or the words before a hit.
    pub label: String,
    pub matched: String,
    pub after: String,
    pub current: bool,
    /// Its words include text that looks hidden, so it isn't gone to. What to say of it, quietly, or empty.
    pub note: String,
}

/// What Find reaches of the app, for one call.
fn handle<R>(inner: &Inner, step: impl FnOnce(&FindHandle<'_, dyn recto_app::Listener>) -> R) -> R {
    let set_view = |events: &mut Vec<Event>, change: fn(&mut recto_session::ViewSettings)| crate::set_view(inner, events, change);
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

pub(crate) fn is_open(inner: &Inner) -> bool {
    app::is_open(&inner.tools.find_field)
}

pub(crate) fn view(inner: &Inner) -> FindView {
    handle(inner, |find| find.view())
}

pub(crate) fn rows(inner: &Inner, first: u32, count: u32) -> Vec<FindRow> {
    handle(inner, |find| find.rows(first, count)).into_iter().collect()
}

pub(crate) fn act(inner: &Inner, call: FindCall, events: &mut Vec<Event>) {
    handle(inner, |find| find.act(call, events));
}

pub(crate) fn keep_up(inner: &Inner) {
    handle(inner, |find| find.keep_up());
}

pub(crate) fn report(inner: &Inner) -> Value {
    handle(inner, |find| find.report())
}
