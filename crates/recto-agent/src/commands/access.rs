//! Access and what agents have done: `permission show`, `permission request`, `log`, `sign` and `level changed`. The server's own loop answers all but `sign`, which nobody can run.

use super::{Cmd, Cx, needs_server, row_access};
use crate::TabRef;
use crate::human::text;
use crate::table::*;
use crate::{AgentError, Level, Runs, SessionAccess};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fmt::Write;

#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionShow {}

impl Cmd for PermissionShow {
    const TAG: &'static str = "permission_show";
    const ROWS: &'static [Spec] = ROWS_PERMISSION_SHOW;
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = false;
    fn tab(&self) -> Option<&TabRef> {
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
        permission_show(data)
    }
}

/// Asks the user for more access. Grants nothing by itself.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRequest {
    pub level: Level,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Cmd for PermissionRequest {
    const TAG: &'static str = "permission_request";
    const ROWS: &'static [Spec] = ROWS_PERMISSION_REQUEST;
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
        Runs::Server
    }
    fn log_details(&self) -> Value {
        json!({ "level": self.level.name() })
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        permission_request(data)
    }
}

/// What agents have done, newest last.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Log {
    #[serde(default)]
    pub last: Option<u32>,
}

impl Cmd for Log {
    const TAG: &'static str = "log";
    const ROWS: &'static [Spec] = ROWS_LOG;
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
        Runs::Server
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        log(data)
    }
}

/// Never reachable by an agent. It exists to say so.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sign {}

impl Cmd for Sign {
    const TAG: &'static str = "sign";
    const ROWS: &'static [Spec] = ROWS_SIGN;
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
        Runs::Session
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, cx: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(crate::refusal("sign", cx.level, Access::Nobody))
    }
    fn human(data: &Value) -> String {
        crate::human::pretty(data)
    }
}

/// The level's file changed: read it again. Carries no level, so it can't raise one.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct LevelChanged {}

impl Cmd for LevelChanged {
    const TAG: &'static str = "level_changed";
    const ROWS: &'static [Spec] = ROWS_LEVEL_CHANGED;
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
        Runs::Server
    }
    fn log_details(&self) -> Value {
        Value::Null
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(needs_server())
    }
    fn human(data: &Value) -> String {
        crate::human::pretty(data)
    }
}

pub(crate) const ROWS_PERMISSION_SHOW: &[Spec] = &[Spec {
    name: "permission show",
    tag: "permission_show",
    group: GROUP_ACCESS,
    access: Access::At(Level::Off),
    summary: "What access agents have now, and what each level allows.",
    data: "`{ level, label, levels: [ { level, label, allows } ], how_to_change, set_at, pending_request }`.",
    examples: &["recto permission show", "recto permission show --json"],
    client_wait_s: 5,
    ..BASE
}];

pub(crate) const ROWS_PERMISSION_REQUEST: &[Spec] = &[Spec {
    name: "permission request",
    tag: "permission_request",
    group: GROUP_ACCESS,
    access: Access::At(Level::Off),
    summary: "Asks the user for more access. It grants nothing and answers at once: tell the user what it prints, and carry on with what you can. Use `--timeout` only when someone's at the keyboard to answer, since it blocks until they do.",
    args: &[
        Arg {
            key: "level",
            flag: None,
            kind: Kind::Level,
            required: true,
            help: "`read_only`, `control` or `full`.",
        },
        Arg {
            key: "timeout_ms",
            flag: Some("--timeout"),
            kind: Kind::Duration,
            required: false,
            help: "Only when someone's at the keyboard to answer: how long to block waiting for them, such as `30s` or `5m`. Answers at once if left out, which is nearly always what you want.",
        },
    ],
    data: "`{ level, granted, asked, tell_the_user, command, what_to_do }`.",
    examples: &["recto permission request read_only", "recto permission request control --timeout 5m"],
    ..BASE
}];

pub(crate) const ROWS_LOG: &[Spec] = &[Spec {
    name: "log",
    tag: "log",
    group: GROUP_ACCESS,
    summary: "What agents have done: time, command, level and outcome, oldest first. Never document text. At a real terminal it needs no session and no level.",
    args: &[Arg {
        key: "last",
        flag: Some("--last"),
        kind: Kind::Positive(5000),
        required: false,
        help: "How many entries, from the newest. 50 if left out.",
    }],
    data: "`{ entries: [ { time, t_ms, cmd, level, outcome, details, count, since } ], kept_bytes, file }`. `count` means that many more of the same since `since`: `status`, bad lines and refusals are counted rather than written each time, so a flood of them can't push out what happened.",
    examples: &["recto log", "recto log --last 10 --json"],
    ..BASE
}];

pub(crate) const ROWS_SIGN: &[Spec] = &[Spec {
    name: "sign",
    tag: "sign",
    group: GROUP_ACCESS,
    access: Access::Nobody,
    summary: "Signing isn't open to agents at any level. Only the user can sign, in the app. This always refuses, and `signatures` is what reports them.",
    data: "Always refuses with `sign_refused`, exit 5.",
    examples: &["recto sign"],
    ..BASE
}];

pub(crate) const ROWS_LEVEL_CHANGED: &[Spec] = &[Spec {
    name: "level changed",
    tag: "level_changed",
    group: GROUP_ACCESS,
    access: Access::At(Level::Off),
    summary: "Tells the session to read the level's file again. Carries no level.",
    data: "`{ level }`.",
    client_wait_s: 5,
    hidden: true,
    ..BASE
}];

fn permission_show(data: &Value) -> String {
    let mut out = format!("Agent access: {}.\n", text(&data["label"]));
    for level in data["levels"].as_array().map_or(&[][..], Vec::as_slice) {
        let _ = writeln!(out, "  {:<10} {}", text(&level["label"]), text(&level["allows"]));
    }
    let _ = write!(out, "{}", text(&data["how_to_change"]));
    if let Some(pending) = data["pending_request"].as_str() {
        let _ = write!(out, "\nAn agent has asked for: {pending}.");
    }
    out
}

fn permission_request(data: &Value) -> String {
    if data["granted"] == true {
        format!("Agent access is already {}.", text(&data["level"]))
    } else {
        format!(
            "{}\nThe command for the user: {}\n{}",
            text(&data["tell_the_user"]),
            text(&data["command"]),
            text(&data["what_to_do"])
        )
    }
}

fn log(data: &Value) -> String {
    let rows = data["entries"].as_array().map_or(&[][..], Vec::as_slice);
    if rows.is_empty() {
        return "Nothing logged yet.".to_string();
    }
    let mut out = String::new();
    for row in rows {
        let _ = write!(
            out,
            "{} {:<18} {:<10} {}",
            text(&row["time"]),
            text(&row["cmd"]),
            text(&row["level"]),
            text(&row["outcome"])
        );
        if let Some(repeats) = row["count"].as_u64() {
            let _ = write!(out, " x{repeats} since {}", text(&row["since"]));
        }
        if !row["details"].is_null() {
            let _ = write!(out, " {}", row["details"]);
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}
