//! **Who asked for a command** (step R2.3): the person at the window, or an agent at the level the user gave it. A person's actions and an agent's run the same
//! commands through the one executor, [`crate::execute`], and every rule that tells them apart is stated here, once, as a method each check asks.
//!
//! What isn't here, as it's not the executor's to decide: the activity log is the server's, and a person's commands never reach the server, so only agents'
//! are logged; during an update's hand-off an agent's command is refused as busy (`AppHost`), and a person's action is dropped by the session thread's queue,
//! as the window takes no input then.

mod refusals;

pub(crate) use refusals::{ASK_FULL, Rewrite, Stake};

use crate::table::Access;
use crate::{AgentError, Level};
use recto_session::{Author, DocLoader, Session, TabId};

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
            Self::Agent(level) => allowed(level, needed),
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

    /// **Whether this is an agent**, at any level: identity, not permission. For what is counted or held for agents as a kind (their share of the hour's
    /// sleep files, the commands refused while the record settles), where the rule is about agents and not about what a level allows.
    #[must_use]
    pub fn is_agent(self) -> bool {
        matches!(self, Self::Agent(_))
    }

    /// **A document the person opened with its password** gives an agent nothing read from it (agent report §5.5). The person typed the password, and it's
    /// theirs to read.
    #[must_use]
    pub fn sees_protected(self) -> bool {
        matches!(self, Self::Person)
    }

    /// **The person, at the window,** whose command comes from a control and returns at once: the window draws what the session publishes as it comes, so
    /// nothing is waited for, and what the person does at it may arrange it, as their Find shows the sidebar it's in. An agent's command answers once, with
    /// what it waited for, and doesn't rearrange the person's window (R2.3, A6).
    #[must_use]
    pub fn at_the_window(self) -> bool {
        matches!(self, Self::Person)
    }

    /// **At Full,** the one line every rule below goes by: the person, and an agent the user gave Full.
    fn at_full(self) -> bool {
        self.level().at_least(Level::Full)
    }

    /// **Held to the owner's-edits rule** (SPEC decision 20): an agent below Full throws away only agents' edits, never the owner's. At Full what's
    /// lost is the holder's call.
    #[must_use]
    pub fn held_to_owners_edits(self) -> bool {
        !self.at_full()
    }

    /// **Refuses what would lose the owner's edits**, if this actor is held to them and the session holds any of the owner's that `stake` would lose:
    /// with that stake's one refusal.
    pub(crate) fn refuse_owner_edits_at_stake<L: DocLoader>(self, session: &Session<L>, stake: Stake) -> Result<(), AgentError> {
        if self.held_to_owners_edits() && stake.at_stake(session) {
            return Err(stake.refusal());
        }
        Ok(())
    }

    /// **Says who the session's next edits are by, for its history's rule:** an agent's at Full may push the owner's steps out of a full history, and
    /// below Full may not. The person's edits are the owner's, and say nothing of it.
    pub(crate) fn note_who_edits<L: DocLoader>(self, session: &mut Session<L>) {
        if let Self::Agent(_) = self {
            session.lists_mut().set_agent_at_full(self.at_full());
        }
    }

    /// **May lose what it can't read** (coordinator, 2026-11-13: unknown is "would lose", not "nothing to lose"): a tab whose edits couldn't be read
    /// back is the person's to give up, as their tab says so; an agent's `discard`, `clear-history` or `close` of it is refused at every level.
    #[must_use]
    pub fn may_lose_unread(self) -> bool {
        matches!(self, Self::Person)
    }

    /// **Refuses throwing away a tab's edits that couldn't be read back** ([`Self::may_lose_unread`]): every command about a tab reads its list back
    /// first, so one still asleep is one whose journal couldn't be read, and its summary says some of what's there, not all of it. `what` is what
    /// wasn't done, as "discarded".
    pub(crate) fn refuse_losing_unread<L: DocLoader>(self, session: &Session<L>, id: TabId, what: &str) -> Result<(), AgentError> {
        if self.may_lose_unread() || !session.lists().is_asleep(id) {
            return Ok(());
        }
        Err(refusals::unreadable(what))
    }

    /// **May write a file anew** (SPEC section 5), which takes what was removed out of it for good, or save over another file: at Full.
    #[must_use]
    pub fn may_rewrite(self) -> bool {
        self.at_full()
    }

    /// **Refuses a rewrite** below Full, with its one refusal.
    pub(crate) fn refuse_rewrite(self, rewrite: Rewrite) -> Result<(), AgentError> {
        if self.may_rewrite() { Ok(()) } else { Err(rewrite.refusal()) }
    }

    /// **Sees the user's own words and the paths of their files**: what they typed in Find, the paths of files recently open. At Control: below it an
    /// agent reads what it's given, not what the user looked for or where their files are.
    #[must_use]
    pub fn sees_users_words_and_paths(self) -> bool {
        self.level().at_least(Level::Control)
    }

    /// **Sees where a save's backup is,** which says something about the user's disk: at Full.
    #[must_use]
    pub fn sees_backup_path(self) -> bool {
        self.at_full()
    }
}

/// **Whether an agent at `have` may run a command that needs `needed`.** Which command needs what is the table's ([`crate::table`]); this is the one
/// comparison of levels it's decided by. What's the user's alone or no one's isn't an agent's at any level.
#[must_use]
pub fn allowed(have: Level, needed: Access) -> bool {
    matches!(needed, Access::At(level) if have.at_least(level))
}

impl Level {
    /// The level's place in the order Off < Read-only < Control < Full. Private to this module: **the order is never compared outside `actor.rs`**, so a
    /// rule about who may do what can't be said again by comparing levels (the compiler holds it: `Level` has no `<` and no `as` cast to a number).
    fn rank(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::ReadOnly => 1,
            Self::Control => 2,
            Self::Full => 3,
        }
    }

    /// Whether this level gives at least what `other` gives. The one comparison of levels; [`allowed`] and the actor's rules are made of it.
    fn at_least(self, other: Self) -> bool {
        self.rank() >= other.rank()
    }

    /// **Whether this is a higher level than `was`**: the user gave an agent more. Raising access needs the user's confirmation (SPEC), so the server and
    /// the terminal drop what was waiting on a lower level when it does.
    #[must_use]
    pub fn raises(self, was: Self) -> bool {
        self.rank() > was.rank()
    }

    /// The higher of two levels: what the server keeps of what agents have asked for (the highest), so granting it clears the request. The server's bookkeeping of the highest level asked for; it decides nothing about who may do what.
    #[must_use]
    pub fn higher(self, other: Self) -> Self {
        if other.raises(self) { other } else { self }
    }

    /// **Whether a build an agent handed over goes in:** only while agent access is Full (SPEC: installing an update is Full's). It's a rule about a level,
    /// so it's said here and not by a comparison where the build is installed. A method of `Level`, not of `Actor`: the question is asked of the level
    /// file as it is now, not of whoever asked for a command.
    #[must_use]
    pub fn takes_a_build(self) -> bool {
        self.at_least(Self::Full)
    }
}

#[cfg(test)]
mod pins_tests;
#[cfg(test)]
pub(crate) mod scan_tests;

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
                assert_eq!(Actor::Agent(have).may(needed), allowed(have, needed), "{have:?} for {needed:?}");
            }
            assert_eq!(Actor::Person.may(needed), !matches!(needed, Access::Nobody), "the person for {needed:?}");
        }
        assert_eq!(Actor::Person.level(), Level::Full);
        assert_eq!(Actor::Person.author(), Author::Owner);
        assert_eq!(Actor::Agent(Level::Full).author(), Author::Agent);
        assert!(Actor::Person.sees_protected() && !Actor::Agent(Level::Full).sees_protected());
        assert!(Actor::Person.at_the_window() && !Actor::Agent(Level::Full).at_the_window());
    }
}
