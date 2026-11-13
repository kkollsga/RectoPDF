//! **How pages are shown, as the person changes it** (step R2.3, C5a): the paper, the sidebar, the agent icon and a tab's layout are each `view`, the command
//! an agent sends, run for the person on the session thread ([`super::person::submit`]); the zoom is [`super::zoom`]'s, on the thread that asked.

use super::{Inner, person};
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

    /// The sidebar's button, beside the window's controls: `view --sidebar toggle`, shown if it's hidden and hidden if not, as it is when it runs, so a quick
    /// second press before the first is shown is a second toggle (accident A3).
    pub fn toggle_sidebar(self: &Arc<Self>) {
        self.view_for_person(View {
            sidebar: Some("toggle".into()),
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

    /// A layout from the context menu: `view --layout --tab`, naming the tab in the pane being read as it's chosen, which a right-click on a pane has just
    /// made it, whatever the session's still to hear: a tab tapped just before isn't laid out instead (accident A4). The active tab's with no tab shown.
    pub fn set_layout(self: &Arc<Self>, spread: record::Spread) {
        let tab = self.panes.tab_of(self.panes.reading());
        self.view_for_person(View {
            layout: Some(spread.name().into()),
            tab: tab.map(|tab| format!("#{}", tab.0)),
            ..View::default()
        });
    }
}
