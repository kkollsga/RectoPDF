//! The in-process route: the same dispatch, the same levels and the same log as the socket, with no server. Tests and QA scripts use it. Tabs last only as long as this process.

use crate::log::ActivityLog;
use crate::{levels, state::StatePaths};
use recto_agent::{AgentError, Command, allowed, refusal, run, table};
use recto_session::{DocLoader, Session};
use serde_json::Value;

/// Runs a command in this process, at the level the user set, and logs it. Nothing about this route raises a level.
pub fn run_here<L: DocLoader + Clone>(session: &mut Session<L>, paths: &StatePaths, log: &mut ActivityLog, command: &Command) -> Result<Value, AgentError> {
    let level = levels::current(paths);
    let result = if allowed(level, command.access()) { run(session, level, command) } else { Err(refusal(table::display_name(command.tag()), level, command.access())) };
    let outcome = result.as_ref().map_or_else(|error| error.code, |_| "ok");
    log.record(command.tag(), level, outcome, &command.log_details());
    result
}
