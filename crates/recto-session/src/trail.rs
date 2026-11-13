//! **Back and forward,** a capped trail of places per tab, as a browser keeps one.
//!
//! **An agent's marks are its own** (coordinator's decision, 2026-11-08, as with undo, SPEC decision 20): a stop an agent's jump left can't push the person's own stops out past the cap, and an agent's jump leaves the person's way forward where it was. A jump leaves a mark: following a link, choosing a contents entry, going to a page by number or label, as a person or an agent. Scrolling doesn't, as it's reading, not going somewhere. Back goes to the last mark and puts where the reader was on the way forward; a new jump clears the way forward, as on any Mac. At most [`MAX_TRAIL`] each way, the oldest going first, so a tab's trail costs at most a few hundred bytes, and a tab nobody's jumped in costs nothing. It's in the state record, so it comes back after a quit and across an update.

use serde::{Deserialize, Serialize};

/// Most places kept each way.
pub const MAX_TRAIL: usize = 50;

/// A place in a tab: a page, zero-based, and how far down it, from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    pub page: u32,
    pub within: f32,
    /// An agent's jump left it, so it gives way to the person's at the cap.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub by_agent: bool,
}

impl Stop {
    pub fn new(page: u32, within: f64) -> Self {
        let within = if within.is_finite() { within.clamp(0.0, 1.0) as f32 } else { 0.0 };
        Self { page, within, by_agent: false }
    }

    /// The same place, marked as left by an agent's jump or the person's.
    #[must_use]
    pub fn by(self, agent: bool) -> Self {
        Self { by_agent: agent, ..self }
    }

    /// Close enough to be the same place: the same page and within a few lines.
    fn same_place(self, other: Self) -> bool {
        self.page == other.page && (self.within - other.within).abs() < 0.02
    }
}

/// One tab's trail.
// size: one Vec each way, held only for a tab someone has jumped in. At most `MAX_TRAIL` stops each way, 8 B a stop.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trail {
    back: Vec<Stop>,
    forward: Vec<Stop>,
}

impl Trail {
    /// From what a record kept, which can't be believed: held to the cap, and to the pages there are.
    pub fn from_record(back: &[Stop], forward: &[Stop], pages: u32) -> Self {
        let keep = |stops: &[Stop]| -> Vec<Stop> {
            let start = stops.len().saturating_sub(MAX_TRAIL);
            stops[start..]
                .iter()
                .map(|stop| Stop::new(stop.page.min(pages.saturating_sub(1)), f64::from(stop.within)).by(stop.by_agent))
                .collect()
        };
        Self {
            back: keep(back),
            forward: keep(forward),
        }
    }

    pub fn back(&self) -> &[Stop] {
        &self.back
    }

    pub fn forward(&self) -> &[Stop] {
        &self.forward
    }

    pub fn is_empty(&self) -> bool {
        self.back.is_empty() && self.forward.is_empty()
    }

    /// A jump from `here`: it's marked to come back to, and the way forward goes, all of it for the person's jump and only an agent's own stops for an agent's. A jump to where the reader already is marks nothing.
    pub fn jumped(&mut self, here: Stop, to: Stop, by_agent: bool) {
        if here.same_place(to) {
            return;
        }
        if !self.back.last().is_some_and(|last| last.same_place(here)) {
            push_capped(&mut self.back, here.by(by_agent));
        }
        if by_agent {
            self.forward.retain(|stop| !stop.by_agent);
        } else {
            self.forward.clear();
        }
    }

    /// Back from `here`, if there's somewhere to go: where to, with `here` kept for going forward again, marked by who went.
    pub fn go_back(&mut self, here: Stop, by_agent: bool) -> Option<Stop> {
        let to = self.back.pop()?;
        push_capped(&mut self.forward, here.by(by_agent));
        Some(to)
    }

    /// Forward from `here`, the same way.
    pub fn go_forward(&mut self, here: Stop, by_agent: bool) -> Option<Stop> {
        let to = self.forward.pop()?;
        push_capped(&mut self.back, here.by(by_agent));
        Some(to)
    }

    /// **The trail after its tab's pages were edited:** each stop goes where its page went, and a stop whose page went with the edit goes too.
    pub fn follow_pages(&mut self, moved: impl Fn(u32) -> Option<u32>) {
        for list in [&mut self.back, &mut self.forward] {
            list.retain_mut(|stop| match moved(stop.page) {
                Some(page) => {
                    stop.page = page;
                    true
                }
                None => false,
            });
        }
    }

    /// What it holds on the heap, for a cold tab's cost.
    pub fn heap_bytes(&self) -> usize {
        (self.back.capacity() + self.forward.capacity()) * std::mem::size_of::<Stop>()
    }

    /// Given back what it doesn't use, as its tab goes cold.
    pub fn shrink(&mut self) {
        self.back.shrink_to_fit();
        self.forward.shrink_to_fit();
    }
}

/// Adds a stop, and past the cap lets the oldest go: the oldest of an agent's for an agent's stop, and if there's none, the agent's own new one, so an agent can never push out the person's.
fn push_capped(list: &mut Vec<Stop>, stop: Stop) {
    list.push(stop);
    if list.len() <= MAX_TRAIL {
        return;
    }
    let oldest = if stop.by_agent { list.iter().position(|kept| kept.by_agent) } else { Some(0) };
    list.remove(oldest.unwrap_or(list.len() - 1));
}

use super::*;

impl<L: DocLoader> Session<L> {
    /// Moves a tab to a zero-based page, clamped to the document. A jump its page view was sent is about the page it's left.
    pub fn go_to(&mut self, id: TabId, page: u32) -> Result<(), SessionError> {
        let pages = self.pages().count(id);
        let tab = self.tabs.tab_mut(id).ok_or(SessionError::NoSuchTab)?;
        let page = page.min(pages.saturating_sub(1));
        if tab.page != page {
            tab.page = page;
            tab.place_dirty = true;
            self.mail.forget_jump_on_its_way(id);
        }
        Ok(())
    }

    /// **Where the reader is in a tab,** for a jump to mark, while the tab is still on the page it went to: a jump the page view hasn't taken yet, then
    /// one it's been sent that no read of the view taken in since has told of, as each is where the session has sent the reader; or else the page, and how
    /// far down it the page view said in the newest read taken in. A read from before such a jump is about where the view was before it, and on the same page
    /// would mark the wrong place down it: Back straight after a jump on the page showing kept the old place for Forward, and so did Back whose read of the
    /// views was taken in after the session thread's look sent the jump, the read having been made before the send. Which read is newer, and whether it came
    /// after a send, goes by the place clock ([`Self::view_told_of`], [`Self::jump_sent`]): the newer read wins, whoever took it in.
    pub fn here(&self, id: TabId) -> Option<trail::Stop> {
        let tab = self.tabs.tab(id)?;
        // Only while it's still where the tab is: a move since that sent the page view no jump of its own, as Find's step, is the newer.
        let on_page = |stop: &trail::Stop| stop.page == tab.page;
        if let Some(jump) = self
            .mail
            .waiting_jump(id)
            .filter(on_page)
            .or_else(|| self.mail.jump_on_its_way(id).filter(on_page))
        {
            return Some(trail::Stop::new(jump.page, f64::from(jump.within)));
        }
        let within = tab
            .reading
            .place
            .filter(|place| place.page == tab.page && place.anchor_page == tab.page)
            .map_or(0.0, |place| place.within);
        Some(trail::Stop::new(tab.page, within))
    }

    /// **A jump:** following a link, a contents entry, or going to a page by its number or label. Where the reader was is marked for Back, and the way forward goes. The page is held to the document, and the page view lands `within` the way down it.
    pub fn jump(&mut self, id: TabId, page: u32, within: f64) -> Result<(), SessionError> {
        self.jump_as(id, page, within, Author::Owner)
    }

    /// A jump, by the person or an agent. An agent's marks give way to the person's: see [`trail`].
    pub fn jump_as(&mut self, id: TabId, page: u32, within: f64, by: Author) -> Result<(), SessionError> {
        let here = self.here(id).ok_or(SessionError::NoSuchTab)?;
        let to = trail::Stop::new(page.min(self.pages().count(id).saturating_sub(1)), within);
        let tab = self.tabs.tab_mut(id).ok_or(SessionError::NoSuchTab)?;
        tab.reading.trail.get_or_insert_default().jumped(here, to, by == Author::Agent);
        self.land(id, to)
    }

    /// Back (`back`) or forward in a tab's trail, by the person or an agent. Says whether there was anywhere to go.
    pub fn step_as(&mut self, id: TabId, back: bool, by: Author) -> Result<bool, SessionError> {
        let here = self.here(id).ok_or(SessionError::NoSuchTab)?;
        let agent = by == Author::Agent;
        let Some(trail) = self.tabs.tab_mut(id).and_then(|tab| tab.reading.trail.as_deref_mut()) else {
            return Ok(false);
        };
        let Some(to) = (if back { trail.go_back(here, agent) } else { trail.go_forward(here, agent) }) else {
            return Ok(false);
        };
        // Pages may have gone since it was marked.
        let to = trail::Stop {
            page: to.page.min(self.pages().count(id).saturating_sub(1)),
            ..to
        };
        self.land(id, to)?;
        Ok(true)
    }

    fn land(&mut self, id: TabId, to: trail::Stop) -> Result<(), SessionError> {
        self.go_to(id, to.page)?;
        self.mail.jump(id, to);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Failure: back doesn't return where the reader was, forward is lost before a new jump clears it, or a jump to the same place adds a step that back has to go through for nothing.
    #[test]
    fn back_and_forward_go_as_a_browser_does() {
        let mut trail = Trail::default();
        let at = |page| Stop::new(page, 0.25);
        trail.jumped(at(0), at(8), false);
        trail.jumped(at(8), at(3), false);
        trail.jumped(at(3), at(3), false);
        assert_eq!(trail.back(), [at(0), at(8)]);
        assert_eq!(trail.go_back(at(3), false), Some(at(8)));
        assert_eq!(trail.go_back(at(8), false), Some(at(0)));
        assert_eq!(trail.go_back(at(0), false), None);
        assert_eq!(trail.go_forward(at(0), false), Some(at(8)));
        trail.jumped(at(8), at(5), false);
        assert_eq!(trail.forward(), [], "a new jump clears the way forward");
        assert_eq!(trail.back(), [at(0), at(8)]);
    }

    /// Failure: a long session of links grows a tab's trail without end, or a record names pages past the end or more stops than the cap.
    #[test]
    fn the_trail_is_capped_both_ways() {
        let mut trail = Trail::default();
        for page in 0..(MAX_TRAIL as u32 * 3) {
            trail.jumped(Stop::new(page, 0.0), Stop::new(page + 1, 0.0), false);
        }
        assert_eq!(trail.back().len(), MAX_TRAIL);
        assert_eq!(trail.back()[0].page, MAX_TRAIL as u32 * 2, "the oldest went first");
        let many: Vec<Stop> = (0..500)
            .map(|page| Stop {
                page,
                within: f32::NAN,
                by_agent: false,
            })
            .collect();
        let kept = Trail::from_record(&many, &many, 10);
        assert_eq!(kept.back().len(), MAX_TRAIL);
        assert!(kept.back().iter().chain(kept.forward()).all(|stop| stop.page < 10 && stop.within == 0.0));
    }

    /// Failure: an agent jumping about pushes the person's own places out of the trail, or clears the way forward they had, which the undo rule doesn't let an agent do to their edits either.
    #[test]
    fn an_agents_jumps_never_push_out_the_persons_stops() {
        let mut trail = Trail::default();
        for page in 0..(MAX_TRAIL as u32) {
            trail.jumped(Stop::new(page, 0.0), Stop::new(page + 1, 0.0), false);
        }
        assert_eq!(
            trail.go_back(Stop::new(MAX_TRAIL as u32, 0.0), false).map(|stop| stop.page),
            Some(MAX_TRAIL as u32 - 1)
        );
        trail.jumped(Stop::new(MAX_TRAIL as u32 - 1, 0.0), Stop::new(900, 0.0), false);
        assert_eq!(trail.forward().len(), 0);
        trail.go_back(Stop::new(900, 0.0), false);
        let persons = trail.back().to_vec();
        for page in 1000..1100 {
            trail.jumped(Stop::new(page, 0.0), Stop::new(page + 1, 0.0), true);
        }
        assert_eq!(trail.back().len(), MAX_TRAIL);
        assert!(
            persons.iter().all(|stop| trail.back().contains(stop)),
            "every stop of the person's is still there"
        );
        assert_eq!(trail.forward().iter().filter(|stop| !stop.by_agent).count(), 1, "and their way forward");
        // The person's own jump goes on as before: theirs can push out anything.
        trail.jumped(Stop::new(5, 0.0), Stop::new(6, 0.0), false);
        assert!(trail.forward().is_empty());
    }

    /// Failure: a stop keeps naming a page that moved, or a deleted one, after an edit.
    #[test]
    fn stops_follow_their_pages_through_an_edit() {
        let mut trail = Trail::default();
        trail.jumped(Stop::new(2, 0.5), Stop::new(9, 0.0), false);
        trail.jumped(Stop::new(9, 0.0), Stop::new(4, 0.0), false);
        trail.follow_pages(|page| match page {
            2 => Some(0),
            9 => None,
            other => Some(other),
        });
        assert_eq!(trail.back(), [Stop::new(0, 0.5)]);
    }
}
