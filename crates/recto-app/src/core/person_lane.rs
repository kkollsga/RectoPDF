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
use crate::queue::PersonCommand;
use crate::queue::lane::{Asked, Start, Turn};
use recto_agent::commands::{Edit, Move, Read};
use recto_agent::{AgentError, Command, TabRef};
use recto_session::TabId;
use std::sync::Arc;

/// What the person is told when an edit isn't taken because too many are waiting already ([`crate::queue::lane::CAP`]): Rust's words, by the page
/// (coordinator, 2026-11-14).
pub const FULL: &str = "That wasn't done: too many edits are waiting for the one before. Try again in a moment.";

/// **Whether a person's command keeps its place among their edits** (the module's notes).
#[must_use]
pub fn ordered(command: &Command) -> bool {
    match command {
        Command::Edit(edit) => !matches!(edit, Edit::Edits(_) | Edit::EditMode(_)),
        Command::Read(Read::Print(_)) | Command::Move(Move::Close(_)) => true,
        _ => false,
    }
}

/// The tab an ordered command names, for a close to ask whether an edit of its tab is still landing: its own number, or for "the tab showing", the
/// one the window shows now. 0 for none.
fn named_tab(published: &std::sync::Mutex<super::Published>, command: &Command) -> u32 {
    let tab = match command {
        Command::Edit(edit) => edit.tab(),
        Command::Read(read) => read.tab(),
        Command::Move(moved) => moved.tab(),
        _ => None,
    };
    match tab {
        Some(TabRef::Id(id)) => id.0,
        Some(TabRef::Active) => crate::lock(published).reader.shown_tab,
        _ => 0,
    }
}

/// **The person's lane, as the app uses it** ([`person::Person::lane`]): a ticket for each ordered command, its start now or when its turn comes.
#[derive(Clone, Copy)]
pub struct PersonsLane<'a> {
    pub(super) app: &'a Arc<Inner>,
}

impl PersonsLane<'_> {
    /// A ticket for the person's command, if it's an ordered one.
    pub fn turn_for(self, command: &Command) -> Option<Turn> {
        ordered(command).then(|| self.app.core.take_turn(named_tab(&self.app.published, command)))
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

    /// **The person's Save of `tab`, asked:** started now if it's its turn, or parked till it is. `start` runs it, holding its turn.
    pub fn submit_save(self, tab: TabId, start: Start) {
        let turn = self.app.core.take_turn(tab.0);
        let counted = person::Counted::new(&self.app.life, &self.app.core);
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
            let mut run = PersonCommand::input(command);
            match turn {
                // Its turn came: it was asked before whatever freezes the queue now.
                Some(turn) => {
                    run.turn = Some(turn);
                    self.app.core.push_turn_come(CoreCall::Run(run));
                }
                None => self.app.core.push(CoreCall::Run(run)),
            }
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
        let turn = match turn {
            Some(turn) if !self.app.core.lane().is_turn(&turn) => turn,
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
                core.push_turn_come(CoreCall::Run(Box::new(PersonCommand {
                    command,
                    origin,
                    fetched,
                    when_run: None,
                    turn: Some(turn),
                })));
            }
            drop(counted);
        });
        // Its turn may have come meanwhile, as the command before passed it: then it's queued again at once, named as it is, and runs as it's taken.
        self.asked(self.app.core.lane().ask(turn, start));
        None
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
