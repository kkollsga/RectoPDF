//! **How pages are shown, as the person changes it** (step R2.3, C5a): the paper, the sidebar, the agent icon and a tab's layout are each `view`, the command
//! an agent sends, run for the person on the session thread ([`super::person::submit`]); the zoom is [`super::zoom`]'s, on the thread that asked.

use super::menu_rows::{Agents, Asked, Row};
use super::{HostAction, Inner};
use crate::menus::Action;
use recto_agent::Command;
use recto_agent::commands::view::{PaperChoice, Shown, View};
use std::sync::Arc;

/// **The view's menu ids** ([`super::menu_rows`]): the paper, the sidebar, the agent icon and a tab's layout are each `view`. A toggle toggles when it runs,
/// so a quick second press before the first is shown is a second toggle (accident A3). A layout names the tab in the pane being read as it's chosen, which a
/// right-click on a pane has just made it, whatever the session's still to hear: a tab tapped just before isn't laid out instead (accident A4).
pub fn row<'a>(inner: &'a Arc<Inner>, asked: &Asked<'_>) -> Option<Row<'a>> {
    let view = match asked.action.as_ref()? {
        Action::SetPaper(paper) => View {
            paper: Some(PaperChoice::Is(*paper)),
            ..View::default()
        },
        Action::ToggleSidebar => View {
            sidebar: Some(Shown::Toggle),
            ..View::default()
        },
        Action::ToggleAgentIcon => View {
            agent_icon: Some(Shown::Toggle),
            ..View::default()
        },
        Action::SetSpread(spread) => View {
            layout: Some(*spread),
            tab: inner.panes.tab_of(inner.panes.reading()).map(recto_agent::TabRef::Id),
            ..View::default()
        },
        Action::Host(HostAction::None) if asked.id == "view.paper" => {
            return Some(Row::host("a submenu's own item only opens it", Agents::Never, HostAction::None));
        }
        _ => return None,
    };
    Some(Row::command(view))
}

/// The sidebar's button, beside the window's controls: `view --sidebar toggle`, shown if it's hidden and hidden if not, as it is when it runs, so a quick
/// second press before the first is shown is a second toggle (accident A3).
pub fn toggle_sidebar() -> Command {
    View {
        sidebar: Some(Shown::Toggle),
        ..View::default()
    }
    .into()
}

/// The sidebar shown, as ⌘K's palette, which is in it, needs: `view --sidebar on`.
pub fn show_sidebar() -> Command {
    View {
        sidebar: Some(Shown::On),
        ..View::default()
    }
    .into()
}
