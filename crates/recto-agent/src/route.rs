//! Where each command is answered: the session, the server, the host's record or only the app. One exhaustive list, which the server, the hosts and the app route by.

use crate::Code;
use crate::commands::{Move, OnlyAsks};
use crate::{AgentError, Command, exit};

/// Where a command is answered. `execute` runs the session's; the others it only says it can't, and the host sends them on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// By `execute`, on the session.
    Session,
    /// By the server's own loop: it holds the connections, the level and the log.
    Server,
    /// By the host's state record and its lifecycle: each host keeps its own.
    Record,
    /// Only by the app: its window, its updater, its QA.
    App,
}

impl Command {
    /// Where this is answered. Spelled out for every command, so a new one can't go to the wrong place by default.
    pub fn route(&self) -> Runs {
        match self {
            Self::Measure(command) => command.route(),
            Self::Edit(command) => command.route(),
            Self::Move(command) => command.route(),
            Self::Read(command) => command.route(),
            Self::Start(command) => command.route(),
            Self::Waits(command) => command.route(),
            Self::Rights(command) => command.route(),
            Self::Session(command) => command.route(),
            Self::Qa(command) => command.route(),
            Self::Record(command) => command.route(),
            Self::Updates(command) => command.route(),
        }
    }

    /// Whether it only asks about the state, so an update needn't wait for it, nor refuse it while it hands off. Declared by every command the app or a host's record answers; a command the session or the server answers is always waited for.
    pub fn only_asks(&self) -> bool {
        match self {
            Self::Record(command) => command.only_asks(),
            Self::Qa(command) => command.only_asks(),
            Self::Move(Move::View(command)) => command.only_asks(),
            Self::Move(Move::Palette(command)) => command.only_asks(),
            Self::Updates(command) => command.only_asks(),
            Self::Measure(_) | Self::Edit(_) | Self::Move(_) | Self::Read(_) | Self::Start(_) | Self::Waits(_) | Self::Rights(_) | Self::Session(_) => false,
        }
    }
}

/// QA commands reach an app started in QA mode, and nothing else. Other sessions, a shipped app's included, say this.
pub fn qa_needs_app() -> AgentError {
    AgentError::new(
        Code::QA_NEEDS_APP,
        "QA commands only work on an app started with `recto qa start`.",
        exit::FAILED,
    )
    .then("Set RECTO_STATE_DIR to a temp folder and run `recto qa start`.")
}

#[cfg(test)]
mod tests {
    use crate::{Runs, parse};

    /// Failure: an update stops waiting for a command that changes the session it carries across (a sleep, a restore, a zoom, a palette's run, an install), so its change is lost past the hand-off's last look; or a look at the state, the record's save or QA is refused as busy while one hands off. Only commands the app or a host's record answers may only ask.
    #[test]
    fn only_looks_the_record_save_and_qa_go_past_an_update() {
        let mut asks: Vec<&str> = include_str!("../tests/golden/requests.txt")
            .lines()
            .filter_map(|line| parse(line).ok())
            .filter(|command| command.only_asks())
            .inspect(|command| {
                assert!(
                    matches!(command.route(), Runs::App | Runs::Record),
                    "{} only asks, and is the session's",
                    command.tag()
                )
            })
            .map(|command| command.tag())
            .collect();
        asks.sort_unstable();
        asks.dedup();
        assert_eq!(
            asks,
            [
                "qa_drive",
                "qa_read",
                "session_check",
                "session_save",
                "session_show",
                "settings",
                "update_dev_builds",
                "update_status"
            ]
        );
        for line in [r#"{"cmd":"settings","auto_update":"on"}"#, r#"{"cmd":"update_dev_builds","set":"on"}"#] {
            assert!(!parse(line).expect("a command").only_asks(), "{line} sets something");
        }
    }
}
