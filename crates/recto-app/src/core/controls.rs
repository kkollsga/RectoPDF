//! **The one way a person's action comes in** (step R2.3): every control the window has (a menu item, a key, a panel button, a row, the Find and go-to fields,
//! a sheet's answer) is a [`UiAction`], and [`Inner::act`] is where it's turned into what it does. An action that's an agent's command too is run as that command,
//! for the person, by the one executor (`core/person.rs`); what's left is the host's alone (a sheet, a panel, focus), which comes back as a [`HostAction`].
//!
//! The bridge has one export for all of it, `AppCore::act`, which is a call to [`Inner::act`]; nothing else of the window's comes in another way.

use super::{HostAction, Inner, lock};
use crate::SidebarRow;
use crate::saving::CloseChoice;
use recto_session::TabId;
use std::path::PathBuf;
use std::sync::Arc;

/// What the person did, as the window reports it: which control, and what it knows. Nothing here decides what it does.
#[derive(Clone, Debug)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Acted {
    pub host: HostAction,
    pub understood: bool,
}

impl Acted {
    const DONE: Self = Self {
        host: HostAction::None,
        understood: true,
    };

    const NOT_UNDERSTOOD: Self = Self {
        host: HostAction::None,
        understood: false,
    };

    fn host(host: HostAction) -> Self {
        Self { host, understood: true }
    }
}

impl Inner {
    /// **Does what the person did,** whatever the control. Never waits: what's slow goes on the session thread or a thread of its own.
    ///
    /// Each control is its feature's: the feature says what command it is (in the feature's file, from what it reads), and the command is the person's,
    /// sent to the one executor ([`super::person::submit`]). What isn't one command (a menu id's row, the palette's, an open, the close prompt's answer,
    /// Save As, a setting, the zoom, run where it's asked) is its feature's whole, and returns here.
    pub fn act(self: &Arc<Self>, action: UiAction) -> Acted {
        let command = match action {
            UiAction::Menu { id } => return Acted::host(self.perform(id)),
            UiAction::Row { row, adding } => super::opening::row_command(row, adding, &self.panes, &self.engine),
            UiAction::PaneRead { tab } => Some(super::comparison::reading(TabId(tab))),
            UiAction::Chevron { first } => super::contents::chevron(&lock(&self.published).contents, first),
            UiAction::Open { path } => {
                super::opening::open_for_person(&self.life, &self.core, PathBuf::from(path));
                return Acted::DONE;
            }
            UiAction::OpenRecent { key } => {
                let recent = Arc::clone(&lock(&self.published).recent);
                super::opening::open_recent(&recent, &self.life, &self.core, &key);
                return Acted::DONE;
            }
            UiAction::FindTyped { text } => Some(super::find::typed(text)),
            UiAction::FindOpen => Some(super::find::opened()),
            UiAction::FindClose => Some(super::find::closed()),
            // The person's jump to the hit, so Back comes here again. ⌘G and ⇧⌘G are the menu's, and only step.
            UiAction::FindReturn { forward } => Some(super::find::step(forward, true)),
            UiAction::FindChoose { row } => super::find::handle(self, |find| super::find::chosen(find, row)),
            UiAction::PageTyped { text } => {
                let Some(goto) = super::navigation::page_typed(&text, &self.core.frozen, &self.panes) else {
                    return Acted::NOT_UNDERSTOOD;
                };
                Some(goto)
            }
            UiAction::Swiped { delta_x } => super::navigation::swiped(delta_x),
            UiAction::ZoomTyped { text, decimal } => {
                let typed = recto_reader::tiles::typed_zoom(&text, decimal.chars().next().unwrap_or('.'));
                return Acted {
                    host: HostAction::None,
                    understood: typed.is_some_and(|zoom| super::zoom::for_person(&self.host, &self.serving, zoom)),
                };
            }
            UiAction::ToggleSidebar => Some(super::viewing::toggle_sidebar()),
            UiAction::PaletteRun { id } => return Acted::host(super::palette::run(self, &id)),
            UiAction::CloseAnswered { tab, choice } => return Acted::host(self.close_answered(tab, choice)),
            UiAction::SaveAs { tab, path } => {
                self.save_as(tab, path);
                return Acted::DONE;
            }
            UiAction::SettingChosen { id, on } => super::updates::setting_command(&id, on),
        };
        if let Some(command) = command {
            super::person::submit(self, command);
        }
        Acted::DONE
    }
}
