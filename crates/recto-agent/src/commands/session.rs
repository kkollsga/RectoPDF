//! The session itself: `status`, which always gets through, and `stop`. The server's own loop answers both.

use super::{Cmd, Cx, needs_server, row_access};
use crate::human::{number, text};
use crate::table::*;
use crate::{AgentError, Level, Runs, SessionAccess};
use serde::Deserialize;
use serde_json::Value;
use std::fmt::Write;

/// The session: is it there, which version, what level.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Status {}

impl Cmd for Status {
    const TAG: &'static str = "status";
    const ROWS: &'static [Spec] = ROWS_STATUS;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Server
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        status(data)
    }
}

/// Ends the session.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {}

impl Cmd for Stop {
    const TAG: &'static str = "stop";
    const ROWS: &'static [Spec] = ROWS_STOP;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        row_access(Self::TAG)
    }
    fn route(&self) -> Runs {
        Runs::Server
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(_: &Value) -> String {
        "The session is stopping.".to_string()
    }
}

/// Every command, in the order `recto help` shows them.
pub(crate) const ROWS_STATUS: &[Spec] = &[Spec {
    name: "status",
    tag: "status",
    group: GROUP_START,
    access: Access::At(Level::Off),
    summary: "Whether the session is running, and what access agents have.",
    data: STATUS_DATA,
    examples: &["recto status", "recto status --json"],
    client_wait_s: 5,
    ..BASE
}];

pub(crate) const ROWS_STOP: &[Spec] = &[Spec {
    name: "stop",
    tag: "stop",
    group: GROUP_SESSION,
    access: Access::At(Level::Control),
    summary: "Ends the session. Refused while a tab has unsaved or undone edits.",
    data: "`{ stopping }`.",
    examples: &["recto stop"],
    client_wait_s: 15,
    ..BASE
}];

fn status(data: &Value) -> String {
    let mut out = format!(
        "RectoPDF is running: process {}, version {}, API version {}.\nAgent access: {}.",
        number(&data["pid"]),
        text(&data["version"]),
        number(&data["api"]),
        text(&data["label"])
    );
    if let Some(tabs) = data["tabs"].as_u64() {
        let _ = write!(out, "\nOpen PDFs: {tabs}.");
    }
    if data["level"] == "off" {
        out.push_str("\nAt Off, only status, help, permission and skill commands work. `recto permission request read_only` asks the user for more.");
    }
    if let Some(warning) = data["warning"].as_str() {
        let _ = write!(out, "\nWarning: {warning}");
    }
    out
}
