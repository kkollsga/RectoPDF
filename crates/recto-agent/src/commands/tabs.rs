//! The list of tabs and files: `recent`, `recent clear` and `close`.

#![allow(unused_imports)]
use super::{Cmd, Cx, row_access};
use crate::human::*;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;
use std::path::PathBuf;

/// Files opened lately, as File > Open Recent lists them.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recent {}

impl Cmd for Recent {
    const TAG: &'static str = "recent";
    const ROWS: &'static [Spec] = ROWS_RECENT;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let (access, level) = (cx.access, cx.level);
        Ok(access.with(|s| recent_json(s, level)))
    }
    fn human(data: &Value) -> String {
        match data["files"].as_array().filter(|files| !files.is_empty()) {
            Some(files) => {
                let mut out = format!("{} opened lately, newest first:", count(files.len() as u64, "file", "files"));
                for file in files {
                    let open = file["open_tab"].as_str().map(|tab| format!(", open as {tab}")).unwrap_or_default();
                    let path = file["untrusted"]["path"].as_str().map(|path| format!(" at {path:?}")).unwrap_or_default();
                    let _ = write!(out, "\n{}. {:?}{path}{open}", number(&file["file"]), text(&file["untrusted"]["name"]));
                }
                let _ = write!(out, "\n{QUOTED_NAMES}");
                out
            }
            None => "No files opened lately.".to_string(),
        }
    }
}

/// Clears that list, as Open Recent > Clear Menu does.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentClear {}

impl Cmd for RecentClear {
    const TAG: &'static str = "recent_clear";
    const ROWS: &'static [Spec] = ROWS_RECENT_CLEAR;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let access = cx.access;
        access.with(|s| {
            let cleared = s.clear_recent();
            Ok(json!({ "cleared": cleared }))
        })
    }
    fn human(data: &Value) -> String {
        if data["cleared"] == true {
            "Cleared the list of files opened lately."
        } else {
            "There were no files opened lately to clear."
        }
        .to_string()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Close {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for Close {
    const TAG: &'static str = "close";
    const ROWS: &'static [Spec] = ROWS_CLOSE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = true;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Session
    }
    fn log_details(&self) -> Value {
        let Self { tab } = self;
        json!({ "tab": super::log_tab(tab) })
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let Self { tab } = self;
        let (access, level, events) = (cx.access, cx.level, &mut *cx.events);
        {
            // What would refuse it comes first: no reading a file for a close that won't happen.
            let id = access.with(|s| {
                let id = select(s, tab)?;
                refuse_close_that_loses_something(s, id, level)?;
                Ok::<_, AgentError>(id)
            })?;
            read_ahead(access, |s| s.wake_for_close(id));
            access.with(|s| {
                let id = select(s, tab)?;
                refuse_close_that_loses_something(s, id, level)?;
                // At Full, what's lost is the holder's call, and `refuse_close_that_loses_something` has let it through. The session itself won't lose the owner's undone steps unasked.
                if level >= Level::Full && s.has_undone_owner_steps(id) {
                    s.discard_edits(id);
                }
                let closed = s.close(id)?;
                event::closed(s, closed, events);
                // Not the whole list: closing a thousand tabs shouldn't build a thousand lists.
                Ok(json!({ "closed": id.0, "count": s.tabs().len(), "active": s.active().map(|active| where_json(s, active)) }))
            })
        }
    }
    fn human(data: &Value) -> String {
        {
            let mut out = format!("Closed #{}. {} open.", number(&data["closed"]), count(number(&data["count"]), "tab", "tabs"));
            if data["active"].is_object() {
                let _ = write!(out, "\nActive: {}\n{QUOTED_NAMES}", tab_line(&data["active"]));
            }
            out
        }
    }
}

pub(crate) const ROWS_RECENT: &[Spec] = &[Spec {
    name: "recent",
    tag: "recent",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Files opened lately, newest first, as File > Open Recent lists them, with the tab each is open in and its whole path, which `recto open` can open. Needs Control: what someone's been reading, closed files included, is theirs, and more than Read-only's look at what's open.",
    data: "`{ count, files: [ { file, open_tab, untrusted: { name, path } } ], paths_shown }`. At most 20.",
    examples: &["recto recent", "recto recent --json"],
    ..BASE
}];

pub(crate) const ROWS_RECENT_CLEAR: &[Spec] = &[Spec {
    name: "recent clear",
    tag: "recent_clear",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Empties the list of files opened lately, as Open Recent > Clear Menu does. Opens and closes nothing.",
    data: "`{ cleared }`: whether there was anything.",
    examples: &["recto recent clear"],
    ..BASE
}];

pub(crate) const ROWS_CLOSE: &[Spec] = &[Spec {
    name: "close",
    tag: "close",
    group: GROUP_MOVE,
    access: Access::At(Level::Control),
    summary: "Closes a tab. Refused while it has unsaved edits, so `recto discard` first, and below Full if undone steps of the user's would go. Undone steps of an agent's own go with it. The refusal says what's at stake and each way on.",
    args: &[TAB],
    data: "`{ closed, count, active }`, with `active` as `where` gives it.",
    emits: &["tab_closed", "tab_activated"],
    examples: &["recto close", "recto close --tab \"#2\""],
    ..BASE
}];
