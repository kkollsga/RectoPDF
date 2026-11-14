//! **The window's features with state of their own,** held together for the app: the sheets that ask, saving's, the snapshot tool, the sidebar's folds, the
//! chrome's marks, printing, ⌘K and Find's field. Each is its feature's own type, in its own file; this only holds them.

use super::{printing, saving, snapshot_tool};
use std::sync::atomic::{AtomicU8, AtomicUsize};
use std::sync::{Arc, Mutex};

/// **The window's features with state of their own:** the sheets that ask, saving's, the snapshot tool, the sidebar's folds, the chrome's marks, printing, ⌘K and
/// Find's field.
pub struct Tools {
    /// The sheets that ask: a password, or whether to open a link.
    prompts: crate::prompts::Prompts,
    /// Saves from the window: how many run, and what one that failed says.
    save_ui: saving::SaveUi,
    /// The snapshot tool: on or off, the box being drawn, and what's on its way to the clipboard.
    tool: snapshot_tool::Tool,
    /// The sidebar's folds as the keyboard moves through them.
    folds: crate::sidebar_access::Folds,
    /// The window's marks: what the pointer is over or holds.
    chrome: crate::chrome::Chrome,
    /// A print being prepared, or waiting for the host to hand to the system.
    printing: printing::Printing,
    /// ⌘K's palette, on the UI thread.
    palette: crate::palette::Palette,
    /// The Find field as the session thread last set it, for the UI thread.
    find_field: Mutex<crate::find::Field>,
    /// Looks at where a tab's generated contents stand, each on a thread of its own, until what it found is queued ([`super::generated_contents`]).
    contents_looks: Arc<AtomicUsize>,
}

impl Tools {
    /// At launch: every tool at rest.
    pub fn new(save_step: &Arc<AtomicU8>) -> Self {
        Self {
            prompts: crate::prompts::Prompts::default(),
            save_ui: saving::SaveUi::new(Arc::clone(save_step)),
            tool: snapshot_tool::Tool::default(),
            folds: crate::sidebar_access::Folds::default(),
            chrome: crate::chrome::Chrome::default(),
            printing: printing::Printing::default(),
            palette: crate::palette::Palette::default(),
            find_field: Mutex::new(crate::find::Field::default()),
            contents_looks: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Looks at generated contents under way, counted by their way back to the session thread ([`super::generated_contents`]).
    pub(crate) fn contents_looks(&self) -> &Arc<AtomicUsize> {
        &self.contents_looks
    }

    /// The sheets that ask: a password, or whether to open a link.
    pub fn prompts(&self) -> &crate::prompts::Prompts {
        &self.prompts
    }

    /// Saves from the window.
    pub(crate) fn save_ui(&self) -> &saving::SaveUi {
        &self.save_ui
    }

    /// The snapshot tool.
    pub fn tool(&self) -> &snapshot_tool::Tool {
        &self.tool
    }

    /// The sidebar's folds.
    pub fn folds(&self) -> &crate::sidebar_access::Folds {
        &self.folds
    }

    /// The window's marks.
    pub fn chrome(&self) -> &crate::chrome::Chrome {
        &self.chrome
    }

    /// A print being prepared, or waiting for the host.
    pub fn printing(&self) -> &printing::Printing {
        &self.printing
    }

    /// ⌘K's palette.
    pub(crate) fn palette(&self) -> &crate::palette::Palette {
        &self.palette
    }

    /// The Find field as the session thread last set it, for the UI thread.
    pub(crate) fn find_field(&self) -> &Mutex<crate::find::Field> {
        &self.find_field
    }
}
