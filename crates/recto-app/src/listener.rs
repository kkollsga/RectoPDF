//! **What the app core tells the host.** The host implements [`Listener`] (`recto-ffi` over its UniFFI listener, a test over [`crate::testkit::FakeListener`]). A module that
//! makes calls in a loop takes it as a generic parameter; the app's own state holds one as `Arc<dyn Listener>`, which the bridge's adapter forwards one for one to the UniFFI
//! listener.
//!
//! Every call is here, each with what the code does today written down as its promise (step R1.3). Where today's behaviour looks like an accident rather than a choice, it says
//! **today:** and isn't promised.

/// What the host hears from the core. Every call returns at once: the host pulls what changed, or acts, on its own thread, when it's ready.
///
/// **What the core promises the host, for every call unless the call says otherwise:**
/// - **Thread:** any of the core's threads. The host never assumes which, and hops to its UI thread before it touches anything it draws.
/// - **Locks:** the core makes the call with none of its own locks held, so a host that pulls at once can't deadlock on the core.
/// - **Ordering:** a call comes after the change it announces can be seen by a pull. Nothing is promised about the order of one kind of call against another.
///
/// **What the host promises the core:** each call returns at once. It doesn't block, wait for its UI thread or call back into the core before returning; it may be called
/// many times a second, so it does no more than note what's owed and hop.
pub trait Listener: Send + Sync {
    /// The window's view models changed: the sidebar, the reader, Find, a tool. The host pulls them.
    ///
    /// From the session thread at the end of a pass that changed what's published, and from the UI thread inside a call the host made (opening or closing the palette, the
    /// snapshot tool's toggle), from a print's thread as it ends, and from the updater's thread. Coalesced: many calls promise only that a pull after the last sees everything.
    fn model_changed(&self);

    /// A page view has layer changes waiting: the host drains every pane's on its UI thread (`drain_ops`).
    ///
    /// From a page view's own thread, or from the UI thread inside `set_viewport`, `drain_ops` or a pointer call that left more waiting, with the page view's lock let go.
    /// One call per batch: a page view says it once and not again until it's drained. In a slow run (`recto_platform::slow`'s draw seam) it comes later, from a timer's
    /// thread.
    fn ops_ready(&self);

    /// A QA command, to run on the UI thread as the UI would, and answer with `qa_answer(id, ..)`. `id` zero wants no answer.
    ///
    /// From the thread running a socket command, which then waits for the answer up to a limit (`qa`'s `UI_ANSWER_WAIT`) and says the window didn't answer past it. A
    /// sweep's scroll steps come with id zero from the sweep's own thread, one at a time: the next isn't sent until the host has said the last was done. Only in a quiet run.
    fn qa_request(&self, id: u64, verb: String, value: String);

    /// The control socket's server has stopped: the host ends the app.
    ///
    /// At most once in the app's life, by construction (`lifecycle`'s `SocketEnded`), from the socket's own thread as its server returns: stopped over the socket
    /// by `recto stop`, or by anything else that ends it. That includes the app's own `shutdown` on the way out, so a host that's already quitting hears it too:
    /// it isn't a promise that someone asked over the socket, and ending an app that's already ending does nothing more.
    fn session_ended(&self);

    /// A copy is ready: the host takes it (`take_copied`) on its UI thread and puts it on the clipboard.
    ///
    /// From a page view's thread or the UI thread, after a selection's text is made, and from the snapshot tool's thread once its pictures are ready.
    fn copy_ready(&self);

    /// The saved state's been read and applied, or the wait for it is over: the host can make the window, in the right place.
    ///
    /// From the session thread. When the state comes in time, once, after what came of it is published. When the patience runs out first (`restore_patience_over`,
    /// with no update to wait for), once then, and again when the state does come: the host makes its window again from it. In a slow run the second comes later, from a
    /// timer's thread. The window's marked as up without the state before the first of the two.
    fn restore_ready(&self);

    /// What to do about a quit the host asked for (`quit_requested`), once everything's been written or the wait for it is over.
    ///
    /// Exactly one per `quit_requested`, from a thread of its own, after the state's flush or [`QUIT_PATIENCE_MS`] at most. With `go: false`, what a person does is
    /// taken again before the call; with `go: true` it stays held back, as the app is quitting.
    fn quit_answer(&self, answer: QuitAnswer);

    /// A hand-off has written and read the state back, so the host quits now, with no prompt of any kind.
    ///
    /// Only once the hand-off has got that far, from the thread that asked first (the updater's, or a QA command's), and at most once a hand-off, by construction
    /// (`lifecycle`'s `HandOffQuit`): asking again, as a QA run's `hand_off_quit` does after the answer's own thread, tells the host nothing more.
    fn hand_off_quit(&self);
}

/// How long a quit waits for the state's flush before it answers anyway, in milliseconds.
pub const QUIT_PATIENCE_MS: u64 = 5000;

/// What comes of asking to quit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuitAnswer {
    /// Go ahead. Otherwise the words below are for a question: quit anyway, or stay.
    pub go: bool,
    pub title: String,
    pub message: String,
    pub quit_button: String,
    pub stay_button: String,
}

impl QuitAnswer {
    /// Everything that needs keeping is kept, or there was nothing to keep: quit.
    #[must_use]
    pub fn go() -> Self {
        Self {
            go: true,
            title: String::new(),
            message: String::new(),
            quit_button: String::new(),
            stay_button: String::new(),
        }
    }

    /// The state couldn't be written and there are unsaved edits: ask, as quitting now loses them.
    #[must_use]
    pub fn edits_not_kept() -> Self {
        Self {
            go: false,
            title: "Your unsaved edits couldn't be kept".into(),
            message: "RectoPDF couldn't write them to the state folder. If you quit now, they'll be lost.".into(),
            quit_button: "Quit Anyway".into(),
            stay_button: "Stay".into(),
        }
    }

    /// What a quit says: go, unless the state wasn't written and there are edits it would lose.
    #[must_use]
    pub fn for_quit(written: bool, unsaved_tabs: usize) -> Self {
        if written || unsaved_tabs == 0 { Self::go() } else { Self::edits_not_kept() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: a quit whose state couldn't be written asks nothing and loses the edits, or one with nothing to lose asks anyway.
    #[test]
    fn a_quit_asks_only_when_it_would_lose_edits() {
        assert!(QuitAnswer::for_quit(true, 3).go, "written: go");
        assert!(QuitAnswer::for_quit(false, 0).go, "nothing to lose: go");
        let asked = QuitAnswer::for_quit(false, 1);
        assert!(!asked.go && !asked.quit_button.is_empty() && !asked.stay_button.is_empty(), "{asked:?}");
    }
}
