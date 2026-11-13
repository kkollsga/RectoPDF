//! **The window's edits:** the panel's Rotate, Edit > Undo and Redo are `page rotate`, `undo` and `redo`, the commands an agent sends, run for the person
//! (R2.3, C5d), and so the owner's, as a person's edits always are; Edit > Clear History is `clear-history`, and the panel's Done `edit mode off`.

use super::*;
use recto_agent::Actor;
use recto_agent::commands::Edit;
use recto_agent::commands::edit_mode::EditMode;
use recto_agent::commands::editing::{ClearHistory, PageRotate, Redo, Undo};
use recto_agent::commands::words::OnOff;

/// The panel's Rotate, a quarter turn each way: every page, or in edit mode the page being read, by the session as it is when it runs, so an edit mode
/// turned on just before is the one it goes by. Its tab is the one being read.
fn rotate(turns: i32) -> Command {
    PageRotate {
        tab: "active".into(),
        pages: "all".into(),
        degrees: turns.saturating_mul(90),
    }
    .into()
}

/// [`rotate`]'s tab and pages, as the session has them when it runs: the tab being read, named, so a tap between this and the run doesn't move the rotate
/// to another tab; and in edit mode the page it's on. A tab withheld from `actor` is refused here, before its page is read.
fn rotate_pages_now(session: &recto_session::Session<DrawableLoader>, actor: Actor, command: Command) -> Result<Command, AgentError> {
    let Command::Edit(Edit::PageRotate(mut asked)) = command else {
        return Ok(command);
    };
    let Some(id) = session.tabs().active() else { return Ok(asked.into()) };
    if !actor.sees_protected() && session.tabs().withheld_from_agents(id) {
        return Err(recto_agent::protected_refusal());
    }
    asked.tab = recto_agent::TabRef::Id(id);
    if session.viewing().edit_mode()
        && let Some(tab) = session.tabs().tab(id)
    {
        asked.pages = (tab.page + 1).to_string();
    }
    Ok(asked.into())
}

/// **What the window says of a person's Rotate:** why it couldn't, by the page, in the session's words; and once it has, any line that was there goes, as
/// the panel's Rotate always had it. An Undo or Redo with nothing to do says nothing, as before.
pub fn answered(inner: &std::sync::Arc<Inner>, ran: &person::Ran<'_>) {
    if !matches!(ran.command, Command::Edit(Edit::PageRotate(_))) {
        return;
    }
    match (ran.result, ran.refused()) {
        (Ok(_), _) => *lock(&inner.window.action_notice) = None,
        (Err(_), Some(error)) => {
            if let Some(words) = person::session_words(error) {
                person::say(inner, format!("Couldn't rotate: {words}"));
            }
        }
        (Err(_), None) => {}
    }
}

/// **Editing's menu ids** ([`super::menu_rows`]): the panel's Rotate is `page rotate`, every page or in edit mode the page being read, by the session as it is
/// when it runs; Undo, Redo and Clear History are `undo`, `redo` and `clear-history` on the tab being read; the panel's Done is `edit mode off`.
pub fn row<'a>(_: &'a Arc<Inner>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    use super::menu_rows::Row;
    let active = || recto_agent::TabRef::Active;
    Some(match asked.action.as_ref()? {
        menus::Action::Rotate(turns) => Row::Command {
            command: rotate(*turns),
            when_run: Some(rotate_pages_now),
            host: HostAction::None,
        },
        menus::Action::Undo => Row::command(Undo { tab: active() }),
        menus::Action::Redo => Row::command(Redo { tab: active() }),
        menus::Action::ClearEditHistory => Row::command(ClearHistory { tab: active() }),
        menus::Action::LeaveEditMode => Row::command(EditMode { set: OnOff::Off }),
        _ => return None,
    })
}

impl Inner {
    /// Into edit mode or out of it: `edit mode on|off`.
    pub fn edit_mode_for_person(self: &Arc<Self>, on: bool) {
        person::submit(self, EditMode { set: OnOff::from_bool(on) }.into());
    }
}
