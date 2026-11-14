//! **A test's hold on the next of the app's steps with a name** (review of the lane, D10): a command the app's host runs, by its tag (`highlight`, `print`),
//! held in flight and counted running before the executor has it, as a highlight is while its pages are read; or a save at a step of its own
//! (`save_starting`, `save_writing`), or a quit at its wait for what's running (`quit_waiting`) or its write (`quit_flush`). One per app, on its queue (not a process-wide one), asked for by [`super::headless::HeadlessApp::hold_next`]; in a
//! test build only, and nothing in the app.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

/// What's held: the name, what tells the test it's held, and what lets it go.
type Held = (&'static str, Sender<()>, Receiver<()>);

/// The holds a test has asked for and that haven't been met yet, one per name.
#[derive(Default)]
pub struct Holds {
    next: Mutex<Vec<Held>>,
}

impl Holds {
    /// The next step named `at` waits there: what tells it's held, and what lets it go (dropped, it goes too).
    pub(crate) fn arm(&self, at: &'static str) -> (Receiver<()>, Sender<()>) {
        let (told, held) = channel();
        let (go, waiting) = channel();
        let mut next = crate::lock(&self.next);
        next.retain(|(name, ..)| *name != at);
        next.push((at, told, waiting));
        (held, go)
    }

    /// A step named `at`: if it's the one asked for, it says so and waits to be let go, at most the kit's limit.
    pub(crate) fn at(&self, at: &str) {
        let held = {
            let mut next = crate::lock(&self.next);
            next.iter().position(|(name, ..)| *name == at).map(|found| next.remove(found))
        };
        if let Some((_, told, go)) = held {
            let _ = told.send(());
            let _ = go.recv_timeout(super::headless::WAIT_LIMIT);
        }
    }
}
