//! `compare`: tabs side by side, with separate or synced scroll.

use super::{Cmd, Cx, row_access};
use crate::human::text;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};

/// Tabs side by side: which, and separate or synced scroll. With nothing, only says.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compare {
    /// Tabs by selector, with commas between: `#1,#3`.
    #[serde(default)]
    pub tabs: Option<String>,
    /// `on` for synced scroll, `off` for separate.
    #[serde(default)]
    pub sync: Option<String>,
    /// Back to one tab, the one being read.
    #[serde(default)]
    pub off: bool,
}

impl Cmd for Compare {
    const TAG: &'static str = "compare";
    const ROWS: &'static [Spec] = ROWS_COMPARE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    /// Asking is reading. Changing it is moving around.
    fn access(&self) -> Access {
        match self {
            Self {
                tabs: None,
                sync: None,
                off: false,
            } => row_access(Self::TAG),
            _ => Access::At(Level::Control),
        }
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        json!({ "sync": self.sync.as_deref().map(recto_core::clean_line), "off": self.off })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        compare(cx.access, self.tabs.as_deref(), self.sync.as_deref(), self.off, cx.events)
    }
    fn human(data: &Value) -> String {
        match data["panes"].as_array() {
            Some(panes) if !panes.is_empty() => {
                let list: Vec<String> = panes
                    .iter()
                    .map(|pane| {
                        format!(
                            "#{} {} ({}){}",
                            pane["tab"],
                            text(&pane["untrusted"]["name"]),
                            if pane["protected"] == true {
                                "protected".to_string()
                            } else {
                                format!("page {} of {}", pane["page"], pane["pages"])
                            },
                            if pane["reading"] == true { ", read" } else { "" }
                        )
                    })
                    .collect();
                format!(
                    "Side by side: {}. Scroll: {}.{}",
                    list.join(", "),
                    if data["sync"] == "on" { "synced" } else { "separate" },
                    data["note"].as_str().map(|note| format!(" {note}")).unwrap_or_default()
                )
            }
            _ => "One tab showing, no comparison.".to_string(),
        }
    }
}

/// The most selectors `compare --tabs` reads, so a long list can't be made to look up thousands of names. More than there can be panes, so too many is said in words.
const MAX_COMPARE_SELECTORS: usize = 8;

/// `recto compare`: what the sidebar's ⌘-click and the right-click menu's scroll items do, for an agent. Owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view, right click menu can enable separate scroll (default) and synced scroll. Zoom is always synced."
fn compare<A: SessionAccess>(access: &A, tabs: Option<&str>, sync: Option<&str>, off: bool, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let usage = |words: String| AgentError::new("usage", words, exit::USAGE);
    let refused = |error: recto_session::compare::CompareError| AgentError::new(error.code(), error.message(), exit::FAILED);
    if off && (tabs.is_some() || sync.is_some()) {
        return Err(usage("`--off` goes alone: it ends the comparison.".into()));
    }
    let sync = match sync {
        None => None,
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(other) => return Err(usage(format!("`{}` isn't for `--sync`. It's `on` or `off`.", recto_core::clean_line(other)))),
    };
    let mut changed = false;
    if off {
        changed |= access.with(|s| s.end_compare());
    }
    if let Some(list) = tabs {
        let selectors: Vec<&str> = list
            .split(',')
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .take(MAX_COMPARE_SELECTORS + 1)
            .collect();
        if selectors.len() > MAX_COMPARE_SELECTORS {
            return Err(refused(recto_session::compare::CompareError::TooMany));
        }
        let ids = access.with(|s| selectors.iter().map(|selector| select(s, selector)).collect::<Result<Vec<TabId>, AgentError>>())?;
        recto_session::compare::Comparison::check(&ids).map_err(refused)?;
        // Each pane is a live document, so each cold one is read here with the session free, as a tap's is, and none with it held.
        let reading = access.with(|s| s.active().filter(|active| ids.contains(active)).unwrap_or(ids[0]));
        let cold = access.with(|s| s.wake_for_compare(&ids));
        let loader = access.with(|s| s.loader().clone());
        for (id, path) in cold {
            let stamp = recto_platform::file_stamp(&path).ok();
            let loaded = loader.load(&path);
            access.with(|s| s.give_woken(id, loaded, stamp));
        }
        changed |= access.with(|s| s.compare(&ids, Some(reading))).map_err(refused)?;
    }
    if let Some(on) = sync {
        changed |= access.with(|s| s.set_compare_synced(on)).ok_or_else(|| {
            AgentError::new("not_comparing", "There's no comparison to sync.", exit::NOT_FOUND)
                .then("`recto compare --tabs \"#1,#2\"` puts two side by side first.")
        })?;
    }
    let after = access.with(|s| compare_json(s));
    if changed {
        events.push(access.with(|s| event::comparison_changed(s)));
    }
    Ok(after)
}

pub(crate) const ROWS_COMPARE: &[Spec] = &[Spec {
    name: "compare",
    tag: "compare",
    group: GROUP_MOVE,
    access: Access::At(Level::ReadOnly),
    summary: "Tabs side by side, as ⌘-clicking them in the sidebar does: each its own document in its own place, zoom always the same. Separate scroll, or synced, which keeps them page for page from where it went on. A plain `tap` goes back to one. With no flags it only says. To change it needs Control.",
    args: &[
        Arg {
            key: "tabs",
            flag: Some("--tabs"),
            kind: Kind::Text,
            required: false,
            help: "Two or three tabs, with commas between, left to right: `#1,#3`, or parts of their names. The active tab stays the one read if it's one of them, and otherwise the first.",
        },
        Arg {
            key: "sync",
            flag: Some("--sync"),
            kind: Kind::Choice(&["on", "off"]),
            required: false,
            help: "`on` for synced scroll, from where each pane is now, or `off` for separate, the default.",
        },
        Arg {
            key: "off",
            flag: Some("--off"),
            kind: Kind::Switch,
            required: false,
            help: "Back to one tab, the one being read.",
        },
    ],
    data: "`{ comparing, panes: [{ tab, page, pages, reading, untrusted: { name } }], sync, sync_from, note, max_panes }`. `note` says when synced documents differ in length.",
    emits: &["view_changed"],
    examples: &[
        "recto compare",
        "recto compare --tabs \"#1,#2\"",
        "recto compare --tabs \"#1,#2,#4\" --sync on",
        "recto compare --sync off",
        "recto compare --off",
    ],
    ..BASE
}];
