//! **The person's commands, on the session thread** (step R2.3): what the window asks for, run as the command an agent would send, by the one executor, as
//! the person's ([`Actor::Person`]). What one needs from a file is read off this thread, as the window's taps and opens always have been, and the command runs
//! again once it's in ([`recto_agent::later`]): a file on a dead volume holds up neither this thread nor a later tap.

use super::{FileReads, Inner};
use crate::queue::{Origin, PersonCommand, Reading};
use recto_agent::later::{Fetched, ReadLater};
use recto_agent::{Actor, AgentError, Command, Event};
use serde_json::Value;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Runs a person's command now. Its answer is nobody's but a test's: the window draws what the session publishes.
pub fn run(inner: &Arc<Inner>, reads: &mut FileReads, run: PersonCommand, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let PersonCommand { command, fetched, .. } = run;
    let later = Later {
        inner,
        reads: RefCell::new(reads),
        command: &command,
        fetched: RefCell::new(fetched),
    };
    inner.host.session.run_reading_later(Actor::Person, &command, events, &later)
}

/// What a person's command reads, read off the session thread.
struct Later<'a> {
    inner: &'a Arc<Inner>,
    reads: RefCell<&'a mut FileReads>,
    command: &'a Command,
    /// A file read for this command before it ran again, for it to take up.
    fetched: RefCell<Option<(PathBuf, Fetched)>>,
}

impl ReadLater for Later<'_> {
    /// As a tap's read always was: the tab the person last asked for is the one wanted, and a command for one they've left by the time it's read doesn't run.
    fn wake(&self, tab: recto_session::TabId, path: PathBuf) {
        let mut reads = self.reads.borrow_mut();
        reads.wanted_tab = Some(tab);
        reads.then.insert(Reading::Tab(tab), Box::new(self.command.clone()));
        reads.start(self.inner, Reading::Tab(tab), path);
    }

    /// As an open's read always was, in the order asked, each to its end.
    fn load(&self, path: &Path) -> Option<Fetched> {
        if let Some((_, fetched)) = self.fetched.borrow_mut().take_if(|(read, _)| read == path) {
            return Some(fetched);
        }
        let mut reads = self.reads.borrow_mut();
        reads.wanted_tab = None;
        reads.then.insert(Reading::File(path.to_path_buf()), Box::new(self.command.clone()));
        reads.start(self.inner, Reading::File(path.to_path_buf()), path.to_path_buf());
        None
    }
}

/// What a read finished for a person's command does: it runs again, with what was read, unless it's been left behind.
pub fn read_in(inner: &Inner, command: Command, fetched: Option<(PathBuf, Fetched)>) {
    inner.core.push(crate::queue::CoreCall::Run(Box::new(PersonCommand {
        command,
        origin: Origin::Finishing,
        fetched,
    })));
}
