//! **The window's edits:** the panel's Rotate, Edit > Undo and Redo are `page rotate`, `undo` and `redo`, the commands an agent sends, run for the person
//! (R2.3, C5d), and so the owner's, as a person's edits always are; Edit > Clear History is `clear-history`, and the panel's Done `edit mode off`.

use super::*;
use recto_agent::commands::Edit;
use recto_agent::commands::edit_mode::EditMode;
use recto_agent::commands::editing::{ClearHistory, PageRotate, Redo, Undo};

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

/// [`rotate`]'s pages, as the session has them when it runs: in edit mode, the page the tab being read is on.
fn rotate_pages_now(session: &recto_session::Session<DrawableLoader>, command: Command) -> Command {
    match command {
        Command::Edit(Edit::PageRotate(mut asked)) if session.edit_mode() => {
            if let Some(tab) = session.active().and_then(|id| session.tab(id)) {
                asked.pages = (tab.page + 1).to_string();
            }
            asked.into()
        }
        other => other,
    }
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

impl Inner {
    /// The panel's Rotate: `page rotate`, every page or in edit mode the page being read.
    pub fn rotate_for_person(self: &Arc<Self>, turns: i32) {
        person::submit_when_run(self, rotate(turns), rotate_pages_now);
    }

    /// Into edit mode or out of it, as the panel's Done takes it out: `edit mode on|off`.
    pub fn edit_mode_for_person(self: &Arc<Self>, on: bool) {
        person::submit(
            self,
            EditMode {
                set: if on { "on" } else { "off" }.into(),
            }
            .into(),
        );
    }

    /// Edit > Clear History: `clear-history`, on the tab being read. The person's level is Full, so the owner's-edits rules never refuse it.
    pub fn clear_history_for_person(self: &Arc<Self>) {
        person::submit(self, ClearHistory { tab: "active".into() }.into());
    }

    /// Edit > Undo, or Redo: `undo` or `redo`, on the tab being read.
    pub fn undo_or_redo_for_person(self: &Arc<Self>, undo: bool) {
        let tab = "active".to_string();
        person::submit(self, if undo { Undo { tab }.into() } else { Redo { tab }.into() });
    }
}
