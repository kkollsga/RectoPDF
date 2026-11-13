//! **What a menu id is, said once, for whoever runs it** (step R2.3; owner, 2026-11-12: "all features should be implemented once"). A menu item, a key, a
//! panel button, the context menu and ⌘K's rows come here by id: the person's through [`Inner::perform`], an agent's `recto palette --run` through
//! [`for_agent`]. Each id is a [`Row`]: the agent's command it is, whoever runs it; that command with the person's run by a way of its own, said why; or the
//! window's alone, said why. Each feature says its own ids, in its own file, and is registered here in one line ([`ROWS`]).
//!
//! Whose the command is, the actor, is passed to the one executor, and each rule about who may do what is the command's: an agent's palette run of a row is
//! the agent's command at its level, with every check that command has, and nothing an agent couldn't send itself. A row that's the window's alone says
//! whether an agent may have it ([`Agents`]): its way, which sends nothing as anyone, at a level, or never, as the person's own.
//!
//! A row that goes by the session when it runs has a resolver ([`WhenRun`]): it runs just before the command, under a hold of the session of its own (on the
//! session thread for the person, on the agent's thread for an agent), and names in the command what it read (the tab, not "the active one"), so whatever
//! comes between it and the run, the command is about what was read; for an agent it refuses a tab withheld from it before reading anything of it.

use super::{HostAction, Inner};
use crate::menus::Action;
use crate::queue::WhenRun;
use recto_agent::Command;
use std::sync::Arc;

/// A row's own way: the person's run of a command row with a way of its own, or what a row that's the window's alone does. Says what's left for the host.
/// Never waits.
pub type Way<'a> = Box<dyn FnOnce() -> HostAction + 'a>;

/// What a menu id comes to now.
pub enum Row<'a> {
    /// **The agent's command it is, whoever runs it:** the person's sent to the one executor as theirs, resolved by `when_run` on the session thread just
    /// before it runs, then `host` left for the host; an agent's run as the agent's, resolved by `when_run` on its own thread just before.
    Command {
        command: Command,
        when_run: Option<WhenRun>,
        host: HostAction,
    },
    /// **The agent's command it is, with the person's run by a way of its own,** said `why`: `person` runs it. An agent's is `command`, as the agent's.
    OwnWay { command: Command, why: &'static str, person: Way<'a> },
    /// **The window's alone, with no command,** said `why`: a system panel or sheet, the keyboard's focus, the pasteboard, the window's own panel or pointer
    /// mode, or something the person decided at a notice. `does` does it for the person; whether an agent's palette run may, `agents` says, as every such
    /// row must.
    Window { why: &'static str, agents: Agents, does: Way<'a> },
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

    /// The window's alone: `does` does it, and an agent's run may as `agents` says.
    pub fn window(why: &'static str, agents: Agents, does: impl FnOnce() -> HostAction + 'a) -> Self {
        Self::Window {
            why,
            agents,
            does: Box::new(does),
        }
    }

    /// The window's alone, and all of it the host's to do.
    pub fn host(why: &'static str, agents: Agents, host: HostAction) -> Self {
        Self::window(why, agents, move || host)
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

/// **Whether an agent's palette run may do a row that's the window's alone,** said by every such row, as there's no default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agents {
    /// Never, at any level: the person's own. A decision they made at a notice (an agent's own is `--anyway`, which is logged), the keyboard's focus, the
    /// app's lifecycle, or an item that does nothing.
    Never,
    /// As the menu does it, at this level: a window to show, the pasteboard, the window's own panel or pointer mode. Its way sends nothing as anyone.
    AsTheMenu(recto_agent::table::Access),
}

impl Agents {
    /// As the menu does it, at Full.
    pub const AT_FULL: Self = Self::AsTheMenu(recto_agent::table::Access::At(recto_agent::Level::Full));
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

/// **Every feature's rows, one line each,** by the feature's name.
const ROWS: &[(&str, Rows)] = &[
    ("viewing", super::viewing::row),
    ("zoom", super::zoom::row),
    ("editing", super::editing::row),
    ("navigation", super::navigation::row),
    ("find", super::find::row),
    ("comparison", super::comparison::row),
    ("closing", super::closing::row),
    ("saving", super::saving::row),
    ("updates", super::updates::row),
    ("opening", super::opening::row),
    ("palette", super::palette::row),
    ("snapshot_tool", super::snapshot_tool::row),
    ("printing", super::printing::row),
    ("window_rows", super::window_rows::row),
];

/// What `id` is now, from what's published and the window's own state: never the session's lock. `None` for an id nothing has.
pub fn row<'a>(inner: &'a Arc<Inner>, id: &str) -> Option<Row<'a>> {
    let asked = Asked {
        id,
        action: crate::menus::action(id),
    };
    ROWS.iter().find_map(|(_, rows)| rows(inner, &asked))
}

/// The features whose rows claim `id`: one, or none for an id nothing has. Two would be one id with two meanings, the first of which [`row`] takes.
#[cfg(test)]
pub fn claimed_by(inner: &Arc<Inner>, id: &str) -> Vec<&'static str> {
    let asked = Asked {
        id,
        action: crate::menus::action(id),
    };
    ROWS.iter().filter(|(_, rows)| rows(inner, &asked).is_some()).map(|(name, _)| *name).collect()
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
        Row::OwnWay { person, .. } => person(),
        Row::Window { does, .. } => does(),
        Row::Nothing => HostAction::None,
    }
}

/// What an agent's palette run of a menu id is.
pub enum ForAgent<'a> {
    /// The command, as the agent's, resolved by the session as it is now where the row says so.
    Command(Command),
    /// The window's alone, which an agent's run may do at `needs`: `does` does it, sending nothing as anyone.
    Window { needs: recto_agent::table::Access, does: Way<'a> },
    /// The person's own, said `why`: refused, at any level.
    PersonsOwn(&'static str),
    /// One of these ids, with nothing to do now.
    NothingNow,
    /// An id no row has.
    Unknown,
}

/// **An agent's palette run of `id`:** the row's command, resolved under `with_session` where it goes by the session when it runs (just before the run,
/// which is under a hold of its own: what the resolver read, it names in the command); or the window's part alone, if the row lets an agent have it; or why
/// not.
///
/// # Errors
///
/// What the resolver refused, as a tab withheld from the agent.
pub fn for_agent<'a>(
    inner: &'a Arc<Inner>,
    id: &str,
    with_session: impl FnOnce(WhenRun, Command) -> Result<Command, recto_agent::AgentError>,
) -> Result<ForAgent<'a>, recto_agent::AgentError> {
    Ok(match row(inner, id) {
        Some(Row::Command {
            command,
            when_run: Some(when_run),
            ..
        }) => ForAgent::Command(with_session(when_run, command)?),
        Some(Row::Command { command, .. } | Row::OwnWay { command, .. }) => ForAgent::Command(command),
        Some(Row::Window {
            agents: Agents::AsTheMenu(needs),
            does,
            ..
        }) => ForAgent::Window { needs, does },
        Some(Row::Window {
            agents: Agents::Never, why, ..
        }) => ForAgent::PersonsOwn(why),
        Some(Row::Nothing) => ForAgent::NothingNow,
        None => ForAgent::Unknown,
    })
}
