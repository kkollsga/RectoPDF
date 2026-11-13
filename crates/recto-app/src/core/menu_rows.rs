//! **What a menu id is, said once, for whoever runs it** (step R2.3; owner, 2026-11-12: "all features should be implemented once"). A menu item, a key, a
//! panel button, the context menu and ⌘K's rows come here by id: the person's through [`Inner::perform`], an agent's `recto palette --run` through
//! [`for_agent`]. Each id is a [`Row`]: the agent's command it is, whoever runs it; that command with the person's run by a way of its own, said why; or the
//! window's alone, said why. Each feature says its own ids, in its own file, and is registered here in one line ([`ROWS`]).
//!
//! Whose the command is, the actor, is passed to the one executor, and each rule about who may do what is the command's: an agent's palette run of a row is
//! the agent's command at its level, with every check that command has, and nothing an agent couldn't send itself.

use super::{HostAction, Inner};
use crate::menus::Action;
use crate::queue::WhenRun;
use recto_agent::Command;
use std::sync::Arc;

/// The person's run of a row that has a way of its own, saying what's left for the host. Never waits.
pub type Way<'a> = Box<dyn FnOnce() -> HostAction + 'a>;

/// What a menu id comes to now.
pub enum Row<'a> {
    /// **The agent's command it is, whoever runs it:** the person's sent to the one executor as theirs, by the session as it is when it runs if `when_run`
    /// says how, then `host` left for the host; an agent's run as the agent's, resolved by `when_run` first.
    Command {
        command: Command,
        when_run: Option<WhenRun>,
        host: HostAction,
    },
    /// **The agent's command it is, with the person's run by a way of its own,** said `why`: `person` runs it. An agent's is `command`, as the agent's.
    OwnWay { command: Command, why: &'static str, person: Way<'a> },
    /// **The window's alone, with no command,** said `why`: a system panel or sheet, the keyboard's focus, the pasteboard, the window's own panel or pointer
    /// mode. `person` does it, for the person, and for an agent's palette run as the menu does it, at the level that needs (Full, or `copy`'s).
    Window { why: &'static str, person: Way<'a> },
    /// One of these ids, with nothing to do now: a close with no document, a recent file that's left the list.
    Nothing,
}

impl<'a> Row<'a> {
    /// The agent's command, the same for both, with nothing left for the host.
    pub fn command(command: impl Into<Command>) -> Self {
        Self::Command {
            command: command.into(),
            when_run: None,
            host: HostAction::None,
        }
    }

    /// The window's alone: `person` does it.
    pub fn window(why: &'static str, person: impl FnOnce() -> HostAction + 'a) -> Self {
        Self::Window { why, person: Box::new(person) }
    }

    /// The window's alone, and all of it the host's to do.
    pub fn host(why: &'static str, host: HostAction) -> Self {
        Self::window(why, move || host)
    }

    /// The command, with the person's run by `person`.
    pub fn own_way(command: impl Into<Command>, why: &'static str, person: impl FnOnce() -> HostAction + 'a) -> Self {
        Self::OwnWay {
            command: command.into(),
            why,
            person: Box::new(person),
        }
    }

    /// Why it's the window's alone, if it is.
    pub fn windows_alone(&self) -> Option<&'static str> {
        match self {
            Self::Window { why, .. } => Some(why),
            _ => None,
        }
    }
}

/// A menu id asked about, with what `menus` makes of it.
pub struct Asked<'a> {
    pub id: &'a str,
    pub action: Option<Action>,
}

impl Asked<'_> {
    /// The `Host` action `menus` has for it, if that's what it is: the host's alone.
    pub fn host(&self) -> Option<HostAction> {
        match self.action {
            Some(Action::Host(host)) => Some(host),
            _ => None,
        }
    }
}

/// A feature's rows: its own ids' [`Row`]s, or `None` for an id that isn't its.
type Rows = for<'a> fn(&'a Arc<Inner>, &Asked<'_>) -> Option<Row<'a>>;

/// **Every feature's rows, one line each.**
const ROWS: &[Rows] = &[
    super::viewing::row,
    super::zoom::row,
    super::editing::row,
    super::navigation::row,
    super::find::row,
    super::comparison::row,
    super::closing::row,
    super::saving::row,
    super::updates::row,
    super::opening::row,
    super::palette::row,
    super::snapshot_tool::row,
    super::printing::row,
    super::window_rows::row,
];

/// What `id` is now, from what's published and the window's own state: never the session's lock. `None` for an id nothing has.
pub fn row<'a>(inner: &'a Arc<Inner>, id: &str) -> Option<Row<'a>> {
    let asked = Asked {
        id,
        action: crate::menus::action(id),
    };
    ROWS.iter().find_map(|rows| rows(inner, &asked))
}

/// **The person's run of a row,** saying what's left for the host. Never waits.
pub fn by_person(inner: &Arc<Inner>, row: Row<'_>) -> HostAction {
    match row {
        Row::Command { command, when_run, host } => {
            match when_run {
                Some(when_run) => super::person::submit_when_run(inner, command, when_run),
                None => super::person::submit(inner, command),
            }
            host
        }
        Row::OwnWay { person, .. } | Row::Window { person, .. } => person(),
        Row::Nothing => HostAction::None,
    }
}

/// What an agent's palette run of a menu id is.
pub enum ForAgent {
    /// The command, as the agent's, resolved by the session as it is now where the row says so.
    Command(Command),
    /// The window's alone: done as the menu does it, at the level that needs.
    Window,
    /// No command, nor the window's alone: refused, whatever the level, as running it the menu's way would run it as the person.
    Refused,
}

/// **An agent's palette run of `id`:** the row's command, resolved under `with_session` where it goes by the session when it runs, as the person's is on the
/// session thread.
pub fn for_agent(inner: &Arc<Inner>, id: &str, with_session: impl FnOnce(WhenRun, Command) -> Command) -> ForAgent {
    match row(inner, id) {
        Some(Row::Command {
            command,
            when_run: Some(when_run),
            ..
        }) => ForAgent::Command(with_session(when_run, command)),
        Some(Row::Command { command, .. } | Row::OwnWay { command, .. }) => ForAgent::Command(command),
        Some(Row::Window { .. }) => ForAgent::Window,
        Some(Row::Nothing) | None => ForAgent::Refused,
    }
}
