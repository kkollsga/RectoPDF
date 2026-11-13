//! **How pages are shown, as the person changes it** (step R2.3, C5a): the paper, the sidebar, the agent icon and a tab's layout are each `view`, the command
//! an agent sends, run for the person on the session thread ([`super::person::submit`]); the zoom is [`super::zoom`]'s, on the thread that asked.

use super::{Inner, lock, person};
use recto_agent::commands::view::View;
use recto_session::{Paper, record};
use std::sync::Arc;

impl Inner {
    /// `view` with what the person changed, for them.
    fn view_for_person(self: &Arc<Self>, asked: View) {
        person::submit(self, asked.into());
    }

    /// The paper menu's choice: `view --paper`.
    pub fn set_paper(self: &Arc<Self>, paper: Paper) {
        self.view_for_person(View {
            paper: Some(paper.name().into()),
            ..View::default()
        });
    }

    /// The sidebar's button, beside the window's controls: `view --sidebar`, shown if it's hidden and hidden if not.
    pub fn toggle_sidebar(self: &Arc<Self>) {
        let hidden = lock(&self.published).reader.sidebar_hidden;
        self.view_for_person(View {
            sidebar: Some((if hidden { "on" } else { "off" }).into()),
            ..View::default()
        });
    }

    /// The sidebar shown, as ⌘K's palette, which is in it, needs: `view --sidebar on`.
    pub fn show_sidebar(self: &Arc<Self>) {
        self.view_for_person(View {
            sidebar: Some("on".into()),
            ..View::default()
        });
    }

    /// View > Agent Icon: `view --agent-icon toggle`.
    pub fn toggle_agent_icon(self: &Arc<Self>) {
        self.view_for_person(View {
            agent_icon: Some("toggle".into()),
            ..View::default()
        });
    }

    /// A layout from the context menu: `view --layout`, for the pane being read as it's chosen, which a right-click on a pane has just made it, whatever the
    /// session's still to hear, when there's more than one; the active tab's otherwise.
    pub fn set_layout(self: &Arc<Self>, spread: record::Spread) {
        let tab = (self.panes.count() > 1).then(|| self.panes.tab_of(self.panes.reading())).flatten();
        self.view_for_person(View {
            layout: Some(spread.name().into()),
            tab: tab.map(|tab| format!("#{}", tab.0)),
            ..View::default()
        });
    }
}
