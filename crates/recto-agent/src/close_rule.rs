//! **The close rule, said once** (step R2.3): what closing a tab would lose, and what each actor does about it. The `close` command asks it of the session;
//! the window asks it of what the session thread published, as the UI thread may not take the session's lock, from the same facts.
//!
//! What a close would lose: unsaved edits, and undone steps, the owner's or an agent's. What each actor does with that:
//! - **A person is asked** before a close that would lose unsaved edits or their own undone steps: the close prompt, as any Mac app asks.
//! - **An agent is refused** a close that would lose unsaved edits, at every level (`discard` first, then `close`, so closing never loses them silently), and
//!   one that would lose the owner's undone steps below Full, as they stop an edit. At Full it goes ahead, and the owner's undone steps go with the tab.
//! - **The person's `close` itself**, sent once the prompt's been answered, is refused for unsaved edits that came in since, as an agent's at Full is.
//! - **What a close would lose can't all be known** when the tab's list is asleep and its journal couldn't be read back: an agent is refused, at every
//!   level, and the person, who sees the tab say so, decides. Unknown is "would lose", not "nothing to lose" (coordinator, 2026-11-13).

use crate::{ASK_FULL, Actor, AgentError, Level, exit};
use recto_session::edit::PageEdits;
use recto_session::{DocLoader, Session, TabId};

/// What closing a tab would lose.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WouldLose {
    /// Unsaved edits.
    pub unsaved: bool,
    /// Undone steps, anyone's, which can still be redone.
    pub undone: bool,
    /// Undone steps of the owner's among them.
    pub owners_undone: bool,
    /// Edits of the owner's anywhere in the tab's history.
    pub owners_anywhere: bool,
    /// Some of the history is past the undo limit, so `undo` can't take all of it off.
    pub history_dropped: bool,
    /// The list couldn't be read back, so only its summary is known: not all of what a close would lose.
    pub unreadable: bool,
}

impl WouldLose {
    /// What closing `id` would lose now, wherever its list is, from its summary. Every command about a tab reads its list back first, so one still asleep
    /// here is one whose journal couldn't be read: [`Self::unreadable`].
    pub fn of<L: DocLoader>(session: &Session<L>, id: TabId) -> Self {
        let summary = session.edits_summary(id).unwrap_or_default();
        Self {
            unsaved: summary.edited,
            undone: summary.redo > 0,
            owners_undone: summary.owner_steps_undone,
            owners_anywhere: summary.holds_owner_edits,
            history_dropped: session.page_edits(id).is_some_and(PageEdits::history_dropped),
            unreadable: session.list_is_asleep(id),
        }
    }

    /// What the window has of it, as the session thread published it: the tab showing's unsaved edits and the owner's undone steps there.
    #[must_use]
    pub fn as_published(unsaved: bool, owners_undone: bool) -> Self {
        Self {
            unsaved,
            owners_undone,
            ..Self::default()
        }
    }

    /// Whether a person is asked before the close: it would lose unsaved edits, or their own undone steps.
    #[must_use]
    pub fn asks_the_person(&self) -> bool {
        self.unsaved || self.owners_undone
    }

    /// **Why `close` is refused for `actor`,** if it is, saying all of what it would lose and every way on that's real.
    #[must_use]
    pub fn refusal(&self, actor: Actor) -> Option<AgentError> {
        if self.unreadable && actor != Actor::Person {
            return Some(
                AgentError::new(
                    "edits_unreadable",
                    "This tab wasn't closed: its edits couldn't be read back just now, so what closing it would lose can't all be known.",
                    exit::FAILED,
                )
                .then("Leave the tab open: it tells the user its edits couldn't be read, and closing it is theirs to decide. `recto session show` says what's known of them."),
            );
        }
        let protected = actor.level() < Level::Full;
        let owners_undone = protected && self.owners_undone;
        let owners_anywhere = protected && self.owners_anywhere;
        if !self.unsaved && !owners_undone {
            return None;
        }
        let (unsaved, undone) = (self.unsaved, self.undone);
        let what = match (unsaved, undone) {
            (true, true) => "unsaved edits and undone steps",
            (true, false) => "unsaved edits",
            _ => "undone steps",
        };
        let whose = if owners_anywhere { " Some of them are the user's." } else { "" };
        let mut ways = vec!["Leave the tab open: nothing is lost until the session ends.".to_string()];
        if undone {
            ways.push("`recto redo` puts the undone steps back, and that includes anything you undid on purpose.".to_string());
        }
        if unsaved {
            if self.history_dropped {
                ways.push("`recto undo` can't take all of it off, because some of it is past the undo limit.".to_string());
            } else {
                ways.push("`recto undo` until there's nothing left to undo takes the unsaved edits off, and keeps them as undone steps.".to_string());
            }
        }
        let discard = if undone {
            "`recto discard` throws all of it away for good, and then the tab can close"
        } else {
            "`recto discard` throws them away for good, and then the tab can close"
        };
        if owners_anywhere {
            ways.push(format!(
                "{discard}, which needs Full access, because it would lose the user's edits: `{ASK_FULL}` asks for it."
            ));
        } else {
            ways.push(format!("{discard}."));
        }
        ways.push("`recto save` puts them in the file, if the user wants that.".to_string());
        let message = format!("This tab wasn't closed: closing it would lose its {what}.{whose}");
        // The user's undone steps are what the level is there to protect. Otherwise it's the edits themselves that stop it, at any level, and `discard` is the way on.
        let mut error = if owners_undone {
            AgentError::new("would_clear_redo", message, exit::PERMISSION)
        } else {
            AgentError::new("unsaved_changes", message, exit::FAILED)
        };
        if owners_anywhere {
            error.ask = Some(ASK_FULL.into());
        }
        error.next = ways.join(" ");
        Some(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: the close rule answers one actor otherwise than it says: a person not asked before losing unsaved edits or their own undone steps, an agent
    /// let close over unsaved edits at any level or over the owner's undone steps below Full, or one at Full stopped by the owner's undone steps alone.
    #[test]
    fn each_actor_does_what_the_rule_says_with_what_a_close_would_lose() {
        let code = |lose: WouldLose, actor: Actor| lose.refusal(actor).map(|error| error.code);
        let unsaved = WouldLose {
            unsaved: true,
            ..WouldLose::default()
        };
        let owners_undone = WouldLose {
            undone: true,
            owners_undone: true,
            owners_anywhere: true,
            ..WouldLose::default()
        };
        let agents_undone = WouldLose {
            undone: true,
            ..WouldLose::default()
        };
        assert!(unsaved.asks_the_person() && owners_undone.asks_the_person());
        assert!(!agents_undone.asks_the_person() && !WouldLose::default().asks_the_person());
        for level in [Level::ReadOnly, Level::Control, Level::Full] {
            assert_eq!(code(unsaved, Actor::Agent(level)), Some("unsaved_changes"), "{level:?}");
            assert_eq!(code(agents_undone, Actor::Agent(level)), None, "an agent's own undone steps, {level:?}");
        }
        assert_eq!(code(owners_undone, Actor::Agent(Level::Control)), Some("would_clear_redo"));
        assert_eq!(code(owners_undone, Actor::Agent(Level::Full)), None, "at Full it goes ahead");
        assert_eq!(code(unsaved, Actor::Person), Some("unsaved_changes"), "the person's close after the prompt");
        assert_eq!(code(owners_undone, Actor::Person), None);
        let unreadable = WouldLose {
            unreadable: true,
            ..WouldLose::default()
        };
        for level in [Level::ReadOnly, Level::Control, Level::Full] {
            assert_eq!(
                code(unreadable, Actor::Agent(level)),
                Some("edits_unreadable"),
                "unknown is would lose, {level:?}"
            );
        }
        assert_eq!(code(unreadable, Actor::Person), None, "the person decides");
    }
}
