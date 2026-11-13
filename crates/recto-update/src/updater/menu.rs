//! **The menu's update item, and its mark** (step R2.3, C8): what's said of what the menu started is put up as an alert too, when there's no window to say it
//! in. The window's item is the person's `update check` or `update install --now` (`recto-app`'s `core::updates`), which marks it first; [`Updater::menu_chosen`]
//! is the same steps in one call, for the updater's own tests of what the mark does.

use super::{Asked, InstallRefused, Updater, lock};
use std::sync::Arc;

impl Updater {
    /// The menu item was chosen. Whichever it says: it starts on a thread of its own, and the UI's not held.
    pub fn menu_chosen(self: &Arc<Self>) {
        {
            let mut st = lock(&self.st);
            // Only what this starts: something already under way was someone else's to ask for.
            if st.status.is_under_way() {
                return;
            }
            st.from_menu = true;
        }
        // The same call as Settings' button and `recto update install`: a waiting build goes at once, by hand, else what's on offer; and with neither, the menu checks.
        let started = match self.update_now() {
            Ok(_) => true,
            // A local build never installs, so for it the menu only ever checks.
            Err(InstallRefused::NothingOnOffer | InstallRefused::NotInThisBuild) => self.check_in_background(Asked::ByHand),
            Err(_) => false,
        };
        if !started {
            lock(&self.st).from_menu = false;
        }
    }

    /// **The person chose the menu's item:** what's said of what it starts is put up as an alert too, when there's no window to say it in. `false` if
    /// something's under way, which was someone else's to ask for, and the item then does nothing.
    pub fn mark_from_menu(&self) -> bool {
        let mut st = lock(&self.st);
        if st.status.is_under_way() {
            return false;
        }
        st.from_menu = true;
        true
    }

    /// Whether what's said next was asked for from the menu.
    #[must_use]
    pub fn marked_from_menu(&self) -> bool {
        lock(&self.st).from_menu
    }

    /// What the menu's item asked for didn't start, or has been answered: nothing more is put up as an alert.
    pub fn unmark_from_menu(&self) {
        lock(&self.st).from_menu = false;
    }

    /// What's been said of something chosen from the menu, to show as an alert when there's no window to say it in. Taken once.
    #[must_use]
    pub fn take_alert(&self) -> Option<String> {
        lock(&self.st).alert.take()
    }
}
