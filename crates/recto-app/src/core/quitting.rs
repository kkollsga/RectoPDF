//! **A quit the host asked for, and the way out:** everything written on a thread of its own, the answer, and the socket ended.

use super::*;

impl Inner {
    /// A quit was asked for, by the menu or the system. Everything's written, on a thread of its own, and `quit_answer` says what to do. Mac apps with Resume don't ask on the way out, and neither does this, as unsaved edits come back: closing a tab with them is where Don't Save, Cancel or Save belongs. It asks only if they couldn't be kept.
    pub fn quit_requested(self: &Arc<Self>) {
        let inner = Arc::clone(self);
        // What's done from here wouldn't come back. If it turns out it isn't quitting, it's taken again.
        inner.set_frozen(true);
        inner.updates.stop();
        inner.tools.tool.quitting();
        inner.tools.printing.quitting();
        let _ = std::thread::Builder::new().name("recto-quit".into()).stack_size(256 * 1024).spawn(move || {
            let (sender, receiver) = std::sync::mpsc::channel();
            let flushing = Arc::clone(&inner);
            let _ = std::thread::Builder::new().name("recto-flush".into()).stack_size(512 * 1024).spawn(move || {
                let _ = sender.send(flushing.saver.flush(false, Duration::from_secs(2)));
            });
            let outcome = receiver.recv_timeout(QUIT_PATIENCE);
            let fine = matches!(outcome, Ok(Ok(_))) || !inner.saver.store.holds_the_record();
            let unsaved = inner.host.session.unsaved_tabs();
            // Asked for, Quit Anyway included, so this run of a version being watched didn't end early. Stay says otherwise.
            inner.updates.quitting_as_asked();
            if !(fine || unsaved.is_empty()) {
                inner.set_frozen(false);
                inner.updates.stop_over();
            }
            inner.listener.quit_answer(crate::listener::QuitAnswer::for_quit(fine, unsaved.len()));
        });
    }

    /// The quit's question was answered with Stay: the app goes on, and isn't quitting after all.
    pub fn quit_stayed(&self) {
        self.updates.quit_not_taken();
    }

    /// Ends the socket, on the way out. Returns at once.
    pub fn shutdown(&self) {
        if let Some(stopper) = lock(&self.serving.stopper).take() {
            stopper.stop();
        }
        self.forget_a_quiet_copy();
    }
}
