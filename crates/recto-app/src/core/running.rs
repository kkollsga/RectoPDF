//! **What's running, said once** (review of the lane, D6): each part's own count, read together, for the two that wait on it. A hand-off goes only with
//! nothing at all under way ([`Running::busy`]); a quit waits, within its patience, for what can change what it writes ([`Running::changes_what_a_quit_writes`]),
//! a named subset of the same counts. So a command that's counted in one place for a while and in another after (a Save, counted the person's as it's
//! asked, then a save of the window's as its thread starts, then a command as it runs) is never in neither for either.

use super::{AppHost, BusyItem, CoreQueue, Lifecycle, Tools};
use std::sync::atomic::Ordering;

/// What's under way now, by each part's own count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Running {
    /// Sheets and panels open over the window.
    pub sheets: u32,
    /// Commands running through the app's host, the person's and agents', but those that only ask.
    pub commands: u32,
    /// Those of them anything of which can be in what a quit writes ([`super::command_kinds::changes`]).
    pub changing: u32,
    /// The person's commands on threads of their own, from when they're asked to when what came of them is said, their edits parked in the lane among them.
    pub persons: u32,
    /// Files being read off the session thread, for a tab or an open, a person's command among them to run again once it's in.
    pub reads: u32,
    /// The window's saves, from their turn to what came of them queued.
    pub saves: u32,
    /// A print being prepared, or with the system's printing.
    pub printing: bool,
}

impl Running {
    /// What's under way now, from the app's life, its host and its tools.
    pub fn now(life: &Lifecycle, host: &AppHost, tools: &Tools) -> Self {
        Self {
            sheets: life.sheets_open(),
            commands: host.running.load(Ordering::SeqCst),
            changing: host.changing.load(Ordering::SeqCst),
            persons: life.persons_running(),
            reads: life.reads_running(),
            saves: tools.save_ui().running.load(Ordering::SeqCst),
            printing: tools.printing().busy(),
        }
    }

    /// **The hand-off's: everything under way,** each named in the person's words.
    #[must_use]
    pub fn busy(&self) -> Vec<BusyItem> {
        super::hand_off_check_parts(self.reads, self.sheets, self.commands, self.persons, self.saves, self.printing)
    }

    /// **The quit's: what can change what it writes** (the record, a journal, a document's file): the person's commands, their parked edits among
    /// them, the window's saves, files being read for a command, and commands that change something (an agent's edit, a jump). Not a sheet, a print, or an
    /// agent's read, render or export, which a quit doesn't wait for (D7).
    #[must_use]
    pub fn changes_what_a_quit_writes(&self) -> bool {
        self.persons > 0 || self.saves > 0 || self.reads > 0 || self.changing > 0
    }
}

/// **Something counted here has ended:** whoever waits for what's running looks again (a quit's wait, woken as the session thread parks once nudged).
/// Every count's end says so here, the last one's among them.
pub(crate) fn ended(core: &CoreQueue) {
    core.nudge();
}
