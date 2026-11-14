//! **No command's guarding properties change when commands move into modules** (step R2.2). For every request in `tests/golden/requests.txt` that parses, one line: its tag, level, the tab it holds, whether it draws, whether it reads pages, where it runs, and what goes in the activity log. Then the reply and the plain words of running each one, in turn, on one session of fake files at Full; and for a request refused as it's read, the command its reply names and its code.
//!
//! Failure caught: a command that forgets `draws()` skips the server's draw cap, the memory rule; one that forgets its tab isn't held, so its tab can go dormant mid-command; a level or a log entry moves, or a reply's bytes change, while code moves between modules.
//!
//! `RECTO_BLESS=1` rewrites the golden files, for a change that means to change them. Their diff is the review.

use super::*;
use recto_core::{DocFacts, OpenError, Toc};

const REQUESTS: &str = include_str!("../tests/golden/requests.txt");
const PROPS: &str = include_str!("../tests/golden/command_props.txt");
const REPLIES: &str = include_str!("../tests/golden/replies.txt");

#[derive(Clone)]
struct Fake;

impl DocLoader for Fake {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        if path.to_string_lossy().contains("missing") {
            return Err(OpenError::NotFound);
        }
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

/// Every request, read: a command, or its refusal as it's read and the command it was for.
fn requests() -> Vec<(&'static str, Result<Command, Refused>)> {
    REQUESTS.lines().map(|line| (line, read(line))).collect()
}

fn commands() -> Vec<(&'static str, Command)> {
    requests().into_iter().filter_map(|(line, read)| Some((line, read.ok()?))).collect()
}

fn route_word(command: &Command) -> &'static str {
    match command.route() {
        Runs::Session => "session",
        Runs::Server => "server",
        Runs::Record => "record",
        Runs::App => "app",
    }
}

fn access_word(access: Access) -> String {
    match access {
        Access::At(level) => level.name().to_string(),
        Access::UserOnly => "user_only".into(),
        Access::Nobody => "nobody".into(),
    }
}

fn props() -> String {
    let mut out = String::new();
    for (_, command) in commands() {
        out.push_str(&format!(
            "{} level={} tab={:?} about={:?} protected_ok={} draws={} reads_pages={} route={} log={}\n",
            command.tag(),
            access_word(command.access()),
            command.tab_selector(),
            command.about(),
            command.protected_ok(),
            command.draws(),
            command.reads_pages(),
            route_word(&command),
            command.log_details(),
        ));
    }
    out
}

fn replies() -> String {
    let mut session = Session::new(Fake);
    for name in ["a", "b", "c"] {
        session.open(&PathBuf::from(format!("/d/{name}.pdf"))).unwrap();
    }
    let mut out = String::new();
    for (line, command) in requests() {
        // A request refused as it's read keeps what its reply says: the command it names and its code, so what a malformed request is answered can't change
        // unseen (review of R2.5, S1: six replies went from this file when their requests stopped parsing).
        let command = match command {
            Ok(command) => command,
            Err(refused) => {
                out.push_str(&format!("{line}\n  refused cmd={} code={}\n", refused.tag, refused.error.code));
                continue;
            }
        };
        // What's measured or drawn depends on the machine, and drawing needs a real file and the worker: held elsewhere.
        if command.draws() || matches!(command.tag(), "budget" | "wait") {
            continue;
        }
        let mut events = Vec::new();
        let result = execute(
            &Direct(RefCell::new(&mut session), None),
            crate::Actor::Agent(Level::Full),
            &command,
            &mut events,
        );
        let words = match &result {
            Ok(data) => human::render(command.tag(), data),
            Err(_) => String::new(),
        };
        let events: Vec<String> = events.iter().map(|event| event.to_json().to_string()).collect();
        out.push_str(&format!(
            "{line}\n  reply {}\n  events {}\n  words {:?}\n",
            reply(command.tag(), &result),
            events.join(" "),
            words
        ));
    }
    out
}

fn held_to(made: &str, golden: &str, file: &str) {
    if std::env::var_os("RECTO_BLESS").is_some() {
        std::fs::write(format!("{}/tests/golden/{file}", env!("CARGO_MANIFEST_DIR")), made).unwrap();
        return;
    }
    for (number, (now, was)) in made.lines().zip(golden.lines()).enumerate() {
        assert_eq!(now, was, "line {} of tests/golden/{file}", number + 1);
    }
    assert_eq!(made.lines().count(), golden.lines().count(), "the number of lines in tests/golden/{file}");
}

#[test]
fn every_commands_guarding_properties_stay_as_they_were() {
    held_to(&props(), PROPS, "command_props.txt");
}

#[test]
fn every_commands_reply_stays_as_it_was() {
    held_to(&replies(), REPLIES, "replies.txt");
}
