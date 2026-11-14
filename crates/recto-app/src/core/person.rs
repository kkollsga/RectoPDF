//! **The person's commands, on the session thread** (step R2.3): what the window asks for, run as the command an agent would send, by the one executor, as
//! the person's ([`Actor::Person`]). What one needs from a file is read off this thread, as the window's taps and opens always have been, and the command runs
//! again once it's in ([`recto_agent::later`]): a file on a dead volume holds up neither this thread nor a later tap.
//!
//! What the window does around a person's command, beyond the command, is each feature's, one line each in the lists below: before it runs ([`BEFORE`]), and
//! once it has ([`ANSWERED`]), as the words a refusal is said in, a password asked for or a list kept at once.

use super::answer_parts::AnswerParts;
use super::{FileReads, Inner};
use crate::queue::{CoreCall, Origin, PersonCommand, Reading};
use recto_agent::Code;
use recto_agent::Runs;
use recto_agent::later::{Fetched, ReadLater};
use recto_agent::{Actor, AgentError, Command, Event};
use serde_json::Value;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// **A person's command, from the window:** on the session thread, as a tap always was, or on a thread of its own if it may hold its thread for long
/// ([`recto_agent::commands::Cmd::WAITS`]), or isn't the session's to answer. Past a hand-off's last look nothing a person does is taken, as before, and
/// this says false, for a control that has kept something for it (a print). Never waits.
pub fn submit(inner: &Arc<Inner>, command: Command) -> bool {
    if inner.core.frozen.load(std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    let lane = Person::of(inner).lane();
    let turn = lane.turn_for(&command);
    if command.waits() || command.route() != Runs::Session {
        match turn {
            Some(turn) => lane.submit_waiting(command, turn),
            None => lane.start_waiting(command, None),
        }
    } else {
        let mut run = PersonCommand::input(command);
        run.turn = turn;
        inner.core.push(CoreCall::Run(run));
    }
    true
}

/// **What a person may start from a menu row or an answer, as the person:** a command, and the feature's own runs that take a thread of their own (a
/// Save with the window's progress, a refused snapshot taken anyway). Each starts on its own thread, which holds the app to run it and to answer
/// how it went, so this holds the app; its field is private to this file, so a row or an answer can do these and nothing more of the app through it.
#[derive(Clone, Copy)]
pub struct Person<'a> {
    app: &'a Arc<Inner>,
}

impl<'a> Person<'a> {
    /// The person's way into `app`.
    pub(in crate::core) fn of(app: &'a Arc<Inner>) -> Self {
        Self { app }
    }

    /// The person's lane: their ordered commands, each in its turn ([`super::person_lane`]).
    pub(in crate::core) fn lane(self) -> super::person_lane::PersonsLane<'a> {
        super::person_lane::PersonsLane { app: self.app }
    }

    /// A command as the person's ([`submit`]), saying whether it was taken: false past a hand-off's last look.
    pub fn submit(self, command: Command) -> bool {
        submit(self.app, command)
    }

    /// A command as the person's, made what it is by the session as it is when it runs ([`submit_when_run`]).
    pub fn submit_when_run(self, command: Command, when_run: crate::queue::WhenRun) {
        submit_when_run(self.app, command, when_run);
    }

    /// Saving's own: the person's Save of `tab`, on a thread of its own with the window's progress ([`super::saving::start_save`]).
    pub fn save(self, tab: recto_session::TabId) {
        super::saving::start_save(self.app, tab, None, false);
    }

    /// The snapshot tool's own: a notice's Copy anyway, for the person's snapshot it refused ([`super::snapshot_tool::anyway`]).
    pub fn snapshot_anyway(self, serial: u64) {
        super::snapshot_tool::anyway(self.app, serial);
    }
}

/// Whether a person's command, as it's queued, is one that runs on a thread of its own: one that may wait, or isn't the session's to answer. Only a command
/// held while a late keep's restore was read is queued so ([`submit`]), and it goes to its thread once it's let go.
pub(crate) fn on_a_thread_of_its_own(run: &PersonCommand) -> bool {
    run.origin == Origin::Input && run.fetched.is_none() && (run.command.waits() || run.command.route() != Runs::Session)
}

/// What the person is told when their command's thread couldn't be started, before its feature's own words, if it has any.
pub const NOT_STARTED: &str = "That couldn't be started. Try again in a moment.";

/// A test's: the next person's command that runs on a thread of its own finds it can't start one.
#[cfg(test)]
static NO_THREAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A person's command that may hold its thread for long, as the window's Save always ran: through the host, counted for a hand-off, and what it did told.
/// A thread that can't be started is said, as a refusal of the command: the person is never left with a click that did nothing.
pub(super) fn on_its_own_thread(command: Command, counted: Running, turn: Option<crate::queue::lane::Turn>) {
    let inner = Arc::clone(&counted.inner);
    let kept = command.clone();
    let spawned = spawn(move || {
        let inner = Arc::clone(&counted.inner);
        let here = crate::queue::lane::hold_here(turn);
        let mut events = Vec::new();
        sent(&inner.serving, &command);
        let result = recto_control::host::Host::run(&*inner.host, Actor::Person, &command, &mut events);
        answered(&AnswerParts::of(&inner), &command, Origin::Input, &result);
        inner.core.push(CoreCall::Events(events));
        // Its turn passes once what it did is told, so watchers hear of it before the person's next edit; and before its count goes, so the app's
        // never seen at rest between the two.
        drop(here);
        drop(counted);
    });
    // The thread that didn't start took its count with it, as the closure holding it was dropped. It's said in the person's words, then answered as
    // refused, so its feature can say more of its own.
    if spawned.is_err() {
        say(inner.window.notices(), &inner.core, NOT_STARTED.to_string());
        let refused = Err(AgentError::new(Code::NOT_STARTED, NOT_STARTED, recto_agent::exit::BUSY));
        answered(&AnswerParts::of(&inner), &kept, Origin::Input, &refused);
    }
}

/// A test's hold on a person's command on a thread of its own: the next with this tag says it's there before it runs, then waits to be let go ([`hold`]).
#[cfg(test)]
static HOLD: std::sync::Mutex<Option<(&'static str, std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>> = std::sync::Mutex::new(None);

/// **A test holds the next command tagged `tag` the app's host runs,** in flight and counted running, before the executor has it, as a highlight is while
/// its pages are read; or the next save as it writes (`save_writing`): what tells it's held, and what lets it go (dropped, it goes too).
#[cfg(test)]
pub(crate) fn hold(tag: &'static str) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (told, held) = std::sync::mpsc::channel();
    let (go, waiting) = std::sync::mpsc::channel();
    *crate::lock(&HOLD) = Some((tag, told, waiting));
    (held, go)
}

/// Where a test's hold named `at` waits, if it's held there ([`hold`]).
#[cfg(test)]
pub(crate) fn held_at(at: &str) {
    let hold = crate::lock(&HOLD).take_if(|(tag, ..)| *tag == at);
    if let Some((_, told, go)) = hold {
        let _ = told.send(());
        let _ = go.recv_timeout(crate::testkit::headless::WAIT_LIMIT);
    }
}

/// A person's command's own thread, started: or, for a test that said so ([`NO_THREAD`]), not, as when the system has none to give.
fn spawn(run: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    #[cfg(test)]
    if NO_THREAD.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return Err(std::io::Error::other("no thread, for a test"));
    }
    std::thread::Builder::new()
        .name("recto-person".into())
        .stack_size(1024 * 1024)
        .spawn(run)
        .map(drop)
}

/// Whether a refusal is a person's command whose thread couldn't be started ([`NOT_STARTED`]), for a feature with words of its own for it.
#[must_use]
pub fn not_started(error: &AgentError) -> bool {
    error.code == Code::NOT_STARTED
}

/// **A person's command counted as running for a hand-off** until it's dropped: as its thread ends, however it ends. A command that panicked was counted
/// running for good, so a hand-off waited on it to the end of its patience and the tests' settle never settled (review, 2026-11-13).
/// The life's count goes first as it's dropped (fields drop in the order they're declared), then the app, as when this let go of the count itself.
pub(super) struct Running {
    _counted: super::life::PersonRunning,
    inner: Arc<Inner>,
}

impl Running {
    pub(super) fn start(inner: Arc<Inner>) -> Self {
        Self {
            _counted: inner.life.person_running(),
            inner,
        }
    }
}

/// **A command run for the person, kept for a test that asked** ([`crate::testkit::headless::HeadlessApp::keep_sent`]): every way one reaches the executor
/// as theirs says so here. Nothing outside a test.
#[allow(unused_variables)]
pub fn sent(serving: &super::Serving, command: &Command) {
    #[cfg(any(test, feature = "testkit"))]
    if let Some(sent) = crate::lock(&serving.heard).sent.as_mut() {
        sent.push(command.clone());
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
const ANSWERED: &[fn(&AnswerParts<'_>, &Ran<'_>)] = &[
    super::navigation::answered,
    super::opening::answered,
    super::closing::answered,
    super::editing::answered,
    super::comparison::answered,
    super::find::answered,
    super::highlight::answered,
    super::updates::answered,
    super::printing::answered,
];

pub(super) fn answered(parts: &AnswerParts<'_>, command: &Command, origin: Origin, result: &Result<Value, AgentError>) {
    let ran = Ran { command, origin, result };
    for answer in ANSWERED {
        answer(parts, &ran);
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
        SessionError::OutsideQuietRun,
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
    every
        .into_iter()
        .find(|known| Code::lower(known.code()) == error.code)
        .map(SessionError::message)
}

/// A line by the page in the person's words, which goes by itself.
pub fn say(notices: &super::window::Notices, core: &crate::queue::CoreQueue, words: String) {
    notices.say(words);
    core.push(CoreCall::Reconcile);
}

/// Runs a person's command now. Its answer is nobody's but a test's: the window draws what the session publishes.
pub fn run(inner: &Arc<Inner>, reads: &mut FileReads, run: PersonCommand, events: &mut Vec<Event>) -> Result<Value, AgentError> {
    // One of the person's ordered edits whose turn hasn't come is parked, to run when it does.
    let Some(run) = Person::of(inner).lane().taken(run) else {
        return Ok(Value::Null);
    };
    let PersonCommand {
        command,
        fetched,
        origin,
        when_run,
        turn: _turn,
    } = run;
    let command = match when_run {
        Some(resolve) => match inner.host.session.with_session(|session| resolve(session, Actor::Person, command.clone())) {
            Ok(resolved) => resolved,
            Err(error) => {
                let result = Err(error);
                answered(&AnswerParts::of(inner), &command, origin, &result);
                return result;
            }
        },
        None => command,
    };
    if origin == Origin::Input {
        sent(&inner.serving, &command);
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
    answered(&AnswerParts::of(inner), &command, origin, &result);
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
    let turn = Person::of(inner).lane().turn_for(&command);
    let mut run = PersonCommand::input(command);
    run.when_run = Some(when_run);
    run.turn = turn;
    inner.core.push(CoreCall::Run(run));
}

#[cfg(test)]
mod tests {
    use recto_agent::AgentError;

    /// Failure (review of 2026-11-13): a person's command that panics on its thread is counted as running for good, so a hand-off waits on it to the end of
    /// its patience. The count goes as the thread ends, a panic included.
    #[test]
    fn a_persons_command_that_panics_isnt_counted_running() {
        let app = crate::testkit::headless::HeadlessApp::start();
        let inner = std::sync::Arc::clone(app.core());
        let counted = super::Running::start(std::sync::Arc::clone(&inner));
        let thread = std::thread::spawn(move || {
            let _counted = counted;
            panic!("a command that panics");
        });
        assert!(thread.join().is_err(), "it panicked");
        assert_eq!(inner.life.persons_running(), 0);
    }
    use recto_session::SessionError;

    /// Failure (S2, the coordinator's word on 2026-11-14: "a person's action that fails says so"): a person's command that may wait, as a print, whose
    /// thread can't be started is dropped with nothing said, so the person clicked and nothing happened. Its thread made to fail: the person is told, in
    /// words Rust owns, and nothing is left counted as running.
    #[test]
    fn a_persons_command_whose_thread_cant_start_says_so() {
        let app = crate::testkit::headless::HeadlessApp::start();
        let doc = app.disk().pdf("a.pdf", 1);
        app.open(&doc);
        app.settle();
        let out = app.disk().files().join("out.pdf");
        let line = format!(r#"{{"cmd":"print","to_pdf":{}}}"#, serde_json::Value::from(out.display().to_string()));
        let command = recto_agent::parse(&line).expect("a print");
        assert!(command.waits(), "a command that runs on a thread of its own");
        super::NO_THREAD.store(true, std::sync::atomic::Ordering::SeqCst);
        super::submit(app.core(), command);
        app.settle();
        let notice = app.reader().selection_notice.map(|notice| notice.text);
        // The print has words of its own for it, said over [`super::NOT_STARTED`], which a feature with none leaves.
        assert_eq!(notice.as_deref(), Some("The print couldn't be started."), "the person is told");
        assert_eq!(app.core().life.persons_running(), 0, "nothing is left counted as running");
        assert!(!out.exists(), "nothing ran");
        // A waiting command no feature has words for is said in the window's own.
        app.core().dismiss_selection_notice();
        let line = format!(
            r#"{{"cmd":"render","page":1,"out":{}}}"#,
            serde_json::Value::from(app.disk().files().join("out.png").display().to_string())
        );
        super::NO_THREAD.store(true, std::sync::atomic::Ordering::SeqCst);
        super::submit(app.core(), recto_agent::parse(&line).expect("a render"));
        app.settle();
        let notice = app.reader().selection_notice.map(|notice| notice.text);
        assert_eq!(notice.as_deref(), Some(super::NOT_STARTED), "told in the window's words");
    }

    /// **Failure (a guard before the answers were given the app's parts and not the app, `build/registries`):** what a person is shown after a command of
    /// theirs ran changes: a refusal said in other words or by another line (the page's notice, or the open failure under the field), one said where it
    /// was said nothing (an agent's refusal, which names `recto` and levels, or a close refused that wasn't the prompt's), a rotate that went leaving the
    /// line there, or a line said without the window being published again to show it, or published for nothing (`say` and `notice` swapped). Through the
    /// one list, as the session thread calls it, for the answers of going to a page, opening (its failure; not its password sheet), closing, editing,
    /// comparison and highlights. Held elsewhere: Find's (`headless/finding.rs`, `a_query_too_long_is_said_and_not_searched`), the updates menu's
    /// (`headless/update_menu.rs`), the password sheet an open asks for (`headless/passwords.rs`); Clear Menu's record written at once is held by none.
    #[test]
    fn what_a_person_is_shown_after_each_command_is_as_it_was() {
        use super::Origin;
        use recto_agent::commands::compare::Compare;
        use recto_agent::commands::editing::PageRotate;
        use recto_agent::commands::highlight::{Highlight, HighlightRemove, RemoveWhat};
        use recto_agent::commands::linking::Goto;
        use recto_agent::commands::tabs::{Close, Open};
        use recto_agent::{Code, Command, TabRef};
        use recto_core::OpenError;
        use recto_session::TabId;
        use recto_session::compare::CompareError;

        let app = crate::testkit::headless::HeadlessApp::start();
        let inner = std::sync::Arc::clone(app.core());
        let agents = |code: Code| {
            Err(AgentError::new(
                code,
                "an agent's words, naming `recto` and Full access",
                recto_agent::exit::PERMISSION,
            ))
        };
        assert!(inner.core.flush(std::time::Duration::from_secs(10)), "the app at rest");
        let shown = |command: Command, origin: Origin, result: Result<serde_json::Value, AgentError>| {
            let notices = inner.window.notices();
            notices.say("before".into());
            notices.clear_open_failure();
            let asked = inner.core.reconciles_asked();
            super::answered(&super::AnswerParts::of(&inner), &command, origin, &result);
            (notices.notice(), notices.open_failure(), inner.core.reconciles_asked() > asked)
        };
        // Said by the page, which goes by itself: the window published again to show it.
        let said = |words: &str| (Some(words.to_string()), None, true);
        let left = (Some("before".to_string()), None, false);
        let goto = Command::from(Goto {
            tab: TabRef::Id(TabId(1)),
            page: None,
            label: Some("xii".into()),
            within: None,
            entry: None,
        });
        let open = Command::from(Open {
            path: "/nowhere/a.pdf".into(),
            page: None,
        });
        let close = Command::from(Close { tab: TabRef::Id(TabId(1)) });
        let rotate = Command::from(PageRotate {
            tab: "active".into(),
            pages: "all".into(),
            degrees: 90,
        });
        let compare = Command::from(Compare::Toggle(TabRef::Id(TabId(1))));
        let highlight = Command::Edit(recto_agent::commands::Edit::Highlight(Highlight {
            tab: TabRef::Id(TabId(1)),
            page: Some(1),
            to_page: None,
            chars: Some("0-4".into()),
            at: None,
            to: None,
            unit: None,
        }));
        let remove = Command::Edit(recto_agent::commands::Edit::HighlightRemove(HighlightRemove {
            tab: TabRef::Id(TabId(1)),
            what: RemoveWhat::Pick {
                page: Some(1),
                to_page: None,
                chars: Some("0-4".into()),
                at: None,
                to: None,
            },
        }));
        let cases = [
            (
                "a typed page that's none",
                shown(goto.clone(), Origin::Input, agents(Code::NO_SUCH_PAGE)),
                said(&crate::linking::no_such_page("xii")),
            ),
            ("a goto that went", shown(goto, Origin::Input, Ok(serde_json::Value::Null)), left.clone()),
            (
                "a file that isn't a PDF",
                shown(open.clone(), Origin::Input, Err(SessionError::Open(OpenError::NotPdf).into())),
                (Some("before".into()), Some(SessionError::Open(OpenError::NotPdf).message().to_string()), false),
            ),
            (
                "an open refused in an agent's words",
                shown(open, Origin::Input, agents(Code::PERMISSION_DENIED)),
                left.clone(),
            ),
            (
                "the prompt's close, after its save, with an edit since",
                shown(close.clone(), Origin::Finishing, agents(Code::UNSAVED_CHANGES)),
                said(crate::saving::SAVED_NOT_CLOSED),
            ),
            ("File > Close refused", shown(close, Origin::Input, agents(Code::UNSAVED_CHANGES)), left.clone()),
            (
                "a rotate refused by the session",
                shown(rotate.clone(), Origin::Input, Err(SessionError::Saving.into())),
                said(&format!("Couldn't rotate: {}", SessionError::Saving.message())),
            ),
            (
                "a rotate refused in an agent's words",
                shown(rotate.clone(), Origin::Input, agents(Code::WOULD_CLEAR_REDO)),
                left.clone(),
            ),
            (
                "a rotate that went",
                shown(rotate, Origin::Input, Ok(serde_json::Value::Null)),
                (None, None, false),
            ),
            (
                "a fourth pane",
                shown(compare.clone(), Origin::Input, agents(Code::lower(CompareError::TooMany.code()))),
                // The comparison's line asks for no pass of its own, as before: it shows with the next.
                (Some(CompareError::TooMany.message().to_string()), None, false),
            ),
            (
                "a pane that couldn't be read",
                shown(compare, Origin::Input, agents(Code::PERMISSION_DENIED)),
                left.clone(),
            ),
            (
                "a highlight where there's no text",
                shown(highlight.clone(), Origin::Input, agents(Code::NO_TEXT_THERE)),
                said("Couldn't highlight: there's no text selected there."),
            ),
            (
                "a highlight refused by the session",
                shown(highlight.clone(), Origin::Input, Err(SessionError::Saving.into())),
                said(&format!("Couldn't highlight: {}", SessionError::Saving.message())),
            ),
            (
                "a highlight refused in an agent's words",
                shown(highlight, Origin::Input, agents(Code::WOULD_CLEAR_REDO)),
                left.clone(),
            ),
            (
                "a removal with nothing there",
                shown(remove.clone(), Origin::Input, agents(Code::NO_SUCH_HIGHLIGHT)),
                said("Couldn't remove the highlight: there's no highlight there."),
            ),
            (
                "a removal of an added file's",
                shown(remove.clone(), Origin::Input, agents(Code::HIGHLIGHT_IN_ADDED_FILE)),
                said("Couldn't remove the highlight: that highlight came with an added PDF, so save first, then it can be removed."),
            ),
            (
                "a removal as the pages moved",
                shown(remove, Origin::Input, agents(Code::CHANGED)),
                said("Couldn't remove the highlight: the pages moved meanwhile, so try again."),
            ),
        ];
        let wrong: Vec<String> = cases
            .into_iter()
            .filter(|(_, now, was)| now != was)
            .map(|(case, now, was)| format!("{case}: shown {now:?}, was {was:?}"))
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    /// Failure: a person's refused edit is said in an agent's words (naming `recto` commands or a level), or a refusal the session made in the person's
    /// words is said as nothing. The session's own refusals keep their words, and an agent's refusal has none for the person.
    #[test]
    fn only_the_sessions_refusals_have_the_persons_words() {
        for error in [SessionError::Saving, SessionError::RendererOnly, SessionError::EditsUnreadable] {
            assert_eq!(super::session_words(&AgentError::from(error)), Some(error.message()), "{error:?}");
        }
        let agents = AgentError::new(recto_agent::Code::WOULD_CLEAR_REDO, "…needs Full access.", recto_agent::exit::PERMISSION);
        assert_eq!(super::session_words(&agents), None);
    }
}
