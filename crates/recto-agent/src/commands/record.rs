//! **The state record's commands:** `session show`, `session sleep`, `session save`, `session restore` and `session check`. Each host keeps its own record and its own lifecycle, which it gives these through [`Records`]; every answer, and the refusal that keeps the owner's edits, is made here, once, for both. `execute` runs them right after the level check: before the protected check, as none reads a document, and before the hold, which would keep `session sleep`'s tab awake.

#![allow(unused_imports)]
use super::{Cmd, Cx, OnlyAsks, needs_server, row_access};
use crate::Code;
use crate::human::*;
use crate::table::*;
use crate::*;
use recto_session::TabId;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;

/// What the state record holds and what's become of it.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionShow {}

impl Cmd for SessionShow {
    const TAG: &'static str = "session_show";
    const ROWS: &'static [Spec] = ROWS_SESSION_SHOW;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
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
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        // What the record says is the host's to look up, and isn't asked with the session held.
        let kept = cx.access.records()?.kept();
        Ok(cx.access.with(|session| session_show_json(session, cx.level, kept)))
    }
    fn human(data: &Value) -> String {
        show_words(data)
    }
}

/// Puts a tab to sleep now, as being left alone for five minutes would.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSleep {
    #[serde(default)]
    pub tab: TabRef,
}

impl Cmd for SessionSleep {
    const TAG: &'static str = "session_sleep";
    const ROWS: &'static [Spec] = ROWS_SESSION_SLEEP;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Its;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
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
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let records = cx.access.records()?;
        let id = cx.access.with(|session| select(session, &self.tab))?;
        let slept = records.put_to_sleep(id, cx.actor)?;
        let mut answer = json!({ "tab": id.0, "dormant": slept.dormant, "list_asleep": slept.list_asleep });
        if let Some(note) = slept.note {
            answer["note"] = json!(note);
        }
        Ok(answer)
    }
    fn human(data: &Value) -> String {
        serde_json::to_string_pretty(data).unwrap_or_default()
    }
}

/// Writes the state record now and reads it back.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSave {}

impl Cmd for SessionSave {
    const TAG: &'static str = "session_save";
    const ROWS: &'static [Spec] = ROWS_SESSION_SAVE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
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
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let saved = cx.access.records()?.save(cx.actor)?;
        let mut answer = json!({ "saved": true, "tabs": saved.tabs, "edited_tabs": saved.edited_tabs, "record_bytes": saved.record_bytes, "journal_bytes": saved.journal_bytes, "wrote_bytes": saved.wrote_bytes, "total_ms": saved.total_us / 1000 });
        if let Some(timings) = saved.timings {
            answer["total_us"] = json!(saved.total_us);
            answer["write_us"] = json!(timings.write_us);
            answer["force_us"] = json!(timings.force_us);
            answer["verify_us"] = json!(timings.verify_us);
        }
        if let Some(note) = saved.note {
            answer["note"] = json!(note);
        }
        Ok(answer)
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
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRestore {}

impl Cmd for SessionRestore {
    const TAG: &'static str = "session_restore";
    const ROWS: &'static [Spec] = ROWS_SESSION_RESTORE;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
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
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let records = cx.access.records()?;
        if cx.level < Level::Full && cx.access.with(|session| session.lists().holds_owner_edits()) {
            return Err(restore_would_lose_owners_edits());
        }
        let restored = records.restore()?;
        let (count, active) = cx.access.with(|session| (session.tabs().len(), session.tabs().active().map(|id| id.0)));
        Ok(json!({ "restored": restored.restored, "count": count, "active": active, "notice": restored.notice }))
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
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCheck {}

impl Cmd for SessionCheck {
    const TAG: &'static str = "session_check";
    const ROWS: &'static [Spec] = ROWS_SESSION_CHECK;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
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
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        let check = cx.access.records()?.hand_off();
        let busy: Vec<Value> = check.busy.iter().map(|(code, message)| json!({ "code": code, "message": message })).collect();
        Ok(json!({ "safe": check.safe, "busy": busy, "seconds_since_input": check.seconds_since_input }))
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

impl super::Record {
    /// Whether it only asks: looking, writing the record and the hand-off check don't change the session an update carries across; sleeping and restoring do.
    pub fn only_asks(&self) -> bool {
        match self {
            Self::SessionShow(command) => command.only_asks(),
            Self::SessionSleep(command) => command.only_asks(),
            Self::SessionSave(command) => command.only_asks(),
            Self::SessionRestore(command) => command.only_asks(),
            Self::SessionCheck(command) => command.only_asks(),
        }
    }
}

impl OnlyAsks for SessionShow {
    fn only_asks(&self) -> bool {
        true
    }
}

impl OnlyAsks for SessionSleep {
    fn only_asks(&self) -> bool {
        false
    }
}

impl OnlyAsks for SessionSave {
    fn only_asks(&self) -> bool {
        true
    }
}

impl OnlyAsks for SessionRestore {
    fn only_asks(&self) -> bool {
        false
    }
}

impl OnlyAsks for SessionCheck {
    fn only_asks(&self) -> bool {
        true
    }
}

/// **What a host keeps of the state record**, for its five commands. Each host has its own: the app writes its record as it goes and sleeps tabs through its sleeper; a session with no window writes only when told to. Reached through [`SessionAccess::records`]. Each is called with nothing held, the session's lock included, as a host's own work can wait on its session thread.
pub trait Records {
    /// What's known of the record, for `session show`: numbers, and Rust's own words.
    fn kept(&self) -> Value;
    /// Puts a tab to sleep now, as five minutes left alone would, for whoever asked (an agent's sleep files are held to its share of the hour).
    fn put_to_sleep(&self, tab: TabId, actor: Actor) -> Result<Slept, AgentError>;
    /// Writes the record now and reads it back, for whoever asked (an agent's leaves the sleep files waiting their cool-down to wait).
    fn save(&self, actor: Actor) -> Result<Saved, AgentError>;
    /// Puts the session back as the record has it. The owner's edits were asked about first.
    fn restore(&self) -> Result<Restored, AgentError>;
    /// Whether this is a safe moment to hand off to a new version.
    fn hand_off(&self) -> HandOff;
}

/// What became of a tab put to sleep.
pub struct Slept {
    pub dormant: bool,
    pub list_asleep: bool,
    /// What the asker is told of the tab's sleep file, in the host's words, when it wasn't written for a reason the asker can act on.
    pub note: Option<String>,
}

/// A record written and read back.
pub struct Saved {
    pub tabs: usize,
    pub edited_tabs: usize,
    pub record_bytes: u64,
    pub journal_bytes: u64,
    pub wrote_bytes: u64,
    pub total_us: u64,
    /// Where the time went, for a host that measures it.
    pub timings: Option<SaveTimings>,
    /// What the asker is told of the sleep file of the tab on screen, in the host's words, when it wasn't written for a reason the asker can act on.
    pub note: Option<String>,
}

/// Where a save's time went.
pub struct SaveTimings {
    pub write_us: u64,
    pub force_us: u64,
    pub verify_us: u64,
}

/// What a restore found.
pub struct Restored {
    pub restored: bool,
    /// What the record's reading told the reader, in Rust's words.
    pub notice: Option<&'static str>,
}

/// Whether a hand-off could go now, and what's in its way: `(code, message)`.
pub struct HandOff {
    pub safe: bool,
    pub busy: Vec<(String, String)>,
    /// Since the user last touched the app, for a host that has a user.
    pub seconds_since_input: Option<f64>,
}

/// A long-running session that keeps no record of its own.
pub fn no_record() -> AgentError {
    AgentError::new(Code::NO_RECORD, "This session doesn't keep a state record.", exit::FAILED)
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
