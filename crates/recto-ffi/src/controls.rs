//! **The one way a person's action comes in, for the host** (step R2.3, C7): [`AppCore::act`], with [`UiAction`] and [`Acted`] restated for UniFFI as
//! `recto-app` has them. What each does is `Inner::act`'s, and no rule of it is here.

use crate::AppCore;
use crate::menus::HostAction;
use crate::saving::CloseChoice;
use recto_app::SidebarRow;
pub use recto_app::core::controls::{Acted, UiAction};

/// What the person did, as the window reports it: which control, and what it knows. Nothing here decides what it does.
#[uniffi::remote(Enum)]
pub enum UiAction {
    /// A menu item, a key, a panel button or the context menu, by its id.
    Menu { id: String },
    /// A sidebar row tapped, or ⌘- or ⌃-clicked (`adding`).
    Row { row: SidebarRow, adding: bool },
    /// The reader's moved into another pane, by the pointer or a scroll, as the page views report it: that pane's tab.
    PaneRead { tab: u32 },
    /// A contents row's chevron, by the row's place in the contents.
    Chevron { first: u32 },
    /// A file to open: the Open panel's, a drop, the Finder's, by its path.
    Open { path: String },
    /// File > Open Recent's file, by its key.
    OpenRecent { key: String },
    /// What's in the Find field now, as it's typed.
    FindTyped { text: String },
    /// ⌘F's opening of Find, from the field's own button.
    FindOpen,
    /// Escape in the Find field, or its close button.
    FindClose,
    /// Return (forward) or ⇧Return in the Find field.
    FindReturn { forward: bool },
    /// A row of Find's list, clicked.
    FindChoose { row: u32 },
    /// Return in the go-to field.
    PageTyped { text: String },
    /// A swipe on the page, as the system reports it.
    Swiped { delta_x: f64 },
    /// Return in the zoom level's field, with the Mac's decimal sign (its first character; `.` if it has none).
    ZoomTyped { text: String, decimal: String },
    /// The sidebar's button beside the window's controls.
    ToggleSidebar,
    /// A row of the palette, run.
    PaletteRun { id: String },
    /// The close prompt's answer, for the tab it asked about.
    CloseAnswered { tab: u32, choice: CloseChoice },
    /// Save As's sheet answered with this path.
    SaveAs { tab: u32, path: String },
    /// A switch in Settings.
    SettingChosen { id: String, on: bool },
}

/// What came of an action for the host: what's left for it to do, and whether the action was something to act on (a typed page or zoom that's understood).
#[uniffi::remote(Record)]
pub struct Acted {
    pub host: HostAction,
    pub understood: bool,
}

#[uniffi::export]
impl AppCore {
    /// **Whatever the person did, at any control:** a menu item, a key, a row, the Find, go-to and zoom fields, a sheet's answer, a switch in Settings. Never
    /// waits, and says what's left for the host.
    pub fn act(&self, action: UiAction) -> Acted {
        self.inner.act(action)
    }
}
