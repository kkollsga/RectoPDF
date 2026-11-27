//! **The window's Clear Leftovers** (D6): the person's `removal check` and `removal clear`, which an agent sends as well, shown as a sheet over the window in
//! Rust's words. Nothing here is a command of its own: the Edit menu's row sends the person's `removal check` for the tab being read, the answer of any of the
//! person's checks becomes the sheet, the sheet's yes sends the person's `removal clear` with the plan, and the answer of any of the person's clears is the
//! notice. An agent's commands are the same commands and never answered here ([`super::super::person`]'s list runs for the person's alone).
//!
//! At rest the sheet is one empty slot and a counter: no thread, no buffer. The plan lives in the sheet's state only until the yes, and is never in the
//! [`SheetModel`], in a log or in `qa removal-state`.

use super::{lock, words};
use crate::core::HostAction;
use crate::core::answer_parts::AnswerParts;
use crate::core::menu_rows::{Asked, Row};
use crate::core::person::{Person, Ran};
use crate::core::row_parts::RowParts;
use crate::core::sheets::SheetModel;
use recto_agent::commands::Removal as Family;
use recto_agent::commands::removal::{RemovalCheck, RemovalClear};
use recto_agent::{AgentError, Code, Command, TabRef};
use recto_session::TabId;
use serde_json::Value;
use std::sync::Mutex;

/// The Edit menu's row.
pub const MENU_ID: &str = "edit.clear_leftovers";
/// The sheet's kind, which an answer names.
pub const KIND: &str = "clear_leftovers";

/// What the sheet is about.
enum What {
    /// A plan to confirm, which the yes sends.
    Offer { plan: String },
    /// A look that found nothing to confirm.
    Looked,
    /// A stop or a refusal: `again` when looking again may help, `stopped` when a clear began and didn't finish.
    Told { again: bool, stopped: bool },
}

impl std::fmt::Debug for What {
    /// The plan is never written out.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offer { .. } => out.write_str("Offer"),
            Self::Looked => out.write_str("Looked"),
            Self::Told { again, stopped } => write!(out, "Told {{ again: {again}, stopped: {stopped} }}"),
        }
    }
}

#[derive(Debug)]
struct Shown {
    id: u64,
    tab: u32,
    what: What,
    words: String,
}

/// The sheet's state: the one showing, boxed so none costs a byte at rest, and the id the next will have (from 1, never used twice).
#[derive(Debug)]
pub struct Leftovers(Mutex<Slot>);

#[derive(Debug)]
struct Slot {
    next: u64,
    shown: Option<Box<Shown>>,
}

impl Default for Leftovers {
    fn default() -> Self {
        Self(Mutex::new(Slot { next: 1, shown: None }))
    }
}

/// The codes whose own sentence is the person's: each is made by `words.rs` or `removal.rs` for a person to read. Any other code (a usage error, a level's
/// refusal, a failure with a path in it) shows [`words::COULDNT`], never its message.
fn is_the_persons_words(code: Code) -> bool {
    matches!(
        code,
        Code::REMOVAL_BUSY
            | Code::REMOVAL_PLAN_CHANGED
            | Code::REMOVAL_PLAN_EXPIRED
            | Code::REMOVAL_UNKNOWN_PLAN
            | Code::REMOVAL_CANT_CLEAR
            | Code::REMOVAL_STOPPED
            | Code::NO_SUCH_TAB
            | Code::FILE_MISSING
    )
}

/// A command dropped from the person's lane is answered `CANCELLED`, which no feature has words for ([`crate::core::person`]).
fn is_cancelled(result: &Result<Value, AgentError>) -> bool {
    result.as_ref().is_err_and(|error| error.code == Code::CANCELLED)
}

impl Leftovers {
    fn show(&self, tab: u32, what: What, words: String) {
        let mut slot = lock(&self.0);
        let id = slot.next;
        slot.next += 1;
        slot.shown = Some(Box::new(Shown { id, tab, what, words }));
    }

    /// A failed check or clear, as a sheet in the person's words, decided by the code alone. A clear that failed in a way with no sentence says it didn't
    /// finish, never that nothing was cleared: it began and may have done part.
    fn told(&self, tab: u32, error: &AgentError, clearing: bool) {
        let known = is_the_persons_words(error.code);
        let words = match (known, clearing) {
            (true, _) => error.message.clone(),
            (false, true) => words::COULDNT_CLEAR.to_owned(),
            (false, false) => words::COULDNT.to_owned(),
        };
        let gone = matches!(error.code, Code::NO_SUCH_TAB | Code::FILE_MISSING);
        self.show(
            tab,
            What::Told {
                again: !gone,
                stopped: error.code == Code::REMOVAL_STOPPED || (clearing && !known),
            },
            words,
        );
    }

    /// The person's `removal check` or `removal clear` was answered. A clear that finished leaves no sheet, and gives its words to be said by the page.
    pub fn answered(&self, family: &Family, result: &Result<Value, AgentError>) -> Option<String> {
        if is_cancelled(result) {
            return None;
        }
        match (family, result) {
            // A tab named any other way than by number can't be asked about again by a sheet; the menu and the palette always name it by number.
            (Family::RemovalCheck(RemovalCheck { tab: TabRef::Id(TabId(tab)) }), result) => {
                match result {
                    Ok(data) => {
                        let words = data["words"].as_str().unwrap_or_default().to_owned();
                        let what = match data["plan"].as_str() {
                            Some(plan) => What::Offer { plan: plan.to_owned() },
                            None => What::Looked,
                        };
                        self.show(*tab, what, words);
                    }
                    Err(error) => self.told(*tab, error, false),
                }
                None
            }
            (
                Family::RemovalClear(RemovalClear {
                    tab: TabRef::Id(TabId(tab)), ..
                }),
                result,
            ) => match result {
                Ok(data) => {
                    lock(&self.0).shown = None;
                    data["words"].as_str().map(str::to_owned)
                }
                Err(error) => {
                    self.told(*tab, error, true);
                    None
                }
            },
            _ => None,
        }
    }

    /// The sheet showing, as the host draws it: the plan is not in it.
    pub fn showing(&self) -> Option<SheetModel> {
        let slot = lock(&self.0);
        let shown = slot.shown.as_deref()?;
        let (title, buttons, destructive) = match &shown.what {
            What::Offer { .. } => (words::OFFER_TITLE, vec![words::BUTTON, words::CANCEL], Some(0)),
            What::Looked => (words::LOOKED_TITLE, vec![words::OK], None),
            What::Told { again, stopped } => (
                if *stopped { words::STOPPED_TITLE } else { words::REFUSED_TITLE },
                if *again { vec![words::LOOK_AGAIN, words::OK] } else { vec![words::OK] },
                None,
            ),
        };
        Some(SheetModel {
            kind: KIND.into(),
            id: shown.id,
            title: title.into(),
            message: shown.words.clone(),
            buttons: buttons.into_iter().map(String::from).collect(),
            destructive,
        })
    }

    /// What the sheet is, for QA: `none`, `offer`, `looked` or `told`.
    pub fn kind(&self) -> &'static str {
        match lock(&self.0).shown.as_deref().map(|shown| &shown.what) {
            None => "none",
            Some(What::Offer { .. }) => "offer",
            Some(What::Looked) => "looked",
            Some(What::Told { .. }) => "told",
        }
    }

    /// A button was chosen on sheet `id`: the command it sends, as the person's, if any. A sheet that isn't the one showing is left alone. The sheet goes
    /// with any answer, and the plan with it, so a second yes sends nothing. Only the first button of an offer sends a clear; any other button, a number
    /// that's no button and [`NO_BUTTON`](crate::core::sheets::NO_BUTTON) send nothing.
    pub fn answer(&self, id: u64, button: u32) -> Option<Command> {
        let mut slot = lock(&self.0);
        if slot.shown.as_ref().map(|shown| shown.id) != Some(id) {
            return None;
        }
        let Shown { tab, what, .. } = *slot.shown.take()?;
        let tab = TabRef::Id(TabId(tab));
        match (what, button) {
            (What::Offer { plan }, 0) => Some(Command::Removal(Family::RemovalClear(RemovalClear { tab, confirm: plan }))),
            (What::Told { again: true, .. }, 0) => Some(Command::Removal(Family::RemovalCheck(RemovalCheck { tab }))),
            _ => None,
        }
    }
}

/// **The Edit menu's Clear Leftovers…** ([`crate::core::menu_rows`]): the person's `removal check` for the tab being read, whose answer is the sheet.
pub fn row<'a>(parts: &RowParts<'a>, asked: &Asked<'_>) -> Option<Row<'a>> {
    if asked.id != MENU_ID {
        return None;
    }
    Some(match parts.reading_tab() {
        Some(tab) => Row::command(RemovalCheck { tab: TabRef::Id(tab) }),
        None => Row::Nothing,
    })
}

/// The person's `removal check` or `removal clear` went: the sheet, or the notice. Only the person's commands come here, so an agent's never opens a sheet.
pub fn answered(parts: &AnswerParts<'_>, ran: &Ran<'_>) {
    let Command::Removal(family) = ran.command else { return };
    // A read still to come is no answer.
    if (ran.result.is_err() && ran.refused().is_none()) || is_cancelled(ran.result) {
        return;
    }
    parts.leftovers_answered(|leftovers| leftovers.answered(family, ran.result));
}

/// A sheet's button, answered: the command it sends goes out as the person's, on a thread of its own, so the window never waits for the clear.
pub fn answer_sheet(person: Person<'_>, removal: &super::Removal, id: u64, button: u32) -> HostAction {
    if let Some(command) = removal.sheet().answer(id, button) {
        // A hand-off that has taken its last look takes nothing more, and the sheet has gone either way.
        let _taken = person.submit(command);
    }
    HostAction::None
}

#[cfg(test)]
mod tests;
