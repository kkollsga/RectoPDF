//! **The app core's listener, as the host's.**

use crate::AppListener;
use recto_app as app;
use std::sync::Arc;

/// The app core's listener is the host's: each call forwards, one for one and on the thread it was made on, to the UniFFI listener. The promises are `recto_app::Listener`'s.
pub(crate) struct HostListener(pub(crate) Arc<dyn AppListener>);

impl app::Listener for HostListener {
    fn model_changed(&self) {
        self.0.model_changed();
    }

    fn ops_ready(&self) {
        self.0.ops_ready();
    }

    fn qa_request(&self, id: u64, verb: String, value: String) {
        self.0.qa_request(id, verb, value);
    }

    fn session_ended(&self) {
        self.0.session_ended();
    }

    fn copy_ready(&self) {
        self.0.copy_ready();
    }

    fn restore_ready(&self) {
        self.0.restore_ready();
    }

    fn quit_answer(&self, answer: app::listener::QuitAnswer) {
        self.0.quit_answer(answer);
    }

    fn hand_off_quit(&self) {
        self.0.hand_off_quit();
    }
}
