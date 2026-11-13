//! **What happened in the session,** for `wait` and `watch`, as a type (review C6, step R2.1).
//!
//! One variant per event, with typed fields. Its wire name, its `watch --events` group and its JSON come from exhaustive `match`es with no `_` arm, so an event without a group, a name or a line doesn't compile. The functions below build events from what a session method says changed, once for an action, so the agent's commands and the app's actions say the same thing and no caller compares state before and after to find out.
//!
//! Holds no document text beyond a cleaned name, marked untrusted.

use recto_session::{Closed, DocLoader, Session, TabId};
use serde_json::{Value, json};

/// Something that happened in the session. Its line on the wire is [`Event::to_json`], and the tests in `tests/event_wire.rs` hold that to what it was before the events were a type.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A tab opened. `name` is the file's name, untrusted.
    TabOpened {
        tab: TabId,
        name: Option<String>,
    },
    /// Another tab is the one showing.
    TabActivated {
        tab: TabId,
        name: Option<String>,
    },
    TabClosed {
        tab: TabId,
    },
    /// The place in a tab moved, 1-based. `None` for a document opened with a password, whose place an agent isn't told.
    PageChanged {
        tab: TabId,
        page: Option<u32>,
    },
    /// A tab's edits changed. `pages` is `None` for a protected document.
    EditsChanged {
        tab: TabId,
        pages: Option<u32>,
        edited: bool,
    },
    Saved {
        tab: TabId,
        writer: &'static str,
    },
    /// A page was drawn, 1-based.
    PageDrawn {
        tab: TabId,
        page: u32,
    },
    PageFailed {
        tab: TabId,
        page: u32,
        code: &'static str,
    },
    IndexingStarted {
        corpus: String,
    },
    /// `code` and `pages` are the helper's own report, passed on as it gave them.
    IndexingFinished {
        corpus: String,
        state: String,
        code: Value,
        pages: Value,
    },
    ContentsGenerating {
        file: String,
    },
    /// `elapsed_ms` and `helper_peak_bytes` are the helper's own numbers, passed on as it gave them.
    ContentsGenerated {
        file: String,
        state: &'static str,
        code: Option<String>,
        elapsed_ms: Value,
        helper_peak_bytes: Value,
    },
    LevelChanged {
        level: &'static str,
        was: &'static str,
    },
    /// How pages are shown: the paper and whether the sidebar is.
    ViewSettingsChanged {
        paper: &'static str,
        sidebar: bool,
    },
    /// A tab's layout, as `recto view --layout` names it.
    LayoutChanged {
        tab: TabId,
        layout: &'static str,
    },
    /// The tabs side by side (none when it ended), and whether their scroll is synced.
    ComparisonChanged {
        panes: Vec<TabId>,
        synced: bool,
    },
    /// Find is looking.
    FindRunning,
    /// Find is open, at a hit's page (1-based) if it has gone to one.
    FindOpened {
        page: Option<u32>,
    },
    FindClosed,
    FindFinished {
        hits: u64,
    },
}

impl Event {
    /// The name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            Self::TabOpened { .. } => "tab_opened",
            Self::TabActivated { .. } => "tab_activated",
            Self::TabClosed { .. } => "tab_closed",
            Self::PageChanged { .. } => "page_changed",
            Self::EditsChanged { .. } => "edits_changed",
            Self::Saved { .. } => "saved",
            Self::PageDrawn { .. } => "page_drawn",
            Self::PageFailed { .. } => "page_failed",
            Self::IndexingStarted { .. } => "indexing_started",
            Self::IndexingFinished { .. } => "indexing_finished",
            Self::ContentsGenerating { .. } => "contents_generating",
            Self::ContentsGenerated { .. } => "contents_generated",
            Self::LevelChanged { .. } => "level_changed",
            Self::ViewSettingsChanged { .. } | Self::LayoutChanged { .. } | Self::ComparisonChanged { .. } => "view_changed",
            Self::FindRunning | Self::FindOpened { .. } | Self::FindClosed => "find_changed",
            Self::FindFinished { .. } => "find_finished",
        }
    }

    /// The group `watch --events` selects it by: one of [`crate::table::EVENT_GROUPS`]. **No default** (review C6): a new variant doesn't compile until it has one.
    pub fn group(&self) -> &'static str {
        match self {
            Self::TabOpened { .. } | Self::TabActivated { .. } | Self::TabClosed { .. } => "tabs",
            Self::PageChanged { .. } => "pages",
            Self::EditsChanged { .. } | Self::Saved { .. } => "edits",
            Self::PageDrawn { .. } | Self::PageFailed { .. } => "draws",
            Self::IndexingStarted { .. } | Self::IndexingFinished { .. } => "indexing",
            Self::ContentsGenerating { .. } | Self::ContentsGenerated { .. } => "contents",
            Self::LevelChanged { .. } => "access",
            Self::ViewSettingsChanged { .. } | Self::LayoutChanged { .. } | Self::ComparisonChanged { .. } => "view",
            Self::FindRunning | Self::FindOpened { .. } | Self::FindClosed | Self::FindFinished { .. } => "find",
        }
    }

    /// The line a watcher receives.
    pub fn to_json(&self) -> Value {
        let mut out = match self {
            Self::TabOpened { tab, name } | Self::TabActivated { tab, name } => json!({ "tab": tab.0, "untrusted": { "name": name } }),
            Self::TabClosed { tab } => json!({ "tab": tab.0 }),
            Self::PageChanged { tab, page } => json!({ "tab": tab.0, "page": page }),
            Self::EditsChanged { tab, pages, edited } => json!({ "tab": tab.0, "pages": pages, "edited": edited }),
            Self::Saved { tab, writer } => json!({ "tab": tab.0, "writer": writer }),
            Self::PageDrawn { tab, page } => json!({ "tab": tab.0, "page": page }),
            Self::PageFailed { tab, page, code } => json!({ "tab": tab.0, "page": page, "code": code }),
            Self::IndexingStarted { corpus } => json!({ "corpus": corpus }),
            Self::IndexingFinished { corpus, state, code, pages } => json!({ "corpus": corpus, "state": state, "code": code, "pages": pages }),
            Self::ContentsGenerating { file } => json!({ "file": file }),
            Self::ContentsGenerated {
                file,
                state,
                code,
                elapsed_ms,
                helper_peak_bytes,
            } => json!({ "file": file, "state": state, "code": code, "elapsed_ms": elapsed_ms, "helper_peak_bytes": helper_peak_bytes }),
            Self::LevelChanged { level, was } => json!({ "level": level, "was": was }),
            Self::ViewSettingsChanged { paper, sidebar } => json!({ "paper": paper, "sidebar": sidebar }),
            Self::LayoutChanged { tab, layout } => json!({ "tab": tab.0, "layout": layout }),
            Self::ComparisonChanged { panes, synced } => {
                json!({ "comparison": panes.iter().map(|tab| tab.0).collect::<Vec<_>>(), "sync": if *synced { "on" } else { "off" } })
            }
            Self::FindRunning => json!({ "running": true }),
            Self::FindOpened { page: None } => json!({ "open": true }),
            Self::FindOpened { page: Some(page) } => json!({ "open": true, "page": page }),
            Self::FindClosed => json!({ "open": false }),
            Self::FindFinished { hits } => json!({ "hits": hits }),
        };
        out["event"] = json!(self.name());
        out
    }
}

/// A tab's name for an event: the file's, untrusted.
fn name_of<L: DocLoader>(session: &Session<L>, id: TabId) -> Option<String> {
    session.tabs().tab(id).map(|tab| tab.name.to_string())
}

/// A tab was opened.
pub fn tab_opened<L: DocLoader>(session: &Session<L>, id: TabId) -> Event {
    Event::TabOpened {
        tab: id,
        name: name_of(session, id),
    }
}

/// A tab is the one showing.
pub fn tab_activated<L: DocLoader>(session: &Session<L>, id: TabId) -> Event {
    Event::TabActivated {
        tab: id,
        name: name_of(session, id),
    }
}

/// **What watchers hear when a tab closes,** wherever it was closed from: the close, and the tab that took its place, if one did. Built from what the session said changed.
pub fn closed<L: DocLoader>(session: &Session<L>, change: Closed, events: &mut Vec<Event>) {
    events.push(Event::TabClosed { tab: change.id });
    if let Some(now) = change.activated {
        events.push(tab_activated(session, now));
    }
}

/// **What watchers hear when a tap or a click switches the tab showing:** `switched` is what the session said, the tab now showing if it wasn't already.
pub fn switched<L: DocLoader>(session: &Session<L>, switched: Option<TabId>, events: &mut Vec<Event>) {
    if let Some(now) = switched {
        events.push(tab_activated(session, now));
    }
}

/// What watchers are told when the person moves to another page: which tab, and its page, unless it's a protected document's, whose place was read with its password.
pub fn page_changed<L: DocLoader>(session: &Session<L>, id: TabId, page: u32) -> Event {
    let page = (!session.tabs().withheld_from_agents(id)).then_some(page);
    Event::PageChanged { tab: id, page }
}

/// The same, for the page the tab is on now: after a jump, a step back, a go-to or a link, the page the session landed on.
pub fn page_now<L: DocLoader>(session: &Session<L>, id: TabId) -> Event {
    page_changed(session, id, session.tabs().tab(id).map_or(1, |tab| tab.page + 1))
}

/// What watchers are told when a tab's edits change, wherever they were changed from: an agent, the panel or the Edit menu.
pub fn edits_changed<L: DocLoader>(session: &Session<L>, id: TabId) -> Event {
    let pages = (!session.tabs().withheld_from_agents(id)).then(|| session.pages_in(id));
    Event::EditsChanged {
        tab: id,
        pages,
        edited: session.is_edited(id),
    }
}

/// A save and what it left of the tab's edits.
pub fn saved<L: DocLoader>(session: &Session<L>, id: TabId, writer: &'static str) -> [Event; 2] {
    [Event::Saved { tab: id, writer }, edits_changed(session, id)]
}

/// The access level changed.
pub fn level_changed(level: crate::Level, was: crate::Level) -> Event {
    Event::LevelChanged {
        level: level.name(),
        was: was.name(),
    }
}

/// What watchers are told when how pages are shown changes: the same words wherever it was changed from.
pub fn view_changed(view: recto_session::ViewSettings) -> Event {
    Event::ViewSettingsChanged {
        paper: view.paper.map_or("auto", recto_session::Paper::name),
        sidebar: !view.sidebar_hidden,
    }
}

/// What watchers are told when a comparison began, changed or ended, or its scroll was synced or set apart.
pub fn comparison_changed<L: DocLoader>(session: &Session<L>) -> Event {
    let comparison = session.comparison();
    Event::ComparisonChanged {
        panes: comparison.map(|comparison| comparison.tabs.clone()).unwrap_or_default(),
        synced: comparison.is_some_and(|comparison| comparison.synced),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of each variant, with the group it must be in. The `match` makes a new variant fail to compile here until it has a sample.
    fn samples() -> Vec<Event> {
        let tab = TabId(1);
        let samples = vec![
            Event::TabOpened { tab, name: None },
            Event::TabActivated { tab, name: None },
            Event::TabClosed { tab },
            Event::PageChanged { tab, page: None },
            Event::EditsChanged {
                tab,
                pages: None,
                edited: false,
            },
            Event::Saved { tab, writer: "lopdf" },
            Event::PageDrawn { tab, page: 1 },
            Event::PageFailed { tab, page: 1, code: "x" },
            Event::IndexingStarted { corpus: String::new() },
            Event::IndexingFinished {
                corpus: String::new(),
                state: String::new(),
                code: Value::Null,
                pages: Value::Null,
            },
            Event::ContentsGenerating { file: String::new() },
            Event::ContentsGenerated {
                file: String::new(),
                state: "done",
                code: None,
                elapsed_ms: Value::Null,
                helper_peak_bytes: Value::Null,
            },
            Event::LevelChanged { level: "off", was: "off" },
            Event::ViewSettingsChanged { paper: "auto", sidebar: true },
            Event::LayoutChanged { tab, layout: "single" },
            Event::ComparisonChanged {
                panes: Vec::new(),
                synced: false,
            },
            Event::FindRunning,
            Event::FindOpened { page: None },
            Event::FindClosed,
            Event::FindFinished { hits: 0 },
        ];
        for event in &samples {
            match event {
                Event::TabOpened { .. }
                | Event::TabActivated { .. }
                | Event::TabClosed { .. }
                | Event::PageChanged { .. }
                | Event::EditsChanged { .. }
                | Event::Saved { .. }
                | Event::PageDrawn { .. }
                | Event::PageFailed { .. }
                | Event::IndexingStarted { .. }
                | Event::IndexingFinished { .. }
                | Event::ContentsGenerating { .. }
                | Event::ContentsGenerated { .. }
                | Event::LevelChanged { .. }
                | Event::ViewSettingsChanged { .. }
                | Event::LayoutChanged { .. }
                | Event::ComparisonChanged { .. }
                | Event::FindRunning
                | Event::FindOpened { .. }
                | Event::FindClosed
                | Event::FindFinished { .. } => {}
            }
        }
        samples
    }

    /// Failure: an event's name or group is in the code and not in the table the reference and `--events` read, so a watcher is told of an event it can't select, or the reference lists one that never comes. The two lists hold exactly the same names and groups.
    #[test]
    fn the_table_of_events_is_the_types() {
        let mut from_type: Vec<(&str, &str)> = samples().iter().map(|event| (event.name(), event.group())).collect();
        from_type.sort_unstable();
        from_type.dedup();
        let mut listed: Vec<(&str, &str)> = crate::table::EVENTS.to_vec();
        listed.sort_unstable();
        assert_eq!(from_type, listed);
        for (_, group) in &listed {
            assert!(crate::table::EVENT_GROUPS.contains(group), "{group} isn't a group `--events` takes");
        }
        for group in crate::table::EVENT_GROUPS {
            assert!(listed.iter().any(|(_, listed)| listed == group), "{group} has no events");
        }
        let reference = crate::skill::reference_md();
        for (event, _) in &listed {
            assert!(reference.contains(&format!("`{event}`")), "{event} is in the reference");
        }
    }

    /// Failure: a line loses its `event` name, so a watcher can't tell what it was sent.
    #[test]
    fn every_line_names_its_event() {
        for event in samples() {
            assert_eq!(event.to_json()["event"], event.name());
        }
    }
}
