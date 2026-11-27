//! **Where the session's events leave for agents** (independent review of phase R, S1). Every event a watcher is told of or a wait looks again for comes
//! to the loop as a [`Cleared`] batch, and the only ways to make one go through the host's [`Host::for_watchers`], which applies
//! [`recto_agent::withheld::for_agents`] (nothing of a document the person opened with its password), or are the server's own news about no document. The
//! three ways in are all here or made here: a command's events on its job thread ([`Cleared::by`] in `run_job`), the host's news outside any command
//! (the notifier [`Server::run`](super::Server::run) attaches, an [`Announcer`]), and the app's own announcements (an [`Announcer`] too).
//!
//! **Costs nothing while no agent listens:** an announcement with no watch and no wait open takes no lock, sends nothing and wakes nothing, so a person
//! reading with no agent pays one atomic load per page drawn. With one listening, a batch about a document takes the session's lock once, on the thread
//! that announced it (never the loop's), and asks each of its events' tab whether it's withheld (a search of the open tabs, as every look up of a tab by its
//! number is); a batch about no document takes no lock. Nothing is allocated: the batch is filtered in place.

use super::{Done, Loop, State, WAKE_BYTE, WaitFor};
use crate::host::Host;
use recto_agent::Event;
use recto_agent::withheld::{About, about};
use recto_platform::local::Stream;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock, Weak};

/// **A batch of events let through to agents,** and the only thing the loop publishes.
#[derive(Debug, Default)]
pub(super) struct Cleared(Vec<Event>);

impl Cleared {
    /// What of `events` the host lets agents be told.
    pub(super) fn by<H: Host + ?Sized>(host: &H, mut events: Vec<Event>) -> Self {
        if !events.is_empty() {
            host.for_watchers(&mut events);
        }
        Self(events)
    }

    /// The server's own news, which is about no document: anything that is, is dropped.
    pub(super) fn server_own(event: Event) -> Self {
        let mut events = vec![event];
        events.retain(|event| about(event) == About::Nothing);
        Self(events)
    }

    pub(super) fn events(&self) -> &[Event] {
        &self.0
    }
}

/// **The one way into the loop for what happened outside a job:** what a server and every [`Announcer`] of it share, one of each a server: the channel
/// and the wake the loop reads, the host to ask once the server runs, and whether any agent is listening. An announcer is a reference to it, so one
/// costs no descriptor of its own.
#[derive(Debug)]
pub(super) struct Outlet {
    pub(super) done: Sender<Done>,
    pub(super) wake: Stream,
    /// The socket's name, for the host's stop to take before it returns ([`super::leaving`]).
    pub(super) socket: std::path::PathBuf,
    host: OnceLock<Weak<dyn Host>>,
    listening: AtomicBool,
}

impl Outlet {
    pub(super) fn new(done: Sender<Done>, wake: Stream, socket: std::path::PathBuf) -> Self {
        Self {
            done,
            wake,
            socket,
            host: OnceLock::new(),
            listening: AtomicBool::new(false),
        }
    }

    /// The host the server runs on: asked about every announcement from now on.
    pub(super) fn open<H: Host>(&self, host: &Arc<H>) {
        let weak: Weak<dyn Host> = Arc::downgrade(host) as Weak<dyn Host>;
        let _ = self.host.set(weak);
    }

    /// Whether a watch or a wait is open: set by the loop before either begins, and lowered when none is left.
    pub(super) fn set_listening(&self, on: bool) {
        self.listening.store(on, Ordering::SeqCst);
    }
}

/// Tells a running server's watchers what happened outside any command. See [`super::Server::announcer`].
#[derive(Clone, Debug)]
pub struct Announcer(pub(super) Arc<Outlet>);

impl Announcer {
    /// **Tells watchers and waits what of `events` agents may be told.** Never blocks on the server: the channel has no limit and only holds what's
    /// published between two turns of the loop, and a full wake pipe already means the loop will look. Before the server runs, or with nobody listening,
    /// there's nobody to tell, and nothing is done.
    pub fn announce(&self, events: Vec<Event>) {
        let outlet = &*self.0;
        if events.is_empty() || !outlet.listening.load(Ordering::SeqCst) {
            return;
        }
        let Some(host) = outlet.host.get().and_then(Weak::upgrade) else { return };
        let cleared = Cleared::by(&*host, events);
        if !cleared.0.is_empty() && outlet.done.send(Done::Published(cleared)).is_ok() {
            let _ = (&outlet.wake).write(&[WAKE_BYTE]);
        }
    }
}

impl<H: Host> Loop<H> {
    /// Whether any agent is listening for events: a watch, or a wait for a condition, which looks again when one comes.
    pub(super) fn listens(&self) -> bool {
        self.conns.iter().any(|conn| {
            matches!(
                conn.state,
                State::Watching { .. }
                    | State::Waiting {
                        what: WaitFor::Condition { .. },
                        ..
                    }
            )
        })
    }
}
