//! **The state record's commands:** `session show`, `session sleep`, `session save`, `session restore` and `session check`. Each host keeps its own record and its own lifecycle, so its route is the record's: a host answers them, and `execute` only says they need the long-running session.

#![allow(unused_imports)]
use super::{Cmd, Cx, needs_server, row_access};
use crate::human::*;
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;

/// What the state record holds and what's become of it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionShow {}

impl Cmd for SessionShow {
    const TAG: &'static str = "session_show";
    const ROWS: &'static [Spec] = ROWS_SESSION_SHOW;
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
        Runs::Record
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    /// Each host keeps its own record, and answers this itself.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        show_words(data)
    }
}

/// Puts a tab to sleep now, as being left alone for five minutes would.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSleep {
    #[serde(default = "crate::active_tab")]
    pub tab: String,
}

impl Cmd for SessionSleep {
    const TAG: &'static str = "session_sleep";
    const ROWS: &'static [Spec] = ROWS_SESSION_SLEEP;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    fn tab(&self) -> Option<&str> {
        Some(&self.tab)
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Record
    }
    fn log_details(&self) -> Value {
        json!({ "tab": super::log_tab(&self.tab) })
    }
    /// Each host keeps its own record, and answers this itself.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        serde_json::to_string_pretty(data).unwrap_or_default()
    }
}

/// Writes the state record now and reads it back.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSave {}

impl Cmd for SessionSave {
    const TAG: &'static str = "session_save";
    const ROWS: &'static [Spec] = ROWS_SESSION_SAVE;
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
        Runs::Record
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    /// Each host keeps its own record, and answers this itself.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        format!(
            "Saved and read back: {}, {} bytes of record and {} of journals, {} written, in {} ms.",
            count(number(&data["tabs"]), "tab", "tabs"),
            number(&data["record_bytes"]),
            number(&data["journal_bytes"]),
            number(&data["wrote_bytes"]),
            number(&data["total_ms"])
        )
    }
}

/// Puts the session back as the state record has it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRestore {}

impl Cmd for SessionRestore {
    const TAG: &'static str = "session_restore";
    const ROWS: &'static [Spec] = ROWS_SESSION_RESTORE;
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
        Runs::Record
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    /// Each host keeps its own record, and answers this itself.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        match data["notice"].as_str() {
            Some(notice) => format!(
                "{}\n{notice}",
                if data["restored"] == true {
                    format!("Restored {}.", count(number(&data["count"]), "tab", "tabs"))
                } else {
                    "Nothing was restored.".to_string()
                }
            ),
            None => {
                if data["restored"] == true {
                    format!("Restored {}.", count(number(&data["count"]), "tab", "tabs"))
                } else {
                    "There's no record to restore.".to_string()
                }
            }
        }
    }
}

/// Whether this is a safe moment to hand off to a new version.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCheck {}

impl Cmd for SessionCheck {
    const TAG: &'static str = "session_check";
    const ROWS: &'static [Spec] = ROWS_SESSION_CHECK;
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
        Runs::Record
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    /// Each host keeps its own record, and answers this itself.
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        {
            let mut out = if data["safe"] == true {
                "Safe to hand off.".to_string()
            } else {
                "Not now:".to_string()
            };
            if let Some(busy) = data["busy"].as_array() {
                for item in busy {
                    let _ = write!(out, "\n- {}", text(&item["message"]));
                }
            }
            out
        }
    }
}

fn show_words(data: &Value) -> String {
    let record = &data["record"];
    let mut out = if record["kept"] == true {
        format!(
            "State record: {} bytes, journals {} bytes.",
            number(&record["record_bytes"]),
            number(&record["journal_bytes"])
        )
    } else {
        "No state record yet.".to_string()
    };
    let _ = write!(out, "\n{} open.", count(number(&data["count"]), "tab", "tabs"));
    if let Some(notice) = record["notice"].as_str() {
        let _ = write!(out, "\n{notice}");
    }
    if let Some(tabs) = data["tabs"].as_array() {
        for tab in tabs {
            if let Some(notice) = tab["notice"]["message"].as_str() {
                let _ = write!(out, "\n#{}: {notice}", number(&tab["tab"]));
            }
        }
    }
    out
}

pub(crate) const ROWS_SESSION_SHOW: &[Spec] = &[Spec {
    name: "session show",
    tag: "session_show",
    group: GROUP_SESSION,
    summary: "What the state record holds, and what's become of each tab's file: moved, changed, missing or needing its password again. No more of a document than `tabs` gives.",
    data: "`{ record: { kept, holds_the_record, record_bytes, journal_bytes, writes, appends, notice }, count, active, tabs: [ { tab, page, pages, zoom, within_page, layout, edited, can_undo, can_redo, notice } ], find, edit_mode, paper, sidebar }`. Find's text is only there at Control. `within_page` is how far down `page` the view's top is, and null when the view's top is on another page, as side by side it can be.",
    examples: &["recto session show"],
    ..BASE
}];

pub(crate) const ROWS_SESSION_SLEEP: &[Spec] = &[Spec {
    name: "session sleep",
    tag: "session_sleep",
    group: GROUP_SESSION,
    access: Access::At(Level::Control),
    summary: "Puts a tab to sleep now, as five minutes left alone would: its sleep file's written, its journal's compacted, and its contents, edits, worker and tiles leave memory. A touch brings them back. Not the tab on screen, or one being worked on.",
    args: &[TAB],
    data: "`{ tab, dormant, list_asleep }`.",
    examples: &["recto session sleep --tab report"],
    ..BASE
}];

pub(crate) const ROWS_SESSION_SAVE: &[Spec] = &[Spec {
    name: "session save",
    tag: "session_save",
    group: GROUP_SESSION,
    access: Access::At(Level::Control),
    summary: "Writes the state record now and reads it back, returning an error if it doesn't match. Never touches a PDF. A session with no window keeps a record of its own, so it can't overwrite the app's.",
    data: "`{ saved, tabs, edited_tabs, record_bytes, journal_bytes, wrote_bytes, total_ms }`.",
    examples: &["recto session save"],
    client_wait_s: 60,
    ..BASE
}];

pub(crate) const ROWS_SESSION_RESTORE: &[Spec] = &[Spec {
    name: "session restore",
    tag: "session_restore",
    group: GROUP_SESSION,
    access: Access::At(Level::Control),
    summary: "Puts the session back as the state record has it, with every tab cold but the active one. Below Full it's refused if the session holds the user's own unsaved edits.",
    data: "`{ restored, count, active, notice }`.",
    examples: &["recto session restore"],
    client_wait_s: 60,
    ..BASE
}];

pub(crate) const ROWS_SESSION_CHECK: &[Spec] = &[Spec {
    name: "session check",
    tag: "session_check",
    group: GROUP_SESSION,
    summary: "Whether this is a safe moment to hand off to a new version: no sheet open, no command or file read under way. And how long since the user touched the app.",
    data: "`{ safe, busy: [ { code, message } ], seconds_since_input }`.",
    examples: &["recto session check"],
    ..BASE
}];
