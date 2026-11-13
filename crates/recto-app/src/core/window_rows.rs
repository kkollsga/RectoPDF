//! **The menu ids that are the window's alone and belong to no document feature** ([`super::menu_rows`]): the app menu's About, Services and Quit, the page
//! view's Copy and Select All, and the tools panel open or shut. None is a command: each is the system's, the page view's or the window's own.

use super::menu_rows::{Asked, Row};
use super::{HostAction, Inner};
use crate::menus::Action;
use std::sync::Arc;
use std::sync::atomic::Ordering;

pub fn row<'a>(inner: &'a Arc<Inner>, asked: &Asked<'_>) -> Option<Row<'a>> {
    Some(match (asked.action.as_ref()?, asked.id) {
        (Action::Host(host), "app.about") => Row::host("the About window is the system's", *host),
        (Action::Host(host), "app.services") => Row::host("the system fills the Services menu", *host),
        (Action::Host(host), "app.quit") => Row::host(
            "quitting is the app's lifecycle: it asks about unsaved edits, then the hand-off of the state",
            *host,
        ),
        (Action::Copy { anyway: false }, _) => Row::window(
            "the selection is the page view's, on the UI thread, and goes to the system's pasteboard",
            move || {
                inner.panes.reading_engine(&inner.engine).copy_selection(false);
                HostAction::None
            },
        ),
        (Action::Copy { anyway: true }, _) => Row::window("the same, after the person has said to copy a protected document's text", move || {
            inner.panes.reading_engine(&inner.engine).copy_selection(true);
            HostAction::None
        }),
        (Action::SelectAll, _) => Row::window("the selection is the page view's", move || {
            inner.panes.reading_engine(&inner.engine).select_all();
            HostAction::None
        }),
        (Action::TogglePanel, _) => Row::window("the tools panel open or shut is the window's own, and nothing of a document's", move || {
            inner.window.panel_open.fetch_xor(true, Ordering::SeqCst);
            inner.listener.model_changed();
            HostAction::None
        }),
        _ => return None,
    })
}
