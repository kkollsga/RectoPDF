//! **Side by side, as the person does it** (step R2.3, C5e): a ⌘-click or ⌃-click on a tab's row is `compare --toggle`, moving into a pane to read it is
//! `compare --reading`, and the context menu's separate or synced scroll is `compare --sync`, the commands an agent sends, run for the person. A pane's cold
//! file is read off the session thread, and the command runs again once it's in, as a tap's does. Each pane's layout is `view`'s.

use super::{Inner, lock, person};
use recto_agent::commands::words::OnOff;
use recto_agent::commands::{Move, compare::Compare};
use recto_agent::{AgentError, Command, TabRef};
use recto_session::TabId;
use recto_session::compare::CompareError;
use std::sync::Arc;

/// **What the window says when a person's ⌘-click couldn't be done:** why, by the page, in the session's words for a comparison (a fourth pane), as the
/// ⌘-click always said. A pane that couldn't be read, or scroll with no comparison to sync, said nothing and says nothing.
pub fn answered(inner: &std::sync::Arc<Inner>, ran: &person::Ran<'_>) {
    if let (Command::Move(Move::Compare(Compare::Toggle(_))), Some(error)) = (ran.command, ran.refused())
        && let Some(words) = compare_words(error)
    {
        *lock(&inner.window.action_notice) = Some(words.to_string());
    }
}

/// The session's words for a comparison that can't be as asked, by the refusal's code.
fn compare_words(error: &AgentError) -> Option<&'static str> {
    [CompareError::TooMany, CompareError::TooFew, CompareError::NoSuchTab, CompareError::SameTabTwice]
        .into_iter()
        .find(|known| known.code() == error.code)
        .map(CompareError::message)
}

/// The reader's moved into another pane, by the pointer or a scroll: `compare --reading`, that pane's tab the one read. Nothing on screen moves.
pub fn reading(tab: TabId) -> Command {
    Compare::Reading(TabRef::Id(tab)).into()
}

/// **The comparison's menu ids** ([`super::menu_rows`]): the context menu's separate or synced scroll is `compare --sync off|on`.
pub fn row<'a>(_: &'a Arc<Inner>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    let crate::menus::Action::SetSynced(on) = asked.action.as_ref()? else {
        return None;
    };
    Some(super::menu_rows::Row::command(Compare::Sync(OnOff::from_bool(*on))))
}

impl Inner {
    /// ⌘-click or ⌃-click on a tab's row: `compare --toggle`, into the comparison or out of it, as it is when it runs.
    pub fn compare_toggle_for_person(self: &Arc<Self>, tab: TabId) {
        person::submit(self, Compare::Toggle(TabRef::Id(tab)).into());
    }
}
