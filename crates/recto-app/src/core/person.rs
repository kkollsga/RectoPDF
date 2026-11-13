//! **The person's commands, on the session thread** (step R2.3): what the window asks for, run as the command an agent would send, by the one executor, as
//! the person's ([`Actor::Person`]). What one needs from a file is read off this thread, as the window's taps and opens always have been, and the command runs
//! again once it's in ([`recto_agent::later`]): a file on a dead volume holds up neither this thread nor a later tap.
//!
//! What the window does around a person's command, beyond the command, is each feature's, one line each in the lists below: before it runs ([`BEFORE`]), and
//! once it has ([`ANSWERED`]), as the words a refusal is said in, a password asked for or a list kept at once.

use super::{FileReads, Inner, lock};
use crate::queue::{CoreCall, Origin, PersonCommand, Reading};
use recto_agent::Runs;
use recto_agent::later::{Fetched, ReadLater};
use recto_agent::{Actor, AgentError, Command, Event};
use serde_json::Value;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// **A person's command, from the window:** on the session thread, as a tap always was, or on a thread of its own if it may hold its thread for long
/// ([`recto_agent::commands::Cmd::WAITS`]), or isn't the session's to answer. Past a hand-off's last look nothing a person does is taken, as before. Never waits.
pub fn submit(inner: &Arc<Inner>, command: Command) {
    if inner.core.frozen.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if command.waits() || command.route() != Runs::Session {
        on_its_own_thread(inner, command);
    } else {
        inner.core.push(CoreCall::Run(PersonCommand::input(command)));
    }
}

/// A person's command that may hold its thread for long, as the window's Save always ran: through the host, counted for a hand-off, and what it did told.
fn on_its_own_thread(inner: &Arc<Inner>, command: Command) {
    let running = &inner.life.persons_running;
    running.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let inner_for_thread = Arc::clone(inner);
    let spawned = std::thread::Builder::new().name("recto-person".into()).stack_size(1024 * 1024).spawn(move || {
        let inner = inner_for_thread;
        let mut events = Vec::new();
        let result = recto_control::host::Host::run(&*inner.host, Actor::Person, &command, &mut events);
        answered(&inner, &command, Origin::Input, &result);
        inner.core.push(CoreCall::Events(events));
        inner.life.persons_running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    });
    if spawned.is_err() {
        running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A person's command, as it ran: what it was, where it came from, and what it came to.
pub struct Ran<'a> {
    pub command: &'a Command,
    pub origin: Origin,
    pub result: &'a Result<Value, AgentError>,
}

impl Ran<'_> {
    /// What it was refused for, unless it's only waiting for a read, which is nothing yet.
    pub fn refused(&self) -> Option<&AgentError> {
        self.result.as_ref().err().filter(|error| !recto_agent::later::is_later(error))
    }
}

/// **The window's own steps before a person's command runs,** feature by feature, one line each, for what the person just did (not a command running again
/// once its read is in): a tap's wake timed from now, and the tab last asked for.
const BEFORE: &[fn(&Inner, &mut FileReads, &Command)] = &[super::opening::before];

/// **What the window does with how a person's command went,** feature by feature, one line each: a failure in the person's own words, from the feature
/// it's about, and the window's own steps after (a list kept at once). A failure no feature has words for is said nothing of, as the window's own calls said
/// nothing (an Undo with nothing to undo); an agent's words, which name `recto` commands and levels, are never shown to a person.
const ANSWERED: &[fn(&Inner, &Ran<'_>)] = &[
    super::navigation::answered,
    super::opening::answered,
    super::closing::answered,
    super::editing::answered,
    super::comparison::answered,
    super::find::answered,
];

fn answered(inner: &Inner, command: &Command, origin: Origin, result: &Result<Value, AgentError>) {
    let ran = Ran { command, origin, result };
    for answer in ANSWERED {
        answer(inner, &ran);
    }
}

/// **A refusal the session made, in the words it has for the person:** a command that failed on what the session or a file's opening said keeps its message
/// (`From<SessionError> for AgentError`), and those messages are the person's. Any other code is an agent's, and has none here.
pub fn session_words(error: &AgentError) -> Option<&'static str> {
    use recto_core::OpenError;
    use recto_session::{SessionError, edit::EditError};
    let every = [
        SessionError::NoSuchTab,
        SessionError::TooManyTabs,
        SessionError::UnsavedChanges,
        SessionError::RendererOnly,
        SessionError::EditsUnreadable,
        SessionError::Saving,
        SessionError::Edit(EditError::OutOfRange),
        SessionError::Edit(EditError::LastPage),
        SessionError::Edit(EditError::TooManyPages),
        SessionError::Edit(EditError::TooManySources),
        SessionError::Edit(EditError::SourceChanged),
        SessionError::Edit(EditError::HistoryFull),
        SessionError::Open(OpenError::NotFound),
        SessionError::Open(OpenError::NotAllowed),
        SessionError::Open(OpenError::PasswordNeeded),
        SessionError::Open(OpenError::NotPdf),
        SessionError::Open(OpenError::TooComplex),
    ];
    every.into_iter().find(|known| known.code() == error.code).map(SessionError::message)
}

/// A line by the page in the person's words, which goes by itself.
pub fn say(inner: &Inner, words: String) {
    *lock(&inner.window.action_notice) = Some(words);
    inner.core.push(CoreCall::Reconcile);
}

/// Runs a person's command now. Its answer is nobody's but a test's: the window draws what the session publishes.
pub fn run(inner: &Arc<Inner>, reads: &mut FileReads, run: PersonCommand, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    let PersonCommand {
        command,
        fetched,
        origin,
        when_run,
    } = run;
    let command = match when_run {
        Some(resolve) => inner.host.session.with_session(|session| resolve(session, command)),
        None => command,
    };
    if origin == Origin::Input {
        for before in BEFORE {
            before(inner, reads, &command);
        }
    }
    let later = Later {
        inner,
        reads: RefCell::new(reads),
        command: &command,
        fetched: RefCell::new(fetched),
    };
    let result = inner
        .host
        .session
        .run_reading_later(Actor::Person, &command, events, &later, &super::find::effects_now(inner));
    answered(inner, &command, origin, &result);
    result
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
        reads.forget_unless_running(&Reading::Tab(tab));
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
        reads.forget_unless_running(&Reading::File(path.to_path_buf()));
        None
    }
}

/// What a read finished for a person's command comes to: the command again, with what was read, to run in the same call that took the read in, as the
/// window's own tap and open did then. So a tap the person makes after the read lands, but before anything else, still comes after it and wins.
pub fn read_in(command: Command, fetched: Option<(PathBuf, Fetched)>) -> PersonCommand {
    PersonCommand::finishing(command, fetched)
}

/// **A person's command that's what it is by the session as it is when it runs** ([`crate::queue::WhenRun`]): a control that goes by how things are then,
/// as a toggle does (accident A3), where the session holds what it goes by. Never waits.
pub fn submit_when_run(inner: &Arc<Inner>, command: Command, when_run: crate::queue::WhenRun) {
    if inner.core.frozen.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let mut run = PersonCommand::input(command);
    run.when_run = Some(when_run);
    inner.core.push(CoreCall::Run(run));
}

#[cfg(test)]
mod tests {
    use recto_agent::AgentError;
    use recto_session::SessionError;

    /// Failure: a person's refused edit is said in an agent's words (naming `recto` commands or a level), or a refusal the session made in the person's
    /// words is said as nothing. The session's own refusals keep their words, and an agent's refusal has none for the person.
    #[test]
    fn only_the_sessions_refusals_have_the_persons_words() {
        for error in [SessionError::Saving, SessionError::RendererOnly, SessionError::EditsUnreadable] {
            assert_eq!(super::session_words(&AgentError::from(error)), Some(error.message()), "{error:?}");
        }
        let agents = AgentError::new("would_clear_redo", "…needs Full access.", recto_agent::exit::PERMISSION);
        assert_eq!(super::session_words(&agents), None);
    }
}
