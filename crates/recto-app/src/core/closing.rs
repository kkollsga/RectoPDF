//! **Closing a tab, as the person does it** (step R2.3, C5c): File > Close is `close`, the command an agent sends, run for the person; the close prompt's
//! Don't Save is `discard` and then `close`, and its Save is the window's Save, on a thread of its own, with the `close` after it as the end of something
//! under way. Cancel and Save As are the host's. What a prompt offers is `crate::saving`'s.

use super::{HostAction, Inner, person};
use crate::queue::{CoreCall, Origin, PersonCommand};
use crate::saving::{CloseChoice, SAVED_NOT_CLOSED};
use recto_agent::Command;
use recto_agent::commands::tabs::Close;
use recto_agent::commands::{Move, editing::Discard};
use recto_session::TabId;
use std::sync::Arc;
use std::sync::atomic::Ordering;

fn close(tab: TabId) -> Command {
    Close { tab: format!("#{}", tab.0) }.into()
}

/// **What the window says when a person's close didn't go:** only the close after the prompt's Save, which an edit made since the save stops, as the file
/// hasn't got it, says so, in the prompt's words. Any other close that's refused (an edit that came in as File > Close was chosen) said nothing, and says
/// nothing now.
pub fn answered(inner: &Inner, ran: &person::Ran<'_>) {
    if let (Some(error), Command::Move(Move::Close(_)), Origin::Finishing) = (ran.refused(), ran.command, ran.origin)
        && error.code == "unsaved_changes"
    {
        person::say(inner, SAVED_NOT_CLOSED.to_string());
    }
}

/// The close after the prompt's Save, once the save's done: the end of something already under way, so a hand-off's freeze lets it through, or a quit that
/// stays would leave the saved tab open.
pub fn after_save(inner: &Inner, tab: TabId) {
    inner.core.push(CoreCall::Run(Box::new(PersonCommand::finishing(close(tab), None))));
}

impl Inner {
    /// File > Close on a tab with nothing to lose: `close`.
    pub fn close_for_person(self: &Arc<Self>, tab: TabId) {
        person::submit(self, close(tab));
    }

    /// The close prompt's answer. Save closes once it's saved, and not if the save fails. Don't Save throws the edits away and closes: `discard`, then
    /// `close`. Save As opens its sheet, and the tab stays. Past a hand-off's last look, neither Save nor Don't Save is taken, as before.
    pub fn close_answered(self: &Arc<Self>, tab: u32, choice: CloseChoice) -> HostAction {
        if tab == 0 {
            return HostAction::None;
        }
        let tab = TabId(tab);
        match choice {
            CloseChoice::SaveAs => return HostAction::ShowSavePanel,
            CloseChoice::Save if !self.core.frozen.load(Ordering::SeqCst) => super::saving::start_save(self, tab, None, true),
            CloseChoice::DontSave => {
                person::submit(self, Discard { tab: format!("#{}", tab.0) }.into());
                person::submit(self, close(tab));
            }
            CloseChoice::Save | CloseChoice::Cancel => {}
        }
        HostAction::None
    }
}
