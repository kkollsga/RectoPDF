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
        let _ = std::thread::Builder::new().name("recto-quit".into()).stack_size(256 * 1024).spawn(move || {
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
        });
    }
}
