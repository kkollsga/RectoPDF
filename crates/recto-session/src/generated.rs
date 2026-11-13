//! Generated contents the sidebar knows of: where each asked-about tab's stand, a few at a time, and a ready list in memory only for the tab on screen.

use crate::{GeneratedView, MAX_GENERATED_VIEWS, TabId};

/// Where tabs' generated contents stand, for tabs someone asked about.
#[derive(Debug, Default)]
pub(crate) struct Generated {
    /// At most [`MAX_GENERATED_VIEWS`], the oldest out, so a thousand tabs can't make this grow.
    generated: Vec<(TabId, GeneratedView)>,
}

impl Generated {
    pub(crate) fn view(&self, id: TabId) -> Option<&GeneratedView> {
        self.generated.iter().find(|(tab, _)| *tab == id).map(|(_, view)| view)
    }

    /// Notes where a tab's stand. The oldest note gives way past [`MAX_GENERATED_VIEWS`], but never that of `active`, the tab on screen.
    pub(crate) fn set(&mut self, id: TabId, view: GeneratedView, active: Option<TabId>) {
        self.generated.retain(|(tab, _)| *tab != id);
        self.generated.push((id, view));
        while self.generated.len() > MAX_GENERATED_VIEWS {
            let Some(oldest) = self.generated.iter().position(|(tab, _)| Some(*tab) != active) else {
                break;
            };
            self.generated.remove(oldest);
        }
    }

    /// Forgets where a tab's stand.
    pub(crate) fn forget(&mut self, id: TabId) {
        self.generated.retain(|(tab, _)| *tab != id);
    }

    /// Only this tab, the one on screen, holds a ready list in memory. The rest are on disk, and a cold tab stays a cold tab's size.
    pub(crate) fn keep_ready_only_for(&mut self, id: TabId) {
        self.generated.retain(|(tab, view)| *tab == id || !matches!(view, GeneratedView::Ready(_)));
    }

    /// Tabs whose generated contents are being made.
    pub(crate) fn making(&self) -> Vec<TabId> {
        self.generated
            .iter()
            .filter(|(_, view)| *view == GeneratedView::Making)
            .map(|(tab, _)| *tab)
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.generated.len()
    }
}
