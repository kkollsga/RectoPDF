//! `view` without a zoom: the paper, the sidebar, the agent icon and the active tab's layout. Zoom is the app's, as it's about a screen.

use crate::{AgentError, Event, SessionAccess, exit, paper_name, view_changed};
use serde_json::{Value, json};

/// Sets what's asked, and says how things are shown now. `[paper, sidebar, agent_icon, layout]`, as `view` has them.
pub(crate) fn set<A: SessionAccess>(access: &A, asked: [&Option<String>; 4], events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let [paper, sidebar, agent_icon, layout] = asked;
    access.with(|s| {
            let was = s.view();
            if let Some(name) = layout.as_deref() {
                let spread = recto_session::record::Spread::from_name(name).ok_or_else(|| AgentError::new("usage", format!("`{}` isn't a layout. It's one of {}.", recto_core::clean_line(name), recto_session::record::Spread::ALL.map(recto_session::record::Spread::name).join(", ")), exit::USAGE))?;
                let id = s.active().ok_or_else(|| AgentError::new("no_active_tab", "There's no tab showing to lay out.", exit::NOT_FOUND).then("`recto open` or `recto tap` one first."))?;
                if s.set_spread(id, spread) {
                    events.push(Event::LayoutChanged { tab: id, layout: spread.name() });
                }
            }
            let mut view = was;
            match paper.as_deref() {
                Some("auto") => view.paper = None,
                Some(name) => view.paper = Some(recto_session::Paper::from_name(name).ok_or_else(|| AgentError::new("usage", format!("`{}` isn't a paper. It's one of {}, or `auto`.", recto_core::clean_line(name), recto_session::Paper::ALL.map(recto_session::Paper::name).join(", ")), exit::USAGE))?),
                None => {}
            }
            match sidebar.as_deref() {
                Some("on") => view.sidebar_hidden = false,
                Some("off") => view.sidebar_hidden = true,
                Some(other) => return Err(AgentError::new("usage", format!("`{}` isn't for the sidebar. It's `on` or `off`.", recto_core::clean_line(other)), exit::USAGE)),
                None => {}
            }
            match agent_icon.as_deref() {
                Some("on") => view.agent_icon_hidden = false,
                Some("off") => view.agent_icon_hidden = true,
                Some(other) => return Err(AgentError::new("usage", format!("`{}` isn't for the agent icon. It's `on` or `off`.", recto_core::clean_line(other)), exit::USAGE)),
                None => {}
            }
            s.set_view(view);
            if view != was {
                events.push(view_changed(view));
            }
            Ok(json!({ "paper": paper_name(view), "papers": recto_session::Paper::ALL.map(recto_session::Paper::name), "sidebar": !view.sidebar_hidden, "agent_icon": !view.agent_icon_hidden, "layout": s.active().and_then(|id| s.spread_of(id)).map(recto_session::record::Spread::name) }))
        })
}
