//! **Each command is answered where its route says, on a session with no window** (R2.2): what the server answers itself, what only the app has and what a host's record keeps each say so here, by their own refusal, rather than reaching another family's handler and answering for it.

use crate::host::{Host, SessionHost};
use recto_agent::{Command, Level, Runs, parse};
use recto_core::{DocFacts, OpenError, Toc};
use recto_session::DocLoader;
use std::path::Path;

/// Every request the registry's golden test makes, one line each: a line of every command, in every shape it takes.
const REQUESTS: &str = include_str!("../../../recto-agent/tests/golden/requests.txt");

#[derive(Clone)]
struct Fake;

impl DocLoader for Fake {
    fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
        Ok(DocFacts {
            page_count: 10,
            byte_len: 1,
            encrypted: false,
            version: "1.7".into(),
            title: None,
            toc: Toc::default(),
            renderer_only: false,
        })
    }
}

/// The refusal each command answered elsewhere gives on a session that isn't where it's answered.
fn own_refusal(command: &Command) -> Option<&'static str> {
    match command.route() {
        Runs::Session => None,
        Runs::Server => Some("needs_server"),
        Runs::Record => Some("no_record"),
        Runs::App => Some(match command.tag() {
            "qa_read" | "qa_drive" => "qa_needs_app",
            "palette" => "palette_needs_app",
            "view" => "zoom_needs_app",
            _ => "needs_app",
        }),
    }
}

/// Failure: a command answered by the server's loop, the app or a host's record reaches the session's `execute` on a long-running session with no window and no record, and answers as another family would, or does something: a Server command that isn't `needs_server`, an App command that isn't its own `*needs_app`, a Record command that isn't `no_record`. The latent case this was written for: `session check` reaching a handler that answers as QA.
#[test]
fn every_route_answers_as_declared_on_a_session_with_no_record() {
    let host = SessionHost::new(Fake);
    host.run(
        recto_agent::Actor::Agent(Level::Full),
        &parse(r#"{"cmd":"open","path":"/d/a.pdf"}"#).expect("open"),
        &mut Vec::new(),
    )
    .expect("opened");
    let mut seen = 0;
    for line in REQUESTS.lines() {
        let Ok(command) = parse(line) else { continue };
        let Some(expected) = own_refusal(&command) else { continue };
        seen += 1;
        let answer = host.run(recto_agent::Actor::Agent(Level::Full), &command, &mut Vec::new());
        let code = answer.as_ref().err().map(|error| error.code);
        assert_eq!(
            code,
            Some(expected),
            "{line} (route {:?}) answered {answer:?} on a session with no window or record, not its own {expected}",
            command.route()
        );
    }
    assert!(seen > 20, "only {seen} commands answered elsewhere were found in the golden requests");
}
