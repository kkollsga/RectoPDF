//! **Who asked for a command** (step R2.3): the person at the window, or an agent at the level the user gave it. A person's actions and an agent's run the same
//! commands through the one executor, [`crate::execute`], and every rule that tells them apart is stated here, once, as a method each check asks.
//!
//! What isn't here, as it's not the executor's to decide: the activity log is the server's, and a person's commands never reach the server, so only agents'
//! are logged; during an update's hand-off an agent's command is refused as busy (`AppHost`), and a person's action is dropped by the session thread's queue,
//! as the window takes no input then.

use crate::Level;
use crate::table::Access;
use recto_session::Author;

/// Who a command is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actor {
    /// The person using the app, at its window.
    Person,
    /// An agent, at the level the user gave it.
    Agent(Level),
}

impl Actor {
    /// **Who may run a command.** An agent by its level. The person may do anything a command does: what no one may do (signing has no command) is refused
    /// for them too.
    #[must_use]
    pub fn may(self, needed: Access) -> bool {
        match self {
            Self::Agent(level) => crate::allowed(level, needed),
            Self::Person => !matches!(needed, Access::Nobody),
        }
    }

    /// **The level a check by level goes by.** An agent's own. The person's is Full: they may do all an agent at Full may, so the rules that keep the owner's
    /// edits from agents below Full (an undone step lost to a new edit, a discard, a close) never refuse the owner.
    #[must_use]
    pub fn level(self) -> Level {
        match self {
            Self::Agent(level) => level,
            Self::Person => Level::Full,
        }
    }

    /// **Whose an edit or a jump is**: the person's are the owner's, which an agent below Full can't throw away, and an agent's marks in the trail give way to
    /// the person's.
    #[must_use]
    pub fn author(self) -> Author {
        match self {
            Self::Agent(_) => Author::Agent,
            Self::Person => Author::Owner,
        }
    }

    /// **A document the person opened with its password** gives an agent nothing read from it (agent report §5.5). The person typed the password, and it's
    /// theirs to read.
    #[must_use]
    pub fn sees_protected(self) -> bool {
        matches!(self, Self::Person)
    }

    /// An agent, at Full. What only an agent at Full may push past is the owner's, as their own edits are.
    #[must_use]
    pub fn is_agent_at_full(self) -> bool {
        matches!(self, Self::Agent(Level::Full))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the person is refused something an agent at Full may do, or let through what no one may (signing), or an agent below its level gets past a
    /// row's level. Every level of every kind of access, for both.
    #[test]
    fn the_person_may_what_full_may_and_no_more_than_anyone() {
        let levels = [Level::Off, Level::ReadOnly, Level::Control, Level::Full];
        for needed in levels.map(Access::At).into_iter().chain([Access::UserOnly, Access::Nobody]) {
            for have in levels {
                assert_eq!(Actor::Agent(have).may(needed), crate::allowed(have, needed), "{have:?} for {needed:?}");
            }
            assert_eq!(Actor::Person.may(needed), !matches!(needed, Access::Nobody), "the person for {needed:?}");
        }
        assert_eq!(Actor::Person.level(), Level::Full);
        assert_eq!(Actor::Person.author(), Author::Owner);
        assert_eq!(Actor::Agent(Level::Full).author(), Author::Agent);
        assert!(Actor::Person.sees_protected() && !Actor::Agent(Level::Full).sees_protected());
    }
}
