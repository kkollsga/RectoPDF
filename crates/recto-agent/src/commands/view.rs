//! How the app shows pages: `view` (the paper, the sidebar, the agent icon, the active tab's layout, and the zoom, which is the app's, as it's about a screen) and ⌘K's `palette`, which only the app has.

use super::{Cmd, Cx, OnlyAsks, row_access};
use crate::human::text;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;

/// How the app shows pages.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    #[serde(default)]
    pub paper: Option<String>,
    #[serde(default)]
    pub sidebar: Option<String>,
    #[serde(default)]
    pub agent_icon: Option<String>,
    #[serde(default)]
    pub zoom: Option<String>,
    /// The active tab's layout: `single`, `two-up` or `two-up-cover`.
    #[serde(default)]
    pub layout: Option<String>,
    /// The tab `layout` is for, and said of: the active tab's if left out. The window names the tab in the pane it was chosen on (R2.3), as the active tab
    /// may have moved on by the time it runs.
    #[serde(default)]
    pub tab: Option<String>,
}

impl Cmd for View {
    const TAG: &'static str = "view";
    const ROWS: &'static [Spec] = ROWS_VIEW;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        self.tab.as_deref()
    }
    /// Asking how things are shown changes nothing, whichever tab's layout it asks for. Changing it is moving around.
    fn access(&self) -> Access {
        match self {
            Self {
                paper: None,
                sidebar: None,
                agent_icon: None,
                zoom: None,
                layout: None,
                tab: _,
            } => row_access(Self::TAG),
            _ => Access::At(Level::Control),
        }
    }
    /// A zoom is about what's on a screen, so it's the app's. The rest is the session's.
    fn route(&self) -> Runs {
        match self.zoom {
            Some(_) => Runs::App,
            None => Runs::Session,
        }
    }
    fn log_details(&self) -> Value {
        match &self.tab {
            Some(tab) => json!({ "tab": super::log_tab(tab) }),
            None => Value::Null,
        }
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self {
            paper,
            sidebar,
            agent_icon,
            zoom,
            layout,
            tab,
        } = self;
        if zoom.is_some() {
            return Err(
                AgentError::new("zoom_needs_app", "Zoom is about what's on a screen, and this session has none.", exit::FAILED)
                    .then("It works when `recto` is talking to the app."),
            );
        }
        set(cx.access, [paper, sidebar, agent_icon, layout], tab.as_deref(), cx.events)
    }
    fn human(data: &Value) -> String {
        format!(
            "Paper: {}. Sidebar: {}. Agent icon: {}.{}",
            text(&data["paper"]),
            if data["sidebar"] == false { "hidden" } else { "shown" },
            if data["agent_icon"] == true { "on" } else { "off" },
            data["layout"].as_str().map(|layout| format!(" Layout: {layout}.")).unwrap_or_default()
        )
    }
}

/// A zoom changes what's on the screen.
impl OnlyAsks for View {
    fn only_asks(&self) -> bool {
        false
    }
}

/// ⌘K's palette: what a query finds among the app's commands and tabs, best first, and running the first.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Palette {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub run: bool,
    #[serde(default)]
    pub limit: Option<u32>,
}

impl Cmd for Palette {
    const TAG: &'static str = "palette";
    const ROWS: &'static [Spec] = ROWS_PALETTE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    /// Running a command is doing it, as its menu item would.
    fn access(&self) -> Access {
        if self.run { Access::At(Level::Control) } else { row_access(Self::TAG) }
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        json!({ "query": self.query.as_deref().map(recto_core::clean_line), "run": self.run })
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(AgentError::new(
            "palette_needs_app",
            "The palette lists the app's commands, and this session has no app.",
            exit::FAILED,
        )
        .then("It works when `recto` is talking to the app."))
    }
    fn human(data: &Value) -> String {
        let mut out = String::new();
        if let Some(ran) = data["ran"].as_object() {
            let _ = writeln!(out, "Ran {}.", text(&ran["title"]));
        }
        let items = data["items"].as_array().map_or(&[][..], Vec::as_slice);
        if items.is_empty() {
            out.push_str("Nothing matches.");
        }
        for item in items {
            let _ = writeln!(
                out,
                "{}  ({}{}{})  {}",
                text(&item["title"]),
                text(&item["detail"]),
                if item["shortcut"].as_str().is_some_and(|key| !key.is_empty()) {
                    ", "
                } else {
                    ""
                },
                text(&item["shortcut"]),
                text(&item["id"])
            );
        }
        out.trim_end().to_string()
    }
}

/// Its `--run` does what the row does, so an update waits for it, as for any command that may change something.
impl OnlyAsks for Palette {
    fn only_asks(&self) -> bool {
        false
    }
}

/// `on`, `off` or `toggle` for something hidden or not: whether it's hidden after, `toggle` going by how it is now, when it runs (R2.3: a toggle worked out
/// from what the window last showed lost a quick second press).
fn hidden_after(word: Option<&str>, hidden: bool, what: &str) -> Result<bool, AgentError> {
    match word {
        None => Ok(hidden),
        Some("on") => Ok(false),
        Some("off") => Ok(true),
        Some("toggle") => Ok(!hidden),
        Some(other) => Err(AgentError::new(
            "usage",
            format!("`{}` isn't for {what}. It's `on`, `off` or `toggle`.", recto_core::clean_line(other)),
            exit::USAGE,
        )),
    }
}

/// Sets what's asked, and says how things are shown now. `[paper, sidebar, agent_icon, layout]`, as `view` has them, and the tab the layout is for, if not the
/// active one. A change is announced only if something changed.
pub(crate) fn set<A: SessionAccess>(access: &A, asked: [&Option<String>; 4], tab: Option<&str>, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let [paper, sidebar, agent_icon, layout] = asked;
    access.with(|s| {
            let was = s.view();
            let laid_out = match tab {
                Some(selector) => Some(select(s, selector)?),
                None => s.active(),
            };
            if let Some(name) = layout.as_deref() {
                let spread = recto_session::record::Spread::from_name(name).ok_or_else(|| AgentError::new("usage", format!("`{}` isn't a layout. It's one of {}.", recto_core::clean_line(name), recto_session::record::Spread::ALL.map(recto_session::record::Spread::name).join(", ")), exit::USAGE))?;
                let id = laid_out.ok_or_else(|| AgentError::new("no_active_tab", "There's no tab showing to lay out.", exit::NOT_FOUND).then("`recto open` or `recto tap` one first."))?;
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
            view.sidebar_hidden = hidden_after(sidebar.as_deref(), view.sidebar_hidden, "the sidebar")?;
            view.agent_icon_hidden = hidden_after(agent_icon.as_deref(), view.agent_icon_hidden, "the agent icon")?;
            s.set_view(view);
            if view != was {
                events.push(view_changed(view));
            }
            Ok(json!({ "paper": paper_name(view), "papers": recto_session::Paper::ALL.map(recto_session::Paper::name), "sidebar": !view.sidebar_hidden, "agent_icon": !view.agent_icon_hidden, "layout": laid_out.and_then(|id| s.spread_of(id)).map(recto_session::record::Spread::name) }))
        })
}

pub(crate) const ROWS_VIEW: &[Spec] = &[Spec {
    name: "view",
    tag: "view",
    group: GROUP_MOVE,
    summary: "How the app shows pages: the paper, the sidebar, the agent icon, the tab showing's layout and the zoom. Only what's on screen changes, never what's saved, drawn or copied. With no flags it only says. To change either needs Control.",
    args: &[
        Arg {
            key: "paper",
            flag: Some("--paper"),
            kind: Kind::Choice(&["white", "warm", "dark", "auto"]),
            required: false,
            help: "`white`, `warm`, `dark`, or `auto`, which goes with the window: white with a light one, dark with a dark one.",
        },
        Arg {
            key: "sidebar",
            flag: Some("--sidebar"),
            kind: Kind::Choice(&["on", "off", "toggle"]),
            required: false,
            help: "`off` hides the sidebar, as its button beside the window's controls does, and it's remembered. `toggle` shows it if it's hidden and hides it if not, as that button does.",
        },
        Arg {
            key: "agent_icon",
            flag: Some("--agent-icon"),
            kind: Kind::Choice(&["on", "off", "toggle"]),
            required: false,
            help: "`off` hides the agent icon, and it's remembered. `toggle` shows it if it's hidden and hides it if not, as View > Agent Icon does.",
        },
        Arg {
            key: "layout",
            flag: Some("--layout"),
            kind: Kind::Choice(&["single", "two-up", "two-up-cover"]),
            required: false,
            help: "The tab showing's layout: `single`, a page to a row; `two-up`, two side by side; `two-up-cover`, the first page alone, then two side by side. Each tab keeps its own.",
        },
        Arg {
            key: "tab",
            flag: Some("--tab"),
            kind: Kind::Tab,
            required: false,
            help: "The tab `--layout` lays out, and whose layout is said: `#3`, `active`, or part of its name. The tab showing if left out.",
        },
        Arg {
            key: "zoom",
            flag: Some("--zoom"),
            kind: Kind::Text,
            required: false,
            help: "`in`, `out`, `fit`, `actual`, a level such as `150%`, where 100% is the page's real size as the top left shows it, or a number, where 1 is the fitted width. Only in the app.",
        },
    ],
    data: "`{ paper, papers, sidebar, agent_icon, layout }`, and `zoom` with `zoom_level` in the app. `layout` is the tab's that `--tab` names, or the tab showing's.",
    emits: &["view_changed"],
    examples: &[
        "recto view",
        "recto view --paper warm",
        "recto view --sidebar off",
        "recto view --layout two-up",
        "recto view --layout single --tab #2",
        "recto view --sidebar toggle",
        "recto view --zoom in",
        "recto view --zoom 150%",
    ],
    ..BASE
}];

pub(crate) const ROWS_PALETTE: &[Spec] = &[Spec {
    name: "palette",
    tag: "palette",
    group: GROUP_MOVE,
    access: Access::At(Level::ReadOnly),
    summary: "⌘K's command palette in the app: what a few letters find among its commands and open tabs, best first, worded and ranked as the palette shows them. `--run` runs the first, as Return does, which needs Control.",
    args: &[
        Arg {
            key: "query",
            flag: None,
            kind: Kind::Text,
            required: false,
            help: "Letters of what's wanted, in order: `zi` finds Zoom In. Everything if left out.",
        },
        Arg {
            key: "run",
            flag: Some("--run"),
            kind: Kind::Switch,
            required: false,
            help: "Run the first, as Return in the palette does.",
        },
        Arg {
            key: "limit",
            flag: Some("--limit"),
            kind: Kind::Positive(50),
            required: false,
            help: "Most listed. 20 if left out.",
        },
    ],
    data: "`{ query, items: [ { id, title, detail, shortcut } ], ran: { id, title, host } }`. `id` is a menu item's id, as `qa menu` takes, or `tab:N`. `detail` is where it lives, as `View` or `Open tab`. Only what can be chosen now is listed. `host` is what the app was left to do, as `show_open_panel`, which a quiet run doesn't show.",
    examples: &["recto palette zoom", "recto palette \"hide sb\" --run", "recto palette --json"],
    ..BASE
}];

#[cfg(test)]
mod tests {
    use crate::*;
    use recto_core::{DocFacts, OpenError, Toc};
    use std::path::{Path, PathBuf};

    #[derive(Clone)]
    struct Fake;

    impl DocLoader for Fake {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts {
                page_count: 4,
                byte_len: 1,
                encrypted: false,
                version: "1.7".into(),
                title: None,
                toc: Toc::default(),
                renderer_only: false,
            })
        }
    }

    fn run(session: &mut Session<Fake>, line: &str) -> (Result<serde_json::Value, AgentError>, Vec<Event>) {
        let mut events = Vec::new();
        let command = parse(line).expect("a command");
        let access = std::sync::Mutex::new(std::mem::replace(session, Session::new(Fake)));
        let result = execute(&access, Actor::Agent(Level::Control), &command, &mut events);
        *session = access.into_inner().expect("not poisoned");
        (result, events)
    }

    /// Failure (R2.3, accident A3): a toggle is worked out from how things were last shown, so two presses before the first is shown hide the sidebar twice
    /// and the second is lost; or a toggle that's on/off changes nothing and still says it did. `toggle` goes by how it is when it runs, each time.
    #[test]
    fn a_toggle_goes_by_how_it_is_when_it_runs() {
        let mut session = Session::new(Fake);
        session.open(&PathBuf::from("/d/a.pdf")).unwrap();
        for (line, sidebar_hidden, agent_icon_hidden) in [
            (r#"{"cmd":"view","sidebar":"toggle"}"#, true, false),
            (r#"{"cmd":"view","sidebar":"toggle"}"#, false, false),
            (r#"{"cmd":"view","agent_icon":"toggle"}"#, false, true),
            (r#"{"cmd":"view","agent_icon":"toggle","sidebar":"toggle"}"#, true, false),
        ] {
            let (result, events) = run(&mut session, line);
            let answer = result.expect("done");
            assert_eq!(
                (session.view().sidebar_hidden, session.view().agent_icon_hidden),
                (sidebar_hidden, agent_icon_hidden),
                "{line}"
            );
            assert_eq!(answer["sidebar"], !sidebar_hidden, "{line}: the answer says how it is now");
            assert_eq!(
                events.iter().filter(|event| event.name() == "view_changed").count(),
                1,
                "{line}: one change, said once"
            );
        }
        let (_, events) = run(&mut session, r#"{"cmd":"view","sidebar":"off"}"#);
        assert!(events.is_empty(), "nothing changed, so nothing's said: {events:?}");
        let (result, _) = run(&mut session, r#"{"cmd":"view","sidebar":"flip"}"#);
        assert_eq!(result.expect_err("not a word for it").code, "usage");
    }

    /// Failure (R2.3, accident A4): a layout chosen on one tab lands on whichever tab is active by the time it runs, or `--tab` lays out the active tab and
    /// says that one's layout. With `--tab` it's that tab's, whatever's active, and the answer and the event name it.
    #[test]
    fn a_layout_lands_on_the_tab_it_names() {
        let mut session = Session::new(Fake);
        let first = session.open(&PathBuf::from("/d/a.pdf")).unwrap();
        let second = session.open(&PathBuf::from("/d/b.pdf")).unwrap();
        assert_eq!(session.active(), Some(second));
        let (result, events) = run(&mut session, &format!(r##"{{"cmd":"view","layout":"two-up","tab":"#{}"}}"##, first.0));
        assert_eq!(result.expect("done")["layout"], "two-up", "the named tab's layout is said");
        assert_eq!(session.spread_of(first), Some(recto_session::record::Spread::TwoUp));
        assert_eq!(
            session.spread_of(second),
            Some(recto_session::record::Spread::Single),
            "the active tab is left as it was"
        );
        assert!(matches!(events.as_slice(), [Event::LayoutChanged { tab, .. }] if *tab == first), "{events:?}");
        let (result, _) = run(&mut session, r##"{"cmd":"view","layout":"two-up","tab":"#99"}"##);
        assert_eq!(result.expect_err("no such tab").code, "no_such_tab");
        // Asking, with a tab, is asking: Read-only may.
        let asked = parse(&format!(r##"{{"cmd":"view","tab":"#{}"}}"##, first.0)).unwrap();
        assert_eq!(asked.access(), table::Access::At(Level::ReadOnly));
    }
}
