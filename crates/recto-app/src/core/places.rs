//! **What the page views say of where they are, taken into the session,** by one rule for every caller: the session thread's look after each call, a
//! command about to jump or step (the person's and an agent's alike), and the record on its way to disk.
//!
//! A jump reaches the session at once and a page view later: on the session thread's look after the command, or by an effect it carries out (Find's hit
//! brought into view), and the page view tells of where that took it later still, when it's next asked. So what a page view says can be about a place
//! the session has since moved on from. Taken then, it undoes the newer move: an agent's `back` straight after `find --next --jump` stayed on the hit's
//! page, as the page view's word that it went there for the jump was taken after Back as the reader's own scroll (the red `main` of CI run 43401013894),
//! and the person's ⌘[ pressed before a link's scroll had landed did the same. Hence [`is_views_own`].
//!
//! **The newer read wins, by when it was made, not by who read** ([`Said::take_in`]). The views are read with no lock on the session and taken in under
//! it, by the session thread's look, a command (the person's or an agent's) and the saver, on threads of their own: so a read can land after a newer one.
//! Each read of a page view is stamped by the place clock under the view's own lock (`recto_reader::tiles::place_stamp`), which every move of the view's
//! place moves on: a jump or follow sent, a re-layout, a zoom, and the host's every viewport, the person's own scroll among them. A read stamped under the
//! last one the session took in for that tab is older than what it has, and its places are skipped (the look still takes its moved pages and sends its
//! jumps). A jump the look sends is where the reader is from its send until a read stamped at or past the send is taken in, by whichever caller: there's
//! no look after a scroll that stays on the page, so a command's read or the saver's tells of it as well as a look's. A follow synced scroll left pending
//! for the page view's thread hasn't moved the view yet: a read before it is applied says where the view is, and its own tick comes when it's applied.

use super::*;
use crate::panes::ViewGiven;
use recto_index::select::Range;
use recto_reader::tiles::SavedPlace;
use recto_session::record::{PlaceRecord, Selection};
use recto_session::trail::Stop;

/// **The rule:** what a page view says of where it is in its tab, the page it moved to and exactly where, is the view's own only when the view has been
/// given everything the session has for that tab: the tab and page it was last given are the tab and the page the session has, and no jump is waiting
/// to be sent to it. Otherwise the session's place is the newer, and what the view says is of where it was before it's sent on, as it will be: it's dropped.
#[must_use]
pub fn is_views_own(given: Option<ViewGiven>, tab: TabId, session_page: u32, jump_waiting: bool) -> bool {
    !jump_waiting && given == Some(ViewGiven { tab, page: session_page })
}

/// What one pane's page view said, read off it.
struct PaneSaid {
    /// The place clock as the view's places were read, under its lock: how new what it said is.
    stamp: u64,
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
            .map(|engine| {
                let (stamp, places) = engine.places();
                PaneSaid {
                    stamp,
                    hot: engine.hot_tab().map(TabId),
                    moved: if moved { engine.take_moved_page() } else { None },
                    places,
                }
            })
            .collect();
        let selection = inner.panes.reading_engine(&inner.engine).hot_selection();
        Self { panes, selection }
    }

    /// **Takes in what was said, by [`is_views_own`],** with the session held. `given` is what each pane's view was last given, `waiting` the tabs with a
    /// jump the session thread is about to send. A tab a view holds but doesn't show is taken as it was left. Each place goes in only if its read is as
    /// new as the last one taken in for the tab ([`recto_session::Session::view_told_of`]), which also tells of a jump sent before the read. Says each pane
    /// whose reader moved to another page, which the session now has, with that page, and announces it.
    pub fn take_in(
        self,
        session: &mut recto_session::Session<DrawableLoader>,
        given: &[Option<ViewGiven>],
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
                was.is_some_and(|was| is_views_own(given.get(pane).copied().flatten(), *hot, was, waits))
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
                session.view_told_of(id, place_record(&saved), said.stamp);
            }
        }
        went
    }
}

impl Said {
    /// **What the session thread's look takes in,** with the session held: the jumps waiting on the tabs in panes are seen first, and left waiting, then
    /// what the views said by [`is_views_own`], with the pages the readers moved to kept as what each pane's view now has. The jumps go back, for the look to
    /// send and then say sent ([`jumps_sent`]); one it doesn't send, as on a tab whose file is being looked at, waits for the next look.
    pub fn take_in_at_the_look(self, session: &mut recto_session::Session<DrawableLoader>, synced: &mut Synced, events: &mut Vec<Event>) -> Vec<(TabId, Stop)> {
        let jumps: Vec<(TabId, Stop)> = session.panes().into_iter().filter_map(|id| Some((id, session.waiting_jump(id)?))).collect();
        for (pane, page) in self.take_in(session, &given_to_views(synced), &jumps, events) {
            if let Some(pane) = synced.panes.get_mut(pane) {
                pane.page = page;
            }
        }
        jumps
    }
}

/// **The jumps the look's page views were really sent,** each with the place clock its send came to, said to the session under a lock of its own, as the
/// sends are made with the session let go: each is where the reader is until a read at or past its send is taken in.
pub fn jumps_sent(inner: &Inner, sent: &[(TabId, Stop, u64)]) {
    if sent.is_empty() {
        return;
    }
    inner.host.session.with_session(|session| {
        for (id, to, stamp) in sent {
            session.jump_sent(*id, *to, *stamp);
        }
    });
}

/// The jump waiting on a tab, for the look to send, given to its pane once.
pub fn jump_for(jumps: &mut Vec<(TabId, Stop)>, id: TabId) -> Option<Stop> {
    jumps.iter().position(|(tab, _)| *tab == id).map(|at| jumps.swap_remove(at).1)
}

/// What each pane's page view has been given: its tab and page, as the session thread's last look left them.
pub fn given_to_views(synced: &Synced) -> Vec<Option<ViewGiven>> {
    synced.panes.iter().map(|pane| pane.tab.map(|tab| ViewGiven { tab, page: pane.page })).collect()
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
    /// jumps it's about to send. A jump on the page already showing, waiting or sent by a look and not yet told of, isn't seen from here; the mark doesn't
    /// need it to be, as the session answers where the reader is from that jump itself (`Session::here`), and a look can send it between this and the step.
    ///
    /// **A read older than what the session has is skipped,** place by place, by its stamp ([`Said::take_in`]): the views are read with no lock on the
    /// session, and a look, another command or the saver can take in a newer read between this one and its take-in.
    pub fn take_places(&self) {
        let said = Said::read(self, false);
        #[cfg(any(test, feature = "testkit"))]
        self.serving.place_seams.command_read();
        let given = self.panes.given();
        self.host.session.with_session(|session| {
            said.take_in(session, &given, &[], &mut Vec::new());
        });
        #[cfg(any(test, feature = "testkit"))]
        self.serving.place_seams.command_taken();
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
        let given = Some(ViewGiven { tab, page: 9 });
        assert!(is_views_own(given, tab, 9, false), "sent everything: the reader's own");
        assert!(!is_views_own(given, tab, 0, false), "the session went back to page 1 since");
        assert!(!is_views_own(given, tab, 9, true), "a jump on the same page waits to be sent");
        assert!(
            !is_views_own(Some(ViewGiven { tab: other, page: 9 }), tab, 9, false),
            "the view was sent another tab"
        );
        assert!(!is_views_own(None, tab, 9, false), "the view was sent nothing yet");
    }
}
