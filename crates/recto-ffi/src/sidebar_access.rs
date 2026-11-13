//! **Accessibility of the sidebar's own parts: the records and the calls.** What VoiceOver hears, what a key or an accessibility action folds, and the thumb's colours with Increase Contrast are `recto_app::sidebar_access`'s; these forward to it.

use crate::{AppCore, RowKind, SidebarRow, lock};
use recto_app::sidebar_access as app;
pub use recto_app::sidebar_access::{AccessibilityStep, ThumbAccess};

/// A scroll thumb as VoiceOver has it.
#[uniffi::remote(Record)]
pub struct ThumbAccess {
    /// Its name, for the list it scrolls.
    pub label: String,
    /// Where the list is, from 0 at the top to 1 at the end.
    pub value: f64,
    /// The same in words.
    pub value_text: String,
    /// How far an increment or a decrement moves it, as a part of what shows.
    pub page_share: f64,
}

/// The thumb's opacity with Increase Contrast on, light and dark, at rest and under the pointer. Its colours are the usual ones.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Record)]
pub struct ThumbContrast {
    pub light_alpha: f64,
    pub light_hover_alpha: f64,
    pub dark_alpha: f64,
    pub dark_hover_alpha: f64,
}

/// One of `recto qa accessibility`'s steps, read by Rust, for the host to call what the system would.
#[uniffi::remote(Enum)]
pub enum AccessibilityStep {
    /// As though Increase Contrast were on or off, for this app alone.
    Contrast { on: bool },
    /// The arrows with the sidebar's list holding the keyboard: `left` or `right`.
    FoldKey { key: String },
    /// Contents row `row`'s fold action.
    FoldAction { row: u32 },
    /// A thumb's increment or decrement: `list` is `contents`, `tabs`, `find` or `pages`.
    Thumb { list: String, increment: bool },
}

#[uniffi::export]
impl AppCore {
    /// A scroll thumb as VoiceOver has it: `list` is `contents`, `tabs`, `find` or `pages`, and `fraction` how far down the list is, 0 to 1.
    pub fn thumb_access(&self, list: String, fraction: f64) -> ThumbAccess {
        app::thumb_access_for(&list, fraction)
    }

    /// `recto qa accessibility`'s value, read: nothing if it isn't one.
    pub fn qa_accessibility_step(&self, value: String) -> Option<AccessibilityStep> {
        app::qa_step(&value)
    }

    /// What's said when `recto qa accessibility`'s step did nothing.
    pub fn qa_accessibility_refusal(&self, value: String) -> String {
        app::refusal(app::qa_step(&value).as_ref()).to_string()
    }

    /// The thumb's opacity with Increase Contrast on.
    pub fn thumb_contrast(&self) -> ThumbContrast {
        let thumb = recto_session::SCROLL_THUMB;
        ThumbContrast {
            light_alpha: thumb.light_contrast.1,
            light_hover_alpha: thumb.light_contrast_hover.1,
            dark_alpha: thumb.dark_contrast.1,
            dark_hover_alpha: thumb.dark_contrast_hover.1,
        }
    }

    /// The action a contents row offers for its fold, in words, or nothing for a row with no sections under it.
    pub fn fold_action_name(&self, row: SidebarRow) -> Option<String> {
        app::fold_action_name(row.kind == RowKind::Contents, row.expandable, row.expanded)
    }

    /// A contents row's fold action, as VoiceOver calls it: what a click on its fold does, and a fold is kept for → to open again.
    pub fn fold_action(&self, first: u32) {
        let entry = lock(&self.inner.published)
            .contents
            .get(first as usize)
            .map(|entry| (entry.tab, entry.entry, entry.expanded));
        self.inner.tools.folds().acted(entry);
        self.act(crate::controls::UiAction::Chevron { first });
    }

    /// A key with the sidebar's list holding the keyboard: `left` folds the section being read, `right` opens it. Says whether it did anything, so the key goes on to whatever else takes it if not.
    pub fn contents_fold_key(&self, key: String) -> bool {
        let Some(key) = app::FoldKey::from_name(&key) else { return false };
        let published = lock(&self.inner.published);
        if !published.sidebar.toc_open {
            return false;
        }
        let (contents, current) = (std::sync::Arc::clone(&published.contents), published.sidebar.contents_current);
        drop(published);
        let Some(target) = self.inner.tools.folds().key_target(key, &contents, current) else {
            return false;
        };
        self.act(crate::controls::UiAction::Chevron {
            first: u32::try_from(target).unwrap_or(u32::MAX),
        });
        true
    }
}
