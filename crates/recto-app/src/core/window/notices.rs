//! **What the window says by the page:** a notice that goes by itself, and why an open failed. Its own part of the window, reached as
//! [`super::Window::notices`]: whoever says something says it here, and the window's look reads it here. Which tab a notice is about is the window's
//! ([`super::Window::shows_tab`]): its atomic sits in the window's padding beside the other two, where in here it would add eight bytes of its own.

use crate::lock;
use std::sync::Mutex;

/// The window's notices: each a short lock, set from the session thread and the threads a person's command runs on, read by the window's look.
pub struct Notices {
    /// Why a file the person opened couldn't be.
    open_failure: Mutex<Option<String>>,
    /// Why something the reader chose couldn't be done, or what a save has to say, as a notice that goes by itself.
    action_notice: Mutex<Option<String>>,
}

impl Notices {
    /// Nothing said.
    pub(super) fn new() -> Self {
        Self {
            open_failure: Mutex::new(None),
            action_notice: Mutex::new(None),
        }
    }

    /// A line by the page that goes by itself, in place of any that was there. Whoever wants it drawn at once tells the session thread to look.
    pub(in crate::core) fn say(&self, words: String) {
        *lock(&self.action_notice) = Some(words);
    }

    /// The notice goes, as the reader dismissed it or what it was about is done; what it said, if anything.
    pub(in crate::core) fn take_notice(&self) -> Option<String> {
        lock(&self.action_notice).take()
    }

    /// The notice as it is now.
    pub(in crate::core) fn notice(&self) -> Option<String> {
        lock(&self.action_notice).clone()
    }

    /// A file the person opened couldn't be, and this is why.
    pub(in crate::core) fn fail_open(&self, words: String) {
        *lock(&self.open_failure) = Some(words);
    }

    /// An open's been asked for again: what the last one failed with goes.
    pub(in crate::core) fn clear_open_failure(&self) {
        *lock(&self.open_failure) = None;
    }

    /// Why the last open failed, if it did.
    pub(in crate::core) fn open_failure(&self) -> Option<String> {
        lock(&self.open_failure).clone()
    }
}
