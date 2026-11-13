//! **A quit the host asked for, and the way out:** everything written on a thread of its own, the answer, and the socket ended. What the quit comes to at each
//! step is the app's life's ([`crate::lifecycle::Life::step`]: `QuitAsked`, `QuitWritten`, `QuitStayed`); what's here carries it out.

use super::*;
use crate::lifecycle::{Event, Out};

impl Inner {
    /// A quit was asked for, by the menu or the system. Everything's written, on a thread of its own, and `quit_answer` says what to do. Mac apps with Resume don't ask on the way out, and neither does this, as unsaved edits come back: closing a tab with them is where Don't Save, Cancel or Save belongs. It asks only if they couldn't be kept.
    pub fn quit_requested(self: &Arc<Self>) {
        let out = self.life.step(Event::QuitAsked);
        self.carry_out_quit(out);
    }

    /// The quit's question was answered with Stay: the app goes on, and isn't quitting after all.
    pub fn quit_stayed(&self) {
        let out = self.life.step(Event::QuitStayed);
        self.carry_out(out);
    }

    /// Ends the socket, on the way out. Returns at once.
    pub fn shutdown(&self) {
        if let Some(stopper) = lock(&self.serving.stopper).take() {
            stopper.stop();
        }
        self.forget_a_quiet_copy();
    }

    /// **What a quit's step came to, carried out,** with the core's own handle, as a quit's write goes on a thread that holds it. Anything else goes to
    /// [`Inner::carry_out`].
    pub(crate) fn carry_out_quit(self: &Arc<Self>, out: Option<Out>) {
        match out {
            Some(Out::QuitBegins) => {
                // What's done from here wouldn't come back. If it turns out it isn't quitting, it's taken again.
                self.set_frozen(true);
                self.updates.stop();
                self.tools.tool.quitting();
                self.tools.printing.quitting();
                self.write_for_quit();
            }
            other => self.carry_out(other),
        }
    }

    /// The quit's answer and the stay, carried out: see [`Out::QuitAnswer`] and [`Out::QuitStayed`].
    pub(crate) fn carry_out_quit_answer(&self, out: Out) {
        match out {
            Out::QuitAnswer { go, unsaved_tabs, unfreeze } => {
                // Asked for, Quit Anyway included, so this run of a version being watched didn't end early. Stay says otherwise.
                self.updates.quitting_as_asked();
                if unfreeze {
                    self.set_frozen(false);
                }
                if !go {
                    self.updates.stop_over();
                }
                self.listener.quit_answer(crate::listener::QuitAnswer::for_quit(go, unsaved_tabs));
            }
            Out::QuitStayed => self.updates.quit_not_taken(),
            _ => {}
        }
    }

    /// Everything written, on the quit's own thread, with a flush on another so a disk that doesn't answer can't hold the answer past [`QUIT_PATIENCE`]; then
    /// what that came to, stepped.
    fn write_for_quit(self: &Arc<Self>) {
        let inner = Arc::clone(self);
        let refused = cfg!(any(test, feature = "testkit")) && QUIT_THREAD_REFUSED.load(std::sync::atomic::Ordering::SeqCst);
        let builder = std::thread::Builder::new().name("recto-quit".into()).stack_size(256 * 1024);
        let spawned = if refused {
            Err(std::io::Error::other("refused for a test"))
        } else {
            Ok(builder)
        }
        .and_then(|builder| {
            builder.spawn(move || {
                let (sender, receiver) = std::sync::mpsc::channel();
                let flushing = Arc::clone(&inner);
                let _ = std::thread::Builder::new().name("recto-flush".into()).stack_size(512 * 1024).spawn(move || {
                    let _ = sender.send(flushing.saver.flush(false, Duration::from_secs(2)));
                });
                let outcome = receiver.recv_timeout(QUIT_PATIENCE);
                let written = matches!(outcome, Ok(Ok(_))) || !inner.saver.store.holds_the_record();
                let unsaved_tabs = inner.host.session.unsaved_tabs().len();
                let out = inner.life.step(Event::QuitWritten { written, unsaved_tabs });
                inner.carry_out(out);
            })
        });
        // No thread to write on: answered as a write that didn't finish, here and now, so the quit's question still comes and nothing waits for a thread
        // that isn't there (review, 2026-11-13: the app stayed frozen). What's unsaved isn't asked of the session here, as this may be the UI
        // thread, which never waits on it; so it asks, as losing edits is worse than one question too many.
        if spawned.is_err() {
            let out = self.life.step(Event::QuitWritten {
                written: false,
                unsaved_tabs: 1,
            });
            self.carry_out(out);
        }
    }
}

/// A test's way to have the quit's thread not start, as a system out of threads would refuse it. Read only in a test's build.
pub(crate) static QUIT_THREAD_REFUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
