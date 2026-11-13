//! **The one way a person's action comes in** (step R2.3): every control the window has (a menu item, a key, a panel button, a row, the Find and go-to fields,
//! a sheet's answer) is a [`UiAction`], and [`Inner::act`] is where it's turned into what it does. An action that's an agent's command too is run as that command,
//! for the person, by the one executor (`core/person.rs`); what's left is the host's alone (a sheet, a panel, focus), which comes back as a [`HostAction`].
//!
//! The bridge's per-control methods are each a call to [`Inner::act`] until the bridge has the one export (R2.3's last commit); nothing else of the window's
//! comes in another way.

use super::{CoreCall, HostAction, Inner, find};
use crate::SidebarRow;
use crate::saving::CloseChoice;
use std::path::PathBuf;
use std::sync::Arc;

/// What the person did, as the window reports it: which control, and what it knows. Nothing here decides what it does.
#[derive(Clone, Debug)]
pub enum UiAction {
    /// A menu item, a key, a panel button or the context menu, by its id.
    Menu { id: String },
    /// A sidebar row tapped, or ⌘- or ⌃-clicked (`adding`).
    Row { row: SidebarRow, adding: bool },
    /// A contents row's chevron, by the row's place in the contents.
    Chevron { first: u32 },
    /// A file to open: the Open panel's, a drop, the Finder's.
    Open { path: PathBuf },
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
    /// Return in the zoom level's field, with the Mac's decimal sign.
    ZoomTyped { text: String, decimal: char },
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

    fn host(host: HostAction) -> Self {
        Self { host, understood: true }
    }
}

impl Inner {
    /// **Does what the person did,** whatever the control. Never waits: what's slow goes on the session thread or a thread of its own.
    pub fn act(self: &Arc<Self>, action: UiAction) -> Acted {
        match action {
            UiAction::Menu { id } => Acted::host(self.perform(id)),
            UiAction::Row { row, adding: false } => {
                self.tap_row(row);
                Acted::DONE
            }
            UiAction::Row { row, adding: true } => {
                self.tap_row_adding(row);
                Acted::DONE
            }
            UiAction::Chevron { first } => {
                self.toggle_contents_section(first);
                Acted::DONE
            }
            UiAction::Open { path } => {
                self.open_file(path);
                Acted::DONE
            }
            UiAction::OpenRecent { key } => {
                self.open_recent(key);
                Acted::DONE
            }
            UiAction::FindTyped { text } => self.find_call(find::FindCall::Typed(text)),
            UiAction::FindOpen => self.find_call(find::FindCall::Open),
            UiAction::FindClose => self.find_call(find::FindCall::Close),
            // The person's jump to the hit, so Back comes here again. ⌘G and ⇧⌘G are the menu's, and only step.
            UiAction::FindReturn { forward } => self.find_call(find::FindCall::Step(forward, recto_agent::find::Going::Jump)),
            UiAction::FindChoose { row } => self.find_call(find::FindCall::Choose(row)),
            UiAction::PageTyped { text } => Acted {
                host: HostAction::None,
                understood: self.page_typed(&text),
            },
            UiAction::Swiped { delta_x } => {
                self.swiped(delta_x);
                Acted::DONE
            }
            UiAction::ZoomTyped { text, decimal } => Acted {
                host: HostAction::None,
                understood: recto_reader::tiles::typed_zoom(&text, decimal).is_some_and(|zoom| self.zoom_for_person(zoom)),
            },
            UiAction::ToggleSidebar => {
                self.toggle_sidebar();
                Acted::DONE
            }
            UiAction::PaletteRun { id } => Acted::host(super::palette::run(self, &id)),
            UiAction::CloseAnswered { tab, choice } => Acted::host(self.close_answered(tab, choice)),
            UiAction::SaveAs { tab, path } => {
                self.save_as(tab, path);
                Acted::DONE
            }
            UiAction::SettingChosen { id, on } => {
                self.setting_chosen(&id, on);
                Acted::DONE
            }
        }
    }

    fn find_call(&self, call: find::FindCall) -> Acted {
        self.core.push(CoreCall::Find(call));
        Acted::DONE
    }
}
