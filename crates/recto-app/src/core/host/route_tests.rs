//! **Through the app's host, each command reaches its own handler at the level it needs** (R2.2): the guard on [`AppHost`]'s routing, in process on the headless app.

use crate::testkit::headless::HeadlessApp;
use recto_agent::{Command, Level, Runs, allowed, parse, refusal, table};
use recto_control::host::Host;

/// Every request the registry's golden test makes, one line each: a line of every command, in every shape it takes.
const REQUESTS: &str = include_str!("../../../../recto-agent/tests/golden/requests.txt");

/// One request of each command and route: `view` with a zoom is the app's, without one the session's.
fn one_of_each() -> Vec<(&'static str, Command)> {
    let mut picked: Vec<(&str, Command)> = Vec::new();
    for line in REQUESTS.lines() {
        let Ok(command) = parse(line) else { continue };
        if !picked.iter().any(|(_, had)| had.tag() == command.tag() && had.route() == command.route()) {
            picked.push((line, command));
        }
    }
    picked
}

/// Failure: through the app's own host, which the socket and the window share, a command is refused when its level allows it or runs when it doesn't (the record's commands had no check of their own on this path, only the server's), or a command answered by the app or its record reaches a handler that isn't its own and answers for it: `needs_server`, or a `*needs_app` that isn't its own. The latent case: `session check` reaching the app's record path and answering as a QA command, `qa_needs_app`. Of the app's commands only QA's may say its own, as this app wasn't started in QA mode.
#[test]
fn every_command_reaches_its_own_handler_through_the_app_at_every_level() {
    let app = HeadlessApp::start();
    let host = &app.core().host;
    let picked = one_of_each();
    assert!(picked.len() > 60, "only {} commands in the golden requests", picked.len());
    let mut wrong = Vec::new();
    for level in [Level::Off, Level::ReadOnly, Level::Control, Level::Full] {
        for (line, command) in &picked {
            let answer = host.run(recto_agent::Actor::Agent(level), command, &mut Vec::new());
            let code = answer.as_ref().err().map(|error| error.code.as_str());
            let needed = command.access();
            let refused = refusal(table::display_name(command.tag()), level, needed).code.as_str();
            if !allowed(level, needed) {
                if code != Some(refused) {
                    wrong.push(format!("{line} at {level:?} needs {needed:?}, and wasn't refused: {answer:?}"));
                }
                continue;
            }
            if code == Some(refused) {
                wrong.push(format!("{line} at {level:?} is allowed, and was refused: {answer:?}"));
            }
            let Some(code) = code else { continue };
            let elsewhere = code == "needs_server" || code.ends_with("needs_app");
            let own_qa = matches!(command, Command::Qa(_)) && code == "qa_needs_app";
            if matches!(command.route(), Runs::App | Runs::Record) && elsewhere && !own_qa {
                wrong.push(format!(
                    "{line} (route {:?}) answered {code} through the app at {level:?}: it reached a handler that isn't its own",
                    command.route()
                ));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
