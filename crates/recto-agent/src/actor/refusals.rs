//! **What an agent below Full is told when a rule of [`super::Actor`]'s refuses it**, each in one place: what's at stake of the owner's edits, and what
//! needs Full because it takes something out of a file for good or writes over one. The rule that decides is on `Actor`; these are only its words.

use crate::{AgentError, Code, exit};
use recto_session::TabId;
use recto_session::{DocLoader, Session};

/// What asking for Full is for, where only that would do. The one command an agent can run to get there.
pub(crate) const ASK_FULL: &str = "recto permission request full";

/// **What of the owner's edits a command would lose** (SPEC decision 20: below Full, an agent can throw away only agents' edits, never the owner's),
/// each with how it's known from the session and its one refusal.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Stake {
    /// A new edit on this tab, which clears what's undone: the owner's undone steps there go for good. Undone steps can still be got back, and a new
    /// edit clears them, which is how an agent could erase the user's edits: undo them all, make one edit. Undone steps of agents' own are theirs to lose.
    UndoneSteps(TabId),
    /// `discard` or `clear-history` of this tab: everything on its list, undone or not, and whatever fell off its capped undo list too, since that's
    /// still in the page order.
    Discard(TabId),
    /// `session restore`, which replaces the whole session.
    Restore,
    /// A save of this tab that leaves out a page the owner's steps about the file's highlights name, which can't be carried past it (coordinator,
    /// 2026-11-14). Such a save is a rewrite too, which below Full is refused anyway; this says why.
    SaveDropsSteps(TabId),
}

impl Stake {
    /// Whether the session holds anything of the owner's this would lose.
    pub(crate) fn at_stake<L: DocLoader>(self, session: &Session<L>) -> bool {
        let lists = session.lists();
        match self {
            Self::UndoneSteps(id) => lists.has_undone_owner_steps(id),
            Self::Discard(id) => lists.summary(id).is_some_and(|summary| summary.holds_owner_edits),
            Self::Restore => lists.holds_owner_edits(),
            Self::SaveDropsSteps(id) => lists.get(id).is_some_and(recto_session::edit::marks::drops_owner_steps),
        }
    }

    /// The refusal, below Full.
    pub(crate) fn refusal(self) -> AgentError {
        match self {
            Self::UndoneSteps(_) => {
                let mut error = AgentError::new(
                    Code::WOULD_CLEAR_REDO,
                    "A new edit here would throw away the user's edits, undone but still there to redo, and that needs Full access.",
                    exit::PERMISSION,
                );
                error.ask = Some(ASK_FULL.into());
                error.next = format!(
                    "Run `recto redo` until there's nothing left to redo, which keeps everything, and then go on. Or run `{ASK_FULL}` to ask for Full access."
                );
                error
            }
            Self::Discard(_) => {
                let mut error = AgentError::new(
                    Code::WOULD_DISCARD_USER_EDITS,
                    "Nothing was discarded: some of this tab's edits are the user's, and throwing those away needs Full access.",
                    exit::PERMISSION,
                );
                error.ask = Some(ASK_FULL.into());
                error.next = format!("`recto undo` steps back through them, and keeps them. Run `{ASK_FULL}` to ask for Full access, or tell the user.");
                error
            }
            // These two point the agent to the user and not to Full (PLAN, the owner's to say).
            Self::Restore => AgentError::new(
                Code::OWNER_EDITS_AT_STAKE,
                "Restoring would replace this session, which holds the user's own unsaved edits, so it wasn't done.",
                exit::PERMISSION,
            )
            .then("Ask them to save or discard those first."),
            Self::SaveDropsSteps(_) => AgentError::new(
                Code::OWNER_EDITS_AT_STAKE,
                "Nothing was saved: the user has undo steps about highlights on a page this save leaves out, which it would drop.",
                exit::PERMISSION,
            )
            .then("Ask them to save, or put the page back first."),
        }
    }
}

/// **An agent's `discard` or `clear-history` of a tab whose edits couldn't be read back:** what it would throw away can't all be known. `what` is what
/// wasn't done, as "discarded".
pub(crate) fn unreadable(what: &str) -> AgentError {
    AgentError::new(
        Code::EDITS_UNREADABLE,
        format!("Nothing was {what}: this tab's edits couldn't be read back just now, so what that would throw away can't all be known."),
        exit::FAILED,
    )
    .then("Leave them: the tab tells the user its edits couldn't be read, and what to do with them is theirs. `recto session show` says what's known of them.")
}

/// **What only Full may write** (SPEC section 5; the coordinator's decision, 2026-11-08): a save that takes what was removed out of a file for good,
/// and a save over another file. At Control an agent may append and Save As to a new file, which lose nothing.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Rewrite {
    /// A Save As over a file that's there.
    ReplacesAnotherFile,
    /// A rewrite, as the edits ask for one, that takes out deleted pages.
    RemovesPages,
    /// A rewrite, as the edits ask for one, that takes out a highlight of the file's.
    TakesOutHighlight,
    /// A rewrite the writer chose once it read the file (a signed or certified file's copy).
    WritesAnew,
}

impl Rewrite {
    /// The refusal, below Full.
    pub(crate) fn refusal(self) -> AgentError {
        let what = match self {
            Self::ReplacesAnotherFile => "Replacing another file",
            Self::RemovesPages => "A save that removes pages for good",
            Self::TakesOutHighlight => "A save that takes a highlight out of the file for good",
            Self::WritesAnew => "A save that writes the file anew",
        };
        let mut error = AgentError::new(Code::PERMISSION_DENIED, format!("{what} needs Full access."), exit::PERMISSION);
        error.next = format!("Run `{ASK_FULL}`, which tells the user what to allow. Nothing else raises this.");
        error.ask = Some(ASK_FULL.into());
        error
    }
}
