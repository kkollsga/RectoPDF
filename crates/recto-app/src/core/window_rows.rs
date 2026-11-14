//! **The menu ids that are the window's alone and belong to no document feature** ([`super::menu_rows`]): the app menu's About, Services and Quit, the page
//! view's Copy and Select All, and the tools panel open or shut. None is a command: each is the system's, the page view's or the window's own.

use super::menu_rows::{Agents, Asked, Row};
use super::{HostAction, Inner};
use crate::menus::Action;
use std::sync::Arc;

pub fn row<'a>(inner: &'a Arc<Inner>, asked: &Asked<'_>) -> Option<Row<'a>> {
    Some(match (asked.action.as_ref()?, asked.id) {
        (Action::Host(host), "app.about") => Row::host("the About window is the system's, for the person to read", Agents::Never, *host),
        (Action::Host(host), "app.services") => Row::host("the system fills the Services menu, and its own item does nothing", Agents::Never, *host),
        (Action::Host(host), "app.quit") => Row::host(
            "quitting is the app's lifecycle, and the person's: it asks about unsaved edits, then the hand-off of the state",
            Agents::Never,
            *host,
        ),
        (Action::Copy { anyway: false }, _) => Row::window(
            "the person's selection, by the page view's own copy, which doesn't know who asked, to the system's clipboard, which is the person's",
            Agents::ItsOwn("`recto copy`"),
            move || {
                inner.panes.reading_engine(&inner.engine).selection().copy(false);
                HostAction::None
            },
        ),
        (Action::Copy { anyway: true }, _) => Row::window(
            "the same, after the person has said to copy a protected document's text: the person's own decision, where an agent's is `copy --anyway`, which is logged",
            Agents::ItsOwn("`recto copy --anyway`"),
            move || {
                inner.panes.reading_engine(&inner.engine).selection().copy(true);
                HostAction::None
            },
        ),
        (Action::SelectAll, _) => Row::window(
            "the person's selection in the page view, which rearranges their window",
            Agents::ItsOwn("`recto select --all`"),
            move || {
                inner.panes.reading_engine(&inner.engine).selection().select_all();
                HostAction::None
            },
        ),
        (Action::TogglePanel, _) => Row::window(
            "the tools panel open or shut is the window's own, and nothing of a document's",
            Agents::AT_FULL,
            move || {
                inner.window.toggle_panel();
                inner.listener.model_changed();
                HostAction::None
            },
        ),
        _ => return None,
    })
}
