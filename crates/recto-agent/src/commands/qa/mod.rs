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
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
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
    /// **The tab the caller means a window-wide verb for** (`qa menu`, `qa button`, `qa key-action`, `qa print-to-pdf`, which act on whatever tab the
    /// window shows): refused, with nothing done, if the window shows another. Owner, 2026-11-13: "Shouldnt it be clear which tab the action is sent to?"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<TabRef>,
}

/// The verbs that act on whatever tab the window shows, and so take `--tab` and say in their answer which tab they acted on.
pub const ON_THE_SHOWN_TAB: &[&str] = &["menu", "button", "key_action", "print_to_pdf"];

impl Cmd for QaDrive {
    const TAG: &'static str = "qa_drive";
    /// In [`ROWS_QA`].
    const ROWS: &'static [Spec] = &[];
    const DRAWS: bool = false;
    const READS_PAGES: bool = false;
    const PROTECTED_OK: bool = false;
    const ABOUT: super::About = super::About::Nothing;
    const WAITS: bool = true;
    fn tab(&self) -> Option<&TabRef> {
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

/// What a `$NAME` in a line stands for, if anything.
pub type Names = dyn Fn(&str) -> Option<String>;

/// **A line's words, as a `.qa` script has them:** spaces between, double quotes around a word with spaces in it, and with `vars`, `$NAME` replaced by
/// what it gives (`recto qa run` gives the environment); without, a `$` is itself. The one splitter, for a script's lines and for `qa as-person`'s command.
///
/// # Errors
///
/// A quote that isn't closed, or a `$NAME` that `vars` hasn't.
pub fn line_words(line: &str, vars: Option<&Names>) -> Result<Vec<String>, String> {
    let (mut words, mut word, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '$' if vars.is_some() => {
                let mut name = String::new();
                while let Some(next) = chars.peek().filter(|next| next.is_ascii_alphanumeric() || **next == '_') {
                    name.push(*next);
                    chars.next();
                }
                word.push_str(&vars.and_then(|vars| vars(&name)).ok_or_else(|| format!("${name} isn't set."))?);
                any = true;
            }
            ' ' | '\t' if !quoted => {
                if any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            other => {
                word.push(other);
                any = true;
            }
        }
    }
    if quoted {
        return Err("A quote isn't closed.".to_string());
    }
    if any {
        words.push(word);
    }
    Ok(words)
}

/// The QA verbs' rows, `recto`'s own and the two commands', in the order the help shows them.
pub(crate) const ROWS_QA: &[&[Spec]] = &[rows_recto::ROWS, rows_window::ROWS, rows_features::ROWS];
