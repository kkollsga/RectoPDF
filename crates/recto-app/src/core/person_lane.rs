//! **The person's edits, in the order they asked for them** (the ⌃⌘H-then-⌘Z fault, 2026-11-14): which of the person's commands keep their order, and how
//! each is started when its turn comes in the lane ([`crate::queue::lane`], which holds only the order). Nothing here waits: a command whose turn hasn't
//! come is parked, with no thread, and started by whoever passes the turn before it.
//!
//! **What's ordered:** what changes a document's state or fixes what's read of it: every edit (highlight and its removal, undo, redo, the page edits,
//! discard, clear history, save), a print (until its pages are read) and a close (so the close prompt's Don't Save, a discard then a close, stays in its
//! order). Not a jump, a zoom, a tap, Find, the view or edit mode: those go as they always have. An agent's commands never come here: the lane is the
//! person's only.
//!
//! **When each passes its turn** (coordinator, 2026-11-14: a command holds the turn only until the document's state it acts on is fixed): a command on the
//! session thread once it has run; a highlight or its removal as it lands; a print once its pages are read (`printing::Watch::begin`); a save once it has
//! begun, its tab held for it (`saving::AppSaving::before_writing`).

use super::person;
use super::{CoreCall, Inner};
use crate::queue::lane::{Asked, Start, Turn};
use crate::queue::{Origin, PersonCommand};
use recto_agent::commands::{Edit, Move, Read};
use recto_agent::{AgentError, Command, TabRef};
use recto_session::TabId;
use std::sync::Arc;

/// What the person is told when an edit isn't taken because too many are waiting already ([`crate::queue::lane::CAP`]): Rust's words, by the page
/// (coordinator, 2026-11-14).
pub const FULL: &str = "That wasn't done: too many edits are waiting for the one before. Try again in a moment.";

/// **Whether a person's command keeps its place among their edits**: [`super::command_kinds`] says, by its tag, in a list with no default.
pub use super::command_kinds::ordered;

/// **The tab an ordered command names as it's asked:** its own number; for "the tab showing", the one the window shows now if it runs on a thread of its
/// own, which takes the session as it is when it starts; and none yet (0) for one the session thread runs, which names the tab showing as it takes it
/// ([`PersonsLane::taken`]).
fn named_tab(published: &std::sync::Mutex<super::Published>, command: &Command) -> u32 {
    match command_tab(command) {
        Some(TabRef::Id(id)) => id.0,
        Some(TabRef::Active) if runs_on_its_own_thread(command) => crate::lock(published).reader.shown_tab,
        _ => 0,
    }
}

/// Whether a person's command runs on a thread of its own: one that may wait, or isn't the session's to answer.
pub(super) fn runs_on_its_own_thread(command: &Command) -> bool {
    command.waits() || command.route() != recto_agent::Runs::Session
}

/// **Whether an ordered command is an edit that lands later** ([`crate::queue::lane::Lane::landing`]): one that can change the document, run on a thread
/// of its own, so the session publishes what it did only once it lands: a highlight, its removal, and an inserted page (which no window path sends today).
/// Not an undo, a turn or a close, which the session thread runs in the order taken; not a save or a print, which change no edit.
#[must_use]
pub fn lands_later(command: &Command) -> bool {
    runs_on_its_own_thread(command) && matches!(command, Command::Edit(Edit::Highlight(_) | Edit::HighlightRemove(_) | Edit::PageInsert(_)))
}

/// **The person's lane, as the app uses it** ([`person::Person::lane`]): a ticket for each ordered command, its start now or when its turn comes.
#[derive(Clone, Copy)]
pub struct PersonsLane<'a> {
    pub(super) app: &'a Arc<Inner>,
}

impl PersonsLane<'_> {
    /// A ticket for the person's command, if it's an ordered one.
    pub fn turn_for(self, command: &Command) -> Option<Turn> {
        ordered(command).then(|| self.app.core.take_turn(named_tab(&self.app.published, command), lands_later(command)))
    }

    /// **A person's ordered command that runs on a thread of its own** (one that may wait): started now if it's its turn, or parked till it is, counted
    /// as the person's running meanwhile. Never waits.
    pub fn submit_waiting(self, command: Command, turn: Turn) {
        let counted = person::Counted::new(&self.app.life, &self.app.core);
        let app = Arc::downgrade(self.app);
        let start: Start = Box::new(move |turn| {
            let started = app.upgrade().map(|inner| person::Person::of(&inner).lane().start_waiting(command, Some(turn)));
            drop((started, counted));
        });
        self.asked(self.app.core.lane().ask(turn, start));
    }

    /// **The person's Save of `tab`, asked:** started now if it's its turn, or parked till it is. `start` runs it, holding its turn. Its ticket is taken
    /// as any ordered command's is ([`Self::turn_for`]): a save is ordered by [`ordered`], the one place that says so.
    pub fn submit_save(self, tab: TabId, start: Start) {
        let save = Command::from(recto_agent::commands::saving::Save {
            tab: TabRef::Id(tab),
            out: None,
            replace: false,
        });
        // Counted first, then looked at again, as `person::submit` does: frozen since its caller looked, it isn't taken.
        let counted = person::Counted::new(&self.app.life, &self.app.core);
        if self.app.core.frozen.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let turn = self.turn_for(&save).unwrap_or_else(|| {
            debug_assert!(false, "a save is ordered");
            self.app.core.take_turn(tab.0, false)
        });
        let start: Start = Box::new(move |turn| {
            start(turn);
            drop(counted);
        });
        self.asked(self.app.core.lane().ask(turn, start));
    }

    /// What came of asking: started now, parked, or refused at the cap, which the person is told.
    fn asked(self, asked: Asked) {
        match asked {
            Asked::Now(start, turn) => start(turn),
            Asked::Parked => {}
            Asked::Full(start, turn) => {
                drop((start, turn));
                person::say(self.app.window.notices(), &self.app.core, FULL.to_string());
            }
        }
    }

    /// **A person's command that runs on a thread of its own, started:** holding its turn in the lane, if it's an ordered one, till it passes.
    pub fn start_waiting(self, command: Command, turn: Option<Turn>) {
        // Counted first, and then looked: a late try at the record's lock decides on keeping it (and restoring) only with none counted, under the life's
        // lock (`keeping::try_back`), so either it sees this one and tries again later, or this one sees it restoring and waits on the queue with what else
        // the person does, to run on the restored session.
        let counted = person::Running::start(Arc::clone(self.app));
        if self.app.life.keeping().restoring() {
            drop(counted);
            self.app.core.push(CoreCall::Run(turn_come(PersonCommand::input(command), turn)));
            return;
        }
        person::on_its_own_thread(command, counted, turn);
    }

    /// A person's command held while a late keep's restore was read, let go: one that runs on a thread of its own goes there, with its turn if it holds one.
    pub fn submit_held(self, run: Box<PersonCommand>) {
        match run.turn {
            Some(turn) => self.start_waiting(run.command, Some(turn)),
            None => {
                person::submit(self.app, run.command);
            }
        }
    }

    /// **A person's ordered command the session thread has taken:** run now, given back, if its turn has come; parked otherwise, made what it is by the
    /// session as it is now, in the order of what the person did (the tab showing named as the one it is now, a rotate's pages read now), and run when its
    /// turn comes. `None` when it was parked or refused.
    pub fn taken(self, run: PersonCommand) -> Option<PersonCommand> {
        let PersonCommand {
            command,
            origin,
            fetched,
            when_run,
            turn,
        } = run;
        let lane = self.app.core.lane();
        // Named as the session thread takes it, if it said "the tab showing": the tab showing now, in the order of what the person did.
        if let Some(turn) = &turn
            && !lane.is_named(turn)
        {
            let tab = match command_tab(&command) {
                Some(TabRef::Id(id)) => Some(id),
                Some(TabRef::Active) => self.app.host.session.with_session(|session| session.tabs().active()),
                _ => None,
            };
            lane.name(turn, tab.map_or(0, |tab| tab.0));
        }
        let turn = match turn {
            Some(turn) if !lane.may_run(&turn) => turn,
            turn => {
                return Some(PersonCommand {
                    command,
                    origin,
                    fetched,
                    when_run,
                    turn,
                });
            }
        };
        // Made what it is now: what it names is what the person meant as they asked, whatever's tapped while it waits.
        let resolved = self.app.host.session.with_session(|session| {
            let command = match when_run {
                Some(resolve) => resolve(session, recto_agent::Actor::Person, command.clone())?,
                None => command.clone(),
            };
            Ok::<_, AgentError>(named(command, session.tabs().active()))
        });
        let command = match resolved {
            Ok(command) => command,
            Err(error) => {
                person::answered(&super::answer_parts::AnswerParts::of(self.app), &command, origin, &Err(error));
                return None;
            }
        };
        let counted = person::Counted::new(&self.app.life, &self.app.core);
        let core = Arc::downgrade(&self.app.core);
        let start: Start = Box::new(move |turn| {
            if let Some(core) = core.upgrade() {
                let run = Box::new(PersonCommand {
                    command,
                    origin,
                    fetched,
                    when_run: None,
                    turn: None,
                });
                core.push(CoreCall::Run(turn_come(run, Some(turn))));
            }
            drop(counted);
        });
        // Its turn may have come meanwhile, as the command before passed it: then it's queued again at once, named as it is, and runs as it's taken.
        self.asked(self.app.core.lane().ask(turn, start));
        None
    }
}

/// A person's command given the turn it holds, if any: one whose turn has come was asked before whatever freezes the queue now, and is let through
/// ([`Origin::TurnCame`]); the end of something under way stays that.
fn turn_come(mut run: Box<PersonCommand>, turn: Option<Turn>) -> Box<PersonCommand> {
    if turn.is_some() && run.origin == Origin::Input {
        run.origin = Origin::TurnCame;
    }
    run.turn = turn;
    run
}

/// The tab a command names, if it names one.
fn command_tab(command: &Command) -> Option<TabRef> {
    match command {
        Command::Edit(edit) => edit.tab().cloned(),
        Command::Read(read) => read.tab().cloned(),
        Command::Move(moved) => moved.tab().cloned(),
        _ => None,
    }
}

/// `command` with "the tab showing" named as `active`, the tab showing now.
fn named(mut command: Command, active: Option<TabId>) -> Command {
    let Some(active) = active else { return command };
    let tab = match &mut command {
        Command::Edit(edit) => match edit {
            Edit::PageRotate(asked) => &mut asked.tab,
            Edit::PageMove(asked) => &mut asked.tab,
            Edit::PageDelete(asked) => &mut asked.tab,
            Edit::PageInsert(asked) => &mut asked.tab,
            Edit::Discard(asked) => &mut asked.tab,
            Edit::Undo(asked) => &mut asked.tab,
            Edit::Redo(asked) => &mut asked.tab,
            Edit::ClearHistory(asked) => &mut asked.tab,
            Edit::Save(asked) => &mut asked.tab,
            Edit::Highlight(asked) => &mut asked.tab,
            Edit::HighlightRemove(asked) => &mut asked.tab,
            Edit::Edits(_) | Edit::EditMode(_) => return command,
        },
        Command::Read(Read::Print(asked)) => &mut asked.tab,
        Command::Move(Move::Close(asked)) => &mut asked.tab,
        _ => return command,
    };
    if *tab == TabRef::Active {
        *tab = TabRef::Id(active);
    }
    command
}

#[cfg(test)]
mod tests {
    /// Failure: a command that changes a document or fixes what's read of it goes around the lane (and can run before an edit asked ahead of it), or one
    /// that only moves the view waits in it behind a highlight's read. Every edit but the list and edit mode, a save, a print and a close are ordered; a
    /// jump, a tap, Find, the view, edit mode and the reads aren't.
    #[test]
    fn what_keeps_its_order_is_what_changes_or_fixes_a_document() {
        let ordered = |line: &str| super::ordered(&recto_agent::parse(line).expect("a command"));
        for line in [
            r#"{"cmd":"undo"}"#,
            r#"{"cmd":"redo"}"#,
            r#"{"cmd":"page_rotate","pages":"all","degrees":90}"#,
            r#"{"cmd":"discard"}"#,
            r#"{"cmd":"clear_history"}"#,
            r#"{"cmd":"save"}"#,
            r#"{"cmd":"highlight","page":1,"chars":"0-4"}"#,
            r#"{"cmd":"highlight_remove","id":1}"#,
            r#"{"cmd":"print","to_pdf":"/tmp/a.pdf"}"#,
            r#"{"cmd":"close"}"#,
        ] {
            assert!(ordered(line), "{line} keeps its order");
        }
        for line in [
            r#"{"cmd":"goto","page":2}"#,
            r##"{"cmd":"tap","tab":"#1"}"##,
            r#"{"cmd":"edits"}"#,
            r#"{"cmd":"edit_mode","set":"on"}"#,
            r#"{"cmd":"view","sidebar":"off"}"#,
            r#"{"cmd":"text","page":1}"#,
        ] {
            assert!(!ordered(line), "{line} goes as it always has");
        }
    }
}
