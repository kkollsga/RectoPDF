//! **The window's marks that come and go** (SPEC section 3). The zoom level in the top right and the page number in the lower right show when they change and fade 3 s after their last change (owner, 2026-11-08: "Zoom level should auto hid 3s after last zoom change, and similar behaviour for page number"). With the sidebar hidden, the window's buttons and the sidebar's button beside them fade, and come back while the pointer is over their corner (owner, the same day: "When sidebar is hidden, I want the traffic light buttons and sidebar toggle to autohide. To show them again you hover above them").
//!
//! What shows, and until when, is decided in `recto_app::chrome`; these are its records and the calls, one line each. The host tells what happened (a change, the pointer coming or going, a field open over a mark), draws what [`ChromeLook`] says with its fade, and arms one timer for `next_change_in_ms`. Nothing runs between changes.

use crate::{AppCore, lock};
use recto_app::chrome as app;

/// A mark that comes and goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ChromeMark {
    /// The zoom level, in the top right.
    Zoom,
    /// The page number, in the lower right.
    Place,
    /// The window's buttons and the sidebar's button beside them, in the top left.
    WindowButtons,
}

/// What shows now, and when that next changes with nothing else happening.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ChromeLook {
    pub zoom_shown: bool,
    pub place_shown: bool,
    pub window_buttons_shown: bool,
    pub fade_seconds: f64,
    /// When to ask again: the soonest a mark's time runs out. None while nothing will change by itself.
    pub next_change_in_ms: Option<u64>,
}

impl From<ChromeMark> for app::ChromeMark {
    fn from(mark: ChromeMark) -> Self {
        match mark {
            ChromeMark::Zoom => Self::Zoom,
            ChromeMark::Place => Self::Place,
            ChromeMark::WindowButtons => Self::WindowButtons,
        }
    }
}

impl From<app::ChromeLook> for ChromeLook {
    fn from(look: app::ChromeLook) -> Self {
        Self {
            zoom_shown: look.zoom_shown,
            place_shown: look.place_shown,
            window_buttons_shown: look.window_buttons_shown,
            fade_seconds: look.fade_seconds,
            next_change_in_ms: look.next_change_in_ms,
        }
    }
}

#[uniffi::export]
impl AppCore {
    /// **A mark changed:** the zoom level's words, or the page number's, which the reader's own scroll counts as. What shows is `chrome_look`.
    pub fn chrome_changed(&self, mark: ChromeMark) -> ChromeLook {
        self.inner.chrome.changed(mark.into());
        self.chrome_look()
    }

    /// **The pointer came over a mark's corner, or left it.** What a tracking area calls, and `recto qa corner-pointer`.
    pub fn chrome_pointer(&self, mark: ChromeMark, inside: bool) -> ChromeLook {
        self.inner.chrome.pointer(mark.into(), inside);
        self.chrome_look()
    }

    /// **A field opened over a mark, or closed:** the zoom level's, or the go-to page's.
    pub fn chrome_held(&self, mark: ChromeMark, held: bool) -> ChromeLook {
        self.inner.chrome.held(mark.into(), held);
        self.chrome_look()
    }

    /// **What shows now,** and when to ask again. Cheap: a lock and a comparison or two.
    pub fn chrome_look(&self) -> ChromeLook {
        let sidebar_hidden = lock(&self.inner.published).reader.sidebar_hidden;
        self.inner.chrome.look(sidebar_hidden).into()
    }
}
