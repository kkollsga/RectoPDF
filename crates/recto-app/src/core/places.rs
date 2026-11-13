//! **What the page views say of where they are, taken into the session,** by one rule for every caller: the session thread's look after each call, a
//! command about to jump or step (the person's and an agent's alike), and the record on its way to disk.
//!
//! A jump reaches the session at once and a page view later: on the session thread's look after the command, or by an effect it carries out (Find's hit
//! brought into view), and the page view tells of where that took it later still, when it's next asked. So what a page view says can be about a place
//! the session has since moved on from. Taken then, it undoes the newer move: an agent's `back` straight after `find --next --jump` stayed on the hit's
//! page, as the page view's word that it went there for the jump was taken after Back as the reader's own scroll (the red `main` of CI run 43401013894),
//! and the person's ⌘[ pressed before a link's scroll had landed did the same. Hence [`is_views_own`].

use super::*;
use crate::panes::Sent;
use recto_index::select::Range;
use recto_reader::tiles::SavedPlace;
use recto_session::record::{PlaceRecord, Selection};
use recto_session::trail::Stop;

/// **The rule:** what a page view says of where it is in its tab, the page it moved to and exactly where, is the view's own only when the view has been
/// sent everything the session has for that tab: the tab and page it was last sent are the tab and the page the session has, and no jump is waiting to be
/// sent to it. Otherwise the session's place is the newer, and what the view says is of where it was before it's sent on, as it will be: it's dropped.
#[must_use]
pub fn is_views_own(sent: Option<Sent>, tab: TabId, session_page: u32, jump_waiting: bool) -> bool {
    !jump_waiting && sent == Some(Sent { tab, page: session_page })
}

/// What one pane's page view said, read off it.
struct PaneSaid {
    /// The tab it shows.
    hot: Option<TabId>,
    /// The page the reader moved to since it was last asked, which asking takes. Read only by the session thread's look.
    moved: Option<u32>,
    /// Exactly where each tab it holds is: the one it shows and the warm ones, left where they were.
    places: Vec<(u32, SavedPlace)>,
}

/// **What the page views said,** read off them with no lock on the session, to be taken in under it by [`Self::take_in`].
pub struct Said {
    panes: Vec<PaneSaid>,
    /// What's selected in the pane being read, by tab: the session keeps one selection.
    selection: Option<(u32, Option<Range>)>,
}

impl Said {
    /// Reads each pane's page view. `moved`: the pages the readers moved to are taken too, which only the session thread's look does, as asking takes them.
    pub fn read(inner: &Inner, moved: bool) -> Self {
        let panes = inner
            .panes
            .engines(&inner.engine)
            .iter()
            .map(|engine| PaneSaid {
                hot: engine.hot_tab().map(TabId),
                moved: if moved { engine.take_moved_page() } else { None },
                places: engine.places(),
            })
            .collect();
        let selection = inner.panes.reading_engine(&inner.engine).hot_selection();
        Self { panes, selection }
    }

    /// **Takes in what was said, by [`is_views_own`],** with the session held. `sent` is what each pane's view was last sent, `waiting` the tabs with a
    /// jump the session thread is about to send. A tab a view holds but doesn't show is taken as it was left. Says each pane whose reader moved to another
    /// page, which the session now has, with that page, and announces it.
    pub fn take_in(
        self,
        session: &mut recto_session::Session<DrawableLoader>,
        sent: &[Option<Sent>],
        waiting: &[(TabId, Stop)],
        events: &mut Vec<Event>,
    ) -> Vec<(usize, u32)> {
        if let Some((tab, range)) = self.selection {
            session.note_selection(
                TabId(tab),
                range.map(|range| Selection {
                    from_page: range.start.page,
                    from_char: range.start.at,
                    to_page: range.end.page,
                    to_char: range.end.at,
                }),
            );
        }
        let in_panes = session.panes();
        let mut went = Vec::new();
        for (pane, said) in self.panes.into_iter().enumerate() {
            let own = said.hot.filter(|hot| {
                let was = session.tab(*hot).map(|tab| tab.page);
                let waits = waiting.iter().any(|(id, _)| id == hot);
                was.is_some_and(|was| is_views_own(sent.get(pane).copied().flatten(), *hot, was, waits))
            });
            // A reader's move is the session's only for the tab the session still has in that pane.
            if let (Some(id), Some(page)) = (own.filter(|id| in_panes.get(pane) == Some(id)), said.moved) {
                let was = session.tab(id).map(|tab| tab.page);
                if session.go_to(id, page).is_ok() {
                    let now = session.tab(id).map_or(page, |tab| tab.page);
                    // Announced when the page changed, and only then: a page view settling on the page the session already has is no news.
                    if was != Some(now) {
                        events.push(recto_agent::page_changed(session, id, now + 1));
                        went.push((pane, now));
                    }
                }
            }
            for (tab, saved) in said.places {
                let id = TabId(tab);
                if said.hot == Some(id) && own != Some(id) {
                    continue;
                }
                session.note_place(id, place_record(&saved));
            }
        }
        went
    }
}

impl Said {
    /// **What the session thread's look takes in,** with the session held: the jumps it's about to send are taken first, then what the views said by
    /// [`is_views_own`], with the pages the readers moved to kept as what each pane's view now has. The jumps go back, for the look to send.
    pub fn take_in_at_the_look(self, session: &mut recto_session::Session<DrawableLoader>, synced: &mut Synced, events: &mut Vec<Event>) -> Vec<(TabId, Stop)> {
        let jumps: Vec<(TabId, Stop)> = session.panes().into_iter().filter_map(|id| Some((id, session.take_jump(id)?))).collect();
        for (pane, page) in self.take_in(session, &sent_to_views(synced), &jumps, events) {
            if let Some(pane) = synced.panes.get_mut(pane) {
                pane.page = page;
            }
        }
        jumps
    }
}

/// The jump taken for a tab, for the look to send, taken once.
pub fn jump_for(jumps: &mut Vec<(TabId, Stop)>, id: TabId) -> Option<Stop> {
    jumps.iter().position(|(tab, _)| *tab == id).map(|at| jumps.swap_remove(at).1)
}

/// What each pane's page view has been sent: its tab and page, as the session thread's last look left them.
pub fn sent_to_views(synced: &Synced) -> Vec<Option<Sent>> {
    synced.panes.iter().map(|pane| pane.tab.map(|tab| Sent { tab, page: pane.page })).collect()
}

fn place_record(saved: &SavedPlace) -> PlaceRecord {
    PlaceRecord {
        page: saved.page,
        anchor_page: saved.anchor.0,
        within: saved.anchor.1,
        x: saved.x,
        y: saved.y,
        document_height: saved.document_height,
        zoom: saved.magnification,
        page_width: saved.page_size.map_or(0.0, |size| size.width),
        page_height: saved.page_size.map_or(0.0, |size| size.height),
    }
}

impl Inner {
    /// **Takes in where the page views say each tab they hold is,** by [`is_views_own`]: before a command jumps or steps, so the trail marks where the
    /// view really is, and before the record is written. The pages the readers moved to are left for the session thread's look, which alone knows which
    /// jumps it's about to send. A jump on the page already showing, still waiting, isn't seen from here; the mark doesn't need it to be, as the session
    /// answers where the reader is from that jump itself (`Session::here`).
    pub fn take_places(&self) {
        let said = Said::read(self, false);
        #[cfg(any(test, feature = "testkit"))]
        self.serving.on_taking_places.read();
        let sent = self.panes.sent();
        self.host.session.with_session(|session| {
            said.take_in(session, &sent, &[], &mut Vec::new());
        });
        #[cfg(any(test, feature = "testkit"))]
        self.serving.on_taking_places.taken();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure (the red `main`): what a page view says of its tab is taken while the session has moved the tab on and not yet sent it there, so the older
    /// place undoes the newer; or it's dropped when the view has everything, so the reader's own scroll is lost.
    #[test]
    fn a_views_place_is_its_own_only_once_it_has_everything() {
        let (tab, other) = (TabId(1), TabId(2));
        let sent = Some(Sent { tab, page: 9 });
        assert!(is_views_own(sent, tab, 9, false), "sent everything: the reader's own");
        assert!(!is_views_own(sent, tab, 0, false), "the session went back to page 1 since");
        assert!(!is_views_own(sent, tab, 9, true), "a jump on the same page waits to be sent");
        assert!(
            !is_views_own(Some(Sent { tab: other, page: 9 }), tab, 9, false),
            "the view was sent another tab"
        );
        assert!(!is_views_own(None, tab, 9, false), "the view was sent nothing yet");
    }
}
