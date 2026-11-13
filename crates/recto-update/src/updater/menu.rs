//! **The menu's update item, and its mark** (step R2.3, C8): what's said of what the menu started is put up as an alert too, when there's no window to say it
//! in. The window's item is the person's `update check` or `update install --now` (`recto-app`'s `core::updates`), which marks it first through
//! [`Updater::menu`], and the updates feature's answered step reads and clears the mark.

use super::{St, Updater, lock};
use std::sync::Mutex;

/// The menu's mark on what it started, and the alert it leaves: a part of the updater's state, reached through [`Updater::menu`].
pub struct Menu<'a> {
    st: &'a Mutex<St>,
}

impl Updater {
    /// The menu's mark and its alert.
    #[must_use]
    pub fn menu(&self) -> Menu<'_> {
        Menu { st: &self.st }
    }
}

impl Menu<'_> {
    /// **The person chose the menu's item:** what's said of what it starts is put up as an alert too, when there's no window to say it in. `false` if
    /// something's under way, which was someone else's to ask for, and the item then does nothing.
    pub fn mark(&self) -> bool {
        let mut st = lock(self.st);
        if st.status.is_under_way() {
            return false;
        }
        st.from_menu = true;
        true
    }

    /// Whether what's said next was asked for from the menu.
    #[must_use]
    pub fn marked(&self) -> bool {
        lock(self.st).from_menu
    }

    /// What the menu's item asked for didn't start, or has been answered: nothing more is put up as an alert.
    pub fn unmark(&self) {
        lock(self.st).from_menu = false;
    }

    /// What's been said of something chosen from the menu, to show as an alert when there's no window to say it in. Taken once.
    #[must_use]
    pub fn take_alert(&self) -> Option<String> {
        lock(self.st).alert.take()
    }
}
