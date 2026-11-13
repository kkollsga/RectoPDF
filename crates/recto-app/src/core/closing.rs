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
pub fn answered(inner: &std::sync::Arc<Inner>, ran: &person::Ran<'_>) {
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

/// **Closing's menu ids** ([`super::menu_rows`]): File > Close is `close` on the document showing, with nothing to do with none or while it's saving; Close
/// Window is the system's.
pub fn row<'a>(inner: &'a Arc<Inner>, asked: &super::menu_rows::Asked<'_>) -> Option<super::menu_rows::Row<'a>> {
    use super::menu_rows::Row;
    match (asked.action.as_ref()?, asked.id) {
        (crate::menus::Action::CloseDocument, _) => {
            let (reader, undone_owner_steps) = {
                let published = super::lock(&inner.published);
                (published.reader.clone(), published.sidebar.undone_owner_steps)
            };
            if reader.home || reader.saving || reader.shown_tab == 0 {
                return Some(Row::Nothing);
            }
            let tab = TabId(reader.shown_tab);
            // The close rule, said once in recto-agent, asked of what the session thread published: the UI thread doesn't take the session's lock.
            let would_lose = recto_agent::close_rule::WouldLose::as_published(reader.edited, undone_owner_steps);
            Some(Row::own_way(
                close(tab),
                "a person is asked first when closing would lose unsaved edits or their own undone steps, as any Mac app asks",
                move || {
                    if would_lose.asks_the_person() {
                        return HostAction::ConfirmClose;
                    }
                    person::submit(inner, close(tab));
                    HostAction::None
                },
            ))
        }
        (crate::menus::Action::Host(host), "file.close_window") => Some(Row::host(
            "closes the window, which is the system's, and the app's lifecycle the person's",
            super::menu_rows::Agents::Never,
            *host,
        )),
        _ => None,
    }
}

impl Inner {
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
