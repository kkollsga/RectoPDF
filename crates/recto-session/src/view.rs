//! The view: settings, find, edit mode, selection, place and draws.

use super::*;

/// **What a read of a page view says of a tab:** where its reader is, which the session keeps and writes, and the line its contents' current entry is
/// found by, the page and thousandths down it (`recto_reader`'s `reading_line`: the top of the view, and in the last view-height of the scroll on its way to
/// the last page's foot), which is the view's word only: not written, not restored, not used by Back. A place alone says the line is its top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewSaid {
    pub place: PlaceRecord,
    pub line: (u32, u16),
}

impl From<PlaceRecord> for ViewSaid {
    fn from(place: PlaceRecord) -> Self {
        Self {
            line: (place.anchor_page, recto_core::thousandths(place.within)),
            place,
        }
    }
}

impl<L: DocLoader> Session<L> {
    /// **Exactly where a tab's reader is, as a read of the page view stamped `stamp` on the place clock says,** taken in only if it's as new as the
    /// read taken in last on this tab, whoever took that in: an older read still landing doesn't undo a newer one, the person's own scroll among it. A jump
    /// the view was sent at or before `stamp` is then told of ([`Self::here`]). Says whether it was taken. Marked to be written only if it's moved: the
    /// line said with it ([`ViewSaid`]) is the page view's word, kept with it while the tab is hot or warm, and never written.
    pub fn view_told_of(&mut self, id: TabId, said: impl Into<ViewSaid>, stamp: u64) -> bool {
        let ViewSaid { place, line } = said.into();
        if self.tabs.tab(id).is_none() || !self.mail.view_told_of(id, stamp, line, (place.anchor_page, place.within)) {
            return false;
        }
        if let Some(tab) = self.tabs.tab_mut(id)
            && tab.reading.place != Some(place)
        {
            tab.reading.place = Some(place);
            tab.place_dirty = true;
        }
        true
    }

    /// What's changed that a line of the log of places can say. Marks it as said.
    pub fn take_place_delta(&mut self, seq: u64) -> PlaceDelta {
        let mut places = Vec::new();
        for tab in self.tabs.iter_mut() {
            if std::mem::take(&mut tab.place_dirty) {
                places.push((tab.id.0, tab.page, tab.reading.place));
            }
        }
        PlaceDelta {
            seq,
            active: self.tabs.active().map(|id| id.0),
            places,
            contents_scroll: None,
            focus: None,
        }
    }
}
