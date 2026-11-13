//! **QA**: an app started in QA mode, asked what it shows (`qa_read`) or made to do what the UI does (`qa_drive`). Only that app answers them: its route is the app's, and a session with no app says so.
//!
//! Their rows are one list, [`ROWS_QA`], with those of the `qa` verbs `recto` runs itself (`qa start`, `qa run`, the budgets): it's a list of verbs in the order a tester reads them, and the two commands' rows come in among each other there, so neither command's rows can be a list of their own without moving the help. So each command's own [`Cmd::ROWS`] is empty, and the list is registered once, where the help has always put it.

mod rows_features;
mod rows_recto;
mod rows_window;
pub mod verbs;

use super::{Cmd, Cx, OnlyAsks, row_access};
use crate::table::*;
use crate::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

/// Asks the running app what it shows. Only an app started in QA mode answers.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct QaRead {
    pub verb: String,
    #[serde(default)]
    pub out: Option<PathBuf>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Cmd for QaRead {
    const TAG: &'static str = "qa_read";
    /// In [`ROWS_QA`].
    const ROWS: &'static [Spec] = &[];
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        verb_access(Self::TAG, &self.verb)
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        json!({ "verb": recto_core::clean_line(&self.verb) })
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(qa_needs_app())
    }
    fn human(data: &Value) -> String {
        serde_json::to_string_pretty(data).unwrap_or_default()
    }
}

/// Has the running app do what the UI does. Only an app started in QA mode answers.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct QaDrive {
    pub verb: String,
    pub value: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl Cmd for QaDrive {
    const TAG: &'static str = "qa_drive";
    /// In [`ROWS_QA`].
    const ROWS: &'static [Spec] = &[];
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&str> {
        None
    }
    fn access(&self) -> Access {
        verb_access(Self::TAG, &self.verb)
    }
    fn route(&self) -> Runs {
        Runs::App
    }
    fn log_details(&self) -> Value {
        json!({ "verb": recto_core::clean_line(&self.verb) })
    }
    fn run<A: SessionAccess>(&self, _: &mut Cx<'_, A>) -> Result<Value, AgentError> {
        Err(qa_needs_app())
    }
    fn human(data: &Value) -> String {
        serde_json::to_string_pretty(data).unwrap_or_default()
    }
}

/// **A verb's level is its own row's,** so one verb can need more than the rest. A verb no row of this command sends has the command's first row's level,
/// and is refused as no QA command where it's answered.
fn verb_access(tag: &str, verb: &str) -> Access {
    verbs::row_of(verb)
        .filter(|row| row.tag == tag)
        .map_or_else(|| row_access(tag), |row| row.access)
}

impl super::Qa {
    /// QA's answers are a tester's look at the app, never something an update carries across, and they're answered while one hands off.
    pub fn only_asks(&self) -> bool {
        match self {
            Self::QaRead(command) => command.only_asks(),
            Self::QaDrive(command) => command.only_asks(),
        }
    }
}

impl OnlyAsks for QaRead {
    fn only_asks(&self) -> bool {
        true
    }
}

impl OnlyAsks for QaDrive {
    fn only_asks(&self) -> bool {
        true
    }
}

/// The QA verbs' rows, `recto`'s own and the two commands', in the order the help shows them.
pub(crate) const ROWS_QA: &[&[Spec]] = &[rows_recto::ROWS, rows_window::ROWS, rows_features::ROWS];
