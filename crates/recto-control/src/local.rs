//! The in-process route: the same dispatch, the same levels and the same log as the socket, with no server. Tests and QA scripts use it. Tabs last only as long as this process.

use crate::log::ActivityLog;
use crate::{levels, state::StatePaths};
use recto_agent::index::ReadOnlyCorpus;
use recto_agent::{AgentError, Command, Level, Refused, acted_tab, allowed, refusal, run_indexed, table, with_acted_tab};
use recto_session::{DocLoader, Session};
use serde_json::Value;

/// Runs a command in this process, at the level the user set, and logs it. Nothing about this route raises a level.
pub fn run_here<L: DocLoader + Clone>(session: &mut Session<L>, paths: &StatePaths, log: &mut ActivityLog, command: &Command) -> Result<Value, AgentError> {
    // A session on a test state folder uses only files in its own folders, however many commands it's given.
    crate::quiet::confine(session, paths);
    let level = levels::current(paths);
    let acted = acted_tab(session, command);
    // What's been built gets read. Nothing lives long enough here to build one.
    let corpus = ReadOnlyCorpus(paths.corpus.clone());
    let result = if allowed(level, command.access()) {
        run_indexed(session, level, command, Some(&corpus))
    } else {
        Err(refusal(table::display_name(command.tag()), level, command.access()))
    };
    let outcome = result.as_ref().map_or_else(|error| error.code.as_str(), |_| "ok");
    log.record(command.tag(), level, outcome, &with_acted_tab(command.log_details(), acted, &result, command));
    result
}

/// Reads one line a client sent, and logs a refusal of it under the command it was for: `unknown` for a line that's no command, and the command's own name
/// for one whose arguments its type refuses (step R2.5), which the log writes as a line of its own rather than counting it as a repeat. Only the name and
/// the code: nothing of what the line held. The socket and `recto session` read every line through this, so a refusal reads and is logged alike by either.
pub fn read_line(log: &mut ActivityLog, level: impl FnOnce() -> Level, line: &[u8]) -> Result<Command, Refused> {
    let read = std::str::from_utf8(line).map_or_else(|_| Err(Refused::not_a_command()), recto_agent::read);
    if let Err(refused) = &read {
        log.record(refused.tag, level(), refused.error.code.as_str(), &Value::Null);
    }
    read
}
